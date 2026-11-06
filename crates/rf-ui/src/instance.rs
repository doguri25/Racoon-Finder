//! One app process at a time: the lock that keeps a second one from opening the same job
//! journal (carry-m1-03 "single-instance journal guard").
//!
//! The lock is a file, [`LOCK_FILE`], in the folder of the journal, held with the system's
//! file lock (`File::try_lock`) for as long as the [`InstanceLock`] lives. The system lets go
//! of it when the process ends, however it ends, so a crash never leaves the app locked out.
//!
//! A second start of the app asks the one that runs to show its window (it may be hidden
//! behind the tray icon, XFER-15) and ends quietly: it puts a request file, [`SHOW_REQUEST`],
//! beside the lock ([`ask_to_show`]); the running app looks for it every [`SHOW_POLL`] on a
//! thread of its own ([`InstanceLock::watch_requests`]), takes it away and shows its window.
//! When nothing takes the request within [`SHOW_WAIT`] (an app that hangs, an older one, or one
//! that is ending and takes no more requests, [`RequestStop`]), the second start takes it back
//! and goes on trying the lock for the wait it is given (the app's is [`crate::QUIT_WAIT`]):
//! an app that is ending lets it go once it has saved its settings and stopped its jobs. Only
//! then does it say that the app runs, as it did before ([`take_or_ask`]). A request left over
//! from an earlier run goes when the lock is taken.
//!
//! The clean removal (`--cleanup`, INST-04) asks the same way for the app to end:
//! it puts [`QUIT_REQUEST`] beside the lock ([`ask_to_quit`]), the running app takes it and ends
//! as its close button would (with its question when jobs run), and the removal waits for the
//! lock to be free ([`take_after_quit`]).

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The name of the lock file in the journal's folder.
pub const LOCK_FILE: &str = "instance.lock";

/// The name of the file a second start of the app puts beside the lock to have the running one
/// show its window.
pub const SHOW_REQUEST: &str = "show.request";

/// The name of the file the clean removal puts beside the lock to have the running app end.
pub const QUIT_REQUEST: &str = "quit.request";

/// How often the running app looks for a request to show its window or to end.
pub const SHOW_POLL: Duration = Duration::from_millis(250);

/// How long a second start waits for the running app to take its request.
pub const SHOW_WAIT: Duration = Duration::from_secs(3);

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
            Ok(()) => {
                // A request of a start that came while the last run was ending.
                let _ = std::fs::remove_file(dir.join(SHOW_REQUEST));
                let _ = std::fs::remove_file(dir.join(QUIT_REQUEST));
                Ok(InstanceLock { _file: file, path })
            }
            Err(TryLockError::WouldBlock) => Err(InstanceError::AlreadyRunning(path)),
            Err(TryLockError::Error(source)) => Err(io(source)),
        }
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Looks every [`SHOW_POLL`], on a thread of its own, for a second start's request to show
    /// the window ([`ask_to_show`]) and for the clean removal's request to end ([`ask_to_quit`]);
    /// takes each one away and calls `on_show` or `on_quit`. The look stops when the
    /// [`RequestWatch`] is dropped, and takes no request once its [`RequestStop`] has stopped
    /// it.
    pub fn watch_requests(
        &self,
        on_show: impl Fn() + Send + 'static,
        on_quit: impl Fn() + Send + 'static,
    ) -> std::io::Result<RequestWatch> {
        let dir = self
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let (show, quit) = (dir.join(SHOW_REQUEST), dir.join(QUIT_REQUEST));
        let stop = RequestStop(Arc::new(Mutex::new(false)));
        let stopped = stop.clone();
        let thread = std::thread::Builder::new()
            .name("rf-ui-instance".to_owned())
            .spawn(move || {
                loop {
                    std::thread::sleep(SHOW_POLL);
                    // Held while a request is taken: once `RequestStop::stop` has returned, none
                    // is.
                    let flag = stopped.0.lock().unwrap_or_else(PoisonError::into_inner);
                    if *flag {
                        return;
                    }
                    // Only the one that takes the file away answers it.
                    let shown = std::fs::remove_file(&show).is_ok();
                    let ended = std::fs::remove_file(&quit).is_ok();
                    drop(flag);
                    if shown {
                        on_show();
                    }
                    if ended {
                        on_quit();
                    }
                }
            })?;
        Ok(RequestWatch {
            stop,
            thread: Some(thread),
        })
    }
}

