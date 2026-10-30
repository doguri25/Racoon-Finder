//! The window's connection to the job engine: [`Jobs`], which starts the engine and carries out
//! what the window asks of it ([`JobEffect`]), and [`JobRows`], the jobs the job panel shows.
//!
//! Every change of a file goes through the engine (`JobEngine::submit`, `undo`, `redo`), which
//! gives the window undo, conflict questions and the recovery of jobs a crash cut off. A bridge
//! thread empties the engine's first event receiver and hands each event to the window; the
//! [`UiResolver`] hands it the jobs' questions. Both reach the UI thread through the functions
//! [`Jobs::start`] is given, so this module never waits for the window and the window never
//! waits for a job.
//!
//! [`JobRows`] is plain data: it takes the events and gives the rows of the panel and, for a job
//! that ended, an [`Ended`] that the window turns into a notice
//! ([`AppState::job_event`](crate::state::AppState::job_event)).
//!
//! [`RecentJobs`] keeps the jobs that ended for the panel's 최근 완료 (XFER-14), the newest
//! first and [`RECENT_JOBS`] of them at most, each with its report; [`save_log`] writes a
//! report's log (`JobReport::write_log`) into a folder, on a worker thread, never over a file
//! that is there. The log is no file of the user's that a job changes: like the settings, it is
//! written here and not through the engine.

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use jiff::tz::TimeZone;
use rf_jobs::{
    ConflictAnswer, DefaultLanes, EngineConfig, JobEngine, JobEvent, JobId, JobReport, JobSpec,
    JobState, LaneId, LanePolicy, Outcome, PreflightAnswer, Progress, Totals, UndoRecord,
};
use rf_vfs::Vfs;

use crate::resolver::{Question, QuestionId, UiResolver};
use crate::words::{self, JobWork};

/// How long the app's end waits for the running jobs to stop before it releases their
/// questions and ends (see [`close_down`]).
pub const QUIT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the app's end waits instead when every job a worker runs waits for an answer
/// (and paused ones, which a cancel wakes): none can stop before its question is released, so
/// a longer wait only keeps the window up.
pub const QUIT_TIMEOUT_ASKING: Duration = Duration::from_millis(500);

/// The engine of the app and the resolver that asks the window. Clones share them.
#[derive(Clone, Debug)]
pub struct Jobs {
    engine: JobEngine,
    resolver: Arc<UiResolver>,
    /// Set once [`Jobs::close_down`] has run.
    closed: Arc<AtomicBool>,
}

/// What the window asks of the engine. [`Jobs::carry_out`] does it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobEffect {
    /// Run `spec`; `origin` is the pane (`LEFT` or `RIGHT`) that asked, if one did.
    Submit {
        spec: JobSpec,
        origin: Option<usize>,
    },
    /// Undo the last job that can be undone (`JobEngine::undo`).
    Undo,
    /// Redo the last undo (`JobEngine::redo`).
    Redo,
    /// Take the record off the undo list if it is the one to undo next: its job was undone
    /// another way (`JobEngine::forget_undo`).
    ForgetUndo(UndoRecord),
    /// Run the recovered job `id`, which was asked to do `spec`, again.
    Resubmit {
        id: JobId,
        spec: JobSpec,
    },
    /// Forget the recovered job `id`.
    Dismiss(JobId),
    Pause(JobId),
    Resume(JobId),
    Cancel(JobId),
    /// The answer to a conflict question.
    AnswerConflict(QuestionId, ConflictAnswer),
    /// The answer to a pre-flight question.
    AnswerPreflight(QuestionId, PreflightAnswer),
}

/// A job [`Jobs::carry_out`] started, for the window to follow ([`JobRows::track`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Started {
    pub id: JobId,
    /// What the job does, when it is known: an undo or a redo that ended before it could be
    /// looked up is known only from its report.
    pub spec: Option<Arc<JobSpec>>,
    pub work: JobWork,
    pub origin: Option<usize>,
    /// Whether it is an undo or a redo of the history: while one runs, no other starts. A
    /// recovered undo run again is not one: the engine runs it as a job the user ran
    /// (`JobEngine::resubmit_recovered`), which its undo and redo do not wait for.
    pub via_history: bool,
}

/// What the history can do next, as the toolbar shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    /// The record [`JobEngine::undo`] undoes next.
    pub top: Option<UndoRecord>,
    /// Whether there is anything to redo.
    pub can_redo: bool,
}

