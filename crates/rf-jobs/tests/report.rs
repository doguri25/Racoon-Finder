#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

mod common;

use common::*;
use rf_jobs::testkit::tree;
use rf_jobs::{
    ConflictPolicy, FailureReason, ItemFailure, JobSpec, Outcome, Stage, SymlinkMode, TransferItem,
    TransferMode, TransferOptions, retry_spec,
};

#[test]
fn retry_spec_holds_only_failed_items() {
    let f = fx();
    std::fs::write(f.src.join("ok"), "ok").unwrap();
    let opts = TransferOptions {
        conflict: ConflictPolicy::Skip,
        ..TransferOptions::default()
    };
    let rep = run(&spec(
        TransferMode::Copy,
        &[f.src.join("m1"), f.src.join("ok"), f.src.join("m2")],
        &f.dst,
        opts,
    ));
    let Some(JobSpec::Transfer(retry)) = rep.retry_spec() else {
        panic!("{:?}", rep.retry_spec())
    };
    assert_eq!((retry.mode, retry.options), (TransferMode::Copy, opts));
    assert_eq!(
        retry
            .items
            .iter()
            .map(|i| i.source.clone())
            .collect::<Vec<_>>(),
        [v(f.src.join("m1")), v(f.src.join("m2"))]
    );
    for n in ["m1", "m2"] {
        std::fs::write(f.src.join(n), n).unwrap();
    }
    assert_eq!(run(&JobSpec::Transfer(retry)).outcome, Outcome::Completed);
    assert_eq!(tree(&f.dst).len(), 3);
}

#[test]
fn a_retry_leaves_out_the_failures_no_retry_can_help() {
    let f = fx();
    let failure = |name: &str, reason: Option<FailureReason>| ItemFailure {
        source: Some(v(f.src.join(name))),
        target: Some(v(f.dst.join(name))),
        stage: Stage::Plan,
        kind: rf_core::ErrorKind::Unsupported,
        message: String::new(),
        reason,
    };
    let failed = [
        failure("loop", Some(FailureReason::LinkLoop)),
        failure("gone", None),
        failure("inside", Some(FailureReason::IntoItself)),
        failure("deep", Some(FailureReason::TooDeep)),
    ];
    let job = |symlinks: SymlinkMode| {
        let options = TransferOptions {
            symlinks,
            ..TransferOptions::default()
        };
        spec(TransferMode::Copy, &[f.src.join("dir")], &f.dst, options)
    };
    let sources = |retry: Option<JobSpec>| match retry {
        Some(JobSpec::Transfer(retry)) => retry
            .items
            .into_iter()
            .map(|TransferItem { source, .. }| source)
            .collect::<Vec<_>>(),
        other => panic!("a transfer: {other:?}"),
    };
    // A link loop and a folder into itself are never asked for again. An item too deep is,
    // while links are copied as links: it is a real folder, which a retry starts from.
    assert_eq!(
        sources(retry_spec(&job(SymlinkMode::CopyLink), &failed)),
        [v(f.src.join("gone")), v(f.src.join("deep"))]
    );
    // While links are followed, "too deep" is how a loop ends where files have no identity.
    assert_eq!(
        sources(retry_spec(&job(SymlinkMode::Follow), &failed)),
        [v(f.src.join("gone"))]
    );
    // With nothing else to try again, there is no retry at all.
    for reason in [FailureReason::LinkLoop, FailureReason::IntoItself] {
        let only = [failure("x", Some(reason))];
        assert_eq!(retry_spec(&job(SymlinkMode::CopyLink), &only), None);
        assert_eq!(retry_spec(&job(SymlinkMode::Follow), &only), None);
    }
    let too_deep = [failure("x", Some(FailureReason::TooDeep))];
    assert_eq!(retry_spec(&job(SymlinkMode::Follow), &too_deep), None);
}

/// The case the rule is for, with real files: a folder copied with its links followed holds a
/// link back to itself. Symbolic links need no rights on Unix.
#[cfg(unix)]
#[test]
fn a_link_loop_is_not_retried_and_nothing_is_copied_twice() {
    use rf_jobs::testkit::write_tree;

    let f = fx();
    write_tree(&f.src, &[("dir/a.txt", "a")]);
    std::os::unix::fs::symlink(".", f.src.join("dir").join("loop")).unwrap();
    let options = TransferOptions {
        symlinks: SymlinkMode::Follow,
        ..TransferOptions::default()
    };
    let job = spec(
        TransferMode::Copy,
        &[f.src.join("dir"), f.src.join("gone")],
        &f.dst,
        options,
    );
    let rep = run(&job);
    assert_eq!(rep.outcome, Outcome::CompletedWithErrors);
    assert_eq!(
        std::fs::read_to_string(f.dst.join("dir/a.txt")).unwrap(),
        "a"
    );
    // Two failures, in the order the job met them: the source that is gone, and the loop.
    let loops = |failure: &&ItemFailure| failure.reason == Some(FailureReason::LinkLoop);
    assert_eq!(rep.failures.len(), 2, "{rep:?}");
    assert_eq!(rep.failures.iter().filter(loops).count(), 1, "{rep:?}");
    let before = tree(&f.dst);

    // The retry holds the item that is gone, and nothing for the loop.
    let retry = rep.retry_spec().expect("`gone` can be tried again");
    let JobSpec::Transfer(asked) = &retry else {
        panic!("a transfer: {retry:?}")
    };
    assert_eq!(asked.items.len(), 1, "{asked:?}");
    assert_eq!(asked.items[0].source, v(f.src.join("gone")));
    // Running it copies nothing a second time: no `dir/loop` with the folder in it again.
    let again = run(&retry);
    assert_eq!((again.done.files, again.done.dirs), (0, 0));
    assert_eq!(tree(&f.dst), before);
    assert!(!f.dst.join("dir").join("loop").exists());

    // A job whose only failure is the loop offers no retry.
    let other = f.root.join("other");
    std::fs::create_dir(&other).unwrap();
    let rep = run(&spec(
        TransferMode::Copy,
        &[f.src.join("dir")],
        &other,
        options,
    ));
    assert_eq!(rep.failures.len(), 1, "{rep:?}");
    assert_eq!(rep.retry_spec(), None);
}

