//! The state of the folder panes: tabs, their listings, sort, filter and selection.
//!
//! Everything here is plain data that the UI thread owns; nothing touches the file system.
//! A tab asks for its folder with a [`LoadRequest`], which the controller hands to
//! [`Loader::request`](crate::loader::Loader::request), and takes the [`LoadEvent`]s that come
//! back through [`Pane::handle`] (or the `apply_*` methods of [`Tab`]).
//!
//! Every request has a new generation number, unique in the process. A tab keeps the
//! generation of its last request and drops every event of another one, so a late listing,
//! change or free-space figure of a folder it has left never reaches the folder it shows now,
//! and an event of a closed tab never reaches another tab.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use rf_core::{Entry, EntryKind, ErrorKind, VPath};
use rf_vfs::{
    Change, SortBy, SortKey, SortOrder, SortSpec, VisibilityFilter, compare_entries, sort_entries,
};

use crate::history::History;
use crate::loader::{LoadEvent, LoadRequest};
use crate::selection::Selection;
use crate::words;

/// How long after a key the next one still adds to the type-to-select prefix (NAV-14).
pub const TYPE_TO_SELECT_WINDOW: Duration = Duration::from_secs(1);

/// A generation that no request has had yet.
fn next_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, AtomicOrdering::Relaxed)
}

/// Where a tab's listing stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadStatus {
    /// A request is out and its listing has not come. After a reload the old rows still
    /// show meanwhile; after a move to another folder there are none.
    Loading,
    /// The rows are the folder's listing, kept up to date by its watch.
    Ready,
    /// The folder could not be listed; the tab shows no rows.
    Failed(ErrorKind),
}

/// What opening a row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenAction {
    /// The row is a folder: show it in the tab ([`Tab::navigate`]).
    Navigate(VPath),
    /// The row is a file: open it with the app the system picks for it.
    OpenFile(VPath),
    /// The row is a link, and only `Provider::stat_target`, which the UI thread does not call,
    /// tells whether it leads to a folder. The controller hands it to
    /// [`Loader::follow`](crate::loader::Loader::follow) with the tab's generation; the
    /// result (`Navigate` for a link to a folder, `OpenFile` for anything else) comes back with
    /// that generation, and only a tab that still has it
    /// ([`Pane::tab_by_generation_mut`]) takes it, so a slow link cannot move a tab the user
    /// has moved on from.
    Follow(VPath),
}

/// One folder tab: where it is, what it lists, in which order, and what is selected.
///
/// The fields change only through the methods, which keep them consistent: `visible` is
/// `all` without the items the filter hides, both in the order of `sort`, and the selection
/// and the cursor refer to `visible`.
///
/// It is not `Clone`: a copy would share the tab's generation and take its events too.
#[derive(Debug)]
pub struct Tab {
    location: VPath,
    history: History,
    /// Every item of the folder, sorted by `sort`.
    all: Vec<Entry>,
    /// The items of `all` that `filter` shows, in the same order: the rows on screen.
    visible: Vec<Entry>,
    sort: SortSpec,
    filter: VisibilityFilter,
    selection: Selection,
    generation: u64,
    status: LoadStatus,
    /// The bytes the user can still write on the folder's volume, once known.
    free: Option<u64>,
    /// The type-to-select prefix and when its last key came.
    type_buffer: (String, Instant),
    /// The batches of the listing in progress ([`Tab::apply_batch`]).
    incoming: Vec<Entry>,
    /// Changes the watch reported before the listing came; they go on top of it.
    pending: Vec<Change>,
    /// The order the listing in progress was asked for in.
    requested_sort: SortSpec,
}

impl Tab {
    /// A tab that is about to show `location`, and the request for its listing.
    pub fn new(location: VPath, sort: SortSpec, filter: VisibilityFilter) -> (Tab, LoadRequest) {
        let mut tab = Tab {
            location: location.clone(),
            history: History::default(),
            all: Vec::new(),
            visible: Vec::new(),
            sort,
            filter,
            selection: Selection::default(),
            generation: 0,
            status: LoadStatus::Loading,
            free: None,
            type_buffer: (String::new(), Instant::now()),
            incoming: Vec::new(),
            pending: Vec::new(),
            requested_sort: sort,
        };
        let request = tab.load(location);
        (tab, request)
    }

    /// The folder the tab shows, or is loading.
    pub fn location(&self) -> &VPath {
        &self.location
    }

    /// The folders Back and Forward go to.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Every item of the folder, hidden ones too, in the tab's order.
    pub fn all(&self) -> &[Entry] {
        &self.all
    }

    /// The rows on screen: the items the filter shows, in the tab's order.
    pub fn visible(&self) -> &[Entry] {
        &self.visible
    }

    /// The order of the rows.
    pub fn sort(&self) -> SortSpec {
        self.sort
    }

    /// Which hidden and system items show.
    pub fn filter(&self) -> VisibilityFilter {
        self.filter
    }

    /// The selected rows, the cursor and the anchor, as positions in [`Tab::visible`].
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// The selection to change, with the rows it refers to: a click is
    /// `let (selection, rows) = tab.selection_mut(); selection.click(i, rows);`.
    pub fn selection_mut(&mut self) -> (&mut Selection, &[Entry]) {
        (&mut self.selection, &self.visible)
    }

    /// The generation of the tab's last request; events of any other are dropped.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Where the listing stands.
    pub fn status(&self) -> LoadStatus {
        self.status
    }

