//! The main window's state, and what its keys, clicks and the loader's events do to it.
//!
//! [`AppState`] is plain data on the UI thread: the two panes, which one has the keyboard and
//! whether hidden items show. Its methods change it and return the [`Effect`]s that need a
//! worker thread or the screen, which the [`Controller`](crate::controller::Controller)
//! carries out: a listing to start, a link to follow or a vanished folder's nearest parent to
//! find, a file to open, a notice, a row to scroll to. Keeping the two apart lets the tests
//! drive the state without a window.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use rf_core::{ErrorKind, VPath};
use rf_vfs::{SortKey, VisibilityFilter};

use crate::keys::{Command, Mods};
use crate::loader::{Followed, LoadEvent, LoadRequest};
use crate::pane::{Handled, OpenAction, Pane, Tab, ancestors};
use crate::words;

/// The left pane's position in [`AppState::panes`].
pub const LEFT: usize = 0;
/// The right pane's position in [`AppState::panes`].
pub const RIGHT: usize = 1;

/// The shortest time between two automatic reloads of a folder whose watch ended while the
/// folder is still there, so that a watch that keeps failing cannot reload it in a loop.
pub const AUTO_RELOAD_GAP: Duration = Duration::from_secs(5);

/// What a change of the [`AppState`] needs done outside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// List and watch a tab's folder ([`Loader::request`](crate::loader::Loader::request)).
    Load(LoadRequest),
    /// The watch of the tab with `generation` ended: find on a worker thread where to go
    /// ([`Loader::nearest_existing_parent`](crate::loader::Loader::nearest_existing_parent) of
    /// `path`, after [`VANISHED_SETTLE`](crate::controller::VANISHED_SETTLE)), then call
    /// [`AppState::found_existing`].
    FindExisting { generation: u64, path: VPath },
    /// The row opened in the tab with `generation` is a link: follow it on a worker thread
    /// ([`Loader::follow`](crate::loader::Loader::follow)), then hand the result, which
    /// carries `generation`, to [`AppState::followed`].
    Follow { generation: u64, link: VPath },
    /// Open the file with the system's default app.
    OpenFile(VPath),
    /// The folder of the tab with `generation` came back after its watch ended, within
    /// [`AUTO_RELOAD_GAP`] of its last automatic reload: call [`AppState::reload_due`] after
    /// `after` (a one-shot timer on the UI thread).
    ReloadLater {
        generation: u64,
        path: VPath,
        after: Duration,
    },
    /// Show a notice in the corner of the window.
    Notice(String),
    /// Show the path of pane `side` as text to edit.
    EditPath(usize),
    /// Scroll pane `side` so that `row` shows.
    ScrollTo { side: usize, row: usize },
}

/// The window's state on the UI thread: two panes, the one with the keyboard, and whether
/// hidden items show.
#[derive(Debug)]
pub struct AppState {
    panes: [Pane; 2],
    active: usize,
    show_hidden: bool,
    /// Where a tab goes when its folder and every parent of it are gone.
    home: Option<VPath>,
    /// When each folder was last reloaded because its watch ended ([`AUTO_RELOAD_GAP`]).
    auto_reloads: HashMap<VPath, Instant>,
    /// The tabs, by generation, with a reload waiting for its gap ([`Effect::ReloadLater`]):
    /// one at most each.
    pending_reloads: HashSet<u64>,
}

impl AppState {
    /// The left pane at `left`, the right one at `right`, the left one active, and the
    /// requests for their listings.
    pub fn new(left: VPath, right: VPath, home: Option<VPath>) -> (AppState, Vec<Effect>) {
        let filter = VisibilityFilter::default();
        let (left_pane, left_request) = Pane::new(left, filter);
        let (right_pane, right_request) = Pane::new(right, filter);
        let state = AppState {
            panes: [left_pane, right_pane],
            active: LEFT,
            show_hidden: filter.show_hidden,
            home,
            auto_reloads: HashMap::new(),
            pending_reloads: HashSet::new(),
        };
        (
            state,
            vec![Effect::Load(left_request), Effect::Load(right_request)],
        )
    }

    pub fn panes(&self) -> &[Pane; 2] {
        &self.panes
    }

    /// The pane with the keyboard: [`LEFT`] or [`RIGHT`].
    pub fn active(&self) -> usize {
        self.active
    }

    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// The generation of every tab's last request: the loads still worth running.
    pub fn generations(&self) -> HashSet<u64> {
        self.panes
            .iter()
            .flat_map(Pane::tabs)
            .map(Tab::generation)
            .collect()
    }

    /// Gives pane `side` the keyboard.
    pub fn activate(&mut self, side: usize) {
        self.active = side.min(RIGHT);
    }

