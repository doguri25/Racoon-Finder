//! How a pane shows its folder (NAV-06, NAV-07, NAV-08, NAV-10), and the commands that change
//! it. A child of [`state`](super), so these methods see the fields of [`AppState`].
//!
//! A [`FolderView`] is the view mode ([`ViewMode`]: 자세히, 목록, or icons of three sizes), the
//! columns of 자세히 after the name ([`ShownColumn`]: which, in what order, how wide) and the
//! order of the rows (a `SortSpec`: a key, a second key for ties, folders first or mixed in).
//!
//! The views are remembered per folder ([`ViewStore`], kept in the settings): a change of the
//! view of a tab is remembered for its folder, and a tab that goes to a folder shows the view
//! remembered for it. A folder without a view of its own shows the view of the nearest folder
//! above it that applies its view to its subfolders (하위 폴더에 적용), or else the default one
//! ([`FolderView::default`]). The store keeps at most [`MAX_FOLDER_VIEWS`] folders, and drops
//! the one used longest ago first (a use is a tab showing the folder's view, or a change of it).
//! Folders are known by their `VPath::to_uri`, compared as the system compares names: ignoring
//! case on Windows and macOS (`rf_vfs::fold_name`), exactly elsewhere.
//!
//! The keys move the cursor as the view lays the rows out ([`target`]): 자세히 one row per line,
//! the icons left to right and then down (row-major), 목록 top to bottom and then right
//! (column-major).

use std::ffi::OsStr;

use rf_core::{Os, VPath};
use rf_vfs::{SortBy, SortKey, SortOrder, SortSpec, fold_name};

use super::{AppState, Effect};
use crate::pane::{Pane, ancestors};
use crate::selection::Pick;
use crate::words;

/// How many folders the store remembers a view for; past it, the one used longest ago goes.
pub const MAX_FOLDER_VIEWS: usize = 1000;
/// A folder whose `VPath::to_uri` is longer than this many bytes has no view remembered: a
/// thousand of them would make a settings file too large to read back.
pub const MAX_FOLDER_URI: usize = 2048;
/// The narrowest and the widest a column of 자세히 can be made, in logical pixels.
pub const MIN_COLUMN_WIDTH: u32 = 40;
pub const MAX_COLUMN_WIDTH: u32 = 600;

/// How a pane lays out its rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ViewMode {
    /// One row per item, with columns (자세히).
    #[default]
    Details,
    /// Names with small icons, top to bottom and then in the next column (목록).
    List,
    /// Icons left to right and then down, with the name beside a small one or under a larger
    /// one (작은 아이콘, 보통 아이콘, 큰 아이콘). Thumbnails are for a later version.
    SmallIcons,
    MediumIcons,
    LargeIcons,
}

impl ViewMode {
    /// Every mode, in the order the 보기 menu lists them after the largest icons (Explorer's).
    pub const ALL: [ViewMode; 5] = [
        ViewMode::LargeIcons,
        ViewMode::MediumIcons,
        ViewMode::SmallIcons,
        ViewMode::List,
        ViewMode::Details,
    ];

    /// The mode's name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            ViewMode::Details => "details",
            ViewMode::List => "list",
            ViewMode::SmallIcons => "small-icons",
            ViewMode::MediumIcons => "medium-icons",
            ViewMode::LargeIcons => "large-icons",
        }
    }

    /// The mode the settings file calls `name`; `None` for a name this version does not know.
    pub fn from_name(name: &str) -> Option<ViewMode> {
        ViewMode::ALL.into_iter().find(|mode| mode.name() == name)
    }

    /// The number the screen knows the mode by (`PaneView.mode`).
    pub fn number(self) -> i32 {
        match self {
            ViewMode::Details => 0,
            ViewMode::List => 1,
            ViewMode::SmallIcons => 2,
            ViewMode::MediumIcons => 3,
            ViewMode::LargeIcons => 4,
        }
    }

    /// The mode of the screen's number; `None` for no mode.
    pub fn from_number(number: i32) -> Option<ViewMode> {
        ViewMode::ALL
            .into_iter()
            .find(|mode| mode.number() == number)
    }
}

/// A column of 자세히. The name always shows, first, and takes the room the others leave; the
/// others show or not, in the order the user put them, as wide as the user made them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    Name,
    Size,
    Modified,
    Created,
    Kind,
    Extension,
    /// The attribute letters (R, H, S, A, ...), [`crate::format::attribute_letters`].
    Attributes,
}

impl Column {
    /// The columns besides the name, in the order the header's menu lists them.
    pub const OPTIONAL: [Column; 6] = [
        Column::Size,
        Column::Modified,
        Column::Created,
        Column::Kind,
        Column::Extension,
        Column::Attributes,
    ];

