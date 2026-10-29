//! The shared folder tree of the sidebar (LAY-03 a): the places as its roots, their folders
//! below them, listed the first time a node opens, and the folder of the active pane found
//! and selected.
//!
//! Everything here is plain data on the UI thread, like the panes' state ([`crate::pane`]). A
//! node asks for its child folders with a [`LoadRequest`], which the caller hands to
//! [`Loader::list_folders`](crate::loader::Loader::list_folders), and takes the listing back
//! through [`Tree::apply_children`] (or [`Tree::listing_failed`]). Every request has a new
//! generation, and a listing that no node waits for any more is dropped.
//!
//! A node is listed the first time it opens and keeps its children, hidden ones too; the rows
//! show them as the panes' [`VisibilityFilter`] says. A listing that failed leaves the node
//! without a chevron, and the next opening or reveal through it tries again. The children
//! follow the folder afterwards: a pane's listing of the same folder reconciles them
//! ([`Tree::sync_folder`]), and a folder a job changed is listed again
//! ([`Tree::relist_folder`]). A reconciled node keeps the nodes of the children that are still
//! there, with what is open below them.
//!
//! [`Tree::reveal`] follows the folder of the active pane: it opens only the folders on the
//! way to it, below the deepest root that holds it, and lists each of them in turn as the
//! listing above it comes, so a far folder costs one listing per level and nothing else. A
//! folder on the way that its parent's listing lacks (made since) has the parent listed again,
//! once per reveal.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::sync::atomic::{AtomicU64, Ordering};

use rf_core::{Attributes, Entry, EntryKind, VPath};
use rf_vfs::{SortSpec, VisibilityFilter, sort_entries};

use crate::loader::LoadRequest;
use crate::pane::next_generation;

/// A node's place in its [`Tree`]; it stays the same until [`Tree::set_roots`] starts a new
/// tree.
pub type NodeId = usize;

/// One folder of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeNode {
    pub path: VPath,
    /// The name the row shows: the place's for a root, the folder's own below.
    pub label: String,
    /// 0 for a root, one more for each level below it.
    pub depth: usize,
    /// The node one level up; `None` for a root.
    pub parent: Option<NodeId>,
    /// The node shows its children (once they are listed).
    pub expanded: bool,
    /// The child folders in natural order, once listed; `None` before, and after a first
    /// listing that failed.
    pub children: Option<Vec<NodeId>>,
    /// A listing of the children is on its way.
    pub loading: bool,
    /// The last listing failed: with no children known, the row shows no chevron.
    pub failed: bool,
    /// The folder's attributes (hidden, system): whether its row shows follows the panes'
    /// [`VisibilityFilter`]. Empty for a root.
    pub attributes: Attributes,
    /// For a root, its position among the roots given to [`Tree::set_roots`].
    pub root: Option<usize>,
}

/// One row of the tree as the sidebar shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a separate fact the row shows, not states of one machine"
)]
pub struct TreeRowData {
    pub id: NodeId,
    pub label: String,
    pub depth: usize,
    pub expanded: bool,
    /// The node has not been listed yet: it may have child folders, so it shows a chevron.
    pub has_children_unknown: bool,
    /// The node has child folders that show.
    pub has_children: bool,
    /// The node is the folder of the active pane.
    pub selected: bool,
    /// For a root, its position among the roots.
    pub root: Option<usize>,
}

/// The folder tree: its nodes, which of them show, and the folder it follows.
///
/// The fields change only through the methods, which keep `visible` (the rows) up to date
/// and give every change of the rows a new `revision`, one no other tree has had either.
#[derive(Debug)]
pub struct Tree {
    /// Every node made since the tree started; a node a reconciliation dropped stays here,
    /// unreachable.
    nodes: Vec<TreeNode>,
    roots: Vec<NodeId>,
    filter: VisibilityFilter,
    /// The folder the tree follows: the one [`Tree::reveal`] or [`Tree::select`] was last
    /// given.
    target: Option<VPath>,
    /// The node of `target`, once found.
    selected: Option<NodeId>,
    /// The node whose listing the reveal of `target` waits for, to go on below it.
    waiting: Option<NodeId>,
    /// The nodes the current reveal has listed again because the next folder was missing.
    relisted: HashSet<NodeId>,
    /// The listings on their way, by generation.
    loads: HashMap<u64, NodeId>,
    /// The nodes the rows show, in order.
    visible: Vec<NodeId>,
    /// Changes with every change of the rows ([`next_revision`]).
    revision: u64,
}

impl Tree {
    /// An empty tree whose rows show what `filter` shows.
    pub fn new(filter: VisibilityFilter) -> Tree {
        Tree {
            nodes: Vec::new(),
            roots: Vec::new(),
            filter,
            target: None,
            selected: None,
            waiting: None,
            relisted: HashSet::new(),
            loads: HashMap::new(),
            visible: Vec::new(),
            revision: next_revision(),
        }
    }

    /// Starts a new tree whose roots are `roots` (a label and a folder each), all closed, and
    /// looks for the folder it follows again. The listings still on their way are dropped
    /// when they come.
    pub fn set_roots(
        &mut self,
        roots: impl IntoIterator<Item = (String, VPath)>,
    ) -> Vec<LoadRequest> {
        self.nodes.clear();
        self.loads.clear();
        self.relisted.clear();
        self.roots.clear();
        for (index, (label, path)) in roots.into_iter().enumerate() {
            self.roots.push(self.nodes.len());
            self.nodes.push(TreeNode {
                root: Some(index),
                ..TreeNode::new(path, label, 0, None, Attributes::empty())
            });
        }
        // The rows first: the folder may show already.
        self.refresh();
        let requests = self.reveal_target();
        self.refresh();
        requests
    }

    /// Node `id`; `None` for one past the end.
    pub fn node(&self, id: NodeId) -> Option<&TreeNode> {
        self.nodes.get(id)
    }

