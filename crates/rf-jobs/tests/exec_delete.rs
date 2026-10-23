#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

mod common;

use std::path::PathBuf;

use common::*;
use rf_core::ErrorKind;
use rf_jobs::testkit::{tree, write_tree};
use rf_jobs::{
    Control, DeleteMethod, DeleteSpec, ErrorPolicy, ExecEnv, ExecEvent, JobId, JobSpec,
    NonInteractive, Outcome, Stage, Totals, execute,
};
use rf_vfs::Vfs;

fn del(paths: &[PathBuf], method: DeleteMethod) -> JobSpec {
    JobSpec::Delete(DeleteSpec {
        paths: paths.iter().map(v).collect(),
        method,
        errors: ErrorPolicy::Continue,
    })
}

#[test]
fn permanent_delete_walks_the_tree() {
    let f = fx();
    write_tree(
        &f.src,
        &[
            ("dir/a.txt", "A"),
            ("dir/sub/b.txt", "B"),
            ("dir/empty/", ""),
        ],
    );
    let rep = run(&del(&[f.src.join("dir")], DeleteMethod::Permanent));
    assert_eq!(rep.outcome, Outcome::Completed);
    assert!(!f.src.join("dir").exists());
    assert_eq!(
        (rep.planned, rep.done),
        (
            Totals {
                files: 2,
                dirs: 3,
                bytes: 0
            },
            Totals {
                files: 2,
                dirs: 3,
                bytes: 0
            }
        )
    );
    assert_eq!(rep.undo, None);
}

#[test]
fn missing_paths_are_reported() {
    let f = fx();
    let rep = run(&del(&[f.src.join("missing")], DeleteMethod::Permanent));
    assert_eq!(
        (rep.failures[0].stage, rep.failures[0].kind),
        (Stage::Plan, ErrorKind::NotFound)
    );
}

#[test]
fn cancelling_after_planning_deletes_nothing() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/b.txt", "B")]);
    let control = Control::new();
    let c2 = control.clone();
    let on = move |e: ExecEvent| {
        if matches!(e, ExecEvent::Planned { .. }) {
            c2.cancel();
        }
    };
    let (vfs, res) = (Vfs::with_local(), NonInteractive);
    let job = del(&[f.src.join("dir")], DeleteMethod::Permanent);
    let rep = execute(
        JobId::generate(),
        &job,
        &ExecEnv::new(&vfs, &res).control(control).on_event(&on),
    );
    assert_eq!(rep.outcome, Outcome::Cancelled);
    assert_eq!(tree(&f.src.join("dir")).len(), 2);
}

