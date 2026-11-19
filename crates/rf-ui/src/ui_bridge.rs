//! What the Slint screens are given: the row models of a file list and of the folder trees,
//! the conversions from the panes' state, the address bar's lists, the dialogs, the
//! destination picker and the properties window to Slint structs, the payload of a drag of
//! rows, the keys the toolbar and the menus show, and [`post`], which brings a worker thread's
//! result back to the UI thread.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fmt;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use jiff::tz::TimeZone;
use rf_core::{Attributes, Entry, EntryKind, Os, VPath};
use rf_jobs::TransferMode;
use rf_platform::dnd::{DragMods, DropEffect};
use rf_platform::shell::{Owner, WindowPoint};
use rf_vfs::{SortKey, SortOrder};
use slint::language::DragAction;
use slint::winit_030::winit::dpi::PhysicalPosition;
use slint::winit_030::winit::keyboard::ModifiersState;
use slint::{
    ComponentHandle, DataTransfer, Model, ModelNotify, ModelRc, ModelTracker, PhysicalSize,
    SharedString, VecModel,
};

use crate::controller::{self, Controller};
use crate::dialogs::{
    Confirm, ConflictChoice, ConflictQuestion, Counting, ErrorChoice, ErrorQuestion, Failures,
    Modal, NameCheck, NameDialog, PatternDialog, PreflightQuestion, Recovery,
};
use crate::format::{
    attribute_letters, file_extension, file_icon, folder_label, format_size, format_time_in,
    kind_label, location_text, short_path, shown_name,
};
use crate::hashing::Algorithm;
use crate::history::{HISTORY_KEY_MENU, HISTORY_MENU, Way};
use crate::jobs::{JobRow, RecentRow, speed_points};
use crate::keys::{self, Command, PanelKey};
use crate::pane::{LoadStatus, Pane, ancestors};
use crate::pick::{POLICIES, PickPurpose, PickRequest, Picker, overwrites, validate};
use crate::places::{Place, PlaceKind, recent_slot};
use crate::preview::PreviewContent;
use crate::properties::{self, Properties, Verdict};
use crate::selection::Selection;
use crate::settings::{DragDefault, FontScale};
use crate::state::drop_rule::DropChoice;
use crate::state::{
    AppState, Column, Completion, DropSpot, HistoryView, LEFT, LinkFacts, MenuFacts, Names,
    PathForm, PreviewShown, RIGHT, RecentAt, SelectState, ViewMode,
};
use crate::theme::{self, Colors, Fonts, IconStyle, Rgba, Sizes, Tokens};
use crate::tree::{NodeId, Tree, TreeRowData};
use crate::words;
use crate::{
    ColumnView, ConfirmView, ConflictView, Crumb, DialogKind, DragPointer, DropAt, DropPlace,
    ErrorView, FactRow, FileRow, FolderMenuRow, FolderMenuView, HashRowView, HistoryRowView,
    ItemMenu, JobGraph, JobRowView, LinkEntry, MainWindow, MessageView, NameView, PaneView,
    PatternView, PickTarget, PickView, PreflightView, PreviewKind, PreviewView, PropertiesView,
    RecentRowView, RecoveryView, RowKind, SectionHead, Shortcuts, TabItem, Theme, TreeRow,
    VisitEntry,
};

/// What separates the parts of a pane's status line in `PaneView.status`:
/// the window picks as many of them as the pane's width takes ([`fit_status`]).
pub const STATUS_SEPARATOR: &str = "\n";

/// The status line of `joined` (the parts of `PaneView.status`, apart by
/// [`STATUS_SEPARATOR`]) for a pane `cells` wide ([`words::fit_status`]).
pub fn fit_status(joined: &str, cells: i32) -> String {
    let parts: Vec<&str> = joined.split(STATUS_SEPARATOR).collect();
    words::fit_status(&parts, usize::try_from(cells).unwrap_or(0))
}

/// How many characters the folder of a conflict card shows, and the folder in its sentence;
/// the middle of a longer path is left out, so that its end stays in view.
const CARD_PLACE_CHARS: usize = 36;
const SENTENCE_PLACE_CHARS: usize = 48;
/// How many characters the folder beside a folder the picker's filter found shows.
const FOUND_PLACE_CHARS: usize = 40;
/// How many of its first lines the window of failed items, and the pre-flight window, lay out
/// to fit their lists to them: enough to fill a list of 360px even when each takes one text
/// line (32px).
const MEASURED_LINES: usize = 12;
/// How many names of the items the picker shows before "외 N개".
const PICK_CHIPS: usize = 3;

/// Runs `f` with the controller on the UI thread, from any thread, and returns at once. Once
/// the window has closed nothing runs and `f` is dropped.
pub fn post(f: impl FnOnce(&Controller) + Send + 'static) {
    // An error means the event loop has ended; there is no window left to update.
    let _ = slint::invoke_from_event_loop(move || {
        controller::with(f);
    });
}

/// The models one pane's view keeps between redraws, so that the screen is told only what
/// changed.
#[derive(Default)]
pub struct PaneModels {
    rows: Rc<FileRows>,
    tabs: Rc<VecModel<TabItem>>,
    crumbs: Rc<VecModel<Crumb>>,
    completions: Rc<VecModel<FolderMenuRow>>,
    /// The columns of 자세히 after the name, and which columns show by number.
    columns: Rc<VecModel<ColumnView>>,
    shows: Rc<VecModel<bool>>,
}

impl fmt::Debug for PaneModels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaneModels")
            .field("rows", &self.rows.row_count())
            .field("tabs", &self.tabs.row_count())
            .finish_non_exhaustive()
    }
}

impl PaneModels {
    /// Shows the dates in `tz` from now on ([`FileRows::set_time_zone`]).
    pub fn set_time_zone(&self, tz: TimeZone) {
        self.rows.set_time_zone(tz);
    }

    /// Shows the extensions of the files' names or not ([`FileRows::set_show_extensions`]).
    pub fn set_show_extensions(&self, show: bool) {
        self.rows.set_show_extensions(show);
    }

    /// Shows `image` as the thumbnail of the file at `path`, asked for at `size` pixels, while
    /// the pane shows its folder ([`FileRows::set_thumb`]); `row` is where the file was.
    pub fn set_thumb(
        &self,
        row: Option<usize>,
        path: &Path,
        stamp: ThumbStamp,
        image: slint::Image,
    ) {
        self.rows.set_thumb(row, path, stamp, image);
    }

    /// Whether the file at `path` shows its thumbnail for `stamp`.
    pub fn has_thumb(&self, path: &Path, stamp: ThumbStamp) -> bool {
        self.rows.has_thumb(path, stamp)
    }

    /// The rows `first..=last` are on screen ([`FileRows::set_on_screen`]).
    pub fn set_on_screen(&self, first: usize, last: usize) {
        self.rows.set_on_screen(first, last);
    }

    /// Shows each kind's icon again in the place of every thumbnail.
    pub fn clear_thumbs(&self) {
        self.rows.clear_thumbs();
    }

    /// Shows the rows named `names` dimmed from the next [`PaneModels::view`] on: the items cut
    /// to the clipboard ([`FileRows::set_cut`]).
    pub fn set_cut(&self, names: HashSet<OsString>) {
        self.rows.set_cut(names);
    }

    /// Gives the rows of 내 PC the space of the drives among `places`, the sidebar's, while the
    /// pane shows `folder` (none elsewhere): [`FileRows::set_drives`].
    pub fn set_drives(&self, folder: &VPath, places: &[Place]) {
        let drives = if crate::format::shell_place_at(folder) == Some(rf_vfs::ShellPlace::Computer)
        {
            places
                .iter()
                .filter(|place| place.kind == PlaceKind::Drive)
                .map(|place| {
                    let (info, fill) = place_space(place);
                    (
                        OsString::from(rf_vfs::drive_name(&place.path)),
                        (SharedString::from(info), fill),
                    )
                })
                .collect()
        } else {
            HashMap::new()
        };
        self.rows.set_drives(drives);
    }

    /// The texts of `column` in `count` rows of the list from row `first` on, one a line, as
    /// the list shows them: what a double click on the column's border fits it to.
    pub fn column_texts(&self, column: Column, first: usize, count: usize) -> String {
        self.rows.texts(column, first, count)
    }

    /// What `pane` shows; `active` when it has the keyboard. The models are brought up to
    /// date and handed over as they are.
    pub fn view(&self, pane: &Pane, active: bool) -> PaneView {
        self.view_with(pane, active, None)
    }

    /// [`PaneModels::view`], the status line's room taken from `drive_free`, the sidebar's
    /// knowledge of the drive, while the pane has not read its own.
    pub fn view_with(&self, pane: &Pane, active: bool, drive_free: Option<u64>) -> PaneView {
        let tab = pane.active_tab();
        self.rows.set_folder(tab.location().as_local());
        self.rows.set_parent(&tab.location().to_string());
        self.rows.sync(tab.visible(), tab.selection());
        // The last tab that is not pinned: the menu of a tab before it can close tabs to its
        // right, and the menu of any other tab, when there is one, can close others.
        let unpinned: Vec<usize> = (0..pane.tabs().len())
            .filter(|&i| !pane.tabs()[i].pinned())
            .collect();
        set_rows(
            &self.tabs,
            pane.tabs()
                .iter()
                .enumerate()
                .map(|(i, each)| TabItem {
                    title: folder_label(each.location()).into(),
                    active: i == pane.active(),
                    pinned: each.pinned(),
                    can_close_others: unpinned.iter().any(|&other| other != i),
                    can_close_right: unpinned.last().is_some_and(|&last| last > i),
                })
                .collect(),
        );
        set_rows(&self.crumbs, crumbs(&ancestors(tab.location())));
        let view = tab.view();
        set_rows(
            &self.columns,
            view.columns
                .iter()
                .map(|shown| ColumnView {
                    id: shown.column.number(),
                    title: shown.column.title().into(),
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "a column is at most a few hundred pixels wide"
                    )]
                    width: shown.width as f32,
                    align_end: shown.column.align_end(),
                    sortable: shown.column.sort_key().is_some(),
                })
                .collect(),
        );
        set_rows(
            &self.shows,
            std::iter::once(Column::Name)
                .chain(Column::offered(tab.location()).iter().copied())
                .map(|column| (column.number(), view.shows(column)))
                .fold(vec![false; Column::COUNT], |mut shows, (number, shown)| {
                    if let Some(slot) = usize::try_from(number).ok().and_then(|n| shows.get_mut(n))
                    {
                        *slot = shown;
                    }
                    shows
                }),
        );
        let sort = view.sort;
        PaneView {
            tabs: ModelRc::from(Rc::clone(&self.tabs)),
            active_tab: i32::try_from(pane.active()).unwrap_or(0),
            crumbs: ModelRc::from(Rc::clone(&self.crumbs)),
            path_text: location_text(tab.location()).into(),
            rows: ModelRc::from(Rc::clone(&self.rows)),
            cursor: tab
                .selection()
                .cursor()
                .and_then(|row| i32::try_from(row).ok())
                .unwrap_or(-1),
            mode: view.mode.number(),
            thumb_size: thumb_size_number(view.mode),
            columns: ModelRc::from(Rc::clone(&self.columns)),
            shows: ModelRc::from(Rc::clone(&self.shows)),
            sort_key: sort_column(sort.primary.key),
            sort_ascending: sort.primary.order == SortOrder::Ascending,
            then_key: sort.secondary.map_or(-1, |by| sort_column(by.key)),
            then_ascending: sort
                .secondary
                .is_none_or(|by| by.order == SortOrder::Ascending),
            folders_first: sort.folders_first,
            own_view: false,
            // A folder that could not be listed has no items to count.
            status: match tab.status() {
                LoadStatus::Failed(_) => SharedString::new(),
                LoadStatus::Loading | LoadStatus::Ready => {
                    tab.status_parts(drive_free).join(STATUS_SEPARATOR).into()
                }
            },
            loading: tab.status() == LoadStatus::Loading,
            error: match tab.status() {
                LoadStatus::Failed(kind) => words::error_kind(kind).into(),
                LoadStatus::Loading | LoadStatus::Ready => SharedString::new(),
            },
            active,
            select_state: match SelectState::of(tab.selection().len(), tab.visible().len()) {
                SelectState::None => 0,
                SelectState::All => 1,
                SelectState::Some => 2,
            },
            can_reopen: pane.can_reopen(),
            completion: FolderMenuView::default(),
            chevron: -1,
            place: place_number(tab.location()),
        }
    }

    /// Gives `view` the address bar's part of the pane: the completion list of its path field
    /// (`completion`), and the piece of its path row whose `›` has its list of folders open
    /// (`chevron`).
    pub fn show_address(
        &self,
        view: &mut PaneView,
        completion: Option<&Completion>,
        chevron: Option<usize>,
    ) {
        let list = completion.map(|completion| (completion.names(), completion.cursor()));
        view.completion = folder_menu(&self.completions, list, false);
        // A Tab waits whenever a request for the text is out, rows or not.
        view.completion.pending = completion.is_some_and(Completion::waiting);
        view.chevron = chevron
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1);
    }
}

/// What a list of the address bar shows of `list` (its names and the keyboard's row; `None`
/// when it is closed), with `model` as its rows. The list of a piece's folders (`notes`) shows
/// from the click on, and says while the folder is listed, when it could not be, when it has
/// no folder, and when links it did not follow may lead to more; the completion list shows only
/// while it has names. Both say how many folders more
/// there are than their rows, and mark the links among them, and whether their first names are
/// still on their way (`pending`).
pub fn folder_menu(
    model: &Rc<VecModel<FolderMenuRow>>,
    list: Option<(&Names, Option<usize>)>,
    notes: bool,
) -> FolderMenuView {
    let Some((names, cursor)) = list else {
        set_rows(model, Vec::new());
        return FolderMenuView {
            rows: ModelRc::from(Rc::clone(model)),
            cursor: -1,
            ..FolderMenuView::default()
        };
    };
    let rows: Vec<FolderMenuRow> = names
        .rows()
        .iter()
        .map(|each| FolderMenuRow {
            name: each.name.to_string_lossy().as_ref().into(),
            link: each.link,
        })
        .collect();
    let note = match names {
        Names::Listed { more, .. } if *more > 0 => words::more_folders(*more),
        // Only links the listing did not follow cut the list short: it may have more folders.
        Names::Listed {
            links_left: true, ..
        } if notes => words::links_unexamined().to_owned(),
        Names::Listed { names, .. } if names.is_empty() && notes => {
            words::no_subfolders().to_owned()
        }
        Names::Loading if notes => words::listing_folders().to_owned(),
        Names::Failed(kind) if notes => words::cannot_open(*kind),
        Names::Listed { .. } | Names::Loading | Names::Failed(_) => String::new(),
    };
    let open = notes || !rows.is_empty();
    set_rows(model, rows);
    FolderMenuView {
        open,
        rows: ModelRc::from(Rc::clone(model)),
        cursor: cursor.and_then(|row| i32::try_from(row).ok()).unwrap_or(-1),
        note: note.into(),
        pending: *names == Names::Loading,
    }
}

/// The rows of one pane's file list as a Slint model.
///
/// It keeps the entries it shows and formats a row only when the list asks for it, which it
/// does only for the rows on screen: a folder of 100,000 items costs no more to show than one
/// of a hundred. [`FileRows::sync`] tells the list which rows came, went or changed, so the
/// rows on screen, the scroll position and the rows' hover state stay where they were.
pub struct FileRows {
    rows: RefCell<Vec<ShownRow>>,
    notify: ModelNotify,
    /// The time zone the dates show in: UTC until [`FileRows::set_time_zone`].
    tz: RefCell<TimeZone>,
    /// The names show their extensions (NAV-11): on until [`FileRows::set_show_extensions`].
    extensions: Cell<bool>,
    /// The names of the rows that show dimmed, cut to the clipboard ([`FileRows::set_cut`]).
    cut: RefCell<HashSet<OsString>>,
    /// The local folder of the rows, whose files' thumbnails show ([`FileRows::set_folder`]).
    folder: RefCell<Option<PathBuf>>,
    /// The folder the rows are in, as the 경로 column shows it ([`FileRows::set_parent`]).
    parent: RefCell<SharedString>,
    /// The first and the last row on screen ([`FileRows::set_on_screen`]).
    on_screen: Cell<(usize, usize)>,
    /// The thumbnails that show, by the file's path: what they were made for, the picture and
    /// when it came (a count), at most [`SHOWN_THUMBS`].
    thumbs: RefCell<ShownThumbs>,
    thumbs_came: Cell<u64>,
    /// The drives of 내 PC by the names of their rows: the space line and the used share each
    /// row's bar shows (as the sidebar's, [`place_space`]); empty elsewhere
    /// ([`FileRows::set_drives`]).
    drives: RefCell<HashMap<OsString, (SharedString, f32)>>,
}

/// What a shown thumbnail was made for: the size asked, and the length and modification time the
/// file had. A file that changed (the same name, another length or time) shows its kind's icon
/// until its new thumbnail comes, and is asked for again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThumbStamp {
    pub size: u32,
    pub len: u64,
    pub mtime: Option<std::time::SystemTime>,
}

impl ThumbStamp {
    /// Whether `entry` is the file as the thumbnail saw it.
    fn is_of(&self, entry: &Entry) -> bool {
        self.len == entry.size && self.mtime == entry.modified
    }
}

/// The most thumbnails a list (each pane's) keeps, and the most bytes of pixels: the pane's pool
/// keeps as many ([`crate::thumbs::MEMORY_THUMBS`], [`crate::thumbs::MEMORY_BYTES`]), so one
/// forgotten here that comes on screen again comes back from there. The list's pictures share
/// their pixels with the pool's, so a pane holds about that much, twice at most.
const SHOWN_THUMBS: usize = crate::thumbs::MEMORY_THUMBS;
const SHOWN_BYTES: usize = crate::thumbs::MEMORY_BYTES;

/// The thumbnails a list shows, by the file's path: what each was made for, the picture, and
/// when it came (a count).
type ShownThumbs = HashMap<PathBuf, (ThumbStamp, slint::Image, u64)>;

/// Forgets the thumbnails that came first while there are more than `most` of them or they hold
/// more than `budget` bytes of pixels, but those of the files on screen (`on_screen`), so that a
/// screenful larger than the budget is not forgotten and asked for again at every redraw; the
/// one that came last stays, however large.
fn trim_shown(thumbs: &mut ShownThumbs, most: usize, budget: usize, on_screen: &HashSet<PathBuf>) {
    let mut held: usize = thumbs
        .values()
        .map(|(_, image, _)| image_bytes(image))
        .sum();
    let last = thumbs.values().map(|(_, _, came)| *came).max();
    while (thumbs.len() > most || held > budget)
        && let Some(first) = thumbs
            .iter()
            .filter(|(path, (_, _, came))| Some(*came) != last && !on_screen.contains(*path))
            .min_by_key(|(_, (_, _, came))| *came)
            .map(|(path, _)| path.clone())
    {
        if let Some((_, gone, _)) = thumbs.remove(&first) {
            held = held.saturating_sub(image_bytes(&gone));
        }
    }
}

/// How many bytes the pixels of `image` take (4 a pixel).
fn image_bytes(image: &slint::Image) -> usize {
    let size = image.size();
    usize::try_from(u64::from(size.width) * u64::from(size.height) * 4).unwrap_or(usize::MAX)
}