    /// The rows: the roots, and below each open node its children that show, in order.
    pub fn rows(&self) -> Vec<TreeRowData> {
        self.visible
            .iter()
            .map(|&id| {
                let node = &self.nodes[id];
                TreeRowData {
                    id,
                    label: node.label.clone(),
                    depth: node.depth,
                    expanded: node.expanded,
                    has_children_unknown: node.children.is_none() && !node.failed,
                    has_children: node
                        .children
                        .as_ref()
                        .is_some_and(|children| children.iter().any(|&child| self.shows(child))),
                    selected: self.selected == Some(id),
                    root: node.root,
                }
            })
            .collect()
    }

    /// The node of row `row`; `None` for a row past the end.
    pub fn node_at(&self, row: usize) -> Option<NodeId> {
        self.visible.get(row).copied()
    }

    /// The node of the folder the tree follows, once found.
    pub fn selected(&self) -> Option<NodeId> {
        self.selected
    }

    /// The row of the selected node; `None` when there is none or it does not show (a node
    /// above it is closed, or it is hidden).
    pub fn selected_row(&self) -> Option<usize> {
        let selected = self.selected?;
        self.visible.iter().position(|&id| id == selected)
    }

    /// The folder the tree follows: the one [`Tree::reveal`] or [`Tree::select`] was last
    /// given.
    pub fn target(&self) -> Option<&VPath> {
        self.target.as_ref()
    }

