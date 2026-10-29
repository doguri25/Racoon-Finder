//! One app process at a time: the lock that keeps a second one from opening the same job
//! journal (carry-m1-03 "single-instance journal guard").
//!
//! The lock is a file, [`LOCK_FILE`], in the folder of the journal, held with the system's
//! file lock (`File::try_lock`) for as long as the [`InstanceLock`] lives. The system lets go
//! of it when the process ends, however it ends, so a crash never leaves the app locked out.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

/// The name of the lock file in the journal's folder.
pub const LOCK_FILE: &str = "instance.lock";

/// The single-instance lock. Dropping it lets another process take it.
#[derive(Debug)]
pub struct InstanceLock {
    /// Held open: closing it ends the lock.
    _file: File,
    path: PathBuf,
}

/// Why [`InstanceLock::acquire`] did not get the lock.
#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
    /// Another process (or another lock in this one) holds it.
    #[error("another instance holds {0}")]
    AlreadyRunning(PathBuf),
    /// The folder or the lock file could not be made or opened, or the system would not lock
    /// it.
    #[error("instance lock {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl InstanceLock {
    /// Takes the lock in the folder `dir`, which is made if it is missing. Never waits: a lock
    /// another process holds is [`InstanceError::AlreadyRunning`] at once.
    pub fn acquire(dir: &Path) -> Result<InstanceLock, InstanceError> {
        let path = dir.join(LOCK_FILE);
        let io = |source| InstanceError::Io {
            path: path.clone(),
            source,
        };
        std::fs::create_dir_all(dir).map_err(io)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(io)?;
        match file.try_lock() {
            Ok(()) => Ok(InstanceLock { _file: file, path }),
            Err(TryLockError::WouldBlock) => Err(InstanceError::AlreadyRunning(path)),
            Err(TryLockError::Error(source)) => Err(io(source)),
        }
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_instance_is_refused() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let journal = dir.path().join("data");
        let first = InstanceLock::acquire(&journal).expect("the first instance gets the lock");
        assert_eq!(first.path(), journal.join(LOCK_FILE));
        assert!(first.path().is_file());

        // A second open of the same lock file, as a second process would make, is refused at
        // once and leaves the first lock as it is.
        match InstanceLock::acquire(&journal) {
            Err(InstanceError::AlreadyRunning(path)) => assert_eq!(path, journal.join(LOCK_FILE)),
            other => panic!("the second instance must be refused: {other:?}"),
        }

        // Once the first instance is gone, the next one starts.
        drop(first);
        let next = InstanceLock::acquire(&journal).expect("the lock is free again");
        drop(next);
    }

    #[test]
    fn a_folder_that_cannot_be_made_is_an_io_error() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let file = dir.path().join("not-a-folder");
        std::fs::write(&file, b"x").expect("a file");
        match InstanceLock::acquire(&file.join("data")) {
            Err(InstanceError::Io { .. }) => {}
            other => panic!("a file in the way is an I/O error: {other:?}"),
        }
    }
}
