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

use super::{AppState, Effect};
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
        let mut effects = Vec::new();
        let to = match key {
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
            TreeKey::Leave | TreeKey::Next => row,
        };
        self.tree_cursor = self.tree.node_at(to);
        effects.push(Effect::ScrollTree(to));
        effects
    }
}
