#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use rf_core::ErrorKind;
use rf_jobs::testkit::{Fault, FaultyProvider, ScriptedResolver, temp_files, tree, write_tree};
use rf_jobs::{
    ConflictPolicy, Control, ErrorAction, ErrorAnswer, ErrorPolicy, ExecEnv, JobId, JobSpec,
    MovedItem, NonInteractive, Outcome, Stage, Totals, TransferItem, TransferMode, TransferOptions,
    TransferSpec, UndoRecord, execute,
};

fn mv(sources: &[PathBuf], dest: &Path) -> JobSpec {
    spec(
        TransferMode::Move,
        sources,
        dest,
        TransferOptions::default(),
    )
}

fn with(conflict: ConflictPolicy) -> TransferOptions {
    TransferOptions {
        conflict,
        ..TransferOptions::default()
    }
}

/// A move of `source` to exactly `target`, name included.
fn mv_to(source: &Path, target: &Path, options: TransferOptions) -> JobSpec {
    JobSpec::Transfer(TransferSpec {
        mode: TransferMode::Move,
        items: vec![TransferItem {
            source: v(source),
            target: v(target),
        }],
        options,
    })
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

#[test]
fn same_volume_move_is_a_rename() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/sub/b.txt", "B")]);
    let before = tree(&f.src.join("dir"));
    let p = FaultyProvider::local(vec![]);
    let rep = run_on(&mv(&[f.src.join("dir")], &f.dst), &p.vfs(), &NonInteractive);
    assert_eq!(rep.outcome, Outcome::Completed);
    assert_eq!(p.opened_writes(), 0);
    assert_eq!(tree(&f.dst.join("dir")), before);
    assert!(!f.src.join("dir").exists());
    assert_eq!(
        (rep.planned, rep.done),
        (
            Totals {
                files: 0,
                dirs: 1,
                bytes: 0
            },
            Totals {
                files: 0,
                dirs: 1,
                bytes: 0
            }
        )
    );
    let moves = vec![MovedItem {
        from: v(f.src.join("dir")),
        to: v(f.dst.join("dir")),
    }];
    assert_eq!(
        rep.undo,
        Some(UndoRecord::Moved {
            moves,
            created_dirs: vec![],
            removed_dirs: vec![],
            overwritten: 0,
            copied_dirs: vec![],
        })
    );
}

#[test]
fn cross_volume_move_copies_then_deletes() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/sub/b.txt", "B")]);
    let before = tree(&f.src.join("dir"));
    let p = FaultyProvider::local(vec![Fault::CrossDevice]);
    let rep = run_on(&mv(&[f.src.join("dir")], &f.dst), &p.vfs(), &NonInteractive);
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    assert_eq!(tree(&f.dst.join("dir")), before);
    assert!(!f.src.join("dir").exists());
    let Some(UndoRecord::Moved {
        moves,
        created_dirs,
        removed_dirs,
        overwritten,
        copied_dirs,
    }) = rep.undo
    else {
        panic!("{:?}", rep.undo)
    };
    let moved: BTreeSet<_> = moves.into_iter().map(|m| (m.from, m.to)).collect();
    assert_eq!(
        moved,
        BTreeSet::from([
            (v(f.src.join("dir/a.txt")), v(f.dst.join("dir/a.txt"))),
            (
                v(f.src.join("dir/sub/b.txt")),
                v(f.dst.join("dir/sub/b.txt"))
            ),
        ])
    );
    assert_eq!(
        created_dirs,
        [v(f.dst.join("dir")), v(f.dst.join("dir/sub"))]
    );
    assert_eq!(
        removed_dirs,
        [v(f.src.join("dir/sub")), v(f.src.join("dir"))]
    );
    // Each folder made at the target, with the source folder it was made for.
    let made_for: Vec<_> = copied_dirs.into_iter().map(|m| (m.from, m.to)).collect();
    assert_eq!(
        made_for,
        [
            (v(f.src.join("dir")), v(f.dst.join("dir"))),
            (v(f.src.join("dir/sub")), v(f.dst.join("dir/sub")))
        ]
    );
    assert_eq!(overwritten, 0);
}

#[test]
fn a_partial_cross_volume_move_keeps_what_failed() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/b.txt", "B")]);
    let p = FaultyProvider::local(vec![
        Fault::CrossDevice,
        Fault::FailWrite { file: 0, after: 0 },
    ]);
    let rep = run_on(&mv(&[f.src.join("dir")], &f.dst), &p.vfs(), &NonInteractive);
    assert_eq!(
        (rep.outcome, rep.failures[0].stage),
        (Outcome::CompletedWithErrors, Stage::Write)
    );
    for (name, body) in [("a.txt", "A"), ("b.txt", "B")] {
        let (at_src, at_dst) = (f.src.join("dir").join(name), f.dst.join("dir").join(name));
        assert!(
            at_src.exists() != at_dst.exists(),
            "{name} must be in exactly one place"
        );
        let place = if at_src.exists() { at_src } else { at_dst };
        assert_eq!(std::fs::read_to_string(place).unwrap(), body);
    }
    assert!(f.src.join("dir").is_dir());
}

#[test]
fn a_file_created_during_the_move_is_not_lost() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A")]);
    let p = FaultyProvider::local(vec![
        Fault::CrossDevice,
        Fault::CreateOnDelete {
            name: "late.txt".into(),
        },
    ]);
    let rep = run_on(&mv(&[f.src.join("dir")], &f.dst), &p.vfs(), &NonInteractive);
    assert_eq!(
        std::fs::read_to_string(f.src.join("dir").join("late.txt")).unwrap(),
        "late"
    );
    assert_eq!(
        std::fs::read_to_string(f.dst.join("dir").join("a.txt")).unwrap(),
        "A"
    );
    assert!(
        rep.warnings.iter().any(|w| w.stage == Stage::DeleteSource),
        "{:?}",
        rep.warnings
    );
}

#[test]
fn moving_onto_itself_does_nothing() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "me").unwrap();
    let rep = run(&mv(&[f.src.join("a.txt")], &f.src));
    assert_eq!(
        (rep.outcome, rep.skipped, rep.failures.len()),
        (Outcome::Completed, 1, 0)
    );
    assert_eq!(std::fs::read_to_string(f.src.join("a.txt")).unwrap(), "me");
    assert_eq!(rep.undo, None);
}

