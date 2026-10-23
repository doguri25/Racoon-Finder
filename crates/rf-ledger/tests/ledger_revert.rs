mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};

use common::*;
use rf_ledger::{EntryId, FakeOp, Ledger};

#[test]
fn revert_all_restores_in_reverse_order() {
    let fx = installed();
    let (x, y) = (slot(1), slot(2));
    fx.fake.set(&x, s("orig"));
    let mut l = fx.open();
    l.apply("g", set(&x, "a")).unwrap();
    l.apply("g", set(&y, "b")).unwrap();
    l.apply("g", set(&x, "c")).unwrap();
    let r = l.revert_all();
    assert_eq!(r.reverted, [EntryId(3), EntryId(2), EntryId(1)]);
    assert!(r.skipped.is_empty() && r.is_clean());
    assert_eq!((fx.fake.get(&x), fx.fake.get(&y)), (s("orig"), None));
    assert_eq!(l.entries(), []);
    assert_eq!(Ledger::inspect(&fx.paths).unwrap(), []);
}

#[test]
fn revert_all_twice_is_harmless() {
    let fx = installed();
    let mut l = fx.open();
    l.apply("g", set(&slot(1), "a")).unwrap();
    l.revert_all();
    let calls = fx.fake.calls().len();
    let r = l.revert_all();
    assert!(r.reverted.is_empty() && r.skipped.is_empty() && r.failed.is_empty());
    assert_eq!(fx.fake.calls().len(), calls);
}

#[test]
fn revert_continues_past_a_failure_and_reports_it() {
    let fx = installed();
    let mut l = fx.open();
    for n in 1..=3 {
        l.apply("g", set(&slot(n), "x")).unwrap();
    }
    fx.fake.fail_next(FakeOp::Restore, &slot(2));
    let r = l.revert_all();
    assert_eq!(r.reverted, [EntryId(3), EntryId(1)]);
    assert_eq!(
        (r.failed.len(), r.failed[0].id, r.failed[0].change.as_str()),
        (1, EntryId(2), r"HKCU\Software\T2 @ v")
    );
    assert!(
        r.failed[0].error.contains("injected"),
        "{}",
        r.failed[0].error
    );
    assert!(!r.is_clean());
    assert_eq!(l.entries().len(), 1);
    assert_eq!(l.revert_all().reverted, [EntryId(2)]);
}

#[test]
fn revert_leaves_values_someone_else_changed_since() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("explorer"));
    let mut l = fx.open();
    l.apply("g", set(&t, "ours")).unwrap();
    fx.fake.set(&t, s("other-app"));
    let r = l.revert_all();
    assert_eq!((r.reverted.len(), r.skipped.clone()), (0, vec![EntryId(1)]));
    assert_eq!(fx.fake.get(&t), s("other-app"));
    assert_eq!(l.entries(), []);
}

#[test]
fn revert_group_touches_only_that_group() {
    let fx = installed();
    let mut l = fx.open();
    l.apply("winx.a", set(&slot(1), "a")).unwrap();
    l.apply("macx.b", set(&slot(2), "b")).unwrap();
    assert_eq!(l.revert_group("winx.a").reverted, [EntryId(1)]);
    assert_eq!(
        (fx.fake.get(&slot(1)), fx.fake.get(&slot(2))),
        (None, s("b"))
    );
    assert_eq!(l.entries()[0].group, "macx.b");
}

/// A `Pending` entry's apply may have died half-way: on macOS a write of another type removes
/// the value before it writes it again, so the setting may hold nothing. That is put back.
#[test]
fn a_pending_entry_is_restored_over_what_a_half_done_apply_left() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    fx.fake.fail_next(FakeOp::Apply, &t);
    fx.fake.fail_next(FakeOp::Restore, &t);
    assert!(l.apply("g", set(&t, "ours")).is_err());
    fx.fake.set(&t, None);
    assert_eq!(l.revert_all().reverted, [EntryId(1)]);
    assert_eq!(fx.fake.get(&t), s("orig"));
}

