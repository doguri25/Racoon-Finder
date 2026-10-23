mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};

use common::*;
use rf_ledger::{
    EntryId, EntryState, FakeOp, Ledger, LedgerEntry, LedgerError, ReconcileReport, Target,
};

fn reopen(fx: &Fx) -> (Ledger, ReconcileReport) {
    Ledger::open(&fx.paths, Box::new(fx.fake.clone())).unwrap()
}

/// Leaves entry 1 `Pending` on disk while the live slot still holds `orig`.
fn pending_not_applied(fx: &Fx) {
    fx.fake.set(&slot(1), s("orig"));
    let mut l = fx.open();
    fx.fake.fail_next(FakeOp::Apply, &slot(1));
    fx.fake.fail_next(FakeOp::Restore, &slot(1));
    assert!(l.apply("g", set(&slot(1), "ours")).is_err());
}

/// Makes `l` record `value` for `t` and leave it `Pending`: the apply and its rollback fail.
fn leave_pending(fx: &Fx, l: &mut Ledger, t: &Target, value: &str) {
    fx.fake.fail_next(FakeOp::Apply, t);
    fx.fake.fail_next(FakeOp::Restore, t);
    assert!(l.apply("g", set(t, value)).is_err());
}

/// `(id, state)` of every entry, oldest first.
fn states(entries: &[LedgerEntry]) -> Vec<(EntryId, EntryState)> {
    entries.iter().map(|e| (e.id, e.state)).collect()
}

#[test]
fn pending_entry_whose_change_is_live_is_confirmed() {
    let fx = installed();
    let mut l = fx.open();
    fx.fake.panic_after_next(FakeOp::Apply, &slot(1));
    assert!(catch_unwind(AssertUnwindSafe(|| l.apply("g", set(&slot(1), "ours")))).is_err());
    drop(l);
    let (l, report) = reopen(&fx);
    assert_eq!(report.confirmed, [EntryId(1)]);
    assert!(report.rolled_back.is_empty() && report.failed.is_empty());
    assert_eq!(l.entries()[0].state, EntryState::Applied);
    assert_eq!(
        Ledger::inspect(&fx.paths).unwrap()[0].state,
        EntryState::Applied
    );
}

#[test]
fn pending_entry_whose_change_is_not_live_is_rolled_back() {
    let fx = installed();
    pending_not_applied(&fx);
    let (l, report) = reopen(&fx);
    assert_eq!(report.rolled_back, [EntryId(1)]);
    assert!(l.entries().is_empty() && Ledger::inspect(&fx.paths).unwrap().is_empty());
    assert_eq!(fx.fake.get(&slot(1)), s("orig"));
}

#[test]
fn reconcile_failure_keeps_the_entry_pending() {
    let fx = installed();
    pending_not_applied(&fx);
    fx.fake.fail_next(FakeOp::Restore, &slot(1));
    let (l, report) = reopen(&fx);
    assert_eq!(report.failed[0].id, EntryId(1));
    assert_eq!(l.entries()[0].state, EntryState::Pending);
}

#[test]
fn corrupt_ledger_is_never_overwritten() {
    let fx = installed();
    let file = Ledger::file_path(&fx.paths);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let text = br#"{"format":1,"next_id":2,"entries":["#;
    std::fs::write(&file, text).unwrap();
    assert!(matches!(
        Ledger::open(&fx.paths, Box::new(fx.fake.clone())),
        Err(LedgerError::Corrupt { ref path, .. }) if *path == file
    ));
    assert!(matches!(
        Ledger::inspect(&fx.paths),
        Err(LedgerError::Corrupt { .. })
    ));
    assert_eq!(std::fs::read(&file).unwrap(), text);
    assert_eq!(fx.fake.calls(), []);
}