    /// Carries out the shortcut `command`, held with `mods`, on the active pane. `page` is how
    /// many rows `PageUp` and `PageDown` move. `None` for a command this window does not take
    /// (yet): the key goes on to the system.
    pub fn key(
        &mut self,
        command: Command,
        mods: Mods,
        page: usize,
        now: Instant,
    ) -> Option<Vec<Effect>> {
        let side = self.active;
        let page = isize::try_from(page.max(1)).unwrap_or(isize::MAX);
        let effects = match command {
            Command::SwitchPane => {
                self.active = RIGHT - side;
                Vec::new()
            }
            Command::Open => match self.tab(side).selection().cursor() {
                Some(row) => self.open_row(side, row),
                None => Vec::new(),
            },
            Command::Up => self.go(side, Tab::go_up),
            Command::Back => self.go(side, Tab::go_back),
            Command::Forward => self.go(side, Tab::go_forward),
            Command::CursorUp => self.move_cursor(side, -1, mods.shift),
            Command::CursorDown => self.move_cursor(side, 1, mods.shift),
            Command::CursorHome => self.move_cursor(side, isize::MIN, mods.shift),
            Command::CursorEnd => self.move_cursor(side, isize::MAX, mods.shift),
            Command::PageUp => self.move_cursor(side, -page, mods.shift),
            Command::PageDown => self.move_cursor(side, page, mods.shift),
            Command::ToggleSelect => {
                let (selection, rows) = self.tab_mut(side).selection_mut();
                if let Some(cursor) = selection.cursor() {
                    selection.toggle(cursor, rows);
                }
                Vec::new()
            }
            Command::SelectAll => {
                let (selection, rows) = self.tab_mut(side).selection_mut();
                selection.select_all(rows);
                Vec::new()
            }
            Command::ClearSelection => {
                self.tab_mut(side).selection_mut().0.clear();
                Vec::new()
            }
            Command::EditPath => vec![Effect::EditPath(side)],
            Command::NewTab => self.add_tab(side),
            Command::CloseTab => {
                let tab = self.panes[side].active();
                self.close_tab(side, tab)
            }
            Command::ToggleHidden => {
                self.toggle_hidden();
                Vec::new()
            }
            Command::TypeToSelect(text) => self
                .tab_mut(side)
                .type_to_select(&text, now)
                .map(|row| Effect::ScrollTo { side, row })
                .into_iter()
                .collect(),
            // Commands that change files run through the job engine, which this window does
            // not have yet; their keys are left to the system.
            Command::Rename
            | Command::Trash
            | Command::DeletePermanently
            | Command::CopyTo
            | Command::MoveTo
            | Command::NewFolder
            | Command::Undo
            | Command::Redo => return None,
        };
        Some(effects)
    }

    /// A click on row `row` of pane `side`: Ctrl flips the row, Shift selects the range from
    /// the anchor, a plain click selects the row alone. The pane gets the keyboard.
    pub fn click_row(&mut self, side: usize, row: usize, ctrl: bool, shift: bool) -> Vec<Effect> {
        self.activate(side);
        let (selection, rows) = self.tab_mut(side).selection_mut();
        if shift {
            selection.extend_to(row, rows);
        } else if ctrl {
            selection.toggle(row, rows);
        } else {
            selection.click(row, rows);
        }
        Vec::new()
    }

    /// Opens row `row` of pane `side` (Enter or a double click): a folder shows in the tab, a
    /// file opens in its app, and a link is followed first.
    pub fn open_row(&mut self, side: usize, row: usize) -> Vec<Effect> {
        self.activate(side);
        let tab = self.tab_mut(side);
        match tab.open_target(row) {
            Some(OpenAction::Navigate(folder)) => vec![Effect::Load(tab.navigate(folder))],
            Some(OpenAction::OpenFile(file)) => vec![Effect::OpenFile(file)],
            Some(OpenAction::Follow(link)) => vec![Effect::Follow {
                generation: tab.generation(),
                link,
            }],
            None => Vec::new(),
        }
    }

    /// Shows tab `index` of pane `side`.
    pub fn select_tab(&mut self, side: usize, index: usize) -> Vec<Effect> {
        self.activate(side);
        self.panes[side].select_tab(index);
        Vec::new()
    }

    /// Closes tab `index` of pane `side`, unless it is the pane's last one.
    pub fn close_tab(&mut self, side: usize, index: usize) -> Vec<Effect> {
        self.activate(side);
        // Its load stops once the controller sees its generation is gone.
        drop(self.panes[side].close_tab(index));
        Vec::new()
    }

    /// Opens a tab in pane `side` at the folder the pane shows.
    pub fn add_tab(&mut self, side: usize) -> Vec<Effect> {
        self.activate(side);
        let here = self.tab(side).location().clone();
        vec![Effect::Load(self.panes[side].add_tab(here))]
    }

