//! What the Slint screens are given: the row model of a file list, the conversions from the
//! panes' state to Slint structs, and [`post`], which brings a worker thread's result back to
//! the UI thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::rc::Rc;

use jiff::tz::TimeZone;
use rf_core::{Attributes, Entry, EntryKind, VPath};
use rf_vfs::{SortKey, SortOrder};
use slint::{Model, ModelNotify, ModelRc, ModelTracker, SharedString, VecModel};

use crate::controller::{self, Controller, ancestors};
use crate::format::{format_size, format_time_in, kind_label};
use crate::pane::{LoadStatus, Pane};
use crate::places::{Place, PlaceKind};
use crate::selection::Selection;
use crate::words;
use crate::{Crumb, FileRow, PaneView, PlaceRow, TabItem};

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

    /// What `pane` shows; `active` when it has the keyboard. The models are brought up to
    /// date and handed over as they are.
    pub fn view(&self, pane: &Pane, active: bool) -> PaneView {
        let tab = pane.active_tab();
        self.rows.sync(tab.visible(), tab.selection());
        set_rows(
            &self.tabs,
            pane.tabs()
                .iter()
                .zip(0..)
                .map(|(each, i)| TabItem {
                    title: folder_label(each.location()).into(),
                    active: i == pane.active(),
                })
                .collect(),
        );
        set_rows(&self.crumbs, crumbs(&ancestors(tab.location())));
        let sort = tab.sort().primary;
        PaneView {
            tabs: ModelRc::from(Rc::clone(&self.tabs)),
            crumbs: ModelRc::from(Rc::clone(&self.crumbs)),
            path_text: tab.location().to_string().into(),
            rows: ModelRc::from(Rc::clone(&self.rows)),
            cursor: tab
                .selection()
                .cursor()
                .and_then(|row| i32::try_from(row).ok())
                .unwrap_or(-1),
            sort_key: sort_column(sort.key),
            sort_ascending: sort.order == SortOrder::Ascending,
            status: tab.status_line().into(),
            loading: tab.status() == LoadStatus::Loading,
            error: match tab.status() {
                LoadStatus::Failed(kind) => words::error_kind(kind).into(),
                LoadStatus::Loading | LoadStatus::Ready => SharedString::new(),
            },
            active,
        }
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
        match changes {
            Some(changes) => {
                for change in changes {
                    match change {
                        RowChange::Removed { at, count } => self.notify.row_removed(at, count),
                        RowChange::Added { at, count } => self.notify.row_added(at, count),
                        RowChange::Changed { at } => self.notify.row_changed(at),
                    }
                }
            }
            None => self.notify.reset(),
        }
    }
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

/// The steps that turn `old` into `new`, where a row is known by its `name` (unique in a
/// list): first the removals and additions, each at its position in the list as the steps
/// before it have left it, then the changes, at positions in `new`. A row that moved is
/// removed and added. `None` when there are more than [`MAX_ROW_CHANGES`] steps, which a
/// fresh start shows faster.
pub fn diff_rows<T: PartialEq>(
    old: &[T],
    new: &[T],
    name: impl Fn(&T) -> &OsStr,
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

    let position: HashMap<&OsStr, usize> = new
        .iter()
        .enumerate()
        .map(|(j, row)| (name(row), j))
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
            let stays = position.get(name(row)).is_some_and(|&p| p >= j);
            if !stays {
                match steps.last_mut() {
                    Some(RowChange::Removed { at: last, count }) if *last == at => *count += 1,
                    _ => steps.push(RowChange::Removed { at, count: 1 }),
                }
                i += 1;
                continue;
            }
        }
        let same_name = old.get(i).is_some_and(|row| name(row) == name(&new[j]));
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

