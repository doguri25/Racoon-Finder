//! The job engine (XFER-01, XFER-15, REL-02, REL-03): the queue, the lanes that run the jobs
//! on background threads, pause, resume, cancel and reordering, the events a window follows,
//! and the recovery of the jobs a crash cut off.
//!
//! The app process owns one engine for its whole life, whatever its windows do: a job runs to
//! its end with or without anyone listening (XFER-15). Every public method of [`JobEngine`]
//! only takes a lock in memory, sends on channels and, for the first job of a lane, starts the
//! lane's thread ([`submit`](JobEngine::submit), [`undo`](JobEngine::undo) and
//! [`redo`](JobEngine::redo)); none touches a file or waits for a worker, so the UI thread may
//! call them (REL-03). [`JobEngine::shutdown`] alone waits, when the app ends after its windows
//! are closed.
//!
//! # Lanes
//!
//! A [`LanePolicy`] gives each job a lane. A lane runs its jobs one at a time, in queue order,
//! on its own thread `rf-jobs-lane-{n}`, started when the lane gets its first job, and passes
//! over a queued job that is paused. Different lanes run at the same time, so a rename does not
//! wait behind a long copy ([`DefaultLanes`]).
//!
//! # Events
//!
//! Every subscriber gets every [`JobEvent`] from the moment it subscribed, in the order the
//! changes happened. The channels are unbounded, so sending never blocks: events are sent
//! under the state lock, which keeps their order, and a subscriber that is gone is dropped at
//! the next event. A job is announced as `State(Queued)` and `Queue` when it is submitted and
//! ends with exactly one `Finished`. A window opened while jobs run takes their list and its
//! subscription together ([`JobEngine::subscribe_with_jobs`]), each job with its spec.
//!
//! # Journal
//!
//! With a journal folder, transfers and deletes ([`JobSpec::journaled`]) are noted in the
//! journal: `Submitted` when they are queued, `Started` (durably) before a worker runs them,
//! the folders of their temporary files as the executor makes them, and `Finished` when they
//! end. At start the journal thread first recovers the jobs the journal holds that never
//! finished; a short-lived thread `rf-jobs-recovery` then removes their temporary files and
//! announces them with one [`JobEvent::Recovered`], even when there are none. The user then
//! resubmits or dismisses each one.
//!
//! The app's end is not the user's cancel (XFER-10): the jobs [`JobEngine::shutdown`] cuts off,
//! queued or running, get no `Finished` record, so the next start recovers them like the jobs a
//! crash cut off. A job the user cancelled has ended for good, and is not recovered.
//!
//! # Undo
//!
//! The engine keeps the [`UndoHistory`] of the app, up to [`UNDO_LIMIT`] records in each list.
//! When a job ends, its record (`JobReport::undo`) is noted before its `Finished` event, so a
//! window that hears the event finds the history up to date: the record of a job the user ran
//! can be undone next and empties the redo list, the record an undo left can be redone next, and
//! the record a redo left can be undone next. A job without a record (a permanent delete, a job
//! that changed nothing) leaves both lists as they are. [`JobEngine::undo`] and
//! [`JobEngine::redo`] take the next record off its list and submit it as a
//! [`JobSpec::Undo`] job, which runs like any other, one at a time: while one is queued or
//! running, neither starts another. One that never runs puts its record back.

use std::collections::BTreeMap;
use std::fmt;
use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use rf_core::{Error, ErrorKind, Os};
use rf_vfs::Vfs;

use crate::control::Control;
use crate::exec::{ExecEnv, ExecEvent, execute};
use crate::id::JobId;
use crate::journal::{self, IncompleteJob, JournalHandle, Record, Recovered};
use crate::progress::Progress;
use crate::report::{ItemFailure, JobReport, Outcome, Stage, Totals};
use crate::resolver::Resolver;
use crate::spec::{JobKind, JobSpec};
use crate::undo::{UNDO_LIMIT, UndoHistory, UndoRecord};

/// How often [`JobEngine::shutdown`] looks whether the threads have ended.
const SHUTDOWN_POLL: Duration = Duration::from_millis(10);

/// How long [`JobEngine::shutdown`] gives the journal's threads to end, after the wait for the
/// workers: the journal then has its last records to write, which takes a moment however
/// late the workers ended.
const JOURNAL_GRACE: Duration = Duration::from_secs(2);

#[cfg(test)]
thread_local! {
    /// In tests, the names of the threads that [`spawn_thread`] refuses to start when it is
    /// called on this thread, as if the system had no room for another thread.
    static REFUSED_SPAWNS: std::cell::RefCell<Vec<&'static str>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Starts the thread `name`, which runs `f`.
pub(crate) fn spawn_thread<T, F>(name: String, f: F) -> std::io::Result<JoinHandle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    #[cfg(test)]
    if REFUSED_SPAWNS.with_borrow(|names| names.contains(&name.as_str())) {
        return Err(std::io::Error::other("injected spawn failure"));
    }
    thread::Builder::new().name(name).spawn(f)
}

/// A lane of the engine. The jobs of one lane run one at a time, in queue order; the jobs of
/// different lanes run at the same time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LaneId(pub u32);

impl LaneId {
    /// The lane of the long jobs: transfers and deletes.
    pub const MAIN: LaneId = LaneId(0);
    /// The lane of the one-step jobs, renames and creates, which should not wait behind a copy.
    pub const QUICK: LaneId = LaneId(1);
}

/// Which lane a job runs in.
pub trait LanePolicy: Send + Sync + fmt::Debug {
    /// The lane of `spec`. It is called on the thread that submits the job, the UI thread
    /// included, so it must not block (no I/O) or panic.
    ///
    /// A panic goes on to the caller of [`JobEngine::submit`], [`JobEngine::undo`] or
    /// [`JobEngine::redo`], and the job is not queued; the record an undo or a redo took off
    /// its list goes back as for an undo that never runs.
    fn lane(&self, spec: &JobSpec) -> LaneId;
}

/// Renames and creates in [`LaneId::QUICK`], and so the undos of one step: of a rename, of a
/// create and of that undo. Everything else, the other undos included, in [`LaneId::MAIN`].
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultLanes;

impl LanePolicy for DefaultLanes {
    fn lane(&self, spec: &JobSpec) -> LaneId {
        match spec {
            JobSpec::Rename { .. }
            | JobSpec::Create { .. }
            | JobSpec::Undo(
                UndoRecord::Renamed { .. }
                | UndoRecord::Created { .. }
                | UndoRecord::Removed { .. },
            ) => LaneId::QUICK,
            JobSpec::Transfer(_) | JobSpec::Delete(_) | JobSpec::Undo(_) => LaneId::MAIN,
        }
    }
}

/// What an engine runs with. The app builds it as
/// `EngineConfig::new(Vfs::with_local(), ui_resolver).with_journal(app_paths.data_dir())`.
pub struct EngineConfig {
    /// Where the jobs find their providers.
    pub vfs: Vfs,
    /// Who answers the questions of every job, on the job's worker thread.
    pub resolver: Arc<dyn Resolver>,
    /// The folder the journal is kept in, or `None` for no journal: then nothing is recovered
    /// after a crash.
    pub journal_dir: Option<PathBuf>,
    /// Which lane each job runs in.
    pub lanes: Arc<dyn LanePolicy>,
    /// The naming rule of new items (`a (2).txt` or `a 2.txt`).
    pub naming_os: Os,
}

impl EngineConfig {
    /// An engine with the providers of `vfs` and the answers of `resolver`, no journal,
    /// [`DefaultLanes`] and the naming rule of this OS.
    pub fn new(vfs: Vfs, resolver: Arc<dyn Resolver>) -> EngineConfig {
        EngineConfig {
            vfs,
            resolver,
            journal_dir: None,
            lanes: Arc::new(DefaultLanes),
            naming_os: Os::CURRENT,
        }
    }

