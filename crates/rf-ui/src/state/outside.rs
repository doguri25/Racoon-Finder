//! Files dragged from another app onto the window (DND-05 받기): File Explorer's, Finder's, or
//! winit's drop of files (Linux). A child of [`state`](super), so these methods see the fields of
//! [`AppState`].
//!
//! The system tells of the drag as it goes: where the pointer is, which the window turns into the
//! place under it ([`DropSpot`], the same as for a drag of rows), the keys held, what the drag
//! allows ([`Effects`]) and what it carries ([`DroppedItems`]). [`AppState::outside_over`] answers
//! at each move with what a drop there would do, for the system's pointer; the window shows what
//! [`AppState::outside_hover`] says, as it does for a drag of rows (the badge beside the pointer,
//! the frame of the place, the time of a rest, [`AppState::drag_rests`]).
//! [`AppState::outside_drop`] makes the drop, and [`AppState::outside_leave`] forgets the drag
//! that left the window.
//!
//! - **The keys are the system's.** winit forgets the modifier keys while another app has the
//!   focus, as it has during its drag, so the keys the system tells with each move choose, never
//!   [`AppState::modifiers_changed`]'s.
//! - **The paths** go through the clipboard's check (`local_paths`): a device's, a relative one,
//!   one that is not local is left out, and the drop tells how many, with the items the system
//!   itself left out ([`DroppedItems::skipped`], macOS's file URLs that name no path to read
//!   safely). A drag that carries nothing else is refused, and says why as it hovers.
//! - **Where they go** is the place's folder, as for a drag of rows: never one of them or a
//!   folder inside one, and the ones already in that folder stay where they are (DND-09), so a
//!   place where all of them are takes nothing.
//! - **What a drop does** is [`drop_rule::choose`](super::drop_rule::choose)'s: the keys, the
//!   setting, what the drag allows. The app's own job copies or moves, and asks about a name that
//!   is taken; the system is told what was done only as a mode (the controller's reply). A drop
//!   with the right button (File Explorer's) shows the menu of 놓을 때 묻기, as the setting 매번
//!   묻기 does. A move offers its undo once it is done (DND-07), as a dragged move does.
//! - **This app's own drag**, which went out of the window and came back through the system's
//!   drag (its id is [`DroppedItems::ours`]), is a drag of rows: the items of its press go, by
//!   [`AppState::drop_items`]'s way, with the keys the system tells.

use rf_core::VPath;
use rf_jobs::TransferMode;
use rf_platform::dnd::{DragMods, DropEffect, DroppedItems, Effects};

use super::clipboard::local_paths;
use super::drag::{AskedDrop, DropOffer, Hover, ask_menu, asked, dropped_effect};
use super::drop_rule::DropChoice;
use super::{AppState, DropSpot, Effect};
use crate::pick::Validity;
use crate::words;

/// A drag of another app over the window, as the system told of it last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Outside {
    /// What it carries: a drag that carries other items is another drag.
    items: DroppedItems,
    /// Its paths that the app takes, as local addresses.
    sources: Vec<VPath>,
    /// The keys held and what it allows at the last hover, which a rest checks again with.
    mods: DragMods,
    allowed: Effects,
    /// The folder the last hover showed the items would go into; `None` after a hover where
    /// nothing takes them. This app's own drag keeps it in its [`Drag`](super::Drag).
    shown: Option<VPath>,
}

impl AppState {
    /// A drag of another app carrying `items`, with the keys `mods` held and allowing `allowed`,
    /// hovers over `spot` (`None`: where nothing takes drops): what a drop there would do, for
    /// the system's pointer; `None` where it would do nothing (no place, a place that refuses the
    /// items, keys that ask for a link, a drag that allows nothing the drop would do, no path to
    /// take, while a window shows over the main one or the app ends). See
    /// [`AppState::outside_hover`].
    pub fn outside_over(
        &mut self,
        spot: Option<DropSpot>,
        items: &DroppedItems,
        mods: DragMods,
        allowed: Effects,
    ) -> Option<DropEffect> {
        self.outside_hover(spot, items, mods, allowed)
            .effect(allowed)
    }