    /// The column's name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            Column::Name => "name",
            Column::Size => "size",
            Column::Modified => "modified",
            Column::Created => "created",
            Column::Kind => "kind",
            Column::Extension => "extension",
            Column::Attributes => "attributes",
        }
    }

    /// The column the settings file calls `name`; `None` for one this version does not know.
    pub fn from_name(name: &str) -> Option<Column> {
        std::iter::once(Column::Name)
            .chain(Column::OPTIONAL)
            .find(|column| column.name() == name)
    }

    /// The number the screen knows the column by: 0 name, 1 size, 2 modified, 3 kind (the
    /// numbers of the first demo's column heads), 4 created, 5 extension, 6 attributes.
    pub fn number(self) -> i32 {
        match self {
            Column::Name => 0,
            Column::Size => 1,
            Column::Modified => 2,
            Column::Kind => 3,
            Column::Created => 4,
            Column::Extension => 5,
            Column::Attributes => 6,
        }
    }

    /// The column of the screen's number; `None` for no column.
    pub fn from_number(number: i32) -> Option<Column> {
        std::iter::once(Column::Name)
            .chain(Column::OPTIONAL)
            .find(|column| column.number() == number)
    }

    /// What its head says.
    pub fn title(self) -> &'static str {
        words::column_title(self)
    }

    /// The key a click on its head sorts by; `None` for the attributes, which sort nothing.
    pub fn sort_key(self) -> Option<SortKey> {
        Some(match self {
            Column::Name => SortKey::Name,
            Column::Size => SortKey::Size,
            Column::Modified => SortKey::Modified,
            Column::Created => SortKey::Created,
            Column::Kind => SortKey::Kind,
            Column::Extension => SortKey::Extension,
            Column::Attributes => return None,
        })
    }

    /// The column whose head sorts by `key`.
    pub fn of_key(key: SortKey) -> Column {
        match key {
            SortKey::Name => Column::Name,
            SortKey::Size => Column::Size,
            SortKey::Modified => Column::Modified,
            SortKey::Created => Column::Created,
            SortKey::Kind => Column::Kind,
            SortKey::Extension => Column::Extension,
        }
    }

    /// How wide it is until the user makes it otherwise, in logical pixels: the date columns
    /// hold the widest `2026-10-28 22:08`.
    pub fn default_width(self) -> u32 {
        match self {
            Column::Name => 120,
            Column::Size => 72,
            Column::Modified | Column::Created => 126,
            Column::Kind => 104,
            Column::Extension => 64,
            Column::Attributes => 56,
        }
    }

    /// Whether its text lines up on the right (the sizes).
    pub fn align_end(self) -> bool {
        self == Column::Size
    }
}

/// A column of 자세히 after the name, and its width in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShownColumn {
    pub column: Column,
    pub width: u32,
}

impl ShownColumn {
    /// `column` at its default width.
    pub fn new(column: Column) -> ShownColumn {
        ShownColumn {
            column,
            width: column.default_width(),
        }
    }
}

/// How a pane shows a folder: the view mode, the columns of 자세히 after the name, and the
/// order of the rows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FolderView {
    pub mode: ViewMode,
    /// Never [`Column::Name`], and each column once.
    pub columns: Vec<ShownColumn>,
    pub sort: SortSpec,
}

impl Default for FolderView {
    /// 자세히 with the size, the modification time and the kind after the name (the first
    /// demo's columns), by name ascending, folders first.
    fn default() -> FolderView {
        FolderView {
            mode: ViewMode::Details,
            columns: [Column::Size, Column::Modified, Column::Kind]
                .into_iter()
                .map(ShownColumn::new)
                .collect(),
            sort: SortSpec::default(),
        }
    }
}

impl FolderView {
    /// The default view in the order `sort`.
    pub fn sorted(sort: SortSpec) -> FolderView {
        FolderView {
            sort,
            ..FolderView::default()
        }
    }

    /// Whether `column` shows (the name always does).
    pub fn shows(&self, column: Column) -> bool {
        column == Column::Name || self.columns.iter().any(|shown| shown.column == column)
    }

    /// Shows `column` after the others, at its default width, or hides it. The name always
    /// shows.
    pub fn toggle_column(&mut self, column: Column) {
        if column == Column::Name {
            return;
        }
        match self.columns.iter().position(|shown| shown.column == column) {
            Some(at) => {
                self.columns.remove(at);
            }
            None => self.columns.push(ShownColumn::new(column)),
        }
    }

    /// Moves the column at `from` (among the columns after the name) to `slot`, a place between
    /// them as they are now (0 before the first, their number after the last). `false`, and no
    /// change, when that leaves it where it is or a position is past the end.
    pub fn move_column(&mut self, from: usize, slot: usize) -> bool {
        let count = self.columns.len();
        if from >= count || slot > count || slot == from || slot == from + 1 {
            return false;
        }
        let moved = self.columns.remove(from);
        let to = if slot > from { slot - 1 } else { slot };
        self.columns.insert(to, moved);
        true
    }

