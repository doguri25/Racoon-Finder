//! A worker thread that does only the newest job of each slot: the sorts of the tabs
//! ([`Sorter`](super::sorter::Sorter), a slot for each tab), the names the pattern window matches
//! ([`Matcher`](super::matcher::Matcher)) and the previews
//! ([`Previewer`](super::previewer::Previewer)), one slot each.
//!
//! A job waits in its slot until its time has come (a delay after it was handed over, none for
//! most) and the worker is free; a newer job of the same slot takes its place there, and stops
//! the slot's job under way: its [`CancelToken`] is cancelled, which the job may look at, and what
//! it does is dropped. The slots take turns in the order their jobs came. [`Newest::stop`] drops
//! every job that waits and stops the one under way.
//!
//! Nothing a slot waits for is lost: a job that panics, or that no thread can be started for,
//! comes back as [`Outcome::Failed`] with its [`Job::Ticket`], for the state to stop waiting. A
//! delivery that panics loses only what it delivered: the worker goes on with the jobs that
//! wait. A worker that ends anyway is started again by the next job.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use rf_vfs::CancelToken;

/// A job for a [`Newest`] worker.
pub trait Job: Send + 'static {
    /// What names the job when it comes back undone ([`Outcome::Failed`]).
    type Ticket: Send + 'static;

    /// The slot the job waits in: a newer job of the same slot takes its place.
    fn slot(&self) -> u64;

    fn ticket(&self) -> Self::Ticket;
}

/// What became of a job that was not stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<T, D> {
    /// It was done, and gave this.
    Done(D),
    /// It panicked, or no thread could be started for it.
    Failed(T),
}

/// No thread could be started for the worker (the system has none to give); the jobs that
/// waited came back as [`Outcome::Failed`].
#[derive(Debug)]
pub struct NoWorker;

/// What the worker does with a job: what it gives, or `None` when its token stopped it.
type Work<J, D> = dyn Fn(J, &CancelToken) -> Option<D> + Send + Sync;

/// What becomes of each job that was not stopped.
type Deliver<J, D> = dyn Fn(Outcome<<J as Job>::Ticket, D>) + Send + Sync;

/// Hands jobs to its worker thread, which gives what becomes of each to `deliver`, on that
/// thread (or, for a job no thread could be started for, on the caller's). The worker starts
/// with the first job and ends when this goes.
pub struct Newest<J: Job, D: Send + 'static> {
    shared: Arc<Shared<J>>,
    /// The worker thread's name.
    name: &'static str,
    /// How long a job waits before it may start.
    delay: Duration,
    work: Arc<Work<J, D>>,
    deliver: Arc<Deliver<J, D>>,
}

struct Shared<J> {
    slots: Mutex<Slots<J>>,
    wake: Condvar,
}

struct Slots<J> {
    /// The jobs that wait, at most one for each slot, in the order their slots got them, each
    /// with the time it may start.
    waiting: VecDeque<(J, Instant)>,
    /// The slot of the job under way, and its token.
    running: Option<(u64, CancelToken)>,
    /// The worker runs.
    started: bool,
    /// The [`Newest`] is gone: the worker ends.
    closed: bool,
}

