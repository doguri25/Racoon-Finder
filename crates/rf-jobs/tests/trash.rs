//! The jobs on the trash as a place (NAV-16): putting items back, deleting them for good and
//! emptying it. They run against a freedesktop trash in a temporary folder (on Unix), never the
//! user's own.

mod common;

use common::*;
use rf_core::{ErrorKind, Os};
use rf_jobs::{
    DefaultLanes, DeleteMethod, DeleteSpec, ErrorPolicy, ExecEnv, JobId, JobKind, JobReport,
    JobSpec, LaneId, LanePolicy, NonInteractive, Outcome, Resolver, Stage, execute,
};
use rf_platform::TrashItem;
use rf_vfs::Vfs;

fn run_with(spec: &JobSpec, vfs: &Vfs, resolver: &dyn Resolver) -> JobReport {
    execute(
        JobId::generate(),
        spec,
        &ExecEnv::new(vfs, resolver).naming_os(Os::Windows),
    )
}

#[test]
fn the_jobs_on_the_trash_are_of_their_kinds_and_lanes() {
    let item = TrashItem::at(std::env::temp_dir().join("x"), false);
    let restore = JobSpec::RestoreFromTrash {
        items: vec![(item.clone(), std::env::temp_dir().join("y"))],
    };
    let delete = JobSpec::DeleteFromTrash {
        items: vec![item.clone()],
    };
    assert_eq!(restore.kind(), JobKind::Restore);
    assert_eq!(JobKind::Restore.name(), "restore");
    assert_eq!(delete.kind(), JobKind::Delete);
    assert_eq!(JobSpec::EmptyTrash.kind(), JobKind::Delete);
    for spec in [&restore, &delete, &JobSpec::EmptyTrash] {
        assert_eq!(DefaultLanes.lane(spec), LaneId::MAIN);
        assert!(!spec.journaled());
    }
    // Only the restore can be undone; the other two delete for good, as a permanent delete does.
    assert!(restore.undoable());
    assert!(!delete.undoable() && !JobSpec::EmptyTrash.undoable());
    let permanent = JobSpec::Delete(DeleteSpec {
        paths: Vec::new(),
        method: DeleteMethod::Permanent,
        errors: ErrorPolicy::Continue,
    });
    let trash = JobSpec::Delete(DeleteSpec {
        paths: Vec::new(),
        method: DeleteMethod::Trash,
        errors: ErrorPolicy::Continue,
    });
    assert!(!permanent.undoable() && trash.undoable());
}

#[test]
fn a_job_on_the_trash_is_stored_and_read_back_whole() {
    let mut item = TrashItem::at(std::env::temp_dir().join("bin").join("문서 a.txt"), false);
    item.original = Some(std::env::temp_dir().join("문서 a.txt"));
    item.deleted = Some(at(1_700_000_000));
    item.size = Some(7);
    #[cfg(unix)]
    let odd = {
        use std::os::unix::ffi::OsStrExt;
        std::env::temp_dir().join(std::ffi::OsStr::from_bytes(b"caf\xE9"))
    };
    #[cfg(not(unix))]
    let odd = std::env::temp_dir().join("café");
    for spec in [
        JobSpec::RestoreFromTrash {
            items: vec![(item.clone(), odd.clone())],
        },
        JobSpec::DeleteFromTrash {
            items: vec![item.clone(), TrashItem::at(odd.clone(), true)],
        },
        JobSpec::EmptyTrash,
    ] {
        let text = serde_json::to_string(&spec).unwrap();
        assert_eq!(serde_json::from_str::<JobSpec>(&text).unwrap(), spec);
    }
}

/// Without a provider of `shell://trash` nothing is done, and each item says so.
#[test]
fn without_the_trash_provider_nothing_is_done() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "A").unwrap();
    let item = TrashItem::at(file.clone(), false);
    let vfs = Vfs::with_local();
    let restore = JobSpec::RestoreFromTrash {
        items: vec![(item.clone(), dir.path().join("b.txt"))],
    };
    let delete = JobSpec::DeleteFromTrash { items: vec![item] };
    for (spec, stage) in [(restore, Stage::Restore), (delete, Stage::Delete)] {
        let rep = run_with(&spec, &vfs, &NonInteractive);
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        assert_eq!(
            (rep.failures[0].stage, rep.failures[0].kind),
            (stage, ErrorKind::Unsupported)
        );
        assert_eq!(rep.failures[0].source, Some(v(&file)));
        assert_eq!(rep.undo, None);
    }
    let rep = run_with(&JobSpec::EmptyTrash, &vfs, &NonInteractive);
    assert_eq!(
        rep.failures[0].target,
        Some(rf_vfs::ShellPlace::Trash.path())
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "A");
    assert!(!dir.path().join("b.txt").exists());
}

