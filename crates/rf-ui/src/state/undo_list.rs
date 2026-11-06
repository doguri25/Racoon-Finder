//! The job panel's 되돌리기 기록 (OPS-12): what the engine's history can undo and what it can
//! do again, the latest first, each with a short account of what was done and when. A row's
//! button does what Ctrl+Z or Ctrl+Y does ([`AppState::undo`], [`AppState::redo`]) and only the
//! first row of a list can be pressed: the history is a stack, and nothing in the middle is
//! undone alone. A row whose items are no longer as its record needs them, as far as the
//! folders on screen show (nothing here looks at the disk), is dimmed and says why. A child of
//! [`state`](super), so these methods see the fields of [`AppState`].

use std::collections::HashMap;
use std::time::SystemTime;

use jiff::tz::TimeZone;
use rf_core::{Entry, EntryKind, Os, VPath};
use rf_jobs::UndoRecord;

use super::{AppState, Effect};
use crate::jobs::History;
use crate::pane::{LoadStatus, Pane, Tab, search_key};
use crate::words::{self, HISTORY_SHOWN, Stale};

/// How many items of one record are looked at for [`Stale`]: a copy of thousands of files is
/// judged by its first ones.
const CHECKED_ITEMS: usize = 64;

/// One row of 되돌리기 기록, as the window shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryLine {
    /// What was done, or what doing it again does ([`words::history_action`](crate::words)).
    pub text: String,
    /// When: `14:32`, or the day too.
    pub time: String,
    /// The second line: whether it is the next one, why it waits, or why it cannot run.
    pub note: String,
    /// The row the button of the list acts on: the first of its list.
    pub next: bool,
    /// Its button can be pressed.
    pub runnable: bool,
    /// The record can no longer be undone (or done again).
    pub dimmed: bool,
}

/// The rows of 되돌리기 기록: what can be undone and what can be done again, the latest first,
/// at most [`HISTORY_SHOWN`](crate::words::HISTORY_SHOWN) of each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryView {
    pub undo: Vec<HistoryLine>,
    pub redo: Vec<HistoryLine>,
    /// How many records each list holds, shown or not.
    pub undo_total: usize,
    pub redo_total: usize,
}

impl AppState {
    /// Takes what the engine's history can undo and redo next. A record not seen before is
    /// taken to be as old as `history.at`; one seen before keeps its time.
    pub fn set_history(&mut self, history: History) {
        let mut seen = std::mem::take(&mut self.history_stamps);
        self.history_stamps = history
            .undo
            .iter()
            .chain(&history.redo)
            .map(|record| {
                let stamp = seen
                    .iter()
                    .position(|(old, _)| old == record)
                    .map_or(history.at, |at| seen.remove(at).1);
                (record.clone(), stamp)
            })
            .collect();
        self.history = history;
    }

    /// The rows of 되돌리기 기록, with times in `tz` as of `now`.
    pub fn history_view(&self, tz: &TimeZone, now: SystemTime) -> HistoryView {
        let (undo, redo) = (&self.history.undo, &self.history.redo);
        let mut listings = Listings::new(self);
        let mut rows = |records: &[UndoRecord], redo_list: bool, first_stamp: usize| {
            let (can, shown) = if redo_list {
                (self.can_redo(), records.len().min(HISTORY_SHOWN))
            } else {
                (self.can_undo(), records.len().min(HISTORY_SHOWN))
            };
            records
                .iter()
                .enumerate()
                .rev()
                .take(shown)
                .enumerate()
                .map(|(place, (at, record))| {
                    let next = place == 0;
                    let stale = stale_of(record, &mut listings);
                    let stamp = self
                        .history_stamps
                        .get(first_stamp + at)
                        .map_or(SystemTime::UNIX_EPOCH, |(_, stamp)| *stamp);
                    HistoryLine {
                        text: words::history_action(record, redo_list),
                        time: words::history_time(stamp, now, tz),
                        note: words::history_note(redo_list, next, stale),
                        next,
                        runnable: next && can && stale.is_none(),
                        dimmed: stale.is_some(),
                    }
                })
                .collect::<Vec<_>>()
        };
        HistoryView {
            undo: rows(undo, false, 0),
            redo: rows(redo, true, undo.len()),
            undo_total: undo.len(),
            redo_total: redo.len(),
        }
    }

    /// How many records the history lists hold in all: what can be undone, then what can be
    /// done again.
    pub fn history_counts(&self) -> (usize, usize) {
        (self.history.undo.len(), self.history.redo.len())
    }

    /// Whether 되돌리기 기록 has anything to list.
    pub fn history_any(&self) -> bool {
        !self.history.undo.is_empty() || !self.history.redo.is_empty()
    }