    /// Keeps the journal in the folder `dir`, which is made with the first record.
    #[must_use]
    pub fn with_journal(mut self, dir: impl Into<PathBuf>) -> EngineConfig {
        self.journal_dir = Some(dir.into());
        self
    }
}

impl fmt::Debug for EngineConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineConfig")
            .field("vfs", &self.vfs)
            .field("journal_dir", &self.journal_dir)
            .field("lanes", &self.lanes)
            .field("naming_os", &self.naming_os)
            .finish_non_exhaustive()
    }
}

/// Where a job is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum JobState {
    /// In the queue, waiting for its lane.
    Queued,
    /// In the queue and passed over by its lane, or running and held at its next checkpoint.
    /// A running job that waits for an answer shows as `Waiting` until it has the answer, and
    /// then as `Paused`. A job that is cancelled is no longer paused.
    Paused,
    /// A worker has taken the job up and it counts what it will do.
    Planning,
    /// The plan is made and the job works through its items.
    Running,
    /// The job waits for its resolver to answer a question. Once it has the answer, it is in
    /// the state it was in before the question again: `Planning` for a question asked while
    /// it planned, `Running` otherwise, or `Paused` if it was paused meanwhile.
    Waiting,
    /// The job has ended. The engine forgets a job when it ends ([`JobEvent::Finished`]), so
    /// this state is in no snapshot and no event; it is for a list that keeps ended jobs on
    /// show.
    Finished(Outcome),
}

/// A job the engine has not finished, as [`JobEngine::jobs`] lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobSnapshot {
    pub id: JobId,
    pub kind: JobKind,
    /// What the job was asked to do, so that a window opened after the job was submitted can
    /// show what it is (its items, its target).
    pub spec: Arc<JobSpec>,
    pub lane: LaneId,
    pub state: JobState,
    /// The last progress the job reported; all zeros before it starts.
    pub progress: Progress,
}

/// What the engine tells its subscribers.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum JobEvent {
    /// The job is now in `state`.
    State { id: JobId, state: JobState },
    /// The queued jobs, paused ones included, in the order they wait in. Sent when a job joins
    /// the queue, is moved in it, or leaves it (cancelled, or taken up by its lane).
    Queue { order: Vec<JobId> },
    /// The job's plan is made: what it expects to process, and how many problems the
    /// pre-flight check found on the target.
    Planned {
        id: JobId,
        totals: Totals,
        issues: usize,
    },
    /// How far the job has got.
    Progress { id: JobId, progress: Progress },
    /// The job has ended, and the engine has forgotten it. Its last word.
    Finished { id: JobId, report: Arc<JobReport> },
    /// The journal's jobs that never finished, found at start, and how many temporary files
    /// they had left that were removed. Sent once by an engine with a journal, after the
    /// journal was read, even when nothing was found or the journal could not be read.
    Recovered {
        jobs: Vec<IncompleteJob>,
        temp_files_removed: usize,
    },
    /// The journal could not be read, compacted or written; the log text, in English. The
    /// jobs run all the same, but one that a crash cuts off may not be recovered.
    JournalError { message: String },
}

/// The queue of jobs and the lanes that run them. Clones share one engine, which lives as
/// long as the app process: dropping the last handle stops nothing, [`shutdown`](Self::shutdown)
/// does. `Send + Sync`, so any thread may hold a handle.
#[derive(Clone)]
pub struct JobEngine {
    shared: Arc<Shared>,
}

impl JobEngine {
    /// Starts an engine with `config`, and gives the receiver of its first subscriber, made
    /// before any thread of the engine runs so that it misses no event (the
    /// [`JobEvent::Recovered`] of the journal included).
    ///
    /// With a journal, the journal thread first reads the journal and finds the jobs that never
    /// finished; then, on the thread `rf-jobs-recovery`, the temporary files each of them left
    /// are removed, [`recovered`](Self::recovered) lists them and `Recovered` is sent. A journal
    /// that cannot be read or compacted is a [`JobEvent::JournalError`]; the engine runs on and
    /// keeps the journal of its new jobs.
    ///
    /// When the system cannot start the journal thread or the recovery thread, the engine runs
    /// without a journal: it sends a `JournalError` that says so and an empty `Recovered`, and
    /// the journal on the disk is left as it is, for the next start.
    pub fn start(config: EngineConfig) -> (JobEngine, Receiver<JobEvent>) {
        let EngineConfig {
            vfs,
            resolver,
            journal_dir,
            lanes,
            naming_os,
        } = config;
        let (subscriber, events) = mpsc::channel();
        let shared = Arc::new(Shared {
            vfs,
            resolver,
            lanes,
            naming_os,
            state: Mutex::new(State::new(subscriber)),
            job_ready: Condvar::new(),
        });
        if let Some(dir) = journal_dir {
            start_journal(&shared, dir);
        }
        (JobEngine { shared }, events)
    }

    /// A new subscriber, which gets every event from now on.
    pub fn subscribe(&self) -> Receiver<JobEvent> {
        let (subscriber, events) = mpsc::channel();
        self.shared.lock().subscribers.push(subscriber);
        events
    }

    /// A new subscriber, with the jobs that have not ended as [`jobs`](Self::jobs) lists them,
    /// both taken in one hold of the engine's lock: the subscriber's first event is the first
    /// change after the list. A window that shows the list and then follows the events neither
    /// misses a change nor applies one the list already shows.
    pub fn subscribe_with_jobs(&self) -> (Receiver<JobEvent>, Vec<JobSnapshot>) {
        let (subscriber, events) = mpsc::channel();
        let mut state = self.shared.lock();
        state.subscribers.push(subscriber);
        (events, state.jobs())
    }

    /// Puts `spec` at the end of the queue as a new job and gives its ID; `State(Queued)` and
    /// `Queue` announce it. A journaled job is noted as submitted (without waiting for the
    /// journal). The first job of a lane starts the lane's worker.
    ///
    /// The job ends at once, with only its `Finished` event and having done nothing, as
    /// [`Outcome::Cancelled`] after [`shutdown`](Self::shutdown), and as [`Outcome::Aborted`]
    /// with a [`Stage::Internal`] failure when the worker of its lane cannot be started.
    ///
    /// A [`JobSpec::Undo`] submitted here (a recovered one, or the `JobReport::retry_spec` of
    /// an undo) runs as a job the user ran, not as an [`undo`](Self::undo): it does not wait for
    /// an undo or a redo under way, and the record its report gives goes on the undo list, which
    /// empties the redo list.
    pub fn submit(&self, spec: JobSpec) -> JobId {
        self.submit_as(spec, Origin::User)
    }

    /// Undoes the last job that can be undone: takes its record off the undo list and submits
    /// it as a [`JobSpec::Undo`] job, whose ID it gives (see [`submit`](Self::submit)). The
    /// record the job's report gives can be redone next. `None` when there is nothing to undo,
    /// or when an undo or a redo is queued or running: one runs at a time, so that each starts
    /// from what the one before it left.
    ///
    /// An undo that never runs (cancelled while it is queued, ended by
    /// [`shutdown`](Self::shutdown) while it is queued, submitted after it, or left without a
    /// worker) puts its record back on top of the undo list, and the redo list stays as it is;
    /// unless a job the user ran has noted its record since, which must be undone first: then
    /// the record is dropped. One that `shutdown` ended is also recovered from the journal at
    /// the next start, as a job that never started (its record is in its spec). An undo that runs
    /// notes only the reverse of what it did: one that stops partway, or fails for some or all
    /// of its items, leaves the rest of its record off both lists (it is not rebuilt). And an
    /// undo that ends after a job the user ran has noted its record leaves no record at all,
    /// since nothing undone before a new action can be redone.
    pub fn undo(&self) -> Option<JobId> {
        let (record, origin) = self.shared.lock().take_record(false)?;
        Some(self.submit_as(JobSpec::Undo(record), origin))
    }