#[test]
fn only_the_failed_items_of_a_job_on_the_trash_are_tried_again() {
    let dir = std::env::temp_dir();
    let (a, b) = (
        TrashItem::at(dir.join("a"), false),
        TrashItem::at(dir.join("b"), false),
    );
    let failed = rf_jobs::ItemFailure {
        source: Some(v(dir.join("b"))),
        target: None,
        stage: Stage::Restore,
        kind: ErrorKind::NotFound,
        message: String::new(),
        reason: None,
    };
    let restore = JobSpec::RestoreFromTrash {
        items: vec![(a.clone(), dir.join("x")), (b.clone(), dir.join("y"))],
    };
    assert_eq!(
        rf_jobs::retry_spec(&restore, std::slice::from_ref(&failed)),
        Some(JobSpec::RestoreFromTrash {
            items: vec![(b.clone(), dir.join("y"))]
        })
    );
    let delete = JobSpec::DeleteFromTrash {
        items: vec![a, b.clone()],
    };
    assert_eq!(
        rf_jobs::retry_spec(&delete, std::slice::from_ref(&failed)),
        Some(JobSpec::DeleteFromTrash { items: vec![b] })
    );
    assert_eq!(rf_jobs::retry_spec(&delete, &[]), None);
    // A new empty would take what was trashed since the user was asked: never by itself.
    assert_eq!(rf_jobs::retry_spec(&JobSpec::EmptyTrash, &[failed]), None);
    assert_eq!(rf_jobs::retry_spec(&JobSpec::EmptyTrash, &[]), None);
}

/// A trash the system empties in one call (the Windows recycle bin), which fails here.
#[derive(Debug)]
struct AtOnce;

impl rf_vfs::TrashSource for AtOnce {
    fn list(&self) -> rf_core::Result<Vec<TrashItem>> {
        Ok(vec![TrashItem::at(std::env::temp_dir().join("x"), false)])
    }
    fn restore(&self, item: &TrashItem, _to: &std::path::Path) -> rf_core::Result<()> {
        Err(rf_core::Error::Unsupported(
            item.location.display().to_string(),
        ))
    }
    fn delete_forever(&self, item: &TrashItem) -> rf_core::Result<()> {
        Err(rf_core::Error::Unsupported(
            item.location.display().to_string(),
        ))
    }
    fn empty(&self) -> rf_core::Result<()> {
        Err(rf_core::Error::Unsupported("the bin refused".to_owned()))
    }
    fn empties_at_once(&self) -> bool {
        true
    }
}

/// A trash emptied in one call fails as a whole, and is never tried again by itself: the user
/// asks for a new empty, and is asked again.
#[test]
fn an_empty_at_once_is_not_tried_again() {
    let mut vfs = Vfs::with_local();
    vfs.register(std::sync::Arc::new(rf_vfs::ShellProvider::with_sources(
        std::sync::Arc::new(AtOnce),
        std::sync::Arc::new(rf_vfs::SystemDrives),
    )));
    let rep = run_with(&JobSpec::EmptyTrash, &vfs, &NonInteractive);
    assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
    assert_eq!(rep.failures.len(), 1);
    assert_eq!(
        rep.failures[0].target,
        Some(rf_vfs::ShellPlace::Trash.path())
    );
    assert_eq!(rep.retry_spec(), None);
}

/// A freedesktop trash in a temporary folder, the local disk whose trash it is, and the trash as
/// `shell://trash`.
#[cfg(unix)]
mod in_a_trash_of_its_own {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    use rf_jobs::testkit::{Fault, FaultyProvider, ScriptedResolver, wait_finished};
    use rf_jobs::{
        ConflictAction, ConflictAnswer, EngineConfig, FailureReason, JobEngine, JobEvent,
        TrashedItem, UndoRecord,
    };
    use rf_platform::FreedesktopTrash;
    use rf_vfs::{ShellProvider, SystemDrives};

