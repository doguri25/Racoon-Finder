//! The tabs of the panes (NAV-01): duplicating, reopening, reordering, pinning, closing several
//! and moving one to the other pane, from a tab's menu, the keys, the 보기 menu's 탭 items or a
//! drag of the tab. A child of [`state`](super), so these methods see the fields of
//! [`AppState`].
//!
//! - **Reopening** brings back the tab its pane closed last, where it was, at its folder, with
//!   its history, order and pin ([`Pane::reopen`](crate::pane::Pane::reopen)). Each pane keeps
//!   its own last ten, and they go with it when the panes swap. A reopened tab, like a copy of a
//!   tab, is a new tab: it lists its folder under a new generation.
//! - **A pinned tab** stays at its folder: a move to another one (a folder of its list, a piece
//!   of the path row, a folder of the address bar's lists, a typed path, the tree, Up, Back,
//!   Forward, a link, the other pane's F9 or navigation sync) opens in a new tab beside it, which
//!   goes on from the pinned tab's history ([`AppState::navigate_shown`],
//!   [`AppState::move_shown`]). Its own folder is listed again in it. A folder that vanishes
//!   moves it as it moves any tab: there is nothing left to show.
//! - **Closing several** (the others, those to the right) spares the pinned tabs; closing one by
//!   name (Ctrl+W, 닫기 in its menu) does not. They are closed from the right, so that reopening
//!   them one by one puts each back in its place.
//! - **A tab that moves** (along its tab bar, or to the other pane) keeps its generation, so its
//!   listing and watch follow it. A pane's last tab stays. Moved to the other pane, it shows
//!   there; with two panes that pane gets the keyboard, with one (the other pane hidden) a notice
//!   says where the tab went. A tab that navigation sync waits for, moved away or closed, is
//!   waited for no more.

use rf_core::VPath;

use super::{AppState, Effect, RIGHT};
use crate::loader::LoadRequest;
use crate::pane::Tab;
use crate::words;

impl AppState {
    /// Opens a copy of tab `index` of pane `side` right after it, and shows it.
    pub fn duplicate_tab(&mut self, side: usize, index: usize) -> Vec<Effect> {
        self.activate(side);
        self.panes[side]
            .duplicate(index)
            .map(Effect::Load)
            .into_iter()
            .collect()
    }

    /// Opens the tab pane `side` closed last again (Ctrl+Shift+T); nothing when it closed none.
    pub fn reopen_tab(&mut self, side: usize) -> Vec<Effect> {
        self.activate(side);
        self.panes[side]
            .reopen()
            .map(Effect::Load)
            .into_iter()
            .collect()
    }

    /// Pins tab `index` of pane `side`, or unpins it.
    pub fn toggle_pin(&mut self, side: usize, index: usize) -> Vec<Effect> {
        self.activate(side);
        if let Some(tab) = self.panes[side].tabs_mut().get_mut(index) {
            let pinned = tab.pinned();
            tab.set_pinned(!pinned);
        }
        Vec::new()
    }

    /// Closes the tabs of pane `side` other than tab `index`, but for the pinned ones; tab
    /// `index` shows.
    pub fn close_other_tabs(&mut self, side: usize, index: usize) -> Vec<Effect> {
        let Some(kept) = self.close_where(side, index, |i| i != index) else {
            return Vec::new();
        };
        let pane = &mut self.panes[side];
        if let Some(at) = pane.position(kept) {
            pane.select_tab(at);
        }
        Vec::new()
    }

    /// Closes the tabs to the right of tab `index` of pane `side`, but for the pinned ones. The
    /// tab that showed keeps showing, or tab `index` when it went.
    pub fn close_tabs_right(&mut self, side: usize, index: usize) -> Vec<Effect> {
        let shown = self.panes[side].active_tab().id();
        let Some(kept) = self.close_where(side, index, |i| i > index) else {
            return Vec::new();
        };
        let pane = &mut self.panes[side];
        if let Some(at) = pane.position(shown).or_else(|| pane.position(kept)) {
            pane.select_tab(at);
        }
        Vec::new()
    }

    /// Moves tab `index` of pane `side` one place left (`delta` below 0) or right; nothing at
    /// either end (Ctrl+Shift+PageUp and Ctrl+Shift+PageDown move the tab that shows).
    pub fn shift_tab(&mut self, side: usize, index: usize, delta: isize) -> Vec<Effect> {
        self.activate(side);
        let slot = if delta < 0 {
            index.checked_sub(1)
        } else {
            Some(index + 2)
        };
        if let Some(slot) = slot {
            self.panes[side].move_tab(index, slot);
        }
        Vec::new()
    }

    /// Moves tab `index` of pane `side` to the other pane, after its tabs, where it shows. A
    /// pane's last tab stays, and a notice says so.
    pub fn move_tab_to_other(&mut self, side: usize, index: usize) -> Vec<Effect> {
        self.activate(side);
        let slot = self.panes[RIGHT - side].tabs().len();
        self.carry_tab(side, index, RIGHT - side, slot)
    }

    /// Whether a drop of the tab whose id is `id` (the payload of a drag of a tab) on `slot`
    /// of the tab bar of pane `side` (0 before its first tab, the number of its tabs after the
    /// last one) would move it: a place in its own bar that is not where it is, or any place in
    /// the other pane's bar when that pane shows and the tab is not its pane's last one. Never
    /// while a window shows over the main window or the app ends.
    pub fn tab_drop_offer(&self, id: u64, side: usize, slot: usize) -> bool {
        if self.modal.is_some() || self.quitting || side > RIGHT {
            return false;
        }
        let Some((from, index)) = self.find_tab(id) else {
            return false;
        };
        let count = self.panes[side].tabs().len();
        if from == side {
            slot <= count && slot != index && slot != index + 1
        } else {
            self.layout.shows(side, self.active)
                && self.panes[from].tabs().len() > 1
                && slot <= count
        }
    }

