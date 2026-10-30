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
//! and an event of a closed tab never reaches another tab. A tab that moves (along the tab
//! bar, or to the other pane) keeps its generation, so its listing follows it there; a tab
//! that comes back after it was closed ([`Pane::reopen`]) or a copy of one
//! ([`Pane::duplicate`]) is a new tab with a request of its own.
//!
//! A tab shows its folder in a [`FolderView`]: a view mode, columns and an order of the rows.
//! A long listing is put in a new order on a worker thread ([`Tab::take_sort_request`]), which
//! hands it back with the tab's generation and the number of that sort: the rows stay in the
//! old order until it comes, and a sort the tab no longer waits for is dropped.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use rf_core::{Entry, EntryKind, ErrorKind, VPath};
use rf_vfs::{Change, SortKey, SortSpec, VisibilityFilter, compare_entries, sort_entries};

use crate::history::History;
use crate::loader::{LoadEvent, LoadRequest};
use crate::selection::Selection;
use crate::state::FolderView;
use crate::words;

/// How long after a key the next one still adds to the type-to-select prefix (NAV-14).
pub const TYPE_TO_SELECT_WINDOW: Duration = Duration::from_secs(1);

/// How many closed tabs a pane keeps to reopen ([`Pane::reopen`]); the oldest goes first.
pub const CLOSED_TABS_KEPT: usize = 10;

/// The most rows a tab puts in a new order on the UI thread; a longer listing is sorted on a
/// worker ([`Tab::take_sort_request`]). Sorting takes some microseconds a row (the natural
/// order of the names), so this many take a few milliseconds.
pub const INLINE_SORT_ROWS: usize = 2_000;

/// A tab's rows to put in a new order on a worker thread: [`SortRequest::run`] there, and the
/// [`Sorted`] rows back to the tab (`AppState::sorted`).
#[derive(Clone, PartialEq, Eq)]
pub struct SortRequest {
    /// The tab that asked ([`Tab::id`]): a newer sort of the same tab takes the place of one
    /// that waits for the worker.
    pub tab: u64,
    /// The generation of the tab's listing: the rows are dropped if the tab has listed again
    /// or moved on by the time they come.
    pub generation: u64,
    /// The number of this sort: the rows are dropped if the tab has asked for another since.
    pub serial: u64,
    pub sort: SortSpec,
    pub entries: Vec<Entry>,
}

impl std::fmt::Debug for SortRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SortRequest")
            .field("tab", &self.tab)
            .field("generation", &self.generation)
            .field("serial", &self.serial)
            .field("sort", &self.sort)
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl SortRequest {
    /// Sorts the rows. It takes long for many rows: call it on a worker thread.
    pub fn run(mut self) -> Sorted {
        sort_entries(&mut self.entries, &self.sort);
        Sorted {
            generation: self.generation,
            serial: self.serial,
            entries: self.entries,
        }
    }
}

/// The rows of a [`SortRequest`], in its order.
#[derive(Clone, PartialEq, Eq)]
pub struct Sorted {
    pub generation: u64,
    pub serial: u64,
    pub entries: Vec<Entry>,
}

impl std::fmt::Debug for Sorted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sorted")
            .field("generation", &self.generation)
            .field("serial", &self.serial)
            .field("entries", &self.entries.len())
            .finish()
    }
}

