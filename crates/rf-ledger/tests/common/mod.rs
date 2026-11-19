#![allow(
    dead_code,
    clippy::unnecessary_wraps,
    reason = "each test binary uses a different subset; `s` mirrors `Change::value`"
)]

use std::path::{Path, PathBuf};

use rf_core::{AppPaths, BaseDirs, Os};
use rf_ledger::{Change, FakeBackend, Ledger, Target, Value};

pub struct Fx {
    pub dir: tempfile::TempDir,
    pub paths: AppPaths,
    pub fake: FakeBackend,
}

impl Fx {
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn open(&self) -> Ledger {
        Ledger::open(&self.paths, Box::new(self.fake.clone()))
            .unwrap()
            .0
    }
}

fn fixture(os: Os, portable: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::create_dir_all(root.join("Temp")).unwrap();
    if portable {
        std::fs::write(
            root.join("bin").join(rf_core::identity::PORTABLE_MARKER),
            b"",
        )
        .unwrap();
    }
    let base = BaseDirs {
        home: Some(root.join("home")),
        app_data: Some(root.join("Roaming")),
        local_app_data: Some(root.join("Local")),
        temp: root.join("Temp"),
    };
    let paths = AppPaths::resolve(os, &root.join("bin").join("app.exe"), &base).unwrap();
    Fx {
        dir,
        paths,
        fake: FakeBackend::new(),
    }
}

/// Windows installed layout under a tempdir (pure path logic, works on every host).
pub fn installed() -> Fx {
    fixture(Os::Windows, false)
}

pub fn portable() -> Fx {
    fixture(Os::Windows, true)
}

pub fn macos() -> Fx {
    fixture(Os::MacOs, false)
}

/// Registry slot `HKCU\Software\T<n>`, value name `v`.
pub fn slot(n: u32) -> Target {
    Target::Registry {
        key: format!(r"HKCU\Software\T{n}"),
        name: "v".into(),
    }
}

pub fn set(target: &Target, v: &str) -> Change {
    Change {
        target: target.clone(),
        value: s(v),
    }
}

pub fn s(v: &str) -> Option<Value> {
    Some(Value::Str(v.into()))
}

/// Every file (not directory) below `dir`, sorted.
pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}
