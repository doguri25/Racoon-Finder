//! The worker that matches the names of the pattern window's rows ([`Effect::MatchNames`]):
//! one thread, which keeps only the newest request. A request that comes while another one runs
//! stops that one (it looks every thousand names or so), and takes the place of one that waits,
//! so a pattern typed key by key over 100,000 names matches only the text as it is.
//!
//! [`Effect::MatchNames`]: crate::state::Effect::MatchNames

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use crate::state::{MatchRequest, Matched};

/// Hands the requests to the worker, which gives what it found to `deliver` on its own thread.
/// The worker starts with the first request and ends when the matcher goes.
pub struct Matcher {
    shared: Arc<Shared>,
    deliver: Arc<dyn Fn(Matched) + Send + Sync>,
}

struct Shared {
    slot: Mutex<Slot>,
    wake: Condvar,
    /// The generation of the newest request (0 once the matcher is gone): one under way stops
    /// when it is no longer this.
    newest: AtomicU64,
}

#[derive(Default)]
struct Slot {
    /// The request that waits for the worker: the newest one, if it has not started.
    waiting: Option<MatchRequest>,
    /// The worker runs.
    started: bool,
    /// The matcher is gone: the worker ends.
    closed: bool,
}

/// The worker could not be started (the system has no thread to give).
#[derive(Debug)]
pub struct NoWorker;

impl Matcher {
    pub fn new(deliver: impl Fn(Matched) + Send + Sync + 'static) -> Matcher {
        Matcher {
            shared: Arc::new(Shared {
                slot: Mutex::new(Slot::default()),
                wake: Condvar::new(),
                newest: AtomicU64::new(0),
            }),
            deliver: Arc::new(deliver),
        }
    }

    /// Hands `request` to the worker, in the place of the one that waits, if one does; a
    /// request under way stops.
    pub fn match_names(&self, request: MatchRequest) -> Result<(), NoWorker> {
        let mut slot = self.lock();
        self.shared
            .newest
            .store(request.generation, Ordering::Relaxed);
        slot.waiting = Some(request);
        if !slot.started {
            let shared = Arc::clone(&self.shared);
            let deliver = Arc::clone(&self.deliver);
            let spawned = std::thread::Builder::new()
                .name("rf-ui-match".to_owned())
                .spawn(move || work(&shared, &*deliver));
            if spawned.is_err() {
                slot.waiting = None;
                return Err(NoWorker);
            }
            slot.started = true;
        }
        drop(slot);
        self.shared.wake.notify_one();
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.shared
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Matcher {
    fn drop(&mut self) {
        let mut slot = self.lock();
        slot.closed = true;
        slot.waiting = None;
        self.shared.newest.store(0, Ordering::Relaxed);
        drop(slot);
        self.shared.wake.notify_one();
    }
}

/// The worker: the newest request each time, until the matcher goes.
fn work(shared: &Shared, deliver: &(dyn Fn(Matched) + Send + Sync)) {
    loop {
        let request = {
            let mut slot = shared.slot.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if slot.closed {
                    return;
                }
                if let Some(request) = slot.waiting.take() {
                    break request;
                }
                slot = shared
                    .wake
                    .wait(slot)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let generation = request.generation;
        let keep_going = || shared.newest.load(Ordering::Relaxed) == generation;
        if let Some(matched) = request.run(&keep_going) {
            deliver(matched);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::selection::{NamePattern, PatternSyntax};

    fn request(generation: u64, rows: usize) -> MatchRequest {
        let names: Arc<[OsString]> = (0..rows)
            .map(|i| OsString::from(format!("f{i}.txt")))
            .collect();
        MatchRequest {
            generation,
            rows: names,
            pattern: NamePattern::new("*.txt", PatternSyntax::Wildcard, true)
                .expect("a pattern")
                .expect("something to match"),
        }
    }

    #[test]
    fn a_newer_request_takes_the_place_of_the_one_that_waits_or_stops_it() {
        // The worker hands each answer over and then waits for `go` before the next request:
        // the requests made meanwhile wait.
        let (done, found) = mpsc::channel::<(u64, usize)>();
        let (go, wait) = mpsc::channel::<()>();
        let wait = Mutex::new(wait);
        let matcher = Matcher::new(move |matched| {
            done.send((matched.generation, matched.hits.len()))
                .expect("the test listens");
            let _ = wait
                .lock()
                .expect("one worker")
                .recv_timeout(Duration::from_secs(10));
        });
        let next = || found.recv_timeout(Duration::from_secs(10)).ok();

        matcher.match_names(request(1, 3)).expect("a worker");
        assert_eq!(next(), Some((1, 3)));
        // While the worker hands over the first answer: two more; only the newest runs.
        matcher.match_names(request(2, 5)).expect("a worker");
        matcher.match_names(request(3, 7)).expect("a worker");
        go.send(()).expect("the worker listens");
        assert_eq!(next(), Some((3, 7)));
        go.send(()).expect("the worker listens");
        assert_eq!(found.recv_timeout(Duration::from_millis(100)).ok(), None);
    }

    #[test]
    fn a_run_stops_once_a_newer_request_is_made() {
        let stopped = request(4, 5_000).run(&|| false);
        assert_eq!(stopped, None);
        let whole = request(4, 5_000).run(&|| true).expect("not stopped");
        assert_eq!(whole.hits.len(), 5_000);
        // A short run has no stop to look at.
        assert!(request(5, 10).run(&|| false).is_some());
    }
}
