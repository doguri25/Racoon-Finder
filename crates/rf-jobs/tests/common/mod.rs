#![allow(dead_code, reason = "each test file uses a different part")]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rf_core::VPath;
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