    /// Makes `column` `width` logical pixels wide, between [`MIN_COLUMN_WIDTH`] and
    /// [`MAX_COLUMN_WIDTH`]; nothing for a column that does not show, or the name, which takes
    /// the room the others leave.
    pub fn set_width(&mut self, column: Column, width: u32) {
        if let Some(shown) = self.columns.iter_mut().find(|shown| shown.column == column) {
            shown.width = width.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH);
        }
    }

    /// A click on the head that sorts by `key` (with Shift: `then`). A plain click sorts by the
    /// key alone, ascending, or the other way round when it is the key that sorts now (the
    /// second key stays then). With Shift, it is the second key, which breaks the ties of the
    /// first: ascending, or the other way round when it is the second key already; on the
    /// first key's head, Shift does what a plain click does.
    pub fn sort_by(&mut self, key: SortKey, then: bool) {
        let flip = |order: SortOrder| match order {
            SortOrder::Ascending => SortOrder::Descending,
            SortOrder::Descending => SortOrder::Ascending,
        };
        let sort = &mut self.sort;
        if sort.primary.key == key {
            sort.primary.order = flip(sort.primary.order);
        } else if !then {
            sort.primary = SortBy {
                key,
                order: SortOrder::Ascending,
            };
            sort.secondary = None;
        } else {
            sort.secondary = Some(match sort.secondary {
                Some(by) if by.key == key => SortBy {
                    key,
                    order: flip(by.order),
                },
                _ => SortBy {
                    key,
                    order: SortOrder::Ascending,
                },
            });
        }
    }

    /// The second key, ascending (as it is, when it is that key already), or none; the key that
    /// sorts first is no second key.
    pub fn set_then(&mut self, key: Option<SortKey>) {
        let sort = &mut self.sort;
        sort.secondary = match key {
            None => None,
            Some(key) if key == sort.primary.key => None,
            Some(key) => match sort.secondary {
                Some(by) if by.key == key => Some(by),
                _ => Some(SortBy {
                    key,
                    order: SortOrder::Ascending,
                }),
            },
        };
    }
}

/// A key that moves the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyMove {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

/// How the rows lie on screen, for the keys: how many items a line holds (a row of icons, a
/// column of 목록, 1 in 자세히) and how many lines a page key moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paging {
    pub line: usize,
    pub page: usize,
}

impl Paging {
    /// One item a line and `page` lines a page: 자세히, where a page is rows.
    pub fn rows(page: usize) -> Paging {
        Paging { line: 1, page }
    }
}

/// Where `step` takes the cursor at `cursor` (`None`: no cursor yet) among `count` items laid
/// out by `mode` as `paging` says; `None` for a step that does not move in this mode (← and →
/// in 자세히) or an empty list.
///
/// - 자세히: ↑ and ↓ one row, the page keys a page of rows, Home and End the first and the last
///   row, and nothing for ← and →. Without a cursor, ↓ lands on the first row, and so does ↑.
/// - Icons (row-major): ← and → the item before and after (from the end of a row to the next
///   one), ↑ and ↓ the item a row up and down (from a last row that is not full, ↓ goes to the
///   last item; at the first and the last row they stay), the page keys a page of rows up and
///   down, as far as the first and the last row.
/// - 목록 (column-major): ↑ and ↓ the item before and after (from the end of a column to the
///   next one), ← and → the item a column left and right, the page keys a page of columns.
///
/// Without a cursor every step but End lands on the first item.
pub fn target(
    mode: ViewMode,
    cursor: Option<usize>,
    count: usize,
    step: KeyMove,
    paging: Paging,
) -> Option<usize> {
    let last = count.checked_sub(1)?;
    let line = paging.line.max(1);
    let page = paging.page.max(1);
    if mode == ViewMode::Details {
        // As `Selection::move_cursor` moves in the first demo.
        let delta = match step {
            KeyMove::Left | KeyMove::Right => return None,
            KeyMove::Up => -1,
            KeyMove::Down => 1,
            KeyMove::Home => isize::MIN,
            KeyMove::End => isize::MAX,
            KeyMove::PageUp => -isize::try_from(page).unwrap_or(isize::MAX),
            KeyMove::PageDown => isize::try_from(page).unwrap_or(isize::MAX),
        };
        let to = match cursor {
            Some(cursor) => cursor.saturating_add_signed(delta),
            None if delta > 0 => delta.unsigned_abs() - 1,
            None => 0,
        };
        return Some(to.min(last));
    }
    let Some(cursor) = cursor.map(|cursor| cursor.min(last)) else {
        return Some(if step == KeyMove::End { last } else { 0 });
    };
    // The steps along the flow (an item) and across it (a line of `line` items): the icons
    // flow along rows, 목록 down columns.
    let list = mode == ViewMode::List;
    let page = isize::try_from(page).unwrap_or(isize::MAX);
    let (along, across): (isize, isize) = match step {
        KeyMove::Home => return Some(0),
        KeyMove::End => return Some(last),
        KeyMove::Up if list => (-1, 0),
        KeyMove::Down if list => (1, 0),
        KeyMove::Left if list => (0, -1),
        KeyMove::Right if list => (0, 1),
        KeyMove::Left => (-1, 0),
        KeyMove::Right => (1, 0),
        KeyMove::Up => (0, -1),
        KeyMove::Down => (0, 1),
        KeyMove::PageUp => (0, -page),
        KeyMove::PageDown => (0, page),
    };
    if along != 0 {
        return Some(cursor.saturating_add_signed(along).min(last));
    }
    // A move across the lines keeps the place in the line, as far as the first and the last
    // line; a last line too short to have that place gives its last item.
    let (at, place) = (cursor / line, cursor % line);
    let to = at.saturating_add_signed(across).min(last / line);
    Some((to * line + place).min(last))
}

/// The folder views remembered (NAV-10), the one used longest ago first.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewStore {
    /// With each the key it is found by ([`ViewStore::key`]).
    entries: Vec<(String, StoredView)>,
    /// Whose rule of case the keys follow.
    os: Os,
}