impl Jobs {
    /// Starts the engine with its journal in `journal_dir`, and the bridge thread that hands
    /// every event of the engine to `on_event`. The engine's questions go to `on_question`
    /// (see [`UiResolver::new`]). Both are called on threads of their own and must not block.
    ///
    /// The bridge thread starts first and waits for the engine's receiver, so a bridge that
    /// cannot start fails this before any engine runs: there is nothing to shut down then, and
    /// nothing here waits for the engine.
    pub fn start(
        journal_dir: &Path,
        on_event: impl Fn(JobEvent) + Send + 'static,
        on_question: impl Fn(Question) -> bool + Send + Sync + 'static,
    ) -> io::Result<Jobs> {
        let (hand_over, handed) = mpsc::channel::<Receiver<JobEvent>>();
        std::thread::Builder::new()
            .name("rf-ui-jobs".to_owned())
            .spawn(move || {
                // Without a receiver (the engine never started) there is nothing to bridge.
                if let Ok(events) = handed.recv() {
                    for event in events {
                        on_event(event);
                    }
                }
            })?;
        let resolver = Arc::new(UiResolver::new(on_question));
        let config = EngineConfig::new(Vfs::with_local(), Arc::clone(&resolver) as _)
            .with_journal(journal_dir);
        let (engine, events) = JobEngine::start(config);
        // The bridge waits for it; it ends only after this.
        let _ = hand_over.send(events);
        Ok(Jobs {
            engine,
            resolver,
            closed: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn engine(&self) -> &JobEngine {
        &self.engine
    }

    pub fn resolver(&self) -> &UiResolver {
        &self.resolver
    }

    /// Does what `effect` asks; may be called on the UI thread (no call here waits for a
    /// job). Gives the job it started, if it started one.
    pub fn carry_out(&self, effect: JobEffect) -> Option<Started> {
        match effect {
            JobEffect::Submit { spec, origin } => {
                let work = JobWork::of(&spec);
                let spec = Arc::new(spec);
                let id = self.engine.submit(JobSpec::clone(&spec));
                Some(Started {
                    id,
                    spec: Some(spec),
                    work,
                    origin,
                    via_history: false,
                })
            }
            JobEffect::Undo => self.history_job(self.engine.undo(), JobWork::Undo),
            JobEffect::Redo => self.history_job(self.engine.redo(), JobWork::Redo),
            JobEffect::ForgetUndo(record) => {
                self.engine.forget_undo(&record);
                None
            }
            JobEffect::Resubmit { id, spec } => {
                let new = self.engine.resubmit_recovered(id)?;
                Some(Started {
                    id: new,
                    work: JobWork::of(&spec),
                    spec: Some(Arc::new(spec)),
                    origin: None,
                    // Even for an undo: see `Started::via_history`.
                    via_history: false,
                })
            }
            JobEffect::Dismiss(id) => {
                self.engine.dismiss_recovered(id);
                None
            }
            JobEffect::Pause(id) => {
                self.engine.pause(id);
                None
            }
            JobEffect::Resume(id) => {
                self.engine.resume(id);
                None
            }
            JobEffect::Cancel(id) => {
                self.engine.cancel(id);
                None
            }
            JobEffect::AnswerConflict(question, answer) => {
                self.resolver.answer_conflict(question, answer);
                None
            }
            JobEffect::AnswerPreflight(question, answer) => {
                self.resolver.answer_preflight(question, answer);
                None
            }
        }
    }

    /// What the history can do next.
    pub fn history(&self) -> History {
        History {
            top: self.engine.undo_history().pop(),
            can_redo: !self.engine.redo_history().is_empty(),
        }
    }

    /// Ends the engine for the app's end, in the order [`close_down`] says, and notes that it
    /// has. Blocks for up to `timeout` and a little more: call it on a worker thread.
    pub fn close_down(&self, timeout: Duration) -> bool {
        let ended = close_down(&self.engine, self.resolver.as_ref(), timeout);
        self.closed.store(true, Ordering::SeqCst);
        ended
    }

    /// Whether [`Jobs::close_down`] has run.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// The undo or redo job `id` the engine started, if it started one.
    fn history_job(&self, id: Option<JobId>, work: JobWork) -> Option<Started> {
        let id = id?;
        let spec = self
            .engine
            .jobs()
            .into_iter()
            .find(|job| job.id == id)
            .map(|job| job.spec);
        Some(Started {
            id,
            spec,
            work,
            origin: None,
            via_history: true,
        })
    }
}

/// What ends the engine: `JobEngine::shutdown`.
pub trait Shutdown {
    /// Whether a job waits for an answer and every other job a worker has taken up is paused
    /// or waits too: then no worker ends before the questions are released.
    fn only_asking(&self) -> bool;

    /// Ends every job and waits up to `timeout` for the workers; `true` if they all ended.
    fn shutdown(&self, timeout: Duration) -> bool;
}

impl Shutdown for JobEngine {
    fn only_asking(&self) -> bool {
        let jobs = self.jobs();
        let taken_up = || jobs.iter().filter(|job| job.state != JobState::Queued);
        taken_up().any(|job| job.state == JobState::Waiting)
            && taken_up().all(|job| matches!(job.state, JobState::Waiting | JobState::Paused))
    }

    fn shutdown(&self, timeout: Duration) -> bool {
        JobEngine::shutdown(self, timeout)
    }
}

/// What answers the questions still open: `UiResolver::release_all`.
pub trait ReleaseQuestions {
    fn release_all(&self);
}

impl ReleaseQuestions for UiResolver {
    fn release_all(&self) {
        UiResolver::release_all(self);
    }
}

/// The app's end (carry-m1-03 "quit order"): first the engine shuts down, which marks every
/// running job as cut off by the app's end, then the questions still open are answered with
/// their cancelling choice. In the other order a job waiting for an answer would take the
/// cancel for the user's, end for good, and not be offered again at the next start. A job
/// that waits for an answer cannot stop until it has one, so it is left in the journal, to be
/// recovered. With only such jobs (and paused ones) under way, the shutdown waits
/// [`QUIT_TIMEOUT_ASKING`] at most rather than `timeout`, as no wait lets them end; with a
/// job running beside them it waits the whole `timeout` for that one.
pub fn close_down(
    engine: &dyn Shutdown,
    questions: &dyn ReleaseQuestions,
    timeout: Duration,
) -> bool {
    let timeout = if engine.only_asking() {
        timeout.min(QUIT_TIMEOUT_ASKING)
    } else {
        timeout
    };
    let ended = engine.shutdown(timeout);
    questions.release_all();
    ended
}

/// How many finished jobs the job panel's 최근 완료 keeps: a newer one pushes the oldest out.
pub const RECENT_JOBS: usize = 20;

/// The most numbers [`save_log`] tries after a name that is taken (`… (2).log` to `… (100).log`).
pub const LOG_NAME_TRIES: u32 = 100;

/// A job's report, shared, that compares by which report it is: two are equal when they are the
/// same report (the report itself has no equality).
#[derive(Clone, Debug)]
pub struct SharedReport(pub Arc<JobReport>);

impl PartialEq for SharedReport {
    fn eq(&self, other: &SharedReport) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SharedReport {}

/// A job that ended, as the panel's 최근 완료 keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    pub work: JobWork,
    pub report: SharedReport,
}

impl Finished {
    /// Whether it has items to tell of: failures, or warnings, which its 자세히 lists.
    pub fn has_details(&self) -> bool {
        !self.report.0.failures.is_empty() || !self.report.0.warnings.is_empty()
    }
}

/// The jobs that ended, the newest first, [`RECENT_JOBS`] at most.
#[derive(Clone, Debug, Default)]
pub struct RecentJobs {
    jobs: VecDeque<Finished>,
}

impl RecentJobs {
    /// Puts the job that ended as `work` with `report` first; the oldest goes past
    /// [`RECENT_JOBS`].
    pub fn add(&mut self, work: JobWork, report: Arc<JobReport>) {
        self.jobs.push_front(Finished {
            work,
            report: SharedReport(report),
        });
        self.jobs.truncate(RECENT_JOBS);
    }

    /// The job at `index`, the newest being 0.
    pub fn get(&self, index: usize) -> Option<&Finished> {
        self.jobs.get(index)
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn clear(&mut self) {
        self.jobs.clear();
    }

    /// The rows of the list, the newest first.
    pub fn rows(&self) -> Vec<RecentRow> {
        self.jobs.iter().map(recent_row).collect()
    }
}

/// One line of 최근 완료, as the window shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentRow {
    /// How it ended: `복사했습니다`, `복사를 취소했습니다` ([`words::outcome`]).
    pub title: String,
    /// What it worked on ([`words::job_detail`]).
    pub detail: String,
    /// What it did: `복사 12개 · 건너뜀 1개 · 오류 0개 · 평균 48 MB/s · 걸린 시간 1분 20초`
    /// ([`words::job_report`]).
    pub stats: String,
    /// It has items to tell of (failures, or warnings): its 자세히 lists them.
    pub details: bool,
    /// It did all it was asked, without a failure.
    pub ok: bool,
}

/// The line of `finished` in 최근 완료.
fn recent_row(finished: &Finished) -> RecentRow {
    let report = &finished.report.0;
    RecentRow {
        title: words::outcome(report.outcome, finished.work),
        detail: words::job_detail(&report.spec),
        stats: words::job_report(
            finished.work,
            items(report.done),
            report.skipped,
            report.failures.len(),
            (report.done.bytes > 0).then(|| report.average_speed()),
            report.elapsed,
        ),
        details: finished.has_details(),
        ok: report.outcome == Outcome::Completed && report.failures.is_empty(),
    }
}

/// The lines 자세히 lists for `report`: each failure, then each warning, and why.
pub fn report_lines(report: &JobReport) -> Vec<String> {
    report
        .failures
        .iter()
        .chain(&report.warnings)
        .map(words::failure_line)
        .collect()
}

/// The name the log of `report` is saved as: `job-copy-20261030-143207.log`, by the job's kind
/// and the time it started in `tz`. The job's number is in the log's first line; two logs of
/// the same second do not meet, as [`save_log`] numbers the second one.
pub fn log_name(report: &JobReport, tz: &TimeZone) -> String {
    let started = jiff::Timestamp::try_from(report.started).map_or_else(
        |_| "0".to_owned(),
        |at| {
            at.to_zoned(tz.clone())
                .strftime("%Y%m%d-%H%M%S")
                .to_string()
        },
    );
    format!("job-{}-{started}.log", report.kind.name())
}

/// Writes the log of `report` ([`JobReport::write_log`]) into `folder` as `name`, or, when a
/// file of that name is there, as the first free one of `name (2).log` to `name (100).log`
/// ([`LOG_NAME_TRIES`]): never over a file that is there, which the file is made anew to make
/// sure of. A log that could not be written whole is taken away again. Gives the path written.
/// Blocks: call it on a worker thread.
pub fn save_log(report: &JobReport, folder: &Path, name: &str) -> io::Result<PathBuf> {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (name, None),
    };
    for number in 1..=LOG_NAME_TRIES {
        let candidate = match (number, extension) {
            (1, _) => name.to_owned(),
            (_, Some(extension)) => format!("{stem} ({number}).{extension}"),
            (_, None) => format!("{stem} ({number})"),
        };
        let path = folder.join(candidate);
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        // `write_log` buffers its records itself, and flushes them at its end.
        let written = report.write_log(&mut file).and_then(|()| file.flush());
        drop(file);
        return match written {
            Ok(()) => Ok(path),
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                Err(error)
            }
        };
    }
    Err(io::Error::from(io::ErrorKind::AlreadyExists))
}

/// The kind of `error`, met writing a log into `folder`, for its words.
pub fn log_error(folder: &Path, error: io::Error) -> rf_core::ErrorKind {
    match rf_core::VPath::local(folder) {
        Ok(path) => rf_core::Error::io("save log", path, error).kind(),
        Err(_) => rf_core::ErrorKind::Other,
    }
}

/// One row of the job panel, as the window shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct JobRow {
    pub id: JobId,
    /// `복사 중`, `일시정지됨` ([`words::job_title`]).
    pub title: String,
    /// What the job works on ([`words::job_detail`]).
    pub detail: String,
    /// How far it has got, 0 to 1: by bytes, or by items when it moves no bytes.
    pub fraction: f32,
    /// [`words::job_progress`], or what the job waits for.
    pub progress: String,
    pub paused: bool,
    /// The job waits for an answer to its question.
    pub waiting: bool,
}

