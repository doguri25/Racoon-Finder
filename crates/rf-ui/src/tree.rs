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
//! A node is listed once, the first time it opens, and keeps its children, hidden ones too;
//! the rows show the hidden ones only while the setting says so. A node whose listing failed
//! has no children and shows no chevron. [`Tree::reveal`] follows the folder of the active
//! pane: it opens only the folders on the way to it, below the deepest root that holds it,
//! and lists each of them in turn as the listing above it comes, so a far folder costs one
//! listing per level and nothing else.

use std::collections::HashMap;

use rf_core::{Attributes, Entry, EntryKind, VPath};
use rf_vfs::SortSpec;

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
    /// The node shows its children (once they are listed).
    pub expanded: bool,
    /// The child folders in the listing's order, once listed; `None` before.
    pub children: Option<Vec<NodeId>>,
    /// The listing of the children is on its way.
    pub loading: bool,
    /// The folder is hidden: its row shows only while hidden items do.
    pub hidden: bool,
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
/// and count every change in `revision`.
#[derive(Debug)]
pub struct Tree {
    nodes: Vec<TreeNode>,
    roots: Vec<NodeId>,
    show_hidden: bool,
    /// The folder the tree follows: the one [`Tree::reveal`] or [`Tree::select`] was last
    /// given.
    target: Option<VPath>,
    /// The node of `target`, once found.
    selected: Option<NodeId>,
    /// The node whose listing the reveal of `target` waits for, to go on below it.
    waiting: Option<NodeId>,
    /// The listings on their way, by generation.
    loads: HashMap<u64, NodeId>,
    /// The nodes the rows show, in order.
    visible: Vec<NodeId>,
    /// Goes up with every change of the rows.
    revision: u64,
}