    /// How many rows each list of 되돌리기 기록 shows (at most [`HISTORY_SHOWN`]).
    pub(super) fn history_shown(&self) -> (usize, usize) {
        let (undo, redo) = self.history_counts();
        (undo.min(HISTORY_SHOWN), redo.min(HISTORY_SHOWN))
    }

    /// Whether 되돌리기 기록 shows its rows below its head.
    pub fn history_open(&self) -> bool {
        self.history_open
    }

    /// 되돌리기 기록's 펼치기 or 접기.
    pub fn history_toggled(&mut self) {
        self.history_open = !self.history_open;
    }

    /// The button of row `index` of the list of what can be undone, or, with `redo`, of what
    /// can be done again: the same as Ctrl+Z (Ctrl+Y) for the first row, and nothing for the
    /// others, or for a row that cannot run ([`HistoryLine::runnable`]).
    pub fn history_run(&mut self, redo: bool, index: usize) -> Vec<Effect> {
        if index != 0 {
            return Vec::new();
        }
        let list = if redo {
            &self.history.redo
        } else {
            &self.history.undo
        };
        let Some(record) = list.last() else {
            return Vec::new();
        };
        if stale_of(record, &mut Listings::new(self)).is_some() {
            return Vec::new();
        }
        if redo { self.redo() } else { self.undo() }
    }
}

/// Whether `record` can no longer be undone, as far as the folders on screen show.
fn stale_of(record: &UndoRecord, listings: &mut Listings<'_>) -> Option<Stale> {
    let mut missing = |path: &VPath| matches!(listings.find(path), Found::Absent);
    match record {
        UndoRecord::Renamed { to, .. } => missing(to).then_some(Stale::Missing),
        UndoRecord::Created { path, .. } => missing(path).then_some(Stale::Missing),
        UndoRecord::Moved { moves, .. } => moves
            .iter()
            .take(CHECKED_ITEMS)
            .any(|moved| missing(&moved.to))
            .then_some(Stale::Missing),
        UndoRecord::Restored { items } => items
            .iter()
            .take(CHECKED_ITEMS)
            .any(|item| missing(&item.original))
            .then_some(Stale::Missing),
        UndoRecord::LinksMade { at } => at
            .iter()
            .take(CHECKED_ITEMS)
            .any(|(link, _, _)| missing(link))
            .then_some(Stale::Missing),
        UndoRecord::Copied { created, .. } => {
            let mut changed = false;
            for item in created.iter().take(CHECKED_ITEMS) {
                match listings.find(&item.path) {
                    Found::Absent => return Some(Stale::Missing),
                    Found::Present(entry) => {
                        // A folder changes as its contents do: only files tell.
                        changed |= !item.is_dir
                            && (entry.kind != EntryKind::File
                                || entry.size != item.size
                                || item
                                    .modified
                                    .is_some_and(|made| entry.modified != Some(made)));
                    }
                    Found::Unknown => {}
                }
            }
            changed.then_some(Stale::Changed)
        }
        // What the trash holds, or what a record added after this was written needs, cannot
        // be told from a folder on screen.
        _ => None,
    }
}

/// What the folders on screen say of one item.
enum Found<'a> {
    /// No tab shows its folder, or not yet whole: nothing tells.
    Unknown,
    /// The folder shows and the item is not in it.
    Absent,
    Present(&'a Entry),
}

/// The listings of the tabs, looked up by name; one is indexed the first time it is asked.
struct Listings<'a> {
    tabs: Vec<&'a Tab>,
    indexed: HashMap<usize, HashMap<String, &'a Entry>>,
}

impl<'a> Listings<'a> {
    fn new(state: &'a AppState) -> Listings<'a> {
        Listings {
            tabs: state
                .panes
                .iter()
                .flat_map(Pane::tabs)
                .filter(|tab| tab.status() == LoadStatus::Ready)
                .collect(),
            indexed: HashMap::new(),
        }
    }

    fn find(&mut self, path: &VPath) -> Found<'a> {
        let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
            return Found::Unknown;
        };
        let Some(at) = self.tabs.iter().position(|tab| *tab.location() == folder) else {
            return Found::Unknown;
        };
        let tab = self.tabs[at];
        let names = self.indexed.entry(at).or_insert_with(|| {
            tab.all()
                .iter()
                .map(|entry| (search_key(&entry.name, Os::CURRENT), entry))
                .collect()
        });
        match names.get(&search_key(name, Os::CURRENT)) {
            Some(entry) => Found::Present(entry),
            None => Found::Absent,
        }
    }
}