/// A job that ended, for the window to tell of.
#[derive(Clone, Debug)]
pub struct Ended {
    pub id: JobId,
    pub work: JobWork,
    /// The pane that asked for it, if one did.
    pub origin: Option<usize>,
    pub report: Arc<JobReport>,
}

/// One job the window follows.
#[derive(Clone, Debug)]
struct Tracked {
    id: JobId,
    work: JobWork,
    spec: Option<Arc<JobSpec>>,
    origin: Option<usize>,
    via_history: bool,
    state: JobState,
    /// When a worker took it up, in the order the jobs started; `None` while it is queued.
    started: Option<u64>,
    progress: Progress,
}

/// The jobs that have not ended, as the job panel shows them: one row for each job a worker
/// has taken up, in the order they started; the queued ones are counted on the first row
/// (`대기 중인 작업 1개`).
#[derive(Clone, Debug, Default)]
pub struct JobRows {
    jobs: Vec<Tracked>,
    /// The number the next job to start gets.
    next_start: u64,
}

impl JobRows {
    /// Follows the job `started`, which the window submitted. Its events may come before this
    /// is called only on another thread than the UI thread's, which never happens: the events
    /// reach the UI thread after the call that submitted the job.
    pub fn track(&mut self, started: Started) {
        let Started {
            id,
            spec,
            work,
            origin,
            via_history,
        } = started;
        match self.find_mut(id) {
            Some(job) => {
                job.work = work;
                job.spec = spec.or(job.spec.take());
                job.origin = origin;
                job.via_history = via_history;
            }
            None => self.jobs.push(Tracked {
                id,
                work,
                spec,
                origin,
                via_history,
                state: JobState::Queued,
                started: None,
                progress: Progress::default(),
            }),
        }
    }

