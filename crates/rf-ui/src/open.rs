//! Opening a file with the app the system picks for it, and the system's other ways to open an
//! item ([`shell`]): 연결 프로그램, a program run as administrator, a terminal, the system's
//! properties window.
//!
//! Every call here goes to a worker thread of its own and returns at once: the system's calls
//! block (a dialog stays until the user closes it, a program takes its time to start), and none
//! of them needs the main thread, so the UI thread never waits for one.

use std::io;
use std::path::Path;
use std::sync::Arc;

use rf_core::{Error, ErrorKind, VPath};
use rf_platform::shell::{self as system, App, OpenWith};

use crate::state::ShellWork;
use crate::words::ShellAction;

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

/// What a worker of [`shell`] has for the UI thread.
#[derive(Debug)]
pub enum ShellNews {
    /// The apps that can open the file of the 연결 프로그램 request `token` (macOS), or why they
    /// are not known.
    Apps {
        token: u64,
        found: Result<Vec<App>, ErrorKind>,
    },
    /// The system did not do what was asked.
    Failed(ShellAction, ErrorKind),
}

/// Asks the system for `work` on a worker thread of its own and returns at once; `on_news` is
/// called there with what the UI thread is to know, if anything (or here, when no thread could
/// be started). A choice the user cancels is no news.
pub fn shell(work: ShellWork, on_news: impl Fn(ShellNews) + Send + Sync + 'static) {
    let action = work.action();
    let on_news = Arc::new(on_news);
    let worker_news = Arc::clone(&on_news);
    let spawned = std::thread::Builder::new()
        .name("rf-ui-shell".into())
        .spawn(move || {
            if let Some(news) = ask_system(work) {
                worker_news(news);
            }
        });
    if spawned.is_err() {
        on_news(ShellNews::Failed(action, ErrorKind::Other));
    }
}

/// Does `work` and waits for the system: what the UI thread is to know of it, if anything.
fn ask_system(work: ShellWork) -> Option<ShellNews> {
    let action = work.action();
    // The plain local path, which the system's calls want; none for another location.
    let local = |path: &VPath| path.as_local().map(Path::to_path_buf);
    let failed = |error: io::Error, path: &VPath| {
        ShellNews::Failed(
            action,
            Error::io("ask the system", path.clone(), error).kind(),
        )
    };
    let done = |result: io::Result<()>, path: &VPath| result.err().map(|error| failed(error, path));
    let Some(path) = (match &work {
        ShellWork::OpenWith { file, .. } | ShellWork::OpenWithApp { file, .. } => local(file),
        ShellWork::RunAsAdmin(path)
        | ShellWork::Terminal(path)
        | ShellWork::SystemProperties(path) => local(path),
    }) else {
        return Some(ShellNews::Failed(action, ErrorKind::Unsupported));
    };
    match work {
        ShellWork::OpenWith { token, file } => match system::OPEN_WITH {
            OpenWith::Apps => Some(ShellNews::Apps {
                token,
                found: system::apps_for(&path)
                    .map_err(|error| Error::io("list the apps", file.clone(), error).kind()),
            }),
            OpenWith::Dialog => done(system::open_with_dialog(&path), &file),
            OpenWith::Unsupported => Some(ShellNews::Failed(action, ErrorKind::Unsupported)),
        },
        ShellWork::OpenWithApp { file, app } => done(system::open_with_app(&path, &app), &file),
        ShellWork::RunAsAdmin(file) => done(system::run_as_admin(&path), &file),
        ShellWork::Terminal(folder) => done(system::open_terminal(&path), &folder),
        ShellWork::SystemProperties(item) => done(system::show_properties(&path), &item),
    }
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

    /// What `work` comes back with on Linux.
    fn news_of(work: ShellWork) -> ShellNews {
        let (tx, rx) = mpsc::channel();
        shell(work, move |news| {
            let _ = tx.send(news);
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("news comes back")
    }

    // Linux opens a terminal and nothing else: the rest says it is not supported here.
    #[test]
    fn linux_says_what_it_does_not_support() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let file = VPath::local(dir.path().join("메모.txt")).expect("a local path");
        let program = VPath::local(dir.path().join("setup.exe")).expect("a local path");
        for (work, action) in [
            (
                ShellWork::OpenWith {
                    token: 1,
                    file: file.clone(),
                },
                ShellAction::OpenWith,
            ),
            (
                ShellWork::SystemProperties(file.clone()),
                ShellAction::SystemProperties,
            ),
            (
                ShellWork::RunAsAdmin(program.clone()),
                ShellAction::RunAsAdmin,
            ),
        ] {
            match news_of(work) {
                ShellNews::Failed(failed, ErrorKind::Unsupported) => assert_eq!(failed, action),
                other => panic!("{action:?}: {other:?}"),
            }
        }
        // A terminal in a folder that is gone.
        let gone = VPath::local(dir.path().join("없는 폴더")).expect("a local path");
        match news_of(ShellWork::Terminal(gone)) {
            ShellNews::Failed(ShellAction::Terminal, ErrorKind::NotFound) => {}
            other => panic!("{other:?}"),
        }
    }
}