/// The look for requests to show the window ([`InstanceLock::watch_requests`]); dropping it
/// stops the look, within a [`SHOW_POLL`].
#[derive(Debug)]
pub struct RequestWatch {
    stop: RequestStop,
    thread: Option<JoinHandle<()>>,
}

impl RequestWatch {
    /// What stops the look from another place, without waiting for its thread: the app's end.
    pub fn stopper(&self) -> RequestStop {
        self.stop.clone()
    }
}

impl Drop for RequestWatch {
    fn drop(&mut self) {
        self.stop.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Stops a [`RequestWatch`] from taking requests. The app's end stops it as it begins: an app
/// that is ending cannot show its window to a second start, which then finds out by itself
/// ([`take_or_ask`]). Cloning is cheap.
#[derive(Clone, Debug)]
pub struct RequestStop(Arc<Mutex<bool>>);

impl RequestStop {
    /// No request is taken from now on. Waits only while the look takes one away.
    pub fn stop(&self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = true;
    }
}

/// How a start of the app with the lock folder `dir` goes ([`take_or_ask`]).
#[derive(Debug)]
pub enum Start {
    /// This process has the lock: the app starts.
    Lock(InstanceLock),
    /// The app that runs took the request to show its window.
    Shown,
    /// The app that runs neither took the request nor ended meanwhile.
    Running,
}

/// Takes the lock in `dir`, or, while another process holds it, asks that one to show its
/// window with `ask` (the app's is [`ask_to_show`] with [`SHOW_WAIT`]: `true` once it was
/// taken). A request nobody took may mean the app that holds the lock is ending, which takes no
/// more requests and lets the lock go only at the end of its end: the lock is tried every
/// [`SHOW_POLL`] for `wait` (the app's is [`crate::QUIT_WAIT`]) before the app is said to run.
pub fn take_or_ask(
    dir: &Path,
    ask: impl FnOnce(&Path) -> bool,
    wait: Duration,
) -> Result<Start, InstanceError> {
    match InstanceLock::acquire(dir) {
        Ok(lock) => return Ok(Start::Lock(lock)),
        Err(InstanceError::AlreadyRunning(_)) => {}
        Err(error) => return Err(error),
    }
    if ask(dir) {
        return Ok(Start::Shown);
    }
    let deadline = Instant::now() + wait;
    loop {
        match InstanceLock::acquire(dir) {
            Ok(lock) => return Ok(Start::Lock(lock)),
            Err(InstanceError::AlreadyRunning(_)) if Instant::now() < deadline => {
                std::thread::sleep(SHOW_POLL);
            }
            Err(InstanceError::AlreadyRunning(_)) => return Ok(Start::Running),
            Err(error) => return Err(error),
        }
    }
}

/// Asks the app that holds the lock in the folder `dir` to show its window, and waits up to
/// `wait` for it to take the request: `true` once it has. A request nobody took is taken back,
/// so that it does not show the window of the next run at a moment nobody asked for.
pub fn ask_to_show(dir: &Path, wait: Duration) -> bool {
    ask(dir, SHOW_REQUEST, b"show", wait)
}

/// Asks the app that holds the lock in the folder `dir` to end, and waits up to `wait` for it to
/// take the request: `true` once it has. Taking it is not ending: the app ends as its close
/// button would, with its question when jobs run, and lets the lock go at the end of its end
/// ([`take_after_quit`]). A request nobody took is taken back.
pub fn ask_to_quit(dir: &Path, wait: Duration) -> bool {
    ask(dir, QUIT_REQUEST, b"quit", wait)
}

fn ask(dir: &Path, name: &str, content: &[u8], wait: Duration) -> bool {
    let request = dir.join(name);
    if std::fs::write(&request, content).is_err() {
        return false;
    }
    let deadline = Instant::now() + wait;
    loop {
        if !request.exists() {
            return true;
        }
        if Instant::now() >= deadline {
            // Taken meanwhile after all, if it is gone already.
            return std::fs::remove_file(&request).is_err() && !request.exists();
        }
        std::thread::sleep(SHOW_POLL / 5);
    }
}

/// How the clean removal gets the lock ([`take_after_quit`]).
#[derive(Debug)]
pub enum Quit {
    /// No app runs (any more): this process has the lock.
    Lock(InstanceLock),
    /// An app still holds the lock after the wait.
    Running,
}

/// Takes the lock in `dir`, or, while another process holds it, has that one end with `ask`
/// (the clean removal's is [`ask_to_quit`]) and tries the lock every [`SHOW_POLL`] for `wait` in
/// all, counted from the call: the app ends once its jobs have, or its user has answered its
/// question, which takes a while. Whether `ask` was answered does not matter: an app that is
/// ending already takes no request, and lets the lock go all the same.
pub fn take_after_quit(
    dir: &Path,
    ask: impl FnOnce(&Path),
    wait: Duration,
) -> Result<Quit, InstanceError> {
    let deadline = Instant::now() + wait;
    match InstanceLock::acquire(dir) {
        Ok(lock) => return Ok(Quit::Lock(lock)),
        Err(InstanceError::AlreadyRunning(_)) => {}
        Err(error) => return Err(error),
    }
    ask(dir);
    loop {
        match InstanceLock::acquire(dir) {
            Ok(lock) => return Ok(Quit::Lock(lock)),
            Err(InstanceError::AlreadyRunning(_)) if Instant::now() < deadline => {
                std::thread::sleep(SHOW_POLL);
            }
            Err(InstanceError::AlreadyRunning(_)) => return Ok(Quit::Running),
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long the tests' second starts go on trying the lock: long enough for an app that ends
    /// a moment later.
    const WAIT: Duration = Duration::from_secs(10);

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
    fn a_second_start_has_the_running_app_show_its_window() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let journal = dir.path().join("data");
        let running = InstanceLock::acquire(&journal).expect("the running app's lock");
        let (shown, asked) = std::sync::mpsc::channel();
        let watch = running
            .watch_requests(
                move || {
                    let _ = shown.send(());
                },
                || {},
            )
            .expect("the look starts");
        // The second start's lock is refused; its request is taken, and answered.
        assert!(matches!(
            InstanceLock::acquire(&journal),
            Err(InstanceError::AlreadyRunning(_))
        ));
        assert!(ask_to_show(&journal, Duration::from_secs(10)));
        asked
            .recv_timeout(Duration::from_secs(10))
            .expect("the running app shows its window");
        assert!(!journal.join(SHOW_REQUEST).exists());
        // Each request once: a second one is answered again.
        assert!(ask_to_show(&journal, Duration::from_secs(10)));
        asked
            .recv_timeout(Duration::from_secs(10))
            .expect("shown again");
        // Once the look stops, nobody answers: the request is taken back.
        drop(watch);
        assert!(!ask_to_show(&journal, Duration::from_millis(300)));
        assert!(!journal.join(SHOW_REQUEST).exists());
        assert_eq!(
            asked.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        );
    }

    #[test]
    fn an_app_that_is_ending_takes_no_request_to_show() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let journal = dir.path().join("data");
        let running = InstanceLock::acquire(&journal).expect("the running app's lock");
        let (shown, asked) = std::sync::mpsc::channel();
        let watch = running
            .watch_requests(
                move || {
                    let _ = shown.send(());
                },
                || {},
            )
            .expect("the look starts");
        // The app's end begins: the look stops taking requests, though it is not dropped yet.
        watch.stopper().stop();
        assert!(!ask_to_show(&journal, Duration::from_millis(800)));
        assert!(!journal.join(SHOW_REQUEST).exists());
        drop(watch);
        assert_eq!(
            asked.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        );
    }

    #[test]
    fn a_second_start_shows_the_app_that_runs_or_takes_the_lock_of_one_that_ended() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let journal = dir.path().join("data");
        // Nobody runs: the lock.
        let first = match take_or_ask(&journal, |_| panic!("nobody to ask"), WAIT) {
            Ok(Start::Lock(lock)) => lock,
            other => panic!("the lock, not {other:?}"),
        };
        // It runs and takes the request: shown.
        assert!(matches!(
            take_or_ask(&journal, |_| true, WAIT),
            Ok(Start::Shown)
        ));
        // It runs and does not take it, and still holds the lock once the wait is over: it
        // runs.
        assert!(matches!(
            take_or_ask(&journal, |_| false, Duration::ZERO),
            Ok(Start::Running)
        ));
        // It ended while it was asked (an app that is ending takes no request): the lock.
        let mut held = Some(first);
        let lock = match take_or_ask(
            &journal,
            |_| {
                drop(held.take());
                false
            },
            Duration::ZERO,
        ) {
            Ok(Start::Lock(lock)) => lock,
            other => panic!("the lock, not {other:?}"),
        };
        assert_eq!(lock.path(), journal.join(LOCK_FILE));
    }

    #[test]
    fn a_second_start_waits_out_an_app_that_ends_slowly() {
        // The app that runs is ending: it took no request, and lets the lock go only once it
        // has saved its settings and stopped its jobs, a while after the request was given up.
        let dir = tempfile::tempdir().expect("a temporary folder");
        let journal = dir.path().join("data");
        let first = InstanceLock::acquire(&journal).expect("the lock");
        let (asked, ending) = std::sync::mpsc::channel::<()>();
        let quit = std::thread::spawn(move || {
            ending.recv().expect("the request was given up");
            std::thread::sleep(Duration::from_millis(300));
            drop(first);
        });
        let start = take_or_ask(
            &journal,
            |_| {
                asked.send(()).expect("the ending app");
                false
            },
            WAIT,
        );
        quit.join().expect("the ending app");
        assert!(matches!(start, Ok(Start::Lock(_))), "{start:?}");
    }

    #[test]
    fn the_clean_removal_has_the_running_app_end_and_takes_the_lock_it_lets_go() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let journal = dir.path().join("data");
        // The running app: it takes the request to end, shows nothing, and lets the lock go a
        // moment after.
        let running = InstanceLock::acquire(&journal).expect("the running app's lock");
        let (quit, asked) = std::sync::mpsc::channel::<()>();
        let watch = running
            .watch_requests(
                || panic!("nobody asked to show the window"),
                move || {
                    let _ = quit.send(());
                },
            )
            .expect("the look starts");
        let ending = std::thread::spawn(move || {
            asked
                .recv_timeout(Duration::from_secs(10))
                .expect("asked to end");
            std::thread::sleep(Duration::from_millis(300));
            drop(watch);
            drop(running);
        });
        let start = take_after_quit(
            &journal,
            |dir| assert!(ask_to_quit(dir, Duration::from_secs(10))),
            WAIT,
        );
        ending.join().expect("the app ended");
        assert!(matches!(start, Ok(Quit::Lock(_))), "{start:?}");
        assert!(!journal.join(QUIT_REQUEST).exists());
    }