/// One row as [`FileRows`] keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ShownRow {
    entry: Entry,
    selected: bool,
    /// Cut to the clipboard: it shows dimmed.
    cut: bool,
}

impl fmt::Debug for FileRows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileRows")
            .field("rows", &self.rows.borrow().len())
            .finish_non_exhaustive()
    }
}

/// Past this many changes, [`FileRows::sync`] tells the list to start over instead.
const MAX_ROW_CHANGES: usize = 512;

impl Default for FileRows {
    fn default() -> FileRows {
        FileRows {
            rows: RefCell::default(),
            notify: ModelNotify::default(),
            tz: RefCell::new(TimeZone::UTC),
            extensions: Cell::new(true),
            cut: RefCell::default(),
            folder: RefCell::default(),
            parent: RefCell::default(),
            on_screen: Cell::new((1, 0)),
            thumbs: RefCell::default(),
            thumbs_came: Cell::new(0),
            drives: RefCell::new(HashMap::new()),
        }
    }
}

impl FileRows {
    /// Shows the dates in `tz` from now on. Looking up the system's time zone reads the
    /// system's settings, so that is done on a worker thread and handed in here.
    pub fn set_time_zone(&self, tz: TimeZone) {
        *self.tz.borrow_mut() = tz;
        self.notify.reset();
    }

    /// Shows the extensions of the files' names (`show`) or not, in every view, from now on;
    /// the rows are drawn again when that changes.
    pub fn set_show_extensions(&self, show: bool) {
        if self.extensions.replace(show) != show {
            self.notify.reset();
        }
    }

    /// Shows the rows named `names` dimmed, and the others not, from the next
    /// [`FileRows::sync`] on, which tells the list of the rows that changed.
    pub fn set_cut(&self, names: HashSet<OsString>) {
        *self.cut.borrow_mut() = names;
    }

    /// Shows each row of 내 PC named in `drives` with its space line and the bar of how full the
    /// drive is (its tile's second and third lines), and no row else; the rows are drawn again
    /// when that changes (a job read the space again).
    pub fn set_drives(&self, drives: HashMap<OsString, (SharedString, f32)>) {
        if *self.drives.borrow() != drives {
            *self.drives.borrow_mut() = drives;
            self.notify.reset();
        }
    }

    /// Makes the rows `entries`, in that order, selected as `selection` says and dimmed as
    /// [`FileRows::set_cut`] said, and tells the list what changed: only the selection or the
    /// dimming (a click or a key, a cut), a few rows (a change another app made), or everything
    /// (another folder), in the cheapest way that keeps it right.
    pub fn sync(&self, entries: &[Entry], selection: &Selection) {
        let cut = self.cut.borrow();
        let same_entries = {
            let rows = self.rows.borrow();
            rows.len() == entries.len() && rows.iter().zip(entries).all(|(row, e)| row.entry == *e)
        };
        if same_entries {
            let flipped: Vec<usize> = {
                let mut rows = self.rows.borrow_mut();
                rows.iter_mut()
                    .enumerate()
                    .filter_map(|(i, row)| {
                        let selected = selection.is_selected(&row.entry.name);
                        let dimmed = cut.contains(&row.entry.name);
                        (row.selected != selected || row.cut != dimmed).then(|| {
                            row.selected = selected;
                            row.cut = dimmed;
                            i
                        })
                    })
                    .collect()
            };
            // The list reads the row back while it is told, so the borrow has ended.
            for i in flipped {
                self.notify.row_changed(i);
            }
            return;
        }

        let new: Vec<ShownRow> = entries
            .iter()
            .map(|entry| ShownRow {
                selected: selection.is_selected(&entry.name),
                cut: cut.contains(&entry.name),
                entry: entry.clone(),
            })
            .collect();
        let changes = {
            let old = self.rows.borrow();
            diff_rows(&old, &new, |row| &row.entry.name)
        };
        *self.rows.borrow_mut() = new;
        tell(&self.notify, changes);
    }
}

impl FileRows {
    /// The rows are of the local folder `folder` (`None`: not a local one) from now on: in
    /// another folder than before, no thumbnail shows until it comes for that folder.
    pub fn set_folder(&self, folder: Option<&Path>) {
        if self.folder.borrow().as_deref() == folder {
            return;
        }
        *self.folder.borrow_mut() = folder.map(Path::to_path_buf);
        self.clear_thumbs();
    }

    /// The rows are in the folder `parent` (as the path field shows it) from now on: what the
    /// 경로 column shows on each row. The rows are drawn again when it changes.
    pub fn set_parent(&self, parent: &str) {
        if self.parent.borrow().as_str() == parent {
            return;
        }
        *self.parent.borrow_mut() = parent.into();
        self.notify.reset();
    }

    /// Shows `image` (asked for at `size` pixels) as the thumbnail of the file at `path`, when
    /// it is in the rows' folder, and tells the list of its row (at `row`, or found by its
    /// name). Past [`crate::thumbs::MEMORY_THUMBS`] the one that came first is forgotten.
    pub fn set_thumb(
        &self,
        row: Option<usize>,
        path: &Path,
        stamp: ThumbStamp,
        image: slint::Image,
    ) {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return;
        };
        if self.folder.borrow().as_deref() != Some(parent) {
            return;
        }
        let (at, stale) = {
            let rows = self.rows.borrow();
            let at = row
                .filter(|&at| rows.get(at).is_some_and(|shown| shown.entry.name == name))
                .or_else(|| rows.iter().position(|shown| shown.entry.name == name));
            let stale = at
                .and_then(|at| rows.get(at))
                .is_some_and(|shown| !stamp.is_of(&shown.entry));
            (at, stale)
        };
        // A picture of the file as it was, before it changed, is not shown.
        if stale {
            return;
        }
        let came = self.thumbs_came.get() + 1;
        self.thumbs_came.set(came);
        {
            let mut thumbs = self.thumbs.borrow_mut();
            thumbs.insert(path.to_path_buf(), (stamp, image, came));
            trim_shown(
                &mut thumbs,
                SHOWN_THUMBS,
                SHOWN_BYTES,
                &self.on_screen_paths(),
            );
        }
        // The list reads the row back while it is told, so the borrow has ended.
        if let Some(at) = at {
            self.notify.row_changed(at);
        }
    }

    /// The rows `first..=last` are on screen from now on (the icon views tell it): their
    /// thumbnails are the last a trim forgets ([`trim_shown`]). An empty range when `last` is
    /// before `first`.
    pub fn set_on_screen(&self, first: usize, last: usize) {
        self.on_screen.set((first, last));
    }

    /// The paths of the files on the rows on screen ([`FileRows::set_on_screen`]).
    fn on_screen_paths(&self) -> HashSet<PathBuf> {
        let (first, last) = self.on_screen.get();
        let Some(folder) = self.folder.borrow().clone() else {
            return HashSet::new();
        };
        if last < first {
            return HashSet::new();
        }
        self.rows
            .borrow()
            .iter()
            .skip(first)
            .take(last - first + 1)
            .map(|shown| folder.join(&shown.entry.name))
            .collect()
    }

    /// Whether the file at `path` shows the thumbnail for `stamp`: asked for at that size, of
    /// the file as it has that length and modification time.
    pub fn has_thumb(&self, path: &Path, stamp: ThumbStamp) -> bool {
        self.thumbs
            .borrow()
            .get(path)
            .is_some_and(|(shown, _, _)| *shown == stamp)
    }

    /// Forgets every thumbnail: each row shows its kind's icon.
    pub fn clear_thumbs(&self) {
        let had = !self.thumbs.borrow().is_empty();
        self.thumbs.borrow_mut().clear();
        if had {
            self.notify.reset();
        }
    }

    /// The texts of `column` in `count` rows from row `first` on, one a line
    /// ([`PaneModels::column_texts`]).
    fn texts(&self, column: Column, first: usize, count: usize) -> String {
        let rows = self.rows.borrow();
        let tz = self.tz.borrow();
        let mut texts: Vec<String> = rows
            .iter()
            .skip(first)
            .take(count)
            .map(|shown| {
                let mut row = file_row(&shown.entry, shown.selected, &tz, self.extensions.get());
                row.path = self.parent.borrow().clone();
                cell_text(&row, column)
            })
            .collect();
        texts.sort_unstable();
        texts.dedup();
        texts.join("\n")
    }
}

/// The text of `column` in `row`.
fn cell_text(row: &FileRow, column: Column) -> String {
    match column {
        Column::Name => &row.name,
        Column::Size => &row.size,
        Column::Modified => &row.modified,
        Column::Created => &row.created,
        Column::Kind => &row.kind,
        Column::Extension => &row.extension,
        Column::Attributes => &row.attributes,
        Column::Accessed => &row.accessed,
        Column::Path => &row.path,
        Column::Original => &row.original,
        Column::Deleted => &row.deleted,
    }
    .to_string()
}

impl Model for FileRows {
    type Data = FileRow;

    fn row_count(&self) -> usize {
        self.rows.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<FileRow> {
        let rows = self.rows.borrow();
        let shown = rows.get(row)?;
        let mut file = file_row(
            &shown.entry,
            shown.selected,
            &self.tz.borrow(),
            self.extensions.get(),
        );
        file.cut = shown.cut;
        file.path = self.parent.borrow().clone();
        if let Some((info, fill)) = self.drives.borrow().get(&shown.entry.name) {
            file.kind = SharedString::new();
            file.size = info.clone();
            file.fill = *fill;
        }
        if let Some(folder) = self.folder.borrow().as_ref()
            && let Some((stamp, image, _)) =
                self.thumbs.borrow().get(&folder.join(&shown.entry.name))
            && stamp.is_of(&shown.entry)
        {
            file.thumb = image.clone();
        }
        Some(file)
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// Tells a list what [`diff_rows`] found: each step, or to start over when it found too many.
/// The rows have been replaced already, since the list reads them back while it is told.
fn tell(notify: &ModelNotify, changes: Option<Vec<RowChange>>) {
    match changes {
        Some(changes) => {
            for change in changes {
                match change {
                    RowChange::Removed { at, count } => notify.row_removed(at, count),
                    RowChange::Added { at, count } => notify.row_added(at, count),
                    RowChange::Changed { at } => notify.row_changed(at),
                }
            }
        }
        None => notify.reset(),
    }
}

/// The rows of a folder tree (the sidebar's, the picker's) as a Slint model.
///
/// [`TreeRows::sync`] makes them the tree's rows and tells the list which rows came, went or
/// changed (a node opened or closed, the selection moved), so the list keeps its scroll
/// position and draws only the rows on screen, however many folders are open.
/// [`TreeRows::show`] shows rows made elsewhere: the picker's, marked for its items, or what
/// its filter found, flat.
///
/// The sidebar's rows hold the group of recent folders ([`TreeRows::set_recent`]) too, between
/// the favorites and the drives: a head and the folders, which are no rows of the tree. The tree
/// keeps its own numbers for its rows (`TreeRow.row`), and the window's list counts the group's
/// rows among them: [`TreeRows::display_of`] and [`TreeRows::slot_at`] go from one to the other.
#[derive(Default)]
pub struct TreeRows {
    rows: RefCell<Vec<(NodeId, TreeRow)>>,
    notify: ModelNotify,
    /// The [`Tree::revision`] of the rows shown.
    revision: Cell<Option<u64>>,
    /// The group of recent folders the rows hold.
    recent: RefCell<RecentBlock>,
    /// Where the group starts among the rows (the row of the tree it lies before), and how many
    /// rows it has (none while it is empty).
    block: Cell<(usize, usize)>,
}

/// The group of recent folders as the sidebar shows it ([`TreeRows::set_recent`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentBlock {
    /// Only its head shows.
    pub folded: bool,
    /// The folders, the latest visited first.
    pub folders: Vec<RecentFolder>,
    /// The folders of the favorites a worker found gone: their rows dim.
    pub missing_favorites: HashSet<VPath>,
}

/// One folder of the group of recent folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentFolder {
    /// The folder's name.
    pub label: String,
    /// The name of the folder it is in: it tells folders of one name apart.
    pub place: String,
    /// A worker found it gone: its row dims.
    pub missing: bool,
    /// It is the folder of the active pane.
    pub selected: bool,
}

/// What a row of the sidebar's list is: a row of the tree, or a part of the group of recent
/// folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowSlot {
    /// Row `n` of the tree.
    Tree(usize),
    /// The head of the group of recent folders.
    RecentHead,
    /// Recent folder `n`.
    Recent(usize),
}

/// The key a model of rows gives the head of the group of recent folders: no node of a tree has
/// it, and a folder of the group has the keys below it.
const RECENT_KEY: NodeId = NodeId::MAX;

impl fmt::Debug for TreeRows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TreeRows")
            .field("rows", &self.rows.borrow().len())
            .field("revision", &self.revision.get())
            .finish_non_exhaustive()
    }
}

impl TreeRows {
    /// Makes the rows those of `tree`, whose roots are `places` in order, and tells the list
    /// what changed. Nothing is done while the tree's revision stays the same: the places
    /// change only with a new tree ([`Tree::set_roots`]).
    pub fn sync(&self, tree: &Tree, places: &[Place]) {
        let revision = Some(tree.revision());
        if self.revision.replace(revision) == revision {
            return;
        }
        let data = tree.rows();
        let mut rows = tree_rows(&data, places);
        {
            let recent = self.recent.borrow();
            for (shown, row) in rows.iter_mut().zip(&data) {
                let gone = row
                    .root
                    .and_then(|root| places.get(root))
                    .is_some_and(|place| {
                        place.kind.is_favorite() && recent.missing_favorites.contains(&place.path)
                    });
                shown.1.dimmed = gone;
            }
        }
        self.hold_recent(&mut rows, recent_slot(&data, places));
        self.replace(rows);
    }

    /// Has the next [`TreeRows::sync`] draw its tree again, whatever its revision: the places
    /// changed without a new tree (a drive's free space).
    pub fn invalidate(&self) {
        self.revision.set(None);
    }

    /// Makes `block` the group of recent folders the rows hold, from the next
    /// [`TreeRows::sync`] on.
    pub fn set_recent(&self, block: RecentBlock) {
        if *self.recent.borrow() != block {
            *self.recent.borrow_mut() = block;
            self.revision.set(None);
        }
    }

    /// Makes the rows `rows`, whatever a tree's revision says (the picker's tree, marked for
    /// its items, or what its filter found), and tells the list what changed.
    pub fn show(&self, rows: Vec<(NodeId, TreeRow)>) {
        // The next `sync` draws its tree again, whatever its revision.
        self.revision.set(None);
        self.block.set((rows.len(), 0));
        self.replace(rows);
    }

    /// Puts the group of recent folders into `rows` before the tree's row `slot`.
    fn hold_recent(&self, rows: &mut Vec<(NodeId, TreeRow)>, slot: usize) {
        let recent = self.recent.borrow();
        if recent.folders.is_empty() {
            self.block.set((slot, 0));
            return;
        }
        let head = TreeRow {
            label: words::recent_folders().into(),
            kind: RowKind::RecentHead,
            row: -1,
            recent: -1,
            expandable: true,
            expanded: !recent.folded,
            ..TreeRow::default()
        };
        let shown = if recent.folded {
            0
        } else {
            recent.folders.len()
        };
        let folders = recent
            .folders
            .iter()
            .take(shown)
            .enumerate()
            .map(|(i, folder)| {
                let row = TreeRow {
                    label: folder.label.as_str().into(),
                    // One level in, under the head.
                    depth: 1,
                    info: folder.place.as_str().into(),
                    kind: RowKind::Recent,
                    row: -1,
                    recent: i32::try_from(i).unwrap_or(i32::MAX),
                    selected: folder.selected,
                    dimmed: folder.missing,
                    ..TreeRow::default()
                };
                (RECENT_KEY - 1 - i, row)
            });
        let block: Vec<(NodeId, TreeRow)> =
            std::iter::once((RECENT_KEY, head)).chain(folders).collect();
        self.block.set((slot, block.len()));
        rows.splice(slot..slot, block);
    }

    /// The place in the list of row `row` of the tree.
    pub fn display_of(&self, row: usize) -> usize {
        let (slot, len) = self.block.get();
        if row < slot { row } else { row + len }
    }

    /// The place in the list of the part `at` of the group of recent folders.
    pub fn recent_display(&self, at: RecentAt) -> usize {
        let (slot, _) = self.block.get();
        match at {
            RecentAt::Head => slot,
            RecentAt::Item(i) => slot + 1 + i,
        }
    }

    /// What row `display` of the list is.
    pub fn slot_at(&self, display: usize) -> RowSlot {
        let (slot, len) = self.block.get();
        if display < slot {
            RowSlot::Tree(display)
        } else if display == slot && len > 0 {
            RowSlot::RecentHead
        } else if display < slot + len {
            RowSlot::Recent(display - slot - 1)
        } else {
            RowSlot::Tree(display - len)
        }
    }

    fn replace(&self, new: Vec<(NodeId, TreeRow)>) {
        let changes = {
            let old = self.rows.borrow();
            diff_rows(&old, &new, |(id, _)| *id)
        };
        *self.rows.borrow_mut() = new;
        tell(&self.notify, changes);
    }
}

impl Model for TreeRows {
    type Data = TreeRow;

    fn row_count(&self) -> usize {
        self.rows.borrow().len()
    }