    /// The bytes the user can still write on the folder's volume, once known.
    pub fn free(&self) -> Option<u64> {
        self.free
    }

    /// Goes to the folder `to` and leaves the current one on the Back list. The rows, the
    /// selection and the free space are cleared until the listing of `to` comes.
    ///
    /// Going to the folder the tab is at is a [`Tab::reload`] and adds nothing to the
    /// history.
    pub fn navigate(&mut self, to: VPath) -> LoadRequest {
        if to == self.location {
            return self.reload();
        }
        self.history.visit(self.location.clone());
        self.load(to)
    }

    /// Lists the folder again. The rows and the selection stay until the new listing
    /// replaces them, which keeps the selection of the rows still there.
    pub fn reload(&mut self) -> LoadRequest {
        self.start_request()
    }

    /// Goes to the folder before this one; `None` when there is none.
    pub fn go_back(&mut self) -> Option<LoadRequest> {
        let previous = self.history.go_back(self.location.clone())?;
        Some(self.load(previous))
    }

    /// Goes to the folder after this one; `None` when there is none.
    pub fn go_forward(&mut self) -> Option<LoadRequest> {
        let next = self.history.go_forward(self.location.clone())?;
        Some(self.load(next))
    }

    /// Goes to the parent folder, like [`Tab::navigate`]; `None` at a root.
    pub fn go_up(&mut self) -> Option<LoadRequest> {
        let parent = self.location.parent()?;
        Some(self.navigate(parent))
    }

    /// Takes one batch of the listing of request `generation`, unsorted. The rows change only
    /// at [`Tab::finish`], which sorts and filters the whole listing once. `false`, and no
    /// change, for a batch of another request or one that comes after the listing ended.
    pub fn apply_batch(&mut self, generation: u64, entries: Vec<Entry>) -> bool {
        if !self.is_loading(generation) {
            return false;
        }
        self.incoming.extend(entries);
        true
    }

    /// Ends the listing of request `generation`: with `Ok`, the batches become the rows
    /// (sorted here, on the calling thread); with `Err`, the tab shows that error and no rows.
    /// `false`, and no change, for another request or a listing that has ended already.
    pub fn finish(&mut self, generation: u64, result: Result<(), ErrorKind>) -> bool {
        if !self.is_loading(generation) {
            return false;
        }
        match result {
            Ok(()) => {
                let mut entries = std::mem::take(&mut self.incoming);
                sort_entries(&mut entries, &self.sort);
                self.show_listing(entries);
            }
            Err(kind) => {
                self.incoming.clear();
                self.pending.clear();
                self.all.clear();
                self.status = LoadStatus::Failed(kind);
                self.refresh_visible();
            }
        }
        true
    }

    /// Takes the whole listing of request `generation`, sorted by the worker in the order
    /// the request asked for, which keeps a long sort off the UI thread. It is sorted again
    /// here only if the tab's order has changed since. `false`, and no change, for another
    /// request or a listing that has ended already.
    pub fn apply_sorted(&mut self, generation: u64, mut entries: Vec<Entry>) -> bool {
        if !self.is_loading(generation) {
            return false;
        }
        if self.requested_sort != self.sort {
            sort_entries(&mut entries, &self.sort);
        }
        self.incoming.clear();
        self.show_listing(entries);
        true
    }

    /// Takes changes the watch of request `generation` reported. A new or changed item is
    /// put where the order puts it (in place of the item of the same name, if any), a removed
    /// one goes, and a renamed one goes under its old name and comes under its new one. The
    /// selection and the cursor stay on the names that are still there.
    ///
    /// Changes that come before the listing are kept and go on top of it once it comes.
    /// `false`, and no change, for changes of another request.
    pub fn apply_changes(&mut self, generation: u64, changes: Vec<Change>) -> bool {
        if generation != self.generation {
            return false;
        }
        match self.status {
            LoadStatus::Loading => self.pending.extend(changes),
            LoadStatus::Ready => {
                merge_changes(&mut self.all, changes, self.sort);
                self.refresh_visible();
            }
            // There are no rows to change.
            LoadStatus::Failed(_) => {}
        }
        true
    }

    /// Takes the free space of the volume of request `generation`. `false`, and no change,
    /// for another request.
    pub fn apply_space(&mut self, generation: u64, available: u64) -> bool {
        if generation != self.generation {
            return false;
        }
        self.free = Some(available);
        true
    }

    /// Sorts by `key`: ascending for a new key, the other way round for the key that sorts
    /// now (a second click on a column head). Folders stay first.
    pub fn set_sort(&mut self, key: SortKey) {
        let order = match self.sort.primary {
            SortBy {
                key: current,
                order: SortOrder::Ascending,
            } if current == key => SortOrder::Descending,
            _ => SortOrder::Ascending,
        };
        self.sort.primary = SortBy { key, order };
        sort_entries(&mut self.all, &self.sort);
        self.refresh_visible();
    }

    /// Shows or hides the hidden items. The selection keeps the rows that still show.
    pub fn set_show_hidden(&mut self, show: bool) {
        self.filter.show_hidden = show;
        self.refresh_visible();
    }

