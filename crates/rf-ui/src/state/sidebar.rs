//! The sidebar's folder trees from the keyboard (KEY-02). A child of [`state`](super), so these
//! methods see the fields of [`AppState`].
//!
//! F6 gives a tree the keys ([`Command::FocusTree`](crate::keys::Command::FocusTree)) while
//! it shows; a hidden one is passed by. With a tree beside each pane (LAY-03 b) F6 visits the
//! left pane's tree, then the right pane's, before it goes on. The tree with the keys has a
//! cursor of its own, apart from its selection (its pane's folder): it starts on the
//! selection's row, or the first row, and the keys of [`TreeKey`] move it, open and close its
//! row, or show its folder in the tree's pane, which leaves the keys with the tree. Esc and Tab
//! give them back to the active pane; F6 takes them on to the next tree that shows, then to the
//! job panel while it shows (module `panel`), else back to the active pane. While a tree has
//! the keys, the window's other commands work as from the panes, and those of the panes' rows
//! do nothing ([`Command::acts_on_rows`](crate::keys::Command::acts_on_rows)).
//!
//! The group of recent folders lies between the favorites and the drives, and the arrows walk
//! through it (the cursor is then in the group, on no row of the tree): ↓ from the head goes into
//! the group, → opens a folded one and ← folds it, ↩ on the head folds or opens it and on a
//! folder shows it. On a favorite's row the menu key (Shift+F10, macOS ⌃↩) opens its menu, F2
//! (macOS ↩) renames it and Delete takes it out of the list; on any other row the menu key opens
//! the menu of the sidebar's empty part.

use super::favorites::RecentAt;
use super::{AppState, Effect, MenuShown};
use crate::keys::TreeKey;

impl AppState {
    /// The row of the keyboard cursor in the tree that had the keys last
    /// ([`AppState::tree_with_keys`]), while it shows.
    pub fn tree_cursor_row(&self) -> Option<usize> {
        self.trees[self.tree_keys].row_of(self.tree_cursor?)
    }

    /// The tree that had the keys last, whose keyboard cursor [`AppState::tree_cursor_row`]
    /// is: 0 the shared one or the left pane's, 1 the right pane's.
    pub fn tree_with_keys(&self) -> usize {
        self.tree_keys
    }

    /// F6 from the panes: the first tree that shows takes the keys (the left pane's, with a
    /// tree for each pane), its cursor on the row of its pane's folder (or the first row). A
    /// hidden tree is passed by, as File Explorer's F6 passes by the parts that do not show: the
    /// job panel takes the keys while it shows, else the panes keep them. The layout stays as
    /// the user chose it.
    pub(super) fn focus_tree(&mut self) -> Vec<Effect> {
        match self.layout.shown_trees(self.active).first() {
            Some(&tree) => self.keys_to_tree(tree),
            None => self.tree_next(),
        }
    }

    /// Tree `tree` takes the keys, its cursor on the row of its pane's folder (or the first
    /// row).
    fn keys_to_tree(&mut self, tree: usize) -> Vec<Effect> {
        self.tree_keys = tree;
        let row = self.trees[tree].selected_row().unwrap_or(0);
        self.tree_cursor = self.trees[tree].node_at(row);
        self.kept.cursor = None;
        let mut effects = vec![Effect::FocusTree(tree)];
        effects.extend(
            self.tree_cursor_row()
                .map(|row| Effect::ScrollTree { tree, row }),
        );
        effects
    }

    /// The window tells that tree `tree` has the keys, for the key it hands on
    /// ([`AppState::tree_key`]): the cursor starts there anew when another tree had them. A
    /// tree that does not show (it went while it had them) gives them back to the panes.
    pub fn tree_has_keys(&mut self, tree: usize) -> Vec<Effect> {
        if !self.layout.tree_shows(tree, self.active) {
            return vec![Effect::FocusPanes];
        }
        if tree != self.tree_keys {
            self.tree_keys = tree;
            self.tree_cursor = None;
            self.kept.cursor = None;
        }
        Vec::new()
    }

