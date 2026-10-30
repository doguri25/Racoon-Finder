use std::path::{Path, PathBuf};

use rf_core::identity::{BUNDLE_ID, DATA_DIR_NAME};
use rf_core::{AppPaths, BaseDirs, Error, InstallMode, Os, PathError};

struct Fake {
    _dir: tempfile::TempDir,
    root: PathBuf,
    exe: PathBuf,
    base: BaseDirs,
}

fn fake() -> Fake {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir(root.join("bin")).unwrap();
    let base = BaseDirs {
        home: Some(root.join("home")),
        app_data: Some(root.join("Roaming")),
        local_app_data: Some(root.join("Local")),
        temp: root.join("Temp"),
    };
    Fake {
        exe: root.join("bin").join("app.exe"),
        _dir: dir,
        root,
        base,
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn windows_installed_layout() {
    let f = fake();
    let p = AppPaths::resolve(Os::Windows, &f.exe, &f.base).unwrap();
    let roaming = f.root.join("Roaming").join(DATA_DIR_NAME);
    let local = f.root.join("Local").join(DATA_DIR_NAME);
    assert_eq!(p.mode(), InstallMode::Installed);
    assert_eq!(p.config_dir(), roaming);
    assert_eq!(p.data_dir(), local.join("state"));
    assert_eq!(p.cache_dir(), local.join("cache"));
    assert_eq!(p.log_dir(), local.join("logs"));
    assert_eq!(p.temp_dir(), f.root.join("Temp"));
    assert_eq!(p.preferences_file(), None);
    assert_eq!(p.owned_paths(), [roaming, local]);
}

#[test]
fn macos_layout() {
    let f = fake();
    let p = AppPaths::resolve(Os::MacOs, &f.exe, &f.base).unwrap();
    let lib = f.root.join("home").join("Library");
    let support = lib.join("Application Support").join(BUNDLE_ID);
    let caches = lib.join("Caches").join(BUNDLE_ID);
    let logs = lib.join("Logs").join(BUNDLE_ID);
    let plist = lib.join("Preferences").join(format!("{BUNDLE_ID}.plist"));
    assert_eq!(p.mode(), InstallMode::Installed);
    assert_eq!(p.config_dir(), support);
    assert_eq!(p.data_dir(), support.join("state"));
    assert_eq!(p.cache_dir(), caches);
    assert_eq!(p.log_dir(), logs);
    assert_eq!(p.preferences_file(), Some(plist.as_path()));
    assert_eq!(p.owned_paths(), [support, caches, logs, plist]);
}

#[test]
fn portable_marker_puts_everything_under_data() {
    let f = fake();
    std::fs::write(f.root.join("bin").join("portable"), b"").unwrap();
    let only_temp = BaseDirs {
        temp: f.base.temp.clone(),
        ..BaseDirs::default()
    };
    let p = AppPaths::resolve(Os::Windows, &f.exe, &only_temp).unwrap();
    let data = f.root.join("bin").join("data");
    assert_eq!(p.mode(), InstallMode::Portable);
    assert_eq!(p.config_dir(), data.join("config"));
    assert_eq!(p.data_dir(), data.join("state"));
    assert_eq!(p.cache_dir(), data.join("cache"));
    assert_eq!(p.log_dir(), data.join("logs"));
    assert_eq!(p.temp_dir(), data.join("tmp"));
    assert_eq!(p.owned_paths(), [data]);
}

#[test]
fn portable_needs_a_file_and_is_ignored_on_macos() {
    let f = fake();
    std::fs::create_dir(f.root.join("bin").join("portable")).unwrap();
    assert_eq!(
        AppPaths::resolve(Os::Windows, &f.exe, &f.base)
            .unwrap()
            .mode(),
        InstallMode::Installed
    );
    let g = fake();
    std::fs::write(g.root.join("bin").join("portable"), b"").unwrap();
    assert_eq!(
        AppPaths::resolve(Os::MacOs, &g.exe, &g.base)
            .unwrap()
            .mode(),
        InstallMode::Installed
    );
}

#[test]
fn ensure_dirs_creates_only_the_layout() {
    let f = fake();
    std::fs::write(f.root.join("bin").join("portable"), b"").unwrap();
    let p = AppPaths::resolve(Os::Windows, &f.exe, &f.base).unwrap();
    p.ensure_dirs().unwrap();
    for d in [
        p.config_dir(),
        p.data_dir(),
        p.cache_dir(),
        p.log_dir(),
        p.temp_dir(),
    ] {
        assert!(d.is_dir(), "{}", d.display());
    }
    assert_eq!(names(&f.root.join("bin")), ["data", "portable"]);
    assert_eq!(names(&f.root), ["bin"]);
}

#[test]
fn missing_or_relative_inputs_are_errors() {
    let f = fake();
    let no_appdata = BaseDirs {
        app_data: None,
        ..f.base.clone()
    };
    assert!(matches!(
        AppPaths::resolve(Os::Windows, &f.exe, &no_appdata),
        Err(Error::MissingEnv("APPDATA"))
    ));
    let no_local = BaseDirs {
        local_app_data: None,
        ..f.base.clone()
    };
    assert!(matches!(
        AppPaths::resolve(Os::Windows, &f.exe, &no_local),
        Err(Error::MissingEnv("LOCALAPPDATA"))
    ));
    let no_home = BaseDirs {
        home: None,
        ..f.base.clone()
    };
    assert!(matches!(
        AppPaths::resolve(Os::MacOs, &f.exe, &no_home),
        Err(Error::MissingEnv("HOME"))
    ));
    let rel = BaseDirs {
        app_data: Some(PathBuf::from("Roaming")),
        ..f.base.clone()
    };
    assert!(matches!(
        AppPaths::resolve(Os::Windows, &f.exe, &rel),
        Err(Error::Path(PathError::NotAbsolute(_)))
    ));
    assert!(matches!(
        AppPaths::resolve(Os::Windows, Path::new("app.exe"), &f.base),
        Err(Error::Path(PathError::NotAbsolute(_)))
    ));
}

#[test]
fn other_os_uses_home_local_share() {
    let f = fake();
    let p = AppPaths::resolve(Os::Other, &f.exe, &f.base).unwrap();
    let root = f
        .root
        .join("home")
        .join(".local")
        .join("share")
        .join(DATA_DIR_NAME);
    assert_eq!(p.config_dir(), root.join("config"));
    assert_eq!(p.owned_paths(), [root]);
}

#[test]
fn from_env_reads_temp_dir() {
    assert_eq!(BaseDirs::from_env().temp, std::env::temp_dir());
}