    /// Redoes the last undo that can be redone: takes its record off the redo list and submits
    /// it as a [`JobSpec::Undo`] job, whose ID it gives. The record the job's report gives can
    /// be undone next. `None` when there is nothing to redo, or when an undo or a redo is
    /// queued or running.
    ///
    /// A redo that never runs puts its record back on top of the redo list, unless a job the
    /// user ran has noted its record since, after which nothing can be redone: then the record
    /// is dropped. A redo that runs notes only the reverse of what it did, as an undo does, and
    /// none at all when it ends after a job the user ran has noted its record, which is the one
    /// to undo next.
    pub fn redo(&self) -> Option<JobId> {
        let (record, origin) = self.shared.lock().take_record(true)?;
        Some(self.submit_as(JobSpec::Undo(record), origin))
    }

    /// What [`undo`](Self::undo) can undo, the oldest first: the last one is undone next.
    pub fn undo_history(&self) -> Vec<UndoRecord> {
        self.shared.lock().history.undo_list()
    }

    /// What [`redo`](Self::redo) can redo: the last one is redone next.
    pub fn redo_history(&self) -> Vec<UndoRecord> {
        self.shared.lock().history.redo_list()
    }

    /// [`submit`](Self::submit), for a job that `origin` asked for. An undo or redo that ends
    /// here without being queued gives its record back (see [`State::unrun`]), even when the
    /// lane policy panics; the panic then goes on.
    fn submit_as(&self, spec: JobSpec, origin: Origin) -> JobId {
        let id = JobId::generate();
        // Outside the lock: the policy is the caller's code.
        let lane = panic::catch_unwind(AssertUnwindSafe(|| self.shared.lanes.lane(&spec)));
        let lane = lane.unwrap_or_else(|payload| {
            self.shared.lock().unrun(origin, &spec);
            panic::resume_unwind(payload)
        });
        let mut state = self.shared.lock();
        if state.shut_down {
            state.unrun(origin, &spec);
            let report = cancelled_report(id, &spec);
            state.broadcast(&JobEvent::Finished { id, report });
            return id;
        }
        if let Err(error) = self.shared.start_worker(&mut state, lane) {
            state.unrun(origin, &spec);
            let mut report = JobReport::new(id, &spec);
            report.outcome = Outcome::Aborted;
            report.failures.push(ItemFailure {
                source: None,
                target: None,
                stage: Stage::Internal,
                kind: ErrorKind::Other,
                message: format!("the worker thread could not be started: {error}"),
                reason: None,
            });
            let report = Arc::new(report);
            state.broadcast(&JobEvent::Finished { id, report });
            return id;
        }
        if spec.journaled() {
            state.journal.append(Record::Submitted {
                job: id,
                spec: spec.clone(),
                at: SystemTime::now(),
            });
        }
        state.queue.push(QueuedJob {
            id,
            spec: Arc::new(spec),
            lane,
            paused: false,
            origin,
        });
        state.broadcast(&JobEvent::State {
            id,
            state: JobState::Queued,
        });
        state.announce_queue();
        drop(state);
        self.shared.job_ready.notify_all();
        id
    }

    /// Pauses the job `id`. A queued job becomes `Paused` and its lane passes over it; a running
    /// one stops at its next checkpoint ([`Control::pause`]) and becomes `Paused`, or, if it is
    /// waiting for an answer, once it has the answer. `true` if the job is queued or running and
    /// not being cancelled, paused already or not; `false` otherwise.
    pub fn pause(&self, id: JobId) -> bool {
        let mut state = self.shared.lock();
        if let Some(job) = state.queued_mut(id) {
            if !job.paused {
                job.paused = true;
                state.broadcast(&JobEvent::State {
                    id,
                    state: JobState::Paused,
                });
            }
            return true;
        }
        state.set_paused(id, true)
    }

    /// Lets the paused job `id` go on: a queued one becomes `Queued` again, a running one goes
    /// back to the state it was in ([`Control::resume`]). `true` if the job is queued or
    /// running and not being cancelled, paused or not; `false` otherwise.
    pub fn resume(&self, id: JobId) -> bool {
        let mut state = self.shared.lock();
        if let Some(job) = state.queued_mut(id) {
            if job.paused {
                job.paused = false;
                state.broadcast(&JobEvent::State {
                    id,
                    state: JobState::Queued,
                });
                drop(state);
                self.shared.job_ready.notify_all();
            }
            return true;
        }
        state.set_paused(id, false)
    }

    /// Cancels the job `id`. A queued job leaves the queue and ends at once as
    /// [`Outcome::Cancelled`], having done nothing. A running one is cancelled through its
    /// [`Control`]: it stops at its next checkpoint, or when a provider call that takes the
    /// cancel token returns, removes what it was writing and ends (a job waiting for its
    /// resolver stops once the answer is back). A paused running job is no longer paused, and
    /// its state is announced at once. `false` for a job that has ended or is unknown.
    ///
    /// The cancel is the user's and final: the journal notes the job as cancelled, and it is
    /// not recovered at the next start.
    pub fn cancel(&self, id: JobId) -> bool {
        let mut state = self.shared.lock();
        if let Some(index) = state.queued_index(id) {
            let job = state.queue.remove(index);
            state.announce_queue();
            state.end_unrun(&job, true);
            return true;
        }
        state.cancel_running(id, false)
    }

    /// Moves the queued job `id`, paused or not, to `position` in the queue (0 is next; a
    /// position past the end is the end) and announces the new order. `false` for a job that is
    /// not queued.
    pub fn move_to(&self, id: JobId, position: usize) -> bool {
        let mut state = self.shared.lock();
        let Some(index) = state.queued_index(id) else {
            return false;
        };
        let job = state.queue.remove(index);
        let position = position.min(state.queue.len());
        state.queue.insert(position, job);
        state.announce_queue();
        true
    }

    /// The jobs that have not ended: the running ones in the order they started, then the
    /// queued ones in queue order.
    pub fn jobs(&self) -> Vec<JobSnapshot> {
        self.shared.lock().jobs()
    }

    /// The jobs recovered from the journal at start that are neither resubmitted nor dismissed
    /// yet, in the order they were submitted. Empty until [`JobEvent::Recovered`] is sent.
    pub fn recovered(&self) -> Vec<IncompleteJob> {
        self.shared.lock().recovered.clone()
    }

    /// Forgets the recovered job `id`: it leaves [`recovered`](Self::recovered) and the journal
    /// notes it as dismissed, so it is not recovered again. `false` for a job that is not
    /// listed there.
    pub fn dismiss_recovered(&self, id: JobId) -> bool {
        self.shared.lock().dismiss(id).is_some()
    }

    /// Dismisses the recovered job `id` and submits what it was asked to do as a new job, whose
    /// ID it gives. `None` for a job that is not listed in [`recovered`](Self::recovered).
    ///
    /// The new job runs the whole spec again, and meets what the job that was cut off already
    /// finished: a copy finds its own copies at the targets and treats them as conflicts (under
    /// `ConflictPolicy::AutoRename` it makes numbered copies of them), and a move fails with
    /// `NotFound` for each item it had moved already. A recovered [`JobSpec::Undo`] runs as a
    /// job the user ran, as [`submit`](Self::submit) says: it does not wait for an undo or a
    /// redo under way, and its record goes on the undo list.
    pub fn resubmit_recovered(&self, id: JobId) -> Option<JobId> {
        let job = self.shared.lock().dismiss(id)?;
        Some(self.submit(job.spec))
    }