    fn row_data(&self, row: usize) -> Option<TreeRow> {
        self.rows.borrow().get(row).map(|(_, data)| data.clone())
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

/// The section of the sidebar a place of `kind` is in: "즐겨찾기" for the favorites, "이 PC" for
/// 내 PC and the drives; 휴지통 after them, under no head of its own.
fn section_of(kind: PlaceKind) -> SectionHead {
    match kind {
        PlaceKind::Favorite(_) | PlaceKind::Kept => SectionHead::Favorites,
        PlaceKind::Drive | PlaceKind::Shell(rf_vfs::ShellPlace::Computer) => SectionHead::Drives,
        PlaceKind::Shell(rf_vfs::ShellPlace::Trash) => SectionHead::None,
    }
}

/// The icon of the row of a place of `kind` (`Icon.name`): a drive's, 내 PC's screen, 휴지통's
/// bin; a folder's for the others.
fn place_icon(kind: PlaceKind) -> &'static str {
    match kind {
        PlaceKind::Drive => "drive",
        PlaceKind::Shell(rf_vfs::ShellPlace::Computer) => "computer",
        PlaceKind::Shell(rf_vfs::ShellPlace::Trash) => "delete",
        PlaceKind::Favorite(_) | PlaceKind::Kept => "folder",
    }
}

/// The sidebar rows of the tree rows `rows`, whose roots are `places` in order, each with its
/// node. A place shows whether it is a drive, and a drive how full it is; the first favorite
/// carries the head "즐겨찾기" and the first place after them of 내 PC and the drives "이 PC"
/// ([`section_of`]); 휴지통 comes last, under no head. A row shows a chevron when it has
/// folders below it, or may have.
pub fn tree_rows(rows: &[TreeRowData], places: &[Place]) -> Vec<(NodeId, TreeRow)> {
    // The section of the last place.
    let mut last_section = None;
    // The last favorite's place in the list of favorites, to say which one is the last.
    let favorites = places
        .iter()
        .filter(|place| place.kind.is_favorite())
        .count();
    rows.iter()
        .enumerate()
        .map(|(number, row)| {
            let place = row.root.and_then(|root| places.get(root));
            let is_drive = place.is_some_and(|place| place.kind == PlaceKind::Drive);
            let section = place.map(|place| section_of(place.kind));
            let head = match section {
                Some(section) if last_section != Some(section) => section,
                _ => SectionHead::None,
            };
            if section.is_some() {
                last_section = section;
            }
            // A favorite's place in the list: the places before it that are favorites.
            let favorite = row
                .root
                .zip(place)
                .filter(|(_, place)| place.kind.is_favorite())
                .map(|(root, _)| {
                    places[..root]
                        .iter()
                        .filter(|place| place.kind.is_favorite())
                        .count()
                });
            let (info, fill) = place.map_or_else(|| (String::new(), 0.0), place_space);
            let expandable = row.has_children_unknown || row.has_children;
            let shown = TreeRow {
                label: row.label.as_str().into(),
                depth: i32::try_from(row.depth).unwrap_or(i32::MAX),
                head,
                is_place: place.is_some(),
                is_drive,
                icon: place
                    .map_or("folder", |place| place_icon(place.kind))
                    .into(),
                info: info.into(),
                fill,
                expandable,
                expanded: expandable && row.expanded,
                selected: row.selected,
                dimmed: false,
                hidden: row.hidden,
                kind: RowKind::Folder,
                row: i32::try_from(number).unwrap_or(i32::MAX),
                is_favorite: favorite.is_some(),
                favorite: favorite
                    .map_or(-1, |favorite| i32::try_from(favorite).unwrap_or(i32::MAX)),
                favorite_last: favorite.is_some_and(|favorite| favorite + 1 == favorites),
                can_up: favorite.is_some_and(|favorite| favorite > 0),
                can_down: favorite.is_some_and(|favorite| favorite + 1 < favorites),
                recent: -1,
            };
            (row.id, shown)
        })
        .collect()
}

/// The rows of the folders `found` in `tree`, flat: each with the folder it is in beside it,
/// and marked when it is the folder the tree follows.
pub fn found_rows(tree: &Tree, found: &[NodeId]) -> Vec<(NodeId, TreeRow)> {
    found
        .iter()
        .enumerate()
        .filter_map(|(number, &id)| {
            let node = tree.node(id)?;
            let info = node
                .path
                .parent()
                .map(|folder| short_path(&folder.to_string(), FOUND_PLACE_CHARS))
                .unwrap_or_default();
            let row = TreeRow {
                label: node.label.as_str().into(),
                depth: 0,
                head: SectionHead::None,
                is_place: false,
                is_drive: false,
                icon: "folder".into(),
                info: info.into(),
                fill: 0.0,
                expandable: false,
                expanded: false,
                selected: tree.target() == Some(&node.path),
                dimmed: false,
                hidden: node.attributes.contains(Attributes::HIDDEN),
                kind: RowKind::Folder,
                row: i32::try_from(number).unwrap_or(i32::MAX),
                is_favorite: false,
                favorite: -1,
                favorite_last: false,
                can_up: false,
                can_down: false,
                recent: -1,
            };
            Some((id, row))
        })
        .collect()
}

/// One step that turns a list into another, as Slint's model notifications take them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowChange {
    /// `count` rows go from position `at`.
    Removed { at: usize, count: usize },
    /// `count` rows come at position `at`.
    Added { at: usize, count: usize },
    /// The row at position `at` of the new list shows something else under the same name.
    Changed { at: usize },
}

/// The steps that turn `old` into `new`, where a row is known by its `key` (unique in a
/// list: a file's name, a tree node): first the removals and additions, each at its position
/// in the list as the steps before it have left it, then the changes, at positions in `new`.
/// A row that moved is removed and added. `None` when there are more than 512 steps
/// (`MAX_ROW_CHANGES`), which a fresh start shows faster.
pub fn diff_rows<'a, T: PartialEq, K: Eq + Hash>(
    old: &'a [T],
    new: &'a [T],
    key: impl Fn(&'a T) -> K,
) -> Option<Vec<RowChange>> {
    // The rows that are the same at both ends need no step, and no lookup.
    let head = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let tail = old[head..]
        .iter()
        .rev()
        .zip(new[head..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (old, new) = (&old[head..old.len() - tail], &new[head..new.len() - tail]);

    let position: HashMap<K, usize> = new
        .iter()
        .enumerate()
        .map(|(j, row)| (key(row), j))
        .collect();
    let mut steps: Vec<RowChange> = Vec::new();
    let mut changed = Vec::new();
    // `i` walks `old`, `j` walks `new`, and `at` is where the list being changed is: every
    // row before it is already as in `new`.
    let (mut i, mut j, mut at) = (0, 0, head);
    while i < old.len() || j < new.len() {
        if steps.len() + changed.len() > MAX_ROW_CHANGES {
            return None;
        }
        if let Some(row) = old.get(i) {
            // Gone, or met earlier in `new` (it moved up and was added there).
            let stays = position.get(&key(row)).is_some_and(|&p| p >= j);
            if !stays {
                match steps.last_mut() {
                    Some(RowChange::Removed { at: last, count }) if *last == at => *count += 1,
                    _ => steps.push(RowChange::Removed { at, count: 1 }),
                }
                i += 1;
                continue;
            }
        }
        let same_name = old.get(i).is_some_and(|row| key(row) == key(&new[j]));
        if !same_name {
            match steps.last_mut() {
                Some(RowChange::Added { at: first, count }) if *first + *count == at => {
                    *count += 1;
                }
                _ => steps.push(RowChange::Added { at, count: 1 }),
            }
            at += 1;
            j += 1;
            continue;
        }
        if old[i] != new[j] {
            changed.push(RowChange::Changed { at });
        }
        i += 1;
        j += 1;
        at += 1;
    }
    steps.extend(changed);
    (steps.len() <= MAX_ROW_CHANGES).then_some(steps)
}

/// The row the list shows for `entry`, not dimmed. Only a file has a size; a time that is not
/// known (the creation time on a file system that keeps none) or beyond what can be shown shows
/// nothing.
pub fn file_row(entry: &Entry, selected: bool, tz: &TimeZone, show_extensions: bool) -> FileRow {
    let time = |time: Option<std::time::SystemTime>| -> SharedString {
        time.map(|time| format_time_in(time, tz))
            .unwrap_or_default()
            .into()
    };
    FileRow {
        name: shown_name(entry, show_extensions).into(),
        size: if entry.kind == EntryKind::File {
            format_size(entry.size).into()
        } else {
            SharedString::new()
        },
        modified: time(entry.modified),
        created: time(entry.created),
        accessed: time(entry.accessed),
        // The folder the row is in, which the list knows ([`FileRows::set_parent`]).
        path: SharedString::new(),
        original: entry
            .origin
            .as_deref()
            .map(|origin| origin.display().to_string())
            .unwrap_or_default()
            .into(),
        deleted: time(entry.deleted),
        kind: kind_label(entry).into(),
        extension: file_extension(entry)
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .unwrap_or_default()
            .into(),
        attributes: attribute_letters(entry.attributes).into(),
        icon: file_icon(entry).into(),
        is_dir: entry.kind == EntryKind::Dir,
        is_link: entry.kind == EntryKind::Symlink,
        // No bar; a drive of 내 PC has one ([`FileRows::set_drives`]).
        fill: -1.0,
        selected,
        hidden: entry.attributes.contains(Attributes::HIDDEN),
        cut: false,
        thumb: slint::Image::default(),
    }
}

/// The path row of `ancestors` (the root first, the folder itself last).
pub fn crumbs(ancestors: &[VPath]) -> Vec<Crumb> {
    ancestors
        .iter()
        .zip(0..)
        .map(|(path, index)| Crumb {
            label: folder_label(path).into(),
            index,
        })
        .collect()
}

/// The space line of `place` and the used share of its drive (0..1): a drive shows how full
/// it is when the system tells; a favorite shows nothing.
pub fn place_space(place: &Place) -> (String, f32) {
    match (place.kind, place.space) {
        (PlaceKind::Drive, Some(space)) if space.total > 0 => {
            let used = space.total.saturating_sub(space.available);
            #[expect(
                clippy::cast_precision_loss,
                reason = "a share of a drive needs no more than f32 precision"
            )]
            let fill = used as f32 / space.total as f32;
            (words::drive_space(space.available, space.total), fill)
        }
        _ => (String::new(), 0.0),
    }
}

/// What the screen knows the place of `folder` by (`PaneView.place`): 0 a folder, 1 the trash,
/// 2 the computer.
pub fn place_number(folder: &VPath) -> i32 {
    match rf_vfs::ShellPlace::of(folder) {
        None => 0,
        Some(rf_vfs::ShellPlace::Trash) => 1,
        Some(rf_vfs::ShellPlace::Computer) => 2,
    }
}

/// The thumbnail view's size as the screen takes it (`PaneView.thumb-size`); 0 in another view.
fn thumb_size_number(mode: ViewMode) -> i32 {
    mode.thumb_size()
        .and_then(|size| i32::try_from(size).ok())
        .unwrap_or(0)
}

/// The number of the column of the file list that sorts by `key` ([`Column::number`]: 0 name,
/// 1 size, 2 modified, 3 kind, 4 created, 5 extension, 7 accessed).
pub fn sort_column(key: SortKey) -> i32 {
    Column::of_key(key).number()
}

/// The key that the column numbered `column` sorts by; `None` for no column, or the
/// attributes, which sort nothing.
pub fn sort_key(column: i32) -> Option<SortKey> {
    Column::from_number(column)?.sort_key()
}

/// The place of the state that a drag over `place` names, with the pane or tree `side` and the
/// row or tab `index` of the window (`drop-check`, `spot-at`); `None` for a negative tab or tree
/// row. The favorites are one list, whichever tree shows it.
pub fn spot_of(place: DropPlace, side: i32, index: i32) -> Option<DropSpot> {
    let side = if side == 1 { RIGHT } else { LEFT };
    let index = usize::try_from(index).ok();
    Some(match place {
        DropPlace::List => DropSpot::Pane { side, row: index },
        DropPlace::Tab => DropSpot::Tab {
            side,
            index: index?,
        },
        DropPlace::Tree => DropSpot::Tree {
            tree: side,
            row: index?,
        },
        DropPlace::Favorites => DropSpot::Favorites { index },
    })
}

/// The place under the point `at` of the window's content, for a drag of another app: the
/// window's `spot-at`, which finds it by the reckoning of each place's own drop area (the rows
/// and the items of a list, the tabs of a strip). `None` where nothing takes drops, and while a
/// dialog, quick look or a menu of the window shows over it. See [`drop_place`].
///
/// # Preconditions
///
/// No drag of Slint's (a drag of rows or of a tab) is under way, `DragPointer.over`: over the
/// tree the window takes one move of the pointer ([`drop_place`]), which Slint would make a
/// step of that drag. While one is, the tree is not looked at, and its rows give `None`.
pub fn drop_spot(window: &MainWindow, at: WindowPoint) -> Option<DropSpot> {
    spot_found(&drop_place(window, at))
}

/// The place under the point `at` of the window's content, as `spot-at` names it, whether a
/// drop there takes the drag or not ([`drop_spot`] in the window's words): for the list or the
/// tree that scrolls under a drag of another app held near its ends (DND-08).
///
/// The rows of the tree differ in height (a head above the first favorite and the first drive,
/// a drive's space bar), and its list places the rows it has not drawn by their average
/// height, so no reckoning from outside them finds the one under a point: over the tree, the
/// pointer moves there first (a `PointerMoved` of the window), and the row under it says it is
/// the one (`pointed-row`), by Slint's own hit test. That changes only which row shows the
/// pointer over it, as a move of the mouse there would. Not while a drag of Slint's is under
/// way (see [`drop_spot`]'s preconditions): the tree's row is then -1, none.
pub fn drop_place(window: &MainWindow, at: WindowPoint) -> DropAt {
    let mut found = window.invoke_spot_at(at.x, at.y);
    if found.found && in_sidebar(found.place) {
        window.invoke_forget_tree_pointer();
        // While a drag of Slint's is under way the pointer cannot be moved to ask: the point is
        // in the tree, and nothing more is known of it (not its empty part, which a pointer
        // that was moved there and found no row would say).
        if window.global::<DragPointer<'_>>().get_over() {
            return DropAt {
                found: true,
                place: DropPlace::Tree,
                side: 0,
                index: -1,
            };
        }
        let position = slint::LogicalPosition::new(at.x, at.y);
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
        found = window.invoke_spot_at(at.x, at.y);
    }
    found
}

/// Whether `place` is one of the sidebar's: a row of its tree, or the list of favorites in it.
pub fn in_sidebar(place: DropPlace) -> bool {
    matches!(place, DropPlace::Tree | DropPlace::Favorites)
}

/// The pointer that [`drop_place`] moved over the tree leaves the window again (a
/// `PointerExited`, as when the mouse leaves it), so that no row of the tree stays shown as
/// under the mouse after a drag of another app has passed over it. Not while a drag of Slint's
/// is under way, to which no move of `drop_place` went.
pub fn leave_tree(window: &MainWindow) {
    if !window.global::<DragPointer<'_>>().get_over() {
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerExited);
    }
}

/// The place of the state that `found` names ([`spot_of`]); `None` for none.
pub fn spot_found(found: &DropAt) -> Option<DropSpot> {
    found
        .found
        .then(|| spot_of(found.place, found.side, found.index))
        .flatten()
}

/// `spot` as the window names it ([`spot_of`] the other way round), for the frame of the place
/// a drag of another app takes; not `found` for none.
pub fn drop_at(spot: Option<DropSpot>) -> DropAt {
    let number = |n: usize| i32::try_from(n).unwrap_or(i32::MAX);
    let (place, side, index) = match spot {
        None => return DropAt::default(),
        Some(DropSpot::Pane { side, row }) => (DropPlace::List, side, row.map_or(-1, number)),
        Some(DropSpot::Tab { side, index }) => (DropPlace::Tab, side, number(index)),
        Some(DropSpot::Tree { tree, row }) => (DropPlace::Tree, tree, number(row)),
        Some(DropSpot::Favorites { index }) => {
            (DropPlace::Favorites, LEFT, index.map_or(-1, number))
        }
    };
    DropAt {
        found: true,
        place,
        side: number(side),
        index,
    }
}

/// What the payload of a drag of rows holds: the id under which the state noted the items
/// ([`AppState::drag_id`](crate::state::AppState::drag_id)). It stays in this app: no path
/// goes through the system's drag and drop.
#[derive(Debug)]
struct DragToken(u64);

/// The payload of a drag whose items the state noted under `id`.
pub fn drag_payload(id: u64) -> DataTransfer {
    let mut data = DataTransfer::default();
    data.set_user_data(Rc::new(DragToken(id)));
    data
}

/// The id a drag's payload carries; 0, which no drag has, for a payload not made by
/// [`drag_payload`].
pub fn drag_id(data: &DataTransfer) -> u64 {
    data.user_data()
        .and_then(|value| value.downcast::<DragToken>().ok())
        .map_or(0, |token| token.0)
}

/// What the payload of a drag of a tab holds: the id of the tab
/// ([`Tab::id`](crate::pane::Tab::id)). Like a drag of rows, it stays in this app.
#[derive(Debug)]
struct TabToken(u64);

/// The payload of a drag of the tab whose id is `id`.
pub fn tab_payload(id: u64) -> DataTransfer {
    let mut data = DataTransfer::default();
    data.set_user_data(Rc::new(TabToken(id)));
    data
}

/// The id of the tab a drag's payload carries; `None` for a payload not made by
/// [`tab_payload`] (a drag of rows, or one from elsewhere).
pub fn tab_id(data: &DataTransfer) -> Option<u64> {
    data.user_data()
        .and_then(|value| value.downcast::<TabToken>().ok())
        .map(|token| token.0)
}

/// What the payload of a drag of a favorite holds: its place in the list and its folder (so that
/// a list that changed since the press is not mistaken for the one it was dragged from). Like the
/// others, it stays in this app.
#[derive(Debug)]
struct FavoriteToken {
    index: usize,
    folder: String,
}

/// The payload of a drag of the favorite at `index`, whose folder is `folder` (as the settings
/// keep it).
pub fn favorite_payload(index: usize, folder: &str) -> DataTransfer {
    let mut data = DataTransfer::default();
    data.set_user_data(Rc::new(FavoriteToken {
        index,
        folder: folder.to_owned(),
    }));
    data
}

/// The place in the list and the folder of the favorite a drag's payload carries; `None` for a
/// payload not made by [`favorite_payload`].
pub fn favorite_dragged(data: &DataTransfer) -> Option<(usize, String)> {
    data.user_data()
        .and_then(|value| value.downcast::<FavoriteToken>().ok())
        .map(|token| (token.index, token.folder.clone()))
}

/// What the window answers a drag over a place where a drop does `choice`, which its pointer
/// shows: a copy, a move (the menu of 놓을 때 묻기 too, which takes the drop), a link, or none.
pub fn drop_action(choice: DropChoice) -> DragAction {
    match choice {
        DropChoice::Do(DropEffect::Copy) => DragAction::Copy,
        DropChoice::Do(DropEffect::Move) | DropChoice::Ask => DragAction::Move,
        DropChoice::Do(DropEffect::Link) => DragAction::Link,
        DropChoice::Refuse(_) => DragAction::None,
    }
}

/// The modifier keys winit tells (`ModifiersChanged`), as a drop's rule reads them: ⌘ on macOS
/// and the Windows key are `logo`.
pub fn drag_mods(state: ModifiersState) -> DragMods {
    DragMods {
        ctrl: state.control_key(),
        shift: state.shift_key(),
        alt: state.alt_key(),
        logo: state.super_key(),
    }
}

/// Whether the pointer at `position` (winit's, in physical pixels of the window's content) is
/// beyond an edge of the window of `size`: a drag holds the pointer, so the window hears of it
/// there, though nothing in the window is under it.
pub fn outside_window(position: PhysicalPosition<f64>, size: PhysicalSize) -> bool {
    position.x < 0.0
        || position.y < 0.0
        || position.x >= f64::from(size.width)
        || position.y >= f64::from(size.height)
}

/// The number of `default` among the items 끌어 놓기: … of 보기 ▸ 설정: 0 이동, 1 복사, 2 OS
/// 방식, 3 매번 묻기.
pub fn drag_default_number(default: DragDefault) -> i32 {
    match default {
        DragDefault::Move => 0,
        DragDefault::Copy => 1,
        DragDefault::System => 2,
        DragDefault::Ask => 3,
    }
}

/// The default whose item is number `number` ([`drag_default_number`]); `None` for no item.
pub fn drag_default_of(number: i32) -> Option<DragDefault> {
    match number {
        0 => Some(DragDefault::Move),
        1 => Some(DragDefault::Copy),
        2 => Some(DragDefault::System),
        3 => Some(DragDefault::Ask),
        _ => None,
    }
}

/// The way of the visit history's menu the window names `number` (0 behind, 1 ahead); `None`
/// for both, the menu of its key (-1).
pub fn way_of(number: i32) -> Option<Way> {
    match number {
        0 => Some(Way::Back),
        1 => Some(Way::Forward),
        _ => None,
    }
}