#[test]
fn moving_into_an_existing_folder_merges() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A")]);
    write_tree(&f.dst, &[("dir/other.txt", "O")]);
    let rep = run(&mv(&[f.src.join("dir")], &f.dst));
    assert_eq!(rep.outcome, Outcome::Completed);
    assert_eq!(
        tree(&f.dst.join("dir")).into_keys().collect::<Vec<_>>(),
        ["a.txt", "other.txt"]
    );
    assert!(!f.src.join("dir").exists());
    let moves = vec![MovedItem {
        from: v(f.src.join("dir/a.txt")),
        to: v(f.dst.join("dir/a.txt")),
    }];
    assert_eq!(
        rep.undo,
        Some(UndoRecord::Moved {
            moves,
            created_dirs: vec![],
            removed_dirs: vec![v(f.src.join("dir"))],
            overwritten: 0,
            copied_dirs: vec![],
        })
    );
}

#[test]
fn read_only_source_location_copies_instead() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "A").unwrap();
    let p = FaultyProvider::local(vec![Fault::ReadOnly { under: v(&f.src) }]);
    let rep = run_on(
        &mv(&[f.src.join("a.txt")], &f.dst),
        &p.vfs(),
        &NonInteractive,
    );
    assert!(rep.copied_instead_of_moved);
    assert!(f.src.join("a.txt").exists() && f.dst.join("a.txt").exists());
    assert!(matches!(rep.undo, Some(UndoRecord::Copied { .. })));
}

#[test]
fn failing_to_delete_the_source_keeps_both_copies() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "A").unwrap();
    let p = FaultyProvider::local(vec![
        Fault::CrossDevice,
        Fault::FailDelete {
            name: "a.txt".into(),
        },
    ]);
    let rep = run_on(
        &mv(&[f.src.join("a.txt")], &f.dst),
        &p.vfs(),
        &NonInteractive,
    );
    assert_eq!(
        (rep.failures[0].stage, rep.failures[0].kind),
        (Stage::DeleteSource, ErrorKind::PermissionDenied)
    );
    assert!(f.src.join("a.txt").exists() && f.dst.join("a.txt").exists());
}

#[test]
fn copy_records_what_it_created_for_undo() {
    let f = fx();
    write_tree(
        &f.src,
        &[("dir/new.txt", "N"), ("x.txt", "X"), ("y.txt", "Y")],
    );
    write_tree(&f.dst, &[("dir/old.txt", "O"), ("x.txt", "old")]);
    let opts = TransferOptions {
        conflict: ConflictPolicy::Overwrite,
        ..TransferOptions::default()
    };
    let sources = [f.src.join("dir"), f.src.join("x.txt"), f.src.join("y.txt")];
    let rep = run(&spec(TransferMode::Copy, &sources, &f.dst, opts));
    let Some(UndoRecord::Copied {
        created,
        overwritten,
        ..
    }) = rep.undo
    else {
        panic!("{:?}", rep.undo)
    };
    let paths: BTreeSet<_> = created.iter().map(|c| c.path.clone()).collect();
    assert_eq!(
        paths,
        BTreeSet::from([v(f.dst.join("dir/new.txt")), v(f.dst.join("y.txt"))])
    );
    assert_eq!(overwritten, 1);
    assert!(created.iter().all(|c| c.identity.is_some() && !c.is_dir));
}

#[test]
fn same_volume_move_can_overwrite() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "new").unwrap();
    std::fs::write(f.dst.join("a.txt"), "old").unwrap();
    let opts = TransferOptions {
        conflict: ConflictPolicy::Overwrite,
        ..TransferOptions::default()
    };
    let rep = run(&spec(
        TransferMode::Move,
        &[f.src.join("a.txt")],
        &f.dst,
        opts,
    ));
    assert_eq!(rep.outcome, Outcome::Completed);
    assert_eq!(std::fs::read_to_string(f.dst.join("a.txt")).unwrap(), "new");
    assert!(!f.src.join("a.txt").exists());
}

/// A second name of the source at the target is the source itself: nothing is renamed over it
/// or deleted, whatever the policy says, and both names keep the file.
#[cfg(unix)]
#[test]
fn moving_onto_a_hard_link_of_itself_does_nothing() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "me").unwrap();
    std::fs::hard_link(f.src.join("a.txt"), f.dst.join("a.txt")).unwrap();
    for faults in [vec![], vec![Fault::CrossDevice]] {
        let p = FaultyProvider::local(faults);
        let job = spec(
            TransferMode::Move,
            &[f.src.join("a.txt")],
            &f.dst,
            with(ConflictPolicy::Overwrite),
        );
        let rep = run_on(&job, &p.vfs(), &NonInteractive);
        assert_eq!(
            (rep.outcome, rep.skipped, rep.failures.len()),
            (Outcome::Completed, 1, 0),
            "{:?}",
            rep.failures
        );
        assert_eq!(
            (read(&f.src.join("a.txt")), read(&f.dst.join("a.txt"))),
            ("me".into(), "me".into())
        );
        assert_eq!(rep.undo, None);
    }
}

/// A rename that only changes the letter case of a name in its folder is a move to the new
/// name. On a file system that ignores case (Windows, macOS) the target is the source itself
/// under another spelling; on one that tells case apart (Linux) it is a free name.
#[test]
fn a_case_only_rename_changes_the_name() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "me").unwrap();
    let job = mv_to(
        &f.src.join("a.txt"),
        &f.src.join("A.TXT"),
        TransferOptions::default(),
    );
    let rep = run(&job);
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    assert_eq!(tree(&f.src).into_keys().collect::<Vec<_>>(), ["A.TXT"]);
    assert_eq!(read(&f.src.join("A.TXT")), "me");
    let moves = vec![MovedItem {
        from: v(f.src.join("a.txt")),
        to: v(f.src.join("A.TXT")),
    }];
    assert_eq!(
        rep.undo,
        Some(UndoRecord::Moved {
            moves,
            created_dirs: vec![],
            removed_dirs: vec![],
            overwritten: 0,
            copied_dirs: vec![],
        })
    );
}

