//! PRD INST-02: every OS change goes through rf-ledger. Enforced by clippy.toml plus these scans.

use std::path::{Path, PathBuf};

const REQUIRED: &[&str] = &[
    "windows_registry::Key::create",
    "windows_registry::Key::set_bytes",
    "windows_registry::Key::set_string",
    "windows_registry::Key::set_u32",
    "windows_registry::Key::set_value",
    "windows_registry::Key::remove_value",
    "windows_registry::Key::remove_tree",
    "windows::Win32::System::Registry::RegSetValueExW",
    "windows::Win32::System::Registry::RegSetValueExA",
    "windows::Win32::System::Registry::RegSetValueW",
    "windows::Win32::System::Registry::RegCreateKeyExW",
    "windows::Win32::System::Registry::RegCreateKeyW",
    "windows::Win32::System::Registry::RegDeleteTreeW",
    "windows::Win32::System::Registry::RegRestoreKeyW",
    "windows::Win32::UI::Shell::SHSetValueW",
    "windows::Win32::UI::Shell::SHDeleteKeyW",
    "windows::Win32::UI::Shell::SHSetKnownFolderPath",
    "windows_sys::Win32::System::Registry::RegSetValueExW",
    "windows_sys::Win32::UI::Shell::SHSetValueW",
    "objc2_core_foundation::CFPreferencesSetValue",
    "objc2_core_foundation::CFPreferencesSetAppValue",
    "objc2_foundation::NSUserDefaults::setObject_forKey",
];

/// OS settings tools that must not be spawned outside rf-ledger.
const TOOLS: &[&str] = &[
    "defaults",
    "/usr/bin/defaults",
    "reg",
    "reg.exe",
    "launchctl",
    "/bin/launchctl",
    "sc",
    "sc.exe",
    "schtasks",
    "schtasks.exe",
];

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd {
        let p = e.unwrap().path();
        if p.file_name().is_some_and(|n| n == "target") {
            continue;
        }
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// `.rs` files outside crates/rf-ledger whose text matches `bad`.
fn offenders(bad: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let ledger = workspace().join("crates").join("rf-ledger");
    let mut files = Vec::new();
    rust_files(&workspace().join("crates"), &mut files);
    rust_files(&workspace().join("apps"), &mut files);
    assert!(
        files.iter().any(|f| f.starts_with(&ledger)),
        "scan did not reach rf-ledger: {files:?}"
    );
    files
        .into_iter()
        .filter(|f| !f.starts_with(&ledger))
        .filter(|f| bad(&std::fs::read_to_string(f).unwrap()))
        .collect()
}

#[test]
fn clippy_toml_disallows_os_write_apis() {
    let text = std::fs::read_to_string(workspace().join("clippy.toml")).unwrap();
    let table: toml::Table = toml::from_str(&text).unwrap();
    let entries = table
        .get("disallowed-methods")
        .and_then(toml::Value::as_array)
        .expect("clippy.toml has no disallowed-methods");
    let field = |e: &toml::Value, k: &str| {
        e.get(k)
            .and_then(toml::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    for required in REQUIRED {
        let entry = entries
            .iter()
            .find(|&e| field(e, "path") == *required)
            .unwrap_or_else(|| panic!("{required} missing"));
        assert!(field(entry, "reason").contains("rf-ledger"), "{required}");
    }
    // The same functions are reachable through `windows-sys`.
    let paths: Vec<String> = entries.iter().map(|e| field(e, "path")).collect();
    for path in &paths {
        if let Some(rest) = path.strip_prefix("windows::") {
            let twin = format!("windows_sys::{rest}");
            assert!(paths.contains(&twin), "{twin} missing");
        }
    }
    assert!(
        table.contains_key("doc-valid-idents"),
        "M1-01 settings were replaced instead of appended"
    );
}

#[test]
fn only_rf_ledger_may_suppress_disallowed_methods() {
    let bad = offenders(|text| text.contains("disallowed_methods"));
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn no_os_settings_tool_is_spawned_outside_rf_ledger() {
    let spawns = |text: &str| {
        text.split("Command::new(").skip(1).any(|rest| {
            rest.strip_prefix('"')
                .and_then(|r| r.split('"').next())
                .is_some_and(|tool| TOOLS.contains(&tool))
        })
    };
    let bad = offenders(spawns);
    assert!(bad.is_empty(), "{bad:?}");
}
