#![allow(dead_code, reason = "each test file uses a different part")]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rf_core::VPath;
use rf_core::identity::TEMP_PREFIX;
use rf_jobs::{
    ExecEnv, JobId, JobReport, JobSpec, NonInteractive, Resolver, TransferMode, TransferOptions,
    TransferSpec, execute,
};
use rf_vfs::Vfs;

pub struct Fx {
    _t: tempfile::TempDir,
    pub root: PathBuf,
    pub src: PathBuf,
    pub dst: PathBuf,
}

pub fn fx() -> Fx {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().to_path_buf();
    let (src, dst) = (root.join("src"), root.join("dst"));
    std::fs::create_dir(&src).unwrap();
    std::fs::create_dir(&dst).unwrap();
    Fx {
        _t: t,
        root,
        src,
        dst,
    }
}

pub fn v(p: impl AsRef<Path>) -> VPath {
    VPath::local(p.as_ref()).unwrap()
}

pub fn spec(
    mode: TransferMode,
    sources: &[PathBuf],
    dest: &Path,
    options: TransferOptions,
) -> JobSpec {
    let sources: Vec<VPath> = sources.iter().map(v).collect();
    JobSpec::Transfer(TransferSpec::into_dir(mode, &sources, &v(dest), options).unwrap())
}

pub fn copy(sources: &[PathBuf], dest: &Path) -> JobSpec {
    spec(
        TransferMode::Copy,
        sources,
        dest,
        TransferOptions::default(),
    )
}

pub fn run(job: &JobSpec) -> JobReport {
    run_on(job, &Vfs::with_local(), &NonInteractive)
}

pub fn run_on(job: &JobSpec, vfs: &Vfs, resolver: &dyn Resolver) -> JobReport {
    execute(JobId::generate(), job, &ExecEnv::new(vfs, resolver))
}

pub fn pattern(len: u64) -> Vec<u8> {
    (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect()
}

pub fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

pub fn set_mtime(p: &Path, secs: u64) {
    rf_platform::set_times(&v(p), Some(at(secs)), None, None).unwrap();
}

pub fn mtime(p: &Path) -> SystemTime {
    std::fs::metadata(p).unwrap().modified().unwrap()
}

/// A name no other test run uses, for an item a test puts in the real trash: it starts with
/// `TEMP_PREFIX`, so a leftover is easy to tell from the user's own items.
pub fn unique_name(suffix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{TEMP_PREFIX}{}-{nanos}-{suffix}", std::process::id())
}

/// Removes a trashed test item from the real trash (and its `$I` record on Windows).
#[cfg(any(windows, target_os = "macos"))]
pub fn purge(location: &Path) {
    let item = v(location).to_extended_length().unwrap();
    if item.is_dir() {
        std::fs::remove_dir_all(&item).unwrap();
    } else {
        std::fs::remove_file(&item).unwrap();
    }
    #[cfg(windows)]
    {
        let name = location.file_name().unwrap().to_string_lossy().into_owned();
        if let Some(rest) = name.strip_prefix("$R") {
            let _ = std::fs::remove_file(location.with_file_name(format!("$I{rest}")));
        }
    }
}