    /// Ends the engine, when the app ends: every queued job ends as [`Outcome::Cancelled`],
    /// every running one is cancelled, and later submits end at once. Then, without the state
    /// lock, it waits up to `timeout` in all, looking every 10 ms, for the lane workers to end,
    /// then lets the journal write what it has and gives its threads 2 s more of their own,
    /// however late the workers ended. `true` if every thread of the engine has ended.
    ///
    /// The app's end is not the user's cancel (XFER-10). The journal notes no end for the
    /// queued jobs, nor for the running ones that end cancelled by this call alone, so the next
    /// start recovers them; in this process their reports and events say `Cancelled` all the
    /// same, and a temporary file such a job was writing is removed as it stops. A job the user
    /// cancelled before ([`cancel`](Self::cancel), or a `Cancel` answer to one of its
    /// questions) is noted as cancelled, and one that ends with another outcome during the wait
    /// is noted with it: neither is recovered. That is why the journal is stopped only after
    /// the workers (or the timeout). A job that is still running when the time is up is noted
    /// as it was then, and is recovered at the next start like a job cut off by a crash.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        let started = Instant::now();
        let journal = {
            let mut state = self.shared.lock();
            state.shut_down = true;
            let queued = mem::take(&mut state.queue);
            if !queued.is_empty() {
                state.announce_queue();
            }
            for job in &queued {
                state.end_unrun(job, false);
            }
            let running: Vec<JobId> = state.running.iter().map(|job| job.id).collect();
            for id in running {
                // A paused worker holds its own clone of the control; only a cancel wakes it.
                state.cancel_running(id, true);
            }
            state.journal.clone()
        };
        self.shared.job_ready.notify_all();
        let workers_ended = |state: &State| state.workers.values().all(JoinHandle::is_finished);
        self.shared.wait_until(started, timeout, workers_ended);
        journal.shutdown();
        let journal_ended = self
            .shared
            .wait_until(Instant::now(), JOURNAL_GRACE, |state| {
                state.journal_threads.iter().all(JoinHandle::is_finished)
            });
        // A worker that ended while the journal did counts as ended too.
        journal_ended && workers_ended(&self.shared.lock())
    }
}

impl fmt::Debug for JobEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobEngine")
            .field("vfs", &self.shared.vfs)
            .field("lanes", &self.shared.lanes)
            .field("naming_os", &self.shared.naming_os)
            .finish_non_exhaustive()
    }
}

/// Starts the journal thread on the folder `dir`, with its recovery, and the thread that ends
/// the recovery.
///
/// The recovery thread starts first, so that it can report whatever happens to the journal
/// thread. A thread that cannot be started leaves the engine without a journal, with a
/// `JournalError` and an empty `Recovered`.
fn start_journal(shared: &Arc<Shared>, dir: PathBuf) {
    let (found, recovery) = mpsc::channel();
    let recovery_thread = {
        let shared = Arc::clone(shared);
        spawn_thread("rf-jobs-recovery".to_owned(), move || {
            shared.finish_recovery(recovery.recv().ok());
        })
    };
    let recovery_thread = match recovery_thread {
        Ok(thread) => thread,
        Err(error) => {
            shared.journal_error(format!("the recovery thread could not be started: {error}"));
            shared.lock().broadcast(&JobEvent::Recovered {
                jobs: Vec::new(),
                temp_files_removed: 0,
            });
            return;
        }
    };
    let on_start = {
        let (dir, found) = (dir.clone(), found.clone());
        // Only the recovery: the temporary files are removed on a thread of their own, so that
        // a folder that does not answer (an offline share) does not hold up the journal and
        // with it every job.
        Box::new(move || {
            // The recovery thread waits for this, and is there to take it.
            let _ = found.send(journal::recover(&dir));
        })
    };
    let on_error = {
        let shared = Arc::clone(shared);
        Box::new(move |error: &Error| shared.journal_error(error.to_string()))
    };
    let mut threads = vec![recovery_thread];
    match JournalHandle::spawn(dir, on_start, on_error) {
        Ok((journal, journal_thread)) => {
            threads.push(journal_thread);
            shared.lock().journal = journal;
        }
        // The recovery thread reports it, with nothing recovered.
        Err(error) => {
            let _ = found.send(Err(error));
        }
    }
    shared.lock().journal_threads = threads;
}

/// The report of the job `id`, cancelled before it ran: it did nothing.
fn cancelled_report(id: JobId, spec: &JobSpec) -> Arc<JobReport> {
    let mut report = JobReport::new(id, spec);
    report.outcome = Outcome::Cancelled;
    Arc::new(report)
}

/// What the engine and its threads share.
struct Shared {
    vfs: Vfs,
    resolver: Arc<dyn Resolver>,
    lanes: Arc<dyn LanePolicy>,
    naming_os: Os,
    state: Mutex<State>,
    /// Signalled when a lane may have a job to take: one was queued or resumed, or the engine
    /// shut down.
    job_ready: Condvar,
}

impl Shared {
    /// The state lock. No update leaves the state half-made where it could panic, so a
    /// poisoned lock is used as it is.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts the worker of `lane`, unless it runs already.
    fn start_worker(self: &Arc<Self>, state: &mut State, lane: LaneId) -> std::io::Result<()> {
        if state.workers.contains_key(&lane) {
            return Ok(());
        }
        let shared = Arc::clone(self);
        let worker = spawn_thread(format!("rf-jobs-lane-{}", lane.0), move || {
            shared.work(lane);
        })?;
        state.workers.insert(lane, worker);
        Ok(())
    }

    /// The worker of `lane`: runs its jobs one by one until the engine shuts down.
    fn work(&self, lane: LaneId) {
        while let Some(job) = self.take(lane) {
            self.run(job);
        }
    }