/// Without identities, a target that ignores case takes `a.txt` and `A.TXT` for one item. Here
/// they are two names of one file on ext4 standing in for that: a move that took them for two
/// items would copy the file over its other name and delete it (on NTFS, deleting the only
/// copy). It is renamed as itself instead, and the injected failure of that rename leaves both
/// names alone.
#[cfg(target_os = "linux")]
#[test]
fn a_case_only_spelling_without_identities_is_never_two_items() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "me").unwrap();
    std::fs::hard_link(f.src.join("a.txt"), f.src.join("A.TXT")).unwrap();
    let p = FaultyProvider::local(vec![
        Fault::FsName("NTFS".into()),
        Fault::NoIdentity,
        Fault::CrossDevice,
    ]);
    let job = mv_to(
        &f.src.join("a.txt"),
        &f.src.join("A.TXT"),
        with(ConflictPolicy::Overwrite),
    );
    let rep = run_on(&job, &p.vfs(), &NonInteractive);
    assert_eq!(
        (
            rep.failures.len(),
            rep.failures[0].stage,
            rep.failures[0].kind
        ),
        (1, Stage::Commit, ErrorKind::CrossesDevices)
    );
    assert_eq!(
        (read(&f.src.join("a.txt")), read(&f.src.join("A.TXT"))),
        ("me".into(), "me".into())
    );
    assert_eq!(p.opened_writes(), 0);
    assert_eq!(rep.undo, None);
}

/// The source is replaced by a folder while its copy is written: the copy is committed, but the
/// folder is not the file that was copied and is never deleted in its place.
#[cfg(unix)]
#[test]
fn a_source_replaced_during_its_copy_is_never_deleted() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "A").unwrap();
    let p = FaultyProvider::local(vec![Fault::CrossDevice, Fault::Hold { file: 0 }]);
    let vfs = p.vfs();
    let job = mv(&[f.src.join("a.txt")], &f.dst);
    let rep = std::thread::scope(|scope| {
        let running = scope.spawn(|| run_on(&job, &vfs, &NonInteractive));
        let held = p.wait_held(Duration::from_secs(10));
        // The copy has the old file open; its name now leads to a folder with a file in it.
        std::fs::remove_file(f.src.join("a.txt")).unwrap();
        write_tree(&f.src, &[("a.txt/keep.txt", "keep")]);
        p.release();
        assert!(held, "the copy never reached its first write");
        running.join().unwrap()
    });
    assert_eq!(
        (rep.failures.len(), rep.failures[0].stage),
        (1, Stage::DeleteSource),
        "{:?}",
        rep.failures
    );
    assert_eq!(read(&f.src.join("a.txt").join("keep.txt")), "keep");
    assert_eq!(read(&f.dst.join("a.txt")), "A");
}

/// A plan that cannot tell whether the move ends with a rename (no volume identities) walks
/// the tree and counts it, as the copy across volumes then does.
#[test]
fn a_move_the_plan_cannot_predict_is_counted_in_full() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/sub/b.txt", "BB")]);
    let p = FaultyProvider::local(vec![Fault::NoIdentity, Fault::CrossDevice]);
    let rep = run_on(&mv(&[f.src.join("dir")], &f.dst), &p.vfs(), &NonInteractive);
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    let all = Totals {
        files: 2,
        dirs: 2,
        bytes: 3,
    };
    assert_eq!((rep.planned, rep.done), (all, all));
    assert!(!f.src.join("dir").exists());
}

/// One source in a place that cannot be written to turns the whole job into a copy: no source
/// is deleted, the writable one neither.
#[test]
fn one_read_only_source_turns_the_whole_move_into_a_copy() {
    let f = fx();
    write_tree(&f.src, &[("ro/a.txt", "A"), ("b.txt", "B")]);
    let p = FaultyProvider::local(vec![Fault::ReadOnly {
        under: v(f.src.join("ro")),
    }]);
    let job = mv(
        &[f.src.join("b.txt"), f.src.join("ro").join("a.txt")],
        &f.dst,
    );
    let rep = run_on(&job, &p.vfs(), &NonInteractive);
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    assert!(rep.copied_instead_of_moved);
    assert_eq!(read(&f.src.join("b.txt")), "B");
    assert_eq!(read(&f.src.join("ro").join("a.txt")), "A");
    assert_eq!(
        tree(&f.dst).into_keys().collect::<Vec<_>>(),
        ["a.txt", "b.txt"]
    );
}

/// Trying a failed delete of the source again deletes again; the file is not copied again.
#[test]
fn retrying_a_failed_source_delete_does_not_copy_again() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "A").unwrap();
    let p = FaultyProvider::local(vec![
        Fault::CrossDevice,
        Fault::FailDelete {
            name: "a.txt".into(),
        },
    ]);
    let answer = |action| ErrorAnswer {
        action,
        apply_to_all: false,
    };
    let r = ScriptedResolver::new()
        .on_error(answer(ErrorAction::Retry))
        .on_error(answer(ErrorAction::Skip));
    let options = TransferOptions {
        errors: ErrorPolicy::Ask,
        ..TransferOptions::default()
    };
    let job = spec(TransferMode::Move, &[f.src.join("a.txt")], &f.dst, options);
    let rep = run_on(&job, &p.vfs(), &r);
    assert_eq!(r.calls().len(), 2);
    assert_eq!(p.opened_writes(), 1);
    assert_eq!(
        (rep.failures.len(), rep.failures[0].stage),
        (1, Stage::DeleteSource)
    );
    assert_eq!((rep.done.files, rep.done.bytes), (1, 1));
    assert!(f.src.join("a.txt").exists() && f.dst.join("a.txt").exists());
}

