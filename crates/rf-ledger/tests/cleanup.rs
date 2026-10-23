mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use common::*;
use rf_core::identity::{BUNDLE_ID, PORTABLE_MARKER, TEMP_PREFIX};
use rf_ledger::{CleanupAction, EntryId, FakeOp, Ledger, LedgerError, plan_cleanup, run_cleanup};

/// Two applied changes plus files in every owned folder and the OS temp folder.
/// Returns the paths a cleanup must delete, in plan order.
fn used(fx: &Fx) -> Vec<PathBuf> {
    fx.fake.set(&slot(1), s("orig"));
    let mut l = fx.open();
    l.apply("g", set(&slot(1), "a")).unwrap();
    l.apply("g", set(&slot(2), "b")).unwrap();
    for (dir, name) in [
        (fx.paths.config_dir(), "settings.toml"),
        (fx.paths.cache_dir(), "thumb.bin"),
        (fx.paths.log_dir(), "app.log"),
    ] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), b"x").unwrap();
    }
    let temp = fx.paths.temp_dir();
    std::fs::write(temp.join(format!("{TEMP_PREFIX}1.tmp")), b"x").unwrap();
    std::fs::create_dir_all(temp.join(format!("{TEMP_PREFIX}dir"))).unwrap();
    std::fs::write(temp.join("other.tmp"), b"x").unwrap();
    let mut deletes = fx.paths.owned_paths().to_vec();
    deletes.push(temp.join(format!("{TEMP_PREFIX}1.tmp")));
    deletes.push(temp.join(format!("{TEMP_PREFIX}dir")));
    deletes
}

fn as_deletes(paths: &[PathBuf]) -> Vec<CleanupAction> {
    paths.iter().cloned().map(CleanupAction::Delete).collect()
}

fn cleanup(fx: &Fx) -> rf_ledger::CleanupReport {
    run_cleanup(&fx.paths, Box::new(fx.fake.clone())).unwrap()
}

#[test]
fn dry_run_lists_reverts_then_deletes_and_changes_nothing() {
    let fx = installed();
    let deletes = used(&fx);
    let calls = fx.fake.calls().len();
    let mut want = vec![
        CleanupAction::Revert {
            id: EntryId(2),
            change: r"HKCU\Software\T2 @ v".into(),
        },
        CleanupAction::Revert {
            id: EntryId(1),
            change: r"HKCU\Software\T1 @ v".into(),
        },
    ];
    want.extend(as_deletes(&deletes));
    assert_eq!(plan_cleanup(&fx.paths).unwrap(), want);
    assert_eq!(fx.fake.calls().len(), calls);
    assert!(deletes.iter().all(|p| p.exists()));
    assert_eq!(Ledger::inspect(&fx.paths).unwrap().len(), 2);
}

#[test]
fn cleanup_reverts_then_deletes_owned_paths_and_prefixed_temp_entries() {
    let fx = installed();
    let deletes = used(&fx);
    let r = cleanup(&fx);
    assert_eq!(r.reverted.reverted, [EntryId(2), EntryId(1)]);
    assert_eq!(r.done, as_deletes(&deletes));
    assert!(r.is_complete() && !r.deletes_skipped);
    assert_eq!(
        (fx.fake.get(&slot(1)), fx.fake.get(&slot(2))),
        (s("orig"), None)
    );
    assert!(deletes.iter().all(|p| p.symlink_metadata().is_err()));
    assert!(fx.paths.temp_dir().join("other.tmp").exists());
}

#[test]
fn cleanup_keeps_everything_when_a_revert_fails() {
    let fx = installed();
    let deletes = used(&fx);
    fx.fake.fail_next(FakeOp::Restore, &slot(1));
    let r = cleanup(&fx);
    assert!(r.deletes_skipped && r.done.is_empty() && !r.is_complete());
    assert_eq!(r.reverted.failed[0].id, EntryId(1));
    assert!(deletes.iter().all(|p| p.exists()));
    // The ledger still records the change that is owed, and only that one.
    let left: Vec<EntryId> = Ledger::inspect(&fx.paths)
        .unwrap()
        .iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(left, [EntryId(1)]);
    let again = cleanup(&fx);
    assert!(again.is_complete(), "{again:?}");
    assert_eq!(again.reverted.reverted, [EntryId(1)]);
    assert_eq!(fx.fake.get(&slot(1)), s("orig"));
    assert!(deletes.iter().all(|p| p.symlink_metadata().is_err()));
}