/// A `Pending` entry is put back only over the app's value, its own before-state, or the
/// nothing a half-done write leaves: another app's value is left alone.
#[test]
fn a_pending_entry_is_not_restored_over_another_apps_value() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    fx.fake.fail_next(FakeOp::Apply, &t);
    fx.fake.fail_next(FakeOp::Restore, &t);
    assert!(l.apply("g", set(&t, "ours")).is_err());
    fx.fake.set(&t, s("other-app"));
    let calls = fx.fake.calls().len();
    let r = l.revert_all();
    assert_eq!(
        (r.reverted, r.skipped, r.failed),
        (vec![], vec![EntryId(1)], vec![])
    );
    assert_eq!(fx.fake.get(&t), s("other-app"));
    assert_eq!(
        fx.fake.calls()[calls..],
        [FakeOp::IsApplied, FakeOp::IsApplied, FakeOp::IsApplied]
    );
    assert_eq!(Ledger::inspect(&fx.paths).unwrap(), []);
}

#[test]
fn a_failed_ownership_check_keeps_the_entry_and_restores_nothing() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    l.apply("g", set(&t, "ours")).unwrap();
    fx.fake.fail_next(FakeOp::IsApplied, &t);
    let r = l.revert_all();
    assert_eq!(
        (r.reverted.len(), r.skipped.len(), r.failed.len()),
        (0, 0, 1)
    );
    assert_eq!(r.failed[0].id, EntryId(1));
    assert_eq!(fx.fake.get(&t), s("ours"));
    assert_eq!(l.entries().len(), 1);
    assert_eq!(l.revert_all().reverted, [EntryId(1)]);
    assert_eq!(fx.fake.get(&t), s("orig"));
}

#[test]
fn skipped_entries_are_reported_in_reverse_order_and_dropped() {
    let fx = installed();
    let t = slot(1);
    let mut l = fx.open();
    l.apply("g", set(&t, "a")).unwrap();
    l.apply("g", set(&t, "b")).unwrap();
    fx.fake.set(&t, s("other-app"));
    let r = l.revert_all();
    assert_eq!(r.skipped, [EntryId(2), EntryId(1)]);
    assert!(r.reverted.is_empty() && r.is_clean());
    assert_eq!(fx.fake.get(&t), s("other-app"));
    assert_eq!(Ledger::inspect(&fx.paths).unwrap(), []);
}

#[test]
fn a_pending_entry_beneath_a_skipped_one_leaves_the_other_apps_value_alone() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    // Entry 1 stays `Pending`: its apply failed and so did the rollback.
    fx.fake.fail_next(FakeOp::Apply, &t);
    fx.fake.fail_next(FakeOp::Restore, &t);
    assert!(l.apply("g", set(&t, "a")).is_err());
    // Entry 2 is on the same slot and succeeds; then another app takes the setting.
    l.apply("g", set(&t, "b")).unwrap();
    fx.fake.set(&t, s("other-app"));
    let calls = fx.fake.calls().len();
    let r = l.revert_group("g");
    assert_eq!(r.skipped, [EntryId(2), EntryId(1)]);
    assert!(r.reverted.is_empty() && r.is_clean());
    assert_eq!(fx.fake.get(&t), s("other-app"));
    // Entry 2 holds neither its value ("b") nor its before-state ("orig"): two questions.
    // Entry 1 was dropped without asking the system anything.
    assert_eq!(
        fx.fake.calls()[calls..],
        [FakeOp::IsApplied, FakeOp::IsApplied]
    );
    assert_eq!(l.entries(), []);
    assert_eq!(Ledger::inspect(&fx.paths).unwrap(), []);
}