impl<J: Job, D: Send + 'static> Newest<J, D> {
    /// A worker named `name` that does each job with `work`, `delay` after it came, and gives
    /// what becomes of it to `deliver`.
    pub fn new(
        name: &'static str,
        delay: Duration,
        work: impl Fn(J, &CancelToken) -> Option<D> + Send + Sync + 'static,
        deliver: impl Fn(Outcome<J::Ticket, D>) + Send + Sync + 'static,
    ) -> Newest<J, D> {
        Newest {
            shared: Arc::new(Shared {
                slots: Mutex::new(Slots {
                    waiting: VecDeque::new(),
                    running: None,
                    started: false,
                    closed: false,
                }),
                wake: Condvar::new(),
            }),
            name,
            delay,
            work: Arc::new(work),
            deliver: Arc::new(deliver),
        }
    }

    /// Hands `job` to the worker, in the place of its slot's job that waits; its slot's job
    /// under way stops. Starts the worker when it does not run; when no thread can be started,
    /// every job that waits comes back as [`Outcome::Failed`] (`job` among them).
    pub fn submit(&self, job: J) -> Result<(), NoWorker> {
        let mut slots = self.lock();
        let slot = job.slot();
        stop_running(&mut slots, Some(slot));
        let due = Instant::now() + self.delay;
        match slots
            .waiting
            .iter_mut()
            .find(|(known, _)| known.slot() == slot)
        {
            Some(known) => *known = (job, due),
            None => slots.waiting.push_back((job, due)),
        }
        if !slots.started {
            let shared = Arc::clone(&self.shared);
            let work = Arc::clone(&self.work);
            let deliver = Arc::clone(&self.deliver);
            let spawned = std::thread::Builder::new()
                .name(self.name.to_owned())
                .spawn(move || run(&shared, &*work, &*deliver));
            if spawned.is_err() {
                let failed: Vec<J::Ticket> = slots
                    .waiting
                    .drain(..)
                    .map(|(job, _)| job.ticket())
                    .collect();
                drop(slots);
                for ticket in failed {
                    (self.deliver)(Outcome::Failed(ticket));
                }
                return Err(NoWorker);
            }
            slots.started = true;
        }
        drop(slots);
        self.shared.wake.notify_one();
        Ok(())
    }

    /// Drops every job that waits and stops the one under way: nothing they would give is
    /// wanted any more.
    pub fn stop(&self) {
        let mut slots = self.lock();
        stop_running(&mut slots, None);
        slots.waiting.clear();
    }

    fn lock(&self) -> MutexGuard<'_, Slots<J>> {
        self.shared
            .slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl<J: Job, D: Send + 'static> Drop for Newest<J, D> {
    fn drop(&mut self) {
        let mut slots = self.lock();
        slots.closed = true;
        slots.waiting.clear();
        stop_running(&mut slots, None);
        drop(slots);
        self.shared.wake.notify_one();
    }
}

impl<J: Job, D: Send + 'static> std::fmt::Debug for Newest<J, D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Newest")
            .field("name", &self.name)
            .field("delay", &self.delay)
            .finish_non_exhaustive()
    }
}

/// Stops the job under way, when it is of `slot` (or of any, for `None`): what it gives is
/// dropped.
fn stop_running<J>(slots: &mut Slots<J>, slot: Option<u64>) {
    if let Some((running, token)) = &slots.running
        && slot.is_none_or(|slot| slot == *running)
    {
        token.cancel();
        slots.running = None;
    }
}

/// Marks the worker as no longer running when its thread ends, however it ends, so that the
/// next job starts another one.
struct Ended<'a, J>(&'a Shared<J>);

impl<J> Drop for Ended<'_, J> {
    fn drop(&mut self) {
        let mut slots = self.0.slots.lock().unwrap_or_else(PoisonError::into_inner);
        slots.started = false;
        slots.running = None;
    }
}

