use rf_core::identity::BUNDLE_ID;
use rf_ledger::change::{FILE_VIEWER_KEY, GLOBAL_DOMAIN, hkcu_subkey, registry_data};
use rf_ledger::{Change, LedgerError, ServiceKind, Target, Value};

fn reg_err(key: &str) -> Option<LedgerError> {
    Change::registry(key, "v", None).err()
}

#[test]
fn registry_changes_accept_only_hkcu() {
    let c = Change::registry(r"HKEY_CURRENT_USER\Software\X", "", None).unwrap();
    assert_eq!(
        c.target,
        Target::Registry {
            key: r"HKCU\Software\X".into(),
            name: String::new()
        }
    );
    assert_eq!(hkcu_subkey(r"hkcu\Software\X").unwrap(), r"Software\X");
    for bad in [
        r"HKLM\Software\X",
        r"HKEY_LOCAL_MACHINE\Software\X",
        r"HKCR\.txt",
        r"HKEY_CLASSES_ROOT\.txt",
        r"HKU\S-1-5-18\X",
        r"HKEY_USERS\X",
        r"HKCC\X",
        r"Software\X",
        "HKCU/Software/X",
        r"HKCUX\Software",
    ] {
        assert!(
            matches!(reg_err(bad), Some(LedgerError::OutsideHkcu(k)) if k == bad),
            "{bad}"
        );
    }
}

#[test]
fn registry_changes_reject_malformed_keys_names_and_values() {
    let long = format!(r"HKCU\{}", "k".repeat(256));
    for bad in [
        "HKCU",
        r"HKCU\",
        r"HKCU\a\\b",
        r"HKCU\a\",
        "HKCU\\a\0b",
        long.as_str(),
    ] {
        assert!(
            matches!(reg_err(bad), Some(LedgerError::InvalidTarget(_))),
            "{bad:?}"
        );
    }
    assert!(matches!(
        Change::registry(r"HKCU\a", "n\0", None),
        Err(LedgerError::InvalidTarget(_))
    ));
    for v in [
        Value::Str("a\0b".into()),
        Value::MultiStr(vec!["a".into(), String::new()]),
        Value::Bool(true),
        Value::Program {
            path: "x".into(),
            args: vec![],
        },
    ] {
        assert!(
            matches!(
                Change::registry(r"HKCU\a", "n", Some(v.clone())),
                Err(LedgerError::InvalidValue(_))
            ),
            "{v:?}"
        );
    }
}

#[test]
fn preference_changes_reject_paths_and_registry_values() {
    assert!(Change::preference(GLOBAL_DOMAIN, FILE_VIEWER_KEY, Some(Value::Bool(true))).is_ok());
    for (domain, key) in [
        ("/tmp/x.plist", "k"),
        ("~/Library/x", "k"),
        ("", "k"),
        ("a.b", ""),
        ("a\0b", "k"),
        (".hidden", "k"),
    ] {
        assert!(
            matches!(
                Change::preference(domain, key, None),
                Err(LedgerError::InvalidTarget(_))
            ),
            "{domain:?} {key:?}"
        );
    }
    for v in [
        Value::U32(1),
        Value::Raw {
            ty: 1,
            data: vec![],
        },
        Value::ExpandStr("x".into()),
    ] {
        assert!(matches!(
            Change::preference("a.b", "k", Some(v)),
            Err(LedgerError::InvalidValue(_))
        ));
    }
}

#[test]
fn file_viewer_change_points_at_the_app() {
    assert_eq!(
        (GLOBAL_DOMAIN, FILE_VIEWER_KEY),
        ("NSGlobalDomain", "NSFileViewer")
    );
    let c = Change::macos_file_viewer();
    assert_eq!(
        c.target,
        Target::Preference {
            domain: GLOBAL_DOMAIN.into(),
            key: FILE_VIEWER_KEY.into()
        }
    );
    assert_eq!(c.value, Some(Value::Str(BUNDLE_ID.into())));
    assert!(c.validate().is_ok());
}