/// A child the conflict policy leaves where it is keeps its folder at the source.
#[test]
fn a_merge_keeps_the_source_folder_when_a_child_stays() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/b.txt", "new")]);
    write_tree(&f.dst, &[("dir/b.txt", "old")]);
    let job = spec(
        TransferMode::Move,
        &[f.src.join("dir")],
        &f.dst,
        with(ConflictPolicy::Skip),
    );
    let rep = run(&job);
    assert_eq!(
        (rep.outcome, rep.skipped),
        (Outcome::Completed, 1),
        "{:?}",
        rep.failures
    );
    assert_eq!(read(&f.src.join("dir").join("b.txt")), "new");
    assert_eq!(read(&f.dst.join("dir").join("b.txt")), "old");
    assert_eq!(read(&f.dst.join("dir").join("a.txt")), "A");
    // The folder is not even tried, so no failed removal is reported.
    assert_eq!(rep.warnings, Vec::new());
    let moves = vec![MovedItem {
        from: v(f.src.join("dir/a.txt")),
        to: v(f.dst.join("dir/a.txt")),
    }];
    assert_eq!(
        rep.undo,
        Some(UndoRecord::Moved {
            moves,
            created_dirs: vec![],
            removed_dirs: vec![],
            overwritten: 0,
            copied_dirs: vec![],
        })
    );
}

/// A move that stops early can still be undone as far as it went.
#[test]
fn a_move_that_stops_early_records_what_it_moved() {
    let f = fx();
    write_tree(&f.src, &[("a.txt", "A"), ("b.txt", "B"), ("c.txt", "C")]);
    // The rename of `b.txt` fails, and the policy stops the job there.
    let p = FaultyProvider::local(vec![Fault::FailRename { call: 1 }]);
    let options = TransferOptions {
        errors: ErrorPolicy::Abort,
        ..TransferOptions::default()
    };
    let sources = ["a.txt", "b.txt", "c.txt"].map(|name| f.src.join(name));
    let job = spec(TransferMode::Move, &sources, &f.dst, options);
    let rep = run_on(&job, &p.vfs(), &NonInteractive);
    assert_eq!(
        (rep.outcome, rep.failures[0].stage),
        (Outcome::Aborted, Stage::Commit)
    );
    assert_eq!(
        tree(&f.src).into_keys().collect::<Vec<_>>(),
        ["b.txt", "c.txt"]
    );
    let moves = vec![MovedItem {
        from: v(f.src.join("a.txt")),
        to: v(f.dst.join("a.txt")),
    }];
    assert_eq!(
        rep.undo,
        Some(UndoRecord::Moved {
            moves,
            created_dirs: vec![],
            removed_dirs: vec![],
            overwritten: 0,
            copied_dirs: vec![],
        })
    );
}

/// An empty folder moved across volumes moves no file, yet the move can still be undone.
#[test]
fn an_empty_folder_moved_across_volumes_can_be_undone() {
    let f = fx();
    write_tree(&f.src, &[("empty/", "")]);
    let p = FaultyProvider::local(vec![Fault::CrossDevice]);
    let rep = run_on(
        &mv(&[f.src.join("empty")], &f.dst),
        &p.vfs(),
        &NonInteractive,
    );
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    assert!(f.dst.join("empty").is_dir() && !f.src.join("empty").exists());
    assert_eq!(
        rep.undo,
        Some(UndoRecord::Moved {
            moves: vec![],
            created_dirs: vec![v(f.dst.join("empty"))],
            removed_dirs: vec![v(f.src.join("empty"))],
            overwritten: 0,
            copied_dirs: vec![MovedItem {
                from: v(f.src.join("empty")),
                to: v(f.dst.join("empty")),
            }],
        })
    );
}

/// A move carries a link as a link, across volumes too and whatever the link mode says: the
/// copy is a link, and only the link is deleted at the source, never what it leads to.
#[cfg(unix)]
#[test]
fn a_move_carries_links_as_links() {
    use rf_jobs::SymlinkMode;
    use rf_jobs::testkit::Node;
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A"), ("outside/keep.txt", "keep")]);
    std::os::unix::fs::symlink("../outside", f.src.join("dir").join("link")).unwrap();
    let p = FaultyProvider::local(vec![Fault::CrossDevice]);
    let options = TransferOptions {
        symlinks: SymlinkMode::Follow,
        ..TransferOptions::default()
    };
    let job = spec(TransferMode::Move, &[f.src.join("dir")], &f.dst, options);
    let rep = run_on(&job, &p.vfs(), &NonInteractive);
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    assert_eq!(
        tree(&f.dst.join("dir")).get("link"),
        Some(&Node::Link("../outside".into()))
    );
    assert!(!f.src.join("dir").exists());
    assert_eq!(read(&f.src.join("outside").join("keep.txt")), "keep");
}

/// A copy whose name in its folder cannot be made durable keeps its source: after a power
/// loss the delete could be on disk and the copy not.
#[test]
fn a_copy_that_cannot_be_made_durable_keeps_its_source() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "A").unwrap();
    let p = FaultyProvider::local(vec![Fault::CrossDevice, Fault::FailSyncDir]);
    let rep = run_on(
        &mv(&[f.src.join("a.txt")], &f.dst),
        &p.vfs(),
        &NonInteractive,
    );
    assert_eq!(
        (rep.failures.len(), rep.failures[0].stage),
        (1, Stage::DeleteSource)
    );
    assert!(
        rep.failures[0]
            .message
            .contains("injected sync_dir failure"),
        "{}",
        rep.failures[0].message
    );
    assert_eq!(
        (read(&f.src.join("a.txt")), read(&f.dst.join("a.txt"))),
        ("A".into(), "A".into())
    );
    assert_eq!(rep.undo, None);
}

/// A folder made at the target whose entry cannot be made durable gets nothing moved into it,
/// and is taken back.
#[test]
fn a_folder_made_for_a_move_that_cannot_be_made_durable_is_taken_back() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A")]);
    let p = FaultyProvider::local(vec![Fault::CrossDevice, Fault::FailSyncDir]);
    let rep = run_on(&mv(&[f.src.join("dir")], &f.dst), &p.vfs(), &NonInteractive);
    assert_eq!(
        (rep.failures.len(), rep.failures[0].stage),
        (1, Stage::Write)
    );
    assert_eq!(read(&f.src.join("dir").join("a.txt")), "A");
    assert_eq!(tree(&f.dst), BTreeMap::new());
    assert_eq!(p.opened_writes(), 0);
    assert_eq!(rep.undo, None);
}