    use super::*;

    const WAIT: Duration = Duration::from_secs(30);

    struct Bin {
        _dir: tempfile::TempDir,
        /// Where the test keeps the items it trashes.
        home: PathBuf,
        /// What lies beside the trash, which nothing may touch.
        outside: PathBuf,
        trash: FreedesktopTrash,
        vfs: Vfs,
    }

    fn bin() -> Bin {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let (home, outside) = (base.join("home"), base.join("outside"));
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep.txt"), "K").unwrap();
        let root = base.join("data").join("Trash");
        let trash = FreedesktopTrash::new(&root);
        let mut vfs = Vfs::new();
        vfs.register(FaultyProvider::local(vec![Fault::TrashInto {
            root: root.clone(),
        }]));
        vfs.register(Arc::new(ShellProvider::with_sources(
            Arc::new(FreedesktopTrash::new(&root)),
            Arc::new(SystemDrives),
        )));
        Bin {
            _dir: dir,
            home,
            outside,
            trash,
            vfs,
        }
    }

    impl Bin {
        /// Trashes a new file `name` with `text` from the home folder, and gives its item.
        fn trashed(&self, name: &str, text: &str) -> TrashItem {
            let path = self.home.join(name);
            std::fs::write(&path, text).unwrap();
            let location = self.trash.move_to_trash(&v(&path)).unwrap().unwrap();
            self.item_at(&location)
        }

        fn item_at(&self, location: &Path) -> TrashItem {
            self.trash
                .list()
                .unwrap()
                .into_iter()
                .find(|item| item.location == location)
                .expect("the item is in the trash")
        }

        fn records(&self) -> Vec<String> {
            names(&self.trash.info_dir())
        }

        fn kept_outside(&self) -> bool {
            names(&self.outside) == ["keep.txt"]
                && std::fs::read_to_string(self.outside.join("keep.txt")).unwrap() == "K"
        }
    }