    /// What the place `spot` makes of a drag of another app (see [`AppState::outside_over`]), for
    /// the window to show as it shows a drag of rows. The drag notes the folder it would go into,
    /// unless the keys or what it allows refuse the drop there, for a rest there and for its drop.
    pub fn outside_hover(
        &mut self,
        spot: Option<DropSpot>,
        items: &DroppedItems,
        mods: DragMods,
        allowed: Effects,
    ) -> Hover {
        self.note_outside(items, mods, allowed);
        if let Some(id) = self.ours(items.ours) {
            let Some(spot) = spot else {
                if let Some(drag) = self.drag.as_mut() {
                    drag.shown = None;
                }
                return Hover::Nothing;
            };
            return self.hover_with(id, spot, mods, allowed);
        }
        // A drag that names nothing of files is none the window knows; one whose items were
        // all left out (by the system, or by the check of the paths) is refused, saying why.
        let sources = self
            .outside
            .as_ref()
            .filter(|_| !items.paths.is_empty() || items.skipped > 0)
            .map(|outside| outside.sources.clone());
        let hover = if let (Some(spot), Some(sources)) = (spot, sources) {
            self.foreign_hover(&sources, spot, mods, allowed)
        } else {
            Hover::Nothing
        };
        let shown = match &hover {
            Hover::Takes(offer) if !matches!(offer.choice, DropChoice::Refuse(_)) => {
                Some(offer.folder.clone())
            }
            _ => None,
        };
        if let Some(outside) = self.outside.as_mut() {
            outside.shown = shown;
        }
        hover
    }

    /// A drag of another app carrying `items`, with the keys `mods` held and allowing `allowed`,
    /// was dropped on `spot`, with the right button (`right_button`) or not: what was done, for
    /// the system (`None` for nothing, and for the menu of 놓을 때 묻기, whose choice comes later),
    /// and the effects that do it. A place that takes the items gets their transfer as the keys,
    /// the setting and what the drag allows choose, or the menu, which a drop with the right
    /// button always shows; the items already in its folder stay out of it, and a notice tells of
    /// the paths left out. Nothing where the place refuses them or the keys refuse the drop, while
    /// a window shows over the main one or the app ends; nor where the rows changed under a still
    /// pointer since the last hover, so that `spot` holds another folder than the one the window
    /// showed (a notice says so, as for a drag of rows). The drag is over.
    pub fn outside_drop(
        &mut self,
        spot: Option<DropSpot>,
        items: DroppedItems,
        mods: DragMods,
        allowed: Effects,
        right_button: bool,
    ) -> (Option<DropEffect>, Vec<Effect>) {
        // What the hovers of this drag showed, if the system told of them.
        let hovered = self.outside.take().filter(|outside| outside.items == items);
        let DroppedItems {
            paths,
            ours,
            skipped,
        } = items;
        if let Some(id) = self.ours(ours) {
            let Some(spot) = spot else {
                return (None, Vec::new());
            };
            let effects = self.drop_with(id, spot, mods, allowed, right_button);
            return (dropped_effect(&effects), effects);
        }
        let Some(spot) = spot.filter(|_| self.modal.is_none() && !self.quitting) else {
            return (None, Vec::new());
        };
        let (sources, left_out) = local_paths(&paths);
        // The items the system left out (macOS) are told with the paths the app does not take.
        let left_out = left_out + skipped;
        if sources.is_empty() {
            let told = (left_out > 0).then(|| Effect::Notice(words::dropped_skipped(left_out)));
            return (None, told.into_iter().collect());
        }
        let shown = hovered.and_then(|outside| outside.shown);
        let offer = match self.foreign_hover(&sources, spot, mods, allowed) {
            Hover::Takes(offer) if shown.as_ref().is_none_or(|shown| *shown == offer.folder) => {
                offer
            }
            // The window showed a folder that takes them, and the place holds another one.
            _ if shown.is_some() => {
                return (None, vec![Effect::Notice(words::drop_moved().to_owned())]);
            }
            _ => return (None, Vec::new()),
        };
        let choice = if right_button {
            asked(allowed)
        } else {
            offer.choice
        };
        let side = match spot {
            DropSpot::Pane { side, .. } | DropSpot::Tab { side, .. } => side,
            DropSpot::Tree(_) => self.active,
        };
        let DropOffer { folder, .. } = offer;
        // The items already there stay where they are (DND-09).
        let sources: Vec<VPath> = sources
            .into_iter()
            .filter(|source| source.parent().as_ref() != Some(&folder))
            .collect();
        let mut effects = match choice {
            DropChoice::Do(DropEffect::Copy) => {
                self.drop_transfer(TransferMode::Copy, &sources, &folder, side)
            }
            DropChoice::Do(DropEffect::Move) => {
                self.drop_transfer(TransferMode::Move, &sources, &folder, side)
            }
            DropChoice::Ask => {
                self.asked_drop = Some(AskedDrop {
                    sources,
                    folder,
                    side,
                    allowed,
                });
                vec![ask_menu(allowed)]
            }
            // The keys refuse it (no links are made), or the drag allows nothing it would do.
            DropChoice::Do(DropEffect::Link) | DropChoice::Refuse(_) => return (None, Vec::new()),
        };
        let done = dropped_effect(&effects);
        if left_out > 0 {
            effects.push(Effect::Notice(words::dropped_left_out(left_out)));
        }
        (done, effects)
    }