    /// What opening row `i` does: a folder is shown in the tab, a link is followed, and
    /// anything else is opened as a file. `None` for a row past the end.
    pub fn open_target(&self, i: usize) -> Option<OpenAction> {
        let entry = self.visible.get(i)?;
        let path = self.location.join(&entry.name).ok()?;
        Some(match entry.kind {
            EntryKind::Dir => OpenAction::Navigate(path),
            EntryKind::Symlink => OpenAction::Follow(path),
            EntryKind::File | EntryKind::Other => OpenAction::OpenFile(path),
        })
    }

    /// The status bar: `항목 9개 · 선택 2개 (1.4 MB) · 남은 공간 212 GB`. The selection part
    /// shows only with a selection, and its size, the sum of the selected files (folders and
    /// links have none), only when a file is selected. The free space shows once known.
    pub fn status_line(&self) -> String {
        let selected = self.selection.indices(&self.visible);
        let files: Vec<&Entry> = selected
            .iter()
            .map(|&i| &self.visible[i])
            .filter(|entry| entry.kind == EntryKind::File)
            .collect();
        let bytes = (!files.is_empty()).then(|| files.iter().map(|entry| entry.size).sum());
        words::status_line(self.visible.len(), selected.len(), bytes, self.free)
    }

    /// Moves the cursor to the first row whose name starts with what was typed, ignoring
    /// case, and selects that row alone (NAV-14). Keys typed within
    /// [`TYPE_TO_SELECT_WINDOW`] of the last one add to the prefix; after a pause `text`
    /// starts a new one, and a prefix of spaces alone does nothing. Returns the row moved
    /// to, or `None` (and no change) when no name matches.
    pub fn type_to_select(&mut self, text: &str, now: Instant) -> Option<usize> {
        let (prefix, last) = &mut self.type_buffer;
        if now.saturating_duration_since(*last) > TYPE_TO_SELECT_WINDOW {
            prefix.clear();
        }
        *last = now;
        prefix.push_str(text);
        if prefix.trim().is_empty() {
            prefix.clear();
            return None;
        }
        let wanted = prefix.to_lowercase();
        let row = self.visible.iter().position(|entry| {
            entry
                .name
                .to_string_lossy()
                .to_lowercase()
                .starts_with(&wanted)
        })?;
        self.selection.click(row, &self.visible);
        Some(row)
    }

    /// Whether the listing of request `generation` is still awaited.
    fn is_loading(&self, generation: u64) -> bool {
        generation == self.generation && self.status == LoadStatus::Loading
    }

    /// Moves to `to` without touching the history: no rows, no selection, no free space.
    fn load(&mut self, to: VPath) -> LoadRequest {
        self.location = to;
        self.all.clear();
        self.visible.clear();
        self.selection = Selection::default();
        self.free = None;
        self.start_request()
    }

    /// A request with a new generation for the tab's folder; the rows stay as they are.
    fn start_request(&mut self) -> LoadRequest {
        self.generation = next_generation();
        self.status = LoadStatus::Loading;
        self.incoming.clear();
        self.pending.clear();
        self.requested_sort = self.sort;
        LoadRequest {
            generation: self.generation,
            path: self.location.clone(),
            sort: self.sort,
        }
    }

    /// Makes the sorted `entries`, with the changes that came before them, the rows.
    fn show_listing(&mut self, entries: Vec<Entry>) {
        self.all = entries;
        let pending = std::mem::take(&mut self.pending);
        if !pending.is_empty() {
            merge_changes(&mut self.all, pending, self.sort);
        }
        self.status = LoadStatus::Ready;
        self.refresh_visible();
    }

    /// Filters `all` into `visible` again and carries the selection over.
    fn refresh_visible(&mut self) {
        let filter = self.filter;
        let old = std::mem::take(&mut self.visible);
        self.visible = self
            .all
            .iter()
            .filter(|entry| filter.is_visible(entry))
            .cloned()
            .collect();
        self.selection.reconcile(&old, &self.visible);
    }
}

/// Applies `changes`, in their order, to `entries`, which are sorted by `sort` and stay so.
///
/// The rows that go are dropped in one pass; the rows that come are sorted among themselves
/// and merged in with one more pass over the old rows, a binary search each finding where
/// they go. For `n` rows and `k` changes that is about `n + k log n` steps, so a burst of
/// changes costs little more than a single one.
fn merge_changes(entries: &mut Vec<Entry>, changes: Vec<Change>, sort: SortSpec) {
    // The names whose current row goes, and the rows that come, by name (a later change of a
    // name replaces an earlier one).
    let mut gone: HashSet<OsString> = HashSet::new();
    let mut arrived: HashMap<OsString, Entry> = HashMap::new();
    for change in changes {
        match change {
            Change::Upsert(entry) => {
                gone.insert(entry.name.clone());
                arrived.insert(entry.name.clone(), entry);
            }
            Change::Removed(name) => {
                arrived.remove(&name);
                gone.insert(name);
            }
            Change::Renamed { from, to } => {
                arrived.remove(&from);
                gone.insert(from);
                // An item that was there under the new name has been replaced.
                gone.insert(to.name.clone());
                arrived.insert(to.name.clone(), to);
            }
        }
    }
    if !gone.is_empty() {
        entries.retain(|entry| !gone.contains(&entry.name));
    }
    if arrived.is_empty() {
        return;
    }
    let mut arrived: Vec<Entry> = arrived.into_values().collect();
    sort_entries(&mut arrived, &sort);

    // Where each new row goes among the old ones: the new rows are in order, so each search
    // starts where the last one ended.
    let mut places = Vec::with_capacity(arrived.len());
    let mut start = 0;
    for entry in &arrived {
        start += entries[start..]
            .partition_point(|row| compare_entries(row, entry, &sort) == Ordering::Less);
        places.push(start);
    }
    // One pass that moves the old rows over and puts the new ones in between.
    let mut old = std::mem::take(entries).into_iter();
    entries.reserve(old.len() + arrived.len());
    let mut moved = 0;
    for (entry, place) in arrived.into_iter().zip(places) {
        entries.extend(old.by_ref().take(place - moved));
        moved = place;
        entries.push(entry);
    }
    entries.extend(old);
}