    /// Takes an event of the engine. Gives the job that ended for a `Finished` event; the job
    /// leaves the rows then. A job the window did not submit is shown as well, as an
    /// [`JobWork::Other`] job, until its report says what it was.
    pub fn apply(&mut self, event: JobEvent) -> Option<Ended> {
        match event {
            JobEvent::State { id, state } => {
                let next = self.next_start;
                let job = self.entry(id);
                job.state = state;
                let working = matches!(
                    state,
                    JobState::Planning | JobState::Running | JobState::Waiting
                );
                if working && job.started.is_none() {
                    job.started = Some(next);
                    self.next_start += 1;
                }
                None
            }
            JobEvent::Planned { id, totals, .. } => {
                if let Some(job) = self.find_mut(id) {
                    job.progress.total = totals;
                }
                None
            }
            JobEvent::Progress { id, progress } => {
                if let Some(job) = self.find_mut(id) {
                    job.progress = progress;
                }
                None
            }
            JobEvent::Finished { id, report } => {
                let job = self
                    .jobs
                    .iter()
                    .position(|job| job.id == id)
                    .map(|index| self.jobs.remove(index));
                let (work, origin) = match job {
                    Some(job) if job.work != JobWork::Other => (job.work, job.origin),
                    Some(job) => (JobWork::of(&report.spec), job.origin),
                    None => (JobWork::of(&report.spec), None),
                };
                Some(Ended {
                    id,
                    work,
                    origin,
                    report,
                })
            }
            // The queue order shows in the states; the journal's news is the window's.
            _ => None,
        }
    }

