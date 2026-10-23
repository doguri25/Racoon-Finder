#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

mod common;

use common::*;
use rf_jobs::testkit::tree;
use rf_jobs::{ConflictPolicy, JobSpec, Outcome, TransferMode, TransferOptions};

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
    let rep = run(&copy(&[f.src.join("a\nb\tc\rd\u{7f}")], &f.dst));
    let mut out = Vec::new();
    rep.write_log(&mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    // The record terminators are the only control characters, one for each of the 5 records.
    assert_eq!(text.matches('\n').count(), 5, "{text:?}");
    assert!(
        text.chars().all(|c| !c.is_control() || c == '\n'),
        "{text:?}"
    );
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 5, "{text:?}");
    let name = r"a\nb\tc\rd\u{7f}";
    let failure = format!(
        "failure stage=plan kind=NotFound source={}/{name} target={}/{name} message=",
        v(&f.src),
        v(&f.dst)
    );
    assert!(lines[4].starts_with(&failure), "{}", lines[4]);
}