/// One folder's view in the [`ViewStore`].
#[derive(Clone, Debug, PartialEq)]
pub struct StoredView {
    /// The folder, as `VPath::to_uri` gave it (as the settings file has it).
    pub folder: String,
    pub view: FolderView,
    /// 하위 폴더에 적용: the folders below show this view too, unless they have one of their own.
    pub subfolders: bool,
    /// The keys of a newer version, written back as they are.
    pub extra: toml::Table,
}

impl Default for ViewStore {
    fn default() -> ViewStore {
        ViewStore::new(Vec::new(), Os::CURRENT)
    }
}

impl ViewStore {
    /// A store of `entries`, the one used longest ago first, with the case rule of `os`. Of two
    /// entries for one folder the later one is kept, and of more than [`MAX_FOLDER_VIEWS`] the
    /// last ones.
    pub fn new(entries: Vec<StoredView>, os: Os) -> ViewStore {
        let mut store = ViewStore {
            entries: Vec::with_capacity(entries.len().min(MAX_FOLDER_VIEWS)),
            os,
        };
        for entry in entries {
            let key = store.key_of(&entry.folder);
            store.entries.retain(|(known, _)| *known != key);
            store.entries.push((key, entry));
        }
        store.trim();
        store
    }

    /// The folders' views, the one used longest ago first.
    pub fn entries(&self) -> impl Iterator<Item = &StoredView> {
        self.entries.iter().map(|(_, entry)| entry)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The view of `folder`, as its own, or the one it takes from above, or the default one
    /// (see the module's documentation). The entry it comes from counts as used.
    pub fn view_for(&mut self, folder: &VPath) -> FolderView {
        if let Some(at) = self.position(&self.key(folder)) {
            return self.touch(at).view.clone();
        }
        // The nearest folder above that applies its view to its subfolders.
        let mut above = ancestors(folder);
        above.pop();
        let found = above.iter().rev().find_map(|parent| {
            let at = self.position(&self.key(parent))?;
            self.entries[at].1.subfolders.then_some(at)
        });
        match found {
            Some(at) => self.touch(at).view.clone(),
            None => FolderView::default(),
        }
    }

    /// The view `folder` has of its own, if it has one.
    pub fn own(&self, folder: &VPath) -> Option<&StoredView> {
        self.position(&self.key(folder))
            .map(|at| &self.entries[at].1)
    }

    /// Remembers `view` as the view of `folder`, which keeps whether it applies to its
    /// subfolders; it is the one used last. Past [`MAX_FOLDER_VIEWS`] the one used longest ago
    /// goes. Nothing for a folder whose `to_uri` is longer than [`MAX_FOLDER_URI`].
    pub fn remember(&mut self, folder: &VPath, view: FolderView) {
        let uri = folder.to_uri();
        if uri.len() > MAX_FOLDER_URI {
            return;
        }
        let key = self.key_of(&uri);
        if let Some(at) = self.position(&key) {
            self.touch(at).view = view;
        } else {
            self.entries.push((
                key,
                StoredView {
                    folder: uri,
                    view,
                    subfolders: false,
                    extra: toml::Table::new(),
                },
            ));
            self.trim();
        }
    }

    /// 하위 폴더에 적용: `view` is the view of `folder` and of every folder below it. The folders
    /// below forget their own views, so that they show this one, now and when they are made;
    /// one that is changed later has its own again. `false`, and no change, for a folder whose
    /// view is not remembered ([`MAX_FOLDER_URI`]).
    pub fn apply_to_subfolders(&mut self, folder: &VPath, view: FolderView) -> bool {
        self.remember(folder, view);
        let key = self.key(folder);
        let Some(at) = self.position(&key) else {
            return false;
        };
        self.entries[at].1.subfolders = true;
        let base = VPath::parse(&key).ok();
        self.entries.retain(|(known, _)| {
            let below = base
                .as_ref()
                .zip(VPath::parse(known).ok())
                .is_some_and(|(base, path)| path != *base && path.starts_with(base));
            !below
        });
        true
    }

    /// 기본값으로 되돌리기: `folder` forgets its own view and whether it applied it to its
    /// subfolders; it shows what it takes from above, or the default view. The folders below
    /// keep their own views. `false` when it had none.
    pub fn reset(&mut self, folder: &VPath) -> bool {
        let Some(at) = self.position(&self.key(folder)) else {
            return false;
        };
        self.entries.remove(at);
        true
    }

    /// The key `folder` is found by: its `to_uri`, folded on a system that ignores case.
    fn key(&self, folder: &VPath) -> String {
        self.key_of(&folder.to_uri())
    }

    fn key_of(&self, uri: &str) -> String {
        if self.os == Os::Other {
            uri.to_owned()
        } else {
            fold_name(OsStr::new(uri))
        }
    }

    fn position(&self, key: &str) -> Option<usize> {
        self.entries.iter().position(|(known, _)| known == key)
    }

    /// Makes the entry at `at` the one used last, and gives it.
    fn touch(&mut self, at: usize) -> &mut StoredView {
        let entry = self.entries.remove(at);
        self.entries.push(entry);
        &mut self.entries.last_mut().expect("just pushed").1
    }

    /// Drops the entries used longest ago past [`MAX_FOLDER_VIEWS`].
    fn trim(&mut self) {
        let over = self.entries.len().saturating_sub(MAX_FOLDER_VIEWS);
        self.entries.drain(..over);
    }
}

impl AppState {
    /// The views remembered per folder (the settings keep them).
    pub fn views(&self) -> &ViewStore {
        &self.views
    }