    /// The names in the folder `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn engine(vfs: Vfs) -> (JobEngine, Receiver<JobEvent>) {
        JobEngine::start(EngineConfig::new(vfs, Arc::new(NonInteractive)))
    }

    #[test]
    fn a_restore_comes_back_and_its_undo_sends_it_to_the_trash_again() {
        let b = bin();
        let item = b.trashed("a.txt", "A");
        let original = item.original.clone().unwrap();
        assert_eq!(original, b.home.join("a.txt"));
        let (e, rx) = engine(b.vfs.clone());

        let spec = JobSpec::RestoreFromTrash {
            items: vec![(item.clone(), original.clone())],
        };
        let rep = wait_finished(&rx, e.submit(spec), WAIT);
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "A");
        assert_eq!(b.records(), Vec::<String>::new());
        assert_eq!(rep.outputs, [v(&original)]);
        assert_eq!((rep.planned.files, rep.done.files), (1, 1));
        assert_eq!(
            rep.undo,
            Some(UndoRecord::Restored {
                items: vec![TrashedItem {
                    original: v(&original),
                    location: Some(v(&item.location)),
                }]
            })
        );

        // Undo: back to the trash, with a record of its own.
        let undone = wait_finished(&rx, e.undo().expect("the restore can be undone"), WAIT);
        assert_eq!(undone.outcome, Outcome::Completed, "{:?}", undone.failures);
        assert!(!original.exists());
        assert_eq!(b.records(), ["a.txt.trashinfo"]);
        let [again] = b.trash.list().unwrap().try_into().unwrap();
        assert_eq!(again.original.as_deref(), Some(original.as_path()));

        // Redo: out again, and the record goes with it.
        let redone = wait_finished(&rx, e.redo().expect("the undo can be redone"), WAIT);
        assert_eq!(redone.outcome, Outcome::Completed, "{:?}", redone.failures);
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "A");
        assert_eq!(b.records(), Vec::<String>::new());
        assert!(b.kept_outside());
        assert!(e.shutdown(WAIT));
    }

    /// A restore to a place the user chose; the folder there must exist.
    #[test]
    fn an_item_goes_where_the_user_chose() {
        let b = bin();
        let item = b.trashed("a.txt", "A");
        let chosen = b.home.join("chosen.txt");
        let missing = b.home.join("no-such-folder").join("a.txt");
        let spec = JobSpec::RestoreFromTrash {
            items: vec![
                (item.clone(), missing.clone()),
                (item.clone(), chosen.clone()),
            ],
        };
        let rep = run_with(&spec, &b.vfs, &NonInteractive);
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        // The folder is gone, not the item: a reason of its own, for "choose a place".
        assert_eq!(
            (
                rep.failures[0].stage,
                rep.failures[0].kind,
                rep.failures[0].reason.clone()
            ),
            (
                Stage::Restore,
                ErrorKind::NotFound,
                Some(FailureReason::OriginalFolderGone)
            )
        );
        assert_eq!(rep.failures[0].target, Some(v(&missing)));
        assert_eq!(std::fs::read_to_string(&chosen).unwrap(), "A");
        assert_eq!(rep.outputs, [v(&chosen)]);
        assert!(!b.home.join("a.txt").exists());
    }

    /// An item that left the trash is gone: `NotFound` with no reason, unlike a gone folder.
    #[test]
    fn an_item_that_left_the_trash_is_gone() {
        let b = bin();
        let item = b.trashed("a.txt", "A");
        std::fs::remove_file(&item.location).unwrap();
        let spec = JobSpec::RestoreFromTrash {
            items: vec![(item.clone(), b.home.join("a.txt"))],
        };
        let rep = run_with(&spec, &b.vfs, &NonInteractive);
        assert_eq!(
            (
                rep.failures[0].stage,
                rep.failures[0].kind,
                rep.failures[0].reason.clone()
            ),
            (Stage::Restore, ErrorKind::NotFound, None)
        );
        assert!(!b.home.join("a.txt").exists());
    }

    /// A taken place is a conflict, and the answer decides; nothing is ever written over.
    #[test]
    fn a_taken_place_follows_the_conflict_answer() {
        let answers = [
            (ConflictAction::Skip, "skip"),
            (ConflictAction::Rename, "rename"),
            (ConflictAction::Overwrite, "overwrite"),
            (ConflictAction::Cancel, "cancel"),
        ];
        for (action, what) in answers {
            let b = bin();
            let item = b.trashed("a.txt", "old");
            let original = b.home.join("a.txt");
            std::fs::write(&original, "new").unwrap();
            let resolver = ScriptedResolver::new().on_conflict(ConflictAnswer::once(action));
            let spec = JobSpec::RestoreFromTrash {
                items: vec![(item.clone(), original.clone())],
            };
            let rep = run_with(&spec, &b.vfs, &resolver);
            assert_eq!(resolver.calls().len(), 1, "{what}");
            // What took the place is never touched.
            assert_eq!(std::fs::read_to_string(&original).unwrap(), "new", "{what}");
            let in_trash = item.location.exists();
            match action {
                ConflictAction::Skip => {
                    assert_eq!((rep.outcome, rep.skipped), (Outcome::Completed, 1));
                    assert!(in_trash);
                    assert_eq!(rep.undo, None);
                }
                ConflictAction::Rename => {
                    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
                    let numbered = b.home.join("a (2).txt");
                    assert_eq!(std::fs::read_to_string(&numbered).unwrap(), "old");
                    assert_eq!(rep.outputs, [v(&numbered)]);
                    assert!(!in_trash);
                    assert_eq!(b.records(), Vec::<String>::new());
                    let Some(UndoRecord::Restored { items }) = &rep.undo else {
                        panic!("{:?}", rep.undo);
                    };
                    assert_eq!(items[0].original, v(&numbered));
                }
                ConflictAction::Overwrite => {
                    assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
                    let failure = &rep.failures[0];
                    assert_eq!(
                        (failure.stage, failure.kind, failure.reason.clone()),
                        (
                            Stage::Conflict,
                            ErrorKind::AlreadyExists,
                            Some(FailureReason::RestoreNeverReplaces)
                        )
                    );
                    assert!(in_trash);
                }
                ConflictAction::Cancel => {
                    assert_eq!(rep.outcome, Outcome::Cancelled);
                    assert!(in_trash);
                }
                _ => unreachable!(),
            }
            if in_trash {
                assert_eq!(b.records(), ["a.txt.trashinfo"], "{what}");
            }
            assert!(b.kept_outside());
        }
    }

    /// One answer for all settles every later conflict of the job.
    #[test]
    fn an_answer_for_all_settles_the_other_conflicts() {
        let b = bin();
        let first = b.trashed("a.txt", "1");
        let second = b.trashed("b.txt", "2");
        for name in ["a.txt", "b.txt"] {
            std::fs::write(b.home.join(name), "taken").unwrap();
        }
        let resolver =
            ScriptedResolver::new().on_conflict(ConflictAnswer::all(ConflictAction::Rename));
        let spec = JobSpec::RestoreFromTrash {
            items: vec![
                (first, b.home.join("a.txt")),
                (second, b.home.join("b.txt")),
            ],
        };
        let rep = run_with(&spec, &b.vfs, &resolver);
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert_eq!(resolver.calls().len(), 1);
        assert_eq!(names(&b.home), ["a (2).txt", "a.txt", "b (2).txt", "b.txt"]);
    }

    /// Deleting for good takes the item and its record, and nothing else: not what a link in it
    /// leads to, not another item of the trash.
    #[test]
    fn delete_forever_removes_only_the_item_and_its_record() {
        let b = bin();
        let folder = b.home.join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("inner.txt"), "I").unwrap();
        std::os::unix::fs::symlink(&b.outside, folder.join("link")).unwrap();
        let location = b.trash.move_to_trash(&v(&folder)).unwrap().unwrap();
        let folder_item = b.item_at(&location);
        let other = b.trashed("other.txt", "O");

        let spec = JobSpec::DeleteFromTrash {
            items: vec![folder_item],
        };
        let rep = run_with(&spec, &b.vfs, &NonInteractive);
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert_eq!((rep.planned.dirs, rep.done.dirs), (1, 1));
        assert_eq!(rep.undo, None);
        assert_eq!(names(&b.trash.files_dir()), ["other.txt"]);
        assert_eq!(b.records(), ["other.txt.trashinfo"]);
        assert!(other.location.exists());
        assert!(b.kept_outside());

        // Gone already: skipped, not failed.
        let again = run_with(&spec, &b.vfs, &NonInteractive);
        assert_eq!((again.outcome, again.skipped), (Outcome::Completed, 1));
    }

    #[test]
    fn empty_leaves_nothing_outside_the_trash_touched() {
        let b = bin();
        b.trashed("a.txt", "A");
        let folder = b.home.join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::os::unix::fs::symlink(&b.outside, folder.join("link")).unwrap();
        b.trash.move_to_trash(&v(&folder)).unwrap();
        std::os::unix::fs::symlink(&b.outside, b.home.join("top-link")).unwrap();
        b.trash.move_to_trash(&v(b.home.join("top-link"))).unwrap();
        std::fs::write(b.home.join("stays.txt"), "S").unwrap();

        let rep = run_with(&JobSpec::EmptyTrash, &b.vfs, &NonInteractive);
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert_eq!((rep.planned.files, rep.planned.dirs), (2, 1));
        assert_eq!(rep.done, rep.planned);
        assert_eq!(rep.undo, None);
        assert_eq!(names(&b.trash.files_dir()), Vec::<String>::new());
        assert_eq!(b.records(), Vec::<String>::new());
        assert!(b.kept_outside());
        assert_eq!(names(&b.home), ["stays.txt"]);
    }

    /// A row of an old listing whose name another trashed item took since is refused, for a
    /// delete and a restore and for their retries: the other item is never touched.
    #[test]
    fn a_stale_item_never_touches_the_item_that_took_its_place() {
        let b = bin();
        let stale = b.trashed("a.txt", "first");
        let first = b.home.join("a.txt");
        let back = JobSpec::RestoreFromTrash {
            items: vec![(stale.clone(), first.clone())],
        };
        assert_eq!(
            run_with(&back, &b.vfs, &NonInteractive).outcome,
            Outcome::Completed
        );
        let other = b.home.join("other");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(other.join("a.txt"), "second").unwrap();
        let location = b
            .trash
            .move_to_trash(&v(other.join("a.txt")))
            .unwrap()
            .unwrap();
        assert_eq!(location, stale.location, "the name is taken again");

        let delete = JobSpec::DeleteFromTrash {
            items: vec![stale.clone()],
        };
        let restore = JobSpec::RestoreFromTrash {
            items: vec![(stale.clone(), b.home.join("elsewhere.txt"))],
        };
        for (spec, stage) in [(delete, Stage::Delete), (restore, Stage::Restore)] {
            let rep = run_with(&spec, &b.vfs, &NonInteractive);
            assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
            let failure = &rep.failures[0];
            assert_eq!(
                (failure.stage, failure.reason.clone()),
                (stage, Some(FailureReason::TrashItemChanged))
            );
            assert_eq!(failure.source, Some(v(&stale.location)));
            // Nothing a retry could change.
            assert_eq!(rep.retry_spec(), None);
        }
        assert_eq!(std::fs::read_to_string(&location).unwrap(), "second");
        assert_eq!(b.records(), ["a.txt.trashinfo"]);
        assert!(!b.home.join("elsewhere.txt").exists());
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "first");
    }

    /// Emptying the trash goes item by item: an item that cannot be deleted is a failure of its
    /// own, and the retry deletes that item alone, as it was listed, never what was trashed since.
    #[test]
    fn an_empty_that_fails_on_an_item_is_retried_as_a_delete_of_that_item() {
        let b = bin();
        let a = b.trashed("a.txt", "A");
        let kept = b.trashed("b.txt", "B");
        let c = b.trashed("c.txt", "C");
        let mut vfs = Vfs::new();
        vfs.register(FaultyProvider::local(Vec::new()));
        let shell = ShellProvider::with_sources(Arc::new(b.trash.clone()), Arc::new(SystemDrives));
        let faulty = FaultyProvider::new(
            Arc::new(shell),
            vec![Fault::FailDelete {
                name: "b.txt".into(),
            }],
        );
        vfs.register(Arc::new(faulty));

        let rep = run_with(&JobSpec::EmptyTrash, &vfs, &NonInteractive);
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        assert_eq!((rep.planned.files, rep.done.files), (3, 2));
        let [failure] = rep.failures.as_slice() else {
            panic!("one failure: {:?}", rep.failures);
        };
        assert_eq!(
            (failure.stage, failure.kind, failure.source.clone()),
            (
                Stage::Delete,
                ErrorKind::PermissionDenied,
                Some(v(&kept.location))
            )
        );
        assert!(!a.location.exists() && !c.location.exists());
        assert!(kept.location.exists());
        assert_eq!(b.records(), ["b.txt.trashinfo"]);

        // Something trashed after the empty is never taken by its retry.
        let later = b.trashed("later.txt", "L");
        let retry = rep.retry_spec().expect("the failed item is tried again");
        assert_eq!(
            retry,
            JobSpec::DeleteFromTrash {
                items: vec![b.item_at(&kept.location)]
            }
        );
        let again = run_with(&retry, &b.vfs, &NonInteractive);
        assert_eq!(again.outcome, Outcome::Completed, "{:?}", again.failures);
        assert!(!kept.location.exists());
        assert_eq!(std::fs::read_to_string(&later.location).unwrap(), "L");
        assert_eq!(b.records(), ["later.txt.trashinfo"]);
        assert!(b.kept_outside());
    }

    /// An item that does not lie in the trash is refused, whatever it says: nothing is moved or
    /// deleted.
    #[test]
    fn a_location_outside_the_trash_is_refused() {
        let b = bin();
        let kept = b.trashed("a.txt", "A");
        let victim = b.outside.join("keep.txt");
        let records = [
            TrashItem::at(victim.clone(), false),
            TrashItem::at(b.trash.info_dir().join("a.txt.trashinfo"), false),
            TrashItem::at(b.trash.files_dir(), true),
            TrashItem::at(b.outside.clone(), true),
        ];
        for item in records {
            let restore = JobSpec::RestoreFromTrash {
                items: vec![(item.clone(), b.home.join("back"))],
            };
            let delete = JobSpec::DeleteFromTrash {
                items: vec![item.clone()],
            };
            for (spec, stage) in [(restore, Stage::Restore), (delete, Stage::Delete)] {
                let rep = run_with(&spec, &b.vfs, &NonInteractive);
                assert_eq!(rep.outcome, Outcome::CompletedWithErrors, "{item:?}");
                assert_eq!(
                    (rep.failures[0].stage, rep.failures[0].kind),
                    (stage, ErrorKind::Unsupported),
                    "{item:?}"
                );
            }
        }
        assert!(b.kept_outside());
        assert!(!b.home.join("back").exists());
        assert!(kept.location.exists());
        assert_eq!(b.records(), ["a.txt.trashinfo"]);
    }
}
