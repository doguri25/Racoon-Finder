mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};

use common::*;
use rf_ledger::{ApplyOutcome, Change, EntryId, EntryState, FakeOp, Ledger, LedgerError, Target};

#[test]
fn apply_captures_before_state_then_changes_the_slot() {
    fn assert_send<T: Send>() {}
    assert_send::<Ledger>();
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("explorer"));
    let mut l = fx.open();
    assert_eq!(
        l.apply("winx.test", set(&t, "ours")).unwrap(),
        ApplyOutcome::Recorded(EntryId(1))
    );
    assert_eq!(fx.fake.get(&t), s("ours"));
    assert_eq!(
        fx.fake.calls(),
        [FakeOp::IsApplied, FakeOp::Capture, FakeOp::Apply]
    );
    let e = &l.entries()[0];
    assert_eq!(
        (e.id, e.group.as_str(), e.state),
        (EntryId(1), "winx.test", EntryState::Applied)
    );
    assert_eq!(
        (e.before.value.clone(), e.change.clone()),
        (s("explorer"), set(&t, "ours"))
    );
}

#[test]
fn crash_during_apply_leaves_a_pending_record() {
    let fx = installed();
    let t = slot(1);
    let mut l = fx.open();
    fx.fake.panic_after_next(FakeOp::Apply, &t);
    assert!(catch_unwind(AssertUnwindSafe(|| l.apply("g", set(&t, "ours")))).is_err());
    let on_disk = Ledger::inspect(&fx.paths).unwrap();
    assert_eq!(on_disk.len(), 1);
    assert_eq!(
        (on_disk[0].state, on_disk[0].before.value.clone()),
        (EntryState::Pending, None)
    );
    assert_eq!(fx.fake.get(&t), s("ours"));
}

#[test]
fn reapplying_the_live_value_records_nothing() {
    let fx = installed();
    let t = slot(1);
    let mut l = fx.open();
    l.apply("g", set(&t, "ours")).unwrap();
    let before = fx.fake.calls().len();
    assert_eq!(
        l.apply("g", set(&t, "ours")).unwrap(),
        ApplyOutcome::Unchanged
    );
    assert_eq!(fx.fake.calls()[before..], [FakeOp::IsApplied]);
    assert_eq!(l.entries().len(), 1);
}

#[test]
fn a_second_change_to_a_slot_records_the_previous_value() {
    let fx = installed();
    let t = slot(1);
    let mut l = fx.open();
    l.apply("g", set(&t, "a")).unwrap();
    assert_eq!(
        l.apply("g", set(&t, "b")).unwrap(),
        ApplyOutcome::Recorded(EntryId(2))
    );
    assert_eq!(l.entries()[1].before.value, s("a"));
}

#[test]
fn another_group_cannot_take_an_owned_slot() {
    let fx = installed();
    let mut l = fx.open();
    l.apply("winx.a", set(&slot(1), "a")).unwrap();
    let same = Target::Registry {
        key: r"hkcu\software\t1".into(),
        name: "V".into(),
    };
    let err = l.apply("winx.b", set(&same, "b")).unwrap_err();
    assert!(
        matches!(err, LedgerError::SlotOwned { ref owner, .. } if owner == "winx.a"),
        "{err:?}"
    );
    assert_eq!(fx.fake.get(&slot(1)), s("a"));
}

#[test]
fn the_hive_spelling_does_not_make_a_second_slot() {
    let fx = installed();
    let mut l = fx.open();
    let long = |n: u32| Target::Registry {
        key: format!(r"HKEY_CURRENT_USER\Software\T{n}"),
        name: "v".into(),
    };
    // Owned as `HKCU\...`, requested as `HKEY_CURRENT_USER\...`.
    l.apply("winx.a", set(&slot(1), "a")).unwrap();
    let err = l.apply("winx.b", set(&long(1), "b")).unwrap_err();
    assert!(
        matches!(err, LedgerError::SlotOwned { ref owner, .. } if owner == "winx.a"),
        "{err:?}"
    );
    // And the other way round.
    l.apply("winx.a", set(&long(2), "a")).unwrap();
    let err = l.apply("winx.b", set(&slot(2), "b")).unwrap_err();
    assert!(
        matches!(err, LedgerError::SlotOwned { ref owner, .. } if owner == "winx.a"),
        "{err:?}"
    );
    assert_eq!(fx.fake.get(&slot(1)), s("a"));
    assert_eq!(fx.fake.get(&slot(2)), None);
}

