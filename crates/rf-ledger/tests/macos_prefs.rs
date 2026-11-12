#![cfg(target_os = "macos")]

//! The preferences backend against the real preferences of the current user. The tests own the
//! keys they name in the domains `<BUNDLE_ID>.ledgertest` and `<BUNDLE_ID>.ledgertest.clear` and
//! touch no other domain, except the CI-only test, which changes the global `NSFileViewer` and
//! puts it back.
//!
//! What the backend wrote is read back with `/usr/bin/defaults`, another process, so the checks
//! see what every other app sees.

mod common;

use std::process::Command;

use rf_core::identity::BUNDLE_ID;
use rf_ledger::change::{FILE_VIEWER_KEY, GLOBAL_DOMAIN};
use rf_ledger::{
    ApplyOutcome, Before, Change, Ledger, LedgerError, ServiceKind, Target, Value, os_backend,
};

/// Runs `/usr/bin/defaults` with `args`: what it printed without the final newline, or `None`
/// when it failed (for `read`: the domain or the key does not exist).
fn defaults(args: &[&str]) -> Option<String> {
    let out = Command::new("/usr/bin/defaults")
        .args(args)
        .output()
        .unwrap();
    out.status.success().then(|| {
        let text = String::from_utf8_lossy(&out.stdout);
        text.strip_suffix('\n').unwrap_or(&text).to_owned()
    })
}

/// How `defaults` names `domain`: `-g` for the global domain.
fn domain_arg(domain: &str) -> &str {
    if domain == GLOBAL_DOMAIN {
        "-g"
    } else {
        domain
    }
}

/// `defaults read <domain> <key>` (`-g` for the global domain); `None` when it does not exist.
fn defaults_read(domain: &str, key: &str) -> Option<String> {
    defaults(&["read", domain_arg(domain), key]).map(|s| s.trim().to_owned())
}

/// The property list type of a value, from `defaults read-type`: `boolean`, `integer`, `float`,
/// `string`, `dictionary`, …; `None` when it does not exist.
fn defaults_type(domain: &str, key: &str) -> Option<String> {
    defaults(&["read-type", domain_arg(domain), key])
        .map(|s| s.trim().trim_start_matches("Type is ").to_owned())
}

fn test_domain(suffix: &str) -> String {
    format!("{BUNDLE_ID}.ledgertest{suffix}")
}

/// Deletes a test's keys when the test ends, pass or fail.
///
/// It is armed only after the test has seen that none of its keys exists, so it never deletes a
/// value the test did not write.
struct Cleanup {
    domain: String,
    keys: &'static [&'static str],
}

impl Cleanup {
    fn arm(domain: &str, keys: &'static [&'static str]) -> Self {
        for key in keys {
            assert_eq!(
                defaults_read(domain, key),
                None,
                "stale `{domain} {key}`: run `defaults delete {domain} {key}` and rerun"
            );
        }
        Self {
            domain: domain.to_owned(),
            keys,
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for key in self.keys {
            // Fails when the test already removed the key.
            let _ = defaults(&["delete", &self.domain, key]);
        }
    }
}

/// Puts the global `NSFileViewer` back as the test found it when the test ends, pass or fail.
struct RestoreFileViewer(Option<String>);

impl RestoreFileViewer {
    /// Records the value. Only a string can be written back with `defaults`, so any other type
    /// stops the test before it changes anything.
    fn arm() -> Self {
        let original = defaults(&["read", "-g", FILE_VIEWER_KEY]);
        if original.is_some() {
            assert_eq!(
                defaults_type(GLOBAL_DOMAIN, FILE_VIEWER_KEY).as_deref(),
                Some("string"),
                "the global {FILE_VIEWER_KEY} is not a string: not touching it"
            );
        }
        Self(original)
    }
}

impl Drop for RestoreFileViewer {
    fn drop(&mut self) {
        // After a passing test this writes (or deletes) what is already there.
        let _ = match &self.0 {
            Some(viewer) => defaults(&["write", "-g", FILE_VIEWER_KEY, "-string", viewer]),
            None => defaults(&["delete", "-g", FILE_VIEWER_KEY]),
        };
    }
}

