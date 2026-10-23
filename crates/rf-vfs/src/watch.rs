//! The types a provider uses to report live changes of one folder.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::time::Duration;

use rf_core::{Entry, FileId, VolumeId};
use rf_platform::DirEvent;

/// How long a provider collects raw notifications before it sends them as one
/// `WatchEvent::Changes`.
pub const WATCH_DEBOUNCE: Duration = Duration::from_millis(100);

/// The most changes a provider holds back for one `WatchEvent::Changes`. When more pile up it
/// sends `WatchEvent::Rescan` instead.
pub const MAX_PENDING_CHANGES: usize = 10_000;

/// One change to a direct child of the watched folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// The entry was added or changed; this is how it looks now.
    Upsert(Entry),
    /// The entry with this name is gone.
    Removed(OsString),
    /// The entry `from` is now `to`.
    Renamed { from: OsString, to: Entry },
}

/// What a watch reports to its sink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchEvent {
    /// The net changes of one debounce window: each name appears at most once, with its state
    /// after the window. Order is unspecified.
    Changes(Vec<Change>),
    /// Changes may have been lost, or cannot be told apart (one file seen under two names, such
    /// as the two spellings of a case-only rename); the receiver lists the folder again.
    Rescan,
    /// The watch cannot go on and no event follows. The receiver drops the `WatchHandle`.
    Ended,
}

/// Receives the events of one watch, on a background thread of the provider. It must not block
/// for long.
pub type WatchSink = Box<dyn FnMut(WatchEvent) + Send>;

/// A running watch. Dropping it stops the watch, and the sink is not called after the drop.
///
/// Dropping it waits for a sink call that is in progress, so never drop it from inside the sink,
/// and never while holding a lock (or anything else) the sink may wait on: the two would wait
/// for each other.
pub struct WatchHandle {
    _guard: Box<dyn Any + Send>,
}

impl WatchHandle {
    /// A handle that keeps `guard` alive until it is dropped. A provider passes whatever stops
    /// its watch when dropped (an OS watcher, a thread join guard).
    pub fn new(guard: impl Send + 'static) -> WatchHandle {
        WatchHandle {
            _guard: Box::new(guard),
        }
    }
}

impl fmt::Debug for WatchHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WatchHandle")
    }
}

/// Collects the raw `DirEvent`s of one debounce window and turns them into `WatchEvent`s.
///
/// A name is looked at once, however often it was reported, and appears at most once in the
/// result. `flush` empties the collector.
#[derive(Debug, Default)]
pub(crate) struct Coalescer {
    /// The names reported in this window, in the order they were first seen.
    names: Vec<OsString>,
    /// Where each name is in `names`.
    index: HashMap<OsString, usize>,
    /// The renames the OS paired, as indexes into `names`.
    renames: Vec<(usize, usize)>,
    /// Too much happened or notifications were lost: the receiver lists the folder again.
    rescan: bool,
    /// The watch is over.
    ended: bool,
}

impl Coalescer {
    pub(crate) fn new() -> Coalescer {
        Coalescer::default()
    }

    /// Whether `DirEvent::Ended` was pushed since the last flush.
    pub(crate) fn has_ended(&self) -> bool {
        self.ended
    }

    pub(crate) fn push(&mut self, ev: DirEvent) {
        match ev {
            DirEvent::Changed(name) => {
                self.note(name);
            }
            DirEvent::Renamed { from, to } => {
                let from = self.note(from);
                let to = self.note(to);
                if let (Some(from), Some(to)) = (from, to) {
                    if self.renames.len() < MAX_PENDING_CHANGES {
                        self.renames.push((from, to));
                    } else {
                        self.give_up();
                    }
                }
            }
            DirEvent::Overflow => self.give_up(),
            DirEvent::Ended => self.ended = true,
        }
    }

    /// Notes that `name` changed and says where it is in `names`. After a rescan is due,
    /// nothing is kept and the answer is `None`.
    fn note(&mut self, name: OsString) -> Option<usize> {
        if self.rescan {
            return None;
        }
        if let Some(&i) = self.index.get(&name) {
            return Some(i);
        }
        if self.names.len() == MAX_PENDING_CHANGES {
            self.give_up();
            return None;
        }
        let i = self.names.len();
        self.index.insert(name.clone(), i);
        self.names.push(name);
        Some(i)
    }