    /// The rows of the panel.
    pub fn rows(&self) -> Vec<JobRow> {
        let queued = self.jobs.iter().filter(|job| job.started.is_none()).count();
        let mut started: Vec<&Tracked> = self
            .jobs
            .iter()
            .filter(|job| job.started.is_some())
            .collect();
        started.sort_by_key(|job| job.started);
        started
            .into_iter()
            .enumerate()
            .map(|(i, job)| row(job, if i == 0 { queued } else { 0 }))
            .collect()
    }

    /// The number of jobs that have not ended, queued ones included.
    pub fn active(&self) -> usize {
        self.jobs.len()
    }

    /// Whether a job will end by itself: one that is not paused, and is not queued in a lane
    /// whose job under way is paused. A lane runs one job at a time and passes over only the
    /// queued jobs that are paused, so a paused job a worker took up holds the jobs queued
    /// behind it. The lanes are those the engine starts with ([`DefaultLanes`]); a job whose
    /// lane is not known (its spec has not come) is taken to wait behind any paused one.
    pub fn going_on(&self) -> bool {
        let lane = |job: &Tracked| job.spec.as_deref().map(|spec| DefaultLanes.lane(spec));
        let held: Vec<Option<LaneId>> = self
            .jobs
            .iter()
            .filter(|job| job.started.is_some() && job.state == JobState::Paused)
            .map(lane)
            .collect();
        self.jobs.iter().any(|job| match job.state {
            JobState::Paused | JobState::Finished(_) => false,
            JobState::Queued if job.started.is_none() => {
                let own = lane(job);
                !held
                    .iter()
                    .any(|paused| own.is_none() || paused.is_none() || *paused == own)
            }
            _ => true,
        })
    }

    /// Whether an undo or a redo of the history is queued or running: then neither starts
    /// another. As in the engine, a recovered undo run again does not count ([`Started`]).
    pub fn undo_running(&self) -> bool {
        self.jobs.iter().any(|job| job.via_history)
    }

    /// The items the job `id` has still to do after the one it works on, by its last
    /// progress.
    pub fn items_left(&self, id: JobId) -> u64 {
        self.find(id).map_or(0, |job| {
            items(job.progress.total)
                .saturating_sub(items(job.progress.done))
                .saturating_sub(1)
        })
    }

    /// What the job `id` does, while the window follows it.
    pub fn work(&self, id: JobId) -> Option<JobWork> {
        self.find(id).map(|job| job.work)
    }

    /// The id of row `row` of [`JobRows::rows`].
    pub fn row_id(&self, row: usize) -> Option<JobId> {
        self.rows().get(row).map(|view| view.id)
    }

    /// The jobs that have not ended, queued ones included, with their states.
    pub fn states(&self) -> Vec<(JobId, JobState)> {
        self.jobs.iter().map(|job| (job.id, job.state)).collect()
    }

    fn find(&self, id: JobId) -> Option<&Tracked> {
        self.jobs.iter().find(|job| job.id == id)
    }

    fn find_mut(&mut self, id: JobId) -> Option<&mut Tracked> {
        self.jobs.iter_mut().find(|job| job.id == id)
    }

    /// The job `id`, followed from now on if it was not.
    fn entry(&mut self, id: JobId) -> &mut Tracked {
        let index = if let Some(index) = self.jobs.iter().position(|job| job.id == id) {
            index
        } else {
            self.jobs.push(Tracked {
                id,
                work: JobWork::Other,
                spec: None,
                origin: None,
                via_history: false,
                state: JobState::Queued,
                started: None,
                progress: Progress::default(),
            });
            self.jobs.len() - 1
        };
        &mut self.jobs[index]
    }
}

