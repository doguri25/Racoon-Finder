#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

mod common;

use std::path::{Path, PathBuf};

use common::*;
use rf_core::{ErrorKind, Os};
use rf_jobs::testkit::{ResolverCall, ScriptedResolver, temp_files, tree, write_tree};
use rf_jobs::{
    Conflict, ConflictAction, ConflictAnswer, ConflictPolicy, ErrorAction, ErrorAnswer,
    ErrorPolicy, ExecEnv, Issue, ItemFailure, JobId, JobReport, JobSpec, NonInteractive, Outcome,
    PreflightAnswer, Resolver, Stage, TransferMode, TransferOptions, execute,
};
use rf_vfs::Vfs;

fn with(policy: ConflictPolicy) -> TransferOptions {
    TransferOptions {
        conflict: policy,
        ..TransferOptions::default()
    }
}

fn both(f: &Fx, name: &str, src: &str, dst: &str) {
    std::fs::write(f.src.join(name), src).unwrap();
    std::fs::write(f.dst.join(name), dst).unwrap();
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

fn run_os(job: &JobSpec, os: Os) -> JobReport {
    let (vfs, res) = (Vfs::with_local(), NonInteractive);
    execute(
        JobId::generate(),
        job,
        &ExecEnv::new(&vfs, &res).naming_os(os),
    )
}

#[test]
fn skip_keeps_the_existing_file() {
    let f = fx();
    both(&f, "a.txt", "new", "old");
    let r = run(&spec(
        TransferMode::Copy,
        &[f.src.join("a.txt")],
        &f.dst,
        with(ConflictPolicy::Skip),
    ));
    assert_eq!(
        (r.outcome, r.skipped, r.done.files),
        (Outcome::Completed, 1, 0)
    );
    assert_eq!(read(&f.dst.join("a.txt")), "old");
}

#[test]
fn overwrite_replaces_the_file() {
    let f = fx();
    both(&f, "a.txt", "new", "old");
    let r = run(&spec(
        TransferMode::Copy,
        &[f.src.join("a.txt")],
        &f.dst,
        with(ConflictPolicy::Overwrite),
    ));
    assert_eq!((r.outcome, r.done.files), (Outcome::Completed, 1));
    assert_eq!(read(&f.dst.join("a.txt")), "new");
    assert_eq!(temp_files(&f.dst), Vec::<String>::new());
}

#[test]
fn if_newer_ignores_two_second_clock_noise() {
    let f = fx();
    both(&f, "a.txt", "new", "old");
    set_mtime(&f.dst.join("a.txt"), 1_000_000_000);
    set_mtime(&f.src.join("a.txt"), 1_000_000_001);
    let job = spec(
        TransferMode::Copy,
        &[f.src.join("a.txt")],
        &f.dst,
        with(ConflictPolicy::OverwriteIfNewer),
    );
    assert_eq!(run(&job).skipped, 1);
    assert_eq!(read(&f.dst.join("a.txt")), "old");
    set_mtime(&f.src.join("a.txt"), 1_000_000_003);
    assert_eq!(run(&job).done.files, 1);
    assert_eq!(read(&f.dst.join("a.txt")), "new");
}

#[test]
fn if_size_differs() {
    let f = fx();
    both(&f, "a.txt", "abc", "xyz");
    let job = spec(
        TransferMode::Copy,
        &[f.src.join("a.txt")],
        &f.dst,
        with(ConflictPolicy::OverwriteIfSizeDiffers),
    );
    assert_eq!(run(&job).skipped, 1);
    std::fs::write(f.src.join("a.txt"), "abcd").unwrap();
    assert_eq!(run(&job).done.files, 1);
    assert_eq!(read(&f.dst.join("a.txt")), "abcd");
}

#[test]
fn auto_rename_follows_the_os_rule() {
    let f = fx();
    both(&f, "a.txt", "new", "old");
    std::fs::write(f.dst.join("a (2).txt"), "taken").unwrap();
    let job = spec(
        TransferMode::Copy,
        &[f.src.join("a.txt")],
        &f.dst,
        with(ConflictPolicy::AutoRename),
    );
    assert_eq!(
        run_os(&job, Os::Windows).outputs,
        [v(f.dst.join("a (3).txt"))]
    );
    assert_eq!(run_os(&job, Os::MacOs).outputs, [v(f.dst.join("a 2.txt"))]);
    assert_eq!(
        (read(&f.dst.join("a (3).txt")), read(&f.dst.join("a.txt"))),
        ("new".into(), "old".into())
    );
}

#[test]
fn ask_with_apply_to_all_asks_once() {
    let f = fx();
    for n in ["a", "b", "c"] {
        both(&f, n, "new", "old");
    }
    let r = ScriptedResolver::new().on_conflict(ConflictAnswer::all(ConflictAction::Overwrite));
    let srcs: Vec<PathBuf> = ["a", "b", "c"].iter().map(|n| f.src.join(n)).collect();
    let rep = run_on(&copy(&srcs, &f.dst), &Vfs::with_local(), &r);
    assert_eq!((rep.done.files, r.calls().len()), (3, 1));
    for n in ["a", "b", "c"] {
        assert_eq!(read(&f.dst.join(n)), "new");
    }
}

#[test]
fn ask_cancel_stops_the_job() {
    let f = fx();
    both(&f, "a", "new", "old");
    std::fs::write(f.src.join("b"), "b").unwrap();
    let r = ScriptedResolver::new().on_conflict(ConflictAnswer::once(ConflictAction::Cancel));
    let rep = run_on(
        &copy(&[f.src.join("a"), f.src.join("b")], &f.dst),
        &Vfs::with_local(),
        &r,
    );
    assert_eq!(rep.outcome, Outcome::Cancelled);
    assert!(!f.dst.join("b").exists());
    assert_eq!(read(&f.dst.join("a")), "old");
}

#[test]
fn a_file_never_replaces_a_folder() {
    let f = fx();
    std::fs::write(f.src.join("x"), "file").unwrap();
    write_tree(&f.dst, &[("x/inner.txt", "keep")]);
    let rep = run(&spec(
        TransferMode::Copy,
        &[f.src.join("x")],
        &f.dst,
        with(ConflictPolicy::Overwrite),
    ));
    assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
    assert_eq!(
        (rep.failures[0].stage, rep.failures[0].kind),
        (Stage::Conflict, ErrorKind::AlreadyExists)
    );
    assert!(
        rep.failures[0]
            .message
            .contains(&v(f.dst.join("x")).to_string())
    );
    assert_eq!(read(&f.dst.join("x").join("inner.txt")), "keep");
    assert_eq!(temp_files(&f.dst), Vec::<String>::new());
}

#[test]
fn copy_onto_itself_makes_a_numbered_copy() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "me").unwrap();
    let rep = run_os(
        &spec(
            TransferMode::Copy,
            &[f.src.join("a.txt")],
            &f.src,
            with(ConflictPolicy::Overwrite),
        ),
        Os::Windows,
    );
    assert_eq!(rep.outputs, [v(f.src.join("a (2).txt"))]);
    assert_eq!(
        (read(&f.src.join("a.txt")), read(&f.src.join("a (2).txt"))),
        ("me".into(), "me".into())
    );
}