    /// Switches to "list the folder again" and frees what was collected.
    fn give_up(&mut self) {
        self.rescan = true;
        self.names = Vec::new();
        self.index = HashMap::new();
        self.renames = Vec::new();
    }

    /// The events for the window so far, and the collector empty again: `[Ended]` if the watch
    /// ended, else `[Rescan]` if a rescan is due, else `[Changes(..)]` if anything changed,
    /// else nothing.
    ///
    /// A rescan is also due when the window may name one file twice, because the receiver
    /// would then keep an entry under a name that no longer exists (see `names_one_file_twice`).
    ///
    /// `stat` gives the entry now at a name, or `None` if there is none. It is called once per
    /// name, and not at all for `Ended` and `Rescan`. `identify` gives the volume and file ID
    /// of the file at a name, for entries that carry none (Windows); it is called only when two
    /// or more names are present and their spellings are all different.
    pub(crate) fn flush(
        &mut self,
        stat: &mut dyn FnMut(&OsStr) -> Option<Entry>,
        identify: &mut dyn FnMut(&OsStr) -> Option<(VolumeId, FileId)>,
    ) -> Vec<WatchEvent> {
        let window = std::mem::take(self);
        if window.ended {
            return vec![WatchEvent::Ended];
        }
        if window.rescan {
            return vec![WatchEvent::Rescan];
        }
        match window.changes(stat, identify) {
            None => vec![WatchEvent::Rescan],
            Some(changes) if changes.is_empty() => Vec::new(),
            Some(changes) => vec![WatchEvent::Changes(changes)],
        }
    }

    /// The net changes, or `None` if the receiver has to list the folder again.
    ///
    /// A pair becomes `Renamed` when its source is gone and its target is there; every other
    /// name is an `Upsert` or `Removed` by what `stat` finds.
    ///
    /// A name takes part in one change only. A rename that shares a name with an earlier
    /// `Renamed` is left to the per-name changes, which give the same end state.
    fn changes(
        self,
        stat: &mut dyn FnMut(&OsStr) -> Option<Entry>,
        identify: &mut dyn FnMut(&OsStr) -> Option<(VolumeId, FileId)>,
    ) -> Option<Vec<Change>> {
        let Coalescer { names, renames, .. } = self;
        let mut now: Vec<Option<Entry>> = names.iter().map(|name| stat(name)).collect();
        if names_one_file_twice(&names, &now, identify) {
            return None;
        }
        let mut taken = vec![false; names.len()];
        let mut changes = Vec::new();
        for (from, to) in renames {
            if taken[from] || taken[to] || now[from].is_some() {
                continue;
            }
            if let Some(entry) = now[to].take() {
                taken[from] = true;
                taken[to] = true;
                changes.push(Change::Renamed {
                    from: names[from].clone(),
                    to: entry,
                });
            }
        }
        for ((name, entry), taken) in names.into_iter().zip(now).zip(taken) {
            if !taken {
                changes.push(match entry {
                    Some(entry) => Change::Upsert(entry),
                    None => Change::Removed(name),
                });
            }
        }
        Some(changes)
    }
}