#[cfg(unix)]
#[test]
fn permanent_delete_removes_links_not_their_targets() {
    let f = fx();
    write_tree(&f.src, &[("outside/keep.txt", "K"), ("dir/", "")]);
    std::os::unix::fs::symlink(f.src.join("outside"), f.src.join("dir").join("link")).unwrap();
    assert_eq!(
        run(&del(&[f.src.join("dir")], DeleteMethod::Permanent)).outcome,
        Outcome::Completed
    );
    assert_eq!(
        std::fs::read_to_string(f.src.join("outside").join("keep.txt")).unwrap(),
        "K"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn trash_never_turns_into_a_permanent_delete() {
    let f = fx();
    write_tree(&f.src, &[("a.txt", "A")]);
    let rep = run(&del(&[f.src.join("a.txt")], DeleteMethod::Trash));
    assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
    assert_eq!(
        (rep.failures[0].stage, rep.failures[0].kind),
        (Stage::Delete, ErrorKind::Unsupported)
    );
    assert!(f.src.join("a.txt").exists());
    assert_eq!(rep.undo, None);
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn trash_records_where_items_went() {
    let f = fx();
    let name = unique_name("trash.txt");
    std::fs::write(f.src.join(&name), "T").unwrap();
    let rep = run(&del(&[f.src.join(&name)], DeleteMethod::Trash));
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    let Some(rf_jobs::UndoRecord::Trashed { items }) = rep.undo else {
        panic!("{:?}", rep.undo)
    };
    assert_eq!(items[0].original, v(f.src.join(&name)));
    purge(
        items[0]
            .location
            .as_ref()
            .expect("trash location")
            .as_local()
            .unwrap(),
    );
}

// What follows goes beyond the brief: the rules the controller added for this task.

mod walk {
    use std::sync::atomic::{AtomicBool, Ordering};

    use rf_jobs::MAX_DEPTH;
    use rf_jobs::testkit::{Fault, FaultyProvider};

    use super::*;

    /// A file that another program puts in a folder while it is deleted is not deleted with
    /// it: the folder is removed only once it is empty, and stays with that file.
    #[test]
    fn a_file_created_during_the_delete_keeps_its_folder() {
        let f = fx();
        write_tree(&f.src, &[("dir/a.txt", "A")]);
        let p = FaultyProvider::local(vec![Fault::CreateOnDelete {
            name: "late.txt".into(),
        }]);
        let job = del(&[f.src.join("dir")], DeleteMethod::Permanent);
        let rep = run_on(&job, &p.vfs(), &NonInteractive);
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        assert_eq!(
            std::fs::read_to_string(f.src.join("dir").join("late.txt")).unwrap(),
            "late"
        );
        assert!(!f.src.join("dir").join("a.txt").exists());
        let failed: Vec<_> = rep
            .failures
            .iter()
            .map(|x| (x.stage, x.kind, x.source.clone()))
            .collect();
        assert_eq!(
            failed,
            [(
                Stage::Delete,
                ErrorKind::DirectoryNotEmpty,
                Some(v(f.src.join("dir")))
            )]
        );
        assert_eq!(
            rep.done,
            Totals {
                files: 1,
                dirs: 0,
                bytes: 0
            }
        );
    }

    /// An item that cannot be deleted keeps every folder above it; the rest goes.
    #[test]
    fn a_failure_keeps_the_folders_above_it() {
        let f = fx();
        write_tree(
            &f.src,
            &[("dir/a.txt", "A"), ("dir/sub/b.txt", "B"), ("dir/c/", "")],
        );
        let p = FaultyProvider::local(vec![Fault::FailDelete {
            name: "b.txt".into(),
        }]);
        let job = del(&[f.src.join("dir")], DeleteMethod::Permanent);
        let rep = run_on(&job, &p.vfs(), &NonInteractive);
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        assert_eq!(
            tree(&f.src.join("dir")).into_keys().collect::<Vec<_>>(),
            ["sub", "sub/b.txt"]
        );
        let failed: Vec<_> = rep
            .failures
            .iter()
            .map(|x| (x.stage, x.kind, x.source.clone()))
            .collect();
        let b = f.src.join("dir").join("sub").join("b.txt");
        assert_eq!(
            failed,
            [(Stage::Delete, ErrorKind::PermissionDenied, Some(v(b)))]
        );
        assert_eq!(
            (rep.planned, rep.done),
            (
                Totals {
                    files: 2,
                    dirs: 3,
                    bytes: 0
                },
                Totals {
                    files: 1,
                    dirs: 1,
                    bytes: 0
                }
            )
        );
    }

    /// A folder on another volume than the folder it is in (a mount point) is neither
    /// entered nor removed, so nothing on that volume is deleted; the folders above it stay.
    #[test]
    fn a_folder_on_another_volume_is_never_entered() {
        let f = fx();
        write_tree(
            &f.src,
            &[
                ("dir/a.txt", "A"),
                ("dir/sub/b.txt", "B"),
                ("dir/mnt/x.txt", "X"),
            ],
        );
        let mnt = f.src.join("dir").join("mnt");
        let p = FaultyProvider::local(vec![Fault::OtherVolume { under: v(&mnt) }]);
        let job = del(&[f.src.join("dir")], DeleteMethod::Permanent);
        let rep = run_on(&job, &p.vfs(), &NonInteractive);
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        assert_eq!(
            tree(&f.src.join("dir")).into_keys().collect::<Vec<_>>(),
            ["mnt", "mnt/x.txt"]
        );
        assert_eq!(std::fs::read_to_string(mnt.join("x.txt")).unwrap(), "X");
        assert_eq!(rep.failures.len(), 1, "{:?}", rep.failures);
        let failure = &rep.failures[0];
        assert_eq!(
            (failure.stage, failure.kind, failure.source.clone()),
            (Stage::Delete, ErrorKind::Unsupported, Some(v(&mnt)))
        );
        assert!(
            failure.message.contains("is on another volume"),
            "{}",
            failure.message
        );
        // What is on the other volume is not planned either.
        assert_eq!(
            (rep.planned, rep.done),
            (
                Totals {
                    files: 2,
                    dirs: 2,
                    bytes: 0
                },
                Totals {
                    files: 2,
                    dirs: 1,
                    bytes: 0
                }
            )
        );
    }

    /// The selected folder itself may be another volume's root: only what is below it is
    /// compared with the folder it is in.
    #[test]
    fn the_selected_folder_may_be_on_another_volume() {
        let f = fx();
        write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/sub/b.txt", "B")]);
        let dir = f.src.join("dir");
        let p = FaultyProvider::local(vec![Fault::OtherVolume { under: v(&dir) }]);
        let rep = run_on(
            &del(std::slice::from_ref(&dir), DeleteMethod::Permanent),
            &p.vfs(),
            &NonInteractive,
        );
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert!(!dir.exists());
    }

    /// An item deeper than `MAX_DEPTH` fails with the nesting message and is not deleted, so
    /// every folder above it stays.
    #[test]
    fn nesting_deeper_than_the_limit_is_not_deleted() {
        let f = fx();
        let chain = vec!["d"; MAX_DEPTH + 1].join("/");
        let deepest = format!("dir/{chain}/x.txt");
        write_tree(&f.src, &[("dir/a.txt", "A"), (deepest.as_str(), "X")]);
        let rep = run(&del(&[f.src.join("dir")], DeleteMethod::Permanent));
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        assert_eq!(rep.failures.len(), 1, "{:?}", rep.failures);
        let failure = &rep.failures[0];
        assert_eq!(
            (failure.stage, failure.kind),
            (Stage::Plan, ErrorKind::Unsupported)
        );
        assert!(
            failure.message.contains("nesting deeper than"),
            "{}",
            failure.message
        );
        // The folder at depth `MAX_DEPTH + 1` and what is in it.
        let too_deep: PathBuf = std::iter::once(f.src.join("dir"))
            .chain(std::iter::repeat_n(PathBuf::from("d"), MAX_DEPTH + 1))
            .collect();
        assert_eq!(failure.source, Some(v(&too_deep)));
        assert_eq!(
            std::fs::read_to_string(too_deep.join("x.txt")).unwrap(),
            "X"
        );
        assert!(!f.src.join("dir").join("a.txt").exists());
        // The plan counts the folders up to the limit and the one file above it.
        let limit = u64::try_from(MAX_DEPTH).unwrap();
        assert_eq!(
            (rep.planned, rep.done),
            (
                Totals {
                    files: 1,
                    dirs: limit + 1,
                    bytes: 0
                },
                Totals {
                    files: 1,
                    dirs: 0,
                    bytes: 0
                }
            )
        );
    }

    /// An item that turned into a folder after it was planned or listed is not deleted with
    /// whatever is in that folder now.
    #[test]
    fn an_item_replaced_by_a_folder_is_left_in_place() {
        let f = fx();
        write_tree(&f.src, &[("a.txt", "A"), ("b.txt", "B")]);
        let b = f.src.join("b.txt");
        let replaced = AtomicBool::new(false);
        // The first progress event comes once `a.txt` is deleted, before `b.txt` is.
        let on = |e: ExecEvent| {
            if matches!(e, ExecEvent::Progress(_)) && !replaced.swap(true, Ordering::SeqCst) {
                std::fs::remove_file(&b).unwrap();
                write_tree(&b, &[("inner.txt", "I")]);
            }
        };
        let (vfs, res) = (Vfs::with_local(), NonInteractive);
        let job = del(&[f.src.join("a.txt"), b.clone()], DeleteMethod::Permanent);
        let rep = execute(
            JobId::generate(),
            &job,
            &ExecEnv::new(&vfs, &res).on_event(&on),
        );
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        assert!(!f.src.join("a.txt").exists());
        assert_eq!(std::fs::read_to_string(b.join("inner.txt")).unwrap(), "I");
        let failure = &rep.failures[0];
        assert_eq!(
            (failure.stage, failure.kind, failure.source.clone()),
            (Stage::Delete, ErrorKind::Unsupported, Some(v(&b)))
        );
        assert!(
            failure.message.contains("changed while it was deleted"),
            "{}",
            failure.message
        );
    }

    /// A folder that turned into a link to another folder after it was planned is not listed
    /// through the link, so nothing in the folder the link leads to is deleted.
    #[cfg(unix)]
    #[test]
    fn a_folder_replaced_by_a_link_is_never_entered() {
        let f = fx();
        write_tree(
            &f.src,
            &[
                ("a.txt", "A"),
                ("dir/x.txt", "X"),
                ("outside/keep.txt", "K"),
            ],
        );
        let dir = f.src.join("dir");
        let replaced = AtomicBool::new(false);
        // The first progress event comes once `a.txt` is deleted, before `dir` is entered.
        let on = |e: ExecEvent| {
            if matches!(e, ExecEvent::Progress(_)) && !replaced.swap(true, Ordering::SeqCst) {
                std::fs::remove_dir_all(&dir).unwrap();
                std::os::unix::fs::symlink(f.src.join("outside"), &dir).unwrap();
            }
        };
        let (vfs, res) = (Vfs::with_local(), NonInteractive);
        let job = del(&[f.src.join("a.txt"), dir.clone()], DeleteMethod::Permanent);
        let rep = execute(
            JobId::generate(),
            &job,
            &ExecEnv::new(&vfs, &res).on_event(&on),
        );
        assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
        assert_eq!(
            std::fs::read_to_string(f.src.join("outside").join("keep.txt")).unwrap(),
            "K"
        );
        assert!(dir.is_symlink());
        let failure = &rep.failures[0];
        assert_eq!(
            (failure.stage, failure.kind, failure.source.clone()),
            (Stage::Delete, ErrorKind::Unsupported, Some(v(&dir)))
        );
        assert!(
            failure.message.contains("changed while it was deleted"),
            "{}",
            failure.message
        );
    }

    /// A link to a folder counts and goes as one file: what it leads to is never entered.
    #[cfg(unix)]
    #[test]
    fn a_link_to_a_folder_is_deleted_as_a_link() {
        let f = fx();
        write_tree(
            &f.src,
            &[
                ("outside/keep.txt", "K"),
                ("outside/in/deep.txt", "D"),
                ("dir/sub/", ""),
            ],
        );
        let before = tree(&f.src.join("outside"));
        let link = f.src.join("dir").join("sub").join("link");
        std::os::unix::fs::symlink(f.src.join("outside"), &link).unwrap();
        let rep = run(&del(&[f.src.join("dir")], DeleteMethod::Permanent));
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert!(!f.src.join("dir").exists());
        assert_eq!(tree(&f.src.join("outside")), before);
        let one_link = Totals {
            files: 1,
            dirs: 2,
            bytes: 0,
        };
        assert_eq!((rep.planned, rep.done), (one_link, one_link));
    }

    /// A socket, FIFO or device is deleted by its name and never opened.
    #[cfg(unix)]
    #[test]
    fn special_files_are_deleted_by_name() {
        use std::os::unix::net::UnixListener;
        let f = fx();
        write_tree(&f.src, &[("dir/", "")]);
        let _socket = UnixListener::bind(f.src.join("dir").join("s.sock")).unwrap();
        let rep = run(&del(&[f.src.join("dir")], DeleteMethod::Permanent));
        assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
        assert!(!f.src.join("dir").exists());
        let counted = Totals {
            files: 1,
            dirs: 1,
            bytes: 0,
        };
        assert_eq!((rep.planned, rep.done), (counted, counted));
    }

    /// A trash counts only the items it was given, never what is in a folder, and leaves
    /// every one of them where it is when it cannot take it.
    #[cfg(target_os = "linux")]
    #[test]
    fn trash_counts_only_the_selected_items() {
        let f = fx();
        write_tree(
            &f.src,
            &[("dir/a.txt", "A"), ("dir/sub/b.txt", "B"), ("c.txt", "C")],
        );
        let before = tree(&f.src);
        let rep = run(&del(
            &[f.src.join("dir"), f.src.join("c.txt")],
            DeleteMethod::Trash,
        ));
        assert_eq!(
            rep.planned,
            Totals {
                files: 1,
                dirs: 1,
                bytes: 0
            }
        );
        assert_eq!(rep.done, Totals::default());
        assert_eq!(rep.failures.len(), 2);
        assert_eq!(tree(&f.src), before);
    }
}