    /// Tree `tree`'s sidebar was used (a click on a row, a menu of a favorite or of its empty
    /// part): the pane the tree follows takes the keyboard, so that what is opened there, and
    /// its 현재 폴더, are that pane's. Nothing for the shared tree, which follows the active
    /// pane.
    pub fn activate_tree_pane(&mut self, tree: usize) {
        let side = self.tree_pane(tree);
        if side != self.active {
            self.activate(side);
        }
    }

    /// F6 in a tree: the next tree that shows takes the keys (the right pane's after the left
    /// one's), else the job panel while it shows, else the panes.
    fn after_tree(&mut self) -> Vec<Effect> {
        let next = self
            .layout
            .shown_trees(self.active)
            .into_iter()
            .find(|&tree| tree > self.tree_keys);
        match next {
            Some(tree) => self.keys_to_tree(tree),
            None => self.tree_next(),
        }
    }

    /// A key of the tree that has the keys ([`crate::keys::tree_key`],
    /// [`AppState::tree_with_keys`]). A cursor whose row no longer shows (a row above it was
    /// closed) starts again from the selection's row.
    pub fn tree_key(&mut self, key: TreeKey) -> Vec<Effect> {
        match key {
            TreeKey::Leave => return vec![Effect::FocusPanes],
            TreeKey::Next => return self.after_tree(),
            _ => {}
        }
        if let Some(at) = self.kept.cursor.filter(|_| !self.kept.recent.is_empty()) {
            return self.recent_key(key, at);
        }
        let tree = self.tree_keys;
        let rows = self.trees[tree].rows();
        let Some(row) = self
            .tree_cursor_row()
            .or_else(|| self.trees[tree].selected_row())
            .or_else(|| (!rows.is_empty()).then_some(0))
        else {
            return Vec::new();
        };
        let here = &rows[row];
        let expandable = here.has_children_unknown || here.has_children;
        let open = expandable && here.expanded;
        // The group of recent folders lies right before this row of the tree.
        let slot = self.recent_slot();
        let favorite = self.favorite_of_row(row);
        let mut effects = Vec::new();
        let to = match key {
            TreeKey::Up if slot == Some(row) => return self.enter_recent(false),
            TreeKey::Down if slot.is_some_and(|slot| slot > 0 && row == slot - 1) => {
                return self.enter_recent(true);
            }
            TreeKey::Up => row.saturating_sub(1),
            TreeKey::Down => (row + 1).min(rows.len() - 1),
            TreeKey::Expand if expandable && !open => {
                let id = here.id;
                effects = self.with_tree(tree, |shown| shown.toggle(id).into_iter().collect());
                row
            }
            // The row below an open one is its first folder, when it has one that shows.
            TreeKey::Expand => match rows.get(row + 1) {
                Some(next) if open && next.depth == here.depth + 1 => row + 1,
                _ => row,
            },
            TreeKey::Collapse if open => {
                let id = here.id;
                effects = self.with_tree(tree, |shown| shown.toggle(id).into_iter().collect());
                row
            }
            TreeKey::Collapse => self.trees[tree]
                .node(here.id)
                .and_then(|node| node.parent)
                .and_then(|parent| self.trees[tree].row_of(parent))
                .unwrap_or(row),
            TreeKey::Open => {
                effects = self.open_tree_row(tree, row);
                row
            }
            TreeKey::OpenOrRename => {
                if let Some(index) = favorite {
                    return self.favorite_command(index, "rename");
                }
                effects = self.open_tree_row(tree, row);
                row
            }
            TreeKey::Rename => {
                return favorite
                    .map_or_else(Vec::new, |index| self.favorite_command(index, "rename"));
            }
            TreeKey::Remove => {
                return favorite
                    .map_or_else(Vec::new, |index| self.favorite_command(index, "remove"));
            }
            TreeKey::Menu => {
                let shown = match favorite {
                    Some(index) => MenuShown::Favorite { index },
                    None => MenuShown::FavoritesBlank,
                };
                return vec![Effect::ShowMenu(shown)];
            }
            TreeKey::Leave | TreeKey::Next => row,
        };
        self.tree_cursor = self.trees[tree].node_at(to);
        effects.push(Effect::ScrollTree { tree, row: to });
        effects
    }

