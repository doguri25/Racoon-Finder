//! The workers of the properties window: the measure ([`Measurer`]: the entry of the folder a
//! pane shows, where a lone link leads, and the count of what the folders hold;
//! [`measure`](crate::properties::measure)) and the checksums ([`Hasher`];
//! [`hash_files`](crate::hashing::hash_files)), each a [`Newest`] worker with one slot, so that
//! they go on side by side. A newer request (another window, another algorithm) stops the one
//! under way through its [`CancelToken`] and takes its place, and `stop` stops it (중지, the
//! window's end). The work tells its news from the worker's thread as it goes; what becomes of
//! the request comes to `deliver`: its end, or a failure (the work panicked, or no thread could
//! be started for it), which the window shows.
//!
//! The checksums of several files are one request, a queue the worker goes through in order:
//! each file's checksum comes as it is made, and only the whole request is replaced by a newer
//! one.

use std::time::Duration;

use rf_vfs::CancelToken;

use super::newest::{Job, Newest, Outcome};
use crate::hashing::HashRequest;
use crate::properties::{MeasureRequest, Tally};

impl Job for MeasureRequest {
    /// The generation of the request.
    type Ticket = u64;

    fn slot(&self) -> u64 {
        0
    }

    fn ticket(&self) -> u64 {
        self.generation
    }
}

impl Job for HashRequest {
    /// The generation of the request.
    type Ticket = u64;

    fn slot(&self) -> u64 {
        0
    }

    fn ticket(&self) -> u64 {
        self.generation
    }
}

/// The measure worker: what it gives is the request's generation and the whole count.
pub type Measurer = Newest<MeasureRequest, (u64, Tally)>;

/// The worker of the checksums: what it gives is the generation of the request it finished.
pub type Hasher = Newest<HashRequest, u64>;

/// A measure worker that does each request with `work` (`None`: its token stopped it) and gives
/// what becomes of it to `deliver`.
pub fn measurer(
    work: impl Fn(&MeasureRequest, &CancelToken) -> Option<Tally> + Send + Sync + 'static,
    deliver: impl Fn(Outcome<u64, (u64, Tally)>) + Send + Sync + 'static,
) -> Measurer {
    Newest::new(
        "rf-ui-measure",
        Duration::ZERO,
        move |request: MeasureRequest, token| {
            let tally = work(&request, token)?;
            Some((request.generation, tally))
        },
        deliver,
    )
}

/// A worker of the checksums that does each request with `work` (`None`: its token stopped it)
/// and gives what becomes of it to `deliver`.
pub fn hasher(
    work: impl Fn(&HashRequest, &CancelToken) -> Option<()> + Send + Sync + 'static,
    deliver: impl Fn(Outcome<u64, u64>) + Send + Sync + 'static,
) -> Hasher {
    Newest::new(
        "rf-ui-hash",
        Duration::ZERO,
        move |request: HashRequest, token| {
            work(&request, token)?;
            Some(request.generation)
        },
        deliver,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, mpsc};

    use rf_core::VPath;

    use super::*;
    use crate::hashing::Algorithm;

    const WAIT: Duration = Duration::from_secs(10);
    const QUIET: Duration = Duration::from_millis(300);

    fn request(generation: u64) -> HashRequest {
        HashRequest {
            generation,
            algorithm: Algorithm::Sha256,
            files: vec![(
                0,
                VPath::local(std::env::temp_dir().join("big.iso")).expect("a local path"),
            )],
        }
    }

    /// A worker whose work for a request reads "forever" (until its token is cancelled) for an
    /// odd generation, panics for 0, and ends at once otherwise; it tells which ones it starts.
    fn worker() -> (
        Hasher,
        mpsc::Receiver<u64>,
        mpsc::Receiver<Outcome<u64, u64>>,
    ) {
        let (started, starts) = mpsc::channel();
        let started = Mutex::new(started);
        let (delivered, outcomes) = mpsc::channel();
        let delivered = Mutex::new(delivered);
        let hasher = hasher(
            move |request, token| {
                let _ = started.lock().expect("one worker").send(request.generation);
                assert!(request.generation != 0, "a bug in the work");
                if request.generation % 2 == 1 {
                    while !token.is_cancelled() {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    return None;
                }
                Some(())
            },
            move |outcome| {
                let _ = delivered.lock().expect("one worker").send(outcome);
            },
        );
        (hasher, starts, outcomes)
    }

    #[test]
    fn a_stop_ends_the_checksums_under_way_and_a_newer_request_takes_their_place() {
        let (hasher, starts, outcomes) = worker();
        hasher.submit(request(1)).expect("a worker");
        assert_eq!(starts.recv_timeout(WAIT).ok(), Some(1));
        // The window closed: the file of many gigabytes is left, and nothing comes of it.
        hasher.stop();
        assert_eq!(outcomes.recv_timeout(QUIET).ok(), None);
        // Another window's request runs, and the one after it replaces it on the way.
        hasher.submit(request(3)).expect("a worker");
        assert_eq!(starts.recv_timeout(WAIT).ok(), Some(3));
        hasher.submit(request(4)).expect("a worker");
        assert_eq!(starts.recv_timeout(WAIT).ok(), Some(4));
        assert_eq!(outcomes.recv_timeout(WAIT).ok(), Some(Outcome::Done(4)));
        assert_eq!(outcomes.recv_timeout(QUIET).ok(), None);
    }

    #[test]
    fn checksums_that_panic_come_back_failed_and_the_worker_goes_on() {
        let (hasher, _starts, outcomes) = worker();
        hasher.submit(request(0)).expect("a worker");
        assert_eq!(outcomes.recv_timeout(WAIT).ok(), Some(Outcome::Failed(0)));
        hasher.submit(request(2)).expect("a worker");
        assert_eq!(outcomes.recv_timeout(WAIT).ok(), Some(Outcome::Done(2)));
    }

    #[test]
    fn a_count_gives_its_generation_with_what_it_found() {
        let (delivered, outcomes) = mpsc::channel();
        let delivered = Mutex::new(delivered);
        let measurer = measurer(
            |_, _| {
                Some(Tally {
                    files: 2,
                    ..Tally::default()
                })
            },
            move |outcome| {
                let _ = delivered.lock().expect("one worker").send(outcome);
            },
        );
        measurer
            .submit(MeasureRequest {
                generation: 9,
                items: Vec::new(),
                link_targets: false,
            })
            .expect("a worker");
        assert_eq!(
            outcomes.recv_timeout(WAIT).ok(),
            Some(Outcome::Done((
                9,
                Tally {
                    files: 2,
                    ..Tally::default()
                }
            )))
        );
    }
}