/// The files and folders of `totals`, the items the panel counts.
fn items(totals: Totals) -> u64 {
    totals.files.saturating_add(totals.dirs)
}

/// `done / total` from 0 to 1, in steps of 1/10000; 0 when `total` is 0.
fn fraction(done: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    let steps = u128::from(done.min(total)) * 10_000 / u128::from(total);
    f32::from(u16::try_from(steps).unwrap_or(10_000)) / 10_000.0
}

/// The row of `job`, with `queued` jobs waiting behind the running ones.
fn row(job: &Tracked, queued: usize) -> JobRow {
    let Progress {
        done,
        total,
        bytes_per_sec,
        eta,
        ..
    } = &job.progress;
    let fraction = if total.bytes > 0 {
        fraction(done.bytes, total.bytes)
    } else {
        fraction(items(*done), items(*total))
    };
    let waiting = job.state == JobState::Waiting;
    let paused = job.state == JobState::Paused;
    let progress = if waiting {
        words::job_waiting().to_owned()
    } else if job.state == JobState::Planning {
        words::job_planning().to_owned()
    } else if paused {
        // Nothing moves while it is paused: no speed, and no time left.
        words::job_progress(items(*done), items(*total), 0, None, queued)
    } else {
        words::job_progress(items(*done), items(*total), *bytes_per_sec, *eta, queued)
    };
    JobRow {
        id: job.id,
        title: words::job_title(job.work, job.state),
        detail: job
            .spec
            .as_deref()
            .map(words::job_detail)
            .unwrap_or_default(),
        fraction,
        progress,
        paused,
        waiting,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use rf_core::VPath;
    use rf_jobs::{TransferMode, TransferOptions, TransferSpec};

    use super::*;

    fn path(name: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(name)).expect("a local path")
    }

    fn copy_spec() -> JobSpec {
        JobSpec::Transfer(
            TransferSpec::into_dir(
                TransferMode::Copy,
                &[path("assets")],
                &path("백업"),
                TransferOptions::default(),
            )
            .expect("a transfer"),
        )
    }

    fn started(id: u64, spec: JobSpec) -> Started {
        Started {
            id: JobId(id),
            work: JobWork::of(&spec),
            spec: Some(Arc::new(spec)),
            origin: Some(0),
            via_history: false,
        }
    }

    fn state(id: u64, state: JobState) -> JobEvent {
        JobEvent::State {
            id: JobId(id),
            state,
        }
    }

    #[test]
    fn rows_show_running_jobs_and_count_the_queue() {
        let mut rows = JobRows::default();
        rows.track(started(1, copy_spec()));
        rows.track(started(2, copy_spec()));
        rows.apply(state(1, JobState::Queued));
        rows.apply(state(2, JobState::Queued));
        // Nothing has started: no row, but two jobs are under way.
        assert_eq!(rows.rows(), []);
        assert_eq!(rows.active(), 2);

        rows.apply(state(1, JobState::Planning));
        let shown = rows.rows();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].title, "복사 중");
        assert_eq!(shown[0].progress, "준비 중");
        // The target folder by its name, as the 화면 사양 shows it.
        assert_eq!(shown[0].detail, "assets → 백업");

        rows.apply(JobEvent::Planned {
            id: JobId(1),
            totals: Totals {
                files: 12,
                dirs: 0,
                bytes: 400,
            },
            issues: 0,
        });
        rows.apply(state(1, JobState::Running));
        rows.apply(JobEvent::Progress {
            id: JobId(1),
            progress: Progress {
                done: Totals {
                    files: 3,
                    dirs: 0,
                    bytes: 100,
                },
                total: Totals {
                    files: 12,
                    dirs: 0,
                    bytes: 400,
                },
                current: None,
                bytes_per_sec: 48 * 1024 * 1024,
                eta: Some(Duration::from_secs(20)),
            },
        });
        let shown = rows.rows();
        assert!((shown[0].fraction - 0.25).abs() < 1e-6);
        assert_eq!(
            shown[0].progress,
            "3 / 12 항목 · 48 MB/s · 남은 시간 약 20초 · 대기 중인 작업 1개"
        );
        assert_eq!(rows.items_left(JobId(1)), 8);

        // Pausing changes the title and the button, and the speed and the time left, which
        // no longer hold, go.
        rows.apply(state(1, JobState::Paused));
        assert_eq!(rows.rows()[0].title, "일시정지됨");
        assert!(rows.rows()[0].paused);
        assert_eq!(rows.rows()[0].progress, "3 / 12 항목 · 대기 중인 작업 1개");
        // A question shows as waiting.
        rows.apply(state(1, JobState::Waiting));
        let shown = rows.rows();
        assert!(shown[0].waiting && !shown[0].paused);
        assert_eq!(shown[0].progress, "답을 기다리는 중");
        assert_eq!(rows.row_id(0), Some(JobId(1)));
        assert_eq!(rows.row_id(1), None);
    }

    #[test]
    fn a_finished_job_leaves_with_its_report() {
        let mut rows = JobRows::default();
        let spec = copy_spec();
        rows.track(started(7, spec.clone()));
        rows.apply(state(7, JobState::Planning));
        let report = Arc::new(JobReport::new(JobId(7), &spec));
        let ended = rows
            .apply(JobEvent::Finished {
                id: JobId(7),
                report,
            })
            .expect("the job ended");
        assert_eq!(ended.work, JobWork::Copy);
        assert_eq!(ended.origin, Some(0));
        assert_eq!(rows.active(), 0);
        assert_eq!(rows.rows(), []);

        // A job nobody tracked is named by its report.
        let report = Arc::new(JobReport::new(JobId(8), &spec));
        let ended = rows
            .apply(JobEvent::Finished {
                id: JobId(8),
                report,
            })
            .expect("the job ended");
        assert_eq!((ended.work, ended.origin), (JobWork::Copy, None));
    }

    #[test]
    fn an_undo_under_way_blocks_the_next_one() {
        let mut rows = JobRows::default();
        assert!(!rows.undo_running());
        rows.track(Started {
            id: JobId(3),
            spec: None,
            work: JobWork::Redo,
            origin: None,
            via_history: true,
        });
        assert!(rows.undo_running());
        rows.apply(state(3, JobState::Planning));
        // A redo is labelled as one, though its spec is an undo spec.
        assert_eq!(rows.rows()[0].title, "다시 실행하는 중");
        let record = UndoRecord::Renamed {
            from: path("a"),
            to: path("b"),
        };
        let spec = JobSpec::Undo(record);
        let ended = rows
            .apply(JobEvent::Finished {
                id: JobId(3),
                report: Arc::new(JobReport::new(JobId(3), &spec)),
            })
            .expect("ended");
        assert_eq!(ended.work, JobWork::Redo);
        assert!(!rows.undo_running());

        // A recovered undo run again is a job the user ran: the engine's undo does not wait for
        // it, and neither does the window's.
        rows.track(Started {
            id: JobId(4),
            spec: Some(Arc::new(spec)),
            work: JobWork::Undo,
            origin: None,
            via_history: false,
        });
        assert_eq!(rows.active(), 1);
        assert!(!rows.undo_running());
    }

    /// The report of job `id`, a copy of `copy_spec`, that ended as `outcome` after `secs`
    /// seconds having copied `files` files of `bytes` bytes.
    fn copy_report(id: u64, outcome: Outcome, files: u64, bytes: u64, secs: u64) -> JobReport {
        let mut report = JobReport::new(JobId(id), &copy_spec());
        report.outcome = outcome;
        report.done = Totals {
            files,
            dirs: 0,
            bytes,
        };
        report.elapsed = Duration::from_secs(secs);
        report
    }

    #[test]
    fn the_recent_list_keeps_the_newest_twenty() {
        let mut recent = RecentJobs::default();
        assert!(recent.is_empty());
        for id in 1..=25 {
            recent.add(
                JobWork::Copy,
                Arc::new(copy_report(id, Outcome::Completed, 1, 1, 1)),
            );
            assert_eq!(
                recent.len(),
                usize::try_from(id).unwrap_or(0).min(RECENT_JOBS)
            );
        }
        // The newest first; the five oldest have gone.
        assert_eq!(recent.len(), 20);
        assert_eq!(recent.get(0).map(|job| job.report.0.id), Some(JobId(25)));
        assert_eq!(recent.get(19).map(|job| job.report.0.id), Some(JobId(6)));
        assert_eq!(recent.get(20), None);
        assert_eq!(recent.rows().len(), 20);
        recent.clear();
        assert!(recent.is_empty());
    }

    #[test]
    fn a_recent_row_tells_what_the_job_did() {
        let mut recent = RecentJobs::default();
        let mut report = copy_report(1, Outcome::Completed, 12, 96 * 1024 * 1024, 2);
        report.skipped = 1;
        recent.add(JobWork::Copy, Arc::new(report));
        assert_eq!(
            recent.rows(),
            [RecentRow {
                title: "복사했습니다".to_owned(),
                detail: "assets → 백업".to_owned(),
                stats: "복사 12개 · 건너뜀 1개 · 오류 0개 · 평균 48 MB/s · 걸린 시간 2초"
                    .to_owned(),
                details: false,
                ok: true,
            }]
        );
        // A trash moves no bytes: no speed. A failure has its 자세히.
        let mut report = JobReport::new(
            JobId(2),
            &JobSpec::Delete(rf_jobs::DeleteSpec {
                paths: vec![path("a"), path("b")],
                method: rf_jobs::DeleteMethod::Trash,
                errors: rf_jobs::ErrorPolicy::Continue,
            }),
        );
        report.outcome = Outcome::CompletedWithErrors;
        report.done.files = 1;
        report.elapsed = Duration::from_millis(300);
        report.failures.push(rf_jobs::ItemFailure {
            source: Some(path("b")),
            target: None,
            stage: rf_jobs::Stage::Delete,
            kind: rf_core::ErrorKind::PermissionDenied,
            message: String::new(),
            reason: None,
        });
        recent.add(JobWork::Trash, Arc::new(report));
        let row = &recent.rows()[0];
        assert_eq!(
            row.stats,
            "휴지통 1개 · 건너뜀 0개 · 오류 1개 · 걸린 시간 1초 미만"
        );
        assert!(row.details && !row.ok);
        let lines = report_lines(&recent.get(0).expect("the trash").report.0);
        assert_eq!(lines, [format!("{}: 권한이 없습니다", path("b"))]);
    }

    #[test]
    fn the_log_name_says_the_job_its_kind_and_when_it_started() {
        let mut report = copy_report(12, Outcome::Completed, 1, 1, 1);
        report.started = std::time::UNIX_EPOCH + Duration::from_secs(1_793_370_727);
        assert_eq!(
            log_name(&report, &TimeZone::UTC),
            "job-copy-20261030-143207.log"
        );
    }

    #[test]
    fn a_log_is_saved_under_a_free_name_and_never_over_a_file() {
        let folder = tempfile::tempdir().expect("a temporary folder");
        let report = copy_report(3, Outcome::Completed, 2, 10, 1);
        let name = "job-copy-20261030-143207.log";
        let first = save_log(&report, folder.path(), name).expect("written");
        assert_eq!(first, folder.path().join(name));
        let mut log = Vec::new();
        report.write_log(&mut log).expect("the log");
        assert_eq!(std::fs::read(&first).expect("read back"), log);
        // A file of the name stays as it is: the next log takes a number.
        std::fs::write(&first, "mine").expect("a file of the user's");
        let second = save_log(&report, folder.path(), name).expect("written");
        assert_eq!(
            second,
            folder.path().join("job-copy-20261030-143207 (2).log")
        );
        assert_eq!(std::fs::read_to_string(&first).expect("read"), "mine");
        let third = save_log(&report, folder.path(), name).expect("written");
        assert_eq!(
            third,
            folder.path().join("job-copy-20261030-143207 (3).log")
        );
        // A folder that is not there is an error, and nothing is made.
        let gone = folder.path().join("없는 폴더");
        let error = save_log(&report, &gone, name).expect_err("no folder");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!gone.exists());
    }

    #[test]
    fn a_log_with_every_name_taken_is_not_written() {
        let folder = tempfile::tempdir().expect("a temporary folder");
        let report = copy_report(4, Outcome::Completed, 1, 1, 1);
        std::fs::write(folder.path().join("log"), "").expect("taken");
        for number in 2..=LOG_NAME_TRIES {
            std::fs::write(folder.path().join(format!("log ({number})")), "").expect("taken");
        }
        let error = save_log(&report, folder.path(), "log").expect_err("all taken");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    }

    /// Records the order of the calls [`close_down`] makes, and the time it gives the
    /// shutdown; its jobs wait only for answers when `.1` says so.
    struct Recorder<'a>(&'a RefCell<Vec<String>>, bool);

    impl Shutdown for Recorder<'_> {
        fn only_asking(&self) -> bool {
            self.1
        }

        fn shutdown(&self, timeout: Duration) -> bool {
            self.0.borrow_mut().push(format!("shutdown {timeout:?}"));
            true
        }
    }

    impl ReleaseQuestions for Recorder<'_> {
        fn release_all(&self) {
            self.0.borrow_mut().push("release".to_owned());
        }
    }

    #[test]
    fn close_shuts_engine_down_before_releasing_questions() {
        let calls = RefCell::new(Vec::new());
        let recorder = Recorder(&calls, false);
        assert!(close_down(&recorder, &recorder, QUIT_TIMEOUT));
        assert_eq!(calls.into_inner(), ["shutdown 5s", "release"]);
    }

    #[test]
    fn close_waits_little_for_jobs_that_only_wait_for_answers() {
        let calls = RefCell::new(Vec::new());
        let recorder = Recorder(&calls, true);
        assert!(close_down(&recorder, &recorder, QUIT_TIMEOUT));
        assert_eq!(calls.take(), ["shutdown 500ms", "release"]);
        // A shorter time stays as it is.
        let short = Duration::from_millis(100);
        assert!(close_down(&recorder, &recorder, short));
        assert_eq!(calls.into_inner(), ["shutdown 100ms", "release"]);
    }
}