/// The target folder is the source folder through a link, and nothing tells them apart by
/// identity: the copy would replace the source with itself and the move then delete the only
/// copy. The temporary file shows up next to the source, and the commit is refused.
#[cfg(unix)]
#[test]
fn a_move_into_its_own_folder_through_a_link_never_deletes_it() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "A").unwrap();
    std::os::unix::fs::symlink(&f.src, f.dst.join("alias")).unwrap();
    let p = FaultyProvider::local(vec![Fault::NoIdentity, Fault::CrossDevice]);
    let job = spec(
        TransferMode::Move,
        &[f.src.join("a.txt")],
        &f.dst.join("alias"),
        with(ConflictPolicy::Overwrite),
    );
    let rep = run_on(&job, &p.vfs(), &NonInteractive);
    assert_eq!(
        (
            rep.failures.len(),
            rep.failures[0].stage,
            rep.failures[0].kind
        ),
        (1, Stage::Commit, ErrorKind::Unsupported),
        "{:?}",
        rep.failures
    );
    // The source, and no temporary file next to it.
    assert_eq!(tree(&f.src).into_keys().collect::<Vec<_>>(), ["a.txt"]);
    assert_eq!(read(&f.src.join("a.txt")), "A");
    assert_eq!(rep.undo, None);
}

/// A move that the plan turned into a copy still does nothing when it is dropped back onto
/// its own folder (DND-09), instead of making a numbered copy.
#[test]
fn a_move_turned_into_a_copy_onto_its_own_folder_does_nothing() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "me").unwrap();
    let p = FaultyProvider::local(vec![Fault::ReadOnly { under: v(&f.src) }]);
    let rep = run_on(
        &mv(&[f.src.join("a.txt")], &f.src),
        &p.vfs(),
        &NonInteractive,
    );
    assert!(rep.copied_instead_of_moved);
    assert_eq!(
        (rep.outcome, rep.skipped, rep.failures.len()),
        (Outcome::Completed, 1, 0)
    );
    assert_eq!(tree(&f.src).into_keys().collect::<Vec<_>>(), ["a.txt"]);
    assert_eq!(rep.undo, None);
}

/// A cancel in the middle of a copy across volumes leaves the source as it was, and nothing
/// at the target, not even a temporary file.
#[test]
fn cancelling_a_move_in_the_middle_of_a_copy_keeps_the_source() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "A").unwrap();
    let p = FaultyProvider::local(vec![Fault::CrossDevice, Fault::Hold { file: 0 }]);
    let (vfs, control) = (p.vfs(), Control::new());
    let job = mv(&[f.src.join("a.txt")], &f.dst);
    let rep = std::thread::scope(|scope| {
        let running = scope.spawn(|| {
            let env = ExecEnv::new(&vfs, &NonInteractive).control(control.clone());
            execute(JobId::generate(), &job, &env)
        });
        let held = p.wait_held(Duration::from_secs(10));
        control.cancel();
        p.release();
        assert!(held, "the copy never reached its first write");
        running.join().unwrap()
    });
    assert_eq!(rep.outcome, Outcome::Cancelled);
    assert_eq!(read(&f.src.join("a.txt")), "A");
    assert_eq!(tree(&f.dst), BTreeMap::new());
    assert_eq!(rep.undo, None);
}

/// A file two levels down that stays keeps both folders above it at the source.
#[test]
fn a_grandchild_that_stays_keeps_both_its_source_folders() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/sub/b.txt", "B")]);
    // Nothing called `b.txt` can be examined, so `b.txt` is not moved: the move looks at its
    // source again first, and fails there.
    let p = FaultyProvider::local(vec![
        Fault::CrossDevice,
        Fault::FailStat {
            name: "b.txt".into(),
        },
    ]);
    let rep = run_on(&mv(&[f.src.join("dir")], &f.dst), &p.vfs(), &NonInteractive);
    assert_eq!(
        (rep.failures.len(), rep.failures[0].stage),
        (1, Stage::Read)
    );
    assert_eq!(
        tree(&f.src.join("dir")).into_keys().collect::<Vec<_>>(),
        ["sub", "sub/b.txt"]
    );
    assert_eq!(read(&f.src.join("dir/sub/b.txt")), "B");
    assert_eq!(read(&f.dst.join("dir/a.txt")), "A");
    assert_eq!(rep.warnings, Vec::new());
    let moves = vec![MovedItem {
        from: v(f.src.join("dir/a.txt")),
        to: v(f.dst.join("dir/a.txt")),
    }];
    assert_eq!(
        rep.undo,
        Some(UndoRecord::Moved {
            moves,
            created_dirs: vec![v(f.dst.join("dir")), v(f.dst.join("dir/sub"))],
            removed_dirs: vec![],
            overwritten: 0,
            copied_dirs: vec![
                MovedItem {
                    from: v(f.src.join("dir")),
                    to: v(f.dst.join("dir")),
                },
                MovedItem {
                    from: v(f.src.join("dir/sub")),
                    to: v(f.dst.join("dir/sub")),
                },
            ],
        })
    );
}

/// Folders moved into their own folder through a link, without identities: each merges into
/// itself, so every empty folder in it would look moved and be removed. A merged folder whose
/// identity is unknown is kept instead, with a warning, and nothing at the source is lost.
#[cfg(unix)]
#[test]
fn a_folder_merged_into_itself_through_a_link_is_kept() {
    for faults in [
        vec![Fault::NoIdentity],
        vec![Fault::NoIdentity, Fault::CrossDevice],
    ] {
        let f = fx();
        write_tree(
            &f.src,
            &[
                ("dir/a.txt", "A"),
                ("dir/empty/", ""),
                ("only/e1/", ""),
                ("only/e2/", ""),
            ],
        );
        let before = tree(&f.src);
        std::os::unix::fs::symlink(&f.src, f.dst.join("alias")).unwrap();
        let p = FaultyProvider::local(faults.clone());
        let job = mv(
            &[f.src.join("dir"), f.src.join("only")],
            &f.dst.join("alias"),
        );
        let rep = run_on(&job, &p.vfs(), &NonInteractive);
        assert_eq!(tree(&f.src), before, "{faults:?}");
        assert_eq!(
            (rep.outcome, rep.skipped, rep.failures.len()),
            (Outcome::Completed, 1, 0),
            "{faults:?}: {:?}",
            rep.failures
        );
        assert!(
            rep.warnings.iter().all(|w| w.stage == Stage::DeleteSource),
            "{faults:?}: {:?}",
            rep.warnings
        );
        let kept: BTreeSet<_> = rep
            .warnings
            .iter()
            .filter_map(|w| w.source.clone())
            .collect();
        let expected = ["dir/empty", "only/e1", "only/e2"].map(|dir| v(f.src.join(dir)));
        assert_eq!(
            (rep.warnings.len(), kept),
            (3, BTreeSet::from(expected)),
            "{faults:?}"
        );
        assert_eq!(rep.undo, None, "{faults:?}");
    }
}

