//! The worker that makes the previews ([`Effect::Preview`]): one thread, which keeps only the
//! newest request. A request waits [`PREVIEW_DELAY`] before it starts, and a newer one that comes
//! meanwhile takes its place, so the keys that run down a list start no work until the cursor
//! rests; one that comes while a preview is made stops it (its [`CancelToken`], which the
//! folder count looks at between batches) and drops what it finds. [`Previewer::stop`] does the
//! same when nothing is to show.
//!
//! [`Effect::Preview`]: crate::state::Effect::Preview
//! [`PREVIEW_DELAY`]: crate::preview::PREVIEW_DELAY

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use rf_vfs::CancelToken;

use crate::preview::{PreviewContent, PreviewRequest, Previewed};

/// What the worker does with a request: the preview, or `None` when the token stopped it.
type Work = dyn Fn(&PreviewRequest, &CancelToken) -> Option<PreviewContent> + Send + Sync;

/// Hands the requests to the worker, which gives what it made to `deliver` on its own thread.
/// The worker starts with the first request and ends when the previewer goes.
pub struct Previewer {
    shared: Arc<Shared>,
    delay: Duration,
    work: Arc<Work>,
    deliver: Arc<dyn Fn(Previewed) + Send + Sync>,
}

struct Shared {
    slot: Mutex<Slot>,
    wake: Condvar,
}

#[derive(Default)]
struct Slot {
    /// The request that waits for the worker, and when it may start.
    waiting: Option<(PreviewRequest, Instant)>,
    /// The generation and the token of the preview being made.
    running: Option<(u64, CancelToken)>,
    /// The worker runs.
    started: bool,
    /// The previewer is gone: the worker ends.
    closed: bool,
}

/// The worker could not be started (the system has no thread to give).
#[derive(Debug)]
pub struct NoWorker;

impl Previewer {
    /// A previewer whose requests wait `delay`, made by `work` and handed to `deliver`.
    pub fn new(
        delay: Duration,
        work: impl Fn(&PreviewRequest, &CancelToken) -> Option<PreviewContent> + Send + Sync + 'static,
        deliver: impl Fn(Previewed) + Send + Sync + 'static,
    ) -> Previewer {
        Previewer {
            shared: Arc::new(Shared {
                slot: Mutex::new(Slot::default()),
                wake: Condvar::new(),
            }),
            delay,
            work: Arc::new(work),
            deliver: Arc::new(deliver),
        }
    }