#[test]
fn a_failed_entry_keeps_the_older_entries_of_its_slot() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    l.apply("g", set(&t, "a")).unwrap();
    l.apply("g", set(&t, "b")).unwrap();
    fx.fake.fail_next(FakeOp::Restore, &t);
    let calls = fx.fake.calls().len();
    let r = l.revert_all();
    // The live value is still ours ("b"), so entry 1 must not be mistaken for "taken over".
    assert_eq!((r.reverted.len(), r.skipped.len()), (0, 0));
    let failed: Vec<EntryId> = r.failed.iter().map(|f| f.id).collect();
    assert_eq!(failed, [EntryId(2), EntryId(1)]);
    assert!(!r.is_clean());
    // Only entry 2 reached the system (checked, then the failing restore); entry 1 was held back.
    assert_eq!(
        fx.fake.calls()[calls..],
        [FakeOp::IsApplied, FakeOp::Restore]
    );
    assert_eq!(fx.fake.get(&t), s("b"));
    assert_eq!(Ledger::inspect(&fx.paths).unwrap().len(), 2);
    let again = l.revert_all();
    assert_eq!(again.reverted, [EntryId(2), EntryId(1)]);
    assert_eq!(fx.fake.get(&t), s("orig"));
}

#[test]
fn every_settled_entry_is_saved_before_the_next_one_is_touched() {
    let fx = installed();
    let mut l = fx.open();
    for n in 1..=3 {
        l.apply("g", set(&slot(n), "x")).unwrap();
    }
    // The process dies right after entry 2 is restored, before its record is dropped.
    fx.fake.panic_after_next(FakeOp::Restore, &slot(2));
    assert!(catch_unwind(AssertUnwindSafe(|| l.revert_all())).is_err());
    let on_disk: Vec<EntryId> = Ledger::inspect(&fx.paths)
        .unwrap()
        .iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(on_disk, [EntryId(1), EntryId(2)]);

    // Slot 2 holds entry 2's before-state: its restore already happened and is done again.
    let r = fx.open().revert_all();
    assert_eq!(
        (r.reverted, r.skipped),
        (vec![EntryId(2), EntryId(1)], vec![])
    );
    assert_eq!(Ledger::inspect(&fx.paths).unwrap(), []);
    assert_eq!(
        (
            fx.fake.get(&slot(1)),
            fx.fake.get(&slot(2)),
            fx.fake.get(&slot(3))
        ),
        (None, None, None)
    );
}

#[test]
fn an_unsaved_removal_keeps_the_entry_and_says_so() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    l.apply("g", set(&t, "ours")).unwrap();
    // A folder where the temporary file must go, so the ledger file cannot be saved.
    std::fs::create_dir(fx.paths.data_dir().join("ledger.json.tmp")).unwrap();
    let r = l.revert_all();
    assert_eq!(fx.fake.get(&t), s("orig"));
    assert!(r.reverted.is_empty() && r.skipped.is_empty());
    assert_eq!((r.failed.len(), r.failed[0].id), (1, EntryId(1)));
    assert!(
        r.failed[0]
            .error
            .starts_with("restored, but the ledger file could not be updated: "),
        "{}",
        r.failed[0].error
    );
    assert!(!r.is_clean());
    assert_eq!(l.entries().len(), 1);
    assert_eq!(Ledger::inspect(&fx.paths).unwrap().len(), 1);
}

#[test]
fn an_unsaved_skip_keeps_the_entry_and_says_so() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    l.apply("g", set(&t, "ours")).unwrap();
    fx.fake.set(&t, s("other-app"));
    // A folder where the temporary file must go, so the ledger file cannot be saved.
    std::fs::create_dir(fx.paths.data_dir().join("ledger.json.tmp")).unwrap();
    let r = l.revert_all();
    assert_eq!(fx.fake.get(&t), s("other-app"));
    assert!(r.reverted.is_empty() && r.skipped.is_empty());
    assert_eq!((r.failed.len(), r.failed[0].id), (1, EntryId(1)));
    assert!(
        r.failed[0]
            .error
            .starts_with("left alone, but the ledger file could not be updated: "),
        "{}",
        r.failed[0].error
    );
    assert!(!r.is_clean());
    assert_eq!(l.entries().len(), 1);
    assert_eq!(Ledger::inspect(&fx.paths).unwrap().len(), 1);
}