/// Whether the names of one window may be one file under two names, which the receiver cannot
/// tell from two files. `stat` finds the same file under every name that is another spelling
/// of its name or another name for it, so after a rename the receiver would get an `Upsert`
/// for the old name as well, an entry that no longer exists. Cases:
///
/// - a case-only rename (`b.txt` to `B.TXT`) on NTFS or default APFS, and macOS reporting a
///   rename as two changes;
/// - the Unicode normalization forms of a name on APFS (NFC and NFD);
/// - an 8.3 short name on Windows;
/// - hard links.
///
/// It is so when two names fold to one string (`to_lowercase` of the lossy name), whether or not
/// both are present; or when two names that are present have the same file ID (compared only
/// when both are known). The IDs cost a call each on Windows, so they are asked for only when
/// two names are present and the spellings are all different.
///
/// This over-reports (two hard links, or `a` and `A` on a case-sensitive volume, are two files
/// that are reported as a rescan too), which only costs a listing.
fn names_one_file_twice(
    names: &[OsString],
    now: &[Option<Entry>],
    identify: &mut dyn FnMut(&OsStr) -> Option<(VolumeId, FileId)>,
) -> bool {
    if names.len() < 2 {
        return false;
    }
    let mut spellings = HashSet::with_capacity(names.len());
    if !names
        .iter()
        .all(|name| spellings.insert(name.to_string_lossy().to_lowercase()))
    {
        return true;
    }
    if now.iter().flatten().nth(1).is_none() {
        return false;
    }
    let mut ids = HashSet::new();
    for (name, entry) in names.iter().zip(now) {
        let Some(entry) = entry else { continue };
        let Some(id) = entry.identity().or_else(|| identify(name)) else {
            continue;
        };
        if !ids.insert(id) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::ffi::OsStr;

    use rf_core::EntryKind;

    use super::*;

    fn o(s: &str) -> OsString {
        OsString::from(s)
    }

    fn entry(s: &str) -> Entry {
        Entry::new(s, EntryKind::File)
    }

    fn stat_from(existing: &'static [&'static str]) -> impl FnMut(&OsStr) -> Option<Entry> {
        move |n: &OsStr| {
            existing
                .iter()
                .any(|e| OsStr::new(e) == n)
                .then(|| entry(&n.to_string_lossy()))
        }
    }

    /// An entry with the volume and file ID the OS gives it (Unix always does).
    fn entry_with_id(s: &str, id: u128) -> Entry {
        let mut e = entry(s);
        e.volume_id = Some(VolumeId(1));
        e.file_id = Some(FileId(id));
        e
    }

    fn no_events() -> Vec<WatchEvent> {
        Vec::new()
    }

    /// The IDs of a platform where the entries carry none and nothing else can say (`None`).
    fn no_identity() -> impl FnMut(&OsStr) -> Option<(VolumeId, FileId)> {
        |_: &OsStr| None
    }

    /// For tests where the flush must not ask for an ID.
    fn no_id_lookup() -> impl FnMut(&OsStr) -> Option<(VolumeId, FileId)> {
        |name: &OsStr| panic!("the ID of {name:?} must not be looked up")
    }

    fn renamed(from: &str, to: &str) -> DirEvent {
        DirEvent::Renamed {
            from: o(from),
            to: o(to),
        }
    }

    #[test]
    fn changes_are_deduplicated_in_first_seen_order() {
        let mut c = Coalescer::new();
        for n in ["a", "b", "a"] {
            c.push(DirEvent::Changed(o(n)));
        }
        let want = WatchEvent::Changes(vec![Change::Upsert(entry("a")), Change::Removed(o("b"))]);
        assert_eq!(c.flush(&mut stat_from(&["a"]), &mut no_identity()), [want]);
    }

    #[test]
    fn paired_rename_is_reported_once() {
        let mut c = Coalescer::new();
        c.push(renamed("x", "y"));
        c.push(DirEvent::Changed(o("x")));
        c.push(DirEvent::Changed(o("y")));
        let want = WatchEvent::Changes(vec![Change::Renamed {
            from: o("x"),
            to: entry("y"),
        }]);
        assert_eq!(c.flush(&mut stat_from(&["y"]), &mut no_identity()), [want]);
    }

    #[test]
    fn inotify_order_of_a_rename_is_reported_once() {
        // inotify reports one rename as Changed(from), Changed(to), Renamed { from, to }.
        let mut c = Coalescer::new();
        c.push(DirEvent::Changed(o("x")));
        c.push(DirEvent::Changed(o("y")));
        c.push(renamed("x", "y"));
        let want = WatchEvent::Changes(vec![Change::Renamed {
            from: o("x"),
            to: entry("y"),
        }]);
        assert_eq!(c.flush(&mut stat_from(&["y"]), &mut no_identity()), [want]);
    }

    #[test]
    fn rename_falls_back_when_source_still_exists() {
        let mut c = Coalescer::new();
        c.push(renamed("x", "y"));
        let want =
            WatchEvent::Changes(vec![Change::Upsert(entry("x")), Change::Upsert(entry("y"))]);
        assert_eq!(
            c.flush(&mut stat_from(&["x", "y"]), &mut no_identity()),
            [want]
        );
    }

    #[test]
    fn rename_falls_back_when_target_is_gone() {
        let mut c = Coalescer::new();
        c.push(renamed("x", "y"));
        let want = WatchEvent::Changes(vec![Change::Removed(o("x")), Change::Removed(o("y"))]);
        assert_eq!(c.flush(&mut stat_from(&[]), &mut no_identity()), [want]);
    }

    #[test]
    fn a_name_in_two_renames_is_reported_once() {
        // a -> b, then a is created again and a -> c: `a` can be the source of one rename only.
        let mut c = Coalescer::new();
        c.push(renamed("a", "b"));
        c.push(renamed("a", "c"));
        let want = WatchEvent::Changes(vec![
            Change::Renamed {
                from: o("a"),
                to: entry("b"),
            },
            Change::Upsert(entry("c")),
        ]);
        assert_eq!(
            c.flush(&mut stat_from(&["b", "c"]), &mut no_identity()),
            [want]
        );
    }

    #[test]
    fn a_chain_of_renames_is_reported_by_its_last_link() {
        // a -> b -> c: only `c` exists at the end of the window.
        let mut c = Coalescer::new();
        c.push(renamed("a", "b"));
        c.push(renamed("b", "c"));
        let want = WatchEvent::Changes(vec![
            Change::Renamed {
                from: o("b"),
                to: entry("c"),
            },
            Change::Removed(o("a")),
        ]);
        assert_eq!(c.flush(&mut stat_from(&["c"]), &mut no_identity()), [want]);
    }

    /// Every sequence of up to three `Changed`/`Renamed` events over three names, against every
    /// set of existing names: each touched name is reported exactly once, and as present or
    /// absent the way `stat` saw it.
    #[test]
    fn every_name_is_reported_once_with_its_final_state() {
        const NAMES: [&str; 3] = ["a", "b", "c"];
        let index = |n: &OsStr| NAMES.iter().position(|x| OsStr::new(x) == n).unwrap();
        let mut events = Vec::new();
        for from in NAMES {
            events.push(DirEvent::Changed(o(from)));
            for to in NAMES {
                events.push(renamed(from, to));
            }
        }
        let mut sequences: Vec<Vec<DirEvent>> = vec![Vec::new()];
        let mut start = 0;
        for _ in 0..3 {
            let end = sequences.len();
            for i in start..end {
                for event in &events {
                    let mut next = sequences[i].clone();
                    next.push(event.clone());
                    sequences.push(next);
                }
            }
            start = end;
        }
        for sequence in sequences {
            for exists in 0..(1_u8 << NAMES.len()) {
                let present = |n: &OsStr| (exists >> index(n)) & 1 == 1;
                let mut c = Coalescer::new();
                let mut touched = BTreeSet::new();
                for event in &sequence {
                    match event {
                        DirEvent::Changed(n) => {
                            touched.insert(n.clone());
                        }
                        DirEvent::Renamed { from, to } => {
                            touched.insert(from.clone());
                            touched.insert(to.clone());
                        }
                        _ => unreachable!("only Changed and Renamed are generated"),
                    }
                    c.push(event.clone());
                }
                let flushed = c.flush(
                    &mut |n: &OsStr| present(n).then(|| Entry::new(n, EntryKind::File)),
                    &mut no_identity(),
                );
                // (name, present) for every name the changes mention.
                let mut mentioned = Vec::new();
                for event in flushed {
                    let WatchEvent::Changes(changes) = event else {
                        panic!("{sequence:?} gave {event:?}");
                    };
                    for change in changes {
                        match change {
                            Change::Upsert(e) => mentioned.push((e.name, true)),
                            Change::Removed(n) => mentioned.push((n, false)),
                            Change::Renamed { from, to } => {
                                mentioned.push((from, false));
                                mentioned.push((to.name, true));
                            }
                        }
                    }
                }
                let names: BTreeSet<_> = mentioned.iter().map(|(n, _)| n.clone()).collect();
                assert_eq!(
                    names.len(),
                    mentioned.len(),
                    "{sequence:?} / {exists:b}: {mentioned:?}"
                );
                assert_eq!(names, touched, "{sequence:?} / {exists:b}");
                for (name, is_present) in mentioned {
                    assert_eq!(is_present, present(&name), "{sequence:?} / {exists:b}");
                }
            }
        }
    }

    /// One file seen under two spellings in one window: a case-insensitive volume finds it under
    /// both names, so the receiver would get one entry per spelling, one of them a ghost.
    #[test]
    fn two_spellings_of_one_name_become_a_rescan() {
        for (a, b) in [("b.txt", "B.TXT"), ("\u{c9}.txt", "\u{e9}.txt")] {
            let mut c = Coalescer::new();
            c.push(DirEvent::Changed(o(a)));
            c.push(DirEvent::Changed(o(b)));
            // Both spellings find the same file, as on NTFS or default APFS. The names alone
            // are enough, so no ID is looked up.
            let mut stat = |n: &OsStr| Some(entry_with_id(&n.to_string_lossy(), 7));
            assert_eq!(
                c.flush(&mut stat, &mut no_id_lookup()),
                [WatchEvent::Rescan],
                "{a} / {b}"
            );
            assert_eq!(c.flush(&mut stat, &mut no_id_lookup()), no_events());
        }
    }

    #[test]
    fn a_case_only_rename_becomes_a_rescan() {
        let mut c = Coalescer::new();
        c.push(renamed("b.txt", "B.TXT"));
        // The old spelling still finds the file (case-insensitive volume).
        let mut stat = |n: &OsStr| Some(entry_with_id(&n.to_string_lossy(), 7));
        assert_eq!(
            c.flush(&mut stat, &mut no_id_lookup()),
            [WatchEvent::Rescan]
        );
    }

    /// Case-sensitive volumes (Linux) report the same pair with the old spelling gone. The
    /// names fold to one string either way, so it is a rescan too.
    #[test]
    fn a_case_only_rename_is_a_rescan_when_the_old_spelling_is_gone() {
        let mut c = Coalescer::new();
        c.push(renamed("b.txt", "B.TXT"));
        assert_eq!(
            c.flush(&mut stat_from(&["B.TXT"]), &mut no_id_lookup()),
            [WatchEvent::Rescan]
        );
    }

    #[test]
    fn distinct_files_are_reported_as_changes() {
        let mut c = Coalescer::new();
        c.push(DirEvent::Changed(o("a")));
        c.push(DirEvent::Changed(o("b")));
        let mut stat = |n: &OsStr| {
            let id = if n == "a" { 1 } else { 2 };
            Some(entry_with_id(&n.to_string_lossy(), id))
        };
        let want = WatchEvent::Changes(vec![
            Change::Upsert(entry_with_id("a", 1)),
            Change::Upsert(entry_with_id("b", 2)),
        ]);
        // The entries carry their IDs, so nothing is looked up.
        assert_eq!(c.flush(&mut stat, &mut no_id_lookup()), [want]);
    }

    #[test]
    fn entries_without_an_id_are_looked_up_and_distinct_ones_stay_changes() {
        let mut c = Coalescer::new();
        c.push(DirEvent::Changed(o("a")));
        c.push(DirEvent::Changed(o("b")));
        let mut asked = Vec::new();
        let mut identify = |n: &OsStr| {
            asked.push(n.to_owned());
            Some((VolumeId(1), FileId(if n == "a" { 1 } else { 2 })))
        };
        let want =
            WatchEvent::Changes(vec![Change::Upsert(entry("a")), Change::Upsert(entry("b"))]);
        assert_eq!(c.flush(&mut stat_from(&["a", "b"]), &mut identify), [want]);
        assert_eq!(asked, [o("a"), o("b")]);
    }

    /// Windows: `entry_from_metadata` has no ID there, and an 8.3 short name is another name
    /// for the same file.
    #[test]
    fn two_names_with_one_looked_up_id_become_a_rescan() {
        let mut c = Coalescer::new();
        c.push(DirEvent::Changed(o("LONGNA~1.TXT")));
        c.push(DirEvent::Changed(o("longname.txt")));
        let mut identify = |_: &OsStr| Some((VolumeId(1), FileId(9)));
        assert_eq!(
            c.flush(
                &mut stat_from(&["LONGNA~1.TXT", "longname.txt"]),
                &mut identify
            ),
            [WatchEvent::Rescan]
        );
    }

    #[test]
    fn names_whose_id_is_unknown_are_not_taken_for_one_file() {
        let mut c = Coalescer::new();
        c.push(DirEvent::Changed(o("a")));
        c.push(DirEvent::Changed(o("b")));
        let want =
            WatchEvent::Changes(vec![Change::Upsert(entry("a")), Change::Upsert(entry("b"))]);
        assert_eq!(
            c.flush(&mut stat_from(&["a", "b"]), &mut no_identity()),
            [want]
        );
    }

    #[test]
    fn ids_are_looked_up_only_for_two_or_more_present_names() {
        let mut c = Coalescer::new();
        c.push(DirEvent::Changed(o("a")));
        c.push(DirEvent::Changed(o("b")));
        c.push(DirEvent::Changed(o("c")));
        let want = WatchEvent::Changes(vec![
            Change::Upsert(entry("a")),
            Change::Removed(o("b")),
            Change::Removed(o("c")),
        ]);
        assert_eq!(c.flush(&mut stat_from(&["a"]), &mut no_id_lookup()), [want]);
    }

    /// Two names of one file (a hard link) in one window cannot be told apart from one file
    /// seen twice, so the window is a rescan. Real files: the IDs come from `stat`.
    #[cfg(unix)]
    #[test]
    fn hard_linked_names_changed_together_become_a_rescan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x"), b"1").unwrap();
        std::fs::hard_link(dir.path().join("x"), dir.path().join("y")).unwrap();
        std::fs::write(dir.path().join("z"), b"2").unwrap();
        let mut stat = |n: &OsStr| {
            let meta = std::fs::symlink_metadata(dir.path().join(n)).ok()?;
            Some(crate::local::entry_from_metadata(n.to_owned(), &meta))
        };
        let mut c = Coalescer::new();
        c.push(DirEvent::Changed(o("x")));
        c.push(DirEvent::Changed(o("y")));
        assert_eq!(
            c.flush(&mut stat, &mut no_id_lookup()),
            [WatchEvent::Rescan]
        );
        // A different file next to them is not a collision.
        c.push(DirEvent::Changed(o("x")));
        c.push(DirEvent::Changed(o("z")));
        let flushed = c.flush(&mut stat, &mut no_id_lookup());
        assert!(
            matches!(&flushed[..], [WatchEvent::Changes(changes)] if changes.len() == 2),
            "{flushed:?}"
        );
    }

    #[test]
    fn overflow_becomes_rescan_and_resets() {
        let mut c = Coalescer::new();
        c.push(DirEvent::Changed(o("a")));
        c.push(DirEvent::Overflow);
        assert_eq!(
            c.flush(&mut stat_from(&[]), &mut no_identity()),
            [WatchEvent::Rescan]
        );
        assert_eq!(
            c.flush(&mut stat_from(&[]), &mut no_identity()),
            no_events()
        );
    }

    #[test]
    fn too_many_changes_become_rescan_without_stat() {
        let mut c = Coalescer::new();
        for i in 0..=MAX_PENDING_CHANGES {
            c.push(DirEvent::Changed(OsString::from(i.to_string())));
        }
        assert_eq!(
            c.flush(
                &mut |_: &OsStr| panic!("stat must not run"),
                &mut no_identity()
            ),
            [WatchEvent::Rescan]
        );
    }

    #[test]
    fn exactly_the_limit_is_still_reported_as_changes() {
        let mut c = Coalescer::new();
        for i in 0..MAX_PENDING_CHANGES {
            c.push(DirEvent::Changed(OsString::from(i.to_string())));
        }
        // A name seen before does not count twice.
        c.push(DirEvent::Changed(o("0")));
        let flushed = c.flush(&mut |_: &OsStr| None, &mut no_identity());
        assert!(
            matches!(&flushed[..], [WatchEvent::Changes(changes)] if changes.len() == MAX_PENDING_CHANGES),
            "{} events",
            flushed.len()
        );
    }

    #[test]
    fn too_many_renames_become_rescan_without_stat() {
        let mut c = Coalescer::new();
        for _ in 0..=MAX_PENDING_CHANGES {
            c.push(renamed("a", "b"));
        }
        assert_eq!(
            c.flush(
                &mut |_: &OsStr| panic!("stat must not run"),
                &mut no_identity()
            ),
            [WatchEvent::Rescan]
        );
    }

    #[test]
    fn ended_wins_over_pending_changes() {
        let mut c = Coalescer::new();
        c.push(DirEvent::Changed(o("a")));
        c.push(DirEvent::Ended);
        assert_eq!(
            c.flush(&mut stat_from(&[]), &mut no_identity()),
            [WatchEvent::Ended]
        );
        assert_eq!(
            c.flush(&mut stat_from(&[]), &mut no_identity()),
            no_events()
        );
    }

    #[test]
    fn ended_wins_over_rescan() {
        let mut c = Coalescer::new();
        c.push(DirEvent::Overflow);
        c.push(DirEvent::Ended);
        assert_eq!(
            c.flush(&mut stat_from(&[]), &mut no_identity()),
            [WatchEvent::Ended]
        );
    }

    #[test]
    fn empty_flush_is_empty() {
        assert_eq!(
            Coalescer::new().flush(&mut stat_from(&[]), &mut no_identity()),
            no_events()
        );
    }
}