#[test]
fn a_clean_job_has_nothing_to_retry() {
    let f = fx();
    std::fs::write(f.src.join("ok"), "ok").unwrap();
    assert_eq!(run(&copy(&[f.src.join("ok")], &f.dst)).retry_spec(), None);
}

#[test]
fn the_log_summarises_the_job() {
    let f = fx();
    std::fs::write(f.src.join("ok"), "12345").unwrap();
    let rep = run(&copy(&[f.src.join("missing"), f.src.join("ok")], &f.dst));
    let mut out = Vec::new();
    rep.write_log(&mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        format!("job {} copy completed_with_errors", rep.id)
    );
    assert_eq!(lines[1], "planned files=1 dirs=0 bytes=5");
    assert_eq!(
        lines[2],
        "done files=1 dirs=0 bytes=5 skipped=0 failed=1 warnings=0"
    );
    assert!(lines[3].starts_with("elapsed_ms="), "{}", lines[3]);
    let failure = format!(
        "failure stage=plan kind=NotFound source={} target=",
        v(f.src.join("missing"))
    );
    assert!(lines[4].starts_with(&failure), "{}", lines[4]);
}

#[test]
fn average_speed_is_bytes_over_elapsed_time() {
    let f = fx();
    std::fs::write(f.src.join("a"), pattern(2 << 20)).unwrap();
    let rep = run(&copy(&[f.src.join("a")], &f.dst));
    let ms = u64::try_from(rep.elapsed.as_millis()).unwrap().max(1);
    assert_eq!(rep.average_speed(), rep.done.bytes * 1000 / ms);
}

#[cfg(target_os = "linux")]
#[test]
fn a_failed_trash_can_be_retried_as_a_permanent_delete() {
    use rf_jobs::{DeleteMethod, DeleteSpec, ErrorPolicy};
    let f = fx();
    std::fs::write(f.src.join("a"), "a").unwrap();
    let rep = run(&JobSpec::Delete(DeleteSpec {
        paths: vec![v(f.src.join("a"))],
        method: DeleteMethod::Trash,
        errors: ErrorPolicy::Continue,
    }));
    let Some(JobSpec::Delete(mut retry)) = rep.retry_spec() else {
        panic!("{:?}", rep.retry_spec())
    };
    assert_eq!(
        (retry.paths.clone(), retry.method),
        (vec![v(f.src.join("a"))], DeleteMethod::Trash)
    );
    retry.method = DeleteMethod::Permanent;
    assert_eq!(run(&JobSpec::Delete(retry)).outcome, Outcome::Completed);
    assert!(!f.src.join("a").exists());
}

/// A Unix file name may hold any byte but NUL and `/`, a line break included. Whatever the log
/// echoes of it stays on the record's own line.
#[cfg(unix)]
#[test]
fn a_file_name_with_control_characters_cannot_split_a_log_record() {
    let f = fx();
    let rep = run(&copy(
        &[f.src.join("a\nb\tc\rd\u{7f}\u{2028}\u{202e}x")],
        &f.dst,
    ));
    let mut out = Vec::new();
    rep.write_log(&mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    // The record terminators are the only control characters, one for each of the 5 records.
    assert_eq!(text.matches('\n').count(), 5, "{text:?}");
    assert!(
        text.chars().all(|c| !c.is_control() || c == '\n'),
        "{text:?}"
    );
    // Nor does the log hold a Unicode line separator or a bidirectional override.
    assert!(!text.contains(['\u{2028}', '\u{202e}']), "{text:?}");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 5, "{text:?}");
    let name = r"a\nb\tc\rd\u{7f}\u{2028}\u{202e}x";
    let failure = format!(
        "failure stage=plan kind=NotFound source={}/{name} target={}/{name} message=",
        v(&f.src),
        v(&f.dst)
    );
    assert!(lines[4].starts_with(&failure), "{}", lines[4]);
}