#[test]
fn failed_apply_is_rolled_back_and_not_recorded() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    fx.fake.fail_next(FakeOp::Apply, &t);
    assert!(matches!(
        l.apply("g", set(&t, "ours")),
        Err(LedgerError::Backend(_))
    ));
    assert_eq!(l.entries(), []);
    assert_eq!(Ledger::inspect(&fx.paths).unwrap(), []);
    assert_eq!(fx.fake.get(&t), s("orig"));
}

#[test]
fn failed_apply_with_failed_rollback_stays_pending() {
    let fx = installed();
    let t = slot(1);
    let mut l = fx.open();
    fx.fake.fail_next(FakeOp::Apply, &t);
    fx.fake.fail_next(FakeOp::Restore, &t);
    assert!(l.apply("g", set(&t, "ours")).is_err());
    assert_eq!(l.entries()[0].state, EntryState::Pending);
    assert_eq!(
        Ledger::inspect(&fx.paths).unwrap()[0].state,
        EntryState::Pending
    );
}

#[test]
fn capture_failure_writes_nothing() {
    let fx = installed();
    let t = slot(1);
    let mut l = fx.open();
    fx.fake.fail_next(FakeOp::Capture, &t);
    assert!(l.apply("g", set(&t, "ours")).is_err());
    assert!(!fx.paths.data_dir().exists());
}

#[test]
fn invalid_changes_and_groups_never_reach_the_backend() {
    let fx = installed();
    let mut l = fx.open();
    let hklm = Change {
        target: Target::Registry {
            key: r"HKLM\Software\X".into(),
            name: String::new(),
        },
        value: None,
    };
    assert!(matches!(
        l.apply("g", hklm),
        Err(LedgerError::OutsideHkcu(_))
    ));
    for group in ["", "WinX", "a b"] {
        assert!(
            matches!(
                l.apply(group, set(&slot(1), "x")),
                Err(LedgerError::InvalidGroup(_))
            ),
            "{group:?}"
        );
    }
    assert_eq!(fx.fake.calls(), []);
}

#[test]
fn ledger_writes_only_its_own_file() {
    let fx = installed();
    let mut l = fx.open();
    assert!(!fx.paths.data_dir().exists());
    l.apply("g", set(&slot(1), "x")).unwrap();
    assert_eq!(
        Ledger::file_path(&fx.paths),
        fx.paths.data_dir().join("ledger.json")
    );
    assert_eq!(files_under(fx.root()), [Ledger::file_path(&fx.paths)]);
}

#[test]
fn entries_survive_reopening() {
    let fx = installed();
    let mut l = fx.open();
    l.apply("g", set(&slot(1), "a")).unwrap();
    l.apply("g", set(&slot(2), "b")).unwrap();
    let saved = l.entries().to_vec();
    drop(l);
    assert_eq!(Ledger::inspect(&fx.paths).unwrap(), saved);
    let mut again = fx.open();
    assert_eq!(again.entries(), saved.as_slice());
    assert_eq!(
        again.apply("g", set(&slot(3), "c")).unwrap(),
        ApplyOutcome::Recorded(EntryId(3))
    );
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
#[test]
fn other_os_backend_refuses_every_change() {
    let fx = installed();
    let (mut l, _) = Ledger::open(&fx.paths, rf_ledger::os_backend()).unwrap();
    assert!(matches!(
        l.apply("g", set(&slot(1), "x")),
        Err(LedgerError::Unsupported(_))
    ));
    assert!(!fx.paths.data_dir().exists());
}