/// Files moved onto themselves through a link, without identities: a rename onto itself does
/// nothing and is no move, and a copy onto itself is refused. Nothing is lost or recorded.
#[cfg(unix)]
#[test]
fn files_moved_onto_themselves_through_a_link_are_not_moves() {
    for (faults, skipped, refused) in [
        (vec![Fault::NoIdentity], 2, 0),
        (vec![Fault::NoIdentity, Fault::CrossDevice], 0, 2),
    ] {
        let f = fx();
        write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/sub/b.txt", "B")]);
        let before = tree(&f.src);
        std::os::unix::fs::symlink(&f.src, f.dst.join("alias")).unwrap();
        let p = FaultyProvider::local(faults.clone());
        let job = spec(
            TransferMode::Move,
            &[f.src.join("dir")],
            &f.dst.join("alias"),
            with(ConflictPolicy::Overwrite),
        );
        let rep = run_on(&job, &p.vfs(), &NonInteractive);
        assert_eq!(tree(&f.src), before, "{faults:?}");
        let failures: Vec<_> = rep.failures.iter().map(|x| (x.stage, x.kind)).collect();
        assert_eq!(
            (rep.skipped, failures),
            (
                skipped,
                vec![(Stage::Commit, ErrorKind::Unsupported); refused]
            ),
            "{faults:?}"
        );
        assert_eq!(rep.warnings, Vec::new(), "{faults:?}");
        assert_eq!(rep.undo, None, "{faults:?}");
    }
}

/// A move turned into a copy, whose target is a second name of the source itself (a hard
/// link), does nothing, as a move would, instead of making a numbered copy.
#[cfg(unix)]
#[test]
fn a_move_turned_into_a_copy_onto_a_second_name_of_itself_does_nothing() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "me").unwrap();
    std::fs::hard_link(f.src.join("a.txt"), f.dst.join("a.txt")).unwrap();
    let p = FaultyProvider::local(vec![Fault::ReadOnly { under: v(&f.src) }]);
    let rep = run_on(
        &mv(&[f.src.join("a.txt")], &f.dst),
        &p.vfs(),
        &NonInteractive,
    );
    assert!(rep.copied_instead_of_moved);
    assert_eq!(
        (rep.outcome, rep.skipped, rep.failures.len()),
        (Outcome::Completed, 1, 0)
    );
    assert_eq!(tree(&f.dst).into_keys().collect::<Vec<_>>(), ["a.txt"]);
    assert_eq!(rep.undo, None);
}

/// A folder made for a move whose name cannot be made durable, and which cannot be removed
/// again either, stays, and the report says so.
#[test]
fn a_made_folder_that_cannot_be_taken_back_is_reported() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A")]);
    let p = FaultyProvider::local(vec![
        Fault::CrossDevice,
        Fault::FailSyncDir,
        Fault::FailRemoveDir,
    ]);
    let rep = run_on(&mv(&[f.src.join("dir")], &f.dst), &p.vfs(), &NonInteractive);
    assert_eq!(
        (rep.failures.len(), rep.failures[0].stage),
        (1, Stage::Write)
    );
    let warnings: Vec<_> = rep
        .warnings
        .iter()
        .map(|w| (w.stage, w.target.clone()))
        .collect();
    assert_eq!(warnings, [(Stage::Write, Some(v(f.dst.join("dir"))))]);
    assert!(f.dst.join("dir").is_dir());
    assert_eq!(read(&f.src.join("dir").join("a.txt")), "A");
}

/// Folders moved into their own folder through a second mount of it (bindfs, an NFS or SMB
/// loopback, one share mounted twice): the two sides have identities, but different ones, so
/// each folder merges into itself. A marker made in the merge target shows through at the
/// source, so no merged folder is removed, and no file is copied over itself.
#[cfg(unix)]
#[test]
fn a_folder_merged_into_itself_across_mounts_is_kept() {
    use rf_jobs::testkit::Node;
    for (policy, skipped, refused) in [
        (ConflictPolicy::Ask, 1, 0),
        (ConflictPolicy::Overwrite, 0, 1),
    ] {
        let f = fx();
        write_tree(
            &f.src,
            &[
                ("dir/a.txt", "A"),
                ("dir/empty/", ""),
                ("only/e1/", ""),
                ("only/e2/", ""),
            ],
        );
        let before = tree(&f.src);
        std::os::unix::fs::symlink(&f.src, f.dst.join("alias")).unwrap();
        let p = FaultyProvider::local(vec![
            Fault::CrossDevice,
            Fault::OtherVolume {
                under: v(f.dst.join("alias")),
            },
        ]);
        let job = spec(
            TransferMode::Move,
            &[f.src.join("dir"), f.src.join("only")],
            &f.dst.join("alias"),
            with(policy),
        );
        let rep = run_on(&job, &p.vfs(), &NonInteractive);
        // Every folder and file is there, and no marker or temporary file is left.
        assert_eq!(tree(&f.src), before, "{policy:?}");
        assert_eq!(
            tree(&f.dst),
            BTreeMap::from([("alias".to_string(), Node::Link(f.src.clone()))])
        );
        let failures: Vec<_> = rep.failures.iter().map(|x| (x.stage, x.kind)).collect();
        assert_eq!(
            (rep.skipped, failures),
            (
                skipped,
                vec![(Stage::Commit, ErrorKind::Unsupported); refused]
            ),
            "{policy:?}"
        );
        assert!(
            rep.warnings.iter().all(|w| w.stage == Stage::DeleteSource),
            "{policy:?}: {:?}",
            rep.warnings
        );
        let kept: BTreeSet<_> = rep
            .warnings
            .iter()
            .filter_map(|w| w.source.clone())
            .collect();
        let expected = ["dir/empty", "only/e1", "only/e2"].map(|dir| v(f.src.join(dir)));
        assert_eq!(
            (rep.warnings.len(), kept),
            (3, BTreeSet::from(expected)),
            "{policy:?}"
        );
        assert_eq!(rep.undo, None, "{policy:?}");
    }
}