    /// The first queued job of `lane` that is not paused, now running, once there is one.
    /// `None` once the engine has shut down.
    fn take(&self, lane: LaneId) -> Option<Taken> {
        let mut state = self.lock();
        loop {
            let next = state
                .queue
                .iter()
                .position(|job| job.lane == lane && !job.paused);
            if let Some(index) = next {
                return Some(state.start(index));
            }
            if state.shut_down {
                return None;
            }
            state = self
                .job_ready
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Runs the job a worker took, without the state lock, and ends it: its `Finished` record,
    /// then out of the state and its undo record into the history, then [`JobEvent::Finished`].
    ///
    /// A job that ends cancelled when only [`JobEngine::shutdown`] cancelled it gets no
    /// `Finished` record: the app's end cut it off, and the next start recovers it.
    fn run(&self, job: Taken) {
        let Taken {
            id,
            spec,
            control,
            journal,
            origin,
        } = job;
        let journaled = spec.journaled();
        if journaled {
            // A failure goes to the journal's error handler, which reports it; the job runs all
            // the same.
            let _ = journal.append_durable(Record::Started { job: id });
        }
        let on_event = |event: ExecEvent| self.exec_event(id, event);
        let env = ExecEnv::new(&self.vfs, self.resolver.as_ref())
            .naming_os(self.naming_os)
            .control(control)
            .on_event(&on_event);
        let env = if journaled {
            env.journal(&journal)
        } else {
            env
        };
        let report = execute(id, &spec, &env);
        let mut state = self.lock();
        let cut_off = report.outcome == Outcome::Cancelled
            && state
                .running_mut(id)
                .is_some_and(|job| job.cancelled == Some(CancelledBy::Shutdown));
        if journaled && !cut_off {
            journal.append(Record::Finished {
                job: id,
                outcome: Some(report.outcome),
            });
        }
        state.running.retain(|job| job.id != id);
        state.note_record(origin, report.undo.as_ref());
        let report = Arc::new(report);
        state.broadcast(&JobEvent::Finished { id, report });
    }

    /// Takes in what the running job `id` tells, on its worker thread.
    fn exec_event(&self, id: JobId, event: ExecEvent) {
        let mut state = self.lock();
        match event {
            ExecEvent::Planned { totals, issues } => {
                if let Some(job) = state.running_mut(id) {
                    job.progress.total = totals;
                }
                state.broadcast(&JobEvent::Planned { id, totals, issues });
                state.update(id, |job| job.phase = JobState::Running);
            }
            ExecEvent::Progress(progress) => {
                if let Some(job) = state.running_mut(id) {
                    job.progress = progress.clone();
                }
                state.broadcast(&JobEvent::Progress { id, progress });
            }
            ExecEvent::Waiting(waiting) => state.update(id, |job| job.waiting = waiting),
        }
    }

    /// Ends the recovery with what the journal thread `found`: reports a journal that could
    /// not be read or compacted, removes the temporary files of the jobs found, lists them and
    /// announces them. `None` when the journal thread ended without an answer.
    fn finish_recovery(&self, found: Option<rf_core::Result<Recovered>>) {
        let jobs = match found {
            Some(Ok(Recovered { jobs, compaction })) => {
                if let Some(error) = compaction {
                    self.journal_error(error.to_string());
                }
                jobs
            }
            Some(Err(error)) => {
                self.journal_error(error.to_string());
                Vec::new()
            }
            None => {
                self.journal_error("the journal thread ended before the recovery".to_owned());
                Vec::new()
            }
        };
        let temp_files_removed = jobs
            .iter()
            .map(|job| journal::remove_orphans(&self.vfs, job))
            .sum();
        let mut state = self.lock();
        state.recovered.clone_from(&jobs);
        state.broadcast(&JobEvent::Recovered {
            jobs,
            temp_files_removed,
        });
    }

    fn journal_error(&self, message: String) {
        self.lock().broadcast(&JobEvent::JournalError { message });
    }

    /// Waits until `done` holds or `timeout` has passed since `started`, and says whether it
    /// holds. The state lock is held only to look, every [`SHUTDOWN_POLL`].
    fn wait_until(
        &self,
        started: Instant,
        timeout: Duration,
        done: impl Fn(&State) -> bool,
    ) -> bool {
        loop {
            if done(&self.lock()) {
                return true;
            }
            let left = timeout.saturating_sub(started.elapsed());
            if left.is_zero() {
                return false;
            }
            thread::sleep(left.min(SHUTDOWN_POLL));
        }
    }
}

/// A job a worker has taken up, with what it runs with.
struct Taken {
    id: JobId,
    spec: Arc<JobSpec>,
    /// The worker's handle to the job's control; the state keeps the other.
    control: Control,
    journal: JournalHandle,
    origin: Origin,
}

/// Who asked for a job, which says where the record its report gives goes in the history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    /// The user, with [`JobEngine::submit`].
    User,
    /// [`JobEngine::undo`], which took the job's record off the undo list in the history's
    /// generation `popped_at`.
    Undo { popped_at: u64 },
    /// [`JobEngine::redo`], which took the job's record off the redo list in the history's
    /// generation `popped_at`.
    Redo { popped_at: u64 },
}

/// The engine's jobs, subscribers and threads, behind its one lock.
#[derive(Debug)]
struct State {
    /// The queued jobs of every lane, in queue order.
    queue: Vec<QueuedJob>,
    /// The running jobs, in the order they started.
    running: Vec<RunningJob>,
    subscribers: Vec<Sender<JobEvent>>,
    /// The recovered jobs not yet resubmitted or dismissed.
    recovered: Vec<IncompleteJob>,
    /// What can be undone and redone.
    history: UndoHistory,
    /// Whether a job of [`JobEngine::undo`] or [`JobEngine::redo`] is queued or running; until
    /// it ends, neither starts another.
    undoing: bool,
    /// A disabled handle without a journal.
    journal: JournalHandle,
    /// The worker of each lane that has had a job.
    workers: BTreeMap<LaneId, JoinHandle<()>>,
    /// The journal thread and the recovery thread, with a journal.
    journal_threads: Vec<JoinHandle<()>>,
    shut_down: bool,
}

impl State {
    fn new(subscriber: Sender<JobEvent>) -> State {
        State {
            queue: Vec::new(),
            running: Vec::new(),
            subscribers: vec![subscriber],
            recovered: Vec::new(),
            history: UndoHistory::new(UNDO_LIMIT),
            undoing: false,
            journal: JournalHandle::disabled(),
            workers: BTreeMap::new(),
            journal_threads: Vec::new(),
            shut_down: false,
        }
    }

    /// Sends `event` to every subscriber and drops the ones that are gone. The channels are
    /// unbounded, so this never blocks.
    fn broadcast(&mut self, event: &JobEvent) {
        self.subscribers
            .retain(|subscriber| subscriber.send(event.clone()).is_ok());
    }

    fn announce_queue(&mut self) {
        let order = self.queue.iter().map(|job| job.id).collect();
        self.broadcast(&JobEvent::Queue { order });
    }

    fn queued_index(&self, id: JobId) -> Option<usize> {
        self.queue.iter().position(|job| job.id == id)
    }

    fn queued_mut(&mut self, id: JobId) -> Option<&mut QueuedJob> {
        self.queue.iter_mut().find(|job| job.id == id)
    }

    fn running_mut(&mut self, id: JobId) -> Option<&mut RunningJob> {
        self.running.iter_mut().find(|job| job.id == id)
    }

    /// The jobs that have not ended, as [`JobEngine::jobs`] lists them.
    fn jobs(&self) -> Vec<JobSnapshot> {
        let running = self.running.iter().map(RunningJob::snapshot);
        running
            .chain(self.queue.iter().map(QueuedJob::snapshot))
            .collect()
    }

    /// Takes the queued job at `index` up: it leaves the queue, runs as `Planning` with a new
    /// control, and is announced.
    fn start(&mut self, index: usize) -> Taken {
        let QueuedJob {
            id,
            spec,
            lane,
            origin,
            ..
        } = self.queue.remove(index);
        let control = Control::new();
        self.running.push(RunningJob::new(
            id,
            Arc::clone(&spec),
            lane,
            control.clone(),
        ));
        self.broadcast(&JobEvent::State {
            id,
            state: JobState::Planning,
        });
        self.announce_queue();
        Taken {
            id,
            spec,
            control,
            journal: self.journal.clone(),
            origin,
        }
    }

    /// Takes the record for a new undo (`redo` false) or redo off its list, with the origin of
    /// the job that runs it, and notes that an undo or redo is under way. `None` when the list
    /// is empty, or when an undo or redo is queued or running already.
    fn take_record(&mut self, redo: bool) -> Option<(UndoRecord, Origin)> {
        if self.undoing {
            return None;
        }
        let popped_at = self.history.generation();
        let taken = if redo {
            (self.history.pop_redo()?, Origin::Redo { popped_at })
        } else {
            (self.history.pop_undo()?, Origin::Undo { popped_at })
        };
        self.undoing = true;
        Some(taken)
    }

    /// Notes `record`, what the job of `origin` that ran left, in the history (see the module
    /// doc), and ends the undo or redo under way if the job was one.
    ///
    /// The reverse an undo or a redo left is dropped when a job the user ran has noted its
    /// record since the undo or redo took its own off the list: nothing undone before a new
    /// action can be redone, and the new action's record is the one to undo next.
    fn note_record(&mut self, origin: Origin, record: Option<&UndoRecord>) {
        if origin != Origin::User {
            self.undoing = false;
        }
        let Some(record) = record.cloned() else {
            return;
        };
        match origin {
            Origin::User => self.history.push(record),
            Origin::Undo { popped_at } | Origin::Redo { popped_at }
                if popped_at != self.history.generation() => {}
            Origin::Undo { .. } => self.history.push_undone(record),
            Origin::Redo { .. } => self.history.push_redone(record),
        }
    }