/// Two entries on one setting: `orig` → `a` (entry 1) → `b` (entry 2). The process dies right
/// after entry 2's restore put `a` back, before the ledger file said so. On the next run the
/// setting holds entry 2's before-state, not someone else's value: entry 2's restore already
/// happened and is done again, and entry 1 then puts `orig` back.
#[test]
fn a_crash_right_after_a_restore_still_reverts_the_setting_to_its_original_value() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    l.apply("g", set(&t, "a")).unwrap();
    l.apply("g", set(&t, "b")).unwrap();
    fx.fake.panic_after_next(FakeOp::Restore, &t);
    assert!(catch_unwind(AssertUnwindSafe(|| l.revert_all())).is_err());
    assert_eq!(fx.fake.get(&t), s("a"));
    assert_eq!(Ledger::inspect(&fx.paths).unwrap().len(), 2);

    let calls = fx.fake.calls().len();
    let r = fx.open().revert_all();
    assert_eq!(
        (r.reverted, r.skipped, r.failed),
        (vec![EntryId(2), EntryId(1)], vec![], vec![])
    );
    assert_eq!(fx.fake.get(&t), s("orig"));
    assert_eq!(Ledger::inspect(&fx.paths).unwrap(), []);
    // Entry 2: not its value but its before-state, so restored again. Entry 1: its value.
    assert_eq!(
        fx.fake.calls()[calls..],
        [
            FakeOp::IsApplied,
            FakeOp::IsApplied,
            FakeOp::Restore,
            FakeOp::IsApplied,
            FakeOp::Restore
        ]
    );
}

/// The same two entries, but a power loss dropped entry 2's write while the ledger file already
/// says it is applied: the setting holds entry 2's before-state, and the revert still ends with
/// `orig`.
#[test]
fn a_lost_write_of_the_newest_entry_still_reverts_the_setting_to_its_original_value() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    l.apply("g", set(&t, "a")).unwrap();
    l.apply("g", set(&t, "b")).unwrap();
    fx.fake.set(&t, s("a"));
    let r = l.revert_all();
    assert_eq!(
        (r.reverted, r.skipped, r.failed),
        (vec![EntryId(2), EntryId(1)], vec![], vec![])
    );
    assert_eq!(fx.fake.get(&t), s("orig"));
    assert_eq!(Ledger::inspect(&fx.paths).unwrap(), []);
}

/// An unsaved skip still marks the setting as someone else's: the older entry beneath it gets
/// no backend call, so it can never put its before-state over the other value.
#[test]
fn an_unsaved_skip_still_marks_the_setting_foreign() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    // Entry 1 stays `Pending`: its apply failed and so did the rollback.
    fx.fake.fail_next(FakeOp::Apply, &t);
    fx.fake.fail_next(FakeOp::Restore, &t);
    assert!(l.apply("g", set(&t, "a")).is_err());
    l.apply("g", set(&t, "b")).unwrap();
    // A third value: neither the app's ("b") nor entry 2's before-state ("orig").
    fx.fake.set(&t, s("other-app"));
    // A folder where the temporary file must go, so the ledger file cannot be saved.
    std::fs::create_dir(fx.paths.data_dir().join("ledger.json.tmp")).unwrap();
    let calls = fx.fake.calls().len();
    let r = l.revert_all();
    let failed: Vec<EntryId> = r.failed.iter().map(|f| f.id).collect();
    assert_eq!(failed, [EntryId(2), EntryId(1)]);
    for f in &r.failed {
        assert!(
            f.error
                .starts_with("left alone, but the ledger file could not be updated: "),
            "{}",
            f.error
        );
    }
    assert!(r.reverted.is_empty() && r.skipped.is_empty());
    assert_eq!(fx.fake.get(&t), s("other-app"));
    // Entry 2 was asked about its value and its before-state; entry 1 was not asked at all.
    assert_eq!(
        fx.fake.calls()[calls..],
        [FakeOp::IsApplied, FakeOp::IsApplied]
    );
    assert_eq!(l.entries().len(), 2);
    assert_eq!(Ledger::inspect(&fx.paths).unwrap().len(), 2);
}