/// The row the list shows for `entry`. Only a file has a size; a time beyond what can be shown
/// shows nothing.
pub fn file_row(entry: &Entry, selected: bool, tz: &TimeZone) -> FileRow {
    FileRow {
        name: entry.name.to_string_lossy().as_ref().into(),
        size: if entry.kind == EntryKind::File {
            format_size(entry.size).into()
        } else {
            SharedString::new()
        },
        modified: entry
            .modified
            .map(|time| format_time_in(time, tz))
            .unwrap_or_default()
            .into(),
        kind: kind_label(entry).into(),
        is_dir: entry.kind == EntryKind::Dir,
        is_link: entry.kind == EntryKind::Symlink,
        selected,
        hidden: entry.attributes.contains(Attributes::HIDDEN),
    }
}

/// The name of a folder in a tab or the path row: its own name, or for a root the path
/// without its last separator (`/`, `C:`, `\\server\share`).
pub fn folder_label(path: &VPath) -> String {
    if let Some(name) = path.file_name() {
        return name.to_string_lossy().into_owned();
    }
    let text = path.to_string();
    let trimmed = text.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        text
    } else {
        trimmed.to_owned()
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

/// The sidebar row of `place`: a drive shows how full it is.
pub fn place_row(place: &Place) -> PlaceRow {
    let (info, fill) = match (place.kind, place.space) {
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
    };
    PlaceRow {
        label: place.label.as_str().into(),
        info: info.into(),
        fill,
        is_drive: place.kind == PlaceKind::Drive,
    }
}

/// The column of the file list that sorts by `key` (0 name, 1 size, 2 modified, 3 kind).
pub fn sort_column(key: SortKey) -> i32 {
    match key {
        SortKey::Name => 0,
        SortKey::Size => 1,
        SortKey::Modified => 2,
        SortKey::Kind => 3,
    }
}

/// The key that column `column` of the file list sorts by; `None` for no column.
pub fn sort_key(column: i32) -> Option<SortKey> {
    Some(match column {
        0 => SortKey::Name,
        1 => SortKey::Size,
        2 => SortKey::Modified,
        3 => SortKey::Kind,
        _ => return None,
    })
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
    use std::ffi::OsString;
    use std::time::{Duration, UNIX_EPOCH};

    use rf_platform::DiskSpace;

    use super::*;

    fn names(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
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
        let steps = diff_rows(&old, &new, |name| name.as_os_str()).expect("a few steps");
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
    fn folder_labels_name_the_last_folder() {
        let root = VPath::local(std::path::MAIN_SEPARATOR_STR).expect("the root is local");
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
        #[cfg(unix)]
        assert_eq!(folder_label(&root), "/");
    }

    #[test]
    fn drives_show_how_full_they_are() {
        const GB: u64 = 1 << 30;
        let root = VPath::local(std::path::MAIN_SEPARATOR_STR).expect("the root is local");
        let drive = Place {
            label: "로컬 디스크 (C:)".to_owned(),
            path: root.clone(),
            kind: PlaceKind::Drive,
            space: Some(DiskSpace {
                total: 512 * GB,
                free: 130 * GB,
                available: 128 * GB,
            }),
        };
        let row = place_row(&drive);
        assert_eq!(row.label, "로컬 디스크 (C:)");
        assert_eq!(row.info, "128 GB 남음 / 512 GB");
        assert!((row.fill - 0.75).abs() < 1e-6, "{}", row.fill);
        assert!(row.is_drive);

        let unknown = Place {
            space: None,
            ..drive.clone()
        };
        assert_eq!(place_row(&unknown).info, "");
        let home = Place {
            label: "홈".to_owned(),
            path: root,
            kind: PlaceKind::Favorite(rf_platform::KnownFolder::Home),
            space: None,
        };
        let row = place_row(&home);
        assert!(!row.is_drive && row.info.is_empty());
    }

    #[test]
    fn sort_columns_round_trip() {
        for key in [
            SortKey::Name,
            SortKey::Size,
            SortKey::Modified,
            SortKey::Kind,
        ] {
            assert_eq!(sort_key(sort_column(key)), Some(key));
        }
        assert_eq!(sort_key(4), None);
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
}