/// What [`Pane::handle`] did with an event, and what the caller does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Handled {
    /// No tab of the pane has the event's generation: the event is late, and was dropped.
    Stale,
    /// Tab `tab` took the event; redraw it if it shows.
    Applied { tab: usize },
    /// Tab `tab` may have missed changes and lists its folder again: the caller hands
    /// `request` to the loader and drops the ticket of the event's generation.
    Reload { tab: usize, request: LoadRequest },
    /// The watch of tab `tab` ended at `path` and no change follows: the folder was removed,
    /// moved or replaced by another one, or the watch failed while it is still there. The tab
    /// is left as it is, and the caller:
    ///
    /// 1. keeps the ticket of `generation`: a listing that is still running still comes;
    /// 2. on a worker thread, waits a moment (a folder replaced in one go, such as
    ///    `rm -rf dist && mkdir dist`, is back by then) and finds
    ///    [`Loader::nearest_existing_parent`] of `path`;
    /// 3. back on the UI thread, if the tab still has `generation`
    ///    ([`Pane::tab_by_generation_mut`]), navigates it to that folder, or home when none is
    ///    left. When the folder found is `path` itself, [`Tab::navigate`] lists it again with a
    ///    new watch, which shows a replaced folder's new contents; the caller allows that at
    ///    most once per folder every few seconds, so that a watch that keeps failing on a
    ///    folder that is there cannot reload it in a loop.
    ///
    /// [`Loader::nearest_existing_parent`]: crate::loader::Loader::nearest_existing_parent
    Vanished {
        tab: usize,
        generation: u64,
        path: VPath,
    },
}

/// One of the two folder panes: its tabs, of which one shows. A pane has at least one tab.
#[derive(Debug)]
pub struct Pane {
    tabs: Vec<Tab>,
    active: usize,
}

impl Pane {
    /// A pane with one tab, about to show `location` in the default order, and the request
    /// for its listing.
    pub fn new(location: VPath, filter: VisibilityFilter) -> (Pane, LoadRequest) {
        let (tab, request) = Tab::new(location, SortSpec::default(), filter);
        (
            Pane {
                tabs: vec![tab],
                active: 0,
            },
            request,
        )
    }

    /// The tabs, in the order of the tab bar.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// The tabs, to change one (to show hidden items in every tab, for example).
    pub fn tabs_mut(&mut self) -> &mut [Tab] {
        &mut self.tabs
    }

    /// The position of the tab that shows.
    pub fn active(&self) -> usize {
        self.active
    }

    /// The tab that shows.
    pub fn active_tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    /// The tab that shows, to change.
    pub fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    /// Opens a tab at `location` after the last one and shows it. It takes the order and the
    /// filter of the tab that showed.
    pub fn add_tab(&mut self, location: VPath) -> LoadRequest {
        let current = self.active_tab();
        let (tab, request) = Tab::new(location, current.sort, current.filter);
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        request
    }

    /// Closes tab `i` and returns it, so that the caller can drop its ticket. The tab that
    /// showed keeps showing, or its right neighbour (the left one at the end) when it was
    /// `i`. `None`, and no change, for the last tab or a position past the end.
    pub fn close_tab(&mut self, i: usize) -> Option<Tab> {
        if self.tabs.len() <= 1 || i >= self.tabs.len() {
            return None;
        }
        let closed = self.tabs.remove(i);
        if i < self.active || self.active == self.tabs.len() {
            self.active -= 1;
        }
        Some(closed)
    }

    /// Shows tab `i`; `false`, and no change, for a position past the end.
    pub fn select_tab(&mut self, i: usize) -> bool {
        if i >= self.tabs.len() {
            return false;
        }
        self.active = i;
        true
    }

    /// The tab whose last request has `generation`, if any.
    pub fn tab_by_generation_mut(&mut self, generation: u64) -> Option<&mut Tab> {
        self.tabs
            .iter_mut()
            .find(|tab| tab.generation == generation)
    }

    /// Hands `event` to the tab that asked for it, whichever tab shows, and says what to do
    /// next. A rescan reloads the tab here; a vanished folder is left to the caller, which
    /// needs a worker thread to find where to go.
    pub fn handle(&mut self, event: LoadEvent) -> Handled {
        let generation = event.generation();
        let Some(tab) = self
            .tabs
            .iter()
            .position(|tab| tab.generation == generation)
        else {
            return Handled::Stale;
        };
        let target = &mut self.tabs[tab];
        match event {
            LoadEvent::Listed { entries, .. } => {
                target.apply_sorted(generation, entries);
            }
            LoadEvent::Failed { kind, .. } => {
                target.finish(generation, Err(kind));
            }
            LoadEvent::Changes { changes, .. } => {
                target.apply_changes(generation, changes);
            }
            LoadEvent::Space { available, .. } => {
                target.apply_space(generation, available);
            }
            LoadEvent::Rescan { .. } => {
                return Handled::Reload {
                    tab,
                    request: target.reload(),
                };
            }
            LoadEvent::Vanished { .. } => {
                return Handled::Vanished {
                    tab,
                    generation,
                    path: target.location.clone(),
                };
            }
        }
        Handled::Applied { tab }
    }
}

