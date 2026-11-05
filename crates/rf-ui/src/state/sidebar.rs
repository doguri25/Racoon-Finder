//! The sidebar's folder tree from the keyboard (KEY-02). A child of [`state`](super), so these
//! methods see the fields of [`AppState`].
//!
//! F6 gives the tree the keys ([`Command::FocusTree`](crate::keys::Command::FocusTree)) while
//! it shows; a hidden one is passed by. The tree has a cursor of its own, apart from its
//! selection (the active pane's folder): it starts on the selection's row, or the first row,
//! and the keys of [`TreeKey`] move it, open and close its row, or show its folder in the
//! active pane, which leaves the keys with the tree. Esc and Tab give them back to the active
//! pane; F6 takes them on to the job panel while it shows (module `panel`), else back to the
//! active pane. While the tree has the keys, the window's other commands work as from the
//! panes, and those of the panes' rows do nothing
//! ([`Command::acts_on_rows`](crate::keys::Command::acts_on_rows)).
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
    /// The row of the tree's keyboard cursor, while it shows.
    pub fn tree_cursor_row(&self) -> Option<usize> {
        self.tree.row_of(self.tree_cursor?)
    }

    /// F6 from the panes: the tree takes the keys, its cursor on the row of the active pane's
    /// folder (or the first row). A hidden tree is passed by, as File Explorer's F6 passes by
    /// the parts that do not show: the job panel takes the keys while it shows, else the panes
    /// keep them. The layout stays as the user chose it.
    pub(super) fn focus_tree(&mut self) -> Vec<Effect> {
        if !self.layout.tree {
            return self.tree_next();
        }
        let mut effects = Vec::new();
        let row = self.tree.selected_row().unwrap_or(0);
        self.tree_cursor = self.tree.node_at(row);
        self.kept.cursor = None;
        effects.push(Effect::FocusTree);
        effects.extend(self.tree_cursor_row().map(Effect::ScrollTree));
        effects
    }

    /// A key of the tree while it has the keys ([`crate::keys::tree_key`]). A cursor whose row
    /// no longer shows (a row above it was closed) starts again from the selection's row.
    pub fn tree_key(&mut self, key: TreeKey) -> Vec<Effect> {
        match key {
            TreeKey::Leave => return vec![Effect::FocusPanes],
            TreeKey::Next => return self.tree_next(),
            _ => {}
        }
        if let Some(at) = self.kept.cursor.filter(|_| !self.kept.recent.is_empty()) {
            return self.recent_key(key, at);
        }
        let rows = self.tree.rows();
        let Some(row) = self
            .tree_cursor_row()
            .or_else(|| self.tree.selected_row())
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
                effects = self.with_tree(|tree| tree.toggle(id).into_iter().collect());
                row
            }
            // The row below an open one is its first folder, when it has one that shows.
            TreeKey::Expand => match rows.get(row + 1) {
                Some(next) if open && next.depth == here.depth + 1 => row + 1,
                _ => row,
            },
            TreeKey::Collapse if open => {
                let id = here.id;
                effects = self.with_tree(|tree| tree.toggle(id).into_iter().collect());
                row
            }
            TreeKey::Collapse => self
                .tree
                .node(here.id)
                .and_then(|node| node.parent)
                .and_then(|parent| self.tree.row_of(parent))
                .unwrap_or(row),
            TreeKey::Open => {
                effects = self.open_tree_row(row);
                row
            }
            TreeKey::OpenOrRename => {
                if let Some(index) = favorite {
                    return self.favorite_command(index, "rename");
                }
                effects = self.open_tree_row(row);
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
        self.tree_cursor = self.tree.node_at(to);
        effects.push(Effect::ScrollTree(to));
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
        vec![Effect::ScrollRecent(at)]
    }

    /// The cursor goes to row `row` of the tree.
    fn cursor_on_row(&mut self, row: usize) -> Vec<Effect> {
        self.kept.cursor = None;
        self.tree_cursor = self.tree.node_at(row);
        vec![Effect::ScrollTree(row)]
    }

    /// A key while the cursor is in the group of recent folders, at `at`.
    fn recent_key(&mut self, key: TreeKey, at: RecentAt) -> Vec<Effect> {
        let count = self.kept.recent.len();
        let slot = self.recent_slot().unwrap_or(0);
        let rows = self.tree.rows().len();
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
                    vec![Effect::ScrollRecent(at)]
                } else {
                    self.cursor_in_recent(RecentAt::Item(0))
                }
            }
            (TreeKey::Collapse, RecentAt::Head) => {
                self.set_recent_folded(true);
                vec![Effect::ScrollRecent(at)]
            }
            (TreeKey::Open | TreeKey::OpenOrRename, RecentAt::Head) => {
                self.toggle_recent_fold();
                vec![Effect::ScrollRecent(RecentAt::Head)]
            }
            (TreeKey::Open | TreeKey::OpenOrRename, RecentAt::Item(i)) => self.open_recent(i),
            (TreeKey::Menu, _) => vec![Effect::ShowMenu(MenuShown::FavoritesBlank)],
            _ => Vec::new(),
        }
    }
}
