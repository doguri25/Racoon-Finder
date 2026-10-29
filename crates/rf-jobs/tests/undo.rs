#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use common::*;
use rf_jobs::testkit::{Fault, FaultyProvider, tree, wait_finished, write_tree};
use rf_jobs::{
    CreateKind, DefaultLanes, EngineConfig, JobEngine, JobEvent, JobSpec, LaneId, LanePolicy,
    NonInteractive, Outcome, Stage, TransferMode, TransferOptions, UndoHistory, UndoRecord,
};
use rf_vfs::Vfs;

/// Long enough for the real trash, which can take up to 20 s a call on macOS.
const WAIT: Duration = Duration::from_secs(60);

fn engine_on(vfs: Vfs) -> (JobEngine, Receiver<JobEvent>) {
    JobEngine::start(EngineConfig::new(vfs, Arc::new(NonInteractive)))
}

fn done(e: &JobEngine, rx: &Receiver<JobEvent>, spec: JobSpec) -> Arc<rf_jobs::JobReport> {
    let id = e.submit(spec);
    wait_finished(rx, id, WAIT)
}

fn undo(e: &JobEngine, rx: &Receiver<JobEvent>) -> Arc<rf_jobs::JobReport> {
    let id = e.undo().expect("something to undo");
    wait_finished(rx, id, WAIT)
}

fn redo(e: &JobEngine, rx: &Receiver<JobEvent>) -> Arc<rf_jobs::JobReport> {
    let id = e.redo().expect("something to redo");
    wait_finished(rx, id, WAIT)
}

fn rename(f: &Fx, a: &str, b: &str) -> JobSpec {
    JobSpec::Rename {
        from: v(f.src.join(a)),
        to: v(f.src.join(b)),
    }
}

#[test]
fn history_is_bounded_and_new_actions_clear_redo() {
    let r = |n: &str| UndoRecord::Renamed {
        from: v(std::env::temp_dir().join(n)),
        to: v(std::env::temp_dir().join(format!("{n}2"))),
    };
    let mut h = UndoHistory::new(2);
    h.push(r("a"));
    h.push(r("b"));
    h.push(r("c"));
    assert_eq!(h.undo_list(), [r("b"), r("c")]);
    assert_eq!(h.pop_undo(), Some(r("c")));
    h.push_undone(r("c-rev"));
    assert_eq!(h.redo_list(), [r("c-rev")]);
    assert_eq!(h.pop_redo(), Some(r("c-rev")));
    h.push_redone(r("c"));
    assert_eq!(h.undo_list(), [r("b"), r("c")]);
    h.pop_undo();
    h.push_undone(r("x"));
    h.push(r("d"));
    assert_eq!(h.redo_list(), Vec::new());
}

#[test]
fn undo_and_redo_a_rename() {
    let f = fx();
    std::fs::write(f.src.join("a"), "A").unwrap();
    let (e, rx) = engine_on(Vfs::with_local());
    assert_eq!(e.undo(), None);
    done(&e, &rx, rename(&f, "a", "b"));
    assert_eq!(e.undo_history().len(), 1);
    assert_eq!(undo(&e, &rx).outcome, Outcome::Completed);
    assert!(f.src.join("a").exists() && !f.src.join("b").exists());
    assert_eq!((e.undo_history().len(), e.redo_history().len()), (0, 1));
    redo(&e, &rx);
    assert!(f.src.join("b").exists());
    assert_eq!((e.undo_history().len(), e.redo_history().len()), (1, 0));
    assert!(e.shutdown(WAIT));
}

#[test]
fn a_new_action_clears_redo() {
    let f = fx();
    std::fs::write(f.src.join("a"), "A").unwrap();
    std::fs::write(f.src.join("c"), "C").unwrap();
    let (e, rx) = engine_on(Vfs::with_local());
    done(&e, &rx, rename(&f, "a", "b"));
    undo(&e, &rx);
    assert_eq!(e.redo_history().len(), 1);
    done(&e, &rx, rename(&f, "c", "d"));
    assert_eq!(e.redo_history(), Vec::new());
    assert!(e.shutdown(WAIT));
}

#[test]
fn undo_moves_a_folder_back_on_the_same_volume() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A")]);
    let before = tree(&f.src);
    let (e, rx) = engine_on(Vfs::with_local());
    done(
        &e,
        &rx,
        spec(
            TransferMode::Move,
            &[f.src.join("dir")],
            &f.dst,
            TransferOptions::default(),
        ),
    );
    assert_eq!(undo(&e, &rx).outcome, Outcome::Completed);
    assert_eq!(tree(&f.src), before);
    assert_eq!(tree(&f.dst), BTreeMap::new());
    redo(&e, &rx);
    assert!(f.dst.join("dir").join("a.txt").exists() && !f.src.join("dir").exists());
    assert!(e.shutdown(WAIT));
}

#[test]
fn undo_of_a_cross_volume_move_restores_the_tree() {
    let f = fx();
    write_tree(
        &f.src,
        &[
            ("dir/a.txt", "A"),
            ("dir/sub/b.txt", "B"),
            ("dir/empty/", ""),
        ],
    );
    let before = tree(&f.src);
    let p = FaultyProvider::local(vec![Fault::CrossDevice]);
    let (e, rx) = engine_on(p.vfs());
    done(
        &e,
        &rx,
        spec(
            TransferMode::Move,
            &[f.src.join("dir")],
            &f.dst,
            TransferOptions::default(),
        ),
    );
    assert!(!f.src.join("dir").exists());
    assert_eq!(undo(&e, &rx).outcome, Outcome::Completed);
    assert_eq!(tree(&f.src), before);
    assert_eq!(tree(&f.dst), BTreeMap::new());
    assert!(e.shutdown(WAIT));
}