#[test]
fn errors_continue_by_default() {
    let f = fx();
    std::fs::write(f.src.join("ok"), "ok").unwrap();
    let rep = run(&copy(&[f.src.join("missing"), f.src.join("ok")], &f.dst));
    assert_eq!(
        (rep.outcome, rep.failures.len(), rep.done.files),
        (Outcome::CompletedWithErrors, 1, 1)
    );
}

#[test]
fn abort_policy_stops_at_the_first_failure() {
    let f = fx();
    std::fs::write(f.src.join("ok"), "ok").unwrap();
    let opts = TransferOptions {
        errors: ErrorPolicy::Abort,
        ..TransferOptions::default()
    };
    let rep = run(&spec(
        TransferMode::Copy,
        &[f.src.join("missing"), f.src.join("ok")],
        &f.dst,
        opts,
    ));
    assert_eq!(rep.outcome, Outcome::Aborted);
    assert!(!f.dst.join("ok").exists());
}

struct FixThenRetry(PathBuf);

impl Resolver for FixThenRetry {
    fn conflict(&self, _: JobId, _: &Conflict) -> ConflictAnswer {
        unreachable!("no conflicts in this test")
    }
    fn error(&self, _: JobId, _: &ItemFailure) -> ErrorAnswer {
        std::fs::write(&self.0, "late").unwrap();
        ErrorAnswer {
            action: ErrorAction::Retry,
            apply_to_all: false,
        }
    }
    fn preflight(&self, _: JobId, _: &[Issue]) -> PreflightAnswer {
        PreflightAnswer::Proceed
    }
}

#[test]
fn ask_retry_runs_the_item_again() {
    let f = fx();
    let late = f.src.join("late.txt");
    let opts = TransferOptions {
        errors: ErrorPolicy::Ask,
        ..TransferOptions::default()
    };
    let rep = run_on(
        &spec(
            TransferMode::Copy,
            std::slice::from_ref(&late),
            &f.dst,
            opts,
        ),
        &Vfs::with_local(),
        &FixThenRetry(late),
    );
    assert_eq!((rep.outcome, rep.done.files), (Outcome::Completed, 1));
    assert_eq!(read(&f.dst.join("late.txt")), "late");
}

