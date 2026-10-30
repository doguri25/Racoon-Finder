//! The worker of the system clipboard: one thread that carries out the window's
//! [`ClipboardWork`] one request at a time, in the order the requests come, so that a paste
//! right after a copy reads that copy, and that never keeps the UI thread waiting for another
//! app that holds the clipboard. It reaches the clipboard through a [`SystemClipboard`] (the
//! system's, [`OsClipboard`], or the tests' own) and hands every answer to a function it was
//! given, as a [`Done`].

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};

use rf_core::Os;
use rf_platform::clipboard::{self, ClipboardFiles, DropEffect};

use crate::state::{ClipboardFailure, ClipboardWork, PathForm, path_text};

/// The clipboard, as the worker uses it: the functions of `rf_platform::clipboard`.
pub trait SystemClipboard: Send + Sync {
    fn put_files(&self, paths: &[PathBuf], effect: DropEffect) -> io::Result<u64>;
    fn get_files(&self) -> io::Result<Option<ClipboardFiles>>;
    fn put_text(&self, text: &str) -> io::Result<u64>;
    fn change_count(&self) -> Option<u64>;
    fn clear_if_unchanged(&self, change: u64) -> io::Result<bool>;
    fn universal_path(&self, path: &Path) -> Option<PathBuf>;
}

/// The system's clipboard.
#[derive(Clone, Copy, Debug, Default)]
pub struct OsClipboard;

impl SystemClipboard for OsClipboard {
    fn put_files(&self, paths: &[PathBuf], effect: DropEffect) -> io::Result<u64> {
        clipboard::put_files(paths, effect)
    }

    fn get_files(&self) -> io::Result<Option<ClipboardFiles>> {
        clipboard::get_files()
    }

    fn put_text(&self, text: &str) -> io::Result<u64> {
        clipboard::put_text(text)
    }

    fn change_count(&self) -> Option<u64> {
        clipboard::change_count()
    }

    fn clear_if_unchanged(&self, change: u64) -> io::Result<bool> {
        clipboard::clear_if_unchanged(change)
    }

    fn universal_path(&self, path: &Path) -> Option<PathBuf> {
        clipboard::universal_path(path)
    }
}

/// What the worker did, for the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Done {
    /// The items of request `token` are on the clipboard, which has this change count now
    /// ([`AppState::clipboard_put`](crate::state::AppState::clipboard_put)).
    Put {
        token: u64,
        result: Result<u64, ClipboardFailure>,
    },
    /// The files on the clipboard for the paste `token`
    /// ([`AppState::clipboard_read`](crate::state::AppState::clipboard_read)).
    Read {
        token: u64,
        result: Result<Option<ClipboardFiles>, ClipboardFailure>,
    },
    /// The paths are on the clipboard as text, or why not
    /// ([`AppState::paths_copied`](crate::state::AppState::paths_copied)).
    Paths(Result<(), ClipboardFailure>),
    /// The text of request `token` is on the clipboard, or why not
    /// ([`AppState::checksum_copied`](crate::state::AppState::checksum_copied)).
    Text {
        token: u64,
        result: Result<(), ClipboardFailure>,
    },
    /// The clipboard was to be emptied if its change count was still `change`: `Ok(true)` it
    /// was, `Ok(false)` something else is there now, or why it could not be
    /// ([`AppState::clipboard_cleared`](crate::state::AppState::clipboard_cleared)).
    Cleared {
        change: u64,
        result: Result<bool, ClipboardFailure>,
    },
    /// The clipboard's change count now
    /// ([`AppState::clipboard_polled`](crate::state::AppState::clipboard_polled)).
    Polled(Option<u64>),
}

