//! Dragging rows into another folder (DND, PRD 5.4): a press on a row of a pane readies a drag
//! of the items it would carry, the window asks while the drag hovers whether a place takes
//! them, and a drop there submits their move or copy at once, without the picker. A child of
//! [`state`](super), so these methods see the fields of [`AppState`].
//!
//! The items are noted when the press readies the drag ([`Drag`]), under an id that the
//! drag's payload carries ([`AppState::drag_id`]): a drop always moves the items the drag
//! started with, even when the pane's rows have changed since (a folder's watch), and a payload
//! of another drag takes nothing. Whether a place takes them is found from the paths alone, as
//! the picker finds before a worker has looked ([`validate`] without facts): never one of the
//! items or a folder inside one, nor the folder they are in, for a copy too (DND-09: a drop
//! back where they are does nothing). A hover asks at every move of the pointer, so each
//! folder's answer is kept for the drag. The job checks the rest (room, rights) and asks about
//! a name that is taken ([`ConflictPolicy::Ask`]).
//!
//! A press on a row that is selected leaves the selection as it is, so that the drag that may
//! follow carries all of it; the release, when no drag came of the press, then does what the
//! click does ([`AppState::release_row`]). A press on a row that is not selected is the click
//! at once.

use std::collections::HashMap;
use std::ffi::OsString;

use rf_core::{EntryKind, VPath};
use rf_jobs::{ConflictPolicy, JobSpec, TransferMode, TransferOptions, TransferSpec};

use super::jobs::submit;
use super::{AppState, Effect};
use crate::pane::next_generation;
use crate::pick::{PickRequest, Validity, validate};
use crate::words;

/// The items a drag from a pane carries, as they were when the press readied it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drag {
    /// What the drag's payload carries ([`AppState::drag_id`]).
    pub id: u64,
    /// The pane the rows are dragged from.
    pub side: usize,
    /// The items and the folder they are in, as a move: [`validate`] then refuses that folder
    /// too, which a drop of either kind does not take.
    pub request: PickRequest,
    /// Whether each folder hovered so far takes the items.
    takes: HashMap<VPath, bool>,
    /// The folder the last hover showed the items would go into; `None` after a hover where
    /// nothing takes them.
    shown: Option<VPath>,
}

/// A place a drag is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropSpot {
    /// Row `row` of the list of pane `side`, or the list below its rows (`None`). The row of a
    /// folder takes the items into that folder; any other row, and the list below them, into
    /// the folder the pane shows.
    Pane { side: usize, row: Option<usize> },
    /// Tab `index` of pane `side`: into its folder.
    Tab { side: usize, index: usize },
    /// Row `row` of the folder tree: into its folder.
    Tree(usize),
}

/// What a place a drag is over would do with the items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropOffer {
    /// The folder they would go into.
    pub folder: VPath,
    /// How many items the drag carries.
    pub count: usize,
}

/// A press on a selected row, whose click waits for the release: no drag came of it then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingClick {
    side: usize,
    /// The row's item, which the click finds by its name: rows may come and go meanwhile.
    name: OsString,
    ctrl: bool,
}

impl AppState {
    /// A left press on row `row` of pane `side`, with Ctrl or Shift held: the pane gets the
    /// keyboard and the row the cursor. On a row that is not selected, or with Shift, it is
    /// the click ([`AppState::click_row`]) at once; on a selected row the selection stays,
    /// for a drag of all of it, and the click waits for [`AppState::release_row`]. Either way
    /// the press readies a drag of the items that are then selected.
    pub fn press_row(&mut self, side: usize, row: usize, ctrl: bool, shift: bool) -> Vec<Effect> {
        self.activate(side);
        self.pending_click = None;
        // A row the window drew and the state no longer has readies no drag, rather than
        // leaving the last press's.
        self.drag = None;
        let (selection, rows) = self.tab_mut(side).selection_mut();
        let Some(entry) = rows.get(row) else {
            return Vec::new();
        };
        let effects = if selection.is_selected(&entry.name) && !shift {
            let name = entry.name.clone();
            selection.set_cursor(row, rows);
            self.pending_click = Some(PendingClick { side, name, ctrl });
            Vec::new()
        } else {
            self.click_row(side, row, ctrl, shift)
        };
        self.drag = self
            .pick_request(side, TransferMode::Move)
            .map(|request| Drag {
                id: next_generation(),
                side,
                request,
                takes: HashMap::new(),
                shown: None,
            });
        effects
    }