#[test]
fn ask_skip_for_all_asks_once() {
    let f = fx();
    let r = ScriptedResolver::new().on_error(ErrorAnswer {
        action: ErrorAction::Skip,
        apply_to_all: true,
    });
    let opts = TransferOptions {
        errors: ErrorPolicy::Ask,
        ..TransferOptions::default()
    };
    let rep = run_on(
        &spec(
            TransferMode::Copy,
            &[f.src.join("m1"), f.src.join("m2")],
            &f.dst,
            opts,
        ),
        &Vfs::with_local(),
        &r,
    );
    assert_eq!(
        (rep.outcome, rep.failures.len()),
        (Outcome::CompletedWithErrors, 2)
    );
    assert_eq!(r.calls(), [ResolverCall::Error { stage: Stage::Plan }]);
}

// What follows goes beyond the brief: the rules the controller added for this task.

#[test]
fn an_answer_to_overwrite_still_never_replaces_a_folder() {
    let f = fx();
    std::fs::write(f.src.join("x"), "file").unwrap();
    write_tree(&f.dst, &[("x/inner.txt", "keep")]);
    let r = ScriptedResolver::new().on_conflict(ConflictAnswer::all(ConflictAction::Overwrite));
    let rep = run_on(
        &spec(
            TransferMode::Copy,
            &[f.src.join("x")],
            &f.dst,
            with(ConflictPolicy::Ask),
        ),
        &Vfs::with_local(),
        &r,
    );
    assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
    assert_eq!(
        (rep.failures[0].stage, rep.failures[0].kind),
        (Stage::Conflict, ErrorKind::AlreadyExists)
    );
    assert_eq!(
        r.calls(),
        [ResolverCall::Conflict {
            target: v(f.dst.join("x"))
        }]
    );
    assert_eq!(read(&f.dst.join("x").join("inner.txt")), "keep");
    assert_eq!(temp_files(&f.dst), Vec::<String>::new());
}

#[test]
fn a_remembered_overwrite_is_checked_again_for_each_pair() {
    let f = fx();
    both(&f, "a", "new", "old");
    std::fs::write(f.src.join("x"), "file").unwrap();
    write_tree(&f.dst, &[("x/inner.txt", "keep")]);
    both(&f, "z", "new", "old");
    let r = ScriptedResolver::new().on_conflict(ConflictAnswer::all(ConflictAction::Overwrite));
    let srcs: Vec<PathBuf> = ["a", "x", "z"].iter().map(|n| f.src.join(n)).collect();
    let rep = run_on(
        &spec(TransferMode::Copy, &srcs, &f.dst, with(ConflictPolicy::Ask)),
        &Vfs::with_local(),
        &r,
    );
    assert_eq!(
        (
            rep.outcome,
            rep.done.files,
            rep.failures.len(),
            r.calls().len()
        ),
        (Outcome::CompletedWithErrors, 2, 1, 1)
    );
    assert_eq!(rep.failures[0].source, Some(v(f.src.join("x"))));
    assert_eq!(rep.failures[0].stage, Stage::Conflict);
    assert_eq!(
        (read(&f.dst.join("a")), read(&f.dst.join("z"))),
        ("new".into(), "new".into())
    );
    assert_eq!(read(&f.dst.join("x").join("inner.txt")), "keep");
}

#[test]
fn ask_rename_keeps_both() {
    let f = fx();
    both(&f, "a.txt", "new", "old");
    let r = ScriptedResolver::new().on_conflict(ConflictAnswer::once(ConflictAction::Rename));
    let rep = run_on(
        &spec(
            TransferMode::Copy,
            &[f.src.join("a.txt")],
            &f.dst,
            with(ConflictPolicy::Ask),
        ),
        &Vfs::with_local(),
        &r,
    );
    assert_eq!(
        (rep.outcome, rep.done.files, rep.skipped),
        (Outcome::Completed, 1, 0)
    );
    assert_eq!(rep.outputs.len(), 1);
    assert_eq!(read(&f.dst.join("a.txt")), "old");
    let kept: Vec<_> = tree(&f.dst).into_keys().collect();
    assert_eq!(kept.len(), 2, "{kept:?}");
    let renamed = kept.iter().find(|name| *name != "a.txt").unwrap();
    assert_eq!(read(&f.dst.join(renamed)), "new");
    assert_eq!(rep.outputs, [v(f.dst.join(renamed))]);
}