/// The lines of the visit history's menu of the active pane's tab (NAV-04), each with the
/// folder it names: the folders `way` goes, nearest first; with no way (its key), the
/// [`HISTORY_KEY_MENU`] nearest ahead, the farthest of them first, then the current folder
/// (checked, off), then the nearest behind, the nearest first, as File Explorer's list of
/// recent locations has them, in no more lines than a menu of one way. Each line is the
/// folder's name and the folder it is in.
pub fn visit_rows(state: &AppState, way: Option<Way>) -> Vec<(VisitEntry, VPath)> {
    let line = |path: &VPath, number: i32, index: usize| {
        let entry = VisitEntry {
            title: visit_title(path).into(),
            way: number,
            index: i32::try_from(index).unwrap_or(i32::MAX),
        };
        (entry, path.clone())
    };
    let lines = |way: Way, number: i32, most: usize| {
        state
            .history_entries(way)
            .iter()
            .take(most)
            .enumerate()
            .map(|(index, path)| line(path, number, index))
            .collect::<Vec<_>>()
    };
    match way {
        Some(Way::Back) => lines(Way::Back, 0, HISTORY_MENU),
        Some(Way::Forward) => lines(Way::Forward, 1, HISTORY_MENU),
        None => {
            let mut rows: Vec<(VisitEntry, VPath)> = lines(Way::Forward, 1, HISTORY_KEY_MENU)
                .into_iter()
                .rev()
                .collect();
            let current = state.panes()[state.active()].active_tab().location();
            rows.push(line(current, -1, 0));
            rows.extend(lines(Way::Back, 0, HISTORY_KEY_MENU));
            rows
        }
    }
}

/// How many characters of the folder a line of the visit history's menu is in show; the middle
/// of a longer path is left out.
const VISIT_PLACE_CHARS: usize = 48;

/// The line of `path` in the visit history's menu ([`words::history_entry`]).
fn visit_title(path: &VPath) -> String {
    let place = path
        .file_name()
        .and(path.parent())
        .map(|parent| short_path(&parent.to_string(), VISIT_PLACE_CHARS));
    words::history_entry(&folder_label(path), place.as_deref())
}

/// Gives the window's toolbar, tooltips and menus the keys of their commands on `os`, from the
/// shortcut table (KEY-03).
pub fn show_shortcuts(window: &MainWindow, os: Os) {
    let shown = window.global::<Shortcuts<'_>>();
    let key =
        |command: Command| SharedString::from(keys::shortcut(&command, os).unwrap_or_default());
    shown.set_back(key(Command::Back));
    shown.set_forward(key(Command::Forward));
    shown.set_visits_menu(key(Command::HistoryMenu));
    shown.set_up(key(Command::Up));
    shown.set_new_folder(key(Command::NewFolder));
    shown.set_copy_to(key(Command::CopyTo));
    shown.set_move_to(key(Command::MoveTo));
    shown.set_trash(key(Command::Trash));
    shown.set_delete_permanently(key(Command::DeletePermanently));
    shown.set_rename(key(Command::Rename));
    shown.set_undo(key(Command::Undo));
    shown.set_redo(key(Command::Redo));
    shown.set_open(key(Command::Open));
    shown.set_toggle_hidden(key(Command::ToggleHidden));
    shown.set_view_menu(key(Command::ViewMenu));
    shown.set_filter(key(Command::FocusFilter));
    shown.set_new_tab(key(Command::NewTab));
    shown.set_close_tab(key(Command::CloseTab));
    shown.set_reopen_tab(key(Command::ReopenTab));
    shown.set_move_tab_left(key(Command::MoveTabLeft));
    shown.set_move_tab_right(key(Command::MoveTabRight));
    shown.set_toggle_tree(key(Command::ToggleTree));
    shown.set_focus_tree(key(Command::FocusTree));
    shown.set_toggle_preview(key(Command::TogglePreview));
    shown.set_toggle_dual_pane(key(Command::ToggleDualPane));
    shown.set_toggle_split(key(Command::ToggleSplit));
    shown.set_swap_panes(key(Command::SwapPanes));
    shown.set_same_folder(key(Command::SameFolderInOther));
    shown.set_sync_navigation(key(Command::SyncNavigation));
    shown.set_view_details(key(Command::ViewDetails));
    shown.set_view_list(key(Command::ViewList));
    shown.set_view_small_icons(key(Command::ViewSmallIcons));
    shown.set_view_medium_icons(key(Command::ViewMediumIcons));
    shown.set_view_large_icons(key(Command::ViewLargeIcons));
    shown.set_fit_columns(key(Command::FitColumns));
    shown.set_select_all(key(Command::SelectAll));
    shown.set_clear_selection(key(Command::ClearSelection));
    shown.set_invert_selection(key(Command::InvertSelection));
    shown.set_select_pattern(key(Command::SelectPattern));
    shown.set_select_same_extension(key(Command::SelectSameExtension));
    shown.set_refresh(key(Command::Refresh));
    shown.set_quick_look(key(Command::QuickLook));
    shown.set_cut(key(Command::Cut));
    shown.set_copy(key(Command::Copy));
    shown.set_paste(key(Command::Paste));
    shown.set_paste_move(key(Command::PasteMove));
    shown.set_copy_path(key(Command::CopyPath));
    shown.set_copy_path_form(PathForm::of_key(os).number());
    shown.set_properties(key(Command::Properties));
    shown.set_open_terminal(key(Command::OpenTerminal));
    shown.set_system_menu(key(Command::SystemMenu));
    show_panel_shortcuts(window, os);
}

/// Gives a window that shows the job panel, the main one or the panel's own (LAY-09), the keys
/// on `os` of the panel's 위로 and 아래로 (KEY-03), from the panel's table
/// ([`keys::panel_shortcut`]). Each window has its own `Shortcuts`: the main window's are set
/// with the rest of its keys ([`show_shortcuts`]), and the panel's own window gets these.
pub fn show_panel_shortcuts<'a, W>(window: &'a W, os: Os)
where
    W: ComponentHandle,
    Shortcuts<'a>: slint::Global<'a, W>,
{
    let shown = window.global::<Shortcuts<'a>>();
    let key = |key: PanelKey| SharedString::from(keys::panel_shortcut(key, os).unwrap_or_default());
    shown.set_move_job_up(key(PanelKey::MoveUp));
    shown.set_move_job_down(key(PanelKey::MoveDown));
}

/// Has `window` take the tokens of its theme from here whenever it asks for them (`Theme.pick`,
/// as it starts, as its `theme-choice`, `density` or `font-scale` changes and as the system's
/// colors turn light or dark): the built-in theme of the choice it shows ([`theme::builtin`]),
/// with the system's high contrast setting read then (`rf_platform::desktop::high_contrast`, a
/// quick look at a setting, never per frame), at the window's density and size of text
/// ([`theme::scaled`], THEME-07; a number it does not know is 보통 or 100 %). Each window that
/// uses `Theme` is given this once, as it is made; its `show-theme` then shows the theme before
/// its first frame.
pub fn follow_theme<W>(window: &W)
where
    W: ComponentHandle + 'static,
    for<'a> Theme<'a>: slint::Global<'a, W>,
{
    let weak = window.as_weak();
    window
        .global::<Theme<'_>>()
        .on_pick(move |choice, system_dark, density, font_scale| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let choice = controller::theme_of(choice).unwrap_or_default();
            let high_contrast = rf_platform::desktop::high_contrast();
            let tokens = theme::builtin(choice, system_dark, high_contrast);
            let density = controller::density_of(density).unwrap_or_default();
            let font_scale = FontScale::of_percent(i64::from(font_scale)).unwrap_or_default();
            show_theme(&window, &theme::scaled(tokens, density, font_scale));
        });
}

/// Gives `window` the tokens of a theme (its `Theme` global): every screen of it shows them at
/// once (THEME-09).
pub fn show_theme<'a, W>(window: &'a W, tokens: &Tokens)
where
    W: ComponentHandle,
    Theme<'a>: slint::Global<'a, W>,
{
    let shown = window.global::<Theme<'a>>();
    let Tokens {
        dark,
        icon_style,
        marks,
        dim_cut,
        dim_hidden,
        color,
        font,
        size,
    } = tokens;
    shown.set_dark(*dark);
    shown.set_marks(*marks);
    shown.set_icon_weight(icon_style.weight());
    shown.set_dim_cut(*dim_cut);
    shown.set_dim_hidden(*dim_hidden);
    let c = |rgba: Rgba| slint::Color::from_argb_u8(rgba.alpha, rgba.red, rgba.green, rgba.blue);
    shown.set_bg(c(color.bg));
    shown.set_panel(c(color.panel));
    shown.set_raised(c(color.raised));
    shown.set_text(c(color.text));
    shown.set_muted(c(color.muted));
    shown.set_faint(c(color.faint));
    shown.set_disabled(c(color.disabled));
    shown.set_border(c(color.border));
    shown.set_pressed(c(color.pressed));
    shown.set_folder(c(color.folder));
    shown.set_warning(c(color.warning));
    shown.set_accent(c(color.accent));
    shown.set_on_accent(c(color.on_accent));
    shown.set_selection(c(color.selection));
    shown.set_text_selection(c(color.text_selection));
    shown.set_on_text_selection(c(color.on_text_selection));
    shown.set_toast_bg(c(color.toast_bg));
    shown.set_toast_text(c(color.toast_text));
    shown.set_toast_link(c(color.toast_link));
    shown.set_toast_hover(c(color.toast_hover));
    shown.set_toast_pressed(c(color.toast_pressed));
    shown.set_shadow(c(color.shadow));
    shown.set_shadow_small(c(color.shadow_small));
    shown.set_shadow_toast(c(color.shadow_toast));
    shown.set_shadow_dialog(c(color.shadow_dialog));
    shown.set_scrim(c(color.scrim));
    shown.set_scrim_light(c(color.scrim_light));
    shown.set_font_ui(font.ui.as_str().into());
    shown.set_font_list(font.list.as_str().into());
    shown.set_font_mono(font.mono.as_str().into());
    shown.set_font_small(font.small);
    shown.set_font_note(font.note);
    shown.set_font_body(font.body);
    shown.set_font_app_name(font.app_name);
    shown.set_font_heading(font.heading);
    shown.set_font_title(font.title);
    shown.set_gap_xs(size.gap_xs);
    shown.set_gap_s(size.gap_s);
    shown.set_gap_m(size.gap_m);
    shown.set_gap_l(size.gap_l);
    shown.set_row_height(size.row_height);
    shown.set_radius(size.radius);
    shown.set_radius_large(size.radius_large);
}

/// The tokens `window` shows (its `Theme` global), as [`show_theme`] gave them: for the tests
/// of the windows.
pub fn shown_theme<'a, W>(window: &'a W) -> Tokens
where
    W: ComponentHandle,
    Theme<'a>: slint::Global<'a, W>,
{
    let shown = window.global::<Theme<'a>>();
    let c = |color: slint::Color| Rgba {
        red: color.red(),
        green: color.green(),
        blue: color.blue(),
        alpha: color.alpha(),
    };
    Tokens {
        dark: shown.get_dark(),
        icon_style: if shown.get_icon_weight() > IconStyle::Line.weight() {
            IconStyle::Bold
        } else {
            IconStyle::Line
        },
        marks: shown.get_marks(),
        dim_cut: shown.get_dim_cut(),
        dim_hidden: shown.get_dim_hidden(),
        color: Colors {
            bg: c(shown.get_bg()),
            panel: c(shown.get_panel()),
            raised: c(shown.get_raised()),
            text: c(shown.get_text()),
            muted: c(shown.get_muted()),
            faint: c(shown.get_faint()),
            disabled: c(shown.get_disabled()),
            border: c(shown.get_border()),
            pressed: c(shown.get_pressed()),
            folder: c(shown.get_folder()),
            warning: c(shown.get_warning()),
            accent: c(shown.get_accent()),
            on_accent: c(shown.get_on_accent()),
            selection: c(shown.get_selection()),
            text_selection: c(shown.get_text_selection()),
            on_text_selection: c(shown.get_on_text_selection()),
            toast_bg: c(shown.get_toast_bg()),
            toast_text: c(shown.get_toast_text()),
            toast_link: c(shown.get_toast_link()),
            toast_hover: c(shown.get_toast_hover()),
            toast_pressed: c(shown.get_toast_pressed()),
            shadow: c(shown.get_shadow()),
            shadow_small: c(shown.get_shadow_small()),
            shadow_toast: c(shown.get_shadow_toast()),
            shadow_dialog: c(shown.get_shadow_dialog()),
            scrim: c(shown.get_scrim()),
            scrim_light: c(shown.get_scrim_light()),
        },
        font: Fonts {
            ui: shown.get_font_ui().into(),
            list: shown.get_font_list().into(),
            mono: shown.get_font_mono().into(),
            small: shown.get_font_small(),
            note: shown.get_font_note(),
            body: shown.get_font_body(),
            app_name: shown.get_font_app_name(),
            heading: shown.get_font_heading(),
            title: shown.get_font_title(),
        },
        size: Sizes {
            gap_xs: shown.get_gap_xs(),
            gap_s: shown.get_gap_s(),
            gap_m: shown.get_gap_m(),
            gap_l: shown.get_gap_l(),
            row_height: shown.get_row_height(),
            radius: shown.get_radius(),
            radius_large: shown.get_radius_large(),
        },
    }
}

/// The app's window as the owner of a dialog of the system and of the window's drop target: its
/// Windows `HWND`, so that the dialogs and the consent prompt come in front of it; on macOS the
/// `NSView` of its content. [`Owner::NONE`] on other systems, for a window of another backend
/// than winit's (the tests'), and before the window is made.
pub fn owner_of(window: &MainWindow) -> Owner {
    use slint::winit_030::WinitWindowAccessor;
    window
        .window()
        .with_winit_window(Owner::of)
        .unwrap_or(Owner::NONE)
}

/// Gives the item menu what it offers for the items it opens for ([`MenuFacts`]).
pub fn show_item_menu(window: &MainWindow, facts: MenuFacts) {
    let menu = window.global::<ItemMenu<'_>>();
    menu.set_one_file(facts.one_file);
    menu.set_runnable(facts.runnable);
    menu.set_one_folder(facts.one_folder);
    menu.set_one_dir(facts.one_dir);
    menu.set_one_item(facts.one_item);
    menu.set_can_restore(facts.can_restore);
}

/// Gives the item menu its 링크 만들기 ▸: the kinds of link `links` offers, named for `os`.
pub fn show_item_links(window: &MainWindow, links: LinkFacts, os: Os) {
    let entries: Vec<LinkEntry> = links
        .choices()
        .map(|choice| LinkEntry {
            title: words::link_entry(choice.kind, choice.enabled, os).into(),
            enabled: choice.enabled,
        })
        .collect();
    window
        .global::<ItemMenu<'_>>()
        .set_links(ModelRc::new(VecModel::from(entries)));
}

/// The models the view of the preview panel and quick look keeps between redraws: the lines of
/// a text and the image of the item shown go to the screen once for each item, not at every
/// redraw.
#[derive(Default)]
pub struct PreviewModels {
    lines: Rc<VecModel<SharedString>>,
    image: RefCell<slint::Image>,
    /// The generation of the item whose content `lines` and `image` hold.
    held: Cell<Option<u64>>,
}

impl fmt::Debug for PreviewModels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreviewModels")
            .field("lines", &self.lines.row_count())
            .field("held", &self.held.get())
            .finish_non_exhaustive()
    }
}

impl PreviewModels {
    /// The view of `preview`, what the preview shows (`None`: nothing), with the names as the
    /// panes show them (`show_extensions`) and the dates in `tz`.
    pub fn view(
        &self,
        preview: Option<PreviewShown<'_>>,
        show_extensions: bool,
        tz: &TimeZone,
    ) -> PreviewView {
        let held = preview.and_then(|shown| shown.content.map(|_| shown.generation));
        if self.held.replace(held) != held {
            let content = preview.and_then(|shown| shown.content);
            let lines = match content {
                Some(PreviewContent::Text(text)) => {
                    text.lines.iter().map(SharedString::from).collect()
                }
                _ => Vec::new(),
            };
            self.lines.set_vec(lines);
            *self.image.borrow_mut() = match content {
                Some(PreviewContent::Image(image)) => {
                    slint::Image::from_rgba8(image.pixels.clone())
                }
                _ => slint::Image::default(),
            };
        }
        let lines = ModelRc::from(Rc::clone(&self.lines));
        let Some(shown) = preview else {
            return PreviewView {
                kind: PreviewKind::None,
                lines,
                ..PreviewView::default()
            };
        };
        let entry = shown.entry;
        let (kind, note) = match shown.content {
            None => (PreviewKind::Loading, String::new()),
            Some(PreviewContent::Image(image)) => (
                PreviewKind::Image,
                words::preview_image_size(image.width, image.height),
            ),
            Some(PreviewContent::Text(text)) => (
                PreviewKind::Text,
                words::preview_text_note(text.encoding, text.truncated),
            ),
            Some(PreviewContent::Folder { items, more }) => {
                (PreviewKind::Folder, words::preview_items(*items, *more))
            }
            Some(PreviewContent::Plain) => (PreviewKind::Plain, String::new()),
            Some(PreviewContent::OnlineOnly) => {
                (PreviewKind::Plain, words::preview_online_only().to_owned())
            }
            Some(PreviewContent::TooLarge) => {
                (PreviewKind::Large, words::preview_too_large().to_owned())
            }
            Some(PreviewContent::Unavailable) => (
                PreviewKind::Unavailable,
                words::preview_unavailable().to_owned(),
            ),
        };
        let size = (entry.kind == EntryKind::File).then(|| format_size(entry.size));
        let modified = entry.modified.map(|time| format_time_in(time, tz));
        PreviewView {
            kind,
            name: shown_name(entry, show_extensions).into(),
            icon: file_icon(entry).into(),
            folder: entry.kind == EntryKind::Dir,
            facts: words::preview_facts(&kind_label(entry), size.as_deref(), modified.as_deref())
                .into(),
            note: note.into(),
            image: self.image.borrow().clone(),
            lines,
        }
    }
}

/// The box the line of a job's speed graph is drawn in, its width and its height: what the job
/// panel says it is (`JobGraph` in `job-panel.slint`, the one place the size is written). The
/// `Path` of the panel's row has this viewbox, in the main window and in the panel's own alike,
/// so the points made for it ([`job_row_view`]) fit the box the row shows.
pub fn graph_box(window: &MainWindow) -> (f32, f32) {
    let graph = window.global::<JobGraph<'_>>();
    (graph.get_width(), graph.get_height())
}

/// The line through `points` as the commands of an SVG path (`M x y L x y …`), to a tenth of a
/// pixel. Empty for fewer than two points, which make no line.
pub fn graph_path(points: &[(f32, f32)]) -> String {
    use std::fmt::Write as _;

    if points.len() < 2 {
        return String::new();
    }
    let mut path = String::new();
    for (i, (x, y)) in points.iter().enumerate() {
        let command = if i == 0 { 'M' } else { 'L' };
        // Writing into a string does not fail.
        let _ = write!(path, "{command}{x:.1} {y:.1} ");
    }
    path.truncate(path.trim_end().len());
    path
}