impl Tree {
    /// An empty tree, showing hidden folders when `show_hidden` is set.
    pub fn new(show_hidden: bool) -> Tree {
        Tree {
            nodes: Vec::new(),
            roots: Vec::new(),
            show_hidden,
            target: None,
            selected: None,
            waiting: None,
            loads: HashMap::new(),
            visible: Vec::new(),
            revision: 0,
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
        self.roots.clear();
        for (index, (label, path)) in roots.into_iter().enumerate() {
            self.roots.push(self.nodes.len());
            self.nodes.push(TreeNode {
                path,
                label,
                depth: 0,
                expanded: false,
                children: None,
                loading: false,
                hidden: false,
                root: Some(index),
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
                    has_children_unknown: node.children.is_none(),
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

    /// A number that changes whenever the rows may have: a model of the rows can skip a
    /// redraw while it stays the same.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The generations of the listings on their way.
    pub fn generations(&self) -> impl Iterator<Item = u64> + '_ {
        self.loads.keys().copied()
    }

    /// Opens node `id`, or closes it. The first time a node opens it asks for its listing;
    /// after that, or while the listing is on its way, it asks for nothing. Closing a folder
    /// on the way to the one the tree follows stops looking for that one there. `None` too for
    /// a node past the end.
    pub fn toggle(&mut self, id: NodeId) -> Option<LoadRequest> {
        let node = self.nodes.get_mut(id)?;
        node.expanded = !node.expanded;
        let request = if node.expanded {
            self.request(id)
        } else {
            if let Some(waiting) = self.waiting
                && self.nodes[waiting].path.starts_with(&self.nodes[id].path)
            {
                self.waiting = None;
            }
            None
        };
        self.refresh();
        request
    }

    /// Takes the listing of request `generation`: its folders become the node's children, in
    /// the listing's order, and files and links are left out. If the folder the tree follows
    /// waited for this listing, the tree goes on toward it and returns the listing that needs
    /// next. A listing that no node waits for any more is dropped.
    pub fn apply_children(&mut self, generation: u64, entries: Vec<Entry>) -> Vec<LoadRequest> {
        let Some(id) = self.loads.remove(&generation) else {
            return Vec::new();
        };
        let parent = self.nodes[id].path.clone();
        let depth = self.nodes[id].depth + 1;
        let mut children = Vec::new();
        for entry in entries {
            if entry.kind != EntryKind::Dir {
                continue;
            }
            let Ok(path) = parent.join(&entry.name) else {
                continue;
            };
            children.push(self.nodes.len());
            self.nodes.push(TreeNode {
                path,
                label: entry.name.to_string_lossy().into_owned(),
                depth,
                expanded: false,
                children: None,
                loading: false,
                hidden: entry.attributes.contains(Attributes::HIDDEN),
                root: None,
            });
        }
        let node = &mut self.nodes[id];
        node.loading = false;
        node.children = Some(children);
        let requests = if self.waiting == Some(id) {
            self.waiting = None;
            self.reveal_from(id)
        } else {
            Vec::new()
        };
        self.refresh();
        requests
    }

    /// The listing of request `generation` failed: the node has no children, and no chevron.
    /// `false`, and no change, for a listing that no node waits for any more.
    pub fn listing_failed(&mut self, generation: u64) -> bool {
        let Some(id) = self.loads.remove(&generation) else {
            return false;
        };
        let node = &mut self.nodes[id];
        node.loading = false;
        node.children = Some(Vec::new());
        if self.waiting == Some(id) {
            self.waiting = None;
        }
        self.refresh();
        true
    }

    /// Follows `path`, the folder of the active pane, and selects its node. When a row shows
    /// it already, that row is the one (the first, when several do) and nothing opens.
    /// Otherwise the tree opens the folders on the way to it below the deepest root that holds
    /// it: a folder on the way that has not been listed is listed (the returned request), and
    /// the tree goes on when the listing comes ([`Tree::apply_children`]); one that is not in
    /// its parent's listing, or is hidden while hidden folders do not show, ends the way, with
    /// nothing selected. A path under no root opens nothing and selects nothing.
    pub fn reveal(&mut self, path: &VPath) -> Vec<LoadRequest> {
        self.target = Some(path.clone());
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

    /// Shows the hidden folders, or hides them. When the folder the tree follows was not found
    /// (it may be below a hidden one), the tree looks for it again.
    pub fn set_show_hidden(&mut self, show: bool) -> Vec<LoadRequest> {
        if self.show_hidden == show {
            return Vec::new();
        }
        self.show_hidden = show;
        self.refresh();
        let requests = if self.selected.is_none() && self.waiting.is_none() {
            self.reveal_target()
        } else {
            Vec::new()
        };
        self.refresh();
        requests
    }

    /// Whether node `id` shows when its parent is open.
    fn shows(&self, id: NodeId) -> bool {
        self.show_hidden || !self.nodes[id].hidden
    }

    /// The request for the listing of node `id`; `None` when it is listed or on its way.
    fn request(&mut self, id: NodeId) -> Option<LoadRequest> {
        let node = &mut self.nodes[id];
        if node.loading || node.children.is_some() {
            return None;
        }
        node.loading = true;
        let generation = next_generation();
        self.loads.insert(generation, id);
        Some(LoadRequest {
            generation,
            path: node.path.clone(),
            sort: SortSpec::default(),
        })
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
    /// yet (listed, and `waiting` for), or one without the next folder of the way.
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
                .find(|&child| self.shows(child) && target.starts_with(&self.nodes[child].path));
            match next {
                Some(child) => id = child,
                None => return Vec::new(),
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
        self.revision += 1;
    }
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
        let mut tree = Tree::new(false);
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
        let mut tree = Tree::new(false);
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

        // A folder that is not in its parent's listing (gone since) is not looked for.
        assert_eq!(tree.reveal(&under(&home, &["gone", "deeper"])), []);
        assert_eq!(tree.selected(), None);
        assert_eq!(shown(&tree).len(), 11);
    }

    #[test]
    fn reveal_does_nothing_outside_the_roots() {
        let home = folder("home");
        let mut tree = Tree::new(false);
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
        let mut tree = Tree::new(false);
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
    fn a_failed_listing_shows_no_chevron() {
        let home = folder("home");
        let mut tree = Tree::new(false);
        tree.set_roots(roots(&[("홈", &home)]));
        let request = the_request(tree.reveal(&under(&home, &["locked", "inner"])));
        assert!(tree.listing_failed(request.generation));
        let root = row(&tree, 0);
        assert!(!root.has_children && !root.has_children_unknown);
        assert_eq!(tree.selected(), None);
        assert_eq!(tree.generations().count(), 0);
        // Nothing is asked for again.
        assert_eq!(tree.toggle(0), None);
        assert_eq!(tree.toggle(0), None);
        assert_eq!(tree.reveal(&under(&home, &["locked"])), []);
    }

    #[test]
    fn hidden_folders_follow_the_setting() {
        let home = folder("home");
        let mut tree = Tree::new(false);
        tree.set_roots(roots(&[("홈", &home)]));
        let request = tree.toggle(0).expect("listed");
        tree.apply_children(
            request.generation,
            vec![hidden(dir(".config")), dir("docs")],
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
        let mut tree = Tree::new(false);
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
        let mut tree = Tree::new(false);
        tree.set_roots(roots(&[("홈", &home)]));
        let request = the_request(tree.reveal(&under(&home, &["a", "b"])));
        // The user closes home before its listing comes.
        assert_eq!(tree.toggle(0), None);
        assert_eq!(tree.apply_children(request.generation, vec![dir("a")]), []);
        assert_eq!(shown(&tree), rows(&[(0, "홈")]));
        assert!(!row(&tree, 0).expanded && row(&tree, 0).has_children);
    }

    #[test]
    fn a_folder_shown_already_is_selected_where_it_is() {
        let home = folder("home");
        let pictures = under(&home, &["사진"]);
        let mut tree = Tree::new(false);
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
    fn rows_change_only_with_the_tree() {
        let home = folder("home");
        let mut tree = Tree::new(false);
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