    /// Takes the views remembered per folder (from the settings, at the start).
    pub fn set_views(&mut self, views: ViewStore) {
        self.views = views;
    }

    /// Gives each tab that has come to another folder the view remembered for it, and hands the
    /// sorts that are too long for the UI thread to a worker. The controller calls this after
    /// every change, with the change's effects: the listing a tab has just asked for (a
    /// [`Effect::Load`] among them) is asked for in the order of the folder's view, so the worker
    /// that lists it sorts it so. A sort a tab needs ([`Tab::take_sort_request`]) becomes an
    /// [`Effect::Sort`].
    ///
    /// [`Tab::take_sort_request`]: crate::pane::Tab::take_sort_request
    pub fn settle_views(&mut self, effects: &mut Vec<Effect>) {
        let views = &mut self.views;
        for tab in self.panes.iter_mut().flat_map(Pane::tabs_mut) {
            if tab.view_for() != Some(tab.location()) {
                let view = views.view_for(tab.location());
                let generation = tab.generation();
                let request = effects.iter_mut().find_map(|effect| match effect {
                    Effect::Load(request) if request.generation == generation => Some(request),
                    _ => None,
                });
                tab.settle_view(view, request);
            }
        }
        for tab in self.panes.iter_mut().flat_map(Pane::tabs_mut) {
            if let Some(request) = tab.take_sort_request() {
                effects.push(Effect::Sort(request));
            }
        }
    }

    /// A worker sorted the rows of a tab ([`Effect::Sort`]): the tab that asked takes them, if
    /// it still waits for them.
    pub fn sorted(&mut self, sorted: crate::pane::Sorted) -> Vec<Effect> {
        if let Some(tab) = self.tab_by_generation(sorted.generation) {
            tab.apply_sort(sorted);
        }
        Vec::new()
    }

    /// Changes the view of the tab pane `side` shows with `change`, and remembers it for the
    /// tab's folder; the pane gets the keyboard.
    fn change_view(&mut self, side: usize, change: impl FnOnce(&mut FolderView)) {
        self.activate(side);
        let tab = self.panes[side].active_tab_mut();
        let mut view = tab.view().clone();
        change(&mut view);
        tab.set_view(view.clone());
        let folder = tab.location().clone();
        self.views.remember(&folder, view);
    }

    /// Shows pane `side` in `mode`, and scrolls to its cursor there.
    pub fn set_view_mode(&mut self, side: usize, mode: ViewMode) -> Vec<Effect> {
        self.change_view(side, |view| view.mode = mode);
        let row = self.tab(side).selection().cursor().unwrap_or(0);
        vec![Effect::ScrollTo { side, row }]
    }

    /// Shows or hides `column` in pane `side` (the head's menu, the 보기 menu's 열).
    pub fn toggle_column(&mut self, side: usize, column: Column) -> Vec<Effect> {
        self.change_view(side, |view| view.toggle_column(column));
        Vec::new()
    }

    /// Moves the column at `from` among those after the name to `slot` in pane `side` (a head
    /// dragged along the header).
    pub fn move_column(&mut self, side: usize, from: usize, slot: usize) -> Vec<Effect> {
        self.change_view(side, |view| {
            view.move_column(from, slot);
        });
        Vec::new()
    }

    /// Makes `column` of pane `side` `width` logical pixels wide (its border dragged, or a
    /// double click on it).
    pub fn set_column_width(&mut self, side: usize, column: Column, width: u32) -> Vec<Effect> {
        self.change_view(side, |view| view.set_width(column, width));
        Vec::new()
    }

    /// A click on a head of pane `side` that sorts by `key` (`then` with Shift: the second
    /// key), as [`FolderView::sort_by`] says.
    pub fn sort_by(&mut self, side: usize, key: SortKey, then: bool) -> Vec<Effect> {
        self.change_view(side, |view| view.sort_by(key, then));
        Vec::new()
    }

    /// The 보기 menu's 2차 정렬 for pane `side`: `key`, or none.
    pub fn sort_then(&mut self, side: usize, key: Option<SortKey>) -> Vec<Effect> {
        self.change_view(side, |view| view.set_then(key));
        Vec::new()
    }

    /// Folders first, or mixed in with the files (NAV-08), in pane `side`.
    pub fn toggle_folders_first(&mut self, side: usize) -> Vec<Effect> {
        self.change_view(side, |view| {
            view.sort.folders_first = !view.sort.folders_first;
        });
        Vec::new()
    }

    /// 하위 폴더에 적용: the view of pane `side` is the view of its folder and of every folder
    /// below it ([`ViewStore::apply_to_subfolders`]); a notice says so. The tabs that show a
    /// folder below it now take it when they go to a folder next.
    pub fn apply_view_to_subfolders(&mut self, side: usize) -> Vec<Effect> {
        self.activate(side);
        let tab = self.panes[side].active_tab();
        let (folder, view) = (tab.location().clone(), tab.view().clone());
        if !self.views.apply_to_subfolders(&folder, view) {
            return vec![Effect::Notice(words::view_not_kept().to_owned())];
        }
        vec![Effect::Notice(words::view_applied_below(&folder_name(
            &folder,
        )))]
    }