    /// Ends the job `spec` of `origin` that never ran (cancelled while queued, submitted after
    /// the shutdown, or without a worker). An undo or redo puts its record back on its list,
    /// unless a job the user ran has noted a record since it was taken off (see
    /// `UndoHistory::put_back_undo`), and the next undo or redo may start.
    fn unrun(&mut self, origin: Origin, spec: &JobSpec) {
        let (Origin::Undo { popped_at } | Origin::Redo { popped_at }) = origin else {
            return;
        };
        self.undoing = false;
        let JobSpec::Undo(record) = spec else {
            return;
        };
        let record = record.clone();
        if matches!(origin, Origin::Undo { .. }) {
            self.history.put_back_undo(record, popped_at);
        } else {
            self.history.put_back_redo(record, popped_at);
        }
    }

    /// Ends `job`, taken out of the queue before it ran, as cancelled; an undo or redo gives its
    /// record back (see [`unrun`](Self::unrun)). With `for_good` (the user's cancel), the
    /// journal notes the end; without (the app's end), it does not, and the next start
    /// recovers the job.
    fn end_unrun(&mut self, job: &QueuedJob, for_good: bool) {
        self.unrun(job.origin, &job.spec);
        if for_good && job.spec.journaled() {
            self.journal.append(Record::Finished {
                job: job.id,
                outcome: Some(Outcome::Cancelled),
            });
        }
        let report = cancelled_report(job.id, &job.spec);
        self.broadcast(&JobEvent::Finished { id: job.id, report });
    }

    /// Changes the running job `id` with `change` and announces its state if that changed.
    fn update(&mut self, id: JobId, change: impl FnOnce(&mut RunningJob)) {
        let Some(job) = self.running_mut(id) else {
            return;
        };
        let before = job.state();
        change(job);
        let after = job.state();
        if after != before {
            self.broadcast(&JobEvent::State { id, state: after });
        }
    }

    /// Pauses or resumes the running job `id` and announces its state if that changed. `false`
    /// when the job is not running or is being cancelled.
    fn set_paused(&mut self, id: JobId, paused: bool) -> bool {
        let Some(job) = self.running_mut(id) else {
            return false;
        };
        if job.cancelled.is_some() {
            return false;
        }
        if paused {
            job.control.pause();
        } else {
            job.control.resume();
        }
        self.update(id, |job| job.paused = paused);
        true
    }

    /// Cancels the running job `id` through its control: it is no longer paused, and its state
    /// is announced if that changed. A cancel `by_shutdown` is the app's end, which leaves the
    /// job to be recovered at the next start, unless the user cancelled it before; any other is
    /// the user's, and final. `false` when the job is not running.
    fn cancel_running(&mut self, id: JobId, by_shutdown: bool) -> bool {
        let Some(job) = self.running_mut(id) else {
            return false;
        };
        job.cancelled = match (by_shutdown, job.cancelled) {
            (false, _) | (true, Some(CancelledBy::User)) => Some(CancelledBy::User),
            // A `Cancel` answer to a question cancels the control from the worker's side.
            (true, None) if job.control.is_cancelled() => Some(CancelledBy::User),
            (true, None | Some(CancelledBy::Shutdown)) => Some(CancelledBy::Shutdown),
        };
        job.control.cancel();
        self.update(id, |job| job.paused = false);
        true
    }

    /// Takes the recovered job `id` off the list and notes in the journal that it was
    /// dismissed.
    fn dismiss(&mut self, id: JobId) -> Option<IncompleteJob> {
        let index = self.recovered.iter().position(|job| job.id == id)?;
        let job = self.recovered.remove(index);
        self.journal.append(Record::Finished {
            job: id,
            outcome: None,
        });
        Some(job)
    }
}

/// A job in the queue.
#[derive(Debug)]
struct QueuedJob {
    id: JobId,
    spec: Arc<JobSpec>,
    lane: LaneId,
    paused: bool,
    origin: Origin,
}

impl QueuedJob {
    fn snapshot(&self) -> JobSnapshot {
        JobSnapshot {
            id: self.id,
            kind: self.spec.kind(),
            spec: Arc::clone(&self.spec),
            lane: self.lane,
            state: if self.paused {
                JobState::Paused
            } else {
                JobState::Queued
            },
            progress: Progress::default(),
        }
    }
}

/// A job a worker runs.
#[derive(Debug)]
struct RunningJob {
    id: JobId,
    spec: Arc<JobSpec>,
    lane: LaneId,
    /// `Planning` until the plan is made, then `Running`: how far the job has got, paused,
    /// waiting or not.
    phase: JobState,
    /// Whether the job waits for an answer from its resolver.
    waiting: bool,
    /// Whether the job was paused and not resumed since. It shows once the job is not waiting
    /// for an answer, so a pause made while it planned or waited is kept.
    paused: bool,
    /// Who cancelled the job, if anyone did; it then only ends.
    cancelled: Option<CancelledBy>,
    progress: Progress,
    /// The engine's handle to the job's control.
    control: Control,
}

/// Who cancelled a running job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CancelledBy {
    /// The user, with [`JobEngine::cancel`] or a `Cancel` answer to one of its questions: the
    /// job has ended for good.
    User,
    /// Only [`JobEngine::shutdown`]: the app's end. A job that then ends cancelled is left in
    /// the journal, to be recovered at the next start.
    Shutdown,
}

impl RunningJob {
    /// The job `id` a worker has just taken up, with the engine's handle to its control.
    fn new(id: JobId, spec: Arc<JobSpec>, lane: LaneId, control: Control) -> RunningJob {
        RunningJob {
            id,
            spec,
            lane,
            phase: JobState::Planning,
            waiting: false,
            paused: false,
            cancelled: None,
            progress: Progress::default(),
            control,
        }
    }

    fn state(&self) -> JobState {
        if self.waiting {
            JobState::Waiting
        } else if self.paused {
            JobState::Paused
        } else {
            self.phase
        }
    }

