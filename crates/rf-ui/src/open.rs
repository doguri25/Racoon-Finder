//! Opening a file with the app the system picks for it.

use std::sync::Arc;

use rf_core::{Error, ErrorKind, VPath};

/// Opens the file at `path` with the system's default app and returns at once. The system
/// call can block (on Windows while the shell starts the app), so it runs on a worker thread;
/// `on_error` is called there with what went wrong, if anything did (or here, when no thread
/// could be started).
pub fn open_file(path: VPath, on_error: impl Fn(ErrorKind) + Send + Sync + 'static) {
    let on_error = Arc::new(on_error);
    let worker_error = Arc::clone(&on_error);
    let worker_path = path.clone();
    let spawned = std::thread::Builder::new()
        .name("rf-ui-open".into())
        .spawn(move || {
            if let Err(kind) = open_now(&worker_path) {
                worker_error(kind);
            }
        });
    if let Err(error) = spawned {
        on_error(Error::io("start a thread to open a file", path, error).kind());
    }
}

/// Opens `path` and waits for the system to take it.
fn open_now(path: &VPath) -> Result<(), ErrorKind> {
    // Shell APIs want the plain local path, not the extended-length form.
    let Some(local) = path.as_local() else {
        return Err(ErrorKind::Unsupported);
    };
    opener::open(local).map_err(|error| match error {
        opener::OpenError::Io(source) | opener::OpenError::Spawn { source, .. } => {
            Error::io("open with the default app", path.clone(), source).kind()
        }
        // The opener ran and failed, or something added after this was written.
        _ => ErrorKind::Other,
    })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    // Only on Linux does the opener check the path before handing it to the system;
    // elsewhere the system's own error report (a dialog on Windows) would come instead.
    #[test]
    fn a_missing_file_reports_not_found() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let missing = VPath::local(dir.path().join("nothing-here.txt")).expect("a local path");
        let (tx, rx) = mpsc::channel();
        open_file(missing, move |kind| {
            let _ = tx.send(kind);
        });
        let kind = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the error comes back");
        assert_eq!(kind, ErrorKind::NotFound);
    }
}