    /// The release of a press on row `row` of pane `side` that no drag came of: the click a
    /// press on a selected row left waiting is done now (the row alone is selected, or with
    /// Ctrl it is no longer selected).
    pub fn release_row(&mut self, side: usize, row: usize) -> Vec<Effect> {
        let Some(pending) = self.pending_click.take() else {
            return Vec::new();
        };
        let tab = self.tab(side);
        let pressed = tab
            .visible()
            .iter()
            .position(|entry| entry.name == pending.name);
        if pending.side != side || pressed != Some(row) {
            return Vec::new();
        }
        self.click_row(side, row, pending.ctrl, false)
    }

    /// The id the payload of a drag from the panes carries now: that of the drag the last
    /// press readied, or 0, which no drag has.
    pub fn drag_id(&self) -> u64 {
        self.drag.as_ref().map_or(0, |drag| drag.id)
    }

    /// The drag readied by the last press, if any.
    pub fn drag(&self) -> Option<&Drag> {
        self.drag.as_ref()
    }

    /// Whether the place `spot` takes the items of the drag whose payload carries `id`, and
    /// into which folder: never while a window shows over the main window or the app ends,
    /// nor for the payload of another drag. Found from the paths alone, so that it can answer
    /// at every move of the pointer. The drag notes the folder, which the window shows.
    pub fn drop_offer(&mut self, id: u64, spot: DropSpot) -> Option<DropOffer> {
        if self.modal.is_some() || self.quitting {
            return None;
        }
        let folder = self.spot_folder(spot);
        let drag = self.drag.as_mut().filter(|drag| drag.id == id && id != 0)?;
        drag.shown = None;
        let folder = folder?;
        let takes = *drag
            .takes
            .entry(folder.clone())
            .or_insert_with(|| validate(&drag.request, &folder, None) == Validity::Ok);
        if !takes {
            return None;
        }
        drag.shown = Some(folder.clone());
        Some(DropOffer {
            folder,
            count: drag.request.sources.len(),
        })
    }

    /// The drag whose payload carries `id` was dropped on `spot`, to `mode` its items: a
    /// place that takes them gets their transfer, which asks about a name that is taken, and
    /// the drag is over (one drop each). Elsewhere nothing happens. Nor where the rows changed
    /// under a still pointer since the last hover (a folder's watch), so that `spot` now holds
    /// another folder than the one the window showed: a notice says so, and the next hover
    /// shows where a drop goes.
    pub fn drop_items(&mut self, id: u64, spot: DropSpot, mode: TransferMode) -> Vec<Effect> {
        let shown = self.drag.as_ref().and_then(|drag| drag.shown.clone());
        let Some(offer) = self.drop_offer(id, spot) else {
            return Vec::new();
        };
        if shown.is_some_and(|shown| shown != offer.folder) {
            return vec![Effect::Notice(words::drop_moved().to_owned())];
        }
        let Some(drag) = self.drag.take() else {
            return Vec::new();
        };
        self.pending_click = None;
        let options = TransferOptions {
            conflict: ConflictPolicy::Ask,
            ..TransferOptions::default()
        };
        match TransferSpec::into_dir(mode, &drag.request.sources, &offer.folder, options) {
            Ok(spec) => vec![submit(JobSpec::Transfer(spec), Some(drag.side))],
            Err(error) => vec![Effect::Notice(words::cannot_transfer(error.kind()))],
        }
    }

    /// The folder a drop on `spot` puts the items into; `None` for a row or a tab that is not
    /// there (any more).
    fn spot_folder(&self, spot: DropSpot) -> Option<VPath> {
        match spot {
            DropSpot::Pane { side, row } => {
                let tab = self.panes.get(side)?.active_tab();
                match row.and_then(|row| tab.visible().get(row)) {
                    Some(entry) if entry.kind == EntryKind::Dir => {
                        tab.location().join(&entry.name).ok()
                    }
                    _ => Some(tab.location().clone()),
                }
            }
            DropSpot::Tab { side, index } => self
                .panes
                .get(side)?
                .tabs()
                .get(index)
                .map(|tab| tab.location().clone()),
            DropSpot::Tree(row) => {
                let id = self.tree.node_at(row)?;
                self.tree.node(id).map(|node| node.path.clone())
            }
        }
    }
}
