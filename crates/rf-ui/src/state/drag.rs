//! Dragging rows into another folder (DND, PRD 5.4): a press on a row of a pane readies a drag
//! of the items it would carry, the window asks while the drag hovers whether a place takes
//! them, and a drop there submits their move or copy at once, without the picker. A child of
//! [`state`](super), so these methods see the fields of [`AppState`].
//!
//! What a drop does is the state's to choose, not the window's (DND-02): the modifier keys held
//! ([`AppState::modifiers_changed`], which the window tells as they change), the setting of
//! 보기 ▸ 설정 ([`AppState::set_drag_default`]) and the drives of the sidebar (for the system's
//! way: a move within a drive, a copy to another) go into [`drop_rule::choose`], at every hover
//! and at the drop. The choice of 놓을 때 묻기 drops nothing yet: the menu shows where the drop
//! was, and the transfer it chooses goes the way a drop's would ([`AppState::drop_menu_chosen`]).
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
//!
//! A drag that rests on a place that takes it for [`SPRING_DELAY`] opens it (DND-08,
//! [`AppState::drag_rests`]): the controller's timer counts the rest, and the drag goes on. A
//! move a drop submits is noted with how many items it carries (`AppState::drop_transfer`),
//! so that its end offers its undo for [`MOVE_TOAST_TIME`] (DND-07, module `jobs`).
//!
//! A drag that leaves the window goes on as the system's drag to other apps (DND-05 보내기,
//! [`AppState::drag_leaves_window`]), once, with the local paths of its items and its id; it
//! stays readied, so that it is still this drag of rows should it come back over the window
//! (module `outside`). Its end ([`AppState::drag_out_ended`]) makes no job, whatever the app that
//! took it did: that app finishes a move, and the app deletes nothing.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rf_core::{EntryKind, Os, VPath};
use rf_jobs::{ConflictPolicy, JobSpec, TransferMode, TransferOptions, TransferSpec};
use rf_platform::dnd::{DragMods, DropEffect, Effects};

use super::drop_rule::{self, DropChoice};
use super::jobs::submit;
use super::{AppState, Effect, MenuShown, load};
use crate::jobs::JobEffect;
use crate::pane::next_generation;
use crate::pick::{PickRequest, Validity, validate};
use crate::places::PlaceKind;
use crate::settings::DragDefault;
use crate::words;

/// How long a drag rests on a place before the place opens ([`AppState::drag_rests`]).
pub const SPRING_DELAY: Duration = Duration::from_millis(1000);

/// How long the toast that offers to undo a dragged move shows (DND-07).
pub const MOVE_TOAST_TIME: Duration = Duration::from_secs(5);

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
    /// What each folder hovered so far makes of the items ([`validate`] without facts).
    takes: HashMap<VPath, Validity>,
    /// The folder the last hover showed the items would go into; `None` after a hover where
    /// nothing takes them.
    pub(super) shown: Option<VPath>,
    /// The drag went on out of the window as the system's drag
    /// ([`AppState::drag_leaves_window`]).
    out: bool,
}

/// A drag of rows that goes on out of the window as the system's drag (DND-05 보내기,
/// [`AppState::drag_leaves_window`]): the local paths of its items, which the system carries to
/// another app, and the drag's id, which the system's drag carries along, so that the window's
/// own drop target knows the drag should it come back over the window
/// ([`DroppedItems::ours`](rf_platform::dnd::DroppedItems::ours)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DragOut {
    pub paths: Vec<PathBuf>,
    pub ours: u64,
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
    /// What a drop there does, with the keys held now.
    pub choice: DropChoice,
}

/// A drop that waits for the menu of 놓을 때 묻기: the items the drag carried, the folder they
/// go into, the pane they came from (or that took them, for a drag of another app), and what
/// the drag allows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AskedDrop {
    pub(super) sources: Vec<VPath>,
    pub(super) folder: VPath,
    pub(super) side: usize,
    pub(super) allowed: Effects,
}

/// What a place a drag hovers over makes of it, for the window to show beside the pointer
/// (DND-03) and around the place, and for a rest there (DND-08): the same for a drag of rows and
/// for a drag of another app (module `outside`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hover {
    /// The place takes the items: a drop there does what the offer says, which is nothing for
    /// keys that ask for a link or for what the drag does not allow.
    Takes(DropOffer),
    /// The place does not take them, for this reason.
    Refused(Validity),
    /// The drag of another app carries paths, none of which is one to take (a device's, a
    /// relative one).
    NoPaths,
    /// Nothing here takes drops; or the drag is none the window knows, or a window shows over
    /// the main one.
    Nothing,
}