/// Carries out `work` with `clipboard`, the paths written as the system `os` writes them, and
/// gives what came of it.
pub fn carry_out(work: ClipboardWork, clipboard: &dyn SystemClipboard, os: Os) -> Done {
    let failure = |error: io::Error| ClipboardFailure::from(&error);
    match work {
        ClipboardWork::PutFiles { token, paths, cut } => {
            let paths: Vec<PathBuf> = paths
                .iter()
                .filter_map(|path| path.as_local().map(Path::to_path_buf))
                .collect();
            let effect = if cut {
                DropEffect::Move
            } else {
                DropEffect::Copy
            };
            let result = clipboard.put_files(&paths, effect).map_err(failure);
            Done::Put { token, result }
        }
        ClipboardWork::ReadFiles { token } => {
            let result = clipboard.get_files().map_err(failure);
            Done::Read { token, result }
        }
        ClipboardWork::PutPaths { paths, form } => {
            let shown: Vec<String> = paths
                .iter()
                .filter_map(|path| path.as_local())
                .map(|local| {
                    let unc = (form == PathForm::Unc)
                        .then(|| clipboard.universal_path(local))
                        .flatten();
                    unc.as_deref()
                        .unwrap_or(local)
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            let text = path_text(&shown, form, os);
            let result = clipboard.put_text(&text).map(drop).map_err(failure);
            Done::Paths(result)
        }
        ClipboardWork::PutText { token, text } => {
            let result = clipboard.put_text(&text).map(drop).map_err(failure);
            Done::Text { token, result }
        }
        ClipboardWork::ClearIf(change) => {
            let result = clipboard.clear_if_unchanged(change).map_err(failure);
            Done::Cleared { change, result }
        }
    }
}

/// A request to the worker's thread.
enum Job {
    Work(ClipboardWork),
    /// Ask the change count ([`Done::Polled`]).
    Poll,
}

/// The worker. Dropping it ends its thread once the requests sent before are done.
pub struct Clipper {
    jobs: Sender<Job>,
    /// A poll is waiting on the thread: another one would only queue behind it.
    polling: Arc<AtomicBool>,
}

impl fmt::Debug for Clipper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Clipper")
            .field("polling", &self.polling.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Clipper {
    /// Starts the worker's thread with `clipboard`; it hands every answer to `done`, from that
    /// thread. When no thread can be started, every request is refused
    /// ([`Clipper::submit`] gives `false`).
    pub fn start(
        clipboard: Arc<dyn SystemClipboard>,
        os: Os,
        done: impl Fn(Done) + Send + 'static,
    ) -> Clipper {
        let (jobs, requests) = mpsc::channel::<Job>();
        let polling = Arc::new(AtomicBool::new(false));
        let waiting = Arc::clone(&polling);
        // A thread that cannot start drops the receiver with the closure: sends fail then.
        let _ = std::thread::Builder::new()
            .name("rf-ui-clipboard".to_owned())
            .spawn(move || {
                for job in requests {
                    let answer = match job {
                        Job::Work(work) => carry_out(work, &*clipboard, os),
                        Job::Poll => {
                            waiting.store(false, Ordering::Relaxed);
                            Done::Polled(clipboard.change_count())
                        }
                    };
                    done(answer);
                }
            });
        Clipper { jobs, polling }
    }

    /// Hands `work` to the worker; `false` when it has no thread.
    pub fn submit(&self, work: ClipboardWork) -> bool {
        self.jobs.send(Job::Work(work)).is_ok()
    }

    /// Asks the worker for the clipboard's change count, unless it has been asked already
    /// and not answered yet.
    pub fn poll(&self) {
        if !self.polling.swap(true, Ordering::Relaxed) && self.jobs.send(Job::Poll).is_err() {
            self.polling.store(false, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use rf_core::VPath;

    use super::*;

    /// A clipboard in memory, which notes what it is asked.
    #[derive(Default)]
    struct Fake {
        files: Mutex<Option<ClipboardFiles>>,
        text: Mutex<Option<String>>,
        change: Mutex<u64>,
        /// The paths that are on a network drive, with their share paths.
        shares: Vec<(PathBuf, PathBuf)>,
        /// Every call fails as another app holding the clipboard would make it.
        busy: bool,
    }

    impl Fake {
        fn check(&self) -> io::Result<()> {
            if self.busy {
                return Err(io::Error::new(io::ErrorKind::ResourceBusy, "held"));
            }
            Ok(())
        }

        fn bump(&self) -> u64 {
            let mut change = self.change.lock().expect("the lock");
            *change += 1;
            *change
        }
    }

    impl SystemClipboard for Fake {
        fn put_files(&self, paths: &[PathBuf], effect: DropEffect) -> io::Result<u64> {
            self.check()?;
            let change = self.bump();
            *self.files.lock().expect("the lock") = Some(ClipboardFiles {
                paths: paths.to_vec(),
                effect: Some(effect),
                change,
            });
            *self.text.lock().expect("the lock") = None;
            Ok(change)
        }

        fn get_files(&self) -> io::Result<Option<ClipboardFiles>> {
            self.check()?;
            Ok(self.files.lock().expect("the lock").clone())
        }

        fn put_text(&self, text: &str) -> io::Result<u64> {
            self.check()?;
            let change = self.bump();
            *self.files.lock().expect("the lock") = None;
            *self.text.lock().expect("the lock") = Some(text.to_owned());
            Ok(change)
        }

        fn change_count(&self) -> Option<u64> {
            Some(*self.change.lock().expect("the lock"))
        }

        fn clear_if_unchanged(&self, change: u64) -> io::Result<bool> {
            self.check()?;
            if *self.change.lock().expect("the lock") != change {
                return Ok(false);
            }
            self.bump();
            *self.files.lock().expect("the lock") = None;
            Ok(true)
        }

        fn universal_path(&self, path: &Path) -> Option<PathBuf> {
            self.shares
                .iter()
                .find(|(local, _)| local == path)
                .map(|(_, share)| share.clone())
        }
    }

    fn temp(name: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(name)).expect("a local path")
    }

    fn local(path: &VPath) -> PathBuf {
        path.as_local().expect("local").to_path_buf()
    }

    #[test]
    fn files_go_on_the_clipboard_to_be_copied_or_moved_and_read_back() {
        let fake = Fake::default();
        let paths = vec![temp("a.txt"), temp("문서 폴더")];
        let put = carry_out(
            ClipboardWork::PutFiles {
                token: 3,
                paths: paths.clone(),
                cut: true,
            },
            &fake,
            Os::CURRENT,
        );
        assert_eq!(
            put,
            Done::Put {
                token: 3,
                result: Ok(1)
            }
        );
        let read = carry_out(ClipboardWork::ReadFiles { token: 4 }, &fake, Os::CURRENT);
        let files = ClipboardFiles {
            paths: paths.iter().map(local).collect(),
            effect: Some(DropEffect::Move),
            change: 1,
        };
        assert_eq!(
            read,
            Done::Read {
                token: 4,
                result: Ok(Some(files))
            }
        );
        // A copy in their place.
        carry_out(
            ClipboardWork::PutFiles {
                token: 5,
                paths: paths[..1].to_vec(),
                cut: false,
            },
            &fake,
            Os::CURRENT,
        );
        let effect = fake
            .files
            .lock()
            .expect("the lock")
            .as_ref()
            .and_then(|files| files.effect);
        assert_eq!(effect, Some(DropEffect::Copy));
    }

    #[test]
    fn a_busy_clipboard_answers_why() {
        let fake = Fake {
            busy: true,
            ..Fake::default()
        };
        let put = ClipboardWork::PutFiles {
            token: 1,
            paths: vec![temp("a")],
            cut: false,
        };
        assert_eq!(
            carry_out(put, &fake, Os::CURRENT),
            Done::Put {
                token: 1,
                result: Err(ClipboardFailure::Unavailable)
            }
        );
        assert_eq!(
            carry_out(ClipboardWork::ReadFiles { token: 2 }, &fake, Os::CURRENT),
            Done::Read {
                token: 2,
                result: Err(ClipboardFailure::Unavailable)
            }
        );
        let paths = ClipboardWork::PutPaths {
            paths: vec![temp("a")],
            form: PathForm::Full,
        };
        assert_eq!(
            carry_out(paths, &fake, Os::CURRENT),
            Done::Paths(Err(ClipboardFailure::Unavailable))
        );
    }

    #[test]
    fn the_clipboard_is_emptied_only_while_it_holds_what_was_read() {
        let fake = Fake::default();
        let put = |token| ClipboardWork::PutFiles {
            token,
            paths: vec![temp("a")],
            cut: true,
        };
        carry_out(put(1), &fake, Os::CURRENT);
        carry_out(put(2), &fake, Os::CURRENT);
        // The first put's count is not the clipboard's any more: it keeps what came since.
        assert_eq!(
            carry_out(ClipboardWork::ClearIf(1), &fake, Os::CURRENT),
            Done::Cleared {
                change: 1,
                result: Ok(false)
            }
        );
        assert!(fake.files.lock().expect("the lock").is_some());
        assert_eq!(
            carry_out(ClipboardWork::ClearIf(2), &fake, Os::CURRENT),
            Done::Cleared {
                change: 2,
                result: Ok(true)
            }
        );
        assert!(fake.files.lock().expect("the lock").is_none());
        // One that cannot be emptied says why, for the window to try again.
        let busy = Fake {
            busy: true,
            ..Fake::default()
        };
        assert_eq!(
            carry_out(ClipboardWork::ClearIf(0), &busy, Os::CURRENT),
            Done::Cleared {
                change: 0,
                result: Err(ClipboardFailure::Unavailable)
            }
        );
    }

    #[test]
    fn paths_go_as_text_and_a_mapped_drive_through_its_share() {
        let mapped = temp("mapped.txt");
        let other = temp("다른 파일.txt");
        let share = PathBuf::from("\\\\server\\share\\mapped.txt");
        let fake = Fake {
            shares: vec![(local(&mapped), share.clone())],
            ..Fake::default()
        };
        let os = Os::CURRENT;
        let shown = |path: &VPath| local(path).to_string_lossy().into_owned();
        for form in PathForm::ALL {
            let work = ClipboardWork::PutPaths {
                paths: vec![mapped.clone(), other.clone()],
                form,
            };
            assert_eq!(carry_out(work, &fake, os), Done::Paths(Ok(())), "{form:?}");
            let first = if form == PathForm::Unc {
                share.to_string_lossy().into_owned()
            } else {
                shown(&mapped)
            };
            let wanted = path_text(&[first, shown(&other)], form, os);
            assert_eq!(
                fake.text.lock().expect("the lock").as_deref(),
                Some(wanted.as_str()),
                "{form:?}"
            );
        }
    }

    #[test]
    fn the_worker_answers_in_the_order_asked_and_polls_once_at_a_time() {
        let fake: Arc<dyn SystemClipboard> = Arc::new(Fake::default());
        let (answers, done) = mpsc::channel();
        let clipper = Clipper::start(Arc::clone(&fake), Os::CURRENT, move |answer| {
            let _ = answers.send(answer);
        });
        assert!(clipper.submit(ClipboardWork::PutFiles {
            token: 1,
            paths: vec![temp("a")],
            cut: true,
        }));
        assert!(clipper.submit(ClipboardWork::ReadFiles { token: 2 }));
        clipper.poll();
        let first = done.recv().expect("an answer");
        let second = done.recv().expect("an answer");
        let third = done.recv().expect("an answer");
        assert_eq!(
            first,
            Done::Put {
                token: 1,
                result: Ok(1)
            }
        );
        // The read came after the put, and found it.
        assert!(
            matches!(&second, Done::Read { token: 2, result: Ok(Some(files)) } if files.change == 1),
            "{second:?}"
        );
        assert_eq!(third, Done::Polled(Some(1)));
        drop(clipper);
        // Once the worker is gone nothing more comes.
        assert!(done.recv().is_err());
    }
}