    /// The drag of another app left the window without a drop: it is forgotten, and nothing it
    /// rested on opens.
    pub fn outside_leave(&mut self) {
        self.outside = None;
    }

    /// The folder the last hover of the drag over the window showed the items would go into: a
    /// drag of another app's while the system tells of one, else the drag readied last (this
    /// app's own through the system too).
    pub(super) fn shown_folder(&self) -> Option<VPath> {
        match &self.outside {
            Some(outside) if self.ours(outside.items.ours).is_none() => outside.shown.clone(),
            _ => self.drag.as_ref().and_then(|drag| drag.shown.clone()),
        }
    }

    /// Whether the place `spot` still takes the items of the drag over the window, with the keys
    /// and what it allowed at its last hover (see [`AppState::shown_folder`]).
    pub(super) fn still_takes(&mut self, spot: DropSpot) -> bool {
        let outside = self.outside.as_ref().map(|outside| {
            let sources = outside.sources.clone();
            (
                self.ours(outside.items.ours),
                outside.mods,
                outside.allowed,
                sources,
            )
        });
        match outside {
            Some((Some(id), mods, allowed, _)) => {
                self.offer_with(id, spot, mods, allowed).is_some()
            }
            Some((None, mods, allowed, sources)) => {
                matches!(
                    self.foreign_hover(&sources, spot, mods, allowed),
                    Hover::Takes(_)
                )
            }
            None => {
                let id = self.drag_id();
                self.drop_offer(id, spot).is_some()
            }
        }
    }

    /// The id of the drag readied last, when a drag of the system's is it: `ours` is the id the
    /// system's drag carries ([`DroppedItems::ours`]), for a drag of rows carried out of the
    /// window and back.
    fn ours(&self, ours: Option<u64>) -> Option<u64> {
        ours.filter(|&id| id != 0 && id == self.drag_id())
    }

    /// Notes the drag of another app that carries `items`: a new one, or the keys and what it
    /// allows now.
    fn note_outside(&mut self, items: &DroppedItems, mods: DragMods, allowed: Effects) {
        match self.outside.as_mut() {
            Some(outside) if outside.items == *items => {
                outside.mods = mods;
                outside.allowed = allowed;
            }
            _ => {
                let (sources, _) = local_paths(&items.paths);
                self.outside = Some(Outside {
                    items: items.clone(),
                    sources,
                    mods,
                    allowed,
                    shown: None,
                });
            }
        }
    }

    /// What the place `spot` makes of `sources`, the paths of another app's drag that the app
    /// takes, with the keys `mods` and a drag that `allowed` these.
    fn foreign_hover(
        &self,
        sources: &[VPath],
        spot: DropSpot,
        mods: DragMods,
        allowed: Effects,
    ) -> Hover {
        if self.modal.is_some() || self.quitting {
            return Hover::Nothing;
        }
        let Some(folder) = self.spot_folder(spot) else {
            return Hover::Nothing;
        };
        let Some(first) = sources.first() else {
            return Hover::NoPaths;
        };
        if let Some(refused) = refusal(sources, &folder) {
            return Hover::Refused(refused);
        }
        let choice = self.drop_choice(first, &folder, mods, allowed);
        Hover::Takes(DropOffer { folder, choice })
    }
}

/// Why `folder` does not take `sources`: one of them is it or holds it, or all of them are in it
/// already (DND-09: a drop back where they are does nothing).
fn refusal(sources: &[VPath], folder: &VPath) -> Option<Validity> {
    if sources.iter().any(|source| folder.starts_with(source)) {
        return Some(Validity::IntoSource);
    }
    sources
        .iter()
        .all(|source| source.parent().as_ref() == Some(folder))
        .then_some(Validity::SameFolder)
}