/// A generation that no request has had yet: the tabs and the folder tree
/// ([`crate::tree`]) draw from the same count, so a generation names one request in the
/// whole process.
pub(crate) fn next_generation() -> u64 {
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
/// `all` without the items the filter hides, both in the order of `shown_sort` (the view's
/// order, or the one before it while a worker sorts), and the selection and the cursor refer to
/// `visible`.
///
/// It is not `Clone`: a copy would share the tab's generation and take its events too
/// ([`Pane::duplicate`] makes a copy with a request of its own).
#[derive(Debug)]
pub struct Tab {
    /// Names the tab for as long as it is open, wherever it moves and whatever it lists: a
    /// drag of the tab carries it.
    id: u64,
    /// A pinned tab stays at its folder: a move to another one opens in a new tab beside it
    /// (NAV-01, [`Tab::fork`]).
    pinned: bool,
    /// The tab came back after it was closed ([`Pane::reopen`]) and its first listing has not
    /// come: a folder that is gone by now is not shown as an error, but left for the nearest
    /// folder above it ([`Tab::take_reopened`]).
    reopened: bool,
    location: VPath,
    history: History,
    /// Every item of the folder, sorted by `shown_sort`.
    all: Vec<Entry>,
    /// The items of `all` that `filter` shows and whose names hold `name_filter`, in the same
    /// order: the rows on screen.
    visible: Vec<Entry>,
    /// How many items of `all` `filter` shows, whatever their names.
    unfiltered: usize,
    /// The view mode, the columns and the order the tab shows its folder in.
    view: FolderView,
    /// The folder whose remembered view `view` is (`AppState::settle_views`); `None` until one
    /// is given.
    view_for: Option<VPath>,
    /// The order `all` is in: the order of `view`, or the one before it while a worker puts the
    /// rows in the new one.
    shown_sort: SortSpec,
    /// The sort a worker does for the tab: its number and its order.
    sorting: Option<(u64, SortSpec)>,
    /// The changes the watch reported since the worker took the rows to sort; they go on top of
    /// what it gives back.
    since_sort: Vec<Change>,
    filter: VisibilityFilter,
    /// What the rows' names must hold, ignoring case, while it is not empty (the toolbar's
    /// "이 폴더에서 거르기"): it filters this folder, and goes when the tab goes elsewhere.
    name_filter: String,
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
    /// The name [`Tab::select_name`] waits for: it is selected when it shows.
    pending_select: Option<OsString>,
    /// The row a waiting [`Tab::select_name`] selected, until [`Tab::take_revealed`].
    revealed: Option<usize>,
    /// The folder of the last run that the tab shows home in place of, until it goes
    /// somewhere ([`Tab::stand_in_for`]).
    stands_in_for: Option<VPath>,
}

impl Tab {
    /// A tab that is about to show `location` in the default view in the order `sort`, and
    /// the request for its listing.
    pub fn new(location: VPath, sort: SortSpec, filter: VisibilityFilter) -> (Tab, LoadRequest) {
        Tab::with_view(location, FolderView::sorted(sort), filter)
    }

    /// A tab that is about to show `location` in `view`, and the request for its listing.
    pub fn with_view(
        location: VPath,
        view: FolderView,
        filter: VisibilityFilter,
    ) -> (Tab, LoadRequest) {
        let mut tab = Tab::idle(location.clone(), view, filter);
        let request = tab.load(location);
        (tab, request)
    }

    /// A tab for going on from this one: at its folder, with its history and its order, not
    /// pinned, and with no request yet. The caller makes it move at once ([`Tab::navigate`], a
    /// step back, forward or up), which asks for the listing of where it goes; it is put in a
    /// pane only then ([`Pane::open_beside`]). A pinned tab's moves go to such a tab.
    #[must_use]
    pub fn fork(&self) -> Tab {
        let mut tab = Tab::idle(self.location.clone(), self.view.clone(), self.filter);
        tab.history = self.history.clone();
        tab
    }

    /// A tab at `location` that has asked for nothing yet (generation 0, which no request
    /// has).
    fn idle(location: VPath, view: FolderView, filter: VisibilityFilter) -> Tab {
        let sort = view.sort;
        Tab {
            id: next_generation(),
            pinned: false,
            reopened: false,
            location,
            history: History::default(),
            all: Vec::new(),
            visible: Vec::new(),
            unfiltered: 0,
            view,
            view_for: None,
            shown_sort: sort,
            sorting: None,
            since_sort: Vec::new(),
            filter,
            name_filter: String::new(),
            selection: Selection::default(),
            generation: 0,
            status: LoadStatus::Loading,
            free: None,
            type_buffer: (String::new(), Instant::now()),
            incoming: Vec::new(),
            pending: Vec::new(),
            requested_sort: sort,
            pending_select: None,
            revealed: None,
            stands_in_for: None,
        }
    }

    /// What names the tab while it is open, wherever it moves.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Whether the tab is pinned: it stays at its folder, and a move to another one opens in a
    /// new tab beside it.
    pub fn pinned(&self) -> bool {
        self.pinned
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.pinned = pinned;
    }

    /// Whether the tab was reopened and still waits for its first listing; it is not any more
    /// after this. Any other request of the tab ends the wait too.
    pub fn take_reopened(&mut self) -> bool {
        std::mem::take(&mut self.reopened)
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

    /// The order of the rows, as the user chose it (the rows may still be in the one before,
    /// while a worker sorts them).
    pub fn sort(&self) -> SortSpec {
        self.view.sort
    }

    /// The view mode, the columns and the order of the rows.
    pub fn view(&self) -> &FolderView {
        &self.view
    }

    /// Shows the folder in `view`; the rows are put in its order.
    pub fn set_view(&mut self, view: FolderView) {
        self.view = view;
        self.resort();
    }

    /// The folder whose remembered view the tab shows, once it has been given one.
    pub fn view_for(&self) -> Option<&VPath> {
        self.view_for.as_ref()
    }

    /// Gives the tab `view`, the view remembered for its folder. When `request` is the request
    /// for its listing, not sent yet, it asks for the listing in the view's order, which the
    /// worker that lists it sorts it in.
    pub fn settle_view(&mut self, view: FolderView, request: Option<&mut LoadRequest>) {
        self.view_for = Some(self.location.clone());
        if let Some(request) = request.filter(|request| request.generation == self.generation) {
            request.sort = view.sort;
            self.requested_sort = view.sort;
        }
        self.set_view(view);
    }

    /// The tab's folder forgot its view: it is to be given the one remembered for it again.
    pub fn unsettle_view(&mut self) {
        self.view_for = None;
    }

    /// The sort a worker is to do, when the rows are not in the order of the view and no
    /// worker sorts them for it yet: the rows, which stay in the old order until
    /// [`Tab::apply_sort`] takes them back.
    pub fn take_sort_request(&mut self) -> Option<SortRequest> {
        let sort = self.view.sort;
        let asked = self.sorting.is_some_and(|(_, asked)| asked == sort);
        if self.status != LoadStatus::Ready || self.shown_sort == sort || asked {
            return None;
        }
        let serial = next_generation();
        self.sorting = Some((serial, sort));
        self.since_sort.clear();
        Some(SortRequest {
            tab: self.id,
            generation: self.generation,
            serial,
            sort,
            entries: self.all.clone(),
        })
    }

    /// Takes the rows a worker sorted, when they are those of the sort the tab waits for: they
    /// are the rows, with the changes since on top of them. `false`, and no change, for rows the
    /// tab no longer waits for (it listed again, moved on, or asked for another order).
    pub fn apply_sort(&mut self, sorted: Sorted) -> bool {
        let Some((serial, sort)) = self.sorting else {
            return false;
        };
        if sorted.generation != self.generation || sorted.serial != serial {
            return false;
        }
        self.sorting = None;
        self.all = sorted.entries;
        self.shown_sort = sort;
        let since = std::mem::take(&mut self.since_sort);
        if !since.is_empty() {
            merge_changes(&mut self.all, since, sort);
        }
        self.refresh_visible();
        true
    }

    /// The sort the tab waits for, of its listing `generation` and number `serial`, could not be
    /// done (the worker panicked, or no thread could be started for it): the tab stops waiting,
    /// and shows its rows in the order they are in, which its view takes again, so that no other
    /// worker is asked for the same sort. `false`, and no change, for a sort it no longer waits
    /// for.
    pub fn sort_failed(&mut self, generation: u64, serial: u64) -> bool {
        let waits = self.sorting.is_some_and(|(waited, _)| waited == serial);
        if generation != self.generation || !waits {
            return false;
        }
        self.sorting = None;
        self.since_sort.clear();
        self.view.sort = self.shown_sort;
        true
    }

    /// Puts the rows in the order of the view: at once when they are few, or else later, by a
    /// worker ([`Tab::take_sort_request`]). While a listing is on its way, the listing brings the
    /// order.
    fn resort(&mut self) {
        let sort = self.view.sort;
        if self.shown_sort == sort {
            self.sorting = None;
            self.since_sort.clear();
            return;
        }
        if self.status == LoadStatus::Loading {
            return;
        }
        if self.all.len() <= INLINE_SORT_ROWS {
            sort_entries(&mut self.all, &sort);
            self.shown_sort = sort;
            self.sorting = None;
            self.since_sort.clear();
            self.refresh_visible();
        }
    }

    /// Which hidden and system items show.
    pub fn filter(&self) -> VisibilityFilter {
        self.filter
    }

    /// What the rows' names must hold, ignoring case; empty for no filter.
    pub fn name_filter(&self) -> &str {
        &self.name_filter
    }

    /// Shows only the rows whose names hold `text`, ignoring case, or every row for an empty
    /// one. The selection keeps the rows that still show.
    pub fn set_name_filter(&mut self, text: &str) {
        if self.name_filter != text {
            text.clone_into(&mut self.name_filter);
            self.refresh_visible();
        }
    }

    /// The selected rows, the cursor and the anchor, as positions in [`Tab::visible`].
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// The selection to change, with the rows it refers to: a click is
    /// `let (selection, rows) = tab.selection_mut(); selection.click(i, rows);`. It is for a
    /// change the user makes (a click, a key, a selection tool): the name a job or
    /// [`Tab::go_up`] waits to select ([`Tab::select_name`]) is forgotten, so that it does not
    /// take the selection the user chose when it shows later.
    pub fn selection_mut(&mut self) -> (&mut Selection, &[Entry]) {
        self.pending_select = None;
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
            // Chosen again: it is the folder the tab shows now, not a stand-in.
            self.stands_in_for = None;
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

    /// Goes to the parent folder, like [`Tab::navigate`]; `None` at a root. The folder it
    /// came from is selected, with the cursor, once it shows ([`Tab::select_name`]).
    pub fn go_up(&mut self) -> Option<LoadRequest> {
        let parent = self.location.parent()?;
        let came_from = self.location.file_name().map(std::ffi::OsStr::to_os_string);
        let request = self.navigate(parent);
        self.pending_select = came_from;
        Some(request)
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
                sort_entries(&mut entries, &self.view.sort);
                self.shown_sort = self.view.sort;
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
    /// the request asked for, which keeps a long sort off the UI thread. When the tab's order
    /// has changed since, a short listing is sorted again here, and a long one shows in the
    /// order it came in until a worker sorts it ([`Tab::take_sort_request`]). `false`, and no
    /// change, for another request or a listing that has ended already.
    pub fn apply_sorted(&mut self, generation: u64, mut entries: Vec<Entry>) -> bool {
        if !self.is_loading(generation) {
            return false;
        }
        self.shown_sort = self.requested_sort;
        if self.requested_sort != self.view.sort && entries.len() <= INLINE_SORT_ROWS {
            sort_entries(&mut entries, &self.view.sort);
            self.shown_sort = self.view.sort;
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
                if self.sorting.is_some() {
                    self.since_sort.extend(changes.iter().cloned());
                }
                merge_changes(&mut self.all, changes, self.shown_sort);
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

    /// Sorts by `key` alone: ascending for a new key, the other way round for the key that
    /// sorts now (a second click on a column head, [`FolderView::sort_by`]). Folders stay
    /// where the order puts them (first, or among the files).
    pub fn set_sort(&mut self, key: SortKey) {
        let mut view = self.view.clone();
        view.sort_by(key, false);
        self.set_view(view);
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
        if self.name_filter.is_empty() {
            words::status_line(self.visible.len(), selected.len(), bytes, self.free)
        } else {
            let shown = (self.visible.len(), self.unfiltered);
            words::filtered_status_line(&self.name_filter, shown, selected.len(), bytes, self.free)
        }
    }

    /// Moves the cursor to the first row whose name starts with what was typed, ignoring
    /// case, and selects that row alone (NAV-14). Keys typed within
    /// [`TYPE_TO_SELECT_WINDOW`] of the last one add to the prefix; after a pause `text`
    /// starts a new one, and a prefix of spaces alone does nothing. Returns the row moved
    /// to, or `None` (and the selection stays) when no name matches. The user chooses here: a
    /// name that a job or [`Tab::go_up`] waits to select is forgotten, as for
    /// [`Tab::selection_mut`].
    pub fn type_to_select(&mut self, text: &str, now: Instant) -> Option<usize> {
        self.pending_select = None;
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

    /// Whether a name is being typed to select (NAV-14) at `now`: some of it has been typed, its
    /// last key came within [`TYPE_TO_SELECT_WINDOW`], and no other key came since
    /// ([`Tab::end_typing`]). A Space then types into the name rather than open quick look
    /// ([`keys::pane_command`](crate::keys::pane_command)).
    pub fn typing(&self, now: Instant) -> bool {
        let (prefix, last) = &self.type_buffer;
        !prefix.is_empty() && now.saturating_duration_since(*last) <= TYPE_TO_SELECT_WINDOW
    }

    /// A key other than a typed character came: the next one starts a new name.
    pub fn end_typing(&mut self) {
        self.type_buffer.0.clear();
    }

    /// Selects the row called `name` alone and puts the cursor on it: the item a job has just
    /// made or renamed. A name that does not show yet is selected when it comes (by the
    /// listing or the watch), unless the tab moves to another folder first or the user changes
    /// the selection meanwhile ([`Tab::selection_mut`], [`Tab::type_to_select`]). Gives the row
    /// when it shows already.
    pub fn select_name(&mut self, name: OsString) -> Option<usize> {
        let Some(row) = self.visible.iter().position(|entry| entry.name == name) else {
            self.pending_select = Some(name);
            return None;
        };
        self.selection.click(row, &self.visible);
        self.pending_select = None;
        Some(row)
    }

    /// The row a waiting [`Tab::select_name`] has selected since the last call, to scroll to.
    pub fn take_revealed(&mut self) -> Option<usize> {
        self.revealed.take()
    }

    /// Makes the tab stand in for `folder`, a folder of the last run that is not there yet (a
    /// slow drive, an unplugged one): it shows another folder (home) meanwhile, and the
    /// settings keep `folder` for it. A reload keeps that; going anywhere ends it.
    pub fn stand_in_for(&mut self, folder: VPath) {
        self.stands_in_for = Some(folder);
    }

    /// The folder the tab stands in for, if it does.
    pub fn stands_in_for(&self) -> Option<&VPath> {
        self.stands_in_for.as_ref()
    }

    /// Ends the tab's standing in (its folder turned out to be gone).
    pub fn end_stand_in(&mut self) {
        self.stands_in_for = None;
    }

    /// Whether the listing of request `generation` is still awaited.
    fn is_loading(&self, generation: u64) -> bool {
        generation == self.generation && self.status == LoadStatus::Loading
    }

    /// Moves to `to` without touching the history: no rows, no selection, no free space, and no
    /// folder stood in for any more.
    fn load(&mut self, to: VPath) -> LoadRequest {
        self.location = to;
        self.stands_in_for = None;
        self.all.clear();
        self.shown_sort = self.view.sort;
        self.visible.clear();
        self.unfiltered = 0;
        self.name_filter.clear();
        self.selection = Selection::default();
        self.free = None;
        self.pending_select = None;
        self.revealed = None;
        self.start_request()
    }

    /// A request with a new generation for the tab's folder; the rows stay as they are.
    fn start_request(&mut self) -> LoadRequest {
        self.generation = next_generation();
        self.reopened = false;
        self.status = LoadStatus::Loading;
        self.incoming.clear();
        self.pending.clear();
        self.requested_sort = self.view.sort;
        self.sorting = None;
        self.since_sort.clear();
        LoadRequest {
            generation: self.generation,
            path: self.location.clone(),
            sort: self.view.sort,
        }
    }

    /// Makes the sorted `entries`, with the changes that came before them, the rows. A list
    /// without a cursor (a folder just opened) gets it on its first row, nothing selected, so
    /// that the keys start from there; that row is not chosen by the user, and no command that
    /// changes files acts on it ([`Selection::place_cursor`]).
    fn show_listing(&mut self, entries: Vec<Entry>) {
        self.all = entries;
        let pending = std::mem::take(&mut self.pending);
        if !pending.is_empty() {
            merge_changes(&mut self.all, pending, self.shown_sort);
        }
        self.status = LoadStatus::Ready;
        self.refresh_visible();
        if self.selection.cursor().is_none() && self.pending_select.is_none() {
            self.selection.place_cursor(0, &self.visible);
        }
    }

    /// Filters `all` into `visible` again and carries the selection over.
    fn refresh_visible(&mut self) {
        let filter = self.filter;
        let wanted = self.name_filter.to_lowercase();
        let old = std::mem::take(&mut self.visible);
        let mut unfiltered = 0;
        self.visible = self
            .all
            .iter()
            .filter(|entry| filter.is_visible(entry))
            .inspect(|_| unfiltered += 1)
            .filter(|entry| {
                wanted.is_empty()
                    || entry
                        .name
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&wanted)
            })
            .cloned()
            .collect();
        self.unfiltered = unfiltered;
        self.selection.reconcile(&old, &self.visible);
        let waited = self
            .pending_select
            .as_ref()
            .and_then(|name| self.visible.iter().position(|entry| entry.name == *name));
        if let Some(row) = waited {
            self.selection.click(row, &self.visible);
            self.pending_select = None;
            self.revealed = Some(row);
        }
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

/// One of the two folder panes: its tabs, of which one shows, and the tabs it closed last. A
/// pane has at least one tab.
#[derive(Debug)]
pub struct Pane {
    tabs: Vec<Tab>,
    active: usize,
    /// The tabs closed last, the last one at the back: at most [`CLOSED_TABS_KEPT`].
    closed: VecDeque<ClosedTab>,
}

/// What [`Pane::reopen`] brings back of a closed tab: where it was in the tab bar, its folder
/// and history, its order, and whether it was pinned. Its rows and selection are not kept: the
/// folder is listed again.
#[derive(Debug)]
struct ClosedTab {
    /// Its position when it was closed.
    index: usize,
    location: VPath,
    history: History,
    view: FolderView,
    pinned: bool,
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
                closed: VecDeque::new(),
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
        let (tab, request) = Tab::with_view(location, current.view.clone(), current.filter);
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        request
    }

    /// Closes tab `i` and returns it, so that the caller can drop its ticket; the pane keeps
    /// what [`Pane::reopen`] needs to bring it back. The tab that showed keeps showing, or its
    /// right neighbour (the left one at the end) when it was `i`. `None`, and no change, for
    /// the last tab or a position past the end.
    pub fn close_tab(&mut self, i: usize) -> Option<Tab> {
        let closed = self.take_tab(i)?;
        if self.closed.len() == CLOSED_TABS_KEPT {
            self.closed.pop_front();
        }
        self.closed.push_back(ClosedTab {
            index: i,
            location: closed.location.clone(),
            history: closed.history.clone(),
            view: closed.view.clone(),
            pinned: closed.pinned,
        });
        Some(closed)
    }

    /// Whether a closed tab is there to reopen.
    pub fn can_reopen(&self) -> bool {
        !self.closed.is_empty()
    }

    /// Opens the tab closed last again, where it was in the tab bar (or at the end, when the
    /// bar is shorter now), at its folder, with its history, its order and its pin, and shows
    /// it. It is a new tab: the request for its listing has a new generation, so a late event
    /// of the closed one does not reach it; and it is [reopened](Tab::take_reopened) until that
    /// listing comes. `None` when no closed tab is left.
    pub fn reopen(&mut self) -> Option<LoadRequest> {
        let closed = self.closed.pop_back()?;
        let filter = self.active_tab().filter;
        let (mut tab, request) = Tab::with_view(closed.location, closed.view, filter);
        tab.history = closed.history;
        tab.pinned = closed.pinned;
        tab.reopened = true;
        let at = closed.index.min(self.tabs.len());
        self.tabs.insert(at, tab);
        self.active = at;
        Some(request)
    }

    /// Opens a copy of tab `i` right after it, at its folder, with its history and its order,
    /// not pinned, and shows it; the copy lists the folder itself. `None` for a position past
    /// the end.
    pub fn duplicate(&mut self, i: usize) -> Option<LoadRequest> {
        let original = self.tabs.get(i)?;
        let (mut tab, request) = Tab::with_view(
            original.location.clone(),
            original.view.clone(),
            original.filter,
        );
        tab.history = original.history.clone();
        self.open_beside(i, tab);
        Some(request)
    }

    /// Puts `tab`, which has made its request ([`Tab::fork`]), right after tab `i`, and shows
    /// it.
    pub fn open_beside(&mut self, i: usize, tab: Tab) {
        let at = (i + 1).min(self.tabs.len());
        self.tabs.insert(at, tab);
        self.active = at;
    }

    /// Moves tab `from` to `slot`, a place between the tabs as they are now (0 before the
    /// first, the number of tabs after the last). The tab that shows keeps showing. `false`,
    /// and no change, when that leaves the tab where it is or a position is past the end.
    pub fn move_tab(&mut self, from: usize, slot: usize) -> bool {
        if from >= self.tabs.len() || slot > self.tabs.len() || slot == from || slot == from + 1 {
            return false;
        }
        let shown = self.tabs[self.active].id;
        let tab = self.tabs.remove(from);
        let to = if slot > from { slot - 1 } else { slot };
        self.tabs.insert(to, tab);
        self.active = self.position(shown).unwrap_or(0);
        true
    }

    /// Takes tab `i` out of the pane, to put it in the other one ([`Pane::insert_tab`]); the
    /// pane does not keep it to reopen. The tab that showed keeps showing, or its right
    /// neighbour (the left one at the end) when it was `i`. `None`, and no change, for the last
    /// tab or a position past the end.
    pub fn take_tab(&mut self, i: usize) -> Option<Tab> {
        if self.tabs.len() <= 1 || i >= self.tabs.len() {
            return None;
        }
        let taken = self.tabs.remove(i);
        if i < self.active || self.active == self.tabs.len() {
            self.active -= 1;
        }
        Some(taken)
    }

    /// Puts `tab`, taken from the other pane with its listing and generation, at `slot` (past
    /// the end: after the last tab), and shows it.
    pub fn insert_tab(&mut self, slot: usize, tab: Tab) {
        let at = slot.min(self.tabs.len());
        self.tabs.insert(at, tab);
        self.active = at;
    }

    /// The position of the tab whose id is `id`, if it is in this pane.
    pub fn position(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
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
            // A tab never asks for names (`Loader::list_names`).
            LoadEvent::Names { .. } => return Handled::Stale,
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

    /// A tab at `at` that has listed `count` files `f0`, `f1`, … of growing sizes, more than
    /// the UI thread sorts.
    fn long_tab(count: usize) -> Tab {
        ready_tab(
            "long",
            (0..count)
                .map(|i| file(&format!("f{i}"), u64::try_from(i).unwrap_or(0)))
                .collect(),
        )
    }

    #[test]
    fn a_long_listing_is_sorted_by_a_worker_and_the_rows_wait_in_their_order() {
        let count = INLINE_SORT_ROWS + 1;
        let mut tab = long_tab(count);
        {
            let (selection, rows) = tab.selection_mut();
            selection.click(1, rows);
        }
        tab.set_sort(SortKey::Size);
        // The header says the new order at once; the rows keep the old one meanwhile.
        assert_eq!(tab.sort().primary.key, SortKey::Size);
        assert_eq!(tab.visible()[0].name, "f0");
        assert_eq!(tab.visible()[1].name, "f1");
        let request = tab.take_sort_request().expect("a worker sorts");
        assert_eq!(request.generation, tab.generation());
        assert_eq!(request.entries.len(), count);
        // Asked once.
        assert_eq!(tab.take_sort_request(), None);

        // A change while the worker sorts goes on top of what it gives back.
        let generation = tab.generation();
        assert!(tab.apply_changes(
            generation,
            vec![
                Change::Upsert(file("new", 0)),
                Change::Removed(OsString::from("f2"))
            ]
        ));
        let by_size = request.run();
        assert!(tab.apply_sort(by_size.clone()));
        assert_eq!(tab.visible().len(), count);
        assert_eq!(names(&tab.visible()[..3]), ["f0", "new", "f1"]);
        assert!(!tab.visible().iter().any(|entry| entry.name == "f2"));
        // The selection follows its name, and the same rows again are no news.
        assert_eq!(selected(&tab), ["f1"]);
        assert!(!tab.apply_sort(by_size));

        // Another order asked for before the first came: the first is dropped.
        tab.set_sort(SortKey::Size);
        let first = tab.take_sort_request().expect("descending");
        tab.set_sort(SortKey::Name);
        let second = tab.take_sort_request().expect("by name");
        assert!(!tab.apply_sort(first.run()));
        assert!(tab.apply_sort(second.run()));
        assert_eq!(tab.visible()[0].name, "f0");

        // A reload drops a sort in progress; so does a move elsewhere.
        tab.set_sort(SortKey::Size);
        let late = tab.take_sort_request().expect("a worker sorts");
        let reload = tab.reload();
        assert!(!tab.apply_sort(late.run()));
        // The listing comes in the order asked for: no worker needed.
        assert_eq!(reload.sort, tab.sort());
        assert!(tab.apply_sorted(reload.generation, vec![file("b", 2), file("a", 1)]));
        assert_eq!(tab.take_sort_request(), None);
    }

    #[test]
    fn a_short_listing_is_sorted_at_once_and_a_listing_in_an_old_order_waits_for_a_worker() {
        let mut tab = long_tab(INLINE_SORT_ROWS);
        tab.set_sort(SortKey::Size);
        tab.set_sort(SortKey::Size);
        assert_eq!(tab.take_sort_request(), None);
        let first = format!("f{}", INLINE_SORT_ROWS - 1);
        assert_eq!(tab.visible()[0].name.to_str(), Some(first.as_str()));

        // A long listing asked for in the order before a change of it shows as it came, then a
        // worker sorts it.
        let request = tab.reload();
        tab.set_sort(SortKey::Name);
        let came: Vec<Entry> = (0..=INLINE_SORT_ROWS)
            .rev()
            .map(|i| file(&format!("f{i}"), 0))
            .collect();
        assert!(tab.apply_sorted(request.generation, came));
        let last = format!("f{INLINE_SORT_ROWS}");
        assert_eq!(tab.visible()[0].name.to_str(), Some(last.as_str()));
        let sort = tab.take_sort_request().expect("a worker sorts");
        assert!(tab.apply_sort(sort.run()));
        assert_eq!(tab.visible()[0].name, "f0");
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
    fn a_name_filter_keeps_the_rows_that_match_and_says_so() {
        let mut tab = ready_tab(
            "a",
            vec![
                dir("docs"),
                file("Alpha.txt", 1),
                file("beta.TXT", 2),
                file("gamma.md", 3),
                hidden(file(".notes.txt", 4)),
            ],
        );
        {
            let (selection, rows) = tab.selection_mut();
            selection.click(1, rows);
            selection.toggle(3, rows);
        }
        // Any part of the name, in any case; hidden items stay hidden.
        tab.set_name_filter("TxT");
        assert_eq!(tab.name_filter(), "TxT");
        assert_eq!(names(tab.visible()), ["Alpha.txt", "beta.TXT"]);
        assert_eq!(names(tab.all()).len(), 5);
        // The selection keeps the rows that still show.
        assert_eq!(selected(&tab), ["Alpha.txt"]);
        let generation = tab.generation();
        assert!(tab.apply_space(generation, 212 * 1024 * 1024 * 1024));
        assert_eq!(
            tab.status_line(),
            "거르는 중: ‘TxT’ · 항목 2개 / 4개 · 선택 1개 (1 바이트) · 남은 공간 212 GB"
        );
        // A row that comes is filtered too.
        assert!(tab.apply_changes(
            generation,
            vec![
                Change::Upsert(file("delta.txt", 1)),
                Change::Upsert(file("epsilon", 1))
            ]
        ));
        assert_eq!(names(tab.visible()), ["Alpha.txt", "beta.TXT", "delta.txt"]);
        // A reload keeps it; nothing matches: no rows, and the count says why.
        tab.reload();
        tab.set_name_filter("zzz");
        assert_eq!(names(tab.visible()), NONE);
        assert!(
            tab.status_line()
                .starts_with("거르는 중: ‘zzz’ · 항목 0개 / 6개")
        );
        // Cleared, every row shows again, and the line is as before.
        tab.set_name_filter("");
        assert_eq!(tab.visible().len(), 6);
        assert!(tab.status_line().starts_with("항목 6개"));
        // It filters this folder only: another folder starts without it.
        tab.set_name_filter("txt");
        tab.navigate(folder("b"));
        assert_eq!(tab.name_filter(), "");
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