/// A row of the job panel: of a job a worker has taken up, with the file it works on and the
/// line of its speed graph ([`speed_points`] in the box `graph`, its width and its height:
/// [`graph_box`]), or of a job that waits, with whether it can go up and down the queue.
pub fn job_row_view(row: &JobRow, (width, height): (f32, f32)) -> JobRowView {
    let graph = row.speeds.as_deref().map_or_else(String::new, |speeds| {
        graph_path(&speed_points(speeds, width, height))
    });
    JobRowView {
        title: row.title.as_str().into(),
        detail: row.detail.as_str().into(),
        progress: row.fraction,
        progress_text: row.progress.as_str().into(),
        paused: row.paused,
        waiting: row.waiting,
        current: row.current.as_str().into(),
        graphed: row.speeds.is_some(),
        graph: graph.into(),
        can_up: row.queued.is_some_and(|moves| moves.up),
        can_down: row.queued.is_some_and(|moves| moves.down),
    }
}

/// The rows of the job panel's 되돌리기 기록: the head of the list of what can be undone and its
/// rows, then the head of what can be done again and its rows, each list latest first, and how
/// many heads there are. A list without rows has no head. The rows number their places among
/// the rows of the keys, as [`AppState::panel_lines`](crate::state::AppState::panel_lines) does.
pub fn history_rows(view: &HistoryView) -> (Vec<HistoryRowView>, i32) {
    let count = |n: usize| i32::try_from(n).unwrap_or(i32::MAX);
    let mut rows = Vec::new();
    let mut heads = 0_usize;
    let mut line = 0_usize;
    for (lines, redo, total) in [
        (&view.undo, false, view.undo_total),
        (&view.redo, true, view.redo_total),
    ] {
        if lines.is_empty() {
            continue;
        }
        let title = if redo {
            words::history_redo_section(total)
        } else {
            words::history_undo_section(total)
        };
        rows.push(HistoryRowView {
            head: true,
            title: title.into(),
            line: -1,
            heads_before: count(heads),
            ..HistoryRowView::default()
        });
        heads += 1;
        for (index, each) in lines.iter().enumerate() {
            rows.push(HistoryRowView {
                head: false,
                title: each.text.as_str().into(),
                time: each.time.as_str().into(),
                note: each.note.as_str().into(),
                next: each.next,
                enabled: each.runnable,
                dimmed: each.dimmed,
                redo,
                button: words::history_button(redo).into(),
                line: count(line),
                index: count(index),
                heads_before: count(heads),
            });
            line += 1;
        }
    }
    (rows, count(heads))
}

/// A line of the job panel's 최근 완료.
pub fn recent_row_view(row: &RecentRow) -> RecentRowView {
    RecentRowView {
        title: row.title.as_str().into(),
        detail: row.detail.as_str().into(),
        stats: row.stats.as_str().into(),
        details: row.details,
        ok: row.ok,
    }
}

/// Which dialog shows for `modal`, or the closing note while the app ends.
pub fn dialog_kind(modal: Option<&Modal>, quitting: bool) -> DialogKind {
    if quitting {
        return DialogKind::Closing;
    }
    match modal {
        None => DialogKind::None,
        Some(Modal::Name(_)) => DialogKind::Name,
        Some(Modal::Confirm(_)) => DialogKind::Confirm,
        Some(Modal::Failures(_)) => DialogKind::Message,
        Some(Modal::Recovery(_)) => DialogKind::Recovery,
        Some(Modal::Conflict(_)) => DialogKind::Conflict,
        Some(Modal::Error(_)) => DialogKind::Error,
        Some(Modal::Preflight(_)) => DialogKind::Preflight,
        Some(Modal::Pick(_)) => DialogKind::Pick,
        Some(Modal::Pattern(_)) => DialogKind::Pattern,
        Some(Modal::Properties) => DialogKind::Properties,
    }
}

/// The models the view of the properties window keeps between its views, and which window and
/// revision of it they show: a view is made only when the window changed (a worker's news, a
/// choice), not at each redraw of the main window.
#[derive(Default)]
pub struct PropertiesModels {
    facts: Rc<VecModel<FactRow>>,
    hashes: Rc<VecModel<HashRowView>>,
    /// The id and revision of the window the models show.
    shown: Cell<Option<(u64, u64)>>,
}

impl fmt::Debug for PropertiesModels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PropertiesModels")
            .field("hashes", &self.hashes.row_count())
            .field("shown", &self.shown.get())
            .finish_non_exhaustive()
    }
}

impl PropertiesModels {
    /// The view of `properties`, with its times in `tz`, when it is another window or another
    /// revision of it than the one shown last; `None` when the screen shows it already.
    pub fn view(&self, properties: &Properties, tz: &TimeZone) -> Option<PropertiesView> {
        let now = Some((properties.id, properties.revision));
        if self.shown.replace(now) == now {
            return None;
        }
        set_rows(
            &self.facts,
            properties::facts(properties, tz)
                .into_iter()
                .map(|fact| FactRow {
                    label: fact.label.into(),
                    value: fact.value.into(),
                    full: fact.full.unwrap_or_default().into(),
                })
                .collect(),
        );
        // Made once, for the rows and for the comparison.
        let lines = properties::hash_lines(properties);
        let verdict = properties::verdict(properties, &lines);
        set_rows(
            &self.hashes,
            lines
                .into_iter()
                .map(|line| HashRowView {
                    name: line.name.into(),
                    value: line.value.into(),
                    progress: line.progress.unwrap_or(-1.0),
                    done: line.done,
                    copied: line.copied,
                    matched: line.matched,
                })
                .collect(),
        );
        let (title, icon, folder) = match properties.single() {
            Some(item) => {
                let (icon, folder) = item.entry.as_ref().map_or(("folder", true), |entry| {
                    (file_icon(entry), entry.kind == EntryKind::Dir)
                });
                (properties.name(0), icon, folder)
            }
            None if properties.chosen.files == 0 => {
                (words::item_count(properties.items.len()), "folder", true)
            }
            None => (words::item_count(properties.items.len()), "file", false),
        };
        let verdict_text = match &verdict {
            Verdict::None => String::new(),
            Verdict::Match(name, count) => words::verdict_match(name.as_deref(), *count),
            Verdict::Mismatch => words::verdict_mismatch().to_owned(),
            Verdict::Waiting => words::verdict_waiting().to_owned(),
        };
        let hashing = &properties.hashing;
        Some(PropertiesView {
            title: title.into(),
            icon: icon.into(),
            folder,
            facts: ModelRc::from(Rc::clone(&self.facts)),
            counting: properties.count.status == properties::CountStatus::Counting,
            count_note: properties::count_note(properties).into(),
            hashable: !hashing.rows.is_empty(),
            hashing: hashing.under_way(),
            algorithm: hashing.algorithm.map_or(-1, Algorithm::number),
            hash_note: properties::hash_note(properties).into(),
            several: hashing.rows.len() > 1,
            system: properties.single().is_some(),
            hashes: ModelRc::from(Rc::clone(&self.hashes)),
            verdict: verdict_text.into(),
            matched: matches!(verdict, Verdict::Match(..)),
            mismatched: verdict == Verdict::Mismatch,
        })
    }
}

/// The models the picker's view keeps between redraws.
#[derive(Default)]
pub struct PickModels {
    tree: Rc<TreeRows>,
    /// What the rows of `tree` were made of last: see [`PickModels::view`].
    tree_made_of: RefCell<Option<TreeRowsKey>>,
    targets: Rc<VecModel<PickTarget>>,
    chips: Rc<VecModel<SharedString>>,
}

/// What the rows of the picker's tree are made of: the [`Tree::revision`] of its tree (whose
/// roots change only with a new revision), what its filter found, and the request whose items
/// they are marked for.
#[derive(PartialEq)]
struct TreeRowsKey {
    revision: u64,
    found: Option<Vec<NodeId>>,
    request: PickRequest,
}

impl fmt::Debug for PickModels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PickModels")
            .field("tree", &self.tree)
            .field("targets", &self.targets.row_count())
            .finish_non_exhaustive()
    }
}

impl PickModels {
    /// What `picker` shows, with its folder tree `tree` (whose roots are `places`), or the
    /// folders `found` there while its filter has found some. The models are brought up to
    /// date and handed over as they are. The tree's rows, which look at every folder shown
    /// for whether it can take the items, are made again only when the tree, what was found
    /// or the request changed, not at each redraw (each keystroke in a field).
    pub fn view(
        &self,
        picker: &Picker,
        tree: &Tree,
        found: Option<&[NodeId]>,
        places: &[Place],
    ) -> PickView {
        let same = self.tree_made_of.borrow().as_ref().is_some_and(|key| {
            key.revision == tree.revision()
                && key.found.as_deref() == found
                && key.request == picker.request
        });
        if !same {
            self.tree.show(pick_tree_rows(picker, tree, found, places));
            *self.tree_made_of.borrow_mut() = Some(TreeRowsKey {
                revision: tree.revision(),
                found: found.map(<[NodeId]>::to_vec),
                request: picker.request.clone(),
            });
        }
        set_rows(&self.targets, pick_targets(picker));
        let request = &picker.request;
        set_rows(
            &self.chips,
            request
                .sources
                .iter()
                .take(PICK_CHIPS)
                .map(|path| folder_label(path).into())
                .collect(),
        );
        let more = request.sources.len().saturating_sub(PICK_CHIPS);
        // While the path field says something else, what is known of the target is beside
        // the point: only what to do about the field shows.
        let field_pending = picker.path_edited || picker.typed.is_some();
        let validity = picker.validity().filter(|_| !field_pending);
        let folder_ok = picker
            .facts
            .as_ref()
            .is_none_or(|facts| facts.folder.is_ok());
        let space = match &picker.facts {
            _ if field_pending || picker.target.is_none() => String::new(),
            None => words::pick_space_pending().to_owned(),
            Some(facts) => facts
                .available
                .filter(|_| facts.folder.is_ok())
                .map(words::pick_space)
                .unwrap_or_default(),
        };
        let (policy_note, policy_danger) = words::pick_policy(picker.policy);
        let (title, summary, confirm_text) = pick_words(picker);
        PickView {
            title: title.into(),
            summary: summary.into(),
            chips: ModelRc::from(Rc::clone(&self.chips)),
            more: if more > 0 {
                words::pick_more(more).into()
            } else {
                SharedString::new()
            },
            is_move: request.mode == TransferMode::Move,
            target_path: picker
                .target
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default()
                .into(),
            path_error: picker.path_error.as_str().into(),
            path_edited: picker.path_edited,
            can_make_folder: picker.target.is_some() && folder_ok && !field_pending,
            filter: picker.filter.as_str().into(),
            tree: ModelRc::from(Rc::clone(&self.tree)),
            tree_note: match found {
                Some([]) => words::pick_no_match().into(),
                _ => SharedString::new(),
            },
            targets: ModelRc::from(Rc::clone(&self.targets)),
            policy: POLICIES
                .iter()
                .position(|&policy| policy == picker.policy)
                .and_then(|index| i32::try_from(index).ok())
                .unwrap_or(0),
            policy_note: policy_note.into(),
            policy_danger,
            can_follow_links: picker.offers_follow_links(),
            follow_links: picker.follow_links,
            space: space.into(),
            problem: validity
                .map(|validity| {
                    if picker.for_links() {
                        words::pick_link_problem(validity)
                    } else {
                        words::pick_problem(validity, request.mode, request.folder_count > 0)
                    }
                })
                .unwrap_or_default()
                .into(),
            hint: pick_hint(picker).into(),
            unentered_hint: words::pick_path_unentered().into(),
            confirm_text: confirm_text.into(),
            can_confirm: picker.can_confirm(),
            for_log: picker.without_options(),
        }
    }
}

/// The title, the line under it and the confirm button of `picker`, as what it finds a folder for
/// words them.
fn pick_words(picker: &Picker) -> (String, String, String) {
    let request = &picker.request;
    match &picker.purpose {
        PickPurpose::Transfer => (
            words::pick_title(request.mode).to_owned(),
            crate::pick::summary(request),
            words::pick_confirm(request.mode).to_owned(),
        ),
        PickPurpose::SaveLog { title, detail, .. } => (
            words::pick_log_title().to_owned(),
            words::pick_log_summary(title, detail),
            words::pick_log_confirm().to_owned(),
        ),
        PickPurpose::Link { kind } => (
            words::pick_link_title().to_owned(),
            words::pick_link_summary(*kind, request.sources.len(), Os::CURRENT),
            words::pick_link_confirm().to_owned(),
        ),
        PickPurpose::Restore { items } => (
            words::pick_restore_title().to_owned(),
            words::pick_restore_summary(items.len()),
            words::pick_restore_confirm().to_owned(),
        ),
    }
}

/// What the picker says beside its buttons about how it confirms: that the path typed must be
/// entered first (the button is off meanwhile), or a path or a folder given when the field was
/// cleared, that it is being looked at, or that an overwrite (a conditional one too) is
/// confirmed by the button alone; empty for none of these.
pub fn pick_hint(picker: &Picker) -> &'static str {
    if picker.typed.is_some() {
        words::pick_path_pending()
    } else if picker.path_edited && picker.path_blank {
        words::pick_path_blank()
    } else if picker.path_edited {
        words::pick_path_unentered()
    } else if overwrites(picker.policy) {
        words::pick_overwrite_by_button()
    } else {
        ""
    }
}

/// The rows of the picker's tree `tree` (whose roots are `places`), or of the folders `found`
/// there by its filter: a folder that cannot take the items of `picker` shows dimmed, with why
/// beside it, as among the quick targets.
pub fn pick_tree_rows(
    picker: &Picker,
    tree: &Tree,
    found: Option<&[NodeId]>,
    places: &[Place],
) -> Vec<(NodeId, TreeRow)> {
    let mut rows = match found {
        Some(found) => found_rows(tree, found),
        None => tree_rows(&tree.rows(), places),
    };
    let request = &picker.request;
    for (id, row) in &mut rows {
        let Some(node) = tree.node(*id) else {
            continue;
        };
        let validity = validate(request, &node.path, None);
        if !validity.can_choose() {
            row.dimmed = true;
            row.info = words::pick_problem(validity, request.mode, request.folder_count > 0).into();
        }
    }
    rows
}

/// The quick targets of `picker`, one row each: the head over the first of a group, the
/// path under the name, or why it cannot be chosen.
pub fn pick_targets(picker: &Picker) -> Vec<PickTarget> {
    let request = &picker.request;
    picker
        .groups
        .iter()
        .flat_map(|group| {
            group.items.iter().enumerate().map(move |(i, item)| {
                let enabled = item.validity.can_choose();
                let note = if enabled {
                    item.note.clone()
                } else {
                    words::pick_problem(item.validity, request.mode, request.folder_count > 0)
                };
                PickTarget {
                    head: if i == 0 {
                        group.title.as_str().into()
                    } else {
                        SharedString::new()
                    },
                    label: item.label.as_str().into(),
                    note: note.into(),
                    enabled,
                    selected: picker.target.as_ref() == Some(&item.path),
                }
            })
        })
        .collect()
}

/// What the name dialog shows: the warning of the name typed so far, and whether OK takes it,
/// with `serial`, the number of the item it names (a new one starts the window again).
pub fn name_view(dialog: &NameDialog, serial: i32) -> NameView {
    let check = dialog.check();
    NameView {
        serial,
        title: dialog.title().into(),
        initial: dialog.text.as_str().into(),
        select_end: i32::try_from(dialog.select_end).unwrap_or(i32::MAX),
        ok_enabled: check == NameCheck::Ready,
        warning: match check {
            NameCheck::Invalid(warning) => warning.into(),
            NameCheck::Ready | NameCheck::Empty | NameCheck::Unchanged => SharedString::new(),
        },
    }
}

/// The pattern window (패턴으로 선택): its choices, how many rows match (the last count while a
/// worker matches again, "세는 중…" before the first, or that the worker could not count them),
/// or why the text is no pattern.
pub fn pattern_view(dialog: &PatternDialog) -> PatternView {
    let count = match (dialog.problem, dialog.count) {
        (Some(_), _) => SharedString::new(),
        _ if dialog.empty => SharedString::new(),
        _ if dialog.counting == Counting::Failed => words::pattern_uncounted().into(),
        (None, Some(count)) => words::pattern_count(count).into(),
        (None, None) if dialog.counting == Counting::Running => words::pattern_counting().into(),
        (None, None) => SharedString::new(),
    };
    PatternView {
        initial: dialog.initial.as_str().into(),
        regex: dialog.choice.syntax == crate::pattern::PatternSyntax::Regex,
        ignore_case: !dialog.choice.match_case,
        add: dialog.choice.add,
        count,
        problem: dialog
            .problem
            .map(words::pattern_problem)
            .unwrap_or_default()
            .into(),
        ok_enabled: dialog.can_confirm(),
    }
}

/// The confirmation; its first `MEASURED_LINES` lines go in `head` too, as in
/// [`message_view`].
pub fn confirm_view(confirm: &Confirm) -> ConfirmView {
    ConfirmView {
        title: confirm.title.as_str().into(),
        message: confirm.message.as_str().into(),
        detail: confirm.detail.as_str().into(),
        lines: lines_model(&confirm.lines),
        head: lines_model(head(&confirm.lines)),
        question: confirm.question.as_str().into(),
        ok_text: confirm.ok.as_str().into(),
        danger: confirm.danger,
        alt_text: confirm.alt.as_str().into(),
        check_text: confirm.check.as_str().into(),
        checked: confirm.checked,
    }
}

/// The window of failed items, with 다시 시도 when they can be tried again. Its first
/// `MEASURED_LINES` lines go in `head` too: the window fits its list to their height once
/// wrapped.
pub fn message_view(failures: &Failures) -> MessageView {
    MessageView {
        title: failures.title.as_str().into(),
        lines: lines_model(&failures.lines),
        head: lines_model(head(&failures.lines)),
        retry: failures.retry.is_some(),
    }
}

/// The window that asks about the item of `question` that failed (XFER-09): what the job was
/// doing, the item by its whole path, and why.
pub fn error_view(question: &ErrorQuestion) -> ErrorView {
    ErrorView {
        serial: i32::try_from(question.id % 0x7FFF_FFFF).unwrap_or(0),
        title: words::error_ask_title(question.work).into(),
        item: words::error_ask_item(&question.failure).into(),
        reason: words::error_ask_reason(&question.failure).into(),
        items_left: if question.items_left > 0 {
            words::items_left(question.items_left).into()
        } else {
            SharedString::new()
        },
        retry_note: words::error_ask_retry_once().into(),
    }
}

/// The first `MEASURED_LINES` of `lines`.
fn head(lines: &[String]) -> &[String] {
    &lines[..lines.len().min(MEASURED_LINES)]
}

/// The recovery dialog, with the times the jobs were submitted in `tz`.
pub fn recovery_view(recovery: &Recovery, tz: &TimeZone) -> RecoveryView {
    let rows: Vec<String> = recovery
        .jobs
        .iter()
        .map(|job| {
            let submitted = format_time_in(job.submitted, tz);
            words::recovered_job(&job.spec, &submitted, job.started)
        })
        .collect();
    RecoveryView {
        title: words::recovery_title(recovery.jobs.len()).into(),
        note: words::recovery_note().into(),
        rows: lines_model(&rows),
    }
}

/// The pre-flight window for `question`; its first `MEASURED_LINES` lines go in `head` too, as
/// in [`message_view`].
pub fn preflight_view(question: &PreflightQuestion) -> PreflightView {
    let lines = question.lines();
    PreflightView {
        title: words::preflight_title().into(),
        lines: lines_model(&lines),
        head: lines_model(head(&lines)),
        has_blocked: question.has_blocked(),
    }
}