#[test]
fn newer_format_is_refused_untouched() {
    let fx = installed();
    let file = Ledger::file_path(&fx.paths);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let text = br#"{"format":2,"next_id":1,"entries":[]}"#;
    std::fs::write(&file, text).unwrap();
    let err = Ledger::open(&fx.paths, Box::new(fx.fake.clone())).unwrap_err();
    assert!(
        matches!(
            err,
            LedgerError::UnsupportedFormat {
                found: 2,
                supported: 1,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(Ledger::FORMAT, 1);
    assert_eq!(std::fs::read(&file).unwrap(), text);
    assert!(matches!(
        Ledger::inspect(&fx.paths),
        Err(LedgerError::UnsupportedFormat { .. })
    ));
    assert_eq!(fx.fake.calls(), []);
}

#[test]
fn leftover_temp_file_is_ignored_and_replaced() {
    let fx = installed();
    fx.open().apply("g", set(&slot(1), "a")).unwrap();
    std::fs::write(fx.paths.data_dir().join("ledger.json.tmp"), b"garbage").unwrap();
    let mut l = fx.open();
    assert_eq!(l.entries().len(), 1);
    l.apply("g", set(&slot(2), "b")).unwrap();
    assert_eq!(
        files_under(fx.paths.data_dir()),
        [Ledger::file_path(&fx.paths)]
    );
}

#[test]
fn a_reconcile_that_cannot_be_saved_fails_the_open_and_is_redone_next_time() {
    let fx = installed();
    pending_not_applied(&fx);
    let file = Ledger::file_path(&fx.paths);
    let text = std::fs::read(&file).unwrap();
    // A folder where the temporary file must go, so the ledger file cannot be saved.
    let tmp = fx.paths.data_dir().join("ledger.json.tmp");
    std::fs::create_dir(&tmp).unwrap();
    assert!(matches!(
        Ledger::open(&fx.paths, Box::new(fx.fake.clone())),
        Err(LedgerError::Core(_))
    ));
    assert_eq!(fx.fake.get(&slot(1)), s("orig"));
    assert_eq!(std::fs::read(&file).unwrap(), text);
    std::fs::remove_dir(&tmp).unwrap();
    let (l, report) = reopen(&fx);
    assert_eq!(report.rolled_back, [EntryId(1)]);
    assert_eq!(l.entries(), []);
    assert_eq!(fx.fake.get(&slot(1)), s("orig"));
}

/// Rewrites the ids of the entries in the ledger file, oldest first; returns the new content.
fn rewrite_ids(fx: &Fx, ids: [u64; 2]) -> Vec<u8> {
    let file = Ledger::file_path(&fx.paths);
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let entries = json["entries"].as_array_mut().unwrap();
    assert_eq!(entries.len(), ids.len());
    for (entry, id) in entries.iter_mut().zip(ids) {
        entry["id"] = id.into();
    }
    let text = serde_json::to_vec(&json).unwrap();
    std::fs::write(&file, &text).unwrap();
    text
}

#[test]
fn entry_ids_out_of_order_or_at_the_end_are_refused_untouched() {
    for ids in [[2, 1], [1, 1], [1, u64::MAX]] {
        let fx = installed();
        let mut l = fx.open();
        for n in 1..=2 {
            // Each entry stays `Pending`, so an open that accepted the file would settle it.
            leave_pending(&fx, &mut l, &slot(n), "x");
        }
        drop(l);
        let text = rewrite_ids(&fx, ids);
        let calls = fx.fake.calls().len();
        let file = Ledger::file_path(&fx.paths);
        let err = Ledger::open(&fx.paths, Box::new(fx.fake.clone())).unwrap_err();
        // Refused for its ids, not because the rewritten file failed to parse.
        assert!(
            matches!(
                &err,
                LedgerError::Corrupt { path, reason } if *path == file && reason.contains("entry id")
            ),
            "{ids:?}: {err:?}"
        );
        assert!(
            matches!(Ledger::inspect(&fx.paths), Err(LedgerError::Corrupt { .. })),
            "{ids:?}"
        );
        assert_eq!(std::fs::read(&file).unwrap(), text, "{ids:?}");
        assert_eq!(fx.fake.calls().len(), calls, "{ids:?}");
    }
}

#[test]
fn a_ledger_without_a_free_id_refuses_new_changes() {
    let fx = installed();
    let file = Ledger::file_path(&fx.paths);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let text = format!(r#"{{"format":1,"next_id":{},"entries":[]}}"#, u64::MAX);
    std::fs::write(&file, &text).unwrap();
    let mut l = fx.open();
    let err = l.apply("g", set(&slot(1), "x")).unwrap_err();
    assert!(matches!(err, LedgerError::Corrupt { .. }), "{err:?}");
    assert_eq!(fx.fake.calls(), []);
    assert_eq!(std::fs::read(&file).unwrap(), text.as_bytes());
}

#[test]
fn a_pending_entry_beneath_a_newer_one_of_its_setting_is_left_alone() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    leave_pending(&fx, &mut l, &t, "a");
    // The failed apply left the setting half-written: removed, to be written again with the
    // new type (what an interrupted macOS write leaves). Then a newer change succeeds.
    fx.fake.set(&t, None);
    l.apply("g", set(&t, "b")).unwrap();
    drop(l);
    let calls = fx.fake.calls().len();
    let (mut l, report) = reopen(&fx);
    // Entry 2's value is live, so entry 1's "is it applied?" means nothing: nothing is asked.
    assert_eq!(report, ReconcileReport::default());
    assert_eq!(fx.fake.calls().len(), calls);
    assert_eq!(fx.fake.get(&t), s("b"));
    let want = [
        (EntryId(1), EntryState::Pending),
        (EntryId(2), EntryState::Applied),
    ];
    assert_eq!(states(l.entries()), want);
    assert_eq!(states(&Ledger::inspect(&fx.paths).unwrap()), want);
    // Entry 1 still holds the value from before both changes.
    assert_eq!(l.revert_all().reverted, [EntryId(2), EntryId(1)]);
    assert_eq!(fx.fake.get(&t), s("orig"));
}

#[test]
fn a_pending_entry_beneath_a_confirmed_one_is_left_alone() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    leave_pending(&fx, &mut l, &t, "a");
    // Entry 2 changes the same setting and the process dies right after.
    fx.fake.panic_after_next(FakeOp::Apply, &t);
    assert!(catch_unwind(AssertUnwindSafe(|| l.apply("g", set(&t, "b")))).is_err());
    drop(l);
    let calls = fx.fake.calls().len();
    let (l, report) = reopen(&fx);
    assert_eq!(report.confirmed, [EntryId(2)]);
    assert!(report.rolled_back.is_empty() && report.failed.is_empty());
    // Entry 2 was checked; entry 1 was left as it is without asking the system anything.
    assert_eq!(fx.fake.calls()[calls..], [FakeOp::IsApplied]);
    assert_eq!(fx.fake.get(&t), s("b"));
    let want = [
        (EntryId(1), EntryState::Pending),
        (EntryId(2), EntryState::Applied),
    ];
    assert_eq!(states(l.entries()), want);
    assert_eq!(states(&Ledger::inspect(&fx.paths).unwrap()), want);
}

#[test]
fn a_pending_entry_waits_for_a_newer_one_that_could_not_be_settled() {
    // The check fails, or the check says "not live, the before-state is" and the rollback fails.
    for (fault, reached) in [
        (FakeOp::IsApplied, &[FakeOp::IsApplied][..]),
        (
            FakeOp::Restore,
            &[FakeOp::IsApplied, FakeOp::IsApplied, FakeOp::Restore][..],
        ),
    ] {
        let fx = installed();
        let t = slot(1);
        fx.fake.set(&t, s("orig"));
        let mut l = fx.open();
        leave_pending(&fx, &mut l, &t, "a");
        // Entry 1's failed apply left the setting half-written (removed, as an interrupted
        // macOS write leaves it), and entry 2's apply failed without an effect.
        fx.fake.set(&t, None);
        leave_pending(&fx, &mut l, &t, "b");
        drop(l);
        fx.fake.fail_next(fault, &t);
        let calls = fx.fake.calls().len();
        let (l, report) = reopen(&fx);
        let failed: Vec<EntryId> = report.failed.iter().map(|f| f.id).collect();
        assert_eq!(failed, [EntryId(2), EntryId(1)], "{fault:?}");
        assert!(report.confirmed.is_empty() && report.rolled_back.is_empty());
        // Only entry 2 reached the system.
        assert_eq!(fx.fake.calls()[calls..], *reached, "{fault:?}");
        assert_eq!(fx.fake.get(&t), None);
        assert_eq!(
            states(l.entries()),
            [
                (EntryId(1), EntryState::Pending),
                (EntryId(2), EntryState::Pending),
            ]
        );
        drop(l);
        // Once entry 2 is rolled back, entry 1 is the newest of its setting and is settled too.
        let (l, report) = reopen(&fx);
        assert_eq!(report.rolled_back, [EntryId(2), EntryId(1)]);
        assert_eq!(l.entries(), []);
        assert_eq!(fx.fake.get(&t), s("orig"));
    }
}

/// Checks that an open reported exactly `ids` as skipped and nothing else, and that no entry
/// is left in memory or in the ledger file.
fn assert_dropped_as_skipped(fx: &Fx, report: &ReconcileReport, l: &Ledger, ids: &[EntryId]) {
    assert_eq!(report.skipped, ids);
    assert!(report.confirmed.is_empty() && report.rolled_back.is_empty());
    assert_eq!(report.failed, []);
    assert_eq!(l.entries(), []);
    assert_eq!(Ledger::inspect(&fx.paths).unwrap(), []);
}

/// The apply failed and so did its rollback, so the entry stays `Pending`; then another app
/// takes the setting. The next open must not put the before-state over the other app's value.
#[test]
fn a_pending_entry_leaves_a_value_another_app_wrote_since() {
    let fx = installed();
    pending_not_applied(&fx);
    fx.fake.set(&slot(1), s("other-app"));
    let calls = fx.fake.calls().len();
    let (l, report) = reopen(&fx);
    assert_dropped_as_skipped(&fx, &report, &l, &[EntryId(1)]);
    assert_eq!(fx.fake.get(&slot(1)), s("other-app"));
    // Not ours, not the before-state, and not the empty setting a half-done write leaves.
    assert_eq!(
        fx.fake.calls()[calls..],
        [FakeOp::IsApplied, FakeOp::IsApplied, FakeOp::IsApplied]
    );
}

/// The apply succeeded but the process died before the last save, so the file says `Pending`;
/// then another app takes the setting. The next open leaves the other app's value alone.
#[test]
fn a_pending_entry_whose_last_save_was_lost_leaves_a_value_another_app_wrote_since() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    fx.fake.panic_after_next(FakeOp::Apply, &t);
    assert!(catch_unwind(AssertUnwindSafe(|| l.apply("g", set(&t, "ours")))).is_err());
    drop(l);
    assert_eq!(
        Ledger::inspect(&fx.paths).unwrap()[0].state,
        EntryState::Pending
    );
    fx.fake.set(&t, s("other-app"));
    let (l, report) = reopen(&fx);
    assert_dropped_as_skipped(&fx, &report, &l, &[EntryId(1)]);
    assert_eq!(fx.fake.get(&t), s("other-app"));
}

/// A setting that holds nothing is what a write that died half-way leaves (removed, to be
/// written again with the new type): the before-state is put back over it.
#[test]
fn a_pending_entry_puts_its_before_state_back_over_an_emptied_setting() {
    let fx = installed();
    pending_not_applied(&fx);
    fx.fake.set(&slot(1), None);
    let (l, report) = reopen(&fx);
    assert_eq!(report.rolled_back, [EntryId(1)]);
    assert!(report.skipped.is_empty() && report.failed.is_empty());
    assert_eq!(l.entries(), []);
    assert_eq!(fx.fake.get(&slot(1)), s("orig"));
}

/// Someone else's value beneath a `Pending` entry makes the setting theirs: the older entries
/// of the setting are dropped too, without asking the system, as a revert would drop them.
#[test]
fn someone_elses_value_drops_the_older_entries_of_its_setting_unasked() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    l.apply("g", set(&t, "a")).unwrap();
    leave_pending(&fx, &mut l, &t, "b");
    drop(l);
    fx.fake.set(&t, s("other-app"));
    let calls = fx.fake.calls().len();
    let (l, report) = reopen(&fx);
    assert_dropped_as_skipped(&fx, &report, &l, &[EntryId(2), EntryId(1)]);
    assert_eq!(fx.fake.get(&t), s("other-app"));
    // Only entry 2 was asked about.
    assert_eq!(
        fx.fake.calls()[calls..],
        [FakeOp::IsApplied, FakeOp::IsApplied, FakeOp::IsApplied]
    );
}