    /// A number that changes whenever the rows may have, and that no other tree has had: a
    /// model of the rows can skip a redraw while it stays the same, even when it is given
    /// another tree (the picker's, made anew each time the picker opens).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The generations of the listings on their way.
    pub fn generations(&self) -> impl Iterator<Item = u64> + '_ {
        self.loads.keys().copied()
    }

    /// Opens node `id`, or closes it. Opening a node that has not been listed (or whose
    /// listing failed) asks for its listing; a listed node, or one whose listing is on its
    /// way, asks for nothing. Closing a node on the way to the folder the tree follows stops
    /// looking for that folder there. `None` too for a node past the end.
    pub fn toggle(&mut self, id: NodeId) -> Option<LoadRequest> {
        let node = self.nodes.get_mut(id)?;
        // A node whose listing failed shows closed, whatever its flag says.
        let opening = !node.expanded || (node.children.is_none() && !node.loading);
        node.expanded = opening;
        let request = if opening {
            self.request(id)
        } else {
            if self
                .waiting
                .is_some_and(|waiting| within(&self.nodes, waiting, id))
            {
                self.waiting = None;
            }
            None
        };
        self.refresh();
        request
    }

    /// Takes the listing of request `generation`, in the natural order the worker sorted it
    /// in: its folders become the node's children, reconciled with the ones it had (as
    /// [`Tree::sync_folder`] does), and files and links are left out. If the folder the tree
    /// follows waited for this listing, the tree goes on toward it and returns the listing
    /// that needs next. A listing that no node waits for any more is dropped.
    pub fn apply_children(&mut self, generation: u64, entries: Vec<Entry>) -> Vec<LoadRequest> {
        let Some(id) = self.loads.remove(&generation) else {
            return Vec::new();
        };
        let folders: Vec<Entry> = entries
            .into_iter()
            .filter(|entry| entry.kind == EntryKind::Dir)
            .collect();
        let node = &mut self.nodes[id];
        node.loading = false;
        node.failed = false;
        self.set_children(id, folders);
        let requests = if self.waiting == Some(id) {
            self.waiting = None;
            self.reveal_from(id)
        } else {
            Vec::new()
        };
        self.refresh();
        requests
    }

    /// The listing of request `generation` failed. A node never listed keeps no children
    /// and shows no chevron, until the next opening or reveal through it tries again; a node
    /// listed before keeps the children it had. `false`, and no change, for a listing that no
    /// node waits for any more.
    pub fn listing_failed(&mut self, generation: u64) -> bool {
        let Some(id) = self.loads.remove(&generation) else {
            return false;
        };
        let node = &mut self.nodes[id];
        node.loading = false;
        node.failed = true;
        if self.waiting == Some(id) {
            self.waiting = None;
        }
        self.refresh();
        true
    }

    /// Takes `entries`, the listing of the folder `path` that a pane shows (in its own order,
    /// files and hidden items too): every node of that folder that has been listed and has no
    /// listing on its way (a place and a folder below another place can both be it) gets its
    /// child folders from it. A child still there keeps its node, with what is open below it
    /// and its attributes brought up to date; a new one comes in natural order; one that went
    /// takes along the selection, the reveal's wait and the listings inside it. `true` when
    /// the children changed; when the folders and their attributes are the same, nothing
    /// changes, not even the revision, since a watch reports often while a copy runs.
    pub fn sync_folder(&mut self, path: &VPath, entries: &[Entry]) -> bool {
        let ids: Vec<NodeId> = self
            .reachable()
            .into_iter()
            .filter(|&id| {
                let node = &self.nodes[id];
                node.path == *path && node.children.is_some() && !node.loading
            })
            .collect();
        if ids.is_empty() {
            return false;
        }
        let mut folders: Vec<Entry> = entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::Dir)
            .cloned()
            .collect();
        sort_entries(&mut folders, &SortSpec::default());
        let mut changed = false;
        for id in ids {
            changed |= self.set_children(id, folders.clone());
        }
        if changed {
            self.refresh();
        }
        changed
    }

    /// The folder `path` changed through a job: every node of it that has been listed, with
    /// no listing on its way, is listed again, and [`Tree::apply_children`] reconciles its
    /// children when the listing comes. The requests for those listings.
    pub fn relist_folder(&mut self, path: &VPath) -> Vec<LoadRequest> {
        let ids: Vec<NodeId> = self
            .reachable()
            .into_iter()
            .filter(|&id| {
                let node = &self.nodes[id];
                node.path == *path && node.children.is_some() && !node.loading
            })
            .collect();
        ids.into_iter().map(|id| self.list(id)).collect()
    }

    /// Follows `path`, the folder of the active pane, and selects its node. When a row shows
    /// it already, that row is the one (the first, when several do) and nothing opens.
    /// Otherwise the tree opens the folders on the way to it below the deepest root that holds
    /// it: a folder on the way that has not been listed is listed (the returned request), and
    /// the tree goes on when the listing comes ([`Tree::apply_children`]). A folder that its
    /// parent's listing lacks has the parent listed again, once per reveal, in case it was
    /// made since; still missing, or hidden while hidden folders do not show, it ends the way,
    /// with nothing selected. A path under no root opens nothing and selects nothing.
    pub fn reveal(&mut self, path: &VPath) -> Vec<LoadRequest> {
        self.target = Some(path.clone());
        self.relisted.clear();
        let requests = self.reveal_target();
        self.refresh();
        requests
    }

    /// Selects node `id`, whose row was clicked to show its folder in the active pane, and
    /// follows that folder from there: another row that shows the same folder does not take
    /// the selection. `false`, and no change, for a node past the end.
    pub fn select(&mut self, id: NodeId) -> bool {
        let Some(node) = self.nodes.get(id) else {
            return false;
        };
        self.target = Some(node.path.clone());
        self.selected = Some(id);
        self.waiting = None;
        self.refresh();
        true
    }

    /// The folders listed so far whose names hold `query` (its spaces at the ends left out),
    /// ignoring letter case: what a filter of the tree shows, flat. They come in the order of
    /// the tree, below closed nodes too, and each folder once, at the first node that has it.
    /// A folder out of sight (hidden while hidden folders do not show, or below such a one)
    /// is left out, and so is one never listed. An empty `query` finds nothing.
    pub fn find(&self, query: &str) -> Vec<NodeId> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let mut found = Vec::new();
        let mut paths: HashSet<&VPath> = HashSet::new();
        let mut stack: Vec<NodeId> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let node = &self.nodes[id];
            if node.label.to_lowercase().contains(&query) && paths.insert(&node.path) {
                found.push(id);
            }
            if let Some(children) = &node.children {
                stack.extend(
                    children
                        .iter()
                        .rev()
                        .copied()
                        .filter(|&child| self.shows(child)),
                );
            }
        }
        found
    }

    /// Shows the hidden folders, or hides them. When the folder the tree follows was not found
    /// (it may be below a hidden one), the tree looks for it again.
    pub fn set_show_hidden(&mut self, show: bool) -> Vec<LoadRequest> {
        if self.filter.show_hidden == show {
            return Vec::new();
        }
        self.filter.show_hidden = show;
        self.refresh();
        let requests = if self.selected.is_none() && self.waiting.is_none() {
            self.reveal_target()
        } else {
            Vec::new()
        };
        self.refresh();
        requests
    }

    /// Whether node `id` shows when its parent is open: the panes' rule for hidden and
    /// system items.
    fn shows(&self, id: NodeId) -> bool {
        let mut probe = Entry::new(OsString::new(), EntryKind::Dir);
        probe.attributes = self.nodes[id].attributes;
        self.filter.is_visible(&probe)
    }

    /// The request for the first listing of node `id`; `None` when it is listed or its
    /// listing is on its way.
    fn request(&mut self, id: NodeId) -> Option<LoadRequest> {
        let node = &self.nodes[id];
        if node.loading || node.children.is_some() {
            return None;
        }
        Some(self.list(id))
    }

    /// A request for a listing of node `id`, which is on its way from now on.
    fn list(&mut self, id: NodeId) -> LoadRequest {
        let node = &mut self.nodes[id];
        node.loading = true;
        node.failed = false;
        let generation = next_generation();
        self.loads.insert(generation, id);
        LoadRequest {
            generation,
            path: node.path.clone(),
            sort: SortSpec::default(),
        }
    }

    /// Makes `folders` (in natural order) the children of node `id`. A child of the same name
    /// keeps its node, with its attributes brought up to date; the children that are gone are
    /// forgotten. `true` when the children or their attributes changed.
    fn set_children(&mut self, id: NodeId, folders: Vec<Entry>) -> bool {
        let previous = self.nodes[id].children.take();
        let listed_before = previous.is_some();
        let old = previous.unwrap_or_default();
        let same = listed_before
            && old.len() == folders.len()
            && old.iter().zip(&folders).all(|(&child, entry)| {
                let node = &self.nodes[child];
                node.path.file_name() == Some(entry.name.as_os_str())
                    && node.attributes == entry.attributes
            });
        if same {
            self.nodes[id].children = Some(old);
            return false;
        }
        let mut by_name: HashMap<OsString, NodeId> = old
            .iter()
            .filter_map(|&child| Some((self.nodes[child].path.file_name()?.to_owned(), child)))
            .collect();
        let (parent, depth) = (self.nodes[id].path.clone(), self.nodes[id].depth + 1);
        let mut children = Vec::with_capacity(folders.len());
        for entry in folders {
            if let Some(child) = by_name.remove(&entry.name) {
                self.nodes[child].attributes = entry.attributes;
                children.push(child);
                continue;
            }
            let Ok(path) = parent.join(&entry.name) else {
                continue;
            };
            let label = entry.name.to_string_lossy().into_owned();
            children.push(self.nodes.len());
            self.nodes.push(TreeNode::new(
                path,
                label,
                depth,
                Some(id),
                entry.attributes,
            ));
        }
        for gone in by_name.into_values() {
            self.forget(gone);
        }
        self.nodes[id].children = Some(children);
        true
    }

    /// Lets go of node `gone` and everything below it, which a listing no longer has: the
    /// selection and the reveal's wait inside it end, its listings on their way are dropped,
    /// and its nodes no longer count as listed.
    fn forget(&mut self, gone: NodeId) {
        if self
            .selected
            .is_some_and(|id| within(&self.nodes, id, gone))
        {
            self.selected = None;
        }
        if self.waiting.is_some_and(|id| within(&self.nodes, id, gone)) {
            self.waiting = None;
        }
        let nodes = &self.nodes;
        self.loads.retain(|_, &mut id| !within(nodes, id, gone));
        let mut stack = vec![gone];
        while let Some(id) = stack.pop() {
            let node = &mut self.nodes[id];
            node.loading = false;
            node.expanded = false;
            stack.extend(node.children.take().unwrap_or_default());
        }
    }

    /// Selects the row that shows the target, or looks for it from the deepest root that
    /// holds it. The rows must be up to date.
    fn reveal_target(&mut self) -> Vec<LoadRequest> {
        self.selected = None;
        self.waiting = None;
        let Some(target) = &self.target else {
            return Vec::new();
        };
        if let Some(&shown) = self
            .visible
            .iter()
            .find(|&&id| self.nodes[id].path == *target)
        {
            self.selected = Some(shown);
            return Vec::new();
        }
        match self.root_of(target) {
            Some(root) => self.reveal_from(root),
            None => Vec::new(),
        }
    }

    /// Goes on toward the target from node `id`, which is on its way: opens each folder above
    /// the target until it reaches the target's node (selected), a folder that is not listed
    /// yet (listed, and `waiting` for), or one without the next folder of the way (listed
    /// again once per reveal, and `waiting` for, or the end of the way).
    fn reveal_from(&mut self, mut id: NodeId) -> Vec<LoadRequest> {
        let Some(target) = self.target.clone() else {
            return Vec::new();
        };
        loop {
            if self.nodes[id].path == target {
                self.selected = Some(id);
                return Vec::new();
            }
            self.nodes[id].expanded = true;
            let Some(children) = &self.nodes[id].children else {
                self.waiting = Some(id);
                return self.request(id).into_iter().collect();
            };
            let next = children
                .iter()
                .copied()
                .find(|&child| target.starts_with(&self.nodes[child].path));
            match next {
                Some(child) if self.shows(child) => id = child,
                // Missing from the listing: it may be new, so the folder is listed again, once
                // per reveal.
                None if self.relisted.insert(id) => {
                    self.waiting = Some(id);
                    if self.nodes[id].loading {
                        return Vec::new();
                    }
                    return vec![self.list(id)];
                }
                // There but out of sight, or still missing.
                Some(_) | None => return Vec::new(),
            }
        }
    }

    /// The deepest root that holds `path`; the first of them when two are the same folder.
    fn root_of(&self, path: &VPath) -> Option<NodeId> {
        self.roots
            .iter()
            .copied()
            .filter(|&root| path.starts_with(&self.nodes[root].path))
            .reduce(|best, root| {
                let (best_path, root_path) = (&self.nodes[best].path, &self.nodes[root].path);
                if root_path != best_path && root_path.starts_with(best_path) {
                    root
                } else {
                    best
                }
            })
    }

    /// Every node in the tree: the roots and, below each listed node, all its children (open
    /// or not, shown or not).
    fn reachable(&self) -> Vec<NodeId> {
        let mut found = Vec::new();
        let mut stack: Vec<NodeId> = self.roots.clone();
        while let Some(id) = stack.pop() {
            found.push(id);
            if let Some(children) = &self.nodes[id].children {
                stack.extend(children);
            }
        }
        found
    }

    /// Lists the rows again after a change.
    fn refresh(&mut self) {
        let mut visible = Vec::with_capacity(self.visible.len());
        let mut stack: Vec<NodeId> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            visible.push(id);
            let node = &self.nodes[id];
            if node.expanded
                && let Some(children) = &node.children
            {
                stack.extend(
                    children
                        .iter()
                        .rev()
                        .copied()
                        .filter(|&child| self.shows(child)),
                );
            }
        }
        self.visible = visible;
        self.revision = next_revision();
    }
}