impl Hover {
    /// What a drop there would do, for the pointer of a drag that `allowed` these: `None` where it
    /// does nothing. The menu of 놓을 때 묻기 shows as a move where the drag allows one, else as a
    /// copy.
    pub fn effect(&self, allowed: Effects) -> Option<DropEffect> {
        let Hover::Takes(offer) = self else {
            return None;
        };
        match offer.choice {
            DropChoice::Do(effect @ (DropEffect::Copy | DropEffect::Move)) => Some(effect),
            DropChoice::Ask if allowed.contains(DropEffect::Move) => Some(DropEffect::Move),
            DropChoice::Ask => Some(DropEffect::Copy),
            DropChoice::Do(DropEffect::Link) | DropChoice::Refuse(_) => None,
        }
    }
}

/// What the effects of a drop did: the mode of the transfer they submit, or `None` (a notice,
/// the menu of 놓을 때 묻기, nothing).
pub fn dropped_effect(effects: &[Effect]) -> Option<DropEffect> {
    effects.iter().find_map(|effect| match effect {
        Effect::Job(JobEffect::Submit {
            spec: JobSpec::Transfer(transfer),
            ..
        }) => Some(match transfer.mode {
            TransferMode::Copy => DropEffect::Copy,
            TransferMode::Move => DropEffect::Move,
        }),
        _ => None,
    })
}