/// A merge into a folder that really is on another volume: the marker is not seen at the
/// source, so the emptied source folders are removed as usual, and the marker is gone.
#[test]
fn a_merge_across_volumes_still_removes_the_emptied_source_folders() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/empty/", "")]);
    write_tree(&f.dst, &[("dir/other.txt", "O"), ("dir/empty/", "")]);
    let p = FaultyProvider::local(vec![
        Fault::CrossDevice,
        Fault::OtherVolume { under: v(&f.dst) },
    ]);
    let rep = run_on(&mv(&[f.src.join("dir")], &f.dst), &p.vfs(), &NonInteractive);
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    assert_eq!(rep.warnings, Vec::new());
    assert!(!f.src.join("dir").exists());
    assert_eq!(
        tree(&f.dst).into_keys().collect::<Vec<_>>(),
        ["dir", "dir/a.txt", "dir/empty", "dir/other.txt"]
    );
    let moves = vec![MovedItem {
        from: v(f.src.join("dir/a.txt")),
        to: v(f.dst.join("dir/a.txt")),
    }];
    assert_eq!(
        rep.undo,
        Some(UndoRecord::Moved {
            moves,
            created_dirs: vec![],
            removed_dirs: vec![v(f.src.join("dir/empty")), v(f.src.join("dir"))],
            overwritten: 0,
            copied_dirs: vec![],
        })
    );
}

/// A folder on another volume than the folder it is in (a mount point) inside a folder moved
/// across volumes is never entered: what is on that volume stays, and so do the folders above
/// it. The rest moves.
#[test]
fn a_move_never_enters_a_folder_on_another_volume() {
    let f = fx();
    write_tree(
        &f.src,
        &[
            ("dir/a.txt", "A"),
            ("dir/mnt/x.txt", "X"),
            ("dir/sub/b.txt", "B"),
        ],
    );
    let mnt = f.src.join("dir").join("mnt");
    let p = FaultyProvider::local(vec![
        Fault::CrossDevice,
        Fault::OtherVolume { under: v(&mnt) },
    ]);
    let rep = run_on(&mv(&[f.src.join("dir")], &f.dst), &p.vfs(), &NonInteractive);
    assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
    assert_eq!(
        tree(&f.src.join("dir")).into_keys().collect::<Vec<_>>(),
        ["mnt", "mnt/x.txt"]
    );
    assert_eq!(read(&mnt.join("x.txt")), "X");
    assert_eq!(
        tree(&f.dst.join("dir")).into_keys().collect::<Vec<_>>(),
        ["a.txt", "sub", "sub/b.txt"]
    );
    assert_eq!(rep.failures.len(), 1, "{:?}", rep.failures);
    let failure = &rep.failures[0];
    assert_eq!(
        (failure.kind, failure.source.clone()),
        (ErrorKind::Unsupported, Some(v(&mnt)))
    );
    assert!(
        failure.message.contains("is on another volume"),
        "{}",
        failure.message
    );
    assert_eq!(failure.reason, Some(rf_jobs::FailureReason::OtherVolume));
}

/// The undo of a folder moved across volumes makes the source folders again with the times
/// they had: the folders made at the target kept them, and give them back before they go.
#[test]
fn undoing_a_move_across_volumes_gives_the_folders_their_times_back() {
    let f = fx();
    write_tree(
        &f.src,
        &[
            ("dir/a.txt", "A"),
            ("dir/sub/b.txt", "B"),
            ("dir/empty/", ""),
        ],
    );
    let times = [
        ("dir/sub", 1_000_000_000),
        ("dir/empty", 1_100_000_000),
        ("dir", 1_200_000_000),
    ];
    for (folder, secs) in times {
        set_mtime(&f.src.join(folder), secs);
    }
    let p = FaultyProvider::local(vec![Fault::CrossDevice]);
    let rep = run_on(&mv(&[f.src.join("dir")], &f.dst), &p.vfs(), &NonInteractive);
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    for (folder, secs) in times {
        assert_eq!(
            mtime(&f.dst.join(folder)),
            at(secs),
            "{folder} at the target"
        );
    }
    let record = rep.undo.expect("the move can be undone");
    let undo = run_on(&JobSpec::Undo(record), &p.vfs(), &NonInteractive);
    assert_eq!(undo.outcome, Outcome::Completed, "{:?}", undo.failures);
    assert!(!f.dst.join("dir").exists());
    assert_eq!(read(&f.src.join("dir/sub/b.txt")), "B");
    for (folder, secs) in times {
        assert_eq!(
            mtime(&f.src.join(folder)),
            at(secs),
            "{folder} back at the source"
        );
    }
    // The redo moves the folders forth again, and they keep their times that way too.
    let record = undo.undo.expect("the undo can be redone");
    let redo = run_on(&JobSpec::Undo(record), &p.vfs(), &NonInteractive);
    assert_eq!(redo.outcome, Outcome::Completed, "{:?}", redo.failures);
    assert!(!f.src.join("dir").exists());
    for (folder, secs) in times {
        assert_eq!(
            mtime(&f.dst.join(folder)),
            at(secs),
            "{folder} at the target again"
        );
    }
}