#[test]
fn ask_skip_for_all_skips_every_later_conflict_unasked() {
    let f = fx();
    for n in ["a", "b"] {
        both(&f, n, "new", "old");
    }
    let r = ScriptedResolver::new().on_conflict(ConflictAnswer::all(ConflictAction::Skip));
    let srcs: Vec<PathBuf> = ["a", "b"].iter().map(|n| f.src.join(n)).collect();
    let rep = run_on(
        &spec(TransferMode::Copy, &srcs, &f.dst, with(ConflictPolicy::Ask)),
        &Vfs::with_local(),
        &r,
    );
    assert_eq!(
        (rep.outcome, rep.skipped, rep.done.files, r.calls().len()),
        (Outcome::Completed, 2, 0, 1)
    );
    assert_eq!(rep.failures, Vec::new());
}

#[test]
fn a_numbered_name_that_is_too_long_fails_cleanly() {
    let f = fx();
    // 254 characters: the number added to it takes the name past 255 units on every OS.
    let name = "n".repeat(254);
    both(&f, &name, "new", "old");
    let rep = run(&spec(
        TransferMode::Copy,
        &[f.src.join(&name)],
        &f.dst,
        with(ConflictPolicy::AutoRename),
    ));
    assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
    assert_eq!(
        (rep.failures[0].stage, rep.failures[0].kind),
        (Stage::Conflict, ErrorKind::InvalidPath)
    );
    assert!(
        rep.failures[0].message.contains("limit is 255"),
        "{:?}",
        rep.failures[0]
    );
    assert_eq!(read(&f.dst.join(&name)), "old");
    assert_eq!(tree(&f.dst).len(), 1);
}

#[cfg(unix)]
#[test]
fn a_temp_path_over_the_limit_fails_before_anything_is_written() {
    use std::collections::BTreeMap;
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "a").unwrap();
    // On macOS a path may hold 1,023 bytes. `deep/a.txt` fits; the temporary file next to
    // it (a name of 29 bytes) does not.
    let mut deep = f.dst.clone();
    let mut need = 1_000 - deep.as_os_str().len();
    while need > 1 {
        let part = need.min(200);
        deep.push("d".repeat(part - 1));
        need -= part;
    }
    std::fs::create_dir_all(&deep).unwrap();
    assert!(
        (994..=1_021).contains(&deep.as_os_str().len()),
        "{}",
        deep.as_os_str().len()
    );
    let job = spec(
        TransferMode::Copy,
        &[f.src.join("a.txt")],
        &deep,
        TransferOptions::default(),
    );
    let rep = run_os(&job, Os::MacOs);
    assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
    assert_eq!(
        (rep.failures[0].stage, rep.failures[0].kind),
        (Stage::Preflight, ErrorKind::InvalidPath)
    );
    assert_eq!(tree(&deep), BTreeMap::new());
}

/// A name no Unix file system accepts: `stat` fails with something other than `NotFound`,
/// which must never be read as "the name is free".
#[cfg(unix)]
#[test]
fn a_target_that_cannot_be_examined_is_not_taken_for_missing() {
    use std::collections::BTreeMap;

    use rf_jobs::testkit::FaultyProvider;
    use rf_jobs::{TransferItem, TransferSpec};
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "a").unwrap();
    let item = TransferItem {
        source: v(f.src.join("a.txt")),
        target: v(&f.dst).join("n".repeat(300)).unwrap(),
    };
    let job = JobSpec::Transfer(TransferSpec {
        mode: TransferMode::Copy,
        items: vec![item],
        options: with(ConflictPolicy::Overwrite),
    });
    let p = FaultyProvider::local(vec![]);
    let rep = run_on(&job, &p.vfs(), &NonInteractive);
    assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
    assert_eq!(rep.failures[0].stage, Stage::Conflict);
    assert_eq!((p.opened_writes(), rep.done.files), (0, 0));
    assert_eq!(tree(&f.dst), BTreeMap::new());
}

/// Two names for one file are one item: the copy gets a new name and nothing is replaced.
#[cfg(unix)]
#[test]
fn a_hard_link_to_the_source_counts_as_the_source() {
    let f = fx();
    std::fs::write(f.src.join("a.txt"), "me").unwrap();
    std::fs::hard_link(f.src.join("a.txt"), f.dst.join("a.txt")).unwrap();
    let job = spec(
        TransferMode::Copy,
        &[f.src.join("a.txt")],
        &f.dst,
        with(ConflictPolicy::Overwrite),
    );
    let rep = run_os(&job, Os::Windows);
    assert_eq!(rep.outputs, [v(f.dst.join("a (2).txt"))]);
    assert_eq!(read(&f.dst.join("a.txt")), "me");
    assert_eq!(read(&f.dst.join("a (2).txt")), "me");
}
