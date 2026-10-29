//! What the Slint screens are given: the row models of a file list and of the folder trees,
//! the conversions from the panes' state, the address bar's lists, the dialogs and the
//! destination picker to Slint structs, the payload of a drag of rows, the keys the toolbar and
//! the menus show, and [`post`], which brings a worker thread's result back to the UI thread.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::rc::Rc;

use jiff::tz::TimeZone;
use rf_core::{Attributes, Entry, EntryKind, Os, VPath};
use rf_jobs::TransferMode;
use rf_vfs::{SortKey, SortOrder};
use slint::language::DragAction;
use slint::{
    ComponentHandle, DataTransfer, Model, ModelNotify, ModelRc, ModelTracker, SharedString,
    VecModel,
};

use crate::controller::{self, Controller};
use crate::dialogs::{
    Confirm, ConflictQuestion, Failures, Modal, NameCheck, NameDialog, PreflightQuestion, Recovery,
};
use crate::format::{
    attribute_letters, file_icon, folder_label, format_size, format_time_in, kind_label, short_path,
};
use crate::jobs::JobRow;
use crate::keys::{self, Command};
use crate::pane::{LoadStatus, Pane, ancestors};
use crate::pick::{POLICIES, PickRequest, Picker, validate};
use crate::places::{Place, PlaceKind};
use crate::selection::Selection;
use crate::state::{Column, Completion, Names};
use crate::tree::{NodeId, Tree, TreeRowData};
use crate::words;
use crate::{
    ColumnView, ConfirmView, ConflictView, Crumb, DialogKind, FileRow, FolderMenuRow,
    FolderMenuView, JobRowView, MainWindow, MessageView, NameView, PaneView, PickTarget, PickView,
    PreflightView, RecoveryView, SectionHead, Shortcuts, TabItem, TreeRow,
};

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

    /// The texts of `column` in `count` rows of the list from row `first` on, one a line, as
    /// the list shows them: what a double click on the column's border fits it to.
    pub fn column_texts(&self, column: Column, first: usize, count: usize) -> String {
        self.rows.texts(column, first, count)
    }

    /// What `pane` shows; `active` when it has the keyboard. The models are brought up to
    /// date and handed over as they are.
    pub fn view(&self, pane: &Pane, active: bool) -> PaneView {
        let tab = pane.active_tab();
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
                .chain(Column::OPTIONAL)
                .map(|column| (column.number(), view.shows(column)))
                .fold(vec![false; 7], |mut shows, (number, shown)| {
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
            path_text: tab.location().to_string().into(),
            rows: ModelRc::from(Rc::clone(&self.rows)),
            cursor: tab
                .selection()
                .cursor()
                .and_then(|row| i32::try_from(row).ok())
                .unwrap_or(-1),
            mode: view.mode.number(),
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
                LoadStatus::Loading | LoadStatus::Ready => tab.status_line().into(),
            },
            loading: tab.status() == LoadStatus::Loading,
            error: match tab.status() {
                LoadStatus::Failed(kind) => words::error_kind(kind).into(),
                LoadStatus::Loading | LoadStatus::Ready => SharedString::new(),
            },
            active,
            can_reopen: pane.can_reopen(),
            completion: FolderMenuView::default(),
            chevron: -1,
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
/// from the click on, and says while the folder is listed, when it could not be, and when it has
/// no folder; the completion list shows only while it has names. Both say how many folders more
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
}

/// One row as [`FileRows`] keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ShownRow {
    entry: Entry,
    selected: bool,
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

    /// Makes the rows `entries`, in that order, selected as `selection` says, and tells the
    /// list what changed: only the selection (a click or a key), a few rows (a change another
    /// app made), or everything (another folder), in the cheapest way that keeps it right.
    pub fn sync(&self, entries: &[Entry], selection: &Selection) {
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
                        (row.selected != selected).then(|| {
                            row.selected = selected;
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
    /// The texts of `column` in `count` rows from row `first` on, one a line
    /// ([`PaneModels::column_texts`]).
    fn texts(&self, column: Column, first: usize, count: usize) -> String {
        let rows = self.rows.borrow();
        let tz = self.tz.borrow();
        let mut texts: Vec<String> = rows
            .iter()
            .skip(first)
            .take(count)
            .map(|shown| cell_text(&file_row(&shown.entry, shown.selected, &tz), column))
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
        Some(file_row(&shown.entry, shown.selected, &self.tz.borrow()))
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
#[derive(Default)]
pub struct TreeRows {
    rows: RefCell<Vec<(NodeId, TreeRow)>>,
    notify: ModelNotify,
    /// The [`Tree::revision`] of the rows shown.
    revision: Cell<Option<u64>>,
}

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
        self.replace(tree_rows(&tree.rows(), places));
    }

    /// Has the next [`TreeRows::sync`] draw its tree again, whatever its revision: the places
    /// changed without a new tree (a drive's free space).
    pub fn invalidate(&self) {
        self.revision.set(None);
    }

    /// Makes the rows `rows`, whatever a tree's revision says (the picker's tree, marked for
    /// its items, or what its filter found), and tells the list what changed.
    pub fn show(&self, rows: Vec<(NodeId, TreeRow)>) {
        // The next `sync` draws its tree again, whatever its revision.
        self.revision.set(None);
        self.replace(rows);
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

/// The sidebar rows of the tree rows `rows`, whose roots are `places` in order, each with its
/// node. A place shows whether it is a drive, and a drive how full it is; the first favorite
/// carries the head "즐겨찾기" and the first drive after them "이 PC". A row shows a chevron
/// when it has folders below it, or may have.
pub fn tree_rows(rows: &[TreeRowData], places: &[Place]) -> Vec<(NodeId, TreeRow)> {
    // Whether the last place was a drive.
    let mut last_drive = None;
    rows.iter()
        .map(|row| {
            let place = row.root.and_then(|root| places.get(root));
            let is_drive = place.is_some_and(|place| place.kind == PlaceKind::Drive);
            let head = match place {
                Some(_) if last_drive == Some(is_drive) => SectionHead::None,
                Some(_) if is_drive => SectionHead::Drives,
                Some(_) => SectionHead::Favorites,
                None => SectionHead::None,
            };
            if place.is_some() {
                last_drive = Some(is_drive);
            }
            let (info, fill) = place.map_or_else(|| (String::new(), 0.0), place_space);
            let expandable = row.has_children_unknown || row.has_children;
            let shown = TreeRow {
                label: row.label.as_str().into(),
                depth: i32::try_from(row.depth).unwrap_or(i32::MAX),
                head,
                is_place: place.is_some(),
                is_drive,
                info: info.into(),
                fill,
                expandable,
                expanded: expandable && row.expanded,
                selected: row.selected,
                dimmed: false,
                hidden: row.hidden,
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
        .filter_map(|&id| {
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
                info: info.into(),
                fill: 0.0,
                expandable: false,
                expanded: false,
                selected: tree.target() == Some(&node.path),
                dimmed: false,
                hidden: node.attributes.contains(Attributes::HIDDEN),
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

/// The row the list shows for `entry`. Only a file has a size; a time that is not known (the
/// creation time on a file system that keeps none) or beyond what can be shown shows nothing.
pub fn file_row(entry: &Entry, selected: bool, tz: &TimeZone) -> FileRow {
    let time = |time: Option<std::time::SystemTime>| -> SharedString {
        time.map(|time| format_time_in(time, tz))
            .unwrap_or_default()
            .into()
    };
    FileRow {
        name: entry.name.to_string_lossy().as_ref().into(),
        size: if entry.kind == EntryKind::File {
            format_size(entry.size).into()
        } else {
            SharedString::new()
        },
        modified: time(entry.modified),
        created: time(entry.created),
        kind: kind_label(entry).into(),
        extension: entry
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .unwrap_or_default()
            .into(),
        attributes: attribute_letters(entry.attributes).into(),
        icon: file_icon(entry).into(),
        is_dir: entry.kind == EntryKind::Dir,
        is_link: entry.kind == EntryKind::Symlink,
        selected,
        hidden: entry.attributes.contains(Attributes::HIDDEN),
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

/// The number of the column of the file list that sorts by `key` ([`Column::number`]: 0 name,
/// 1 size, 2 modified, 3 kind, 4 created, 5 extension).
pub fn sort_column(key: SortKey) -> i32 {
    Column::of_key(key).number()
}

/// The key that the column numbered `column` sorts by; `None` for no column, or the
/// attributes, which sort nothing.
pub fn sort_key(column: i32) -> Option<SortKey> {
    Column::from_number(column)?.sort_key()
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

/// What a drop with `action` does with the items; `None` for an action this app does not
/// take (none, link).
pub fn drag_mode(action: DragAction) -> Option<TransferMode> {
    match action {
        DragAction::Move => Some(TransferMode::Move),
        DragAction::Copy => Some(TransferMode::Copy),
        _ => None,
    }
}

/// Gives the window's toolbar, tooltips and menus the keys of their commands on `os`, from the
/// shortcut table (KEY-03).
pub fn show_shortcuts(window: &MainWindow, os: Os) {
    let shown = window.global::<Shortcuts<'_>>();
    let key =
        |command: Command| SharedString::from(keys::shortcut(&command, os).unwrap_or_default());
    shown.set_back(key(Command::Back));
    shown.set_forward(key(Command::Forward));
    shown.set_up(key(Command::Up));
    shown.set_new_folder(key(Command::NewFolder));
    shown.set_copy_to(key(Command::CopyTo));
    shown.set_move_to(key(Command::MoveTo));
    shown.set_trash(key(Command::Trash));
    shown.set_delete_permanently(key(Command::DeletePermanently));
    shown.set_rename(key(Command::Rename));
    shown.set_undo(key(Command::Undo));
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
}

/// A row of the job panel.
pub fn job_row_view(row: &JobRow) -> JobRowView {
    JobRowView {
        title: row.title.as_str().into(),
        detail: row.detail.as_str().into(),
        progress: row.fraction,
        progress_text: row.progress.as_str().into(),
        paused: row.paused,
        waiting: row.waiting,
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
        Some(Modal::Preflight(_)) => DialogKind::Preflight,
        Some(Modal::Pick(_)) => DialogKind::Pick,
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
        PickView {
            title: words::pick_title(request.mode).into(),
            summary: crate::pick::summary(request).into(),
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
            space: space.into(),
            problem: validity
                .map(|validity| {
                    words::pick_problem(validity, request.mode, request.folder_count > 0)
                })
                .unwrap_or_default()
                .into(),
            hint: pick_hint(picker).into(),
            unentered_hint: words::pick_path_unentered().into(),
            confirm_text: words::pick_confirm(request.mode).into(),
            can_confirm: picker.can_confirm(),
        }
    }
}

/// What the picker says beside its buttons about how it confirms: that the path typed must be
/// entered first (the button is off meanwhile), or a path or a folder given when the field was
/// cleared, that it is being looked at, or that an overwrite is confirmed by the button alone;
/// empty for none of these.
pub fn pick_hint(picker: &Picker) -> &'static str {
    if picker.typed.is_some() {
        words::pick_path_pending()
    } else if picker.path_edited && picker.path_blank {
        words::pick_path_blank()
    } else if picker.path_edited {
        words::pick_path_unentered()
    } else if picker.policy == rf_jobs::ConflictPolicy::Overwrite {
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

/// What the name dialog shows: the warning of the name typed so far, and whether OK takes it.
pub fn name_view(dialog: &NameDialog) -> NameView {
    let check = dialog.check();
    NameView {
        title: dialog.title().into(),
        initial: dialog.initial.as_str().into(),
        select_end: i32::try_from(dialog.select_end).unwrap_or(i32::MAX),
        ok_enabled: check == NameCheck::Ready,
        warning: match check {
            NameCheck::Invalid(warning) => warning.into(),
            NameCheck::Ready | NameCheck::Empty | NameCheck::Unchanged => SharedString::new(),
        },
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
    }
}

/// The window of failed items. Its first `MEASURED_LINES` lines go in `head` too: the window
/// fits its list to their height once wrapped.
pub fn message_view(failures: &Failures) -> MessageView {
    MessageView {
        title: failures.title.as_str().into(),
        lines: lines_model(&failures.lines),
        head: lines_model(head(&failures.lines)),
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
        overwrite_hint: words::overwrite_hint(can_overwrite, question.work).into(),
        skip_hint: words::skip_hint(conflict.incoming.kind, question.work).into(),
        rename_hint: words::rename_hint(&question.numbered_name()).into(),
        warning_once: words::overwrite_warning(false).into(),
        warning_all: words::overwrite_warning(true).into(),
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

    use super::*;
    use crate::loader::LoadEvent;

    fn names(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
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
        let row = file_row(&pdf, true, &TimeZone::UTC);
        assert_eq!(row.name, "설계안.pdf");
        assert_eq!(row.size, "1.2 MB");
        assert_eq!(row.modified, "2025-10-25 07:20");
        assert_eq!(row.kind, "PDF 문서");
        assert!(row.selected && !row.is_dir && !row.is_link && !row.hidden);

        let mut dot = entry(".config", EntryKind::Dir, 4096);
        dot.attributes |= Attributes::HIDDEN;
        let row = file_row(&dot, false, &TimeZone::UTC);
        // A folder has no size, and no time when none is known.
        assert_eq!((row.size.as_str(), row.modified.as_str()), ("", ""));
        assert!(row.is_dir && row.hidden && !row.selected);

        let link = file_row(
            &entry("latest", EntryKind::Symlink, 9),
            false,
            &TimeZone::UTC,
        );
        assert!(link.is_link && !link.is_dir);
        assert_eq!(link.size, "");
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
        ] {
            assert_eq!(sort_key(sort_column(key)), Some(key));
        }
        // The first demo's numbers stay.
        assert_eq!(sort_column(SortKey::Kind), 3);
        // The attributes sort nothing.
        assert_eq!(sort_key(6), None);
        assert_eq!(sort_key(7), None);
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

        assert_eq!(drag_mode(DragAction::Move), Some(TransferMode::Move));
        assert_eq!(drag_mode(DragAction::Copy), Some(TransferMode::Copy));
        assert_eq!(drag_mode(DragAction::Link), None);
        assert_eq!(drag_mode(DragAction::None), None);
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
}
