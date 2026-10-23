#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use common::*;
use rf_core::ErrorKind;
use rf_jobs::testkit::{Fault, FaultyProvider, ScriptedResolver, tree, write_tree};
use rf_jobs::{
    ConflictPolicy, ErrorAction, ErrorAnswer, ErrorPolicy, JobSpec, MovedItem, NonInteractive,
    Outcome, Stage, Totals, TransferItem, TransferMode, TransferOptions, TransferSpec, UndoRecord,
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
            removed_dirs: vec![]
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
            removed_dirs: vec![v(f.src.join("dir"))]
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
            removed_dirs: vec![]
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
    use std::time::Duration;
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
            removed_dirs: vec![]
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
            removed_dirs: vec![]
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
            removed_dirs: vec![v(f.src.join("empty"))]
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