    /// Goes to piece `index` of the path row of pane `side` (0 is the root).
    pub fn open_crumb(&mut self, side: usize, index: usize) -> Vec<Effect> {
        self.activate(side);
        let Some(folder) = ancestors(self.tab(side).location()).into_iter().nth(index) else {
            return Vec::new();
        };
        vec![Effect::Load(self.tab_mut(side).navigate(folder))]
    }

    /// Goes to the path typed into the path row of pane `side`. Text that is no path gives
    /// a notice and changes nothing.
    pub fn enter_path(&mut self, side: usize, text: &str) -> Vec<Effect> {
        self.activate(side);
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        match VPath::parse(text) {
            Ok(folder) => vec![Effect::Load(self.tab_mut(side).navigate(folder))],
            Err(_) => vec![Effect::Notice(words::cannot_open(ErrorKind::InvalidPath))],
        }
    }

    /// Sorts pane `side` by `key`, the other way round when it already sorts by it.
    pub fn sort(&mut self, side: usize, key: SortKey) -> Vec<Effect> {
        self.activate(side);
        self.tab_mut(side).set_sort(key);
        Vec::new()
    }

    /// Lists the folder of pane `side` again ("다시 시도").
    pub fn retry(&mut self, side: usize) -> Vec<Effect> {
        self.activate(side);
        vec![Effect::Load(self.tab_mut(side).reload())]
    }

    /// Shows `folder` in the active pane (a sidebar place).
    pub fn go_to(&mut self, folder: VPath) -> Vec<Effect> {
        let side = self.active;
        vec![Effect::Load(self.tab_mut(side).navigate(folder))]
    }

    /// Takes an event of the loader. A rescan lists the folder again; a vanished folder is
    /// looked for on a worker thread. The ticket of a vanished folder stays: its listing, if
    /// still running, still comes.
    pub fn loaded(&mut self, event: LoadEvent) -> Vec<Effect> {
        let Some(side) = self.side_of(event.generation()) else {
            return Vec::new();
        };
        match self.panes[side].handle(event) {
            Handled::Stale | Handled::Applied { .. } => Vec::new(),
            Handled::Reload { request, .. } => vec![Effect::Load(request)],
            Handled::Vanished {
                generation, path, ..
            } => vec![Effect::FindExisting { generation, path }],
        }
    }

    /// Takes where the tab with `generation`, whose folder `path` vanished, can go: `found`,
    /// the nearest folder of `path` that is still there, or home when none is. A tab that has
    /// moved on since takes nothing. When `found` is `path` itself (the folder was replaced,
    /// or only its watch failed), the folder is listed again; within [`AUTO_RELOAD_GAP`] of
    /// its last automatic reload, that waits for the gap to pass ([`Effect::ReloadLater`]),
    /// so a watch that keeps failing cannot reload it in a loop and a folder replaced twice
    /// in quick succession still shows the second one. Otherwise the tab goes to `found`.
    pub fn found_existing(
        &mut self,
        generation: u64,
        path: &VPath,
        found: Option<VPath>,
        now: Instant,
    ) -> Vec<Effect> {
        let Some(target) = found.or_else(|| self.home.clone()) else {
            return Vec::new();
        };
        if target == *path {
            if self.tab_by_generation(generation).is_none() {
                return Vec::new();
            }
            let since = self
                .auto_reloads
                .get(path)
                .map(|&at| now.saturating_duration_since(at))
                .filter(|&since| since < AUTO_RELOAD_GAP);
            if let Some(since) = since {
                if !self.pending_reloads.insert(generation) {
                    return Vec::new();
                }
                return vec![Effect::ReloadLater {
                    generation,
                    path: path.clone(),
                    after: AUTO_RELOAD_GAP.saturating_sub(since),
                }];
            }
            return self.auto_reload(generation, path, now);
        }
        match self.tab_by_generation(generation) {
            Some(tab) => vec![Effect::Load(tab.navigate(target))],
            None => Vec::new(),
        }
    }

    /// The time of the reload [`Effect::ReloadLater`] asked for has come: the tab with
    /// `generation` lists its folder `path` again, unless it has moved on since.
    pub fn reload_due(&mut self, generation: u64, path: &VPath, now: Instant) -> Vec<Effect> {
        if !self.pending_reloads.remove(&generation) {
            return Vec::new();
        }
        self.auto_reload(generation, path, now)
    }

    /// Lists the folder `path` of the tab with `generation` again after its watch ended, and
    /// notes when; nothing for a tab that has moved on.
    fn auto_reload(&mut self, generation: u64, path: &VPath, now: Instant) -> Vec<Effect> {
        self.pending_reloads.remove(&generation);
        let Some(tab) = self.tab_by_generation(generation) else {
            return Vec::new();
        };
        let request = tab.reload();
        self.auto_reloads
            .retain(|_, &mut at| now.saturating_duration_since(at) < AUTO_RELOAD_GAP);
        self.auto_reloads.insert(path.clone(), now);
        vec![Effect::Load(request)]
    }