#[test]
fn cleanup_finishes_after_a_crash_mid_revert() {
    let fx = installed();
    let deletes = used(&fx);
    fx.fake.panic_after_next(FakeOp::Restore, &slot(2));
    assert!(catch_unwind(AssertUnwindSafe(|| cleanup(&fx))).is_err());
    // The process died before the ledger file was updated: both changes are still recorded,
    // and nothing was deleted.
    assert_eq!(Ledger::inspect(&fx.paths).unwrap().len(), 2);
    assert!(deletes.iter().all(|p| p.exists()));
    let calls = fx.fake.calls().len();
    let r = cleanup(&fx);
    assert!(r.is_complete(), "{r:?}");
    // Slot 2 was put back before the crash, so only slot 1 is restored now.
    assert_eq!(r.reverted.reverted, [EntryId(1)]);
    assert_eq!(r.reverted.skipped, [EntryId(2)]);
    assert_eq!(
        fx.fake.calls()[calls..],
        [FakeOp::IsApplied, FakeOp::IsApplied, FakeOp::Restore]
    );
    assert_eq!(
        (fx.fake.get(&slot(1)), fx.fake.get(&slot(2))),
        (s("orig"), None)
    );
    assert!(deletes.iter().all(|p| p.symlink_metadata().is_err()));
}

#[test]
fn cleanup_refuses_to_delete_when_the_ledger_is_unreadable() {
    let fx = installed();
    let deletes = used(&fx);
    let file = Ledger::file_path(&fx.paths);
    let readable = std::fs::read(&file).unwrap();
    std::fs::write(&file, b"{").unwrap();
    assert!(matches!(
        run_cleanup(&fx.paths, Box::new(fx.fake.clone())),
        Err(LedgerError::Corrupt { .. })
    ));
    assert!(matches!(
        plan_cleanup(&fx.paths),
        Err(LedgerError::Corrupt { .. })
    ));
    assert!(deletes.iter().all(|p| p.exists()));
    assert_eq!(std::fs::read(&file).unwrap(), b"{");

    // A newer build's ledger that records the same two changes: nothing is reverted or
    // deleted, and the file stays as it is.
    let mut newer: serde_json::Value = serde_json::from_slice(&readable).unwrap();
    newer["format"] = (Ledger::FORMAT + 1).into();
    let newer = serde_json::to_vec(&newer).unwrap();
    std::fs::write(&file, &newer).unwrap();
    let calls = fx.fake.calls().len();
    assert!(matches!(
        run_cleanup(&fx.paths, Box::new(fx.fake.clone())),
        Err(LedgerError::UnsupportedFormat { .. })
    ));
    assert!(matches!(
        plan_cleanup(&fx.paths),
        Err(LedgerError::UnsupportedFormat { .. })
    ));
    assert!(deletes.iter().all(|p| p.exists()));
    assert_eq!(fx.fake.calls().len(), calls);
    assert_eq!(std::fs::read(&file).unwrap(), newer);
}

#[cfg(unix)]
#[test]
fn cleanup_removes_links_but_not_what_they_point_to() {
    let fx = installed();
    let outside = fx.root().join("elsewhere");
    for (sub, file) in [("cache", "keep.bin"), ("config", "keep.toml")] {
        std::fs::create_dir_all(outside.join(sub)).unwrap();
        std::fs::write(outside.join(sub).join(file), b"x").unwrap();
    }
    std::fs::create_dir_all(fx.paths.cache_dir().parent().unwrap()).unwrap();
    std::fs::create_dir_all(fx.paths.config_dir().parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(outside.join("cache"), fx.paths.cache_dir()).unwrap();
    std::os::unix::fs::symlink(outside.join("config"), fx.paths.config_dir()).unwrap();
    // The links lead somewhere, so the checks below cannot pass for want of a target.
    assert!(fx.paths.cache_dir().join("keep.bin").exists());
    let r = cleanup(&fx);
    assert!(r.is_complete(), "{r:?}");
    assert!(
        fx.paths
            .owned_paths()
            .iter()
            .all(|p| p.symlink_metadata().is_err())
    );
    assert!(outside.join("cache").join("keep.bin").exists());
    assert!(outside.join("config").join("keep.toml").exists());
}

/// A junction is the folder link Windows makes without privilege: the settings folder itself
/// (an owned path) and the cache folder inside the local one lead to another drive.
#[cfg(windows)]
#[test]
fn cleanup_removes_junctions_but_not_what_they_point_to() {
    let fx = installed();
    let outside = fx.root().join("elsewhere");
    for (sub, file) in [("cache", "keep.bin"), ("config", "keep.toml")] {
        std::fs::create_dir_all(outside.join(sub)).unwrap();
        std::fs::write(outside.join(sub).join(file), b"x").unwrap();
    }
    std::fs::create_dir_all(fx.paths.cache_dir().parent().unwrap()).unwrap();
    std::fs::create_dir_all(fx.paths.config_dir().parent().unwrap()).unwrap();
    let made = junction(&outside.join("cache"), fx.paths.cache_dir())
        .and_then(|()| junction(&outside.join("config"), fx.paths.config_dir()));
    if let Err(e) = made {
        eprintln!("skipped: cannot create junctions here: {e}");
        return;
    }
    assert!(fx.paths.config_dir().join("keep.toml").exists());
    let r = cleanup(&fx);
    assert!(r.is_complete(), "{r:?}");
    assert!(
        fx.paths
            .owned_paths()
            .iter()
            .all(|p| p.symlink_metadata().is_err())
    );
    assert!(outside.join("cache").join("keep.bin").exists());
    assert!(outside.join("config").join("keep.toml").exists());
}

/// A folder symbolic link as an owned path, and a file symbolic link as a temp entry: each
/// link is removed the way Windows removes that kind, and the targets stay.
#[cfg(windows)]
#[test]
fn cleanup_removes_windows_symlinks_but_not_what_they_point_to() {
    let fx = installed();
    let outside = fx.root().join("elsewhere");
    std::fs::create_dir_all(outside.join("config")).unwrap();
    std::fs::write(outside.join("config").join("keep.toml"), b"x").unwrap();
    std::fs::write(outside.join("keep.tmp"), b"x").unwrap();
    std::fs::create_dir_all(fx.paths.config_dir().parent().unwrap()).unwrap();
    let temp_link = fx.paths.temp_dir().join(format!("{TEMP_PREFIX}link.tmp"));
    let made = std::os::windows::fs::symlink_dir(outside.join("config"), fx.paths.config_dir())
        .and_then(|()| std::os::windows::fs::symlink_file(outside.join("keep.tmp"), &temp_link));
    if let Err(e) = made {
        eprintln!("skipped: cannot create symbolic links here: {e}");
        return;
    }
    assert!(fx.paths.config_dir().join("keep.toml").exists() && temp_link.exists());
    let r = cleanup(&fx);
    assert!(r.is_complete(), "{r:?}");
    assert!(r.done.contains(&CleanupAction::Delete(temp_link.clone())));
    assert!(
        fx.paths
            .owned_paths()
            .iter()
            .all(|p| p.symlink_metadata().is_err())
    );
    assert!(temp_link.symlink_metadata().is_err());
    assert!(outside.join("config").join("keep.toml").exists());
    assert!(outside.join("keep.tmp").exists());
}

/// Makes `link` a junction to the folder `target`.
#[cfg(windows)]
fn junction(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("mklink /J failed: {status}")))
    }
}