/// The choice of the conflict window's row `number`, as its `answered` callback numbers them:
/// 0 덮어쓰기, 1 건너뛰기, 2 이름 바꿔 둘 다 두기, 3 더 새 파일만 덮어쓰기, 4 크기가 다를 때만
/// 덮어쓰기. Any other number is the safe one, keeping both.
pub fn conflict_choice(number: i32) -> ConflictChoice {
    match number {
        0 => ConflictChoice::Overwrite,
        1 => ConflictChoice::Skip,
        3 => ConflictChoice::OverwriteIfNewer,
        4 => ConflictChoice::OverwriteIfSizeDiffers,
        _ => ConflictChoice::Rename,
    }
}

/// The choice of the error window's button `number`, as its `answered` callback numbers them:
/// 0 다시 시도, 2 중단. Any other number is the one that neither repeats nor stops, 건너뛰기 (1).
pub fn error_choice(number: i32) -> ErrorChoice {
    match number {
        0 => ErrorChoice::Retry,
        2 => ErrorChoice::Abort,
        _ => ErrorChoice::Skip,
    }
}

/// The conflict dialog for `question`, with dates in `tz`.
pub fn conflict_view(question: &ConflictQuestion, tz: &TimeZone) -> ConflictView {
    let conflict = &question.conflict;
    let (incoming_newer, existing_newer) = question.newer();
    let name_of = |path: &VPath| {
        path.file_name().map_or_else(
            || path.to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    let folder_of = |path: &VPath, max: usize| {
        path.parent()
            .map(|folder| short_path(&folder.to_string(), max))
            .unwrap_or_default()
    };
    let size = |entry: &Entry| {
        if entry.kind == EntryKind::Dir {
            "-".to_owned()
        } else {
            words::exact_size(entry.size)
        }
    };
    let time = |entry: &Entry| {
        entry
            .modified
            .map(|t| format_time_in(t, tz))
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "-".to_owned())
    };
    let existing_name = name_of(&conflict.target);
    let existing_folder = folder_of(&conflict.target, CARD_PLACE_CHARS);
    let can_overwrite = question.can_overwrite();
    // What each conditional choice does to this file, as the engine would decide it.
    let newer_overwrites = question.would_overwrite(ConflictChoice::OverwriteIfNewer);
    let size_overwrites = question.would_overwrite(ConflictChoice::OverwriteIfSizeDiffers);
    ConflictView {
        serial: i32::try_from(question.id % 0x7FFF_FFFF).unwrap_or(0),
        title: words::conflict_title(conflict.existing.kind).into(),
        description: words::conflict_description(
            &existing_name,
            &folder_of(&conflict.target, SENTENCE_PLACE_CHARS),
        )
        .into(),
        incoming_label: words::conflict_card(true, conflict.incoming.kind, question.work).into(),
        incoming_name: name_of(&conflict.source).into(),
        incoming_place: folder_of(&conflict.source, CARD_PLACE_CHARS).into(),
        incoming_size: size(&conflict.incoming).into(),
        incoming_time: time(&conflict.incoming).into(),
        incoming_newer,
        incoming_dir: conflict.incoming.kind == EntryKind::Dir,
        existing_label: words::conflict_card(false, conflict.existing.kind, question.work).into(),
        existing_name: existing_name.as_str().into(),
        existing_place: existing_folder.as_str().into(),
        existing_size: size(&conflict.existing).into(),
        existing_time: time(&conflict.existing).into(),
        existing_newer,
        existing_dir: conflict.existing.kind == EntryKind::Dir,
        can_overwrite,
        restore: !question.offers_overwrite(),
        overwrite_hint: words::overwrite_hint(can_overwrite, question.work).into(),
        skip_hint: words::skip_hint(conflict.incoming.kind, question.work).into(),
        rename_hint: words::rename_hint(&question.numbered_name()).into(),
        newer_hint: words::newer_hint(can_overwrite, newer_overwrites, question.work).into(),
        newer_overwrites,
        size_hint: words::size_differs_hint(can_overwrite, size_overwrites).into(),
        size_overwrites,
        warning_once: words::overwrite_warning(false).into(),
        warning_all: words::overwrite_warning(true).into(),
        warning_rule_all: words::conditional_warning_all().into(),
        items_left: if question.items_left > 0 {
            words::items_left(question.items_left).into()
        } else {
            SharedString::new()
        },
    }
}

/// `lines` as a list model of text.
fn lines_model(lines: &[String]) -> ModelRc<SharedString> {
    let lines: Vec<SharedString> = lines.iter().map(|line| line.as_str().into()).collect();
    ModelRc::new(VecModel::from(lines))
}

/// Makes `model` hold `rows`, telling the screen only about the rows that differ.
pub fn set_rows<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    if model.row_count() != rows.len() {
        model.set_vec(rows);
        return;
    }
    for (i, row) in rows.into_iter().enumerate() {
        if model.row_data(i).as_ref() != Some(&row) {
            model.set_row_data(i, row);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::time::{Duration, UNIX_EPOCH};

    use rf_core::ErrorKind;
    use rf_platform::DiskSpace;
    use rf_vfs::VisibilityFilter;
    use slint::SharedPixelBuffer;

    use super::*;
    use crate::loader::LoadEvent;

    fn names(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_graph_s_line_goes_through_its_points() {
        // Fewer than two points make no line.
        assert_eq!(graph_path(&[]), "");
        assert_eq!(graph_path(&[(120.0, 0.0)]), "");
        assert_eq!(
            graph_path(&[(115.93, 28.0), (117.966, 14.0), (120.0, 0.0)]),
            "M115.9 28.0 L118.0 14.0 L120.0 0.0"
        );
        // Samples of zero draw a line along the bottom of the box.
        let flat = graph_path(&speed_points(&[0, 0], 120.0, 28.0));
        assert_eq!(flat, "M118.0 28.0 L120.0 28.0");
        // A full minute spans the box.
        let full: Vec<u64> = (0..60).collect();
        let line = graph_path(&speed_points(&full, 120.0, 28.0));
        assert!(line.starts_with("M0.0 28.0 L"), "{line}");
        assert!(line.ends_with(" L120.0 0.0"), "{line}");
        assert_eq!(line.matches('L').count(), 59);
    }

    #[test]
    fn a_job_s_row_shows_its_file_its_graph_and_its_place_in_the_queue() {
        use crate::jobs::QueueMoves;

        let row = JobRow {
            id: rf_jobs::JobId(1),
            title: "복사 중".to_owned(),
            detail: "assets → 백업".to_owned(),
            fraction: 0.5,
            progress: "1 / 2 항목".to_owned(),
            paused: false,
            waiting: false,
            current: "로고.png".to_owned(),
            speeds: Some(vec![10, 20]),
            queued: None,
        };
        let graph = (120.0, 28.0);
        let view = job_row_view(&row, graph);
        assert_eq!(view.current, "로고.png");
        assert!(view.graphed);
        assert_eq!(view.graph, "M118.0 14.0 L120.0 0.0");
        assert!(!view.can_up && !view.can_down);
        // The line is made for the box it is given: in one half as large, it is half as large.
        assert_eq!(
            job_row_view(&row, (60.0, 14.0)).graph,
            "M59.0 7.0 L60.0 0.0"
        );
        // A copy that has no sample yet has its box, and no line in it.
        let fresh = job_row_view(
            &JobRow {
                speeds: Some(Vec::new()),
                ..row.clone()
            },
            graph,
        );
        assert!(fresh.graphed);
        assert_eq!(fresh.graph, "");
        // A job that moves no bytes has neither.
        let trash = job_row_view(
            &JobRow {
                speeds: None,
                ..row.clone()
            },
            graph,
        );
        assert!(!trash.graphed);
        assert_eq!(trash.graph, "");
        // A job that waits: its buttons are on for the ways it can go.
        for (up, down) in [(false, false), (false, true), (true, true), (true, false)] {
            let waits = JobRow {
                speeds: None,
                queued: Some(QueueMoves { up, down }),
                ..row.clone()
            };
            let view = job_row_view(&waits, graph);
            assert_eq!((view.can_up, view.can_down), (up, down));
            assert!(!view.graphed);
        }
    }

    /// The root of the system's drive, which `VPath::local` takes on every system: `C:\` on
    /// Windows, where a bare `\` names no drive, and `/` elsewhere.
    fn system_root() -> VPath {
        VPath::local(if cfg!(windows) { r"C:\" } else { "/" }).expect("the root is local")
    }

    /// The sidebar name of [`system_root`], as `places::drives` gives it.
    fn system_drive_label() -> String {
        if cfg!(windows) {
            words::drive_letter('C')
        } else {
            words::system_disk().to_owned()
        }
    }

    /// `old` with `steps` applied the way Slint applies notifications to its rows.
    fn apply<T: Clone>(old: &[T], new: &[T], steps: &[RowChange]) -> Vec<Option<T>> {
        let mut list: Vec<Option<T>> = old.iter().cloned().map(Some).collect();
        for step in steps {
            match *step {
                RowChange::Removed { at, count } => {
                    list.drain(at..at + count);
                }
                RowChange::Added { at, count } => {
                    for k in 0..count {
                        list.insert(at + k, Some(new[at + k].clone()));
                    }
                }
                RowChange::Changed { at } => list[at] = Some(new[at].clone()),
            }
        }
        list
    }

    fn check(old: &[&str], new: &[&str]) -> Vec<RowChange> {
        let (old, new) = (names(old), names(new));
        let steps = diff_rows(&old, &new, OsString::as_os_str).expect("a few steps");
        let applied = apply(&old, &new, &steps);
        let expected: Vec<Option<OsString>> = new.iter().cloned().map(Some).collect();
        assert_eq!(applied, expected, "{steps:?}");
        steps
    }

    #[test]
    fn diff_of_equal_lists_is_empty() {
        assert_eq!(check(&["a", "b", "c"], &["a", "b", "c"]), []);
        assert_eq!(check(&[], &[]), []);
    }

    #[test]
    fn diff_adds_and_removes_in_runs() {
        assert_eq!(
            check(&["a", "d"], &["a", "b", "c", "d"]),
            [RowChange::Added { at: 1, count: 2 }]
        );
        assert_eq!(
            check(&["a", "b", "c", "d"], &["a", "d"]),
            [RowChange::Removed { at: 1, count: 2 }]
        );
        assert_eq!(
            check(&["a", "b", "c"], &["x", "a", "c", "y"]),
            [
                RowChange::Added { at: 0, count: 1 },
                RowChange::Removed { at: 2, count: 1 },
                RowChange::Added { at: 3, count: 1 },
            ]
        );
        check(&[], &["a", "b"]);
        check(&["a", "b"], &[]);
    }

    #[test]
    fn diff_moves_a_row_that_changed_place() {
        check(&["a", "b", "c", "d"], &["b", "c", "d", "a"]);
        check(&["b", "c", "d", "a"], &["a", "b", "c", "d"]);
        check(&["a", "b", "c", "d", "e"], &["e", "d", "c", "b", "a"]);
        check(&["a", "b", "c"], &["c", "x", "a"]);
    }

    #[test]
    fn diff_reports_rows_that_changed_in_place() {
        let old = [("a", 1), ("b", 1), ("c", 1)];
        let new = [("a", 1), ("b", 2), ("x", 0), ("c", 3)];
        let steps = diff_rows(&old, &new, |row| OsStr::new(row.0)).expect("a few steps");
        // The changes come last, at their places in the new list.
        assert_eq!(
            steps,
            [
                RowChange::Added { at: 2, count: 1 },
                RowChange::Changed { at: 1 },
                RowChange::Changed { at: 3 },
            ]
        );
        let applied: Vec<_> = apply(&old, &new, &steps).into_iter().flatten().collect();
        assert_eq!(applied, new);
    }

    #[test]
    fn diff_gives_up_on_many_changes() {
        let old: Vec<String> = (0..2000).map(|i| format!("a{i}")).collect();
        let new: Vec<String> = (0..2000).step_by(2).map(|i| format!("a{i}")).collect();
        assert_eq!(diff_rows(&old, &new, |s| OsStr::new(s.as_str())), None);
        // A long list with one change is still told row by row.
        let mut one_more = old.clone();
        one_more.insert(1000, "new".to_owned());
        assert_eq!(
            diff_rows(&old, &one_more, |s| OsStr::new(s.as_str())),
            Some(vec![RowChange::Added { at: 1000, count: 1 }])
        );
    }

    fn entry(name: &str, kind: EntryKind, size: u64) -> Entry {
        let mut entry = Entry::new(name, kind);
        entry.size = size;
        entry
    }

    #[test]
    fn rows_show_what_the_list_needs() {
        let mut pdf = entry("설계안.pdf", EntryKind::File, 1_258_291);
        pdf.modified = Some(UNIX_EPOCH + Duration::from_mins(29_356_280));
        let row = file_row(&pdf, true, &TimeZone::UTC, true);
        assert_eq!(row.name, "설계안.pdf");
        // Without the extensions, the name before it; the 확장자 column keeps it.
        let bare = file_row(&pdf, true, &TimeZone::UTC, false);
        assert_eq!(
            (bare.name.as_str(), bare.extension.as_str()),
            ("설계안", "pdf")
        );
        assert_eq!(row.size, "1.2 MB");
        assert_eq!(row.modified, "2025-10-25 07:20");
        assert_eq!(row.kind, "PDF 문서");
        assert!(row.selected && !row.is_dir && !row.is_link && !row.hidden);

        let mut dot = entry(".config", EntryKind::Dir, 4096);
        dot.attributes |= Attributes::HIDDEN;
        let row = file_row(&dot, false, &TimeZone::UTC, true);
        // A folder has no size, and no time when none is known.
        assert_eq!((row.size.as_str(), row.modified.as_str()), ("", ""));
        assert!(row.is_dir && row.hidden && !row.selected);

        let link = file_row(
            &entry("latest", EntryKind::Symlink, 9),
            false,
            &TimeZone::UTC,
            true,
        );
        assert!(link.is_link && !link.is_dir);
        assert_eq!(link.size, "");
        // A link may lead to a folder: its 확장자 column is empty, as its name shows whole.
        let album = file_row(
            &entry("앨범.2025", EntryKind::Symlink, 9),
            false,
            &TimeZone::UTC,
            false,
        );
        assert_eq!(
            (album.name.as_str(), album.extension.as_str()),
            ("앨범.2025", "")
        );
    }

    #[test]
    fn file_rows_follow_entries_and_selection() {
        let model = FileRows::default();
        let mut selection = Selection::default();
        let list = vec![
            entry("a", EntryKind::Dir, 0),
            entry("b.txt", EntryKind::File, 10),
        ];
        model.sync(&list, &selection);
        assert_eq!(model.row_count(), 2);
        assert_eq!(model.row_data(1).map(|row| row.name), Some("b.txt".into()));
        assert!(!model.row_data(1).is_some_and(|row| row.selected));

        selection.click(1, &list);
        model.sync(&list, &selection);
        assert!(model.row_data(1).is_some_and(|row| row.selected));

        let mut changed = list.clone();
        changed[1].size = 2048;
        changed.insert(1, entry("ab", EntryKind::File, 1));
        model.sync(&changed, &selection);
        let shown: Vec<FileRow> = (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .collect();
        assert_eq!(shown.len(), 3);
        assert_eq!(shown[1].name, "ab");
        assert_eq!(shown[2].size, "2 KB");
        // The selection is by name.
        assert!(shown[2].selected && !shown[1].selected);
        assert_eq!(model.row_data(3), None);
    }

    #[test]
    fn file_rows_dim_the_items_cut_by_name() {
        let model = FileRows::default();
        let selection = Selection::default();
        let list = vec![
            entry("a", EntryKind::Dir, 0),
            entry("b.txt", EntryKind::File, 10),
        ];
        let cut = |model: &FileRows| -> Vec<bool> {
            (0..model.row_count())
                .filter_map(|i| model.row_data(i).map(|row| row.cut))
                .collect()
        };
        model.sync(&list, &selection);
        assert_eq!(cut(&model), [false, false]);
        // Dimmed from the next sync on, the rows as they are.
        model.set_cut(HashSet::from([OsString::from("b.txt")]));
        assert_eq!(cut(&model), [false, false]);
        model.sync(&list, &selection);
        assert_eq!(cut(&model), [false, true]);
        // A row that comes keeps the dimming of its name; one that goes takes it along.
        let mut changed = list.clone();
        changed.insert(0, entry("0.txt", EntryKind::File, 1));
        model.sync(&changed, &selection);
        assert_eq!(cut(&model), [false, false, true]);
        model.set_cut(HashSet::new());
        model.sync(&changed, &selection);
        assert_eq!(cut(&model), [false, false, false]);
        // The row of `file_row` alone is never dimmed.
        let row = file_row(&list[1], false, &TimeZone::UTC, true);
        assert!(!row.cut);
    }

    #[test]
    fn a_list_keeps_its_thumbnails_to_a_count_and_to_bytes() {
        let picture = |side: u32| {
            slint::Image::from_rgba8(SharedPixelBuffer::<slint::Rgba8Pixel>::new(side, side))
        };
        let stamp = ThumbStamp {
            size: 48,
            len: 1,
            mtime: None,
        };
        let mut thumbs = ShownThumbs::new();
        for (came, (name, side)) in [("a", 10), ("b", 10), ("c", 10), ("d", 20)]
            .into_iter()
            .enumerate()
        {
            let came = u64::try_from(came).expect("a few");
            thumbs.insert(PathBuf::from(name), (stamp, picture(side), came));
        }
        let names = |thumbs: &ShownThumbs| {
            let mut names: Vec<String> = thumbs
                .keys()
                .map(|path| path.display().to_string())
                .collect();
            names.sort();
            names
        };
        let none = HashSet::new();
        // 400 bytes each for the three first, 1600 for the last: 2800 bytes in all.
        trim_shown(&mut thumbs, 10, 2800, &none);
        assert_eq!(names(&thumbs), ["a", "b", "c", "d"]);
        trim_shown(&mut thumbs, 10, 2400, &none);
        assert_eq!(names(&thumbs), ["b", "c", "d"], "the first that came goes");
        // Those on screen stay, over the budget too: the next one goes in their place.
        let screen = HashSet::from([PathBuf::from("b")]);
        trim_shown(&mut thumbs, 2, 10_000, &screen);
        assert_eq!(names(&thumbs), ["b", "d"]);
        trim_shown(&mut thumbs, 1, 100, &screen);
        assert_eq!(names(&thumbs), ["b", "d"], "nothing left to forget");
        // The last that came stays, however large.
        trim_shown(&mut thumbs, 10, 100, &none);
        assert_eq!(names(&thumbs), ["d"]);
        assert_eq!(image_bytes(&thumbs[&PathBuf::from("d")].1), 1600);
    }

    #[test]
    fn a_list_keeps_the_thumbnails_on_screen_past_its_budget() {
        let model = FileRows::default();
        let dir = std::env::temp_dir().join("rf-thumb-screen");
        let list: Vec<Entry> = (0..4)
            .map(|i| entry(&format!("p{i}.png"), EntryKind::File, 1))
            .collect();
        model.set_folder(Some(&dir));
        model.sync(&list, &Selection::default());
        model.set_on_screen(0, 1);
        assert_eq!(
            model.on_screen_paths(),
            HashSet::from([dir.join("p0.png"), dir.join("p1.png")])
        );
        model.set_on_screen(3, 9);
        assert_eq!(model.on_screen_paths(), HashSet::from([dir.join("p3.png")]));
        model.set_on_screen(1, 0);
        assert!(model.on_screen_paths().is_empty());
        model.set_folder(None);
        model.set_on_screen(0, 3);
        assert!(
            model.on_screen_paths().is_empty(),
            "no folder, no thumbnails"
        );
    }

    #[test]
    fn file_rows_show_the_thumbnails_of_their_folder() {
        let model = FileRows::default();
        let dir = std::env::temp_dir().join("rf-thumb-rows");
        let list = vec![
            entry("a.png", EntryKind::File, 1),
            entry("b.png", EntryKind::File, 1),
        ];
        model.set_folder(Some(&dir));
        model.sync(&list, &Selection::default());
        let widths = |model: &FileRows| -> Vec<u32> {
            (0..model.row_count())
                .filter_map(|i| model.row_data(i).map(|row| row.thumb.size().width))
                .collect()
        };
        assert_eq!(
            widths(&model),
            [0, 0],
            "each kind's icon until a thumbnail comes"
        );
        let picture = slint::Image::from_rgba8(SharedPixelBuffer::new(6, 3));
        let stamp = |size| ThumbStamp {
            size,
            len: 1,
            mtime: None,
        };
        // A wrong hint finds the row all the same.
        model.set_thumb(Some(0), &dir.join("b.png"), stamp(48), picture.clone());
        assert_eq!(widths(&model), [0, 6]);
        assert!(model.has_thumb(&dir.join("b.png"), stamp(48)));
        assert!(
            !model.has_thumb(&dir.join("b.png"), stamp(96)),
            "another size is asked for again"
        );
        // One of another folder is not this list's.
        model.set_thumb(
            None,
            &std::env::temp_dir().join("a.png"),
            stamp(48),
            picture.clone(),
        );
        assert_eq!(widths(&model), [0, 6]);
        // Another folder with the same names shows none of these.
        model.set_folder(Some(&dir.join("other")));
        assert_eq!(widths(&model), [0, 0]);
        model.set_folder(Some(&dir));
        model.set_thumb(None, &dir.join("a.png"), stamp(48), picture);
        assert_eq!(widths(&model), [6, 0]);
        model.clear_thumbs();
        assert_eq!(widths(&model), [0, 0]);
    }

    #[test]
    fn a_file_that_changed_loses_its_thumbnail_and_is_asked_for_again() {
        let model = FileRows::default();
        let dir = std::env::temp_dir().join("rf-thumb-changed");
        let path = dir.join("a.png");
        let mut file = entry("a.png", EntryKind::File, 10);
        file.modified = Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        model.set_folder(Some(&dir));
        model.sync(std::slice::from_ref(&file), &Selection::default());
        let width = |model: &FileRows| model.row_data(0).map(|row| row.thumb.size().width);
        let old = ThumbStamp {
            size: 48,
            len: file.size,
            mtime: file.modified,
        };
        let picture = slint::Image::from_rgba8(SharedPixelBuffer::new(6, 3));
        model.set_thumb(None, &path, old, picture.clone());
        assert!(model.has_thumb(&path, old));
        assert_eq!(width(&model), Some(6));

        // Overwritten: the same name, a later time. The old picture is no longer its own.
        file.modified = file.modified.map(|time| time + Duration::from_secs(5));
        model.sync(std::slice::from_ref(&file), &Selection::default());
        let new = ThumbStamp {
            mtime: file.modified,
            ..old
        };
        assert!(!model.has_thumb(&path, new), "asked for again");
        assert_eq!(width(&model), Some(0), "its kind's icon until then");
        // The old picture arriving late is dropped; the new one shows.
        model.set_thumb(None, &path, old, picture.clone());
        assert!(!model.has_thumb(&path, new));
        assert_eq!(width(&model), Some(0));
        model.set_thumb(None, &path, new, picture);
        assert!(model.has_thumb(&path, new));
        assert_eq!(width(&model), Some(6));
    }

    #[test]
    fn file_rows_show_dates_in_the_time_zone_given() {
        let model = FileRows::default();
        let mut dated = entry("a.txt", EntryKind::File, 1);
        dated.modified = Some(UNIX_EPOCH + Duration::from_mins(29_356_280));
        model.sync(&[dated], &Selection::default());
        let modified = || model.row_data(0).map(|row| row.modified);
        assert_eq!(modified(), Some("2025-10-25 07:20".into()));
        model.set_time_zone(TimeZone::fixed(jiff::tz::offset(9)));
        assert_eq!(modified(), Some("2025-10-25 16:20".into()));
    }

    #[test]
    fn a_folder_that_failed_shows_why_and_no_count() {
        let folder = VPath::local(std::env::temp_dir().join("gone")).expect("a local path");
        let (mut pane, request) = Pane::new(folder, VisibilityFilter::default());
        let models = PaneModels::default();
        assert_eq!(models.view(&pane, true).status, "항목 0개");
        pane.handle(LoadEvent::Failed {
            generation: request.generation,
            kind: ErrorKind::NotFound,
        });
        let view = models.view(&pane, true);
        assert_eq!(view.error, "찾을 수 없습니다");
        assert_eq!(view.status, "");
        assert!(!view.loading);
    }

    #[test]
    fn folder_labels_name_the_last_folder() {
        let root = system_root();
        let label = folder_label(&root);
        assert_ne!(label, "");
        let docs = root.join("docs").expect("a plain name");
        assert_eq!(folder_label(&docs), "docs");
        let shown = crumbs(&[root.clone(), docs]);
        assert_eq!(
            shown,
            [
                Crumb {
                    label: label.into(),
                    index: 0,
                },
                Crumb {
                    label: "docs".into(),
                    index: 1,
                },
            ]
        );
        // A root is named by its path without the last separator.
        assert_eq!(folder_label(&root), if cfg!(windows) { "C:" } else { "/" });
    }

    #[test]
    fn drives_show_how_full_they_are() {
        const GB: u64 = 1 << 30;
        let root = system_root();
        let drive = Place {
            label: system_drive_label(),
            path: root.clone(),
            kind: PlaceKind::Drive,
            space: Some(DiskSpace {
                total: 512 * GB,
                free: 130 * GB,
                available: 128 * GB,
            }),
        };
        let (info, fill) = place_space(&drive);
        assert_eq!(info, "128 GB 남음 / 512 GB");
        assert!((fill - 0.75).abs() < 1e-6, "{fill}");

        let unknown = Place {
            space: None,
            ..drive.clone()
        };
        assert_eq!(place_space(&unknown), (String::new(), 0.0));
        let home = Place {
            label: "홈".to_owned(),
            path: root,
            kind: PlaceKind::Favorite(rf_platform::KnownFolder::Home),
            space: Some(DiskSpace {
                total: GB,
                free: GB,
                available: GB,
            }),
        };
        assert_eq!(place_space(&home), (String::new(), 0.0));
    }

    /// The places of `tree_rows_show_places_heads_and_chevrons`: two favorites, then two
    /// drives.
    fn places() -> Vec<Place> {
        let root = system_root();
        let place = |label: &str, kind| Place {
            label: label.to_owned(),
            path: root.clone(),
            kind,
            space: None,
        };
        let home = PlaceKind::Favorite(rf_platform::KnownFolder::Home);
        vec![
            place("홈", home),
            place("문서", home),
            place("시스템 디스크", PlaceKind::Drive),
            place("USB", PlaceKind::Drive),
        ]
    }

    fn data(id: NodeId, label: &str, depth: usize, root: Option<usize>) -> TreeRowData {
        TreeRowData {
            id,
            label: label.to_owned(),
            depth,
            expanded: false,
            has_children_unknown: true,
            has_children: false,
            selected: false,
            hidden: false,
            root,
        }
    }

    #[test]
    fn tree_rows_show_places_heads_and_chevrons() {
        let rows = vec![
            TreeRowData {
                expanded: true,
                has_children_unknown: false,
                has_children: true,
                ..data(0, "홈", 0, Some(0))
            },
            TreeRowData {
                selected: true,
                ..data(4, "프로젝트", 1, None)
            },
            // Listed, and nothing below: no chevron, so no "open" either.
            TreeRowData {
                expanded: true,
                has_children_unknown: false,
                ..data(5, "빈 폴더", 1, None)
            },
            data(1, "문서", 0, Some(1)),
            data(2, "시스템 디스크", 0, Some(2)),
            data(3, "USB", 0, Some(3)),
        ];
        let mut rows = rows;
        rows[2].hidden = true;
        let shown = tree_rows(&rows, &places());
        // A hidden folder shows at half strength, as in the list.
        let hidden: Vec<bool> = shown.iter().map(|(_, row)| row.hidden).collect();
        assert_eq!(hidden, [false, false, true, false, false, false]);
        let ids: Vec<NodeId> = shown.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [0, 4, 5, 1, 2, 3]);
        let heads: Vec<SectionHead> = shown.iter().map(|(_, row)| row.head).collect();
        assert_eq!(
            heads,
            [
                SectionHead::Favorites,
                SectionHead::None,
                SectionHead::None,
                SectionHead::None,
                SectionHead::Drives,
                SectionHead::None,
            ]
        );
        let home = &shown[0].1;
        assert!(home.is_place && !home.is_drive && home.expandable && home.expanded);
        let project = &shown[1].1;
        assert_eq!((project.label.as_str(), project.depth), ("프로젝트", 1));
        assert!(project.selected && !project.is_place && project.expandable);
        let empty = &shown[2].1;
        assert!(!empty.expandable && !empty.expanded);
        assert!(shown[4].1.is_drive && shown[4].1.is_place);
    }

    #[test]
    fn tree_rows_follow_the_tree() {
        let home = VPath::local(std::env::temp_dir().join("home")).expect("a local path");
        let mut tree = Tree::new(VisibilityFilter::default());
        let place = Place {
            label: "홈".to_owned(),
            path: home,
            kind: PlaceKind::Favorite(rf_platform::KnownFolder::Home),
            space: None,
        };
        tree.set_roots([(place.label.clone(), place.path.clone())]);
        let model = TreeRows::default();
        let labels = |model: &TreeRows| -> Vec<String> {
            (0..model.row_count())
                .filter_map(|i| model.row_data(i))
                .map(|row| row.label.to_string())
                .collect()
        };
        model.sync(&tree, std::slice::from_ref(&place));
        assert_eq!(labels(&model), ["홈"]);
        let request = tree.toggle(0).expect("listed");
        tree.apply_children(
            request.generation,
            vec![
                Entry::new("a", EntryKind::Dir),
                Entry::new("b", EntryKind::Dir),
            ],
        );
        model.sync(&tree, std::slice::from_ref(&place));
        assert_eq!(labels(&model), ["홈", "a", "b"]);
        assert_eq!(model.row_data(1).map(|row| row.depth), Some(1));
        tree.toggle(0);
        model.sync(&tree, std::slice::from_ref(&place));
        assert_eq!(labels(&model), ["홈"]);
        assert_eq!(model.row_data(3), None);
    }

    #[test]
    fn the_failures_window_measures_its_first_lines() {
        let failures = |count: usize| Failures {
            title: "처리하지 못한 항목".to_owned(),
            lines: (0..count)
                .map(|i| format!("/a/{i}.txt: 권한이 없습니다"))
                .collect(),
            retry: None,
        };
        let shown = |model: &ModelRc<SharedString>| -> Vec<String> {
            model.iter().map(|line| line.to_string()).collect()
        };
        let many = failures(MEASURED_LINES + 5);
        let view = message_view(&many);
        assert_eq!(shown(&view.lines), many.lines);
        assert_eq!(shown(&view.head), many.lines[..MEASURED_LINES]);
        let few = failures(2);
        assert_eq!(shown(&message_view(&few).head), few.lines);
        // 다시 시도 shows only for a window that has something to try again.
        assert!(!view.retry);
        let retry = crate::dialogs::Retry {
            job: rf_jobs::JobId(1),
            spec: rf_jobs::JobSpec::Rename {
                from: system_root(),
                to: system_root(),
            },
        };
        let again = Failures {
            retry: Some(retry),
            ..few
        };
        assert!(message_view(&again).retry);
    }

    /// A conflict question of a copy between two files of 3 bytes, the one that would come a
    /// minute newer.
    fn conflict_question() -> ConflictQuestion {
        let at = UNIX_EPOCH + Duration::from_hours(500_000);
        let file = |modified| {
            let mut entry = Entry::new("a.txt", EntryKind::File);
            entry.size = 3;
            entry.modified = Some(modified);
            entry
        };
        let path = |folder: &str| {
            system_root()
                .join(folder)
                .and_then(|folder| folder.join("a.txt"))
                .expect("plain names")
        };
        ConflictQuestion {
            id: 7,
            job: rf_jobs::JobId(1),
            conflict: Box::new(rf_jobs::Conflict {
                source: path("src"),
                target: path("dst"),
                incoming: file(at + Duration::from_secs(60)),
                existing: file(at),
            }),
            items_left: 0,
            work: words::JobWork::Copy,
        }
    }

    #[test]
    fn the_conflict_window_says_what_each_conditional_choice_does_to_this_file() {
        let mut question = conflict_question();
        let view = conflict_view(&question, &TimeZone::UTC);
        // Newer, and as large: 더 새 파일만 replaces it, 크기가 다를 때만 leaves it.
        assert!(view.can_overwrite && view.incoming_newer);
        assert_eq!(
            (view.newer_overwrites, view.newer_hint.as_str()),
            (true, "복사해 올 파일이 더 최근이라 이 파일은 덮어씁니다")
        );
        assert_eq!(
            (view.size_overwrites, view.size_hint.as_str()),
            (false, "크기가 같아 이 파일은 건너뜁니다")
        );
        assert_eq!(
            view.warning_rule_all,
            "남은 충돌도 같은 조건으로 덮어씁니다. 덮어쓴 파일의 내용은 되살릴 수 없습니다."
        );
        // Older, and of another size: the other way round.
        question.conflict.incoming.modified = question
            .conflict
            .existing
            .modified
            .map(|at| at - Duration::from_secs(60));
        question.conflict.incoming.size = 4;
        question.work = words::JobWork::Move;
        let view = conflict_view(&question, &TimeZone::UTC);
        assert_eq!(
            (view.newer_overwrites, view.newer_hint.as_str()),
            (
                false,
                "옮겨 올 파일이 더 최근이 아니라 이 파일은 건너뜁니다"
            )
        );
        assert_eq!(
            (view.size_overwrites, view.size_hint.as_str()),
            (true, "크기가 달라 이 파일은 덮어씁니다")
        );
        // A file and a folder: no overwrite, by a condition either, and nothing to say of one.
        question.conflict.existing.kind = EntryKind::Dir;
        let view = conflict_view(&question, &TimeZone::UTC);
        assert!(!view.can_overwrite && !view.newer_overwrites && !view.size_overwrites);
        assert_eq!(
            (view.newer_hint.as_str(), view.size_hint.as_str()),
            ("", "")
        );
    }

    #[test]
    fn the_windows_choice_numbers_are_the_choices_of_the_state() {
        // The rows of the conflict window, in their order.
        let question = conflict_question();
        let actions: Vec<rf_jobs::ConflictAction> = (0..5)
            .map(|number| {
                let answer = question.answer(conflict_choice(number), true);
                answer.expect("two files offer every choice").action
            })
            .collect();
        assert_eq!(
            actions,
            [
                rf_jobs::ConflictAction::Overwrite,
                rf_jobs::ConflictAction::Skip,
                rf_jobs::ConflictAction::Rename,
                rf_jobs::ConflictAction::OverwriteIfNewer,
                rf_jobs::ConflictAction::OverwriteIfSizeDiffers,
            ]
        );
        // A number the window does not send destroys nothing.
        assert_eq!(conflict_choice(-1), ConflictChoice::Rename);
        assert_eq!(conflict_choice(5), ConflictChoice::Rename);
        // The buttons of the error window.
        assert_eq!(
            [error_choice(0), error_choice(1), error_choice(2)],
            [ErrorChoice::Retry, ErrorChoice::Skip, ErrorChoice::Abort]
        );
        assert_eq!(error_choice(9), ErrorChoice::Skip);
    }

    #[test]
    fn the_error_window_names_the_item_and_why_it_failed() {
        let source = system_root().join("a.txt").expect("a plain name");
        let question = ErrorQuestion {
            id: 3,
            job: rf_jobs::JobId(1),
            failure: Box::new(rf_jobs::ItemFailure {
                source: Some(source.clone()),
                target: None,
                stage: rf_jobs::Stage::Write,
                kind: ErrorKind::PermissionDenied,
                message: String::new(),
                reason: None,
            }),
            items_left: 4,
            work: words::JobWork::Move,
        };
        let view = error_view(&question);
        assert_eq!(view.serial, 3);
        assert_eq!(view.title, "이동 중 오류가 났습니다");
        assert_eq!(view.item.as_str(), source.to_string());
        assert_eq!(view.reason, "권한이 없습니다");
        assert_eq!(view.items_left, "이 작업에 남은 항목 4개");
        assert_eq!(
            view.retry_note,
            "다시 시도는 한 번에 한 항목만 할 수 있습니다"
        );
        // The last item of a job has none left to tell of.
        let last = ErrorQuestion {
            items_left: 0,
            ..question
        };
        assert_eq!(error_view(&last).items_left, "");
    }

    #[test]
    fn sort_columns_round_trip() {
        for key in [
            SortKey::Name,
            SortKey::Size,
            SortKey::Modified,
            SortKey::Kind,
            SortKey::Created,
            SortKey::Extension,
            SortKey::Accessed,
            SortKey::Origin,
            SortKey::Deleted,
        ] {
            assert_eq!(sort_key(sort_column(key)), Some(key));
        }
        // The first demo's numbers stay.
        assert_eq!(sort_column(SortKey::Kind), 3);
        // The attributes and the path sort nothing.
        assert_eq!(sort_key(6), None);
        assert_eq!(sort_key(7), Some(SortKey::Accessed));
        assert_eq!(sort_key(8), None);
        // The trash's: where an item was, and when it was trashed.
        assert_eq!(sort_key(9), Some(SortKey::Origin));
        assert_eq!(sort_key(10), Some(SortKey::Deleted));
        assert_eq!(sort_key(11), None);
        assert_eq!(sort_key(-1), None);
    }

    #[test]
    fn set_rows_keeps_equal_rows() {
        let model = VecModel::from(vec![1, 2, 3]);
        set_rows(&model, vec![1, 5, 3]);
        assert_eq!(model.iter().collect::<Vec<_>>(), [1, 5, 3]);
        set_rows(&model, vec![7]);
        assert_eq!(model.iter().collect::<Vec<_>>(), [7]);
    }

    /// A picker of a move of `a/docs` and `a/x.txt` into the other pane's `b`, and its tree,
    /// whose place `a` is listed: `docs`, `docs 보관`.
    fn picker_over_a() -> (Picker, Tree, Vec<Place>) {
        let temp = |name: &str| VPath::local(std::env::temp_dir().join(name)).expect("local");
        let a = temp("a");
        let pick = PickRequest {
            mode: TransferMode::Move,
            sources: vec![
                a.join("docs").expect("a name"),
                a.join("x.txt").expect("a name"),
            ],
            file_bytes: 10,
            folder_count: 1,
            other_count: 0,
            from_dir: a.clone(),
        };
        let groups = crate::pick::quick_targets(&pick, &temp("b"), &[], &[], &[]);
        let place = Place {
            label: "작업".to_owned(),
            path: a,
            kind: PlaceKind::Favorite(rf_platform::KnownFolder::Documents),
            space: None,
        };
        let mut tree = Tree::new(VisibilityFilter::default());
        tree.set_roots([(place.label.clone(), place.path.clone())]);
        let listing = tree.toggle(0).expect("listed");
        tree.apply_children(
            listing.generation,
            vec![
                Entry::new("docs", EntryKind::Dir),
                Entry::new("docs 보관", EntryKind::Dir),
            ],
        );
        (Picker::new(pick, 0, groups), tree, vec![place])
    }

    #[test]
    fn the_pickers_tree_dims_what_cannot_take_the_items() {
        let (picker, tree, places) = picker_over_a();
        let shown = |rows: Vec<(NodeId, TreeRow)>| -> Vec<(String, bool, String)> {
            rows.into_iter()
                .map(|(_, row)| (row.label.to_string(), row.dimmed, row.info.to_string()))
                .collect()
        };
        let row =
            |label: &str, dimmed: bool, info: &str| (label.to_owned(), dimmed, info.to_owned());
        // The items' own folder and an item show dimmed, with why, as among the quick
        // targets; the sidebar's rows never are.
        assert_eq!(
            shown(pick_tree_rows(&picker, &tree, None, &places)),
            [
                row("작업", true, "원래 있던 폴더라 고를 수 없음"),
                row("docs", true, "옮기는 항목 안이라 고를 수 없음"),
                row("docs 보관", false, ""),
            ]
        );
        assert!(
            tree_rows(&tree.rows(), &places)
                .iter()
                .all(|(_, row)| !row.dimmed)
        );
        // So do the folders a filter finds; the others say where they are.
        let found = tree.find("docs");
        let rows = shown(pick_tree_rows(&picker, &tree, Some(&found), &places));
        assert_eq!(
            rows[0],
            row("docs", true, "옮기는 항목 안이라 고를 수 없음")
        );
        assert_eq!((rows[1].0.as_str(), rows[1].1), ("docs 보관", false));
        assert!(rows[1].2.ends_with('a'), "{rows:?}");
    }

    #[test]
    fn the_pickers_tree_rows_are_made_again_only_when_they_change() {
        let (mut picker, mut tree, places) = picker_over_a();
        let models = PickModels::default();
        let first_info = |view: &PickView| {
            view.tree
                .row_data(0)
                .map(|row| row.info.to_string())
                .unwrap_or_default()
        };
        let view = models.view(&picker, &tree, None, &places);
        assert_eq!(first_info(&view), "원래 있던 폴더라 고를 수 없음");
        // A mark on the rows shown stays while nothing they are made of changes: a keystroke in
        // a field redraws the picker without making its rows again.
        let mark = || models.tree.rows.borrow_mut()[0].1.info = "mark".into();
        mark();
        picker.edit_path("/somewhere");
        picker.set_filter("do");
        assert_eq!(
            first_info(&models.view(&picker, &tree, None, &places)),
            "mark"
        );

        // They are made again for another mode (a copy may go to the items' own folder)...
        picker.set_mode(TransferMode::Copy);
        assert_eq!(first_info(&models.view(&picker, &tree, None, &places)), "");
        // ...for what a filter found...
        mark();
        let found = tree.find("docs");
        let view = models.view(&picker, &tree, Some(&found), &places);
        assert_eq!(
            view.tree.row_data(0).map(|row| row.label.to_string()),
            Some("docs".to_owned())
        );
        mark();
        assert_eq!(
            first_info(&models.view(&picker, &tree, Some(&found), &places)),
            "mark"
        );
        assert_eq!(first_info(&models.view(&picker, &tree, None, &places)), "");
        // ...and for a change of the tree.
        mark();
        tree.toggle(0);
        let view = models.view(&picker, &tree, None, &places);
        assert_eq!((view.tree.row_count(), first_info(&view).as_str()), (1, ""));
    }

    #[test]
    fn the_pickers_footer_says_how_it_confirms() {
        let (mut picker, tree, places) = picker_over_a();
        let models = PickModels::default();
        let view = |picker: &Picker| models.view(picker, &tree, None, &places);
        // The target is being looked at; nothing to beware of in how it confirms.
        let first = view(&picker);
        assert_eq!(first.space, "남은 공간을 확인하는 중…");
        assert_eq!((first.hint.as_str(), first.problem.as_str()), ("", ""));
        assert!(first.can_confirm && !first.path_edited);
        // What the footer says while a path is composed, which Rust does not see.
        assert_eq!(first.unentered_hint, "Enter로 경로를 먼저 확인하세요");
        let b = picker.target.clone().expect("the other pane's folder");
        let look = picker.choose(b.clone());
        picker.looked(
            look,
            &b,
            crate::pick::TargetFacts {
                folder: Ok(()),
                writable: true,
                available: Some(1 << 30),
                same_volume: false,
            },
        );
        assert_eq!(view(&picker).space, "남은 공간 1 GB");

        // An overwrite says that only the button confirms it, with its warning.
        picker.policy = rf_jobs::ConflictPolicy::Overwrite;
        let overwrite = view(&picker);
        assert_eq!(overwrite.hint, "덮어쓰기는 버튼으로만 확인합니다");
        assert!(overwrite.policy_danger && overwrite.can_confirm);
        // So do the two conditional overwrites, each with its own note, at the places of their
        // buttons (4 and 5).
        for (policy, at, note) in [
            (
                rf_jobs::ConflictPolicy::OverwriteIfNewer,
                4,
                "수정한 날짜가 더 최근일 때만 같은 이름의 파일을 바꾸고, 아니면 건너뜁니다.\n\
                 바꾼 파일의 내용은 되살릴 수 없습니다.",
            ),
            (
                rf_jobs::ConflictPolicy::OverwriteIfSizeDiffers,
                5,
                "크기가 다를 때만 같은 이름의 파일을 바꾸고, 같으면 건너뜁니다.\n\
                 바꾼 파일의 내용은 되살릴 수 없습니다.",
            ),
        ] {
            picker.policy = policy;
            let conditional = view(&picker);
            assert_eq!(conditional.policy, at);
            assert_eq!(conditional.policy_note, note);
            assert_eq!(conditional.hint, "덮어쓰기는 버튼으로만 확인합니다");
            assert!(conditional.policy_danger && conditional.can_confirm);
        }
        // The box of the links shows for a copy alone, off until it is flipped.
        picker.policy = rf_jobs::ConflictPolicy::Ask;
        let moving = view(&picker);
        assert!(!moving.can_follow_links && !moving.follow_links);
        picker.set_mode(TransferMode::Copy);
        let copying = view(&picker);
        assert!(copying.can_follow_links && !copying.follow_links);
        picker.follow_links = true;
        assert!(view(&picker).follow_links);
        picker.set_mode(TransferMode::Move);
        assert!(!view(&picker).can_follow_links);
        picker.follow_links = false;

        // A path typed and not entered: the button is off, and the footer says what to do,
        // with nothing about the target the field no longer shows.
        let somewhere = system_root()
            .join("somewhere")
            .and_then(|path| path.join("else"))
            .expect("plain names");
        picker.edit_path(&somewhere.to_string());
        let edited = view(&picker);
        assert_eq!(edited.hint, "Enter로 경로를 먼저 확인하세요");
        assert!(edited.path_edited && !edited.can_confirm && !edited.can_make_folder);
        assert_eq!((edited.space.as_str(), edited.problem.as_str()), ("", ""));
        // A field cleared gives Enter nothing to look at: the footer asks for a path or a
        // folder instead.
        picker.edit_path(" ");
        let cleared = view(&picker);
        assert_eq!(cleared.hint, "경로를 입력하거나 폴더를 고르세요");
        assert!(cleared.path_edited && !cleared.can_confirm);
        picker.edit_path(&somewhere.to_string());
        picker.type_path(somewhere);
        let looking = view(&picker);
        assert_eq!(looking.hint, "입력한 경로를 확인하는 중…");
        assert!(!looking.can_confirm);
    }

    #[test]
    fn a_drags_payload_carries_its_id_and_nothing_else() {
        let payload = drag_payload(7);
        assert_eq!(drag_id(&payload), 7);
        // A drag of rows is no drag of a tab, and the other way round.
        assert_eq!(tab_id(&payload), None);
        let tab = tab_payload(9);
        assert_eq!(tab_id(&tab), Some(9));
        assert_eq!(drag_id(&tab), 0);
        assert!(!tab.is_empty());
        // Nothing for the system's drag and drop: no text, no paths.
        assert!(!payload.has_plain_text() && !payload.has_file_paths() && !payload.is_empty());
        // A payload from elsewhere (text another drag carries) is no drag of this app's.
        assert_eq!(drag_id(&DataTransfer::from(SharedString::from("/etc"))), 0);
        assert_eq!(drag_id(&DataTransfer::default()), 0);
    }

    #[test]
    fn the_pointer_shows_what_a_drop_does() {
        use crate::state::drop_rule::Refusal;

        assert_eq!(
            drop_action(DropChoice::Do(DropEffect::Copy)),
            DragAction::Copy
        );
        assert_eq!(
            drop_action(DropChoice::Do(DropEffect::Move)),
            DragAction::Move
        );
        // The menu of 놓을 때 묻기 takes the drop as a move would, and asks.
        assert_eq!(drop_action(DropChoice::Ask), DragAction::Move);
        assert_eq!(
            drop_action(DropChoice::Do(DropEffect::Link)),
            DragAction::Link
        );
        let refused = DropChoice::Refuse(Refusal::NotAllowed);
        assert_eq!(drop_action(refused), DragAction::None, "{refused:?}");
    }

    #[test]
    fn the_keys_winit_tells_are_those_of_a_drop() {
        assert_eq!(drag_mods(ModifiersState::empty()), DragMods::default());
        assert_eq!(
            drag_mods(ModifiersState::CONTROL | ModifiersState::SHIFT),
            DragMods {
                ctrl: true,
                shift: true,
                ..DragMods::default()
            }
        );
        // ⌥⌘ on macOS, Alt+Win on Windows.
        assert_eq!(
            drag_mods(ModifiersState::ALT | ModifiersState::SUPER),
            DragMods {
                alt: true,
                logo: true,
                ..DragMods::default()
            }
        );
    }

    #[test]
    fn a_pointer_past_an_edge_of_the_window_is_outside_it() {
        let size = PhysicalSize::new(900, 560);
        let outside = |x, y| outside_window(PhysicalPosition::new(x, y), size);
        assert!(!outside(0.0, 0.0));
        assert!(!outside(899.5, 559.5));
        assert!(!outside(450.0, 280.0));
        // A drag holds the pointer, so the window hears of it beyond its edges.
        for (x, y) in [
            (-1.0, 10.0),
            (10.0, -0.5),
            (900.0, 10.0),
            (10.0, 560.0),
            (-50.0, 900.0),
        ] {
            assert!(outside(x, y), "({x}, {y})");
        }
    }

    #[test]
    fn a_place_of_a_drag_is_named_the_same_both_ways() {
        let spots = [
            DropSpot::Pane {
                side: LEFT,
                row: Some(3),
            },
            DropSpot::Pane {
                side: RIGHT,
                row: None,
            },
            DropSpot::Tab {
                side: RIGHT,
                index: 2,
            },
            DropSpot::Tree { tree: LEFT, row: 5 },
            DropSpot::Tree {
                tree: RIGHT,
                row: 0,
            },
        ];
        for spot in spots {
            let at = drop_at(Some(spot));
            assert!(at.found, "{spot:?}");
            assert_eq!(spot_of(at.place, at.side, at.index), Some(spot));
        }
        assert!(!drop_at(None).found);
        // Below the rows is a place of the list; a tab or a row of the tree is not.
        assert_eq!(
            spot_of(DropPlace::List, 0, -1),
            Some(DropSpot::Pane {
                side: LEFT,
                row: None
            })
        );
        assert_eq!(spot_of(DropPlace::Tab, 1, -1), None);
        assert_eq!(spot_of(DropPlace::Tree, 0, -1), None);
    }

    #[test]
    fn the_drag_defaults_have_the_numbers_of_their_menu_items() {
        for default in [
            DragDefault::Move,
            DragDefault::Copy,
            DragDefault::System,
            DragDefault::Ask,
        ] {
            assert_eq!(drag_default_of(drag_default_number(default)), Some(default));
        }
        assert_eq!(drag_default_number(DragDefault::Move), 0);
        assert_eq!(drag_default_number(DragDefault::Ask), 3);
        assert_eq!(drag_default_of(-1), None);
        assert_eq!(drag_default_of(4), None);
    }

    #[test]
    fn the_completion_list_is_pending_while_a_request_for_its_text_is_out() {
        use crate::loader::{FolderName, LoadEvent};
        use crate::state::{AppState, Effect, LEFT};

        let at = |name: &str| VPath::local(std::env::temp_dir().join(name)).expect("a local path");
        let (mut state, _) = AppState::new(at("a"), at("b"), None);
        let models = PaneModels::default();
        let typed = format!("{}{}d", at("a"), std::path::MAIN_SEPARATOR);
        let request = |effects: &[Effect]| {
            effects
                .iter()
                .find_map(|effect| match effect {
                    Effect::ListNames(request) => Some(request.generation),
                    _ => None,
                })
                .expect("a listing of names")
        };
        // Whether the list shows, and whether a Tab waits.
        let shown = |state: &AppState| {
            let mut view = models.view(&state.panes()[LEFT], true);
            models.show_address(&mut view, state.completion(LEFT), None);
            (view.completion.open, view.completion.pending)
        };
        let first = request(&state.path_edited(LEFT, &typed));
        assert_eq!(shown(&state), (false, true));
        state.loaded(LoadEvent::Names {
            generation: first,
            names: vec![FolderName::folder("docs")],
            more: 3,
            links_left: false,
        });
        assert_eq!(shown(&state), (true, false));
        // A longer text: the rows that still fit show while the request for it is out.
        let _ = request(&state.path_edited(LEFT, &format!("{typed}o")));
        assert_eq!(shown(&state), (true, true));
        // None fits: the list hides, and a Tab still waits.
        let _ = request(&state.path_edited(LEFT, &format!("{typed}z")));
        assert_eq!(shown(&state), (false, true));
    }

    #[test]
    fn the_visit_history_s_menu_lists_each_way_and_both_around_the_current_folder() {
        use crate::history::Way;
        use crate::state::AppState;

        let at = |name: &str| VPath::local(std::env::temp_dir().join(name)).expect("a local path");
        let (mut state, _) = AppState::new(at("a"), at("b"), None);
        // a → c → d → e, then two places back to c: a is behind, d and e ahead.
        for folder in ["c", "d", "e"] {
            state.go_to(at(folder));
        }
        state.history_chosen(Way::Back, 1, &at("c"));
        let parent = at("a").parent().expect("a folder it is in");
        let place = short_path(&parent.to_string(), VISIT_PLACE_CHARS);
        let title = |name: &str| format!("{name} — {place}");
        let lines = |state: &AppState, way| -> Vec<(String, i32, i32)> {
            visit_rows(state, way)
                .into_iter()
                .map(|(line, path)| {
                    // Each line goes with the folder it names.
                    assert_eq!(line.title.as_str(), visit_title(&path));
                    (line.title.to_string(), line.way, line.index)
                })
                .collect()
        };
        assert_eq!(lines(&state, Some(Way::Back)), [(title("a"), 0, 0)]);
        assert_eq!(
            lines(&state, Some(Way::Forward)),
            [(title("d"), 1, 0), (title("e"), 1, 1)]
        );
        // From the key: ahead (the farthest first), the current folder, behind (the nearest
        // first).
        assert_eq!(
            lines(&state, None),
            [
                (title("e"), 1, 1),
                (title("d"), 1, 0),
                (title("c"), -1, 0),
                (title("a"), 0, 0),
            ]
        );
        assert_eq!(way_of(0), Some(Way::Back));
        assert_eq!(way_of(1), Some(Way::Forward));
        assert_eq!(way_of(-1), None);
        // A root is in no folder: its name alone.
        assert_eq!(words::history_entry("C:", None), "C:");
    }

    #[test]
    fn the_key_s_history_menu_lists_no_more_lines_than_a_menu_of_one_way() {
        use crate::history::{HISTORY_KEY_MENU, HISTORY_MENU, Way};
        use crate::state::AppState;

        let at = |name: &str| VPath::local(std::env::temp_dir().join(name)).expect("a local path");
        let (mut state, _) = AppState::new(at("start"), at("b"), None);
        // 30 folders on, then 12 back: 12 ahead and 18 behind, at f17.
        for i in 0..30 {
            state.go_to(at(&format!("f{i:02}")));
        }
        let back = state.history_entries(Way::Back);
        state.history_chosen(Way::Back, 11, &back[11]);
        let rows = visit_rows(&state, None);
        assert_eq!(rows.len(), 2 * HISTORY_KEY_MENU + 1);
        assert!(rows.len() <= HISTORY_MENU);
        let ways: Vec<(i32, i32)> = rows
            .iter()
            .map(|(line, _)| (line.way, line.index))
            .collect();
        let mut wanted: Vec<(i32, i32)> = (0..7).rev().map(|index| (1, index)).collect();
        wanted.push((-1, 0));
        wanted.extend((0..7).map(|index| (0, index)));
        assert_eq!(ways, wanted);
        // The nearest ones: f17 is the current folder, f18 ahead and f16 behind next to it,
        // f24 and f10 at the ends.
        assert_eq!(rows[7].1, at("f17"));
        assert_eq!(rows[6].1, at("f18"));
        assert_eq!(rows[8].1, at("f16"));
        assert_eq!(rows[0].1, at("f24"));
        assert_eq!(rows[14].1, at("f10"));
        // One way lists its fifteen at most.
        assert_eq!(visit_rows(&state, Some(Way::Back)).len(), HISTORY_MENU);
        assert_eq!(visit_rows(&state, Some(Way::Forward)).len(), 12);
    }

    #[test]
    fn a_list_of_subfolders_says_when_links_it_did_not_follow_may_lead_to_more() {
        use crate::loader::FolderName;
        let model = Rc::new(VecModel::default());
        let listed = |names: &[&str], more: usize, links_left: bool| Names::Listed {
            names: names.iter().map(|name| FolderName::folder(*name)).collect(),
            more,
            links_left,
        };
        let note = |names: &Names, notes: bool| {
            folder_menu(&model, Some((names, None)), notes)
                .note
                .to_string()
        };
        let unexamined = "바로 가기 일부는 확인하지 않았습니다";
        // Only links left unexamined cut the list short.
        assert_eq!(note(&listed(&["docs"], 0, true), true), unexamined);
        // Not even one folder: there may be some among those links.
        assert_eq!(note(&listed(&[], 0, true), true), unexamined);
        assert_eq!(note(&listed(&[], 0, false), true), "하위 폴더가 없습니다");
        // Folders left out say how many; the whole list says nothing.
        assert_eq!(note(&listed(&["docs"], 4, true), true), "외 4개");
        assert_eq!(note(&listed(&["docs"], 0, false), true), "");
        // The completion list has no such note.
        assert_eq!(note(&listed(&["docs"], 0, true), false), "");
    }
}