#[test]
fn own_domain_changes_are_visible_to_defaults_and_revert() {
    let d = test_domain("");
    let _cleanup = Cleanup::arm(&d, &["Probe"]);
    let fx = common::installed();
    let (mut l, _) = Ledger::open(&fx.paths, os_backend()).unwrap();
    l.apply(
        "test",
        Change::preference(&d, "Probe", Some(Value::Str("probe".into()))).unwrap(),
    )
    .unwrap();
    assert_eq!(defaults_read(&d, "Probe").as_deref(), Some("probe"));
    l.apply(
        "test",
        Change::preference(&d, "Probe", Some(Value::I64(5))).unwrap(),
    )
    .unwrap();
    assert_eq!(defaults_read(&d, "Probe").as_deref(), Some("5"));
    assert!(l.revert_all().is_clean());
    assert_eq!(defaults_read(&d, "Probe"), None);
}

#[test]
fn restores_a_dictionary_value_exactly() {
    let d = test_domain("");
    let _cleanup = Cleanup::arm(&d, &["Dict"]);
    let xml = br#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>a</key><integer>1</integer></dict></plist>"#;
    let mut backend = os_backend();
    let seed = Change::preference(&d, "Dict", Some(Value::Plist(xml.to_vec()))).unwrap();
    let seed_before = backend.capture(&seed).unwrap();
    assert_eq!(seed_before, Before::default());
    backend.apply(&seed).unwrap();
    let fx = common::installed();
    let (mut l, _) = Ledger::open(&fx.paths, os_backend()).unwrap();
    l.apply(
        "test",
        Change::preference(&d, "Dict", Some(Value::Bool(true))).unwrap(),
    )
    .unwrap();
    assert_eq!(defaults_type(&d, "Dict").as_deref(), Some("boolean"));
    assert!(l.revert_all().is_clean());
    assert_eq!(defaults_type(&d, "Dict").as_deref(), Some("dictionary"));
    assert!(defaults_read(&d, "Dict").unwrap().contains("a = 1;"));
    backend.restore(&seed, &seed_before).unwrap();
    assert_eq!(defaults_read(&d, "Dict"), None);
}

/// A value of another type is not the app's value, even though `CFPreferences` finds the two
/// equal (an integer 1 and the boolean `true` are both `NSNumber`s) and skips a plain write of
/// one over the other: the app's value is applied over it with its own type, and the revert
/// brings back the value with its type.
#[test]
fn the_type_is_part_of_the_value() {
    let d = test_domain("");
    let _cleanup = Cleanup::arm(&d, &["Typed"]);
    let real_one = br#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><real>1</real></plist>"#;
    let cases = [
        (Value::I64(1), "integer", Value::Bool(true), "boolean"),
        (
            Value::Plist(real_one.to_vec()),
            "float",
            Value::I64(1),
            "integer",
        ),
    ];
    for (theirs, their_type, ours, our_type) in cases {
        let case = format!("{our_type} over {their_type}");
        let mut backend = os_backend();
        let seed = Change::preference(&d, "Typed", Some(theirs)).unwrap();
        backend.apply(&seed).unwrap();
        assert_eq!(
            defaults_type(&d, "Typed").as_deref(),
            Some(their_type),
            "{case}: seed"
        );
        let ours = Change::preference(&d, "Typed", Some(ours)).unwrap();
        assert!(!backend.is_applied(&ours).unwrap(), "{case}");
        let fx = common::installed();
        let (mut l, _) = Ledger::open(&fx.paths, os_backend()).unwrap();
        // An error names the type found after the write.
        let outcome = l.apply("test", ours.clone());
        assert!(
            matches!(outcome, Ok(ApplyOutcome::Recorded(_))),
            "{case}: {outcome:?}"
        );
        assert_eq!(
            defaults_type(&d, "Typed").as_deref(),
            Some(our_type),
            "{case}: applied"
        );
        assert!(backend.is_applied(&ours).unwrap(), "{case}: applied");
        let r = l.revert_all();
        assert!(r.is_clean() && r.reverted.len() == 1, "{case}: {r:?}");
        assert_eq!(
            (
                defaults_type(&d, "Typed").as_deref(),
                defaults_read(&d, "Typed").as_deref()
            ),
            (Some(their_type), Some("1")),
            "{case}: reverted"
        );
        assert!(backend.is_applied(&seed).unwrap(), "{case}: reverted");
        backend
            .apply(&Change::preference(&d, "Typed", None).unwrap())
            .unwrap();
        assert_eq!(defaults_read(&d, "Typed"), None);
    }
}