    #[test]
    fn the_clean_removal_waits_the_time_it_is_given_for_an_app_that_does_not_end() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let journal = dir.path().join("data");
        let running = InstanceLock::acquire(&journal).expect("the running app's lock");
        let (quit, asked) = std::sync::mpsc::channel::<()>();
        let watch = running
            .watch_requests(
                || {},
                move || {
                    let _ = quit.send(());
                },
            )
            .expect("the look starts");
        // With no lock to be had, the removal gives up when its time is over, and an app that
        // asks its user about running jobs has taken the request by then.
        let started = Instant::now();
        let start = take_after_quit(
            &journal,
            |dir| assert!(ask_to_quit(dir, Duration::from_secs(10))),
            Duration::from_millis(900),
        );
        assert!(matches!(start, Ok(Quit::Running)), "{start:?}");
        assert!(started.elapsed() >= Duration::from_millis(900));
        asked.try_recv().expect("the request was taken");
        // Nothing runs: the lock at once, and nobody is asked.
        drop(watch);
        drop(running);
        let start = take_after_quit(&journal, |_| panic!("nobody to ask"), WAIT);
        assert!(matches!(start, Ok(Quit::Lock(_))), "{start:?}");
    }

    #[test]
    fn an_app_that_is_ending_takes_no_request_to_end_and_a_stale_one_goes_with_the_lock() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let journal = dir.path().join("data");
        let running = InstanceLock::acquire(&journal).expect("the running app's lock");
        let (quit, asked) = std::sync::mpsc::channel::<()>();
        let watch = running
            .watch_requests(
                || {},
                move || {
                    let _ = quit.send(());
                },
            )
            .expect("the look starts");
        watch.stopper().stop();
        assert!(!ask_to_quit(&journal, Duration::from_millis(800)));
        assert!(!journal.join(QUIT_REQUEST).exists());
        drop(watch);
        assert_eq!(
            asked.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        );
        drop(running);
        // A request left from an earlier run is not the next run's to answer.
        std::fs::write(journal.join(QUIT_REQUEST), b"quit").expect("left over");
        let lock = InstanceLock::acquire(&journal).expect("the lock");
        assert!(!journal.join(QUIT_REQUEST).exists());
        drop(lock);
    }

    #[test]
    fn a_request_left_from_an_earlier_run_goes_with_the_lock() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let journal = dir.path().join("data");
        std::fs::create_dir_all(&journal).expect("the folder");
        std::fs::write(journal.join(SHOW_REQUEST), b"show").expect("a request left over");
        let lock = InstanceLock::acquire(&journal).expect("the lock");
        assert!(!journal.join(SHOW_REQUEST).exists());
        drop(lock);
        // Without a folder there is nobody to ask.
        assert!(!ask_to_show(
            &dir.path().join("no folder"),
            Duration::from_millis(100)
        ));
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