/// The worker: the job that waited longest, once its time has come, each in turn, until the
/// [`Newest`] goes.
fn run<J: Job, D>(shared: &Shared<J>, work: &Work<J, D>, deliver: &Deliver<J, D>) {
    let _ended = Ended(shared);
    loop {
        let (job, token) = {
            let mut slots = shared.slots.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if slots.closed {
                    return;
                }
                let now = Instant::now();
                match slots.waiting.front().map(|(_, due)| *due) {
                    None => {
                        slots = shared
                            .wake
                            .wait(slots)
                            .unwrap_or_else(PoisonError::into_inner);
                    }
                    Some(due) if due > now => {
                        slots = shared
                            .wake
                            .wait_timeout(slots, due - now)
                            .unwrap_or_else(PoisonError::into_inner)
                            .0;
                    }
                    Some(_) => {
                        let Some((job, _)) = slots.waiting.pop_front() else {
                            continue;
                        };
                        let token = CancelToken::new();
                        slots.running = Some((job.slot(), token.clone()));
                        break (job, token);
                    }
                }
            }
        };
        let ticket = job.ticket();
        let outcome = match catch_unwind(AssertUnwindSafe(|| work(job, &token))) {
            Ok(Some(done)) => Some(Outcome::Done(done)),
            // Stopped by its token.
            Ok(None) => None,
            Err(_) => Some(Outcome::Failed(ticket)),
        };
        let current = {
            let mut slots = shared.slots.lock().unwrap_or_else(PoisonError::into_inner);
            if !token.is_cancelled() {
                slots.running = None;
            }
            !token.is_cancelled()
        };
        if let (true, Some(outcome)) = (current, outcome) {
            // Caught too, so that the jobs that wait are not left until the next one comes. Every
            // delivery of the app hands the outcome to the event loop, which cannot panic.
            let _ = catch_unwind(AssertUnwindSafe(|| deliver(outcome)));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    /// A job of slot `slot` named `name`; one named 0 panics.
    #[derive(Debug)]
    struct Named {
        slot: u64,
        name: u64,
    }

    impl Job for Named {
        type Ticket = u64;

        fn slot(&self) -> u64 {
            self.slot
        }

        fn ticket(&self) -> u64 {
            self.name
        }
    }

    const WAIT: Duration = Duration::from_secs(10);
    const QUIET: Duration = Duration::from_millis(200);

    /// A worker whose jobs give their names, a job named 0 panicking, and whose outcomes come
    /// to the returned receiver.
    fn worker() -> (Newest<Named, u64>, mpsc::Receiver<Outcome<u64, u64>>) {
        let (delivered, outcomes) = mpsc::channel();
        let delivered = Mutex::new(delivered);
        let worker = Newest::new(
            "rf-ui-test",
            Duration::ZERO,
            |job: Named, _: &CancelToken| {
                assert!(job.name != 0, "a bug in the work");
                Some(job.name)
            },
            move |outcome| {
                let _ = delivered.lock().expect("one worker").send(outcome);
            },
        );
        (worker, outcomes)
    }

    #[test]
    fn a_job_that_panics_comes_back_failed_and_the_worker_goes_on() {
        let (worker, outcomes) = worker();
        worker.submit(Named { slot: 1, name: 0 }).expect("a worker");
        assert_eq!(outcomes.recv_timeout(WAIT).ok(), Some(Outcome::Failed(0)));
        worker.submit(Named { slot: 1, name: 7 }).expect("a worker");
        assert_eq!(outcomes.recv_timeout(WAIT).ok(), Some(Outcome::Done(7)));
    }

    #[test]
    fn a_delivery_that_panics_leaves_no_job_that_waits_behind() {
        // Job 1 is held until job 2 waits behind it; then its delivery panics.
        let (go, hold) = mpsc::channel::<()>();
        let hold = Mutex::new(hold);
        let (started, starts) = mpsc::channel::<u64>();
        let started = Mutex::new(started);
        let (delivered, outcomes) = mpsc::channel();
        let delivered = Mutex::new(delivered);
        let worker = Newest::new(
            "rf-ui-test",
            Duration::ZERO,
            move |job: Named, _: &CancelToken| {
                let _ = started.lock().expect("one worker").send(job.name);
                if job.name == 1 {
                    let _ = hold.lock().expect("one worker").recv_timeout(WAIT);
                }
                Some(job.name)
            },
            move |outcome: Outcome<u64, u64>| {
                assert_ne!(outcome, Outcome::Done(1), "a bug in the delivery");
                let _ = delivered.lock().expect("one worker").send(outcome);
            },
        );
        worker.submit(Named { slot: 1, name: 1 }).expect("a worker");
        assert_eq!(starts.recv_timeout(WAIT).ok(), Some(1));
        worker.submit(Named { slot: 2, name: 2 }).expect("a worker");
        go.send(()).expect("the worker listens");
        // Job 2 is done without another job to wake the worker.
        assert_eq!(outcomes.recv_timeout(WAIT).ok(), Some(Outcome::Done(2)));
        assert!(worker.lock().started, "the same worker goes on");
    }

    #[test]
    fn a_newer_job_of_a_slot_stops_the_one_under_way_and_takes_the_place_of_the_one_that_waits() {
        let (started, starts) = mpsc::channel::<u64>();
        let started = Mutex::new(started);
        let (go, hold) = mpsc::channel::<()>();
        let hold = Mutex::new(hold);
        let (delivered, outcomes) = mpsc::channel();
        let delivered = Mutex::new(delivered);
        let worker = Newest::new(
            "rf-ui-test",
            Duration::ZERO,
            move |job: Named, token: &CancelToken| {
                let _ = started.lock().expect("one worker").send(job.name);
                let _ = hold.lock().expect("one worker").recv_timeout(WAIT);
                (!token.is_cancelled()).then_some(job.name)
            },
            move |outcome: Outcome<u64, u64>| {
                let _ = delivered.lock().expect("one worker").send(outcome);
            },
        );
        worker
            .submit(Named { slot: 1, name: 10 })
            .expect("a worker");
        assert_eq!(starts.recv_timeout(WAIT).ok(), Some(10));
        // While 10 runs: two more of its slot (the last one stays) and one of another slot.
        worker
            .submit(Named { slot: 1, name: 11 })
            .expect("a worker");
        worker
            .submit(Named { slot: 2, name: 20 })
            .expect("a worker");
        worker
            .submit(Named { slot: 1, name: 12 })
            .expect("a worker");
        for _ in 0..3 {
            go.send(()).expect("the worker listens");
        }
        // 10 was stopped and gives nothing; 12 runs in the place of 11, then 20.
        assert_eq!(starts.recv_timeout(WAIT).ok(), Some(12));
        assert_eq!(starts.recv_timeout(WAIT).ok(), Some(20));
        assert_eq!(outcomes.recv_timeout(WAIT).ok(), Some(Outcome::Done(12)));
        assert_eq!(outcomes.recv_timeout(WAIT).ok(), Some(Outcome::Done(20)));
        assert_eq!(outcomes.recv_timeout(QUIET).ok(), None);
    }

    #[test]
    fn a_stop_drops_what_waits() {
        // No clock: job 1 holds the worker, so that job 2 waits behind it, until the stop.
        let (go, hold) = mpsc::channel::<()>();
        let hold = Mutex::new(hold);
        let (started, starts) = mpsc::channel::<u64>();
        let started = Mutex::new(started);
        let (delivered, outcomes) = mpsc::channel();
        let delivered = Mutex::new(delivered);
        let worker = Newest::new(
            "rf-ui-test",
            Duration::ZERO,
            move |job: Named, _: &CancelToken| {
                let _ = started.lock().expect("one worker").send(job.name);
                if job.name == 1 {
                    let _ = hold.lock().expect("one worker").recv_timeout(WAIT);
                }
                Some(job.name)
            },
            move |outcome: Outcome<u64, u64>| {
                let _ = delivered.lock().expect("one worker").send(outcome);
            },
        );
        worker.submit(Named { slot: 1, name: 1 }).expect("a worker");
        assert_eq!(starts.recv_timeout(WAIT).ok(), Some(1));
        worker.submit(Named { slot: 2, name: 2 }).expect("a worker");
        worker.stop();
        go.send(()).expect("the worker listens");
        // Neither the one under way nor the one that waited gives anything: the first outcome
        // after them is the next job's, which runs in their place.
        worker.submit(Named { slot: 1, name: 3 }).expect("a worker");
        assert_eq!(outcomes.recv_timeout(WAIT).ok(), Some(Outcome::Done(3)));
        assert_eq!(
            starts.recv_timeout(WAIT).ok(),
            Some(3),
            "job 2 never started"
        );
    }
}