/// A move that replaced files under `Overwrite` cannot bring their old content back: its undo
/// moves the items back and says so once for each file it replaced, on one volume (a rename
/// over the file) and across volumes (a copy committed over it).
#[test]
fn undoing_a_move_that_overwrote_says_what_cannot_come_back() {
    for faults in [vec![], vec![Fault::CrossDevice]] {
        let f = fx();
        std::fs::write(f.src.join("a.txt"), "new").unwrap();
        std::fs::write(f.dst.join("a.txt"), "old").unwrap();
        std::fs::write(f.src.join("b.txt"), "b").unwrap();
        let p = FaultyProvider::local(faults.clone());
        let job = spec(
            TransferMode::Move,
            &[f.src.join("a.txt"), f.src.join("b.txt")],
            &f.dst,
            with(ConflictPolicy::Overwrite),
        );
        let rep = run_on(&job, &p.vfs(), &NonInteractive);
        assert_eq!(
            rep.outcome,
            Outcome::Completed,
            "{faults:?}: {:?}",
            rep.failures
        );
        assert_eq!(read(&f.dst.join("a.txt")), "new");
        let record = rep.undo.expect("the move can be undone");
        let undo = run_on(&JobSpec::Undo(record), &p.vfs(), &NonInteractive);
        assert_eq!(
            undo.outcome,
            Outcome::Completed,
            "{faults:?}: {:?}",
            undo.failures
        );
        assert_eq!(read(&f.src.join("a.txt")), "new");
        assert_eq!(tree(&f.dst), BTreeMap::new(), "{faults:?}");
        let warned: Vec<_> = undo.warnings.iter().map(|w| (w.stage, w.kind)).collect();
        assert_eq!(
            warned,
            [(Stage::Undo, ErrorKind::Unsupported)],
            "{faults:?}"
        );
        assert!(
            undo.warnings[0].message.contains("overwritten by the move"),
            "{}",
            undo.warnings[0].message
        );
        assert_eq!(
            undo.warnings[0].reason,
            Some(rf_jobs::FailureReason::Overwritten)
        );
    }
}

/// A merge across volumes of folders listed without identities, as the local provider lists
/// them on Windows, still knows each merged folder for its own: the move looks at each folder
/// with `stat`, which gives its identity, so the emptied source folders go as they do on Unix.
#[test]
fn a_merge_of_folders_listed_without_identities_still_removes_them() {
    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "A"), ("dir/empty/", "")]);
    write_tree(&f.dst, &[("dir/other.txt", "O"), ("dir/empty/", "")]);
    let p = FaultyProvider::local(vec![
        Fault::CrossDevice,
        Fault::NoListedIdentity,
        Fault::OtherVolume { under: v(&f.dst) },
    ]);
    let rep = run_on(&mv(&[f.src.join("dir")], &f.dst), &p.vfs(), &NonInteractive);
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    assert_eq!(rep.warnings, Vec::new());
    assert!(!f.src.join("dir").exists());
    assert_eq!(read(&f.dst.join("dir/a.txt")), "A");
}

/// Merges two empty folders across volumes with job `id` under `faults`: `src/dir/empty` into
/// `dst/dir/empty`. The marker that tells whether a merged folder is its own target is the
/// job's first temporary name, `temp_name(id, 0)`, made in `dst/dir/empty`.
fn merge_empty_folders(f: &Fx, id: JobId, faults: Vec<Fault>) -> rf_jobs::JobReport {
    write_tree(&f.src, &[("dir/empty/", "")]);
    write_tree(&f.dst, &[("dir/empty/", "")]);
    let mut faults = faults;
    faults.extend([Fault::CrossDevice, Fault::OtherVolume { under: v(&f.dst) }]);
    let p = FaultyProvider::local(faults);
    let (vfs, res) = (p.vfs(), NonInteractive);
    execute(
        id,
        &mv(&[f.src.join("dir")], &f.dst),
        &ExecEnv::new(&vfs, &res),
    )
}

/// The marker cannot be looked for at the source: whether the merged folder is the target
/// folder under another path cannot be told, so it is kept, and so is the folder above it,
/// with a warning that says why. The marker itself is gone.
#[test]
fn a_marker_that_cannot_be_looked_for_keeps_the_merged_folder() {
    let f = fx();
    let id = JobId(9);
    let marker = rf_jobs::temp_name(id, 0);
    let rep = merge_empty_folders(
        &f,
        id,
        vec![Fault::FailStat {
            name: marker.clone(),
        }],
    );
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    assert_eq!(
        tree(&f.src).into_keys().collect::<Vec<_>>(),
        ["dir", "dir/empty"]
    );
    assert_eq!(rep.warnings.len(), 1, "{:?}", rep.warnings);
    let warning = &rep.warnings[0];
    assert_eq!(
        (
            warning.stage,
            warning.source.clone(),
            warning.reason.clone()
        ),
        (
            Stage::DeleteSource,
            Some(v(f.src.join("dir/empty"))),
            Some(rf_jobs::FailureReason::CannotTellOwnFolder)
        )
    );
    let kept = format!(
        "kept {}: cannot tell whether it is the target folder: ",
        v(f.src.join("dir/empty"))
    );
    assert!(warning.message.contains(&kept), "{}", warning.message);
    assert_eq!(temp_files(&f.dst), Vec::<String>::new());
}

/// The marker cannot be removed again: it stays under its temporary name, and the report names
/// it. It was not seen at the source, so the merged folders are removed there as usual.
#[test]
fn a_marker_that_cannot_be_removed_is_reported() {
    let f = fx();
    let id = JobId(9);
    let marker = rf_jobs::temp_name(id, 0);
    let rep = merge_empty_folders(
        &f,
        id,
        vec![Fault::FailDelete {
            name: marker.clone(),
        }],
    );
    assert_eq!(rep.outcome, Outcome::Completed, "{:?}", rep.failures);
    assert!(!f.src.join("dir").exists());
    let left = f.dst.join("dir").join("empty").join(&marker);
    assert!(left.is_file());
    let warned: Vec<_> = rep
        .warnings
        .iter()
        .map(|w| (w.stage, w.target.clone(), w.reason.clone()))
        .collect();
    assert_eq!(
        warned,
        [(
            Stage::Write,
            Some(v(&left)),
            Some(rf_jobs::FailureReason::TempLeftBehind)
        )]
    );
}