/// What a drop with the right button, or another drop that asks whatever the keys, does where
/// the drag `allowed` these: the menu of 놓을 때 묻기, where a copy or a move is allowed.
pub(super) fn asked(allowed: Effects) -> DropChoice {
    drop_rule::choose(
        Os::CURRENT,
        DragDefault::Ask,
        DragMods::default(),
        allowed,
        || false,
    )
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
        // A drop that waited for the menu of 놓을 때 묻기 is not made after another drag starts,
        // and no drag of another app is over the window while the button is down in it.
        self.asked_drop = None;
        self.outside = None;
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
                out: false,
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

    /// The modifier keys held now, as the window tells them at each change: the next hover and
    /// the next drop choose with them.
    pub fn modifiers_changed(&mut self, mods: DragMods) {
        self.drag_mods = mods;
    }

    /// What a drop does without a modifier key (보기 ▸ 설정 ▸ 끌어 놓기: …).
    pub fn drag_default(&self) -> DragDefault {
        self.drag_default
    }

    pub fn set_drag_default(&mut self, default: DragDefault) {
        self.drag_default = default;
    }

    /// Whether a move made by a drag offers its undo once it is done (보기 ▸ 설정, DND-07).
    pub fn move_undo_toast(&self) -> bool {
        self.move_undo_toast
    }

    pub fn set_move_undo_toast(&mut self, on: bool) {
        self.move_undo_toast = on;
    }

    /// Whether the place `spot` takes the items of the drag whose payload carries `id`, into
    /// which folder, and what a drop there does with the keys held now: never while a window
    /// shows over the main window or the app ends, nor for the payload of another drag. Found
    /// from the paths alone, so that it can answer at every move of the pointer. The drag notes
    /// the folder, which the window shows, unless the keys refuse the drop there.
    pub fn drop_offer(&mut self, id: u64, spot: DropSpot) -> Option<DropOffer> {
        self.offer_with(id, spot, self.drag_mods, Effects::COPY | Effects::MOVE)
    }

    /// What the place `spot` makes of the drag readied last, whose payload carries `id`, for the
    /// window to show ([`AppState::drop_offer`], else [`AppState::drop_refused`]).
    pub fn drop_hover(&mut self, id: u64, spot: DropSpot) -> Hover {
        self.hover_with(id, spot, self.drag_mods, Effects::COPY | Effects::MOVE)
    }

    /// [`AppState::drop_hover`] with the keys `mods` and a drag that `allowed` these: a drag of
    /// rows that went out of the window and came back through the system's drag tells its own
    /// (module `outside`).
    pub(super) fn hover_with(
        &mut self,
        id: u64,
        spot: DropSpot,
        mods: DragMods,
        allowed: Effects,
    ) -> Hover {
        match self.offer_with(id, spot, mods, allowed) {
            Some(offer) => Hover::Takes(offer),
            None => self
                .drop_refused(id, spot)
                .map_or(Hover::Nothing, Hover::Refused),
        }
    }

    /// [`AppState::drop_offer`] with the keys `mods` and a drag that `allowed` these: a drag of
    /// rows that went out of the window and came back through the system's drag tells its own
    /// (module `outside`).
    pub(super) fn offer_with(
        &mut self,
        id: u64,
        spot: DropSpot,
        mods: DragMods,
        allowed: Effects,
    ) -> Option<DropOffer> {
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
            .or_insert_with(|| validate(&drag.request, &folder, None));
        if takes != Validity::Ok {
            return None;
        }
        let choice = self.drop_choice(
            &self.drag.as_ref()?.request.from_dir,
            &folder,
            mods,
            allowed,
        );
        if !matches!(choice, DropChoice::Refuse(_))
            && let Some(drag) = self.drag.as_mut()
        {
            drag.shown = Some(folder.clone());
        }
        Some(DropOffer { folder, choice })
    }

    /// What a drop of items from `from` (their folder, or one of them) into `folder` does, with
    /// the keys `mods`, the setting, and a drag that `allowed` these (a drag of this window's
    /// allows a copy and a move). The drives are those of the sidebar, which a worker found (none
    /// known yet: another drive), looked at only for the system's way.
    pub(super) fn drop_choice(
        &self,
        from: &VPath,
        folder: &VPath,
        mods: DragMods,
        allowed: Effects,
    ) -> DropChoice {
        let same_volume = || {
            let drives: Vec<PathBuf> = self
                .places
                .iter()
                .filter(|place| place.kind == PlaceKind::Drive)
                .filter_map(|place| place.path.as_local().map(Path::to_path_buf))
                .collect();
            match (from.as_local(), folder.as_local()) {
                (Some(from), Some(folder)) => drop_rule::same_volume(from, folder, &drives),
                _ => false,
            }
        };
        drop_rule::choose(Os::CURRENT, self.drag_default, mods, allowed, same_volume)
    }

    /// Why the place `spot` does not take the items of the drag whose payload carries `id`,
    /// for the window to say while the drag hovers there: one of them, or a folder inside one
    /// ([`Validity::IntoSource`]), or the folder they are in ([`Validity::SameFolder`]).
    /// `None` where they would go, for a place that is no folder (any more), for the payload of
    /// another drag, and while a window shows over the main window or the app ends.
    pub fn drop_refused(&mut self, id: u64, spot: DropSpot) -> Option<Validity> {
        if self.modal.is_some() || self.quitting {
            return None;
        }
        let folder = self.spot_folder(spot)?;
        let drag = self.drag.as_mut().filter(|drag| drag.id == id && id != 0)?;
        let takes = *drag
            .takes
            .entry(folder.clone())
            .or_insert_with(|| validate(&drag.request, &folder, None));
        (takes != Validity::Ok).then_some(takes)
    }

    /// The drag whose payload carries `id` was dropped on `spot`: a place that takes them gets
    /// their transfer as the keys held and the setting choose ([`DropOffer::choice`]), which
    /// asks about a name that is taken, or the menu of 놓을 때 묻기 ([`MenuShown::AskDrop`]),
    /// and the drag is over (one drop each). Elsewhere, and where the keys refuse the drop,
    /// nothing happens. Nor where the rows changed under a still pointer since the last hover (a
    /// folder's watch), so that `spot` now holds another folder than the one the window showed,
    /// one that may not take them at all: a notice says so, and the next hover shows where a
    /// drop goes.
    pub fn drop_items(&mut self, id: u64, spot: DropSpot) -> Vec<Effect> {
        self.drop_with(
            id,
            spot,
            self.drag_mods,
            Effects::COPY | Effects::MOVE,
            false,
        )
    }

    /// [`AppState::drop_items`] with the keys `mods` and a drag that `allowed` these, which asks
    /// with the menu of 놓을 때 묻기 whatever the keys and the setting when `ask` (a drop with the
    /// right button): a drag of rows that comes back through the system's drag (module
    /// `outside`).
    pub(super) fn drop_with(
        &mut self,
        id: u64,
        spot: DropSpot,
        mods: DragMods,
        allowed: Effects,
        ask: bool,
    ) -> Vec<Effect> {
        let shown = self
            .drag
            .as_ref()
            .filter(|drag| drag.id == id && id != 0)
            .and_then(|drag| drag.shown.clone());
        let live = self.modal.is_none() && !self.quitting;
        let Some(offer) = self.offer_with(id, spot, mods, allowed) else {
            // The window showed a folder that takes them, and the place holds one that does not.
            return if shown.is_some() && live {
                vec![Effect::Notice(words::drop_moved().to_owned())]
            } else {
                Vec::new()
            };
        };
        if shown.is_some_and(|shown| shown != offer.folder) {
            return vec![Effect::Notice(words::drop_moved().to_owned())];
        }
        let choice = if ask { asked(allowed) } else { offer.choice };
        let mode = match choice {
            DropChoice::Do(DropEffect::Copy) => Some(TransferMode::Copy),
            DropChoice::Do(DropEffect::Move) => Some(TransferMode::Move),
            DropChoice::Ask => None,
            // The keys refuse it (no links are made).
            DropChoice::Do(DropEffect::Link) | DropChoice::Refuse(_) => return Vec::new(),
        };
        let Some(drag) = self.drag.take() else {
            return Vec::new();
        };
        self.pending_click = None;
        let Some(mode) = mode else {
            self.asked_drop = Some(AskedDrop {
                sources: drag.request.sources,
                folder: offer.folder,
                side: drag.side,
                allowed,
            });
            return vec![Effect::ShowMenu(MenuShown::AskDrop)];
        };
        self.drop_transfer(mode, &drag.request.sources, &offer.folder, drag.side)
    }

    /// The menu of 놓을 때 묻기 chose `mode` (여기로 이동, 여기로 복사), or nothing (취소, and Esc
    /// or a click beside the menu, which the window tells the same): the drop it showed for gets
    /// that transfer, as a drop that chose it at once would, or is never made. A drag of another
    /// app that does not allow a move copies for 여기로 이동 (DND-05), and one that does not allow
    /// a copy makes nothing of 여기로 복사, as the rule of a drop's action goes. Nothing while a
    /// window shows over the main window or the app ends, as for a drop; nor once a press has
    /// readied another drag. The drop is answered once.
    pub fn drop_menu_chosen(&mut self, mode: Option<TransferMode>) -> Vec<Effect> {
        let (Some(asked), Some(mode)) = (self.asked_drop.take(), mode) else {
            return Vec::new();
        };
        if self.modal.is_some() || self.quitting {
            return Vec::new();
        }
        let default = match mode {
            TransferMode::Move => DragDefault::Move,
            TransferMode::Copy => DragDefault::Copy,
        };
        let choice = drop_rule::choose(
            Os::CURRENT,
            default,
            DragMods::default(),
            asked.allowed,
            || false,
        );
        let mode = match choice {
            DropChoice::Do(DropEffect::Move) => TransferMode::Move,
            DropChoice::Do(DropEffect::Copy) => TransferMode::Copy,
            DropChoice::Do(DropEffect::Link) | DropChoice::Ask | DropChoice::Refuse(_) => {
                return Vec::new();
            }
        };
        self.drop_transfer(mode, &asked.sources, &asked.folder, asked.side)
    }

    /// The transfer that a drop of `sources`, dragged from pane `side`, into `folder` submits:
    /// it asks about a name that is taken. A move is noted with how many items it carries, so
    /// that its end offers its undo ([`AppState::track`], DND-07).
    pub(super) fn drop_transfer(
        &mut self,
        mode: TransferMode,
        sources: &[VPath],
        folder: &VPath,
        side: usize,
    ) -> Vec<Effect> {
        let options = TransferOptions {
            conflict: ConflictPolicy::Ask,
            ..TransferOptions::default()
        };
        match TransferSpec::into_dir(mode, sources, folder, options) {
            Ok(spec) => {
                let spec = JobSpec::Transfer(spec);
                if mode == TransferMode::Move {
                    self.drag_submits.push((spec.clone(), sources.len()));
                }
                vec![submit(spec, Some(side))]
            }
            Err(error) => vec![Effect::Notice(words::cannot_transfer(error.kind()))],
        }
    }

    /// The drag over the window (the one readied last, or a drag of another app while the system
    /// tells of one, module `outside`) has rested on `spot` for [`SPRING_DELAY`] (머무르면 열기,
    /// DND-08), as the controller's timer tells: the row of a folder opens that folder in its
    /// pane (in a new tab beside a pinned one), a tab shows, a closed folder of the tree opens.
    /// The drag goes on, and a drop goes by what shows then; the pane with the keys keeps them.
    ///
    /// Only the folder the last hover showed opens, the one the timer counted for: the rows under
    /// a still pointer may have changed since (a watch, a job that ended, a listing that came, a
    /// folder of the tree above that opened), and another folder at `spot` now opens nothing, as
    /// a drop there would not be made ([`AppState::drop_items`]). Nothing either where a drop
    /// would not take the items ([`AppState::drop_offer`] is `None`: one of them or a folder
    /// inside one, the folder they are in, while a window shows over the main window or the app
    /// ends), on a row that is no folder or below the rows, on a tab that shows already, or on a
    /// folder of the tree that is open or has no folders.
    pub fn drag_rests(&mut self, spot: DropSpot) -> Vec<Effect> {
        let shown = self.shown_folder();
        let Some(folder) = self
            .spot_folder(spot)
            .filter(|folder| Some(folder) == shown.as_ref())
        else {
            return Vec::new();
        };
        if !self.still_takes(spot) {
            return Vec::new();
        }
        match spot {
            // The folder of a row that is no folder's is the pane's own, which shows already.
            DropSpot::Pane { side, row: Some(_) } if folder != *self.tab(side).location() => {
                load(self.navigate_shown(side, folder))
            }
            DropSpot::Pane { .. } => Vec::new(),
            DropSpot::Tab { side, index } => {
                let pane = &mut self.panes[side];
                if pane.active() != index {
                    pane.select_tab(index);
                }
                Vec::new()
            }
            DropSpot::Tree(row) => {
                // A closed row with a chevron, as the sidebar shows it.
                let opens = self.tree.rows().get(row).is_some_and(|row| {
                    !row.expanded && (row.has_children_unknown || row.has_children)
                });
                match self.tree.node_at(row) {
                    Some(node) if opens => {
                        self.with_tree(|tree| tree.toggle(node).into_iter().collect())
                    }
                    _ => Vec::new(),
                }
            }
        }
    }

    /// The drag of rows under way left the window, and the system carries it on to other apps
    /// (DND-05 보내기): the local paths of its items and its id, for the system's drag. `None`,
    /// and the drag stays the window's, for no drag readied (or one dropped already), for a drag
    /// that went out once already (never twice), for one of items that have no local path, and
    /// while a window shows over the main one or the app ends. A drag that goes out makes no
    /// click of its press ([`AppState::release_row`]).
    ///
    /// The drag stays readied under its id: the system's drag may come back over the window,
    /// where it is this drag of rows ([`AppState::outside_drop`]), until it ends
    /// ([`AppState::drag_out_ended`]).
    pub fn drag_leaves_window(&mut self) -> Option<DragOut> {
        if self.modal.is_some() || self.quitting {
            return None;
        }
        let drag = self.drag.as_mut().filter(|drag| !drag.out)?;
        let paths: Vec<PathBuf> = drag
            .request
            .sources
            .iter()
            .filter_map(|source| source.as_local().map(Path::to_path_buf))
            .collect();
        if paths.is_empty() {
            return None;
        }
        drag.out = true;
        let ours = drag.id;
        self.pending_click = None;
        Some(DragOut { paths, ours })
    }

    /// The system's drag that the drag of rows `ours` went on as
    /// ([`AppState::drag_leaves_window`], [`DragOut::ours`]) is over, and the app that took it
    /// did `done` (`None`: nothing). A drop back on the window, which made its own job then, is
    /// what that drop answered (`None` for a move on Windows; a copy, or on macOS the move its
    /// generic operation reads as). The app does nothing of it, whatever it was (a move
    /// included): it deletes nothing, since a move is the other app's to finish, and an app that
    /// only copied leaves a copy; the watch of a folder shows what changed. The drag is over.
    /// Nothing for a drag that never went out, nor for the end of an earlier drag, which comes
    /// late: a drag readied since stays.
    pub fn drag_out_ended(&mut self, ours: u64, done: Option<DropEffect>) -> Vec<Effect> {
        let _ = done;
        if self
            .drag
            .as_ref()
            .is_some_and(|drag| drag.out && drag.id == ours)
        {
            self.drag = None;
        }
        Vec::new()
    }

    /// The folder a drop on `spot` puts the items into; `None` for a row or a tab that is not
    /// there (any more).
    pub(super) fn spot_folder(&self, spot: DropSpot) -> Option<VPath> {
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