    /// Takes what following a link gave ([`Loader::follow`](crate::loader::Loader::follow)).
    /// Only the tab that still has the generation the result carries takes it: a tab that has
    /// moved on since takes nothing.
    pub fn followed(&mut self, followed: Followed) -> Vec<Effect> {
        let Followed { generation, result } = followed;
        let Some(tab) = self.tab_by_generation(generation) else {
            return Vec::new();
        };
        match result {
            Ok(OpenAction::Navigate(folder)) => vec![Effect::Load(tab.navigate(folder))],
            Ok(OpenAction::OpenFile(file)) => vec![Effect::OpenFile(file)],
            // Following never gives another link to follow.
            Ok(OpenAction::Follow(_)) => Vec::new(),
            Err(kind) => vec![Effect::Notice(words::cannot_open(kind))],
        }
    }

    fn tab(&self, side: usize) -> &Tab {
        self.panes[side].active_tab()
    }

    fn tab_mut(&mut self, side: usize) -> &mut Tab {
        self.panes[side].active_tab_mut()
    }

    fn tab_by_generation(&mut self, generation: u64) -> Option<&mut Tab> {
        self.panes
            .iter_mut()
            .find_map(|pane| pane.tab_by_generation_mut(generation))
    }

    /// The pane one of whose tabs made the request `generation`.
    fn side_of(&self, generation: u64) -> Option<usize> {
        self.panes
            .iter()
            .position(|pane| pane.tabs().iter().any(|tab| tab.generation() == generation))
    }

    /// Moves the active tab of pane `side` with `step` (back, forward, up), if it can go.
    fn go(&mut self, side: usize, step: fn(&mut Tab) -> Option<LoadRequest>) -> Vec<Effect> {
        step(self.tab_mut(side))
            .map(Effect::Load)
            .into_iter()
            .collect()
    }

    fn move_cursor(&mut self, side: usize, delta: isize, extend: bool) -> Vec<Effect> {
        let (selection, rows) = self.tab_mut(side).selection_mut();
        selection.move_cursor(delta, extend, rows);
        selection
            .cursor()
            .map(|row| Effect::ScrollTo { side, row })
            .into_iter()
            .collect()
    }

    /// Shows or hides the hidden items in every tab of both panes.
    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        for pane in &mut self.panes {
            for tab in pane.tabs_mut() {
                tab.set_show_hidden(self.show_hidden);
            }
        }
    }
}

/// The folders the panes start at: the left one at `start` (made absolute against
/// `current_dir`) or else home, the right one at the documents folder or else home. Without
/// a home folder, `root` stands in. `None` when there is no folder at all.
pub fn start_folders(
    start: Option<&Path>,
    current_dir: Option<&Path>,
    home: Option<&VPath>,
    documents: Option<&VPath>,
    root: Option<&VPath>,
) -> Option<(VPath, VPath)> {
    let fallback = home.or(root)?;
    let left = start
        .and_then(|path| match current_dir {
            Some(dir) if path.is_relative() => VPath::local(dir.join(path)).ok(),
            _ => VPath::local(path).ok(),
        })
        .unwrap_or_else(|| fallback.clone());
    let right = documents.unwrap_or(fallback).clone();
    Some((left, right))
}

#[cfg(test)]
mod tests {
    use rf_core::{Attributes, Entry, EntryKind};
    use rf_vfs::SortOrder;
    use slint::platform::Key;

    use super::*;
    use crate::keys;

