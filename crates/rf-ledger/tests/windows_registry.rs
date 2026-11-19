#![cfg(target_os = "windows")]

//! The registry backend against the real registry. Every test owns one key and touches no
//! other: `HKCU\Software\<DATA_DIR_NAME>Test`, `…Test-raw` or `…Test-refuse`.

mod common;

use rf_core::identity::DATA_DIR_NAME;
use rf_ledger::{Before, Change, Ledger, LedgerError, ServiceKind, Target, Value, os_backend};
use windows_registry::CURRENT_USER;

/// Deletes a test's key tree when the test ends, pass or fail.
///
/// It is armed only after the test has seen that the key did not exist, so it never deletes
/// a key the test did not create.
struct Cleanup(String);

impl Cleanup {
    fn arm(root: &str) -> Self {
        assert!(
            CURRENT_USER.open(root).is_err(),
            r"stale HKCU\{root}: delete it and rerun"
        );
        Self(root.to_owned())
    }
}

impl Drop for Cleanup {
    #[expect(
        clippy::disallowed_methods,
        reason = "a test deletes the key tree it created, whatever state a failure left it in"
    )]
    fn drop(&mut self) {
        // An error here means the test already removed the key.
        let _ = CURRENT_USER.remove_tree(&self.0);
    }
}

#[test]
fn revert_removes_the_created_test_key() {
    let root = format!(r"Software\{DATA_DIR_NAME}Test");
    let _cleanup = Cleanup::arm(&root);
    let fx = common::installed();
    let (mut l, _) = Ledger::open(&fx.paths, os_backend()).unwrap();
    let deep = format!(r"HKCU\{root}\a\b");
    l.apply(
        "test",
        Change::registry(&deep, "s", Some(Value::Str("one".into()))).unwrap(),
    )
    .unwrap();
    l.apply(
        "test",
        Change::registry(&deep, "n", Some(Value::U32(7))).unwrap(),
    )
    .unwrap();
    l.apply(
        "test",
        Change::registry(&format!(r"HKCU\{root}\a"), "", Some(Value::Str("x".into()))).unwrap(),
    )
    .unwrap();
    {
        let b = CURRENT_USER.open(format!(r"{root}\a\b")).unwrap();
        assert_eq!(
            (b.get_string("s").unwrap(), b.get_u32("n").unwrap()),
            ("one".to_owned(), 7)
        );
        assert_eq!(
            CURRENT_USER
                .open(format!(r"{root}\a"))
                .unwrap()
                .get_string("")
                .unwrap(),
            "x"
        );
    }
    assert_eq!(
        l.entries()[0].before.created_keys,
        [format!(r"HKCU\{root}"), format!(r"HKCU\{root}\a"), deep]
    );
    let r = l.revert_all();
    assert!(r.is_clean() && r.reverted.len() == 3, "{r:?}");
    assert!(
        CURRENT_USER.open(&root).is_err(),
        r"HKCU\{root} still exists"
    );
}

#[test]
fn restores_a_preexisting_value_of_any_type_exactly() {
    let root = format!(r"Software\{DATA_DIR_NAME}Test-raw");
    let key = format!(r"HKCU\{root}");
    let _cleanup = Cleanup::arm(&root);
    // `REG_NONE`; a `REG_SZ` another program stored without its terminating NUL; an empty
    // `REG_SZ`. The registry takes all three as they are, so they must come back as they were.
    for (ty, data) in [(0, vec![1, 2, 3]), (1, vec![b'a', 0]), (1, vec![])] {
        let mut backend = os_backend();
        let seed = Change::registry(
            &key,
            "v",
            Some(Value::Raw {
                ty,
                data: data.clone(),
            }),
        )
        .unwrap();
        let seed_before = backend.capture(&seed).unwrap();
        assert_eq!(
            seed_before,
            Before {
                value: None,
                created_keys: vec![key.clone()]
            }
        );
        backend.apply(&seed).unwrap();
        let fx = common::installed();
        let (mut l, _) = Ledger::open(&fx.paths, os_backend()).unwrap();
        l.apply(
            "test",
            Change::registry(&key, "v", Some(Value::Str("ours".into()))).unwrap(),
        )
        .unwrap();
        assert!(l.revert_all().is_clean());
        {
            let v = CURRENT_USER.open(&root).unwrap().get_value("v").unwrap();
            assert_eq!((u32::from(v.ty()), v.to_vec()), (ty, data));
        }
        backend.restore(&seed, &seed_before).unwrap();
        assert!(CURRENT_USER.open(&root).is_err());
    }
}

#[test]
fn refuses_other_hives_and_other_kinds_before_touching_the_registry() {
    let mut b = os_backend();
    let hklm = Change {
        target: Target::Registry {
            key: r"HKLM\Software\X".into(),
            name: String::new(),
        },
        value: None,
    };
    assert!(matches!(b.capture(&hklm), Err(LedgerError::OutsideHkcu(_))));
    assert!(matches!(b.apply(&hklm), Err(LedgerError::OutsideHkcu(_))));
    let ok = Change::registry(
        &format!(r"HKCU\Software\{DATA_DIR_NAME}Test-refuse"),
        "v",
        None,
    )
    .unwrap();
    let forged = Before {
        value: None,
        created_keys: vec![r"HKLM\Software\X".into()],
    };
    assert!(matches!(
        b.restore(&ok, &forged),
        Err(LedgerError::OutsideHkcu(_))
    ));
    assert!(matches!(
        b.capture(&Change::macos_file_viewer()),
        Err(LedgerError::Unsupported(_))
    ));
    let svc = Change::service(ServiceKind::WindowsService, "rf-indexd", None).unwrap();
    assert!(matches!(b.capture(&svc), Err(LedgerError::Unsupported(_))));
}

/// Shares `…Test-refuse` with the test above, which never writes to the registry.
#[test]
fn leaves_keys_it_did_not_create_or_that_are_not_empty() {
    let root = format!(r"Software\{DATA_DIR_NAME}Test-refuse");
    let _cleanup = Cleanup::arm(&root);
    let mut b = os_backend();
    let (top, mid, deep) = (
        format!(r"HKCU\{root}"),
        format!(r"HKCU\{root}\a"),
        format!(r"HKCU\{root}\a\b"),
    );

    // Removing a value of a missing key succeeds and creates no key on the way.
    let gone = Change::registry(&deep, "w", None).unwrap();
    assert_eq!(b.capture(&gone).unwrap(), Before::default());
    assert!(b.is_applied(&gone).unwrap());
    b.apply(&gone).unwrap();
    let missing = Before {
        value: None,
        created_keys: vec![top.clone(), mid.clone(), deep.clone()],
    };
    b.restore(&gone, &missing).unwrap();
    assert!(
        CURRENT_USER.open(&root).is_err(),
        r"HKCU\{root} was created"
    );

    // A created key that someone else has put a value in since stays, and so do its parents.
    let ours = Change::registry(&deep, "w", Some(Value::U32(1))).unwrap();
    let before = b.capture(&ours).unwrap();
    assert_eq!(before.created_keys, [top, mid.clone(), deep]);
    b.apply(&ours).unwrap();
    b.apply(&Change::registry(&mid, "theirs", Some(Value::U32(2))).unwrap())
        .unwrap();
    b.restore(&ours, &before).unwrap();
    let a = CURRENT_USER.open(format!(r"{root}\a")).unwrap();
    assert_eq!(a.get_u32("theirs").unwrap(), 2);
    assert!(
        a.open("b").is_err(),
        "the empty key it created was left behind"
    );
}