    /// The tab whose id is `id` was dropped on `slot` of the tab bar of pane `side`: it moves
    /// there when [`AppState::tab_drop_offer`] says it would.
    pub fn drop_tab(&mut self, id: u64, side: usize, slot: usize) -> Vec<Effect> {
        if !self.tab_drop_offer(id, side, slot) {
            return Vec::new();
        }
        let Some((from, index)) = self.find_tab(id) else {
            return Vec::new();
        };
        if from != side {
            return self.carry_tab(from, index, side, slot);
        }
        self.activate(side);
        self.panes[side].move_tab(index, slot);
        Vec::new()
    }

    /// Closes tab `index` of pane `side`, unless it is the pane's last one. Its load stops once
    /// the controller sees its generation is gone.
    pub(super) fn close_one(&mut self, side: usize, index: usize) {
        if let Some(closed) = self.panes[side].close_tab(index) {
            self.forget_sync_wait(closed.generation());
        }
    }

    /// The tab pane `side` shows goes to `folder`; the request for its listing. A pinned tab
    /// stays: a tab forked from it goes there, beside it, and shows. Its own folder is listed
    /// again in the tab itself.
    pub(super) fn navigate_shown(&mut self, side: usize, folder: VPath) -> Option<LoadRequest> {
        let index = self.panes[side].active();
        self.navigate_tab(side, index, folder)
    }

    /// Tab `index` of pane `side` goes to `folder`, as [`AppState::navigate_shown`]; the tab
    /// forked from a pinned one shows only when the pinned one did. `None` for a tab that is
    /// not there.
    pub(super) fn navigate_tab(
        &mut self,
        side: usize,
        index: usize,
        folder: VPath,
    ) -> Option<LoadRequest> {
        let tab = self.panes[side].tabs_mut().get_mut(index)?;
        if *tab.location() == folder {
            return Some(tab.navigate(folder));
        }
        self.go_or_fork(side, index, |tab| Some(tab.navigate(folder)))
    }

    /// Moves the tab pane `side` shows with `go` (a step back, forward or up), if it can go:
    /// a pinned tab stays, and a tab forked from it makes the move beside it, and shows.
    pub(super) fn move_shown(
        &mut self,
        side: usize,
        go: impl FnOnce(&mut Tab) -> Option<LoadRequest>,
    ) -> Option<LoadRequest> {
        let index = self.panes[side].active();
        self.go_or_fork(side, index, go)
    }

    /// Moves tab `index` of pane `side` with `go`, or, when it is pinned, a tab forked from it,
    /// which then goes right after it, and shows if the pinned one did. Nothing (and no new
    /// tab) when it cannot go.
    fn go_or_fork(
        &mut self,
        side: usize,
        index: usize,
        go: impl FnOnce(&mut Tab) -> Option<LoadRequest>,
    ) -> Option<LoadRequest> {
        let pane = &mut self.panes[side];
        let tab = pane.tabs_mut().get_mut(index)?;
        if !tab.pinned() {
            return go(tab);
        }
        let mut fork = tab.fork();
        let request = go(&mut fork)?;
        let shown = (pane.active() != index).then(|| pane.active_tab().id());
        pane.open_beside(index, fork);
        if let Some(at) = shown.and_then(|id| pane.position(id)) {
            pane.select_tab(at);
        }
        Some(request)
    }

    /// The pane and the position of the tab whose id is `id`.
    fn find_tab(&self, id: u64) -> Option<(usize, usize)> {
        self.panes
            .iter()
            .enumerate()
            .find_map(|(side, pane)| pane.position(id).map(|index| (side, index)))
    }

    /// Takes tab `index` of pane `from` to `slot` of the bar of pane `to`, where it shows. With
    /// two panes, `to` gets the keyboard; with one, a notice says the tab went to the hidden
    /// pane. A pane's last tab stays, and a notice says so.
    fn carry_tab(&mut self, from: usize, index: usize, to: usize, slot: usize) -> Vec<Effect> {
        let count = self.panes[from].tabs().len();
        if index >= count {
            return Vec::new();
        }
        let Some(tab) = self.panes[from].take_tab(index) else {
            return vec![Effect::Notice(words::last_tab_stays().to_owned())];
        };
        // It no longer is the other pane's tab that navigation sync waits for; and a press on
        // a row waiting for its release names a row of a tab that may have gone.
        self.forget_sync_wait(tab.generation());
        self.pending_click = None;
        self.panes[to].insert_tab(slot, tab);
        if self.layout.dual {
            self.activate(to);
            Vec::new()
        } else {
            vec![Effect::Notice(words::tab_moved_to_hidden().to_owned())]
        }
    }

    /// Closes the tabs `closes` names of pane `side` but for the pinned ones, from the right;
    /// the id of tab `index`, or `None` (and nothing closed) for a tab that is not there.
    fn close_where(
        &mut self,
        side: usize,
        index: usize,
        closes: impl Fn(usize) -> bool,
    ) -> Option<u64> {
        self.activate(side);
        let kept = self.panes[side].tabs().get(index)?.id();
        for i in (0..self.panes[side].tabs().len()).rev() {
            if closes(i) && !self.panes[side].tabs()[i].pinned() {
                self.close_one(side, i);
            }
        }
        Some(kept)
    }
}