#[test]
fn service_change_is_a_reserved_record_shape() {
    let program = Value::Program {
        path: r"C:\x\rf-indexd.exe".into(),
        args: vec!["--service".into()],
    };
    let c = Change::service(ServiceKind::WindowsService, "rf-indexd", Some(program)).unwrap();
    let json = serde_json::to_string(&c).unwrap();
    assert_eq!(serde_json::from_str::<Change>(&json).unwrap(), c);
    assert!(matches!(
        Change::service(ServiceKind::LaunchAgent, "a/b", None),
        Err(LedgerError::InvalidTarget(_))
    ));
    assert!(matches!(
        Change::service(ServiceKind::LaunchAgent, "a", Some(Value::Str("x".into()))),
        Err(LedgerError::InvalidValue(_))
    ));
}

#[test]
fn json_shape_is_stable() {
    let c = Change::registry(r"HKCU\Software\X", "", Some(Value::Str("a".into()))).unwrap();
    assert_eq!(
        serde_json::to_string(&c).unwrap(),
        r#"{"target":{"kind":"registry","key":"HKCU\\Software\\X","name":""},"value":{"type":"str","data":"a"}}"#
    );
    let raw = Value::Raw {
        ty: 0,
        data: vec![1, 2],
    };
    assert_eq!(
        serde_json::to_string(&raw).unwrap(),
        r#"{"type":"raw","data":{"ty":0,"data":[1,2]}}"#
    );
    for v in [
        Value::ExpandStr("%A%".into()),
        Value::MultiStr(vec!["a".into()]),
        Value::U32(1),
        Value::U64(2),
        Value::Bytes(vec![3]),
        Value::Bool(true),
        Value::I64(-1),
        Value::Plist(vec![4]),
        raw,
    ] {
        let back: Value = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(back, v);
    }
}

#[test]
fn registry_slots_compare_case_insensitively() {
    let a = Target::Registry {
        key: r"HKCU\Software\X".into(),
        name: "V".into(),
    };
    assert!(a.same_slot(&Target::Registry {
        key: r"hkcu\software\x".into(),
        name: "v".into()
    }));
    assert!(!a.same_slot(&Target::Registry {
        key: r"HKCU\Software\X".into(),
        name: "W".into()
    }));
    let p = Target::Preference {
        domain: "A".into(),
        key: "k".into(),
    };
    assert!(!p.same_slot(&Target::Preference {
        domain: "a".into(),
        key: "k".into()
    }));
}

#[test]
fn registry_data_encoding() {
    assert_eq!(
        registry_data(&Value::Str("ab".into())).unwrap(),
        (1, vec![0x61, 0, 0x62, 0, 0, 0])
    );
    assert_eq!(
        registry_data(&Value::ExpandStr("a".into())).unwrap(),
        (2, vec![0x61, 0, 0, 0])
    );
    assert_eq!(
        registry_data(&Value::MultiStr(vec!["a".into(), "b".into()])).unwrap(),
        (7, vec![0x61, 0, 0, 0, 0x62, 0, 0, 0, 0, 0])
    );
    assert_eq!(
        registry_data(&Value::MultiStr(vec![])).unwrap(),
        (7, vec![0, 0])
    );
    assert_eq!(
        registry_data(&Value::U32(0x0102_0304)).unwrap(),
        (4, vec![4, 3, 2, 1])
    );
    assert_eq!(
        registry_data(&Value::U64(1)).unwrap(),
        (11, vec![1, 0, 0, 0, 0, 0, 0, 0])
    );
    assert_eq!(registry_data(&Value::Bytes(vec![9])).unwrap(), (3, vec![9]));
    assert_eq!(
        registry_data(&Value::Raw {
            ty: 0,
            data: vec![1, 2]
        })
        .unwrap(),
        (0, vec![1, 2])
    );
    assert!(matches!(
        registry_data(&Value::Bool(true)),
        Err(LedgerError::InvalidValue(_))
    ));
}

#[test]
fn targets_display_for_logs_and_dry_runs() {
    let reg = |name: &str| Target::Registry {
        key: r"HKCU\Software\X".into(),
        name: name.into(),
    };
    assert_eq!(reg("").to_string(), r"HKCU\Software\X @ (default)");
    assert_eq!(reg("V").to_string(), r"HKCU\Software\X @ V");
    assert_eq!(
        Change::macos_file_viewer().target.to_string(),
        "defaults NSGlobalDomain NSFileViewer"
    );
    assert_eq!(
        Target::Service {
            service: ServiceKind::LaunchAgent,
            name: "a".into()
        }
        .to_string(),
        "launch_agent a"
    );
}