    /// The keyboard cursor goes into the group of recent folders from the tree's row above it
    /// (`from_above`, by ↓: on its head) or from the row below it (by ↑: on its last folder,
    /// else on its head when it is folded).
    fn enter_recent(&mut self, from_above: bool) -> Vec<Effect> {
        let last = self.kept.recent.len().saturating_sub(1);
        let at = if from_above || self.kept.folded {
            RecentAt::Head
        } else {
            RecentAt::Item(last)
        };
        self.cursor_in_recent(at)
    }

    fn cursor_in_recent(&mut self, at: RecentAt) -> Vec<Effect> {
        self.tree_cursor = None;
        self.kept.cursor = Some(at);
        vec![Effect::ScrollRecent {
            tree: self.tree_keys,
            at,
        }]
    }

    /// The cursor goes to row `row` of the tree with the keys.
    fn cursor_on_row(&mut self, row: usize) -> Vec<Effect> {
        let tree = self.tree_keys;
        self.kept.cursor = None;
        self.tree_cursor = self.trees[tree].node_at(row);
        vec![Effect::ScrollTree { tree, row }]
    }

    /// A key while the cursor is in the group of recent folders, at `at`.
    fn recent_key(&mut self, key: TreeKey, at: RecentAt) -> Vec<Effect> {
        let tree = self.tree_keys;
        let count = self.kept.recent.len();
        let slot = self.recent_slot().unwrap_or(0);
        let rows = self.trees[tree].rows().len();
        // The last stop of the group: its head when it is folded, else its last folder.
        let end = if self.kept.folded {
            RecentAt::Head
        } else {
            RecentAt::Item(count - 1)
        };
        match (key, at) {
            (TreeKey::Up, RecentAt::Item(0)) | (TreeKey::Collapse, RecentAt::Item(_)) => {
                self.cursor_in_recent(RecentAt::Head)
            }
            (TreeKey::Up, RecentAt::Item(i)) => self.cursor_in_recent(RecentAt::Item(i - 1)),
            (TreeKey::Up, RecentAt::Head) if slot > 0 => self.cursor_on_row(slot - 1),
            (TreeKey::Down, RecentAt::Head) if !self.kept.folded => {
                self.cursor_in_recent(RecentAt::Item(0))
            }
            (TreeKey::Down, at) if at == end && slot < rows => self.cursor_on_row(slot),
            (TreeKey::Down, RecentAt::Item(i)) if i + 1 < count => {
                self.cursor_in_recent(RecentAt::Item(i + 1))
            }
            (TreeKey::Expand, RecentAt::Head) => {
                if self.kept.folded {
                    self.set_recent_folded(false);
                    vec![Effect::ScrollRecent { tree, at }]
                } else {
                    self.cursor_in_recent(RecentAt::Item(0))
                }
            }
            (TreeKey::Collapse, RecentAt::Head) => {
                self.set_recent_folded(true);
                vec![Effect::ScrollRecent { tree, at }]
            }
            (TreeKey::Open | TreeKey::OpenOrRename, RecentAt::Head) => {
                self.toggle_recent_fold();
                vec![Effect::ScrollRecent {
                    tree,
                    at: RecentAt::Head,
                }]
            }
            (TreeKey::Open | TreeKey::OpenOrRename, RecentAt::Item(i)) => self.open_recent(tree, i),
            (TreeKey::Menu, _) => vec![Effect::ShowMenu(MenuShown::FavoritesBlank)],
            _ => Vec::new(),
        }
    }
}