/// `path` and the folders above it, the root first: the pieces of a tab's path row.
pub fn ancestors(path: &VPath) -> Vec<VPath> {
    let mut chain = vec![path.clone()];
    while let Some(parent) = chain.last().and_then(VPath::parent) {
        chain.push(parent);
    }
    chain.reverse();
    chain
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::time::Duration;

    use rf_core::{Attributes, EntryKind};
    use rf_vfs::SortOrder;

    use super::*;

    fn folder(name: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(name)).expect("a local path")
    }

    fn file(name: &str, size: u64) -> Entry {
        let mut entry = Entry::new(name, EntryKind::File);
        entry.size = size;
        entry
    }

    fn dir(name: &str) -> Entry {
        Entry::new(name, EntryKind::Dir)
    }

    fn hidden(mut entry: Entry) -> Entry {
        entry.attributes |= Attributes::HIDDEN;
        entry
    }

    /// No names, to compare an empty list with.
    const NONE: [&str; 0] = [];

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries
            .iter()
            .map(|entry| entry.name.to_str().expect("test names are text"))
            .collect()
    }

    /// A tab at `at` that has listed `entries` (in any order).
    fn ready_tab(at: &str, entries: Vec<Entry>) -> Tab {
        let (mut tab, request) =
            Tab::new(folder(at), SortSpec::default(), VisibilityFilter::default());
        assert!(tab.apply_batch(request.generation, entries));
        assert!(tab.finish(request.generation, Ok(())));
        assert_eq!(tab.status(), LoadStatus::Ready);
        tab
    }

    fn selected(tab: &Tab) -> Vec<&str> {
        let indices = tab.selection().indices(tab.visible());
        indices
            .into_iter()
            .map(|i| tab.visible()[i].name.to_str().expect("test names are text"))
            .collect()
    }

    #[test]
    fn navigate_bumps_generation() {
        let mut tab = ready_tab("a", vec![file("x", 1)]);
        let before = tab.generation();
        tab.selection_mut().0.select_all(&[OsString::from("x")]);

        let request = tab.navigate(folder("b"));
        assert!(request.generation > before);
        assert_eq!(request.generation, tab.generation());
        assert_eq!(request.path, folder("b"));
        assert_eq!(tab.location(), &folder("b"));
        assert_eq!(tab.status(), LoadStatus::Loading);
        assert!(tab.visible().is_empty() && tab.all().is_empty());
        assert!(tab.selection().is_empty());
        assert!(tab.history().can_back());

        // Opening the folder the tab is at reloads it: a new generation, no new history.
        let again = tab.navigate(folder("b"));
        assert!(again.generation > request.generation);
        assert_eq!(tab.go_back().map(|r| r.path), Some(folder("a")));
        assert!(!tab.history().can_back());
    }

    #[test]
    fn generations_differ_across_tabs() {
        let (one, first) = Tab::new(
            folder("a"),
            SortSpec::default(),
            VisibilityFilter::default(),
        );
        let (two, second) = Tab::new(
            folder("a"),
            SortSpec::default(),
            VisibilityFilter::default(),
        );
        assert_ne!(first.generation, second.generation);
        assert_ne!(one.generation(), two.generation());
    }

    #[test]
    fn stale_listing_is_dropped() {
        let (mut tab, at_a) = Tab::new(
            folder("a"),
            SortSpec::default(),
            VisibilityFilter::default(),
        );
        let at_b = tab.navigate(folder("b"));

        // A's listing comes in late, in batches or sorted: B's screen does not take it.
        assert!(!tab.apply_batch(at_a.generation, vec![file("from-a", 1)]));
        assert!(!tab.finish(at_a.generation, Ok(())));
        assert!(!tab.apply_sorted(at_a.generation, vec![file("from-a", 1)]));
        assert!(!tab.finish(at_a.generation, Err(ErrorKind::NotFound)));
        assert!(!tab.apply_space(at_a.generation, 5));
        assert_eq!(tab.status(), LoadStatus::Loading);
        assert_eq!(names(tab.visible()), NONE);
        assert_eq!(tab.free(), None);

        assert!(tab.apply_sorted(at_b.generation, vec![file("from-b", 1)]));
        assert!(tab.apply_space(at_b.generation, 7));
        assert_eq!(names(tab.visible()), ["from-b"]);
        assert_eq!(tab.free(), Some(7));
        // A listing comes once: a second one for the same request is not taken either.
        assert!(!tab.apply_sorted(at_b.generation, vec![file("again", 1)]));
        assert_eq!(names(tab.visible()), ["from-b"]);
    }

    #[test]
    fn listing_is_sorted_and_filtered_once() {
        let (mut tab, request) = Tab::new(
            folder("a"),
            SortSpec::default(),
            VisibilityFilter::default(),
        );
        assert!(tab.apply_batch(request.generation, vec![file("b10", 1), dir("z")]));
        assert!(tab.apply_batch(
            request.generation,
            vec![file("b2", 1), hidden(file("h", 1))]
        ));
        // Nothing shows before the listing is complete.
        assert_eq!(names(tab.visible()), NONE);
        assert!(tab.finish(request.generation, Ok(())));
        assert_eq!(names(tab.visible()), ["z", "b2", "b10"]);
        assert_eq!(names(tab.all()), ["z", "b2", "b10", "h"]);

        // A listing sorted by the worker in an order the tab has left since is sorted again.
        let request = tab.reload();
        tab.set_sort(SortKey::Name);
        assert!(tab.apply_sorted(
            request.generation,
            vec![dir("z"), file("a", 1), file("b", 1)]
        ));
        assert_eq!(names(tab.visible()), ["z", "b", "a"]);

        let request = tab.reload();
        assert!(tab.finish(request.generation, Err(ErrorKind::PermissionDenied)));
        assert_eq!(
            tab.status(),
            LoadStatus::Failed(ErrorKind::PermissionDenied)
        );
        assert_eq!(names(tab.visible()), NONE);
    }

    #[test]
    fn stale_changes_are_dropped() {
        let (mut tab, at_a) = Tab::new(
            folder("a"),
            SortSpec::default(),
            VisibilityFilter::default(),
        );
        let at_b = tab.navigate(folder("b"));
        assert!(tab.apply_sorted(at_b.generation, vec![file("b1", 1)]));

        // A's watch reports a change after the tab left A.
        assert!(!tab.apply_changes(
            at_a.generation,
            vec![
                Change::Upsert(file("from-a", 1)),
                Change::Removed(OsString::from("b1")),
            ]
        ));
        assert_eq!(names(tab.visible()), ["b1"]);

        // Through the pane, the late events are reported as stale.
        let (mut pane, first) = Pane::new(folder("a"), VisibilityFilter::default());
        let second = pane.active_tab_mut().navigate(folder("b"));
        for event in [
            LoadEvent::Changes {
                generation: first.generation,
                changes: vec![Change::Upsert(file("from-a", 1))],
            },
            LoadEvent::Listed {
                generation: first.generation,
                entries: vec![file("from-a", 1)],
            },
            LoadEvent::Space {
                generation: first.generation,
                available: 1,
            },
            LoadEvent::Rescan {
                generation: first.generation,
            },
            LoadEvent::Vanished {
                generation: first.generation,
            },
        ] {
            assert_eq!(pane.handle(event), Handled::Stale);
        }
        assert_eq!(pane.active_tab().generation(), second.generation);
        assert_eq!(pane.active_tab().status(), LoadStatus::Loading);
    }

    #[test]
    fn changes_keep_selection_by_name() {
        let mut tab = ready_tab(
            "a",
            vec![file("a", 1), file("b", 2), file("c", 3), file("d", 4)],
        );
        let generation = tab.generation();
        {
            let (selection, rows) = tab.selection_mut();
            selection.click(1, rows);
            selection.toggle(3, rows);
        }
        assert_eq!(selected(&tab), ["b", "d"]);

        // Another app adds a0, removes c, renames a to z and rewrites b.
        assert!(tab.apply_changes(
            generation,
            vec![
                Change::Upsert(file("a0", 1)),
                Change::Removed(OsString::from("c")),
                Change::Renamed {
                    from: OsString::from("a"),
                    to: file("z", 1),
                },
                Change::Upsert(file("b", 20)),
            ]
        ));
        assert_eq!(names(tab.visible()), ["a0", "b", "d", "z"]);
        assert_eq!(tab.visible()[1].size, 20);
        assert_eq!(selected(&tab), ["b", "d"]);
        // The cursor stays on d, which moved up a row.
        assert_eq!(tab.selection().cursor(), Some(2));

        // A change the listing already showed changes nothing, and removing what is not
        // there either.
        assert!(tab.apply_changes(
            generation,
            vec![
                Change::Upsert(file("d", 4)),
                Change::Removed(OsString::from("gone"))
            ]
        ));
        assert_eq!(names(tab.visible()), ["a0", "b", "d", "z"]);

        // Many changes at once land in order too.
        let many: Vec<Change> = (0..300)
            .map(|i| Change::Upsert(file(&format!("m{i}"), 1)))
            .collect();
        assert!(tab.apply_changes(generation, many));
        assert_eq!(tab.visible().len(), 304);
        assert_eq!(names(&tab.visible()[..4]), ["a0", "b", "d", "m0"]);
        assert_eq!(names(&tab.visible()[4..6]), ["m1", "m2"]);
        assert_eq!(tab.visible()[303].name, "z");
        assert_eq!(selected(&tab), ["b", "d"]);
    }

    #[test]
    fn a_changed_row_moves_to_its_place() {
        let mut tab = ready_tab(
            "a",
            vec![file("a", 10), file("b", 20), file("c", 30), dir("d")],
        );
        tab.set_sort(SortKey::Size);
        assert_eq!(names(tab.visible()), ["d", "a", "b", "c"]);
        let generation = tab.generation();
        // a grows past c, x arrives in the middle, and y and w at the ends.
        assert!(tab.apply_changes(
            generation,
            vec![
                Change::Upsert(file("a", 40)),
                Change::Upsert(file("x", 25)),
                Change::Upsert(file("y", 99)),
                Change::Upsert(file("w", 1)),
            ]
        ));
        assert_eq!(names(tab.visible()), ["d", "w", "b", "x", "c", "a", "y"]);
        assert_eq!(names(tab.all()), names(tab.visible()));
    }

    #[test]
    fn changes_during_loading_are_kept() {
        let (mut tab, request) = Tab::new(
            folder("a"),
            SortSpec::default(),
            VisibilityFilter::default(),
        );
        // The watch starts before the listing, so its changes can arrive first.
        assert!(tab.apply_changes(
            request.generation,
            vec![
                Change::Upsert(file("new", 1)),
                Change::Removed(OsString::from("old"))
            ]
        ));
        assert_eq!(names(tab.visible()), NONE);
        assert!(tab.apply_sorted(request.generation, vec![file("keep", 1), file("old", 1)]));
        assert_eq!(names(tab.visible()), ["keep", "new"]);
    }

    #[test]
    fn sort_toggles_on_same_key() {
        let mut tab = ready_tab(
            "a",
            vec![file("b", 30), dir("d"), file("a", 10), file("c", 20)],
        );
        assert_eq!(names(tab.visible()), ["d", "a", "b", "c"]);
        {
            let (selection, rows) = tab.selection_mut();
            selection.click(1, rows);
        }

        tab.set_sort(SortKey::Name);
        assert_eq!(tab.sort().primary.order, SortOrder::Descending);
        // Folders stay first in either direction.
        assert_eq!(names(tab.visible()), ["d", "c", "b", "a"]);
        assert_eq!(selected(&tab), ["a"]);
        assert_eq!(tab.selection().cursor(), Some(3));

        tab.set_sort(SortKey::Name);
        assert_eq!(tab.sort().primary.order, SortOrder::Ascending);
        assert_eq!(names(tab.visible()), ["d", "a", "b", "c"]);

        // Another key starts ascending.
        tab.set_sort(SortKey::Size);
        assert_eq!(tab.sort().primary.key, SortKey::Size);
        assert_eq!(tab.sort().primary.order, SortOrder::Ascending);
        assert_eq!(names(tab.visible()), ["d", "a", "c", "b"]);
        tab.set_sort(SortKey::Size);
        assert_eq!(names(tab.visible()), ["d", "b", "c", "a"]);
        // The next listing is asked for in the new order.
        assert_eq!(tab.reload().sort, tab.sort());
    }

    #[test]
    fn hidden_toggle_refilters() {
        let mut tab = ready_tab("a", vec![file("a", 1), hidden(file("ab", 1)), file("b", 1)]);
        assert_eq!(names(tab.visible()), ["a", "b"]);
        assert_eq!(tab.all().len(), 3);

        tab.set_show_hidden(true);
        assert!(tab.filter().show_hidden);
        assert_eq!(names(tab.visible()), ["a", "ab", "b"]);
        {
            let (selection, rows) = tab.selection_mut();
            selection.click(1, rows);
            selection.toggle(2, rows);
        }
        assert_eq!(selected(&tab), ["ab", "b"]);

        // Hiding drops the hidden item from the selection and keeps the rest.
        tab.set_show_hidden(false);
        assert_eq!(names(tab.visible()), ["a", "b"]);
        assert_eq!(selected(&tab), ["b"]);
        assert!(!tab.selection().is_selected(OsStr::new("ab")));

        // A hidden item that another app adds stays out of sight.
        let generation = tab.generation();
        assert!(tab.apply_changes(generation, vec![Change::Upsert(hidden(file("ac", 1)))]));
        assert_eq!(names(tab.visible()), ["a", "b"]);
        assert_eq!(tab.all().len(), 4);
    }

    #[test]
    fn status_line_matches_mockup() {
        let mut entries = vec![
            dir("문서"),
            dir("사진"),
            file("설계안.pdf", 1_000_000),
            file("메모.txt", 477_427),
        ];
        entries.extend((0..5).map(|i| file(&format!("z{i}"), 1)));
        let mut tab = ready_tab("a", entries);
        let generation = tab.generation();
        assert_eq!(tab.status_line(), "항목 9개");
        assert!(tab.apply_space(generation, 212 * 1024 * 1024 * 1024));
        assert_eq!(tab.status_line(), "항목 9개 · 남은 공간 212 GB");

        let find = |tab: &Tab, name: &str| {
            tab.visible()
                .iter()
                .position(|entry| entry.name == name)
                .expect("the row is listed")
        };
        let (pdf, txt, docs) = (
            find(&tab, "설계안.pdf"),
            find(&tab, "메모.txt"),
            find(&tab, "문서"),
        );
        {
            let (selection, rows) = tab.selection_mut();
            selection.click(pdf, rows);
            selection.toggle(txt, rows);
        }
        assert_eq!(
            tab.status_line(),
            "항목 9개 · 선택 2개 (1.4 MB) · 남은 공간 212 GB"
        );

        // A folder adds to the count but not to the size.
        {
            let (selection, rows) = tab.selection_mut();
            selection.toggle(docs, rows);
        }
        assert_eq!(
            tab.status_line(),
            "항목 9개 · 선택 3개 (1.4 MB) · 남은 공간 212 GB"
        );
        // Folders alone have no size to show.
        {
            let (selection, rows) = tab.selection_mut();
            selection.click(docs, rows);
        }
        assert_eq!(tab.status_line(), "항목 9개 · 선택 1개 · 남은 공간 212 GB");
    }

    #[test]
    fn type_to_select_jumps_by_prefix() {
        let mut tab = ready_tab(
            "a",
            vec![
                file("alpha", 1),
                file("beta", 1),
                file("Bravo", 1),
                file("charlie", 1),
            ],
        );
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);

        assert_eq!(tab.type_to_select("b", at(0)), Some(1));
        assert_eq!(selected(&tab), ["beta"]);
        // Typing on within a second adds to the prefix, ignoring case.
        assert_eq!(tab.type_to_select("R", at(500)), Some(2));
        assert_eq!(selected(&tab), ["Bravo"]);
        assert_eq!(tab.selection().cursor(), Some(2));
        // After a pause the prefix starts again.
        assert_eq!(tab.type_to_select("c", at(2_000)), Some(3));
        // No match leaves the cursor where it is.
        assert_eq!(tab.type_to_select("x", at(2_200)), None);
        assert_eq!(tab.selection().cursor(), Some(3));
        // A space alone does not start a prefix.
        assert_eq!(tab.type_to_select(" ", at(5_000)), None);
        assert_eq!(tab.type_to_select("a", at(5_100)), Some(0));
    }

    #[test]
    fn open_target_follows_the_kind() {
        let link = Entry::new("link", EntryKind::Symlink);
        let tab = ready_tab("a", vec![dir("sub"), file("f.txt", 1), link]);
        let at = |name: &str| folder("a").join(name).expect("a plain name");
        assert_eq!(tab.open_target(0), Some(OpenAction::Navigate(at("sub"))));
        assert_eq!(tab.open_target(1), Some(OpenAction::OpenFile(at("f.txt"))));
        assert_eq!(tab.open_target(2), Some(OpenAction::Follow(at("link"))));
        assert_eq!(tab.open_target(3), None);
    }

    #[test]
    fn back_forward_and_up() {
        let child = folder("a").join("child").expect("a plain name");
        let (mut tab, _) = Tab::new(
            child.clone(),
            SortSpec::default(),
            VisibilityFilter::default(),
        );
        assert_eq!(tab.go_back(), None);

        let up = tab.go_up().expect("the folder has a parent");
        assert_eq!(up.path, folder("a"));
        assert_eq!(tab.location(), &folder("a"));
        assert_eq!(tab.status(), LoadStatus::Loading);

        let back = tab.go_back().expect("the child is behind");
        assert_eq!(back.path, child);
        assert!(back.generation > up.generation);
        assert!(tab.history().can_forward());
        let forward = tab.go_forward().expect("the parent is ahead");
        assert_eq!(forward.path, folder("a"));
        assert!(!tab.history().can_forward());

        let root = VPath::local(std::env::temp_dir().ancestors().last().expect("a root"))
            .expect("a local path");
        tab.navigate(root);
        assert_eq!(tab.go_up(), None);
    }

    #[test]
    fn events_reach_the_tab_that_asked() {
        let (mut pane, first) = Pane::new(folder("a"), VisibilityFilter::default());
        let second = pane.add_tab(folder("b"));
        assert_eq!(pane.tabs().len(), 2);
        assert_eq!(pane.active(), 1);
        assert_eq!(pane.active_tab().location(), &folder("b"));

        // The first tab's listing goes to the first tab, even though the second one shows.
        let handled = pane.handle(LoadEvent::Listed {
            generation: first.generation,
            entries: vec![file("in-a", 1)],
        });
        assert_eq!(handled, Handled::Applied { tab: 0 });
        assert_eq!(names(pane.tabs()[0].visible()), ["in-a"]);
        assert_eq!(names(pane.tabs()[1].visible()), NONE);

        // A rescan reloads the tab; the old generation is stale from then on.
        let Handled::Reload { tab: 1, request } = pane.handle(LoadEvent::Rescan {
            generation: second.generation,
        }) else {
            panic!("a rescan reloads the tab");
        };
        assert_eq!(request.path, folder("b"));
        assert_eq!(
            pane.handle(LoadEvent::Failed {
                generation: second.generation,
                kind: ErrorKind::NotFound,
            }),
            Handled::Stale
        );
        assert_eq!(
            pane.handle(LoadEvent::Vanished {
                generation: request.generation,
            }),
            Handled::Vanished {
                tab: 1,
                generation: request.generation,
                path: folder("b"),
            }
        );
        assert!(pane.tab_by_generation_mut(request.generation).is_some());
        assert!(pane.tab_by_generation_mut(second.generation).is_none());

        // Tabs open, switch and close; the last one stays.
        assert!(pane.select_tab(0));
        assert!(!pane.select_tab(2));
        assert_eq!(pane.active(), 0);
        let closed = pane.close_tab(0).expect("two tabs are open");
        assert_eq!(closed.location(), &folder("a"));
        assert_eq!(pane.active(), 0);
        assert_eq!(pane.active_tab().location(), &folder("b"));
        assert!(pane.close_tab(0).is_none());
        assert!(pane.close_tab(5).is_none());

        // A new tab keeps the order and the filter of the tab it was opened from.
        pane.active_tab_mut().set_sort(SortKey::Size);
        pane.active_tab_mut().set_show_hidden(true);
        let third = pane.add_tab(folder("c"));
        assert_eq!(third.sort, pane.tabs()[0].sort());
        assert!(pane.active_tab().filter().show_hidden);
    }
}