    /// 기본값으로 되돌리기: the folder of pane `side` forgets its own view
    /// ([`ViewStore::reset`]) and shows what it takes from above, or the default view.
    pub fn reset_view(&mut self, side: usize) -> Vec<Effect> {
        self.activate(side);
        let folder = self.tab(side).location().clone();
        self.views.reset(&folder);
        let tab = self.tab_mut(side);
        tab.unsettle_view();
        let mut effects = Vec::new();
        self.settle_views(&mut effects);
        effects.push(Effect::Notice(words::view_reset(&folder_name(&folder))));
        effects
    }

    /// A key that moves the cursor of pane `side` by `step`, the view laying its rows out as
    /// `paging` says: Shift selects the range from the anchor to where it goes, Ctrl moves the
    /// cursor alone, and with neither the row it goes to is selected alone.
    pub(super) fn step_cursor(
        &mut self,
        side: usize,
        step: KeyMove,
        pick: Pick,
        paging: Paging,
    ) -> Vec<Effect> {
        let tab = self.tab_mut(side);
        let mode = tab.view().mode;
        let (selection, rows) = tab.selection_mut();
        let Some(to) = target(mode, selection.cursor(), rows.len(), step, paging) else {
            return Vec::new();
        };
        selection.go_to(to, pick, rows);
        vec![Effect::ScrollTo { side, row: to }]
    }
}

/// The name a notice gives `folder`: its last part, or the whole path at a root.
fn folder_name(folder: &VPath) -> String {
    folder.file_name().map_or_else(
        || folder.to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(path: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(path)).expect("a local path")
    }

    fn icons(mode: ViewMode) -> FolderView {
        FolderView {
            mode,
            ..FolderView::default()
        }
    }

    const GRID: Paging = Paging { line: 4, page: 2 };

    #[test]
    fn icons_move_along_rows_and_down_columns() {
        let at = |cursor, step| target(ViewMode::MediumIcons, Some(cursor), 10, step, GRID);
        // Rows of four: 0 1 2 3 / 4 5 6 7 / 8 9.
        assert_eq!(at(1, KeyMove::Right), Some(2));
        assert_eq!(
            at(3, KeyMove::Right),
            Some(4),
            "from a row's end to the next row"
        );
        assert_eq!(at(9, KeyMove::Right), Some(9));
        assert_eq!(at(4, KeyMove::Left), Some(3));
        assert_eq!(at(0, KeyMove::Left), Some(0));
        assert_eq!(at(1, KeyMove::Down), Some(5));
        assert_eq!(at(5, KeyMove::Down), Some(9));
        assert_eq!(
            at(6, KeyMove::Down),
            Some(9),
            "the last row is short: its last item"
        );
        assert_eq!(at(9, KeyMove::Down), Some(9));
        assert_eq!(at(6, KeyMove::Up), Some(2));
        assert_eq!(at(2, KeyMove::Up), Some(2), "the first row stays");
        // A page is two rows here, as far as the first and the last row.
        assert_eq!(at(1, KeyMove::PageDown), Some(9));
        assert_eq!(at(9, KeyMove::PageUp), Some(1));
        assert_eq!(at(5, KeyMove::PageUp), Some(1));
        assert_eq!(at(5, KeyMove::Home), Some(0));
        assert_eq!(at(5, KeyMove::End), Some(9));
        // Without a cursor every step starts at the first item, End at the last.
        let fresh = |step| target(ViewMode::LargeIcons, None, 10, step, GRID);
        assert_eq!(fresh(KeyMove::Down), Some(0));
        assert_eq!(fresh(KeyMove::Right), Some(0));
        assert_eq!(fresh(KeyMove::End), Some(9));
        assert_eq!(
            target(ViewMode::SmallIcons, Some(0), 0, KeyMove::Down, GRID),
            None
        );
    }

    #[test]
    fn the_list_moves_down_columns_and_across_them() {
        // Columns of four: 0 4 8 / 1 5 9 / 2 6 / 3 7.
        let at = |cursor, step| target(ViewMode::List, Some(cursor), 10, step, GRID);
        assert_eq!(at(1, KeyMove::Down), Some(2));
        assert_eq!(
            at(3, KeyMove::Down),
            Some(4),
            "from a column's end to the next one"
        );
        assert_eq!(at(4, KeyMove::Up), Some(3));
        assert_eq!(at(1, KeyMove::Right), Some(5));
        assert_eq!(
            at(6, KeyMove::Right),
            Some(9),
            "the last column is short: its last item"
        );
        assert_eq!(at(9, KeyMove::Right), Some(9));
        assert_eq!(at(5, KeyMove::Left), Some(1));
        assert_eq!(at(1, KeyMove::Left), Some(1), "the first column stays");
        assert_eq!(at(1, KeyMove::PageDown), Some(9));
        assert_eq!(at(9, KeyMove::PageUp), Some(1));
    }

    #[test]
    fn details_move_by_rows_and_not_sideways() {
        let rows = Paging::rows(3);
        let at = |cursor, step| target(ViewMode::Details, cursor, 10, step, rows);
        assert_eq!(at(Some(4), KeyMove::Down), Some(5));
        assert_eq!(at(Some(4), KeyMove::Up), Some(3));
        assert_eq!(at(Some(4), KeyMove::Left), None);
        assert_eq!(at(Some(4), KeyMove::Right), None);
        assert_eq!(at(Some(4), KeyMove::PageDown), Some(7));
        assert_eq!(at(Some(8), KeyMove::PageDown), Some(9));
        assert_eq!(at(Some(1), KeyMove::PageUp), Some(0));
        assert_eq!(at(None, KeyMove::Down), Some(0));
        assert_eq!(at(None, KeyMove::Up), Some(0));
        assert_eq!(at(None, KeyMove::PageDown), Some(2));
        assert_eq!(at(None, KeyMove::End), Some(9));
    }

    #[test]
    fn columns_show_move_and_take_their_widths() {
        let mut view = FolderView::default();
        let shown = |view: &FolderView| -> Vec<Column> {
            view.columns.iter().map(|shown| shown.column).collect()
        };
        assert_eq!(shown(&view), [Column::Size, Column::Modified, Column::Kind]);
        assert!(view.shows(Column::Name));
        view.toggle_column(Column::Created);
        view.toggle_column(Column::Modified);
        view.toggle_column(Column::Name);
        assert_eq!(shown(&view), [Column::Size, Column::Kind, Column::Created]);
        assert!(!view.shows(Column::Modified));
        // The first one goes after the last; a move that leaves it in place is none.
        assert!(view.move_column(0, 3));
        assert_eq!(shown(&view), [Column::Kind, Column::Created, Column::Size]);
        assert!(!view.move_column(1, 1) && !view.move_column(1, 2));
        assert!(!view.move_column(5, 0));
        assert!(view.move_column(2, 0));
        assert_eq!(shown(&view), [Column::Size, Column::Kind, Column::Created]);
        view.set_width(Column::Kind, 5);
        view.set_width(Column::Created, 10_000);
        view.set_width(Column::Size, 90);
        let widths: Vec<u32> = view.columns.iter().map(|shown| shown.width).collect();
        assert_eq!(widths, [90, MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH]);
        // A column shown again starts at its default width.
        view.toggle_column(Column::Kind);
        view.toggle_column(Column::Kind);
        assert_eq!(view.columns[2], ShownColumn::new(Column::Kind));
    }

    #[test]
    fn heads_sort_by_one_key_then_another() {
        let mut view = FolderView::default();
        let by = |key, order| SortBy { key, order };
        view.sort_by(SortKey::Size, false);
        assert_eq!(view.sort.primary, by(SortKey::Size, SortOrder::Ascending));
        // Shift: the second key, ascending, then the other way round.
        view.sort_by(SortKey::Name, true);
        assert_eq!(
            view.sort.secondary,
            Some(by(SortKey::Name, SortOrder::Ascending))
        );
        view.sort_by(SortKey::Name, true);
        assert_eq!(
            view.sort.secondary,
            Some(by(SortKey::Name, SortOrder::Descending))
        );
        // The first key's head flips the first key, with Shift too, and keeps the second.
        view.sort_by(SortKey::Size, true);
        assert_eq!(view.sort.primary, by(SortKey::Size, SortOrder::Descending));
        assert!(view.sort.secondary.is_some());
        // A plain click on another head sorts by it alone.
        view.sort_by(SortKey::Kind, false);
        assert_eq!(view.sort.primary, by(SortKey::Kind, SortOrder::Ascending));
        assert_eq!(view.sort.secondary, None);
        // The menu's second key: not the first key.
        view.set_then(Some(SortKey::Kind));
        assert_eq!(view.sort.secondary, None);
        view.set_then(Some(SortKey::Created));
        assert_eq!(
            view.sort.secondary,
            Some(by(SortKey::Created, SortOrder::Ascending))
        );
        view.set_then(None);
        assert_eq!(view.sort.secondary, None);
        assert!(view.sort.folders_first);
    }

    #[test]
    fn a_folder_takes_its_own_view_then_one_applied_from_above_then_the_default() {
        let mut store = ViewStore::new(Vec::new(), Os::Other);
        let (top, mid, low) = (folder("top"), folder("top/mid"), folder("top/mid/low"));
        assert_eq!(store.view_for(&low), FolderView::default());

        store.remember(&top, icons(ViewMode::LargeIcons));
        // Remembered for `top` alone: the folders below do not take it.
        assert_eq!(store.view_for(&mid), FolderView::default());
        assert_eq!(store.view_for(&top), icons(ViewMode::LargeIcons));

        // Applied to the subfolders: every folder below shows it, but one with a view of its own.
        store.remember(&mid, icons(ViewMode::List));
        assert!(store.apply_to_subfolders(&top, icons(ViewMode::SmallIcons)));
        assert_eq!(store.view_for(&low), icons(ViewMode::SmallIcons));
        assert_eq!(
            store.view_for(&mid),
            icons(ViewMode::SmallIcons),
            "forgot its own"
        );
        assert!(store.own(&mid).is_none());
        store.remember(&mid, icons(ViewMode::List));
        assert_eq!(store.view_for(&mid), icons(ViewMode::List));
        assert_eq!(store.view_for(&low), icons(ViewMode::SmallIcons));
        // A change of `top` keeps applying below.
        store.remember(&top, icons(ViewMode::MediumIcons));
        assert_eq!(store.view_for(&low), icons(ViewMode::MediumIcons));

        // Reset: `top` forgets its own view and its applying below; `mid` keeps its own.
        assert!(store.reset(&top));
        assert!(!store.reset(&top));
        assert_eq!(store.view_for(&top), FolderView::default());
        assert_eq!(store.view_for(&low), FolderView::default());
        assert_eq!(store.view_for(&mid), icons(ViewMode::List));
        // Reset below a folder that applies its view: it takes that one again.
        assert!(store.apply_to_subfolders(&top, icons(ViewMode::LargeIcons)));
        store.remember(&low, icons(ViewMode::List));
        assert!(store.reset(&low));
        assert_eq!(store.view_for(&low), icons(ViewMode::LargeIcons));
    }

    #[test]
    fn folders_are_known_by_the_systems_rule_of_case() {
        let (upper, lower) = (folder("Photos"), folder("photos"));
        for (os, alike) in [(Os::Windows, true), (Os::MacOs, true), (Os::Other, false)] {
            let mut store = ViewStore::new(Vec::new(), os);
            store.remember(&upper, icons(ViewMode::List));
            assert_eq!(
                store.view_for(&lower) == icons(ViewMode::List),
                alike,
                "{os:?}"
            );
            // One entry either way when the same folder is remembered under both spellings.
            store.remember(&lower, icons(ViewMode::SmallIcons));
            assert_eq!(store.len(), if alike { 1 } else { 2 }, "{os:?}");
            // The folders below, too.
            let mut store = ViewStore::new(Vec::new(), os);
            store.apply_to_subfolders(&upper, icons(ViewMode::LargeIcons));
            let below = folder("photos/2026");
            assert_eq!(
                store.view_for(&below) == icons(ViewMode::LargeIcons),
                alike,
                "{os:?}"
            );
        }
    }

    #[test]
    fn the_store_drops_the_view_used_longest_ago() {
        let mut store = ViewStore::new(Vec::new(), Os::Other);
        let numbered = |i: usize| folder(&format!("f{i}"));
        for i in 0..MAX_FOLDER_VIEWS {
            store.remember(&numbered(i), icons(ViewMode::List));
        }
        assert_eq!(store.len(), MAX_FOLDER_VIEWS);
        // Showing f0 uses it: f1 is the one used longest ago now, and goes first.
        assert_eq!(store.view_for(&numbered(0)), icons(ViewMode::List));
        store.remember(&numbered(MAX_FOLDER_VIEWS), icons(ViewMode::List));
        assert_eq!(store.len(), MAX_FOLDER_VIEWS);
        assert!(store.own(&numbered(0)).is_some());
        assert!(store.own(&numbered(1)).is_none());
        assert!(store.own(&numbered(2)).is_some());

        // A folder too long to keep is not remembered.
        let long = folder(&"x".repeat(MAX_FOLDER_URI));
        store.remember(&long, icons(ViewMode::List));
        assert!(store.own(&long).is_none());
        assert!(!store.apply_to_subfolders(&long, icons(ViewMode::List)));

        // A store read from more entries than it keeps keeps the last ones, and of two for one
        // folder the later.
        let stored = |i: usize, mode| StoredView {
            folder: numbered(i).to_uri(),
            view: icons(mode),
            subfolders: false,
            extra: toml::Table::new(),
        };
        let mut many: Vec<StoredView> = (0..=MAX_FOLDER_VIEWS)
            .map(|i| stored(i, ViewMode::List))
            .collect();
        many.push(stored(5, ViewMode::LargeIcons));
        let read = ViewStore::new(many, Os::Other);
        assert_eq!(read.len(), MAX_FOLDER_VIEWS);
        // 1001 folders, f5 the one used last: f0 goes.
        assert!(read.own(&numbered(0)).is_none());
        assert!(read.own(&numbered(1)).is_some());
        assert_eq!(
            read.own(&numbered(5)).map(|entry| entry.view.mode),
            Some(ViewMode::LargeIcons)
        );
        assert_eq!(
            read.entries().last().map(|entry| entry.folder.clone()),
            Some(numbered(5).to_uri())
        );
    }

    #[test]
    fn names_and_numbers_round_trip() {
        for mode in ViewMode::ALL {
            assert_eq!(ViewMode::from_name(mode.name()), Some(mode));
            assert_eq!(ViewMode::from_number(mode.number()), Some(mode));
        }
        for column in std::iter::once(Column::Name).chain(Column::OPTIONAL) {
            assert_eq!(Column::from_name(column.name()), Some(column));
            assert_eq!(Column::from_number(column.number()), Some(column));
            if let Some(key) = column.sort_key() {
                assert_eq!(Column::of_key(key), column);
            }
        }
        assert_eq!(ViewMode::from_name("thumbnails"), None);
        assert_eq!(Column::from_number(7), None);
        assert_eq!(Column::Attributes.sort_key(), None);
    }
}
