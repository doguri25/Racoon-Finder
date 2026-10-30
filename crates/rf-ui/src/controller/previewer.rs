//! The worker that makes the previews ([`Effect::Preview`]): a [`Newest`] worker with one slot.
//! A request waits [`PREVIEW_DELAY`] before it starts, and a newer one that comes meanwhile
//! takes its place, so the keys that run down a list start no work until the cursor rests; one
//! that comes while a preview is made stops it (its [`CancelToken`], which the folder count looks
//! at between batches) and drops what it finds. [`Previewer::stop`] does the same when nothing is
//! to show. A preview that panics (the decoders run guarded already), or that no thread can be
//! started for, shows that the item cannot be previewed.
//!
//! [`Effect::Preview`]: crate::state::Effect::Preview
//! [`PREVIEW_DELAY`]: crate::preview::PREVIEW_DELAY

use std::time::Duration;

use rf_vfs::CancelToken;

use super::newest::{Job, Newest, NoWorker, Outcome};
use crate::preview::{PreviewContent, PreviewRequest, Previewed};

impl Job for PreviewRequest {
    /// The generation of the request.
    type Ticket = u64;

    fn slot(&self) -> u64 {
        0
    }

    fn ticket(&self) -> u64 {
        self.generation
    }
}

/// Hands the requests to the worker, which gives what it made to `deliver` on its own thread.
/// The worker starts with the first request and ends when the previewer goes.
pub struct Previewer {
    worker: Newest<PreviewRequest, Previewed>,
}

impl Previewer {
    /// A previewer whose requests wait `delay`, made by `work` and handed to `deliver`.
    pub fn new(
        delay: Duration,
        work: impl Fn(&PreviewRequest, &CancelToken) -> Option<PreviewContent> + Send + Sync + 'static,
        deliver: impl Fn(Previewed) + Send + Sync + 'static,
    ) -> Previewer {
        Previewer {
            worker: Newest::new(
                "rf-ui-preview",
                delay,
                move |request: PreviewRequest, token| {
                    let content = work(&request, token)?;
                    Some(Previewed {
                        generation: request.generation,
                        content,
                    })
                },
                move |outcome| {
                    deliver(match outcome {
                        Outcome::Done(previewed) => previewed,
                        Outcome::Failed(generation) => Previewed {
                            generation,
                            content: PreviewContent::Unavailable,
                        },
                    });
                },
            ),
        }
    }

    /// Hands `request` to the worker, in the place of the one that waits; a preview being made
    /// stops.
    pub fn preview(&self, request: PreviewRequest) -> Result<(), NoWorker> {
        self.worker.submit(request)
    }

    /// Drops the request that waits and stops the preview being made: nothing it would show
    /// shows any more.
    pub fn stop(&self) {
        self.worker.stop();
    }
}

impl std::fmt::Debug for Previewer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Previewer")
            .field("worker", &self.worker)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Instant;

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