#[test]
fn undo_of_a_merge_recreates_the_removed_source_folder() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A")]);
    write_tree(&f.dst, &[("dir/other.txt", "O")]);
    let (before_src, before_dst) = (tree(&f.src), tree(&f.dst));
    let (e, rx) = engine_on(Vfs::with_local());
    done(
        &e,
        &rx,
        spec(
            TransferMode::Move,
            &[f.src.join("dir")],
            &f.dst,
            TransferOptions::default(),
        ),
    );
    assert!(!f.src.join("dir").exists());
    assert_eq!(undo(&e, &rx).outcome, Outcome::Completed);
    assert_eq!((tree(&f.src), tree(&f.dst)), (before_src, before_dst));
    assert!(e.shutdown(WAIT));
}

#[test]
fn undo_of_a_new_folder_removes_it_only_while_empty() {
    let f = fx();
    let (e, rx) = engine_on(Vfs::with_local());
    let make = || JobSpec::Create {
        parent: v(&f.src),
        name: "n".into(),
        kind: CreateKind::Dir,
    };
    done(&e, &rx, make());
    assert_eq!(undo(&e, &rx).outcome, Outcome::Completed);
    assert!(!f.src.join("n").exists());
    done(&e, &rx, make());
    std::fs::write(f.src.join("n").join("work.txt"), "w").unwrap();
    let rep = undo(&e, &rx);
    assert_eq!(
        (rep.outcome, rep.failures[0].stage),
        (Outcome::CompletedWithErrors, Stage::Undo)
    );
    assert!(f.src.join("n").join("work.txt").exists());
    assert!(e.shutdown(WAIT));
}

#[test]
fn undo_routes_quick_records_to_the_quick_lane() {
    let p = |n: &str| v(std::env::temp_dir().join(n));
    assert_eq!(
        DefaultLanes.lane(&JobSpec::Undo(UndoRecord::Renamed {
            from: p("a"),
            to: p("b")
        })),
        LaneId::QUICK
    );
    let moved = UndoRecord::Moved {
        moves: vec![],
        created_dirs: vec![],
        removed_dirs: vec![],
        overwritten: 0,
        copied_dirs: vec![],
    };
    assert_eq!(DefaultLanes.lane(&JobSpec::Undo(moved)), LaneId::MAIN);
}