/// A value that is there but empty is captured as a value, so the revert puts the empty value
/// back instead of deleting the key.
#[test]
fn an_empty_value_is_not_a_missing_one() {
    let d = test_domain("");
    let _cleanup = Cleanup::arm(&d, &["Empty"]);
    let mut backend = os_backend();
    let empty = Change::preference(&d, "Empty", Some(Value::Str(String::new()))).unwrap();
    assert_eq!(backend.capture(&empty).unwrap(), Before::default());
    backend.apply(&empty).unwrap();
    assert!(matches!(
        backend.capture(&empty).unwrap().value,
        Some(Value::Plist(_))
    ));
    let fx = common::installed();
    let (mut l, _) = Ledger::open(&fx.paths, os_backend()).unwrap();
    l.apply(
        "test",
        Change::preference(&d, "Empty", Some(Value::Str("x".into()))).unwrap(),
    )
    .unwrap();
    assert_eq!(defaults_read(&d, "Empty").as_deref(), Some("x"));
    assert!(l.revert_all().is_clean());
    assert_eq!(
        (
            defaults_type(&d, "Empty").as_deref(),
            defaults_read(&d, "Empty").as_deref()
        ),
        (Some("string"), Some(""))
    );
    backend
        .apply(&Change::preference(&d, "Empty", None).unwrap())
        .unwrap();
    assert_eq!(defaults_read(&d, "Empty"), None);
}

#[test]
fn clear_domain_removes_every_key() {
    let d = test_domain(".clear");
    let _cleanup = Cleanup::arm(&d, &["A", "B"]);
    let mut backend = os_backend();
    for k in ["A", "B"] {
        backend
            .apply(&Change::preference(&d, k, Some(Value::Bool(true))).unwrap())
            .unwrap();
    }
    assert_eq!(defaults_read(&d, "A").as_deref(), Some("1"));
    backend.clear_domain(&d).unwrap();
    assert_eq!(
        (defaults_read(&d, "A"), defaults_read(&d, "B")),
        (None, None)
    );
}

/// Each of these is refused while it is checked, before any preference is read or written. The
/// one well-formed preference among them, `Never` in the test domain, is not written by any test.
#[test]
fn refuses_other_kinds_and_domains_it_does_not_own() {
    let mut b = os_backend();
    let registry = Change::registry(r"HKCU\Software\X", "v", None).unwrap();
    assert!(matches!(
        b.capture(&registry),
        Err(LedgerError::Unsupported(_))
    ));
    let service = Change::service(ServiceKind::LaunchAgent, "rf-indexd", None).unwrap();
    assert!(matches!(
        b.apply(&service),
        Err(LedgerError::Unsupported(_))
    ));

    // What a tampered ledger file could hold: a path for a domain (`CFPreferences` would write
    // that file), and before-values that are not preference values.
    let path = Change {
        target: Target::Preference {
            domain: "/tmp/x".into(),
            key: "k".into(),
        },
        value: None,
    };
    assert!(matches!(b.apply(&path), Err(LedgerError::InvalidTarget(_))));
    let ours = Change::preference(&test_domain(""), "Never", None).unwrap();
    for value in [
        Value::Raw {
            ty: 1,
            data: Vec::new(),
        },
        Value::Plist(Vec::new()),
    ] {
        let forged = Before {
            value: Some(value),
            created_keys: Vec::new(),
        };
        assert!(matches!(
            b.restore(&ours, &forged),
            Err(LedgerError::InvalidValue(_))
        ));
    }

    // Only the app's own domains can be cleared (`owns_domain`, tested in change.rs). These two
    // are not the app's, and no one else's either: a broken check would clear nothing that
    // matters.
    for domain in [format!("{BUNDLE_ID}x"), format!("{BUNDLE_ID}.")] {
        assert!(
            matches!(b.clear_domain(&domain), Err(LedgerError::InvalidTarget(_))),
            "{domain}"
        );
    }
}

#[test]
fn global_file_viewer_roundtrip_on_ci_only() {
    if std::env::var_os("GITHUB_ACTIONS").is_none() {
        eprintln!("skipped: changes the global NSFileViewer default; runs only on CI");
        return;
    }
    let _restore = RestoreFileViewer::arm();
    let original = defaults_read(GLOBAL_DOMAIN, FILE_VIEWER_KEY);
    let fx = common::installed();
    let (mut l, _) = Ledger::open(&fx.paths, os_backend()).unwrap();
    l.apply("macx.file-viewer", Change::macos_file_viewer())
        .unwrap();
    assert_eq!(
        defaults_read(GLOBAL_DOMAIN, FILE_VIEWER_KEY).as_deref(),
        Some(BUNDLE_ID)
    );
    assert!(l.revert_group("macx.file-viewer").is_clean());
    assert_eq!(defaults_read(GLOBAL_DOMAIN, FILE_VIEWER_KEY), original);
}
