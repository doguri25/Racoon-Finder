mod common;

use common::*;
use rf_ledger::{FakeOp, Ledger, LedgerError};

#[test]
fn portable_mode_never_calls_the_backend() {
    let fx = portable();
    let (mut l, report) = Ledger::open(&fx.paths, Box::new(fx.fake.clone())).unwrap();
    assert_eq!(report.confirmed, []);
    assert_eq!(report.rolled_back, []);
    assert_eq!(report.failed, []);
    assert!(matches!(
        l.apply("g", set(&slot(1), "x")),
        Err(LedgerError::PortableMode)
    ));
    assert_eq!(l.revert_all().reverted, []);
    assert_eq!(l.revert_group("g").reverted, []);
    assert_eq!(fx.fake.calls(), []);
    assert!(!fx.paths.data_dir().exists());
}

#[test]
fn portable_mode_ignores_a_copied_installed_ledger() {
    let installed_fx = installed();
    installed_fx
        .open()
        .apply("g", set(&slot(1), "ours"))
        .unwrap();
    let fx = portable();
    let copied = Ledger::file_path(&fx.paths);
    std::fs::create_dir_all(copied.parent().unwrap()).unwrap();
    std::fs::copy(Ledger::file_path(&installed_fx.paths), &copied).unwrap();
    let text = std::fs::read(&copied).unwrap();
    assert_eq!(Ledger::inspect(&fx.paths).unwrap(), []);
    let (mut l, _) = Ledger::open(&fx.paths, Box::new(fx.fake.clone())).unwrap();
    assert_eq!(l.entries(), []);
    assert_eq!(l.revert_all().reverted, []);
    assert_eq!(fx.fake.calls(), []);
    assert_eq!(std::fs::read(&copied).unwrap(), text);
}

/// A crash-pending entry in a copied ledger must not be settled (confirmed or rolled back) by a
/// portable run: that would write to the installed setup's registry or preferences.
#[test]
fn portable_mode_does_not_reconcile_or_revert_a_copied_pending_entry() {
    let installed_fx = installed();
    installed_fx.fake.set(&slot(1), s("orig"));
    let mut l = installed_fx.open();
    installed_fx.fake.fail_next(FakeOp::Apply, &slot(1));
    installed_fx.fake.fail_next(FakeOp::Restore, &slot(1));
    assert!(l.apply("g", set(&slot(1), "ours")).is_err());
    drop(l);

    let fx = portable();
    fx.fake.set(&slot(1), s("orig"));
    let copied = Ledger::file_path(&fx.paths);
    std::fs::create_dir_all(copied.parent().unwrap()).unwrap();
    std::fs::copy(Ledger::file_path(&installed_fx.paths), &copied).unwrap();
    let text = std::fs::read(&copied).unwrap();

    let (mut l, report) = Ledger::open(&fx.paths, Box::new(fx.fake.clone())).unwrap();
    assert_eq!(report.confirmed, []);
    assert_eq!(report.rolled_back, []);
    assert_eq!(report.failed, []);
    assert!(matches!(
        l.apply("g", set(&slot(1), "x")),
        Err(LedgerError::PortableMode)
    ));
    assert_eq!(l.revert_group("g").reverted, []);
    assert_eq!(l.revert_all().reverted, []);
    assert_eq!(fx.fake.calls(), []);
    assert_eq!(fx.fake.get(&slot(1)), s("orig"));
    assert_eq!(std::fs::read(&copied).unwrap(), text);
}

/// The ledger file is not even read in portable mode, so a file that would be refused when
/// installed (cut off, or from a newer build) is no obstacle and stays byte-identical.
#[test]
fn portable_mode_does_not_read_an_unreadable_ledger() {
    let fx = portable();
    let file = Ledger::file_path(&fx.paths);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let cut_off: &[u8] = br#"{"format":1,"next_id":2,"entries":["#;
    let newer: &[u8] = br#"{"format":99,"next_id":1,"entries":[]}"#;
    for text in [cut_off, newer] {
        std::fs::write(&file, text).unwrap();
        assert_eq!(Ledger::inspect(&fx.paths).unwrap(), []);
        let (mut l, report) = Ledger::open(&fx.paths, Box::new(fx.fake.clone())).unwrap();
        assert_eq!(report.failed, []);
        assert_eq!(l.revert_all().reverted, []);
        assert_eq!(fx.fake.calls(), []);
        assert_eq!(std::fs::read(&file).unwrap(), text);
    }
}