/// A revision no tree has had yet.
fn next_revision() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl TreeNode {
    /// A closed node, not listed yet.
    fn new(
        path: VPath,
        label: String,
        depth: usize,
        parent: Option<NodeId>,
        attributes: Attributes,
    ) -> TreeNode {
        TreeNode {
            path,
            label,
            depth,
            parent,
            expanded: false,
            children: None,
            loading: false,
            failed: false,
            attributes,
            root: None,
        }
    }
}

/// Whether node `id` is node `ancestor` or lies below it.
fn within(nodes: &[TreeNode], id: NodeId, ancestor: NodeId) -> bool {
    let mut current = Some(id);
    while let Some(node) = current {
        if node == ancestor {
            return true;
        }
        current = nodes[node].parent;
    }
    false
}

#[cfg(test)]
mod tests {
    use rf_core::{Attributes, EntryKind};

    use super::*;
    use crate::pane::ancestors;

    fn folder(name: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(name)).expect("a local path")
    }

    /// `base` with `names` below it.
    fn under(base: &VPath, names: &[&str]) -> VPath {
        names.iter().fold(base.clone(), |path, name| {
            path.join(name).expect("a plain name")
        })
    }

    fn dir(name: &str) -> Entry {
        Entry::new(name, EntryKind::Dir)
    }

    fn hidden(mut entry: Entry) -> Entry {
        entry.attributes |= Attributes::HIDDEN;
        entry
    }

    fn file(name: &str) -> Entry {
        Entry::new(name, EntryKind::File)
    }

    fn roots(list: &[(&str, &VPath)]) -> Vec<(String, VPath)> {
        list.iter()
            .map(|&(label, path)| (label.to_owned(), path.clone()))
            .collect()
    }

    /// The rows as `(depth, label)`, the selected one marked with `*`.
    fn shown(tree: &Tree) -> Vec<(usize, String)> {
        tree.rows()
            .into_iter()
            .map(|row| {
                let mark = if row.selected { "*" } else { "" };
                (row.depth, format!("{mark}{}", row.label))
            })
            .collect()
    }

    fn rows(list: &[(usize, &str)]) -> Vec<(usize, String)> {
        list.iter()
            .map(|&(depth, label)| (depth, label.to_owned()))
            .collect()
    }

    /// The one request among `requests`.
    fn the_request(requests: Vec<LoadRequest>) -> LoadRequest {
        match <[LoadRequest; 1]>::try_from(requests) {
            Ok([request]) => request,
            Err(other) => panic!("expected one request, got {other:?}"),
        }
    }

    fn row(tree: &Tree, index: usize) -> TreeRowData {
        tree.rows().into_iter().nth(index).expect("the row shows")
    }

    #[test]
    fn expanding_requests_children_once() {
        let home = folder("home");
        let mut tree = Tree::new(VisibilityFilter::default());
        assert_eq!(tree.set_roots(roots(&[("홈", &home)])), []);
        assert_eq!(shown(&tree), rows(&[(0, "홈")]));
        let root = row(&tree, 0);
        assert!(root.has_children_unknown && !root.has_children && !root.expanded);

        let request = tree.toggle(0).expect("the first opening lists the folder");
        assert_eq!(request.path, home);
        // The worker sorts the folders by name, naturally.
        assert_eq!(request.sort, SortSpec::default());
        assert_eq!(tree.generations().collect::<Vec<_>>(), [request.generation]);
        assert!(
            tree.node(0)
                .is_some_and(|node| node.loading && node.expanded)
        );
        // Closing and opening again while the listing is on its way asks for nothing more.
        assert_eq!(tree.toggle(0), None);
        assert_eq!(tree.toggle(0), None);

        // The listing keeps the folders, in its order.
        let entries = vec![
            dir("b2"),
            Entry::new("notes.txt", EntryKind::File),
            dir("b10"),
            Entry::new("latest", EntryKind::Symlink),
        ];
        assert_eq!(tree.apply_children(request.generation, entries), []);
        assert_eq!(shown(&tree), rows(&[(0, "홈"), (1, "b2"), (1, "b10")]));
        let root = row(&tree, 0);
        assert!(root.has_children && !root.has_children_unknown && root.expanded);
        assert_eq!(tree.generations().count(), 0);
        assert_eq!(
            tree.node(1).map(|node| &node.path),
            Some(&under(&home, &["b2"]))
        );

        // Closing hides the children; opening again shows them without a new listing.
        assert_eq!(tree.toggle(0), None);
        assert_eq!(shown(&tree), rows(&[(0, "홈")]));
        assert_eq!(tree.toggle(0), None);
        assert_eq!(shown(&tree).len(), 3);

        // A folder without subfolders is listed once and loses its chevron.
        let b2 = tree.node_at(1).expect("a second row");
        let request = tree.toggle(b2).expect("b2 is listed the first time");
        assert_eq!(request.path, under(&home, &["b2"]));
        tree.apply_children(request.generation, vec![Entry::new("x", EntryKind::File)]);
        let b2_row = row(&tree, 1);
        assert!(!b2_row.has_children && !b2_row.has_children_unknown);
        assert_eq!(tree.toggle(b2), None);
        assert_eq!(tree.toggle(b2), None);
        assert_eq!(tree.toggle(99), None);
    }

    #[test]
    fn reveal_expands_ancestors() {
        let home = folder("home");
        let docs = under(&home, &["문서"]);
        let disk = ancestors(&home)[0].clone();
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[
            ("홈", &home),
            ("문서", &docs),
            ("시스템 디스크", &disk),
        ]));

        // The active pane is three folders below home, which is below the disk too: only the
        // deepest root that holds it opens.
        let target = under(&home, &["프로젝트", "앱", "docs"]);
        let request = the_request(tree.reveal(&target));
        assert_eq!(request.path, home);
        assert_eq!(tree.target(), Some(&target));
        assert_eq!(tree.selected(), None);

        // Each listing opens the next folder on the way, and only that one.
        let request = the_request(tree.apply_children(
            request.generation,
            vec![dir("다운로드"), dir("프로젝트"), dir("사진")],
        ));
        assert_eq!(request.path, under(&home, &["프로젝트"]));
        let request =
            the_request(tree.apply_children(request.generation, vec![dir("앱"), dir("other")]));
        assert_eq!(request.path, under(&home, &["프로젝트", "앱"]));
        // The folder itself is selected, not opened.
        assert_eq!(
            tree.apply_children(
                request.generation,
                vec![dir("assets"), dir("docs"), dir("src")]
            ),
            []
        );
        assert_eq!(
            shown(&tree),
            rows(&[
                (0, "홈"),
                (1, "다운로드"),
                (1, "프로젝트"),
                (2, "앱"),
                (3, "assets"),
                (3, "*docs"),
                (3, "src"),
                (2, "other"),
                (1, "사진"),
                (0, "문서"),
                (0, "시스템 디스크"),
            ])
        );
        assert_eq!(tree.selected_row(), Some(5));
        assert_eq!(
            tree.selected()
                .and_then(|id| tree.node(id))
                .map(|node| &node.path),
            Some(&target)
        );
        assert!(!row(&tree, 5).expanded);
        assert!(!row(&tree, 9).expanded && !row(&tree, 10).expanded);

        // A folder the tree has listed asks for nothing; a root is selected as it is.
        assert_eq!(tree.reveal(&under(&home, &["프로젝트"])), []);
        assert_eq!(tree.selected_row(), Some(2));
        assert_eq!(tree.reveal(&docs), []);
        assert_eq!(tree.selected_row(), Some(9));
        assert!(!row(&tree, 9).expanded);

        // A folder that is not in its parent's listing is looked for in a new listing of the
        // parent, once; still missing, it ends the way.
        let request = the_request(tree.reveal(&under(&home, &["gone", "deeper"])));
        assert_eq!(request.path, home);
        assert_eq!(
            tree.apply_children(
                request.generation,
                vec![dir("다운로드"), dir("프로젝트"), dir("사진")]
            ),
            []
        );
        assert_eq!(tree.selected(), None);
        assert_eq!(shown(&tree).len(), 11);
    }

    #[test]
    fn reveal_does_nothing_outside_the_roots() {
        let home = folder("home");
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[("홈", &home)]));
        tree.reveal(&home);
        assert_eq!(tree.selected_row(), Some(0));

        // `home2` is no folder of `home`, though its name starts the same.
        for elsewhere in [folder("home2"), under(&folder("other"), &["x"])] {
            assert_eq!(tree.reveal(&elsewhere), []);
            assert_eq!(tree.selected(), None);
            assert_eq!(tree.target(), Some(&elsewhere));
            assert!(!row(&tree, 0).expanded);
        }
    }

    #[test]
    fn stale_children_are_dropped() {
        let home = folder("home");
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[("홈", &home)]));
        let first = tree.toggle(0).expect("listed");

        // A drive comes, so the places and the tree start again; the first listing comes late.
        tree.set_roots(roots(&[("홈", &home), ("USB", &folder("usb"))]));
        assert_eq!(tree.generations().count(), 0);
        assert_eq!(tree.apply_children(first.generation, vec![dir("late")]), []);
        assert!(!tree.listing_failed(first.generation));
        assert_eq!(shown(&tree), rows(&[(0, "홈"), (0, "USB")]));
        assert!(row(&tree, 0).has_children_unknown && !row(&tree, 0).expanded);

        // A listing is taken once.
        let second = tree.toggle(0).expect("listed again in the new tree");
        assert_ne!(second.generation, first.generation);
        tree.apply_children(second.generation, vec![dir("a")]);
        assert_eq!(
            tree.apply_children(second.generation, vec![dir("again")]),
            []
        );
        assert!(!tree.listing_failed(second.generation));
        assert_eq!(shown(&tree), rows(&[(0, "홈"), (1, "a"), (0, "USB")]));
    }

    #[test]
    fn a_failed_listing_is_tried_again() {
        let home = folder("home");
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[("홈", &home)]));
        let request = the_request(tree.reveal(&under(&home, &["locked", "inner"])));
        assert!(tree.listing_failed(request.generation));
        // No chevron, and no notice: the row is just empty.
        let root = row(&tree, 0);
        assert!(!root.has_children && !root.has_children_unknown);
        assert_eq!(tree.selected(), None);
        assert_eq!(tree.generations().count(), 0);

        // The next opening lists it again (the folder may be readable by now).
        let again = tree.toggle(0).expect("the opening tries again");
        assert_eq!(again.path, home);
        assert!(tree.listing_failed(again.generation));
        // So does the next reveal through it, which goes on when the listing comes.
        let request = the_request(tree.reveal(&under(&home, &["locked"])));
        assert_eq!(request.path, home);
        assert_eq!(
            tree.apply_children(request.generation, vec![dir("locked")]),
            []
        );
        assert_eq!(shown(&tree), rows(&[(0, "홈"), (1, "*locked")]));
    }

    #[test]
    fn hidden_folders_follow_the_setting() {
        let home = folder("home");
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[("홈", &home)]));
        let request = tree.toggle(0).expect("listed");
        let mut recycle = hidden(dir("$Recycle.Bin"));
        recycle.attributes |= Attributes::SYSTEM;
        tree.apply_children(
            request.generation,
            vec![recycle, hidden(dir(".config")), dir("docs")],
        );
        assert_eq!(shown(&tree), rows(&[(0, "홈"), (1, "docs")]));

        // A folder below a hidden one is not looked for while hidden folders do not show...
        let target = under(&home, &[".config", "app"]);
        assert_eq!(tree.reveal(&target), []);
        assert_eq!(tree.selected(), None);
        // ...and is once they do.
        let request = the_request(tree.set_show_hidden(true));
        assert_eq!(request.path, under(&home, &[".config"]));
        tree.apply_children(request.generation, vec![dir("app")]);
        assert_eq!(
            shown(&tree),
            rows(&[(0, "홈"), (1, ".config"), (2, "*app"), (1, "docs")])
        );

        // A protected folder (hidden and system) stays out of sight, as in the panes.
        assert!(tree.rows().iter().all(|row| row.label != "$Recycle.Bin"));

        // Hiding them again hides the branch; the listing is kept.
        assert_eq!(tree.set_show_hidden(false), []);
        assert_eq!(shown(&tree), rows(&[(0, "홈"), (1, "docs")]));
        assert_eq!(tree.selected_row(), None);
        assert_eq!(tree.set_show_hidden(true), []);
        assert_eq!(tree.selected_row(), Some(2));
    }

    #[test]
    fn a_newer_reveal_replaces_the_one_waiting() {
        let (home, usb) = (folder("home"), folder("usb"));
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[("홈", &home), ("USB", &usb)]));
        let at_home = the_request(tree.reveal(&under(&home, &["a", "b"])));
        let at_usb = the_request(tree.reveal(&under(&usb, &["c"])));
        assert_eq!(tree.generations().count(), 2);

        // Home's listing still shows, but the tree no longer goes down toward `a/b`.
        assert_eq!(tree.apply_children(at_home.generation, vec![dir("a")]), []);
        assert_eq!(tree.apply_children(at_usb.generation, vec![dir("c")]), []);
        assert_eq!(
            shown(&tree),
            rows(&[(0, "홈"), (1, "a"), (0, "USB"), (1, "*c")])
        );
    }

    #[test]
    fn closing_a_folder_on_the_way_stops_the_reveal() {
        let home = folder("home");
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[("홈", &home)]));
        let request = the_request(tree.reveal(&under(&home, &["a", "b"])));
        // The user closes home before its listing comes.
        assert_eq!(tree.toggle(0), None);
        assert_eq!(tree.apply_children(request.generation, vec![dir("a")]), []);
        assert_eq!(shown(&tree), rows(&[(0, "홈")]));
        assert!(!row(&tree, 0).expanded && row(&tree, 0).has_children);
    }

    #[test]
    fn closing_an_unrelated_folder_keeps_the_reveal() {
        // The documents place lies inside home's folder, but not inside home's node.
        let home = folder("home");
        let docs = under(&home, &["문서"]);
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[("홈", &home), ("문서", &docs)]));
        let request = the_request(tree.reveal(&under(&docs, &["세금"])));
        assert_eq!(request.path, docs);
        // Home opens and closes while the documents' listing is on its way.
        assert!(tree.toggle(0).is_some());
        assert_eq!(tree.toggle(0), None);
        assert_eq!(
            tree.apply_children(request.generation, vec![dir("세금")]),
            []
        );
        assert_eq!(tree.selected_row(), Some(2));
    }

    #[test]
    fn pane_listing_updates_tree_children() {
        let home = folder("home");
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[("홈", &home)]));
        let request = tree.toggle(0).expect("listed");
        tree.apply_children(request.generation, vec![dir("a"), dir("c")]);
        let a = tree.node_at(1).expect("a shows");
        let request = tree.toggle(a).expect("a is listed");
        tree.apply_children(request.generation, vec![dir("x")]);
        assert_eq!(
            shown(&tree),
            rows(&[(0, "홈"), (1, "a"), (2, "x"), (1, "c")])
        );

        // A pane lists home in its own order, files too: b2 and b10 came, c went. The new
        // folders take their place in natural order, and a keeps its node and what is open
        // below it.
        let before = tree.revision();
        assert!(tree.sync_folder(&home, &[file("z.txt"), dir("b10"), dir("b2"), dir("a")]));
        assert_ne!(tree.revision(), before);
        assert_eq!(
            shown(&tree),
            rows(&[(0, "홈"), (1, "a"), (2, "x"), (1, "b2"), (1, "b10")])
        );
        assert_eq!(tree.node_at(1), Some(a));
        assert!(row(&tree, 1).expanded);
        assert_eq!(
            tree.node(tree.node_at(3).expect("b2 shows"))
                .map(|node| &node.path),
            Some(&under(&home, &["b2"]))
        );

        // A folder that became hidden goes out of sight; its node is kept.
        assert!(tree.sync_folder(&home, &[dir("a"), hidden(dir("b2")), dir("b10")]));
        assert_eq!(
            shown(&tree),
            rows(&[(0, "홈"), (1, "a"), (2, "x"), (1, "b10")])
        );
        // Every listed node of the folder follows, a place and a folder below another place.
        let mut tree = Tree::new(VisibilityFilter::default());
        let docs = under(&home, &["문서"]);
        tree.set_roots(roots(&[("홈", &home), ("문서", &docs)]));
        let request = tree.toggle(0).expect("listed");
        tree.apply_children(request.generation, vec![dir("문서")]);
        let nested = tree.node_at(1).expect("home's documents");
        let request = tree.toggle(nested).expect("listed");
        tree.apply_children(request.generation, vec![]);
        let place = tree.node_at(2).expect("the documents place");
        let request = tree.toggle(place).expect("the documents place is listed");
        tree.apply_children(request.generation, vec![]);
        assert!(tree.sync_folder(&docs, &[dir("새 폴더")]));
        assert_eq!(
            shown(&tree),
            rows(&[
                (0, "홈"),
                (1, "문서"),
                (2, "새 폴더"),
                (0, "문서"),
                (1, "새 폴더")
            ])
        );
    }

    #[test]
    fn removed_node_clears_selection_and_its_load() {
        let home = folder("home");
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[("홈", &home)]));
        let request = tree.toggle(0).expect("listed");
        tree.apply_children(request.generation, vec![dir("a"), dir("b"), dir("c")]);
        // b's listing is on its way, a is selected, and the reveal waits for c's listing.
        let at_b = tree
            .toggle(tree.node_at(2).expect("b shows"))
            .expect("b is listed");
        assert_eq!(tree.reveal(&under(&home, &["a"])), []);
        assert_eq!(tree.selected_row(), Some(1));
        let at_c = the_request(tree.reveal(&under(&home, &["c", "deep"])));
        assert_eq!(tree.generations().count(), 2);

        // Another app removes a, b and c.
        assert!(tree.sync_folder(&home, &[dir("d")]));
        assert_eq!(shown(&tree), rows(&[(0, "홈"), (1, "d")]));
        assert_eq!(tree.selected(), None);
        assert_eq!(tree.generations().count(), 0);
        // Their listings are dropped when they come.
        assert_eq!(tree.apply_children(at_b.generation, vec![dir("x")]), []);
        assert_eq!(tree.apply_children(at_c.generation, vec![dir("deep")]), []);
        assert_eq!(shown(&tree), rows(&[(0, "홈"), (1, "d")]));

        // A selected folder that goes away leaves nothing selected.
        assert_eq!(tree.reveal(&under(&home, &["d"])), []);
        assert_eq!(tree.selected_row(), Some(1));
        assert!(tree.sync_folder(&home, &[]));
        assert_eq!(tree.selected(), None);
        assert_eq!(tree.selected_row(), None);
    }

    #[test]
    fn reveal_relists_a_parent_once_when_the_folder_is_new() {
        let home = folder("home");
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[("홈", &home)]));
        let request = tree.toggle(0).expect("listed");
        tree.apply_children(request.generation, vec![dir("a"), hidden(dir(".h"))]);

        // The pane went into a folder made after home was listed: home is listed again, its
        // listing adds the folder, and the tree goes on below it.
        let target = under(&home, &["new", "deeper"]);
        let request = the_request(tree.reveal(&target));
        assert_eq!(request.path, home);
        // The rows stay while the new listing comes.
        assert_eq!(shown(&tree), rows(&[(0, "홈"), (1, "a")]));
        let request = the_request(tree.apply_children(
            request.generation,
            vec![dir("a"), dir("new"), hidden(dir(".h"))],
        ));
        assert_eq!(request.path, under(&home, &["new"]));
        assert_eq!(
            tree.apply_children(request.generation, vec![dir("deeper")]),
            []
        );
        assert_eq!(
            shown(&tree),
            rows(&[(0, "홈"), (1, "a"), (1, "new"), (2, "*deeper")])
        );

        // Still missing after the new listing, the folder ends the way: once per reveal.
        let request = the_request(tree.reveal(&under(&home, &["gone"])));
        assert_eq!(
            tree.apply_children(
                request.generation,
                vec![dir("a"), dir("new"), hidden(dir(".h"))]
            ),
            []
        );
        assert_eq!(tree.selected(), None);
        assert_eq!(tree.generations().count(), 0);
        // A hidden folder is there, only out of sight: no new listing.
        assert_eq!(tree.reveal(&under(&home, &[".h"])), []);
    }

    #[test]
    fn unchanged_listing_keeps_the_revision() {
        let home = folder("home");
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[("홈", &home)]));
        let request = tree.toggle(0).expect("listed");
        tree.apply_children(request.generation, vec![dir("a"), dir("b")]);
        let before = tree.revision();

        // The same folders in another order, with files: nothing changes (a watch reports
        // often while a copy runs).
        assert!(!tree.sync_folder(&home, &[dir("b"), file("x.txt"), dir("a")]));
        assert_eq!(tree.revision(), before);
        // Nor for a folder the tree has not listed, or one whose new listing is on its way
        // (that listing brings the change).
        assert!(!tree.sync_folder(&folder("other"), &[dir("y")]));
        assert!(!tree.sync_folder(&under(&home, &["a"]), &[dir("y")]));
        let again = the_request(tree.relist_folder(&home));
        assert!(!tree.sync_folder(&home, &[dir("c")]));
        assert_eq!(tree.revision(), before);
        assert!(
            tree.generations()
                .any(|generation| generation == again.generation)
        );
    }

    #[test]
    fn changed_folders_are_listed_again() {
        let home = folder("home");
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[("홈", &home), ("USB", &folder("usb"))]));
        let request = tree.toggle(0).expect("listed");
        tree.apply_children(request.generation, vec![dir("a")]);

        // A job made `b` in home: the listed node is listed again, and the listing reconciles.
        let request = the_request(tree.relist_folder(&home));
        assert_eq!(request.path, home);
        // Not twice while that listing is on its way, and not a folder never listed.
        assert_eq!(tree.relist_folder(&home), []);
        assert_eq!(tree.relist_folder(&folder("usb")), []);
        assert_eq!(
            tree.apply_children(request.generation, vec![dir("a"), dir("b")]),
            []
        );
        assert_eq!(
            shown(&tree),
            rows(&[(0, "홈"), (1, "a"), (1, "b"), (0, "USB")])
        );
    }

    #[test]
    fn a_folder_shown_already_is_selected_where_it_is() {
        let home = folder("home");
        let pictures = under(&home, &["사진"]);
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[("홈", &home), ("사진", &pictures)]));
        let request = tree.toggle(0).expect("listed");
        tree.apply_children(request.generation, vec![dir("문서"), dir("사진")]);
        assert_eq!(
            shown(&tree),
            rows(&[(0, "홈"), (1, "문서"), (1, "사진"), (0, "사진")])
        );

        // The pane goes into home's pictures: the row under home shows it already, so that one
        // is selected, though the pictures place is the deepest root.
        assert_eq!(tree.reveal(&pictures), []);
        assert_eq!(tree.selected_row(), Some(2));
        // With home closed, the place is the row that shows it.
        tree.toggle(0);
        assert_eq!(tree.reveal(&home), []);
        assert_eq!(tree.reveal(&pictures), []);
        assert_eq!(tree.selected_row(), Some(1));
        assert!(!row(&tree, 0).expanded);

        // A click selects its own row, even when another row shows the same folder.
        tree.toggle(0);
        let place = tree.node_at(3).expect("four rows");
        assert!(tree.select(place));
        assert_eq!(tree.selected_row(), Some(3));
        assert_eq!(tree.target(), Some(&pictures));
        assert!(!tree.select(99));
        assert_eq!(tree.selected_row(), Some(3));
    }

    #[test]
    fn find_lists_the_listed_folders_that_match_flat() {
        let home = folder("home");
        let docs = under(&home, &["문서"]);
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots(roots(&[("홈", &home), ("문서", &docs)]));
        let request = tree.toggle(0).expect("listed");
        tree.apply_children(
            request.generation,
            vec![dir("문서"), dir("Backup"), hidden(dir(".backup-old"))],
        );
        let nested = tree.node_at(1).expect("home's documents");
        let request = tree.toggle(nested).expect("listed");
        tree.apply_children(request.generation, vec![dir("백업"), dir("backups 2026")]);
        // Closed again: what was listed below it is still searched.
        tree.toggle(nested);
        assert_eq!(shown(&tree).len(), 4);

        let labels = |found: Vec<NodeId>| -> Vec<String> {
            found
                .into_iter()
                .map(|id| tree.node(id).expect("a node").label.clone())
                .collect()
        };
        // Letter case does not matter; a hidden folder stays out of sight.
        assert_eq!(labels(tree.find("BACK")), ["backups 2026", "Backup"]);
        assert_eq!(labels(tree.find(" 백업 ")), ["백업"]);
        // The documents place and home's documents are one folder: it shows once, where the
        // tree has it first.
        let found = tree.find("문서");
        assert_eq!(found, [nested]);
        assert_eq!(tree.find(""), Vec::<NodeId>::new());
        assert_eq!(tree.find("없는 이름"), Vec::<NodeId>::new());
        // A folder not listed yet is not found: the documents place was never opened.
        assert_eq!(tree.find("new"), Vec::<NodeId>::new());
    }

    #[test]
    fn two_trees_never_share_a_revision() {
        // The picker gets a new tree each time it opens, and the model of its rows skips a
        // redraw while the revision stays the same: a new tree must not take an old one's.
        let home = folder("home");
        let mut first = Tree::new(VisibilityFilter::default());
        first.set_roots(roots(&[("홈", &home)]));
        let mut second = Tree::new(VisibilityFilter::default());
        assert_ne!(second.revision(), first.revision());
        second.set_roots(roots(&[("USB", &folder("usb"))]));
        assert_ne!(second.revision(), first.revision());
    }

    #[test]
    fn rows_change_only_with_the_tree() {
        let home = folder("home");
        let mut tree = Tree::new(VisibilityFilter::default());
        let empty = tree.revision();
        tree.set_roots(roots(&[("홈", &home)]));
        let with_roots = tree.revision();
        assert_ne!(with_roots, empty);
        assert_eq!(tree.node_at(0), Some(0));
        assert_eq!(tree.node_at(1), None);
        let request = tree.toggle(0).expect("listed");
        assert_ne!(tree.revision(), with_roots);
        let before = tree.revision();
        tree.apply_children(request.generation + 1000, vec![dir("stale")]);
        assert_eq!(tree.revision(), before);
    }
}
