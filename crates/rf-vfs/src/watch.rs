//! The types a provider uses to report live changes of one folder.

use std::any::Any;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::time::Duration;

use rf_core::Entry;
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
    /// Changes may have been lost; the receiver lists the folder again.
    Rescan,
    /// The watch cannot go on and no event follows. The receiver drops the `WatchHandle`.
    Ended,
}

/// Receives the events of one watch, on a background thread of the provider. It must not block
/// for long.
pub type WatchSink = Box<dyn FnMut(WatchEvent) + Send>;

/// A running watch. Dropping it stops the watch, and the sink is not called after the drop.
///
/// Never drop it from inside the sink: stopping can wait for the thread that runs the sink.
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
    /// `stat` gives the entry now at a name, or `None` if there is none. It is called once per
    /// name, and not at all for `Ended` and `Rescan`.
    pub(crate) fn flush(
        &mut self,
        stat: &mut dyn FnMut(&OsStr) -> Option<Entry>,
    ) -> Vec<WatchEvent> {
        let window = std::mem::take(self);
        if window.ended {
            return vec![WatchEvent::Ended];
        }
        if window.rescan {
            return vec![WatchEvent::Rescan];
        }
        let changes = window.changes(stat);
        if changes.is_empty() {
            Vec::new()
        } else {
            vec![WatchEvent::Changes(changes)]
        }
    }

    /// The net changes: a pair becomes `Renamed` when its source is gone and its target is
    /// there; every other name is an `Upsert` or `Removed` by what `stat` finds.
    ///
    /// A name takes part in one change only. A rename that shares a name with an earlier
    /// `Renamed` is left to the per-name changes, which give the same end state.
    fn changes(self, stat: &mut dyn FnMut(&OsStr) -> Option<Entry>) -> Vec<Change> {
        let Coalescer { names, renames, .. } = self;
        let mut now: Vec<Option<Entry>> = names.iter().map(|name| stat(name)).collect();
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
        changes
    }
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

    fn no_events() -> Vec<WatchEvent> {
        Vec::new()
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
        assert_eq!(c.flush(&mut stat_from(&["a"])), [want]);
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
        assert_eq!(c.flush(&mut stat_from(&["y"])), [want]);
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
        assert_eq!(c.flush(&mut stat_from(&["y"])), [want]);
    }

    #[test]
    fn rename_falls_back_when_source_still_exists() {
        let mut c = Coalescer::new();
        c.push(renamed("x", "y"));
        let want =
            WatchEvent::Changes(vec![Change::Upsert(entry("x")), Change::Upsert(entry("y"))]);
        assert_eq!(c.flush(&mut stat_from(&["x", "y"])), [want]);
    }

    #[test]
    fn rename_falls_back_when_target_is_gone() {
        let mut c = Coalescer::new();
        c.push(renamed("x", "y"));
        let want = WatchEvent::Changes(vec![Change::Removed(o("x")), Change::Removed(o("y"))]);
        assert_eq!(c.flush(&mut stat_from(&[])), [want]);
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
        assert_eq!(c.flush(&mut stat_from(&["b", "c"])), [want]);
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
        assert_eq!(c.flush(&mut stat_from(&["c"])), [want]);
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
                let flushed =
                    c.flush(&mut |n: &OsStr| present(n).then(|| Entry::new(n, EntryKind::File)));
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

    #[test]
    fn overflow_becomes_rescan_and_resets() {
        let mut c = Coalescer::new();
        c.push(DirEvent::Changed(o("a")));
        c.push(DirEvent::Overflow);
        assert_eq!(c.flush(&mut stat_from(&[])), [WatchEvent::Rescan]);
        assert_eq!(c.flush(&mut stat_from(&[])), no_events());
    }

    #[test]
    fn too_many_changes_become_rescan_without_stat() {
        let mut c = Coalescer::new();
        for i in 0..=MAX_PENDING_CHANGES {
            c.push(DirEvent::Changed(OsString::from(i.to_string())));
        }
        assert_eq!(
            c.flush(&mut |_: &OsStr| panic!("stat must not run")),
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
        let flushed = c.flush(&mut |_: &OsStr| None);
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
            c.flush(&mut |_: &OsStr| panic!("stat must not run")),
            [WatchEvent::Rescan]
        );
    }

    #[test]
    fn ended_wins_over_pending_changes() {
        let mut c = Coalescer::new();
        c.push(DirEvent::Changed(o("a")));
        c.push(DirEvent::Ended);
        assert_eq!(c.flush(&mut stat_from(&[])), [WatchEvent::Ended]);
        assert_eq!(c.flush(&mut stat_from(&[])), no_events());
    }

    #[test]
    fn ended_wins_over_rescan() {
        let mut c = Coalescer::new();
        c.push(DirEvent::Overflow);
        c.push(DirEvent::Ended);
        assert_eq!(c.flush(&mut stat_from(&[])), [WatchEvent::Ended]);
    }

    #[test]
    fn empty_flush_is_empty() {
        assert_eq!(Coalescer::new().flush(&mut stat_from(&[])), no_events());
    }
}