    fn snapshot(&self) -> JobSnapshot {
        JobSnapshot {
            id: self.id,
            kind: self.spec.kind(),
            spec: Arc::clone(&self.spec),
            lane: self.lane,
            state: self.state(),
            progress: self.progress.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rf_core::VPath;

    use super::*;
    use crate::conflict::{Conflict, ConflictAction, ConflictAnswer};
    use crate::naming::temp_name;
    use crate::preflight::Issue;
    use crate::resolver::{ErrorAction, ErrorAnswer, NonInteractive, PreflightAnswer};
    use crate::spec::{
        DeleteMethod, DeleteSpec, ErrorPolicy, TransferMode, TransferOptions, TransferSpec,
    };
    use crate::testkit::{Fault, FaultyProvider, wait_finished, wait_for};

    const WAIT: Duration = Duration::from_secs(20);

    // Any thread of the app may hold the engine.
    const _: () = {
        const fn send_sync<T: Send + Sync>() {}
        send_sync::<JobEngine>();
    };

    fn v(path: &Path) -> VPath {
        VPath::local(path).unwrap()
    }

    /// A folder `src` with the file `name` of `len` bytes, an empty folder `dst`, and a copy of
    /// the file into `dst`.
    fn copy_fixture(name: &str, len: usize) -> (tempfile::TempDir, PathBuf, JobSpec) {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        std::fs::create_dir(&src).unwrap();
        std::fs::create_dir(&dst).unwrap();
        std::fs::write(src.join(name), vec![7; len]).unwrap();
        let sources = [v(&src.join(name))];
        let options = TransferOptions::default();
        let spec = TransferSpec::into_dir(TransferMode::Copy, &sources, &v(&dst), options);
        (t, dst, JobSpec::Transfer(spec.unwrap()))
    }

    fn local_engine(journal: Option<&Path>) -> (JobEngine, Receiver<JobEvent>) {
        let config = EngineConfig::new(Vfs::with_local(), Arc::new(NonInteractive));
        JobEngine::start(match journal {
            Some(dir) => config.with_journal(dir),
            None => config,
        })
    }

    /// The IDs and the removed count of the next `Recovered` event.
    fn recovered(events: &Receiver<JobEvent>) -> (Vec<JobId>, usize) {
        let event = wait_for(events, WAIT, |e| matches!(e, JobEvent::Recovered { .. }));
        let JobEvent::Recovered {
            jobs,
            temp_files_removed,
        } = event
        else {
            unreachable!()
        };
        (jobs.iter().map(|job| job.id).collect(), temp_files_removed)
    }

    /// An engine without threads whose only job, `id`, a worker has just taken up.
    fn taken_up(id: JobId) -> (JobEngine, Receiver<JobEvent>) {
        let (subscriber, events) = mpsc::channel();
        let mut state = State::new(subscriber);
        let spec = JobSpec::Transfer(TransferSpec {
            mode: TransferMode::Copy,
            items: Vec::new(),
            options: TransferOptions::default(),
        });
        let control = Control::new();
        let job = RunningJob::new(id, Arc::new(spec), LaneId::MAIN, control);
        state.running.push(job);
        let shared = Arc::new(Shared {
            vfs: Vfs::new(),
            resolver: Arc::new(NonInteractive),
            lanes: Arc::new(DefaultLanes),
            naming_os: Os::CURRENT,
            state: Mutex::new(state),
            job_ready: Condvar::new(),
        });
        (JobEngine { shared }, events)
    }

    /// The states announced since the last call.
    fn states(events: &Receiver<JobEvent>) -> Vec<JobState> {
        let states = events.try_iter().filter_map(|event| match event {
            JobEvent::State { state, .. } => Some(state),
            _ => None,
        });
        states.collect()
    }

    const ONE_FILE: Totals = Totals {
        files: 1,
        dirs: 0,
        bytes: 0,
    };

    const PLANNED: ExecEvent = ExecEvent::Planned {
        totals: ONE_FILE,
        issues: 0,
    };

    #[test]
    fn a_pause_made_while_planning_or_waiting_is_kept() {
        let id = JobId(1);
        let (e, events) = taken_up(id);
        assert!(e.pause(id));
        assert!(e.shared.lock().running[0].control.is_paused());
        e.shared.exec_event(id, PLANNED);
        assert_eq!(states(&events), [JobState::Paused]);
        // An open question shows; the pause is back with its answer.
        e.shared.exec_event(id, ExecEvent::Waiting(true));
        e.shared.exec_event(id, ExecEvent::Waiting(false));
        assert_eq!(states(&events), [JobState::Waiting, JobState::Paused]);
        assert!(e.resume(id));
        assert_eq!(states(&events), [JobState::Running]);
        // A pause made during a question shows once the question is answered.
        e.shared.exec_event(id, ExecEvent::Waiting(true));
        assert!(e.pause(id));
        e.shared.exec_event(id, ExecEvent::Waiting(false));
        assert_eq!(states(&events), [JobState::Waiting, JobState::Paused]);
        assert_eq!(e.jobs()[0].state, JobState::Paused);
    }

    #[test]
    fn a_job_resumed_before_its_plan_is_planning_again() {
        let id = JobId(1);
        let (e, events) = taken_up(id);
        assert!(e.pause(id) && e.resume(id));
        assert!(!e.shared.lock().running[0].control.is_paused());
        e.shared.exec_event(id, PLANNED);
        assert_eq!(
            states(&events),
            [JobState::Paused, JobState::Planning, JobState::Running]
        );
        assert_eq!(e.jobs()[0].progress.total, ONE_FILE);
    }

    #[test]
    fn a_cancelled_job_only_ends_and_unknown_jobs_are_refused() {
        let id = JobId(1);
        let (e, _events) = taken_up(id);
        assert!(e.cancel(id));
        assert!(e.shared.lock().running[0].control.is_cancelled());
        assert!(!e.pause(id) && !e.resume(id));
        assert!(e.cancel(id), "it has not ended yet");
        let unknown = JobId(2);
        assert!(!e.pause(unknown) && !e.resume(unknown) && !e.cancel(unknown));
        assert!(!e.move_to(unknown, 0) && !e.dismiss_recovered(unknown));
        assert_eq!(e.resubmit_recovered(unknown), None);
    }

    #[test]
    fn a_job_is_announced_from_its_queue_to_its_end() {
        let (_t, _dst, spec) = copy_fixture("a.bin", 10);
        let (e, events) = local_engine(None);
        let id = e.submit(spec);
        let mut seen = Vec::new();
        loop {
            match wait_for(&events, WAIT, |_| true) {
                JobEvent::State { state, .. } => seen.push(format!("{state:?}")),
                JobEvent::Queue { order } => seen.push(format!("queue {}", order.len())),
                JobEvent::Planned { totals, .. } => seen.push(format!("planned {}", totals.files)),
                JobEvent::Finished { id: done, .. } if done == id => break,
                _ => {}
            }
        }
        let want = [
            "Queued",
            "queue 1",
            "Planning",
            "queue 0",
            "planned 1",
            "Running",
        ];
        assert_eq!(seen, want);
        assert!(e.shutdown(WAIT));
    }

    #[test]
    fn a_job_submitted_after_shutdown_ends_without_running() {
        let (_t, dst, spec) = copy_fixture("a.bin", 10);
        let (e, events) = local_engine(None);
        assert!(e.shutdown(WAIT));
        let id = e.submit(spec);
        let report = wait_finished(&events, id, WAIT);
        assert_eq!(
            (report.outcome, report.done),
            (Outcome::Cancelled, Totals::default())
        );
        assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 0);
        assert_eq!(e.jobs(), Vec::new());
        assert!(e.shared.lock().workers.is_empty());
    }

    /// Lets the writer `p` holds go on after a while, for a `shutdown` to wait for.
    fn release_later(p: &Arc<FaultyProvider>) -> JoinHandle<()> {
        let p = Arc::clone(p);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            p.release();
        })
    }

    #[test]
    fn jobs_cut_off_by_shutdown_are_recovered_but_a_cancelled_one_is_not() {
        let (t, dst, held) = copy_fixture("a.bin", 4 << 20);
        let (_t2, _, cancelled) = copy_fixture("b.bin", 10);
        let (_t3, _, queued) = copy_fixture("c.bin", 10);
        let journal = t.path().join("state");
        let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
        let config = EngineConfig::new(p.vfs(), Arc::new(NonInteractive)).with_journal(&journal);
        let (e, events) = JobEngine::start(config);
        let running = e.submit(held);
        assert!(p.wait_held(WAIT));
        let user = e.submit(cancelled);
        assert!(e.cancel(user));
        let waiting = e.submit(queued);
        let releaser = release_later(&p);
        assert!(e.shutdown(WAIT));
        releaser.join().unwrap();
        // In this process all three end as cancelled, and nothing is left behind.
        for id in [user, waiting, running] {
            let report = wait_finished(&events, id, WAIT);
            assert_eq!(report.outcome, Outcome::Cancelled);
        }
        assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 0);
        // The app's end is no cancel of the user's: the next start offers both jobs it cut
        // off again (XFER-10), but not the one the user cancelled.
        let (again, events) = local_engine(Some(&journal));
        assert_eq!(recovered(&events), (vec![running, waiting], 0));
        let started: Vec<bool> = again.recovered().iter().map(|job| job.started).collect();
        assert_eq!(started, [true, false]);
        assert!(again.shutdown(WAIT));
    }

    #[test]
    fn a_running_job_the_user_cancelled_before_shutdown_is_not_recovered() {
        let (t, dst, held) = copy_fixture("a.bin", 4 << 20);
        let journal = t.path().join("state");
        let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
        let config = EngineConfig::new(p.vfs(), Arc::new(NonInteractive)).with_journal(&journal);
        let (e, events) = JobEngine::start(config);
        let id = e.submit(held);
        assert!(p.wait_held(WAIT));
        assert!(e.cancel(id));
        let releaser = release_later(&p);
        assert!(e.shutdown(WAIT));
        releaser.join().unwrap();
        assert_eq!(wait_finished(&events, id, WAIT).outcome, Outcome::Cancelled);
        assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 0);
        let (again, events) = local_engine(Some(&journal));
        assert_eq!(recovered(&events), (Vec::new(), 0));
        assert!(again.shutdown(WAIT));
    }

    #[test]
    fn a_paused_job_that_is_cancelled_is_no_longer_shown_paused() {
        let id = JobId(1);
        let (e, events) = taken_up(id);
        assert!(e.pause(id));
        assert_eq!(states(&events), [JobState::Paused]);
        assert!(e.cancel(id));
        assert_eq!(states(&events), [JobState::Planning]);
        assert_eq!(e.jobs()[0].state, JobState::Planning);
        assert!(!e.shared.lock().running[0].control.is_paused());
    }

    /// An engine with a journal in a new folder, started while this thread refuses to start
    /// the thread `refused`, and its events.
    fn without_thread(refused: &'static str) -> (tempfile::TempDir, JobEngine, Receiver<JobEvent>) {
        let t = tempfile::tempdir().unwrap();
        REFUSED_SPAWNS.with_borrow_mut(|names| names.push(refused));
        let (e, events) = local_engine(Some(&t.path().join("state")));
        REFUSED_SPAWNS.with_borrow_mut(Vec::clear);
        (t, e, events)
    }

    #[test]
    fn a_journal_thread_that_cannot_start_is_reported_and_the_engine_runs_on() {
        for refused in ["rf-jobs-journal", "rf-jobs-recovery"] {
            let (t, e, events) = without_thread(refused);
            let error = wait_for(&events, WAIT, |e| {
                matches!(e, JobEvent::JournalError { .. })
            });
            let JobEvent::JournalError { message } = error else {
                unreachable!()
            };
            assert!(message.contains("could not be started"), "{message}");
            assert_eq!(recovered(&events), (Vec::new(), 0));
            // Without a journal, but the jobs run.
            let (_t, dst, spec) = copy_fixture("a.bin", 10);
            let id = e.submit(spec);
            assert_eq!(wait_finished(&events, id, WAIT).outcome, Outcome::Completed);
            assert_eq!(std::fs::read(dst.join("a.bin")).unwrap(), [7; 10]);
            assert!(e.shutdown(WAIT));
            let file = t.path().join("state").join(journal::JOURNAL_FILE);
            assert!(!file.exists(), "{refused}");
        }
    }

    #[test]
    fn a_journal_that_cannot_be_read_is_reported_and_the_engine_runs_on() {
        let (t, dst, spec) = copy_fixture("a.bin", 10);
        let journal = t.path().join("state");
        // A folder in place of the journal file can be neither read nor written.
        std::fs::create_dir_all(journal.join(journal::JOURNAL_FILE)).unwrap();
        let (e, events) = local_engine(Some(&journal));
        wait_for(&events, WAIT, |e| {
            matches!(e, JobEvent::JournalError { .. })
        });
        assert_eq!(recovered(&events), (Vec::new(), 0));
        let id = e.submit(spec);
        assert_eq!(wait_finished(&events, id, WAIT).outcome, Outcome::Completed);
        assert_eq!(std::fs::read(dst.join("a.bin")).unwrap(), [7; 10]);
        assert!(e.shutdown(WAIT));
    }

    #[test]
    fn a_journal_that_cannot_be_compacted_still_gives_its_jobs() {
        let t = tempfile::tempdir().unwrap();
        let journal = t.path().join("state");
        let (done, cut_off) = (JobId(1), JobId(2));
        let spec = JobSpec::Delete(DeleteSpec {
            paths: vec![v(t.path())],
            method: DeleteMethod::Trash,
            errors: ErrorPolicy::Continue,
        });
        let at = SystemTime::now();
        let records = [
            Record::Submitted {
                job: done,
                spec: spec.clone(),
                at,
            },
            Record::Finished {
                job: done,
                outcome: Some(Outcome::Completed),
            },
            Record::Submitted {
                job: cut_off,
                spec,
                at,
            },
            Record::Started { job: cut_off },
            Record::TempDir {
                job: cut_off,
                dir: v(t.path()),
            },
        ];
        let (writer, thread) = JournalHandle::spawn(
            journal.clone(),
            Box::new(|| {}),
            Box::new(|e| panic!("{e}")),
        )
        .unwrap();
        for record in records {
            writer.append_durable(record).unwrap();
        }
        writer.shutdown();
        thread.join().unwrap();
        let orphan = t.path().join(temp_name(cut_off, 0));
        std::fs::write(&orphan, "x").unwrap();
        // A folder where the compacted journal is written makes the compaction fail.
        std::fs::create_dir(journal.join(journal::COMPACTED_FILE)).unwrap();
        let (e, events) = local_engine(Some(&journal));
        wait_for(&events, WAIT, |e| {
            matches!(e, JobEvent::JournalError { .. })
        });
        assert_eq!(recovered(&events), (vec![cut_off], 1));
        assert!(!orphan.exists());
        assert_eq!(e.recovered().len(), 1);
        assert!(e.shutdown(WAIT));
    }

    /// Answers like [`NonInteractive`] and notes the thread each conflict is asked on.
    #[derive(Debug, Default)]
    struct WhereAsked(Mutex<Vec<Option<String>>>);

    impl Resolver for WhereAsked {
        fn conflict(&self, _job: JobId, _conflict: &Conflict) -> ConflictAnswer {
            let name = thread::current().name().map(str::to_owned);
            self.0.lock().unwrap().push(name);
            ConflictAnswer::once(ConflictAction::Skip)
        }

        fn error(&self, _job: JobId, _failure: &ItemFailure) -> ErrorAnswer {
            ErrorAnswer {
                action: ErrorAction::Skip,
                apply_to_all: false,
            }
        }

        fn preflight(&self, _job: JobId, _issues: &[Issue]) -> PreflightAnswer {
            PreflightAnswer::SkipBlocked
        }
    }

    #[derive(Debug)]
    struct LaneSeven;

    impl LanePolicy for LaneSeven {
        fn lane(&self, _spec: &JobSpec) -> LaneId {
            LaneId(7)
        }
    }

    #[test]
    fn a_lane_runs_its_jobs_on_a_thread_named_after_it() {
        let (_t, dst, spec) = copy_fixture("a.bin", 10);
        std::fs::write(dst.join("a.bin"), "taken").unwrap();
        let asked = Arc::new(WhereAsked::default());
        let mut config = EngineConfig::new(Vfs::with_local(), Arc::clone(&asked) as _);
        config.lanes = Arc::new(LaneSeven);
        let (e, events) = JobEngine::start(config);
        let id = e.submit(spec);
        assert_eq!(wait_finished(&events, id, WAIT).skipped, 1);
        let names = asked.0.lock().unwrap().clone();
        assert_eq!(names, [Some("rf-jobs-lane-7".to_owned())]);
        assert!(e.shutdown(WAIT));
    }
}