    fn folder(name: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(name)).expect("a local path")
    }

    fn file(name: &str) -> Entry {
        Entry::new(name, EntryKind::File)
    }

    fn dir(name: &str) -> Entry {
        Entry::new(name, EntryKind::Dir)
    }

    fn loads(effects: &[Effect]) -> Vec<&LoadRequest> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Load(request) => Some(request),
                _ => None,
            })
            .collect()
    }

    /// The one request among `effects`.
    fn the_load(effects: &[Effect]) -> LoadRequest {
        match loads(effects).as_slice() {
            [request] => (*request).clone(),
            other => panic!("expected one load, got {other:?} in {effects:?}"),
        }
    }

    /// A window state whose left pane at `a` lists `left` and whose right pane at `b` lists
    /// `right`.
    fn ready(left: Vec<Entry>, right: Vec<Entry>) -> AppState {
        let (mut state, effects) = AppState::new(folder("a"), folder("b"), Some(folder("home")));
        let requests = loads(&effects);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, folder("a"));
        assert_eq!(requests[1].path, folder("b"));
        for (request, entries) in requests.into_iter().zip([left, right]) {
            let generation = request.generation;
            let effects = state.loaded(LoadEvent::Listed {
                generation,
                entries,
            });
            assert_eq!(effects, []);
        }
        state
    }

    fn rows(state: &AppState, side: usize) -> Vec<&str> {
        state.panes()[side]
            .active_tab()
            .visible()
            .iter()
            .map(|entry| entry.name.to_str().expect("test names are text"))
            .collect()
    }

    fn selected(state: &AppState, side: usize) -> Vec<usize> {
        let tab = state.panes()[side].active_tab();
        tab.selection().indices(tab.visible())
    }

    fn cursor(state: &AppState, side: usize) -> Option<usize> {
        state.panes()[side].active_tab().selection().cursor()
    }

    fn press(state: &mut AppState, text: &str, mods: Mods) -> Option<Vec<Effect>> {
        let command = keys::command_for(text, mods).expect("a shortcut");
        state.key(command, mods, 10, Instant::now())
    }

    fn special(key: Key) -> String {
        char::from(key).to_string()
    }

    const NONE: Mods = Mods {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const SHIFT: Mods = Mods {
        shift: true,
        ..NONE
    };
    const CTRL: Mods = Mods { ctrl: true, ..NONE };
    const ALT: Mods = Mods { alt: true, ..NONE };

    #[test]
    fn startup_lists_both_panes() {
        let state = ready(vec![dir("x"), file("y")], vec![file("z")]);
        assert_eq!(state.active(), LEFT);
        assert_eq!(rows(&state, LEFT), ["x", "y"]);
        assert_eq!(rows(&state, RIGHT), ["z"]);
        assert_eq!(state.generations().len(), 2);
    }

    #[test]
    fn tab_switches_panes_and_clicks_activate() {
        let mut state = ready(vec![file("x")], vec![file("z")]);
        assert_eq!(press(&mut state, &special(Key::Tab), NONE), Some(vec![]));
        assert_eq!(state.active(), RIGHT);
        press(&mut state, &special(Key::Tab), SHIFT);
        assert_eq!(state.active(), LEFT);
        state.click_row(RIGHT, 0, false, false);
        assert_eq!(state.active(), RIGHT);
        assert_eq!(selected(&state, RIGHT), [0]);
    }

    #[test]
    fn keys_move_the_cursor_and_select() {
        let names = ["a", "b", "c", "d", "e"];
        let mut state = ready(names.iter().copied().map(file).collect(), vec![]);
        let down = special(Key::DownArrow);
        assert_eq!(
            press(&mut state, &down, NONE),
            Some(vec![Effect::ScrollTo { side: LEFT, row: 0 }])
        );
        press(&mut state, &down, NONE);
        assert_eq!(
            (cursor(&state, LEFT), selected(&state, LEFT)),
            (Some(1), vec![1])
        );
        // Shift extends from the anchor.
        press(&mut state, &down, SHIFT);
        press(&mut state, &down, SHIFT);
        assert_eq!(selected(&state, LEFT), [1, 2, 3]);
        // Ctrl+Space flips the cursor's row; Esc clears; Ctrl+A selects all.
        press(&mut state, &special(Key::Space), CTRL);
        assert_eq!(selected(&state, LEFT), [1, 2]);
        press(&mut state, &special(Key::Escape), NONE);
        assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
        assert_eq!(cursor(&state, LEFT), Some(3));
        press(&mut state, "a", CTRL);
        assert_eq!(selected(&state, LEFT), [0, 1, 2, 3, 4]);
        // Home, End and the page keys stop at the ends.
        press(&mut state, &special(Key::Home), NONE);
        assert_eq!(cursor(&state, LEFT), Some(0));
        assert_eq!(
            press(&mut state, &special(Key::PageDown), NONE),
            Some(vec![Effect::ScrollTo { side: LEFT, row: 4 }])
        );
        press(&mut state, &special(Key::End), SHIFT);
        assert_eq!(selected(&state, LEFT), [4]);
        // Clicks: Ctrl adds, Shift takes the range from the anchor.
        state.click_row(LEFT, 1, false, false);
        state.click_row(LEFT, 3, true, false);
        assert_eq!(selected(&state, LEFT), [1, 3]);
        state.click_row(LEFT, 4, false, true);
        assert_eq!(selected(&state, LEFT), [3, 4]);
    }

    #[test]
    fn typing_jumps_to_a_name() {
        let mut state = ready(vec![file("apple"), file("banana"), file("berry")], vec![]);
        let now = Instant::now();
        let effects = state.key(Command::TypeToSelect("be".into()), NONE, 10, now);
        assert_eq!(effects, Some(vec![Effect::ScrollTo { side: LEFT, row: 2 }]));
        assert_eq!(selected(&state, LEFT), [2]);
        // No match moves nothing, and the key is still used.
        let effects = state.key(
            Command::TypeToSelect("x".into()),
            NONE,
            10,
            now + Duration::from_secs(5),
        );
        assert_eq!(effects, Some(vec![]));
        assert_eq!(selected(&state, LEFT), [2]);
    }

    #[test]
    fn open_follows_the_row_kind() {
        let link = Entry::new("latest", EntryKind::Symlink);
        let mut state = ready(vec![dir("sub"), file("notes.txt"), link], vec![]);
        let generation = state.panes()[LEFT].active_tab().generation();

        assert_eq!(
            state.open_row(LEFT, 1),
            [Effect::OpenFile(
                folder("a").join("notes.txt").expect("a name")
            )]
        );
        assert_eq!(
            state.open_row(LEFT, 2),
            [Effect::Follow {
                generation,
                link: folder("a").join("latest").expect("a name"),
            }]
        );
        // Enter opens the cursor's row: the folder shows in the tab.
        state.click_row(LEFT, 0, false, false);
        let effects = press(&mut state, &special(Key::Return), NONE).expect("used");
        let request = the_load(&effects);
        assert_eq!(request.path, folder("a").join("sub").expect("a name"));
        assert_ne!(request.generation, generation);
        // Back and Up go to `a` again.
        let effects = press(&mut state, &special(Key::LeftArrow), ALT).expect("used");
        assert_eq!(the_load(&effects).path, folder("a"));
        let effects = press(&mut state, &special(Key::Backspace), NONE).expect("used");
        let temp = VPath::local(std::env::temp_dir()).expect("a local path");
        assert_eq!(the_load(&effects).path, temp);
    }

    #[test]
    fn a_late_link_result_is_dropped() {
        let mut state = ready(vec![Entry::new("latest", EntryKind::Symlink)], vec![]);
        let effects = state.open_row(LEFT, 0);
        let Effect::Follow { generation, link } = effects[0].clone() else {
            panic!("a link is followed: {effects:?}");
        };
        // The user moves on before the link's target is known.
        state.go_to(folder("elsewhere"));
        let late = state.followed(Followed {
            generation,
            result: Ok(OpenAction::Navigate(link.clone())),
        });
        assert_eq!(late, []);
        assert_eq!(
            state.panes()[LEFT].active_tab().location(),
            &folder("elsewhere")
        );

        // In time, a link to a folder shows it, one to a file opens it, and a broken one
        // gives a notice.
        let generation = state.panes()[LEFT].active_tab().generation();
        assert_eq!(
            state.followed(Followed {
                generation,
                result: Ok(OpenAction::OpenFile(link.clone())),
            }),
            [Effect::OpenFile(link.clone())]
        );
        assert_eq!(
            state.followed(Followed {
                generation,
                result: Err(ErrorKind::NotFound),
            }),
            [Effect::Notice(
                "열 수 없습니다: 찾을 수 없습니다".to_owned()
            )]
        );
        let effects = state.followed(Followed {
            generation,
            result: Ok(OpenAction::Navigate(link.clone())),
        });
        assert_eq!(the_load(&effects).path, link);
    }

    #[test]
    fn a_vanished_folder_goes_to_the_nearest_parent() {
        let mut state = ready(vec![file("x")], vec![]);
        let generation = state.panes()[LEFT].active_tab().generation();
        let effects = state.loaded(LoadEvent::Vanished { generation });
        assert_eq!(
            effects,
            [Effect::FindExisting {
                generation,
                path: folder("a"),
            }]
        );
        // The ticket stays until the tab moves on: a listing still running still comes.
        assert!(state.generations().contains(&generation));

        let parent = folder("a").parent().expect("a parent");
        let effects = state.found_existing(
            generation,
            &folder("a"),
            Some(parent.clone()),
            Instant::now(),
        );
        assert_eq!(the_load(&effects).path, parent);
        // A second answer for the same generation is late.
        assert_eq!(
            state.found_existing(generation, &folder("a"), Some(parent), Instant::now()),
            []
        );
    }

    #[test]
    fn a_replaced_folder_is_listed_again_but_not_in_a_loop() {
        let mut state = ready(vec![file("old")], vec![]);
        let now = Instant::now();
        let generation = state.panes()[LEFT].active_tab().generation();
        state.loaded(LoadEvent::Vanished { generation });
        let effects = state.found_existing(generation, &folder("a"), Some(folder("a")), now);
        let request = the_load(&effects);
        assert_eq!(request.path, folder("a"));
        // A reload keeps the rows until the new listing comes.
        assert_eq!(rows(&state, LEFT), ["old"]);

        // The new watch ends at once again: no second reload within the gap, but one when it
        // has passed.
        state.loaded(LoadEvent::Vanished {
            generation: request.generation,
        });
        let soon = now + Duration::from_secs(1);
        assert_eq!(
            state.found_existing(request.generation, &folder("a"), Some(folder("a")), soon),
            [Effect::ReloadLater {
                generation: request.generation,
                path: folder("a"),
                after: AUTO_RELOAD_GAP.saturating_sub(Duration::from_secs(1)),
            }]
        );
        // An answer after the gap reloads at once, and the waiting reload then has nothing to do.
        let later = now + AUTO_RELOAD_GAP + Duration::from_secs(1);
        let effects =
            state.found_existing(request.generation, &folder("a"), Some(folder("a")), later);
        assert_eq!(the_load(&effects).path, folder("a"));
        assert_eq!(
            state.reload_due(request.generation, &folder("a"), later),
            []
        );
    }

    #[test]
    fn a_folder_replaced_twice_quickly_is_listed_once_more_after_the_gap() {
        let mut state = ready(vec![file("first")], vec![]);
        let now = Instant::now();
        let a = folder("a");
        // The first replacement reloads at once.
        let generation = state.panes()[LEFT].active_tab().generation();
        state.loaded(LoadEvent::Vanished { generation });
        let first = the_load(&state.found_existing(generation, &a, Some(a.clone()), now));

        // The second, a second later, waits for the rest of the gap; asking again adds nothing.
        state.loaded(LoadEvent::Vanished {
            generation: first.generation,
        });
        let soon = now + Duration::from_secs(1);
        let later = vec![Effect::ReloadLater {
            generation: first.generation,
            path: a.clone(),
            after: AUTO_RELOAD_GAP.saturating_sub(Duration::from_secs(1)),
        }];
        assert_eq!(
            state.found_existing(first.generation, &a, Some(a.clone()), soon),
            later
        );
        assert_eq!(
            state.found_existing(first.generation, &a, Some(a.clone()), soon),
            []
        );

        // When the gap has passed, exactly one more reload comes.
        let due = now + AUTO_RELOAD_GAP;
        let second = the_load(&state.reload_due(first.generation, &a, due));
        assert_eq!(second.path, a);
        assert_ne!(second.generation, first.generation);
        assert_eq!(state.reload_due(first.generation, &a, due), []);
        assert_eq!(
            state.panes()[LEFT].active_tab().generation(),
            second.generation
        );
    }

    #[test]
    fn a_waiting_reload_of_a_tab_that_moved_on_does_nothing() {
        let mut state = ready(vec![file("x")], vec![]);
        let now = Instant::now();
        let a = folder("a");
        let generation = state.panes()[LEFT].active_tab().generation();
        let first = the_load(&state.found_existing(generation, &a, Some(a.clone()), now));
        let soon = now + Duration::from_secs(1);
        let effects = state.found_existing(first.generation, &a, Some(a.clone()), soon);
        assert!(matches!(effects.as_slice(), [Effect::ReloadLater { .. }]));
        // The user opens another folder before the gap has passed.
        state.go_to(folder("elsewhere"));
        assert_eq!(
            state.reload_due(first.generation, &a, now + AUTO_RELOAD_GAP),
            []
        );
        assert_eq!(
            state.panes()[LEFT].active_tab().location(),
            &folder("elsewhere")
        );
    }

    #[test]
    fn a_folder_with_nothing_left_goes_home() {
        let mut state = ready(vec![], vec![]);
        let generation = state.panes()[RIGHT].active_tab().generation();
        let effects = state.found_existing(generation, &folder("b"), None, Instant::now());
        assert_eq!(the_load(&effects).path, folder("home"));
        assert_eq!(
            state.panes()[RIGHT].active_tab().location(),
            &folder("home")
        );
    }

    #[test]
    fn hidden_items_toggle_in_every_tab() {
        let mut dot = file(".dot");
        dot.attributes |= Attributes::HIDDEN;
        let mut state = ready(vec![dot.clone(), file("x")], vec![dot, file("y")]);
        assert_eq!(rows(&state, LEFT), ["x"]);
        assert_eq!(press(&mut state, "h", CTRL), Some(vec![]));
        assert!(state.show_hidden());
        assert_eq!(rows(&state, LEFT), [".dot", "x"]);
        assert_eq!(rows(&state, RIGHT), [".dot", "y"]);
        // A new tab takes the setting.
        let request = the_load(&press(&mut state, "t", CTRL).expect("used"));
        state.loaded(LoadEvent::Listed {
            generation: request.generation,
            entries: vec![{
                let mut dot = file(".new");
                dot.attributes |= Attributes::HIDDEN;
                dot
            }],
        });
        assert_eq!(rows(&state, LEFT), [".new"]);
        press(&mut state, "h", CTRL);
        assert_eq!(rows(&state, LEFT), Vec::<&str>::new());
    }

    #[test]
    fn tabs_open_show_and_close() {
        let mut state = ready(vec![file("x")], vec![]);
        let first = state.panes()[LEFT].active_tab().generation();
        let effects = state.add_tab(LEFT);
        let request = the_load(&effects);
        assert_eq!(request.path, folder("a"));
        assert_eq!(state.panes()[LEFT].tabs().len(), 2);
        assert_eq!(state.panes()[LEFT].active(), 1);
        state.select_tab(LEFT, 0);
        assert_eq!(state.panes()[LEFT].active(), 0);
        // Ctrl+W closes the tab that shows; the last tab stays.
        press(&mut state, "w", CTRL);
        assert_eq!(state.panes()[LEFT].tabs().len(), 1);
        assert!(!state.generations().contains(&first));
        assert!(state.generations().contains(&request.generation));
        press(&mut state, "w", CTRL);
        assert_eq!(state.panes()[LEFT].tabs().len(), 1);
        // A click on the other pane's tab makes that pane active.
        state.close_tab(RIGHT, 0);
        assert_eq!(state.active(), RIGHT);
    }

    #[test]
    fn crumbs_and_typed_paths_navigate() {
        let mut state = ready(vec![], vec![]);
        let chain = ancestors(&folder("a"));
        assert_eq!(chain.last(), Some(&folder("a")));
        assert_eq!(chain[0].parent(), None);
        let effects = state.open_crumb(LEFT, 0);
        assert_eq!(the_load(&effects).path, chain[0]);
        assert_eq!(state.open_crumb(LEFT, 99), []);

        let typed = std::env::temp_dir().join("typed");
        let effects = state.enter_path(RIGHT, &format!("  {}  ", typed.display()));
        assert_eq!(the_load(&effects).path, folder("typed"));
        assert_eq!(state.active(), RIGHT);
        assert_eq!(state.enter_path(RIGHT, "   "), []);
        assert_eq!(
            state.enter_path(RIGHT, "relative/path"),
            [Effect::Notice(
                "열 수 없습니다: 쓸 수 없는 이름이나 경로입니다".to_owned()
            )]
        );
        assert_eq!(
            press(&mut state, "l", CTRL),
            Some(vec![Effect::EditPath(RIGHT)])
        );
    }

    #[test]
    fn sort_and_retry_act_on_the_clicked_pane() {
        // The loader sends a listing sorted by name.
        let mut state = ready(vec![file("a"), file("b")], vec![]);
        assert_eq!(rows(&state, LEFT), ["a", "b"]);
        state.sort(LEFT, SortKey::Name);
        assert_eq!(rows(&state, LEFT), ["b", "a"]);
        let sort = state.panes()[LEFT].active_tab().sort();
        assert_eq!(sort.primary.order, SortOrder::Descending);
        let effects = state.retry(RIGHT);
        assert_eq!(the_load(&effects).path, folder("b"));
        assert_eq!(state.active(), RIGHT);
    }

    #[test]
    fn file_commands_are_left_to_the_system() {
        let mut state = ready(vec![file("x")], vec![]);
        for (text, mods) in [
            (special(Key::F2), NONE),
            (special(Key::Delete), NONE),
            (special(Key::Delete), SHIFT),
            (special(Key::F5), NONE),
            (special(Key::F6), NONE),
            (special(Key::F7), NONE),
            ("z".to_owned(), CTRL),
            ("y".to_owned(), CTRL),
        ] {
            assert_eq!(press(&mut state, &text, mods), None, "{text:?}");
        }
        assert_eq!(rows(&state, LEFT), ["x"]);
    }

    #[test]
    fn late_events_of_a_closed_tab_are_dropped() {
        let mut state = ready(vec![file("x")], vec![]);
        let request = the_load(&state.add_tab(LEFT));
        state.close_tab(LEFT, 1);
        let effects = state.loaded(LoadEvent::Listed {
            generation: request.generation,
            entries: vec![file("late")],
        });
        assert_eq!(effects, []);
        assert_eq!(rows(&state, LEFT), ["x"]);
        assert_eq!(
            state.found_existing(request.generation, &folder("a"), None, Instant::now()),
            []
        );
    }

    #[test]
    fn start_folders_fall_back_to_home() {
        let (home, docs, root) = (folder("home"), folder("docs"), folder("root"));
        let cwd = std::env::temp_dir();
        assert_eq!(
            start_folders(None, Some(&cwd), Some(&home), Some(&docs), Some(&root)),
            Some((home.clone(), docs.clone()))
        );
        assert_eq!(
            start_folders(
                Some(Path::new("rel")),
                Some(&cwd),
                Some(&home),
                None,
                Some(&root)
            ),
            Some((folder("rel"), home.clone()))
        );
        let absolute = std::env::temp_dir().join("abs");
        assert_eq!(
            start_folders(Some(&absolute), None, Some(&home), None, None),
            Some((folder("abs"), home))
        );
        assert_eq!(
            start_folders(None, None, None, None, Some(&root)),
            Some((root.clone(), root))
        );
        assert_eq!(start_folders(None, None, None, None, None), None);
    }
}
