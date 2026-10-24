#![allow(
    clippy::many_single_char_names,
    reason = "short fixture names in tests"
)]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rf_core::{Entry, EntryKind, VPath};
use rf_jobs::testkit::{ResolverCall, ScriptedResolver};
use rf_jobs::{
    Conflict, ConflictAction, ConflictAnswer, ConflictPolicy, Decision, ErrorAction, ErrorAnswer,
    ItemFailure, JobId, MTIME_TOLERANCE, NonInteractive, PreflightAnswer, Resolver, Stage, decide,
};

fn file(size: u64, secs: u64) -> Entry {
    let mut e = Entry::new("a", EntryKind::File);
    e.size = size;
    e.modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
    e
}

fn dir() -> Entry {
    Entry::new("a", EntryKind::Dir)
}

#[test]
fn fixed_policies() {
    let (a, b) = (file(1, 10), file(2, 20));
    assert_eq!(decide(ConflictPolicy::Ask, &a, &b), Decision::Ask);
    assert_eq!(
        decide(ConflictPolicy::Overwrite, &a, &b),
        Decision::Overwrite
    );
    assert_eq!(decide(ConflictPolicy::Skip, &a, &b), Decision::Skip);
    assert_eq!(decide(ConflictPolicy::AutoRename, &a, &b), Decision::Rename);
}

#[test]
fn newer_uses_a_two_second_tolerance() {
    assert_eq!(MTIME_TOLERANCE, Duration::from_secs(2));
    let existing = file(5, 1_000);
    assert_eq!(
        decide(ConflictPolicy::OverwriteIfNewer, &file(5, 1_002), &existing),
        Decision::Skip
    );
    assert_eq!(
        decide(ConflictPolicy::OverwriteIfNewer, &file(5, 1_003), &existing),
        Decision::Overwrite
    );
    assert_eq!(
        decide(ConflictPolicy::OverwriteIfNewer, &file(5, 900), &existing),
        Decision::Skip
    );
    assert_eq!(
        decide(
            ConflictPolicy::OverwriteIfNewer,
            &Entry::new("a", EntryKind::File),
            &existing
        ),
        Decision::Skip
    );
}

/// A time within a second of the latest one this platform can represent.
fn latest_time() -> SystemTime {
    let mut t = SystemTime::UNIX_EPOCH;
    for shift in (0..=62).rev() {
        if let Some(next) = t.checked_add(Duration::from_secs(1 << shift)) {
            t = next;
        }
    }
    t
}

#[test]
fn newer_skips_when_a_time_is_missing_or_cannot_be_compared() {
    let timeless = Entry::new("a", EntryKind::File);
    assert_eq!(
        decide(ConflictPolicy::OverwriteIfNewer, &file(5, 1_000), &timeless),
        Decision::Skip
    );
    assert_eq!(
        decide(ConflictPolicy::OverwriteIfNewer, &timeless, &timeless),
        Decision::Skip
    );

    // `existing + MTIME_TOLERANCE` overflows `SystemTime`: no verdict, so no overwrite, though
    // the incoming file is the later of the two.
    let last = latest_time();
    let before_last = last.checked_sub(Duration::from_secs(1)).unwrap();
    assert_eq!(before_last.checked_add(MTIME_TOLERANCE), None);
    let (mut incoming, mut existing) = (file(5, 0), file(5, 0));
    incoming.modified = Some(last);
    existing.modified = Some(before_last);
    assert_eq!(
        decide(ConflictPolicy::OverwriteIfNewer, &incoming, &existing),
        Decision::Skip
    );
}

#[test]
fn size_policy_compares_sizes() {
    assert_eq!(
        decide(
            ConflictPolicy::OverwriteIfSizeDiffers,
            &file(5, 1),
            &file(5, 99)
        ),
        Decision::Skip
    );
    assert_eq!(
        decide(
            ConflictPolicy::OverwriteIfSizeDiffers,
            &file(6, 1),
            &file(5, 1)
        ),
        Decision::Overwrite
    );
}

#[test]
fn file_and_folder_never_replace_each_other() {
    for policy in [
        ConflictPolicy::Overwrite,
        ConflictPolicy::OverwriteIfNewer,
        ConflictPolicy::OverwriteIfSizeDiffers,
    ] {
        assert_eq!(
            decide(policy, &file(1, 9_999), &dir()),
            Decision::Refuse,
            "{policy:?}"
        );
        assert_eq!(
            decide(policy, &dir(), &file(1, 0)),
            Decision::Refuse,
            "{policy:?}"
        );
    }
    assert_eq!(
        decide(ConflictPolicy::Skip, &dir(), &file(1, 0)),
        Decision::Skip
    );
    assert_eq!(
        decide(ConflictPolicy::AutoRename, &dir(), &file(1, 0)),
        Decision::Rename
    );
    assert_eq!(
        decide(ConflictPolicy::Ask, &dir(), &file(1, 0)),
        Decision::Ask
    );
}