    /// Hands `request` to the worker, in the place of the one that waits; a preview being made
    /// stops.
    pub fn preview(&self, request: PreviewRequest) -> Result<(), NoWorker> {
        let mut slot = self.lock();
        stop_running(&mut slot);
        slot.waiting = Some((request, Instant::now() + self.delay));
        if !slot.started {
            let shared = Arc::clone(&self.shared);
            let work = Arc::clone(&self.work);
            let deliver = Arc::clone(&self.deliver);
            let spawned = std::thread::Builder::new()
                .name("rf-ui-preview".to_owned())
                .spawn(move || run(&shared, &*work, &*deliver));
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

    /// Drops the request that waits and stops the preview being made: nothing it would show
    /// shows any more.
    pub fn stop(&self) {
        let mut slot = self.lock();
        stop_running(&mut slot);
        slot.waiting = None;
    }

    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.shared
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Previewer {
    fn drop(&mut self) {
        let mut slot = self.lock();
        slot.closed = true;
        slot.waiting = None;
        stop_running(&mut slot);
        drop(slot);
        self.shared.wake.notify_one();
    }
}

impl std::fmt::Debug for Previewer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Previewer")
            .field("delay", &self.delay)
            .finish_non_exhaustive()
    }
}

/// Stops the preview being made, if one is: what it finds is dropped.
fn stop_running(slot: &mut Slot) {
    if let Some((_, token)) = slot.running.take() {
        token.cancel();
    }
}

/// The worker: the newest request each time, once its time has come, until the previewer goes.
fn run(shared: &Shared, work: &Work, deliver: &(dyn Fn(Previewed) + Send + Sync)) {
    loop {
        let (request, token) = {
            let mut slot = shared.slot.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if slot.closed {
                    return;
                }
                let now = Instant::now();
                match slot.waiting.as_ref().map(|(_, due)| *due) {
                    None => {
                        slot = shared
                            .wake
                            .wait(slot)
                            .unwrap_or_else(PoisonError::into_inner);
                    }
                    Some(due) if due > now => {
                        slot = shared
                            .wake
                            .wait_timeout(slot, due - now)
                            .unwrap_or_else(PoisonError::into_inner)
                            .0;
                    }
                    Some(_) => {
                        let Some((request, _)) = slot.waiting.take() else {
                            continue;
                        };
                        let token = CancelToken::new();
                        slot.running = Some((request.generation, token.clone()));
                        break (request, token);
                    }
                }
            }
        };
        // A panic in the work (the decoders run guarded already) does not end the worker.
        let made = catch_unwind(AssertUnwindSafe(|| work(&request, &token)))
            .unwrap_or(Some(PreviewContent::Unavailable));
        let current = {
            let mut slot = shared.slot.lock().unwrap_or_else(PoisonError::into_inner);
            let current =
                matches!(&slot.running, Some((generation, _)) if *generation == request.generation);
            if current {
                slot.running = None;
            }
            current && !token.is_cancelled()
        };
        if let (true, Some(content)) = (current, made) {
            deliver(Previewed {
                generation: request.generation,
                content,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use rf_core::{EntryKind, VPath};
    use rf_vfs::VisibilityFilter;

    use super::*;

    fn request(generation: u64) -> PreviewRequest {
        PreviewRequest {
            generation,
            path: VPath::local(std::env::temp_dir().join(format!("p{generation}")))
                .expect("a local path"),
            kind: EntryKind::File,
            filter: VisibilityFilter::default(),
        }
    }

    /// A previewer whose work tells which requests it starts and answers each with `Plain`,
    /// and whose answers come to the returned receivers.
    fn previewer(
        delay: Duration,
        hold: Option<Arc<Mutex<mpsc::Receiver<()>>>>,
    ) -> (Previewer, mpsc::Receiver<u64>, mpsc::Receiver<u64>) {
        let (started, starts) = mpsc::channel();
        let started = Mutex::new(started);
        let (delivered, answers) = mpsc::channel();
        let delivered = Mutex::new(delivered);
        let previewer = Previewer::new(
            delay,
            move |request, token| {
                let _ = started.lock().expect("one worker").send(request.generation);
                if let Some(hold) = &hold {
                    // Waits for the test, then gives up when the token stopped it.
                    let _ = hold
                        .lock()
                        .expect("one worker")
                        .recv_timeout(Duration::from_secs(10));
                    if token.is_cancelled() {
                        return None;
                    }
                }
                Some(PreviewContent::Plain)
            },
            move |previewed| {
                let _ = delivered
                    .lock()
                    .expect("one worker")
                    .send(previewed.generation);
            },
        );
        (previewer, starts, answers)
    }

    const WAIT: Duration = Duration::from_secs(10);
    const QUIET: Duration = Duration::from_millis(300);

    #[test]
    fn requests_within_the_delay_leave_only_the_last_one_to_be_made() {
        let (previewer, starts, answers) = previewer(Duration::from_millis(200), None);
        let asked = Instant::now();
        for generation in 1..=5 {
            previewer.preview(request(generation)).expect("a worker");
        }
        assert_eq!(starts.recv_timeout(WAIT).ok(), Some(5));
        // Not before the delay has passed since the last request.
        assert!(asked.elapsed() >= Duration::from_millis(200));
        assert_eq!(answers.recv_timeout(WAIT).ok(), Some(5));
        assert_eq!(starts.recv_timeout(QUIET).ok(), None);
    }

    #[test]
    fn a_newer_request_stops_the_preview_being_made_and_drops_it() {
        let (go, hold) = mpsc::channel::<()>();
        let (previewer, starts, answers) =
            previewer(Duration::ZERO, Some(Arc::new(Mutex::new(hold))));
        previewer.preview(request(1)).expect("a worker");
        assert_eq!(starts.recv_timeout(WAIT).ok(), Some(1));
        // While the first one is made, a second one comes: the first one's token stops it.
        previewer.preview(request(2)).expect("a worker");
        go.send(()).expect("the worker listens");
        assert_eq!(starts.recv_timeout(WAIT).ok(), Some(2));
        go.send(()).expect("the worker listens");
        assert_eq!(answers.recv_timeout(WAIT).ok(), Some(2));
        assert_eq!(answers.recv_timeout(QUIET).ok(), None);
    }

    #[test]
    fn a_stop_drops_the_request_that_waits_and_the_one_being_made() {
        let (previewer, starts, _answers) = previewer(Duration::from_millis(150), None);
        previewer.preview(request(1)).expect("a worker");
        previewer.stop();
        assert_eq!(starts.recv_timeout(QUIET).ok(), None);
        let (go, hold) = mpsc::channel::<()>();
        let (previewer, starts, made) =
            self::previewer(Duration::ZERO, Some(Arc::new(Mutex::new(hold))));
        previewer.preview(request(2)).expect("a worker");
        assert_eq!(starts.recv_timeout(WAIT).ok(), Some(2));
        previewer.stop();
        go.send(()).expect("the worker listens");
        assert_eq!(made.recv_timeout(QUIET).ok(), None);
    }

    #[test]
    fn a_panic_in_the_work_shows_it_cannot_be_previewed_and_the_worker_goes_on() {
        let (delivered, answers) = mpsc::channel();
        let delivered = Mutex::new(delivered);
        let previewer = Previewer::new(
            Duration::ZERO,
            |request, _| {
                assert!(request.generation != 1, "a bug in the work");
                Some(PreviewContent::Plain)
            },
            move |previewed| {
                let _ = delivered.lock().expect("one worker").send(previewed);
            },
        );
        previewer.preview(request(1)).expect("a worker");
        assert_eq!(
            answers.recv_timeout(WAIT).ok(),
            Some(Previewed {
                generation: 1,
                content: PreviewContent::Unavailable
            })
        );
        previewer.preview(request(2)).expect("a worker");
        assert_eq!(
            answers.recv_timeout(WAIT).ok(),
            Some(Previewed {
                generation: 2,
                content: PreviewContent::Plain
            })
        );
    }
}