#[test]
fn portable_cleanup_deletes_only_the_data_folder() {
    let fx = portable();
    std::fs::create_dir_all(fx.paths.cache_dir()).unwrap();
    let exe = fx.root().join("bin").join("app.exe");
    std::fs::write(&exe, b"exe").unwrap();
    let r = cleanup(&fx);
    assert_eq!(r.done, as_deletes(fx.paths.owned_paths()));
    assert!(exe.exists() && fx.root().join("bin").join(PORTABLE_MARKER).exists());
    assert_eq!(fx.fake.calls(), []);
}

/// A ledger file in the portable data folder (say one copied from an installed setup) is not
/// read: its changes are not reverted, and one that does not parse is no obstacle either.
#[test]
fn portable_cleanup_never_reads_a_ledger_file_in_the_data_folder() {
    let installed_fx = installed();
    installed_fx
        .open()
        .apply("g", set(&slot(1), "ours"))
        .unwrap();
    let copy = std::fs::read(Ledger::file_path(&installed_fx.paths)).unwrap();
    let cut_off: &[u8] = b"{";
    let fx = portable();
    let file = Ledger::file_path(&fx.paths);
    for text in [copy.as_slice(), cut_off] {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        assert_eq!(
            plan_cleanup(&fx.paths).unwrap(),
            as_deletes(fx.paths.owned_paths())
        );
        let r = cleanup(&fx);
        assert!(r.is_complete(), "{r:?}");
        assert_eq!(r.done, as_deletes(fx.paths.owned_paths()));
    }
    assert_eq!(fx.fake.calls(), []);
}

#[test]
fn cleanup_twice_is_harmless() {
    let fx = installed();
    used(&fx);
    assert!(cleanup(&fx).is_complete());
    let again = cleanup(&fx);
    assert!(again.is_complete() && again.done.is_empty() && again.reverted.reverted.is_empty());
}

#[test]
fn macos_cleanup_clears_the_preferences_domain_before_deleting_the_plist() {
    let fx = macos();
    let plist = fx.paths.preferences_file().unwrap().to_path_buf();
    std::fs::create_dir_all(plist.parent().unwrap()).unwrap();
    std::fs::write(&plist, b"plist").unwrap();
    let plan = plan_cleanup(&fx.paths).unwrap();
    assert_eq!(
        plan,
        [
            CleanupAction::ClearDomain(BUNDLE_ID.into()),
            CleanupAction::Delete(plist.clone())
        ]
    );
    let r = cleanup(&fx);
    assert!(r.is_complete() && r.done == plan);
    assert_eq!(fx.fake.calls(), [FakeOp::ClearDomain]);
    assert!(!plist.exists());
}