#[test]
fn two_folders_are_never_overwritten_either() {
    // Callers merge two folders and never ask `decide`; if one does anyway, the overwrite
    // policies fail closed instead of replacing a folder.
    for policy in [
        ConflictPolicy::Overwrite,
        ConflictPolicy::OverwriteIfNewer,
        ConflictPolicy::OverwriteIfSizeDiffers,
    ] {
        assert_eq!(
            decide(policy, &dir(), &dir()),
            Decision::Refuse,
            "{policy:?}"
        );
    }
}

fn conflict() -> Conflict {
    let p = VPath::local(std::env::temp_dir()).unwrap();
    Conflict {
        source: p.clone(),
        target: p,
        incoming: file(1, 1),
        existing: file(2, 2),
    }
}

fn failure() -> ItemFailure {
    ItemFailure::new(Stage::Write, None, None, &rf_core::Error::Cancelled)
}

#[test]
fn non_interactive_is_safe() {
    let r = NonInteractive;
    assert_eq!(
        r.conflict(JobId(1), &conflict()),
        ConflictAnswer::once(ConflictAction::Skip)
    );
    assert_eq!(
        r.error(JobId(1), &failure()),
        ErrorAnswer {
            action: ErrorAction::Skip,
            apply_to_all: false
        }
    );
    assert_eq!(r.preflight(JobId(1), &[]), PreflightAnswer::SkipBlocked);
}

#[test]
fn scripted_resolver_answers_in_order_and_records_calls() {
    let r = ScriptedResolver::new()
        .on_conflict(ConflictAnswer::all(ConflictAction::Overwrite))
        .on_conflict(ConflictAnswer::once(ConflictAction::Rename))
        .on_error(ErrorAnswer {
            action: ErrorAction::Retry,
            apply_to_all: false,
        })
        .on_preflight(PreflightAnswer::Cancel);
    assert_eq!(
        r.conflict(JobId(1), &conflict()).action,
        ConflictAction::Overwrite
    );
    assert_eq!(
        r.conflict(JobId(1), &conflict()).action,
        ConflictAction::Rename
    );
    assert_eq!(r.error(JobId(1), &failure()).action, ErrorAction::Retry);
    assert_eq!(r.preflight(JobId(1), &[]), PreflightAnswer::Cancel);
    let target = conflict().target;
    assert_eq!(
        r.calls(),
        [
            ResolverCall::Conflict {
                target: target.clone()
            },
            ResolverCall::Conflict { target },
            ResolverCall::Error {
                stage: Stage::Write
            },
            ResolverCall::Preflight { issues: 0 },
        ]
    );
}

#[test]
#[should_panic(expected = "unscripted conflict")]
fn scripted_resolver_panics_when_out_of_answers() {
    ScriptedResolver::new().conflict(JobId(1), &conflict());
}

#[test]
fn scripted_resolver_survives_an_unscripted_panic() {
    // A panicking call must not break the resolver: the next call still answers and the log is intact.
    let r = Arc::new(ScriptedResolver::new().on_error(ErrorAnswer {
        action: ErrorAction::Abort,
        apply_to_all: true,
    }));
    let worker = Arc::clone(&r);
    let panicked = std::thread::spawn(move || worker.conflict(JobId(1), &conflict())).join();
    assert!(panicked.is_err());
    assert_eq!(r.error(JobId(1), &failure()).action, ErrorAction::Abort);
    assert_eq!(
        r.calls(),
        [
            ResolverCall::Conflict {
                target: conflict().target
            },
            ResolverCall::Error {
                stage: Stage::Write
            },
        ]
    );
}

#[test]
fn resolvers_are_shareable_trait_objects() {
    // Compile-time: both resolvers, and the trait object itself, can cross threads.
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<dyn Resolver>();
    assert_send_sync::<NonInteractive>();
    assert_send_sync::<ScriptedResolver>();

    let by_ref: &dyn Resolver = &NonInteractive;
    assert_eq!(
        by_ref.preflight(JobId(3), &[]),
        PreflightAnswer::SkipBlocked
    );

    let resolvers: [Arc<dyn Resolver>; 2] = [
        Arc::new(NonInteractive),
        Arc::new(ScriptedResolver::new().on_preflight(PreflightAnswer::Proceed)),
    ];
    let answers: Vec<_> = resolvers
        .into_iter()
        .map(|resolver| std::thread::spawn(move || resolver.preflight(JobId(2), &[])))
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(
        answers,
        [PreflightAnswer::SkipBlocked, PreflightAnswer::Proceed]
    );
}