#[cfg(target_os = "linux")]
#[test]
fn undo_of_a_copy_without_trash_deletes_nothing() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "A").unwrap();
    let (e, rx) = engine_on(Vfs::with_local());
    done(&e, &rx, copy(&[f.src.join("a.txt")], &f.dst));
    let rep = undo(&e, &rx);
    assert_eq!(
        (rep.outcome, rep.failures[0].kind),
        (
            Outcome::CompletedWithErrors,
            rf_core::ErrorKind::Unsupported
        )
    );
    assert!(f.dst.join("a.txt").exists());
    assert!(e.shutdown(WAIT));
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn undo_of_a_copy_trashes_copies_but_keeps_edited_ones() {
    let f = fx();
    let (a, b) = (unique_name("a.txt"), unique_name("b.txt"));
    std::fs::write(f.src.join(&a), "A").unwrap();
    std::fs::write(f.src.join(&b), "B").unwrap();
    let (e, rx) = engine_on(Vfs::with_local());
    done(&e, &rx, copy(&[f.src.join(&a), f.src.join(&b)], &f.dst));
    std::fs::write(f.dst.join(&b), "edited after the copy").unwrap();
    let rep = undo(&e, &rx);
    assert!(!f.dst.join(&a).exists());
    assert_eq!(
        std::fs::read_to_string(f.dst.join(&b)).unwrap(),
        "edited after the copy"
    );
    assert_eq!(rep.failures.len(), 1);
    let Some(UndoRecord::Trashed { items }) = &rep.undo else {
        panic!("{:?}", rep.undo)
    };
    for item in items {
        purge(
            item.location
                .as_ref()
                .expect("trash location")
                .as_local()
                .unwrap(),
        );
    }
    assert!(e.shutdown(WAIT));
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn undo_of_a_trash_puts_the_file_back() {
    let f = fx();
    let n = unique_name("t.txt");
    std::fs::write(f.src.join(&n), "T").unwrap();
    let (e, rx) = engine_on(Vfs::with_local());
    let del = rf_jobs::DeleteSpec {
        paths: vec![v(f.src.join(&n))],
        method: rf_jobs::DeleteMethod::Trash,
        errors: rf_jobs::ErrorPolicy::Continue,
    };
    done(&e, &rx, JobSpec::Delete(del));
    assert!(!f.src.join(&n).exists());
    assert_eq!(undo(&e, &rx).outcome, Outcome::Completed);
    assert_eq!(std::fs::read_to_string(f.src.join(&n)).unwrap(), "T");
    let rep = redo(&e, &rx);
    assert!(!f.src.join(&n).exists());
    let Some(UndoRecord::Trashed { items }) = &rep.undo else {
        panic!("{:?}", rep.undo)
    };
    purge(
        items[0]
            .location
            .as_ref()
            .expect("trash location")
            .as_local()
            .unwrap(),
    );
    assert!(e.shutdown(WAIT));
}

// What follows goes beyond the brief: the rules the controller added for this task, checked
// on every system with a trash of the tests' own (`Bin`).

mod beyond {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;

    use rf_core::{Entry, ErrorKind, Result, Scheme, VPath};
    use rf_jobs::testkit::Node;
    use rf_jobs::{ConflictPolicy, DeleteMethod, DeleteSpec, ErrorPolicy, JobReport, TrashedItem};
    use rf_vfs::{
        CancelToken, Capabilities, DeleteMode, Deleted, DiskSpace, LinkTarget, LocalProvider,
        MetadataPatch, Provider, ReadStream, RenameMode, WatchHandle, WatchSink, WriteMode,
        WriteStream,
    };

    use super::*;

    /// The local provider with a trash of its own, on every system: a trashed item is renamed
    /// into the folder `holds` under a new name, and is put back from there without replacing
    /// anything. Every other call goes to the local provider.
    #[derive(Debug)]
    struct Bin {
        local: LocalProvider,
        holds: PathBuf,
        trashed: AtomicUsize,
    }

    impl Bin {
        /// A provider whose trash is the new folder `bin` in the fixture's root.
        fn new(f: &Fx) -> Arc<Bin> {
            let bin = f.root.join("bin");
            std::fs::create_dir(&bin).unwrap();
            Arc::new(Bin {
                local: LocalProvider::new(),
                holds: bin,
                trashed: AtomicUsize::new(0),
            })
        }

        fn vfs(self: &Arc<Self>) -> Vfs {
            let mut vfs = Vfs::new();
            vfs.register(Arc::clone(self) as Arc<dyn Provider>);
            vfs
        }

        /// What the trash holds.
        fn held(&self) -> BTreeMap<String, Node> {
            tree(&self.holds)
        }
    }

    impl Provider for Bin {
        fn scheme(&self) -> Scheme {
            self.local.scheme()
        }
        fn capabilities(&self, path: &VPath) -> Capabilities {
            self.local.capabilities(path)
        }
        fn list(
            &self,
            dir: &VPath,
            cancel: &CancelToken,
            sink: &mut dyn FnMut(Vec<Entry>),
        ) -> Result<()> {
            self.local.list(dir, cancel, sink)
        }
        fn stat(&self, path: &VPath) -> Result<Entry> {
            self.local.stat(path)
        }
        fn create_dir(&self, path: &VPath) -> Result<()> {
            self.local.create_dir(path)
        }
        fn create_file(&self, path: &VPath) -> Result<()> {
            self.local.create_file(path)
        }
        fn rename(&self, from: &VPath, to: &VPath, mode: RenameMode) -> Result<()> {
            self.local.rename(from, to, mode)
        }
        fn delete(&self, path: &VPath, mode: DeleteMode) -> Result<Deleted> {
            if mode == DeleteMode::Permanent {
                return self.local.delete(path, mode);
            }
            let n = self.trashed.fetch_add(1, Ordering::SeqCst);
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let location = v(self.holds.join(format!("{n}-{name}")));
            self.local.rename(path, &location, RenameMode::NoReplace)?;
            Ok(Deleted::Trashed {
                location: Some(location),
            })
        }
        fn open_read(&self, path: &VPath) -> Result<Box<dyn ReadStream>> {
            self.local.open_read(path)
        }
        fn open_write(&self, path: &VPath, mode: WriteMode) -> Result<Box<dyn WriteStream>> {
            self.local.open_write(path, mode)
        }
        fn space(&self, path: &VPath) -> Result<Option<DiskSpace>> {
            self.local.space(path)
        }
        fn watch(&self, dir: &VPath, sink: WatchSink) -> Result<WatchHandle> {
            self.local.watch(dir, sink)
        }
        fn stat_target(&self, path: &VPath) -> Result<Entry> {
            self.local.stat_target(path)
        }
        fn read_link(&self, path: &VPath) -> Result<LinkTarget> {
            self.local.read_link(path)
        }
        fn create_symlink(&self, link: &VPath, target: &LinkTarget) -> Result<()> {
            self.local.create_symlink(link, target)
        }
        fn set_metadata(&self, path: &VPath, patch: &MetadataPatch) -> Result<()> {
            self.local.set_metadata(path, patch)
        }
        fn copy_extras(&self, from: &VPath, to: &VPath) -> Result<()> {
            self.local.copy_extras(from, to)
        }
        fn remove_dir(&self, path: &VPath) -> Result<()> {
            self.local.remove_dir(path)
        }
        fn delete_leaf(&self, path: &VPath) -> Result<()> {
            self.local.delete_leaf(path)
        }
        fn sync_dir(&self, path: &VPath) -> Result<()> {
            self.local.sync_dir(path)
        }
        fn fs_name(&self, path: &VPath) -> Option<String> {
            self.local.fs_name(path)
        }
        fn restore_trashed(&self, location: &VPath, original: &VPath) -> Result<()> {
            self.local.rename(location, original, RenameMode::NoReplace)
        }
    }

    /// Waits long enough that what a test changes next is stamped later than the job that has
    /// just ended: file systems stamp a change with a clock that can lag behind
    /// `SystemTime::now` by one tick (about 16 ms on Windows).
    fn later() {
        std::thread::sleep(Duration::from_millis(100));
    }

    /// The failures of `rep` that left an item in place because it changed after the copy, by
    /// the path of the item.
    fn left_as_changed(rep: &JobReport) -> Vec<VPath> {
        rep.failures
            .iter()
            .filter(|failure| {
                failure.stage == Stage::Undo
                    && failure.kind == ErrorKind::Unsupported
                    && failure
                        .message
                        .ends_with("changed after it was copied; left in place")
            })
            .map(|failure| failure.source.clone().unwrap())
            .collect()
    }

    fn trash(paths: &[PathBuf]) -> JobSpec {
        JobSpec::Delete(DeleteSpec {
            paths: paths.iter().map(v).collect(),
            method: DeleteMethod::Trash,
            errors: ErrorPolicy::Continue,
        })
    }

    fn originals(rep: &JobReport) -> Vec<VPath> {
        match &rep.undo {
            Some(UndoRecord::Trashed { items } | UndoRecord::Restored { items }) => {
                items.iter().map(|item| item.original.clone()).collect()
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_copy_records_when_it_finished() {
        let f = fx();
        std::fs::write(f.src.join("a.txt"), "A").unwrap();
        let before = SystemTime::now();
        let rep = run(&copy(&[f.src.join("a.txt")], &f.dst));
        let after = SystemTime::now();
        let Some(UndoRecord::Copied { at: Some(at), .. }) = rep.undo else {
            panic!("{:?}", rep.undo)
        };
        assert!(before <= at && at <= after);
    }

    #[test]
    fn undo_of_a_copy_trashes_what_is_unchanged_and_redo_brings_it_back() {
        let f = fx();
        write_tree(
            &f.src,
            &[
                ("a.txt", "A"),
                ("b.txt", "B"),
                ("dir/x.txt", "X"),
                ("dir/sub/", ""),
            ],
        );
        let b = Bin::new(&f);
        let (e, rx) = engine_on(b.vfs());
        let sources = [f.src.join("a.txt"), f.src.join("b.txt"), f.src.join("dir")];
        done(&e, &rx, copy(&sources, &f.dst));
        std::fs::write(f.dst.join("b.txt"), "edited after the copy").unwrap();
        let rep = undo(&e, &rx);
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        assert_eq!(left_as_changed(&rep), [v(f.dst.join("b.txt"))]);
        assert_eq!(rep.failures.len(), 1, "{:?}", rep.failures);
        let mut left = BTreeMap::new();
        left.insert(
            "b.txt".to_owned(),
            Node::File(b"edited after the copy".to_vec()),
        );
        assert_eq!(tree(&f.dst), left);
        assert_eq!(
            originals(&rep),
            [v(f.dst.join("a.txt")), v(f.dst.join("dir"))]
        );
        // Redo puts back what the undo trashed, and a second undo trashes it again.
        let rep = redo(&e, &rx);
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert!(f.dst.join("a.txt").exists() && f.dst.join("dir").join("x.txt").exists());
        assert_eq!(b.held(), BTreeMap::new());
        let rep = undo(&e, &rx);
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert_eq!(tree(&f.dst), left);
        assert_eq!(e.redo_history().len(), 1);
        assert!(e.shutdown(WAIT));
    }

    #[test]
    fn a_copied_folder_with_any_change_inside_is_left_in_place() {
        type Change = fn(&Path);
        let changes: [(&str, Change); 4] = [
            ("a file deep inside edited", |dir| {
                std::fs::write(dir.join("sub").join("deep.txt"), "D").unwrap();
            }),
            ("a file added deep inside", |dir| {
                std::fs::write(dir.join("sub").join("new.txt"), "N").unwrap();
            }),
            ("a file removed deep inside", |dir| {
                std::fs::remove_file(dir.join("sub").join("deep.txt")).unwrap();
            }),
            ("a file removed from the folder itself", |dir| {
                std::fs::remove_file(dir.join("a.txt")).unwrap();
            }),
        ];
        for (what, change) in changes {
            let f = fx();
            write_tree(
                &f.src,
                &[
                    ("dir/a.txt", "A"),
                    ("dir/sub/deep.txt", "d"),
                    ("dir/empty/", ""),
                ],
            );
            let b = Bin::new(&f);
            let (e, rx) = engine_on(b.vfs());
            done(&e, &rx, copy(&[f.src.join("dir")], &f.dst));
            later();
            change(&f.dst.join("dir"));
            let before = tree(&f.dst);
            let rep = undo(&e, &rx);
            assert_eq!(left_as_changed(&rep), [v(f.dst.join("dir"))], "{what}");
            assert_eq!(tree(&f.dst), before, "{what}");
            assert_eq!(b.held(), BTreeMap::new(), "{what}");
            assert!(e.shutdown(WAIT));
        }
    }

    #[test]
    fn a_future_dated_file_keeps_its_copied_folder_but_not_itself() {
        let f = fx();
        write_tree(&f.src, &[("dir/future.txt", "F"), ("future.txt", "G")]);
        let tomorrow = SystemTime::now() + Duration::from_hours(24);
        for path in [
            f.src.join("dir").join("future.txt"),
            f.src.join("future.txt"),
        ] {
            rf_platform::set_times(&v(&path), Some(tomorrow), None, None).unwrap();
        }
        let b = Bin::new(&f);
        let (e, rx) = engine_on(b.vfs());
        let sources = [f.src.join("dir"), f.src.join("future.txt")];
        done(&e, &rx, copy(&sources, &f.dst));
        let rep = undo(&e, &rx);
        // Nothing tells whether the copy in the folder was changed after the copy, so the
        // folder stays; the file at the top matches its own record and goes.
        assert_eq!(left_as_changed(&rep), [v(f.dst.join("dir"))]);
        assert!(f.dst.join("dir").join("future.txt").exists());
        assert!(!f.dst.join("future.txt").exists());
        assert_eq!(originals(&rep), [v(f.dst.join("future.txt"))]);
        assert!(e.shutdown(WAIT));
    }

    #[test]
    fn undo_of_an_overwriting_copy_warns_and_leaves_the_overwritten_file() {
        let f = fx();
        write_tree(&f.src, &[("x.txt", "new"), ("y.txt", "Y")]);
        write_tree(&f.dst, &[("x.txt", "old")]);
        let b = Bin::new(&f);
        let (e, rx) = engine_on(b.vfs());
        let options = TransferOptions {
            conflict: ConflictPolicy::Overwrite,
            ..TransferOptions::default()
        };
        let sources = [f.src.join("x.txt"), f.src.join("y.txt")];
        done(&e, &rx, spec(TransferMode::Copy, &sources, &f.dst, options));
        let rep = undo(&e, &rx);
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert_eq!(std::fs::read_to_string(f.dst.join("x.txt")).unwrap(), "new");
        assert!(!f.dst.join("y.txt").exists());
        let warnings: Vec<_> = rep.warnings.iter().map(|w| (w.stage, w.kind)).collect();
        assert_eq!(warnings, [(Stage::Undo, ErrorKind::Unsupported)]);
        assert!(e.shutdown(WAIT));
    }

    #[test]
    fn undo_of_a_trash_never_replaces_what_took_its_place() {
        let f = fx();
        write_tree(&f.src, &[("t1.txt", "T1"), ("t2.txt", "T2")]);
        let b = Bin::new(&f);
        let (e, rx) = engine_on(b.vfs());
        done(
            &e,
            &rx,
            trash(&[f.src.join("t1.txt"), f.src.join("t2.txt")]),
        );
        std::fs::write(f.src.join("t2.txt"), "new").unwrap();
        let rep = undo(&e, &rx);
        assert_eq!(std::fs::read_to_string(f.src.join("t1.txt")).unwrap(), "T1");
        assert_eq!(
            std::fs::read_to_string(f.src.join("t2.txt")).unwrap(),
            "new"
        );
        assert_eq!(rep.failures.len(), 1, "{:?}", rep.failures);
        let failure = &rep.failures[0];
        assert_eq!(
            (failure.stage, failure.kind, failure.target.clone()),
            (
                Stage::Undo,
                ErrorKind::AlreadyExists,
                Some(v(f.src.join("t2.txt")))
            )
        );
        assert_eq!(b.held().len(), 1);
        assert_eq!(originals(&rep), [v(f.src.join("t1.txt"))]);
        // Redo trashes again only what came back.
        let rep = redo(&e, &rx);
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert!(!f.src.join("t1.txt").exists());
        assert_eq!(
            std::fs::read_to_string(f.src.join("t2.txt")).unwrap(),
            "new"
        );
        assert_eq!(b.held().len(), 2);
        assert!(e.shutdown(WAIT));
    }

    #[test]
    fn undo_of_a_trash_without_its_location_reports_it() {
        let f = fx();
        let original = v(f.src.join("gone.txt"));
        let record = UndoRecord::Trashed {
            items: vec![TrashedItem {
                original: original.clone(),
                location: None,
            }],
        };
        let rep = run(&JobSpec::Undo(record));
        let failure = &rep.failures[0];
        assert_eq!(
            (rep.outcome, failure.stage, failure.kind),
            (
                Outcome::CompletedWithErrors,
                Stage::Undo,
                ErrorKind::Unsupported
            )
        );
        assert_eq!(
            failure.message,
            format!("not supported: trash location of {original} is unknown")
        );
        assert_eq!(rep.undo, None);
    }

    #[test]
    fn undo_of_a_new_file_removes_it_only_while_empty_and_redo_never_replaces() {
        let f = fx();
        let (e, rx) = engine_on(Vfs::with_local());
        let n = f.src.join("n.txt");
        let make = JobSpec::Create {
            parent: v(&f.src),
            name: "n.txt".into(),
            kind: CreateKind::File,
        };
        done(&e, &rx, make);
        assert_eq!(undo(&e, &rx).outcome, Outcome::Completed);
        assert!(!n.exists());
        assert_eq!(redo(&e, &rx).outcome, Outcome::Completed);
        assert_eq!(std::fs::read(&n).unwrap(), b"");
        std::fs::write(&n, "work").unwrap();
        let rep = undo(&e, &rx);
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        assert_eq!(
            rep.failures[0].message,
            format!("not supported: {} is not empty; left in place", v(&n))
        );
        assert_eq!(std::fs::read_to_string(&n).unwrap(), "work");
        assert!(e.shutdown(WAIT));
        // Doing a removed file again makes it at its own path, never over what is there.
        let removed = UndoRecord::Removed {
            path: v(&n),
            kind: CreateKind::File,
        };
        let rep = run(&JobSpec::Undo(removed));
        let failure = &rep.failures[0];
        assert_eq!(
            (failure.stage, failure.kind),
            (Stage::Undo, ErrorKind::AlreadyExists)
        );
        assert_eq!(std::fs::read_to_string(&n).unwrap(), "work");
        assert_eq!(tree(&f.src).len(), 1);
    }

    #[test]
    fn undo_of_a_move_never_replaces_what_took_the_old_place() {
        let f = fx();
        write_tree(&f.src, &[("a.txt", "A")]);
        let (e, rx) = engine_on(Vfs::with_local());
        done(
            &e,
            &rx,
            spec(
                TransferMode::Move,
                &[f.src.join("a.txt")],
                &f.dst,
                TransferOptions::default(),
            ),
        );
        std::fs::write(f.src.join("a.txt"), "new").unwrap();
        let rep = undo(&e, &rx);
        assert_eq!((rep.outcome, rep.skipped), (Outcome::Completed, 1));
        assert_eq!(std::fs::read_to_string(f.src.join("a.txt")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(f.dst.join("a.txt")).unwrap(), "A");
        assert!(e.shutdown(WAIT));
    }

    #[test]
    fn a_job_without_an_undo_record_leaves_the_history_alone() {
        let f = fx();
        write_tree(&f.src, &[("a", "A"), ("c", "C")]);
        let (e, rx) = engine_on(Vfs::with_local());
        done(&e, &rx, rename(&f, "a", "b"));
        undo(&e, &rx);
        let gone = JobSpec::Delete(DeleteSpec {
            paths: vec![v(f.src.join("c"))],
            method: DeleteMethod::Permanent,
            errors: ErrorPolicy::Continue,
        });
        let rep = done(&e, &rx, gone);
        assert_eq!((rep.outcome, rep.undo.clone()), (Outcome::Completed, None));
        assert_eq!((e.undo_history().len(), e.redo_history().len()), (0, 1));
        assert!(e.shutdown(WAIT));
    }

    // Fix round 1: fail-safe cases.

    /// The record of a copy of `sources` into `dst`, made through `b`.
    fn copied(b: &Arc<Bin>, sources: &[PathBuf], dst: &Path) -> Vec<rf_jobs::CreatedItem> {
        let rep = run_on(&copy(sources, dst), &b.vfs(), &NonInteractive);
        let Some(UndoRecord::Copied { created, .. }) = rep.undo else {
            panic!("{:?}", rep.undo)
        };
        created
    }

    #[test]
    fn an_item_that_cannot_be_confirmed_unchanged_is_left_in_place() {
        let f = fx();
        write_tree(
            &f.src,
            &[
                ("dir/x.txt", "X"),
                ("a.txt", "A"),
                ("b.txt", "B"),
                ("c.txt", "C"),
            ],
        );
        let b = Bin::new(&f);
        let names = ["dir", "a.txt", "b.txt", "c.txt"];
        let sources: Vec<PathBuf> = names.iter().map(|name| f.src.join(name)).collect();
        let mut created = copied(&b, &sources, &f.dst);
        assert_eq!(created.len(), 4);
        // `a.txt` has no recorded time, as for a time before 1970.
        created[1].modified = None;
        // `b.txt` is replaced by another file of the same size and time.
        let (b_txt, b_new) = (f.dst.join("b.txt"), f.dst.join("b.new"));
        std::fs::write(&b_new, "B").unwrap();
        rf_platform::set_times(&v(&b_new), created[2].modified, None, None).unwrap();
        std::fs::rename(&b_new, &b_txt).unwrap();
        let before = tree(&f.dst);
        // The folder is in a record without the job's time.
        let record = UndoRecord::Copied {
            created,
            overwritten: 0,
            at: None,
        };
        let rep = run_on(&JobSpec::Undo(record), &b.vfs(), &NonInteractive);
        let left: Vec<VPath> = names[..3].iter().map(|name| v(f.dst.join(name))).collect();
        assert_eq!(left_as_changed(&rep), left);
        assert_eq!(rep.failures.len(), 3, "{:?}", rep.failures);
        // Only the file that is still as the copy left it goes.
        let mut kept = before;
        kept.remove("c.txt");
        assert_eq!(tree(&f.dst), kept);
        assert_eq!(originals(&rep), [v(f.dst.join("c.txt"))]);
    }

    /// A link in a copied folder counts by its own time and is never entered: what it leads to
    /// can change without making the folder count as changed, and the link goes to the trash
    /// with the folder while what it leads to stays as it is.
    #[cfg(unix)]
    #[test]
    fn a_link_in_a_copied_folder_is_not_followed() {
        let f = fx();
        write_tree(&f.root, &[("outside/o.txt", "O"), ("src/dir/a.txt", "A")]);
        let outside = f.root.join("outside");
        std::os::unix::fs::symlink(&outside, f.src.join("dir").join("link")).unwrap();
        let b = Bin::new(&f);
        let (e, rx) = engine_on(b.vfs());
        done(&e, &rx, copy(&[f.src.join("dir")], &f.dst));
        assert!(f.dst.join("dir").join("link").is_symlink());
        later();
        std::fs::write(outside.join("o.txt"), "edited after the copy").unwrap();
        let rep = undo(&e, &rx);
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert_eq!(tree(&f.dst), BTreeMap::new());
        let mut target = BTreeMap::new();
        target.insert(
            "o.txt".to_owned(),
            Node::File(b"edited after the copy".to_vec()),
        );
        assert_eq!(tree(&outside), target);
        assert!(e.shutdown(WAIT));
    }

    #[test]
    fn a_restore_whose_trash_entry_is_gone_names_the_trash_location() {
        let f = fx();
        write_tree(&f.src, &[("t.txt", "T")]);
        let b = Bin::new(&f);
        let (e, rx) = engine_on(b.vfs());
        let rep = done(&e, &rx, trash(&[f.src.join("t.txt")]));
        let Some(UndoRecord::Trashed { items }) = &rep.undo else {
            panic!("{:?}", rep.undo)
        };
        let location = items[0].location.clone().unwrap();
        // The trash was emptied.
        std::fs::remove_file(location.as_local().unwrap()).unwrap();
        let rep = undo(&e, &rx);
        assert_eq!(rep.failures.len(), 1, "{:?}", rep.failures);
        let failure = &rep.failures[0];
        assert_eq!(
            (failure.stage, failure.kind),
            (Stage::Undo, ErrorKind::NotFound)
        );
        assert_eq!(
            (failure.source.clone(), failure.target.clone()),
            (Some(location), Some(v(f.src.join("t.txt"))))
        );
        assert!(e.shutdown(WAIT));
    }

    /// An undo puts no question: the move back meets a pre-flight issue (a name FAT32 cannot
    /// hold) and an old place that is taken, and settles both without its resolver, which has
    /// no answer and would panic if asked.
    #[cfg(unix)]
    #[test]
    fn an_undo_asks_nothing() {
        let f = fx();
        write_tree(&f.src, &[("a?b.txt", "Q"), ("c.txt", "C")]);
        let sources = [f.src.join("a?b.txt"), f.src.join("c.txt")];
        let rep = run(&spec(
            TransferMode::Move,
            &sources,
            &f.dst,
            TransferOptions::default(),
        ));
        let record = rep.undo.expect("a move record");
        std::fs::write(f.src.join("c.txt"), "new").unwrap();
        let resolver = rf_jobs::testkit::ScriptedResolver::new();
        let fat = FaultyProvider::local(vec![Fault::FsName("FAT32".into())]);
        let rep = run_on(&JobSpec::Undo(record), &fat.vfs(), &resolver);
        assert_eq!(resolver.calls(), Vec::new());
        assert_ne!(rep.issues, Vec::new());
        assert_eq!((rep.outcome, rep.skipped), (Outcome::Completed, 1));
        assert_eq!(std::fs::read_to_string(f.src.join("a?b.txt")).unwrap(), "Q");
        assert_eq!(std::fs::read_to_string(f.src.join("c.txt")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(f.dst.join("c.txt")).unwrap(), "C");
    }
}

// Fix round 1: one undo or redo at a time, and the record of one that never runs goes back.

mod one_at_a_time {
    use rf_jobs::JobId;

    use super::*;

    /// Writes `a.txt`, `b.txt`, `c.txt`, `x` and `big.bin` in `src` and an empty folder
    /// `held`.
    fn fixture(f: &Fx) {
        write_tree(
            &f.root,
            &[
                ("src/a.txt", "A"),
                ("src/b.txt", "B"),
                ("src/c.txt", "C"),
                ("src/x", "X"),
                ("src/big.bin", "B"),
                ("held/", ""),
            ],
        );
    }

    /// The fixture, and an engine whose provider holds the first file it writes until it is
    /// released.
    fn engine(f: &Fx) -> (Arc<FaultyProvider>, JobEngine, Receiver<JobEvent>) {
        fixture(f);
        let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
        let (e, rx) = engine_on(p.vfs());
        (p, e, rx)
    }

    /// Moves `name` from `src` to `dst`; its undo runs on the main lane.
    fn move_one(f: &Fx, e: &JobEngine, rx: &Receiver<JobEvent>, name: &str) {
        let moved = spec(
            TransferMode::Move,
            &[f.src.join(name)],
            &f.dst,
            TransferOptions::default(),
        );
        assert_eq!(done(e, rx, moved).outcome, Outcome::Completed);
    }

    /// Moves `a.txt` from `src` to `dst`.
    fn move_a(f: &Fx, e: &JobEngine, rx: &Receiver<JobEvent>) {
        move_one(f, e, rx, "a.txt");
    }

    /// Submits a copy whose first write waits, so that it holds the main lane, and gives its ID.
    fn hold_the_main_lane(f: &Fx, p: &FaultyProvider, e: &JobEngine) -> JobId {
        let id = e.submit(copy(&[f.src.join("big.bin")], &f.root.join("held")));
        assert!(p.wait_held(WAIT));
        id
    }

    /// Cancels the queued job `id` and waits for its end.
    fn cancel(e: &JobEngine, rx: &Receiver<JobEvent>, id: JobId) {
        assert!(e.cancel(id));
        assert_eq!(wait_finished(rx, id, WAIT).outcome, Outcome::Cancelled);
    }

    /// Lets the held copy `held` finish, and the engine end.
    fn finish(p: &FaultyProvider, e: &JobEngine, rx: &Receiver<JobEvent>, held: JobId) {
        p.release();
        assert_eq!(wait_finished(rx, held, WAIT).outcome, Outcome::Completed);
        assert!(e.shutdown(WAIT));
    }

    #[test]
    fn a_record_goes_back_only_in_the_generation_it_was_taken_in() {
        let f = fx();
        let (p, e, rx) = engine(&f);
        move_a(&f, &e, &rx);
        move_one(&f, &e, &rx, "b.txt");
        move_one(&f, &e, &rx, "c.txt");
        assert_eq!(undo(&e, &rx).outcome, Outcome::Completed);
        let (undos, redos) = (e.undo_history(), e.redo_history());
        assert_eq!((undos.len(), redos.len()), (2, 1));
        let held = hold_the_main_lane(&f, &p, &e);
        // An undo that never runs: its record goes back on top, and the redo list stays.
        let first = e.undo().expect("something to undo");
        cancel(&e, &rx, first);
        assert_eq!(
            (e.undo_history(), e.redo_history()),
            (undos.clone(), redos.clone())
        );
        // A redo that never runs: likewise, and the undo list stays.
        let first = e.redo().expect("something to redo");
        cancel(&e, &rx, first);
        assert_eq!((e.undo_history(), e.redo_history()), (undos.clone(), redos));
        // A new action of the user's in between: the record taken before it is dropped.
        let first = e.undo().expect("something to undo");
        done(&e, &rx, rename(&f, "x", "y"));
        cancel(&e, &rx, first);
        let after = e.undo_history();
        assert_eq!((after.len(), &after[0]), (2, &undos[0]));
        assert!(matches!(after[1], UndoRecord::Renamed { .. }));
        assert_eq!(e.redo_history(), Vec::new());
        finish(&p, &e, &rx, held);
    }

    #[test]
    fn one_undo_at_a_time_and_a_cancelled_one_keeps_its_record() {
        let f = fx();
        let (p, e, rx) = engine(&f);
        // Two records of the main lane: the one put back must go back on top.
        move_a(&f, &e, &rx);
        move_one(&f, &e, &rx, "b.txt");
        let history = e.undo_history();
        assert_eq!(history.len(), 2);
        let held = hold_the_main_lane(&f, &p, &e);
        let first = e.undo().expect("something to undo");
        assert_eq!((e.undo(), e.redo()), (None, None));
        cancel(&e, &rx, first);
        assert_eq!(e.undo_history(), history);
        // The next undo may start again, takes the same record, and is put back again.
        let again = e.undo().expect("the record is back");
        assert_eq!(e.undo_history(), &history[..1]);
        assert_eq!(e.undo(), None);
        cancel(&e, &rx, again);
        assert_eq!(e.undo_history(), history);
        assert!(f.dst.join("a.txt").exists() && f.dst.join("b.txt").exists());
        finish(&p, &e, &rx, held);
    }

    // Final review B.

    #[test]
    fn an_undo_that_runs_after_a_new_action_leaves_nothing_to_redo() {
        let f = fx();
        let (p, e, rx) = engine(&f);
        move_a(&f, &e, &rx);
        let held = hold_the_main_lane(&f, &p, &e);
        let undone = e.undo().expect("something to undo");
        // A job the user runs meanwhile, on the quick lane, notes its record.
        done(&e, &rx, rename(&f, "x", "y"));
        p.release();
        assert_eq!(wait_finished(&rx, held, WAIT).outcome, Outcome::Completed);
        // The undo runs all the same, after the new action.
        assert_eq!(wait_finished(&rx, undone, WAIT).outcome, Outcome::Completed);
        assert!(f.src.join("a.txt").exists() && !f.dst.join("a.txt").exists());
        // Nothing undone before a new action can be redone, so its reverse is dropped; the
        // records of the user's jobs (the rename and the held copy) stay.
        assert_eq!(e.redo_history(), Vec::new());
        assert_eq!(e.undo_history().len(), 2);
        assert!(e.shutdown(WAIT));
    }

    /// Panics, once, when it is asked for the lane of an undo.
    #[derive(Debug)]
    struct PanicsOnAnUndo(std::sync::atomic::AtomicBool);

    impl LanePolicy for PanicsOnAnUndo {
        fn lane(&self, spec: &JobSpec) -> LaneId {
            let undo = matches!(spec, JobSpec::Undo(_));
            let first = undo && self.0.swap(false, std::sync::atomic::Ordering::SeqCst);
            assert!(!first, "a bug in the policy");
            DefaultLanes.lane(spec)
        }
    }

    #[test]
    fn a_lane_policy_that_panics_gives_the_record_back() {
        let f = fx();
        fixture(&f);
        let mut config = EngineConfig::new(Vfs::with_local(), Arc::new(NonInteractive));
        config.lanes = Arc::new(PanicsOnAnUndo(true.into()));
        let (e, rx) = JobEngine::start(config);
        done(&e, &rx, rename(&f, "a.txt", "a2.txt"));
        done(&e, &rx, rename(&f, "b.txt", "b2.txt"));
        let history = e.undo_history();
        let panicking = e.clone();
        assert!(std::thread::spawn(move || panicking.undo()).join().is_err());
        // The record is back on top, and the next undo may start.
        assert_eq!(e.undo_history(), history);
        assert_eq!(undo(&e, &rx).outcome, Outcome::Completed);
        assert!(f.src.join("b.txt").exists() && f.src.join("a2.txt").exists());
        assert!(e.shutdown(WAIT));
    }

    #[test]
    fn a_queued_undo_ended_by_shutdown_keeps_its_record_and_is_recovered() {
        let f = fx();
        fixture(&f);
        let journal = f.root.join("state");
        let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
        let config = EngineConfig::new(p.vfs(), Arc::new(NonInteractive)).with_journal(&journal);
        let (e, rx) = JobEngine::start(config);
        move_a(&f, &e, &rx);
        let history = e.undo_history();
        let held = hold_the_main_lane(&f, &p, &e);
        let queued = e.undo().expect("something to undo");
        let releaser = {
            let p = Arc::clone(&p);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                p.release();
            })
        };
        assert!(e.shutdown(WAIT));
        releaser.join().unwrap();
        assert_eq!(wait_finished(&rx, queued, WAIT).outcome, Outcome::Cancelled);
        // In this process it never ran, so its record is back on the list.
        assert_eq!(e.undo_history(), history);
        assert!(f.dst.join("a.txt").exists());
        // The app's end cut it off: the next start offers it again, as never started, and the
        // copy that held the lane as started.
        let (again, rx) = engine_with_journal(&journal);
        let JobEvent::Recovered { jobs, .. } =
            rf_jobs::testkit::wait_for(&rx, WAIT, |ev| matches!(ev, JobEvent::Recovered { .. }))
        else {
            unreachable!()
        };
        let found: Vec<_> = jobs.iter().map(|j| (j.id, j.started)).collect();
        assert_eq!(found, [(held, true), (queued, false)]);
        assert_eq!(jobs[1].spec, JobSpec::Undo(history[0].clone()));
        assert!(again.shutdown(WAIT));
    }

    /// An engine on the local disk with the journal in `journal`.
    fn engine_with_journal(journal: &std::path::Path) -> (JobEngine, Receiver<JobEvent>) {
        let config = EngineConfig::new(Vfs::with_local(), Arc::new(NonInteractive));
        JobEngine::start(config.with_journal(journal))
    }

    #[test]
    fn one_redo_at_a_time_and_a_cancelled_one_keeps_its_record() {
        let f = fx();
        let (p, e, rx) = engine(&f);
        done(&e, &rx, rename(&f, "x", "y"));
        move_a(&f, &e, &rx);
        undo(&e, &rx);
        let (undos, redos) = (e.undo_history(), e.redo_history());
        assert_eq!((undos.len(), redos.len()), (1, 1));
        let held = hold_the_main_lane(&f, &p, &e);
        let first = e.redo().expect("something to redo");
        // The rename could be undone, but not while the redo waits.
        assert_eq!((e.redo(), e.undo()), (None, None));
        cancel(&e, &rx, first);
        assert_eq!(
            (e.undo_history(), e.redo_history()),
            (undos.clone(), redos.clone())
        );
        let again = e.redo().expect("the record is back");
        assert_eq!(e.redo(), None);
        cancel(&e, &rx, again);
        assert_eq!((e.undo_history(), e.redo_history()), (undos, redos));
        assert!(f.src.join("a.txt").exists());
        finish(&p, &e, &rx, held);
    }

    #[test]
    fn a_new_action_drops_the_record_of_a_cancelled_undo() {
        let f = fx();
        let (p, e, rx) = engine(&f);
        move_a(&f, &e, &rx);
        let held = hold_the_main_lane(&f, &p, &e);
        let first = e.undo().expect("something to undo");
        // A job the user runs meanwhile, on the quick lane, notes its record.
        done(&e, &rx, rename(&f, "x", "y"));
        let newer = e.undo_history();
        assert_eq!(newer.len(), 1);
        cancel(&e, &rx, first);
        // The move's record would now be undone before the newer rename: it is dropped.
        assert_eq!(e.undo_history(), newer);
        assert_eq!(e.redo_history(), Vec::new());
        finish(&p, &e, &rx, held);
    }

    #[test]
    fn a_new_action_drops_the_record_of_a_cancelled_redo() {
        let f = fx();
        let (p, e, rx) = engine(&f);
        move_a(&f, &e, &rx);
        undo(&e, &rx);
        assert_eq!(e.redo_history().len(), 1);
        let held = hold_the_main_lane(&f, &p, &e);
        let first = e.redo().expect("something to redo");
        done(&e, &rx, rename(&f, "x", "y"));
        let newer = e.undo_history();
        cancel(&e, &rx, first);
        // After a new action there is nothing to redo.
        assert_eq!((e.undo_history(), e.redo_history()), (newer, Vec::new()));
        finish(&p, &e, &rx, held);
    }
}
