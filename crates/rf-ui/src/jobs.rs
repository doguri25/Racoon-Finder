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
//! ([`AppState::job_event`](crate::state::AppState::job_event)). A job a worker has taken up has
//! a row with the file it works on now and, for one that moves bytes, its speed over the last
//! minute ([`SpeedSamples`], a sample a second, which [`speed_points`] turns into the line of
//! its graph); a job that waits has a row in the order of the queue, where it can be moved
//! ([`JobRows::move_target`], XFER-01).
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
use std::time::{Duration, Instant, SystemTime};

use jiff::tz::TimeZone;
use rf_jobs::{
    ConflictAnswer, DefaultLanes, EngineConfig, ErrorAnswer, JobEngine, JobEvent, JobId, JobReport,
    JobSpec, JobState, LaneId, LanePolicy, Outcome, PreflightAnswer, Progress, Totals, UndoRecord,
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
    /// The lanes of the engine's config.
    lanes: Arc<dyn LanePolicy>,
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
    /// Move the queued job `id` to `position` in the queue, 0 being next
    /// (`JobEngine::move_to`). A job that no longer waits there stays as it is.
    MoveTo {
        id: JobId,
        position: usize,
    },
    /// The answer to a conflict question.
    AnswerConflict(QuestionId, ConflictAnswer),
    /// The answer to the question about an item that failed (XFER-09, 파일마다 묻기).
    AnswerError(QuestionId, ErrorAnswer),
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

/// What the history can do next, as the toolbar shows it, and what it holds, as the job
/// panel's 되돌리기 기록 lists it (OPS-12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct History {
    /// The record [`JobEngine::undo`] undoes next.
    pub top: Option<UndoRecord>,
    /// Whether there is anything to redo.
    pub can_redo: bool,
    /// What can be undone, the oldest first: the last one is undone next
    /// (`JobEngine::undo_history`).
    pub undo: Vec<UndoRecord>,
    /// What can be done again, the oldest first: the last one is redone next.
    pub redo: Vec<UndoRecord>,
    /// When the history was read: a record not seen before is taken to be as old as this. The
    /// epoch for a history that does not tell, which then shows no times.
    pub at: SystemTime,
}

impl Default for History {
    fn default() -> History {
        History {
            top: None,
            can_redo: false,
            undo: Vec::new(),
            redo: Vec::new(),
            at: SystemTime::UNIX_EPOCH,
        }
    }
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
        let lanes = Arc::clone(&config.lanes);
        let (engine, events) = JobEngine::start(config);
        // The bridge waits for it; it ends only after this.
        let _ = hand_over.send(events);
        Ok(Jobs {
            engine,
            resolver,
            lanes,
            closed: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Which lane the engine runs each job in: the rule of its config, for the window to tell
    /// which queued jobs wait behind a paused one ([`JobRows::set_lanes`]).
    pub fn lanes(&self) -> Arc<dyn LanePolicy> {
        Arc::clone(&self.lanes)
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
            JobEffect::MoveTo { id, position } => {
                self.engine.move_to(id, position);
                None
            }
            JobEffect::AnswerConflict(question, answer) => {
                self.resolver.answer_conflict(question, answer);
                None
            }
            JobEffect::AnswerError(question, answer) => {
                self.resolver.answer_error(question, answer);
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
        let undo = self.engine.undo_history();
        let redo = self.engine.redo_history();
        History {
            top: undo.last().cloned(),
            can_redo: !redo.is_empty(),
            undo,
            redo,
            at: SystemTime::now(),
        }
    }

    /// Ends the engine for the app's end, in the order [`close_down`] says, and notes that it
    /// has. Blocks for up to `timeout` and [`rf_jobs::JOURNAL_GRACE`] more, for the journal's
    /// last records: call it on a worker thread.
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

/// How many speed samples a job keeps for its graph: its last minute, a sample a second.
pub const SPEED_SAMPLES: usize = 60;

/// The time between two speed samples.
pub const SPEED_SAMPLE_EVERY: Duration = Duration::from_secs(1);

/// The speed of a job over its last minute of work, for the graph of its row: a ring of the
/// last [`SPEED_SAMPLES`] samples, one for each second, each the bytes the job moved in that
/// second. The time comes from the caller ([`SpeedSamples::sample`]), so nothing here reads a
/// clock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpeedSamples {
    /// The samples, the oldest first.
    ring: VecDeque<u64>,
    /// When the bytes were last looked at, and how many the job had done then.
    last: Option<(Instant, u64)>,
}

impl SpeedSamples {
    /// Adds the sample `bytes_per_sec`; the oldest one goes past [`SPEED_SAMPLES`].
    pub fn push(&mut self, bytes_per_sec: u64) {
        if self.ring.len() == SPEED_SAMPLES {
            self.ring.pop_front();
        }
        self.ring.push_back(bytes_per_sec);
    }

    /// Looks at the job at `now`, when it has done `bytes_done` bytes in all, and takes the
    /// samples that are due: one for each second since the last look, to the nearest second,
    /// each the bytes per second over that time (a look that comes late, after a timer the
    /// system held back, fills the seconds it missed with their average). The first look only
    /// notes where the job is, and so does one that comes within half a second of the last.
    ///
    /// While the job does not work (`working` is false: it is paused, plans, or waits for an
    /// answer), no sample is taken and the time does not count: the graph shows the seconds
    /// the job worked. Bytes taken back (a failed attempt) count as none moved. `true` if a
    /// sample was added.
    pub fn sample(&mut self, now: Instant, bytes_done: u64, working: bool) -> bool {
        let Some((at, bytes)) = self.last else {
            self.last = Some((now, bytes_done));
            return false;
        };
        if !working {
            self.last = Some((now, bytes_done));
            return false;
        }
        let millis = now.saturating_duration_since(at).as_millis();
        let every = SPEED_SAMPLE_EVERY.as_millis();
        let due = usize::try_from((millis + every / 2) / every).unwrap_or(usize::MAX);
        if due == 0 {
            return false;
        }
        let moved = u128::from(bytes_done.saturating_sub(bytes));
        let rate = u64::try_from(moved * 1_000 / millis).unwrap_or(u64::MAX);
        // More than a ring of them would push each other out again.
        for _ in 0..due.min(SPEED_SAMPLES) {
            self.push(rate);
        }
        self.last = Some((now, bytes_done));
        true
    }

    /// The samples, the oldest first.
    pub fn samples(&self) -> Vec<u64> {
        self.ring.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}

/// The points of the line that draws `samples` (speeds, the oldest first) in a box `width` by
/// `height`: one point for each sample, the newest at the right edge and each older one a step
/// to its left, a step being the box's width over the gaps of a full ring
/// ([`SPEED_SAMPLES`]` - 1`), so that the line grows from the right as the samples come and a
/// second is as wide whatever their number. The fastest sample is at the top (`y` 0) and a
/// speed of 0 at the bottom (`y` is `height`), the others between them by their share of the
/// fastest, to a ten-thousandth; when every sample is 0, all are at the bottom. Of more
/// samples than a ring holds, the newest ones are drawn. No point for no sample.
pub fn speed_points(samples: &[u64], width: f32, height: f32) -> Vec<(f32, f32)> {
    let shown = &samples[samples.len().saturating_sub(SPEED_SAMPLES)..];
    let top = shown.iter().copied().max().unwrap_or(0);
    let step = width / small(SPEED_SAMPLES - 1);
    shown
        .iter()
        .enumerate()
        .map(|(i, &sample)| {
            let from_right = small(shown.len() - 1 - i);
            (
                width - from_right * step,
                height * (1.0 - fraction(sample, top)),
            )
        })
        .collect()
}

/// `count` as a float; exact, for it is at most [`SPEED_SAMPLES`].
fn small(count: usize) -> f32 {
    f32::from(u16::try_from(count).unwrap_or(u16::MAX))
}

/// Which way a job that waits in the queue can be moved: whether its 위로 and its 아래로 do
/// anything ([`JobRows::move_target`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QueueMoves {
    /// It can go a place earlier: it is not the first of those that wait, and the engine has
    /// told its place in the queue and that of the job above it.
    pub up: bool,
    /// It can go a place later: it is not the last, and the engine has told its place and
    /// that of the job below it.
    pub down: bool,
}

/// One row of the job panel, as the window shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct JobRow {
    pub id: JobId,
    /// `복사 중`, `일시정지됨`, `대기 중` ([`words::job_title`]).
    pub title: String,
    /// What the job works on ([`words::job_detail`]); for a job that waits, after what it will
    /// do ([`words::queued_detail`]).
    pub detail: String,
    /// How far it has got, 0 to 1: by bytes, or by items when it moves no bytes.
    pub fraction: f32,
    /// [`words::job_progress`], or what the job waits for; empty for a job in the queue.
    pub progress: String,
    pub paused: bool,
    /// The job waits for an answer to its question.
    pub waiting: bool,
    /// The name of the file it works on now, a long one shortened in the middle
    /// ([`words::job_current`]); empty while none is known, and for a job in the queue.
    pub current: String,
    /// Its speed over its last minute of work, a sample a second, the oldest first
    /// ([`SpeedSamples`]): the graph of its row. `None` for a job that moves no bytes (a trash,
    /// a rename), which has no graph, and for a job in the queue.
    pub speeds: Option<Vec<u64>>,
    /// For a job that waits in the queue, which way it can be moved there; `None` once a
    /// worker has taken it up. The jobs that wait come after the others, in the queue's order.
    pub queued: Option<QueueMoves>,
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
    /// Its speed over its last minute of work ([`JobRows::sample_speeds`]).
    speeds: SpeedSamples,
}

/// The jobs that have not ended, as the job panel shows them: one row for each job a worker
/// has taken up, in the order they started, the first of which also counts the queued ones
/// (`대기 중인 작업 1개`); then one row for each job that waits, in the order of the queue.
#[derive(Clone, Debug, Default)]
pub struct JobRows {
    jobs: Vec<Tracked>,
    /// The number the next job to start gets.
    next_start: u64,
    /// The engine's lanes ([`JobRows::set_lanes`]).
    lanes: Lanes,
    /// The queued jobs in the order they wait in, as the engine last told (`JobEvent::Queue`).
    queue: Vec<JobId>,
}

/// Which lane each job runs in, as the engine's config says: [`DefaultLanes`], the engine's
/// own default, until the window is given the engine's.
#[derive(Clone, Debug)]
struct Lanes(Arc<dyn LanePolicy>);

impl Default for Lanes {
    fn default() -> Lanes {
        Lanes(Arc::new(DefaultLanes))
    }
}

impl JobRows {
    /// Takes the lanes the engine runs the jobs in ([`Jobs::lanes`]), which
    /// [`JobRows::going_on`] follows.
    pub fn set_lanes(&mut self, lanes: Arc<dyn LanePolicy>) {
        self.lanes = Lanes(lanes);
    }

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
                speeds: SpeedSamples::default(),
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
            JobEvent::Queue { order } => {
                self.queue = order;
                None
            }
            // The journal's news is the window's.
            _ => None,
        }
    }

    /// The rows of the panel: the jobs a worker has taken up, then those that wait.
    pub fn rows(&self) -> Vec<JobRow> {
        let waiting = self.waiting();
        let mut started: Vec<&Tracked> = self
            .jobs
            .iter()
            .filter(|job| job.started.is_some())
            .collect();
        started.sort_by_key(|job| job.started);
        let started = started
            .into_iter()
            .enumerate()
            .map(|(i, job)| row(job, if i == 0 { waiting.len() } else { 0 }));
        let queued = waiting.iter().enumerate().map(|(at, job)| {
            let moves = QueueMoves {
                up: self.target(&waiting, at, true).is_some(),
                down: self.target(&waiting, at, false).is_some(),
            };
            queued_row(job, moves)
        });
        started.chain(queued).collect()
    }

    /// The jobs no worker has taken up, in the order they wait in: as the engine told the
    /// queue, and after them, as they were submitted, those it has not told of yet.
    fn waiting(&self) -> Vec<&Tracked> {
        let mut waiting: Vec<&Tracked> = self
            .jobs
            .iter()
            .filter(|job| job.started.is_none())
            .collect();
        // Stable: the jobs the queue does not name keep their order, after the others.
        waiting.sort_by_key(|job| {
            self.queue
                .iter()
                .position(|id| *id == job.id)
                .unwrap_or(usize::MAX)
        });
        waiting
    }

    /// Where the job of row `row` goes when it is moved a place up the queue (`up`) or down:
    /// its id and its new position in the engine's queue (`JobEngine::move_to`), which puts it
    /// just before the job that waits above it, or just after the one below. `None` for a row
    /// that cannot move: a job a worker has taken up (running, paused or asking: it has left
    /// the queue), the first one going up, the last one going down, a row that is not there,
    /// and a job the engine has not told the place of yet.
    pub fn move_target(&self, row: usize, up: bool) -> Option<(JobId, usize)> {
        let waiting = self.waiting();
        let at = row.checked_sub(self.jobs.len() - waiting.len())?;
        self.target(&waiting, at, up)
    }

    /// [`JobRows::move_target`] for the job at `at` of `waiting`, the jobs that wait in their
    /// order ([`JobRows::waiting`]).
    fn target(&self, waiting: &[&Tracked], at: usize, up: bool) -> Option<(JobId, usize)> {
        let job = waiting.get(at)?;
        let beside = if up {
            waiting.get(at.checked_sub(1)?)?
        } else {
            waiting.get(at.checked_add(1)?)?
        };
        // Both have a place in the queue the engine told.
        let place = |id: JobId| self.queue.iter().position(|queued| *queued == id);
        place(job.id)?;
        Some((job.id, place(beside.id)?))
    }

    /// The row of the job `id` while it waits in the queue.
    pub fn queued_row(&self, id: JobId) -> Option<usize> {
        let waiting = self.waiting();
        let at = waiting.iter().position(|job| job.id == id)?;
        Some(self.jobs.len() - waiting.len() + at)
    }

    /// Takes the speed samples that are due at `now` of every job a worker has taken up that
    /// moves bytes, the jobs whose rows have a graph ([`SpeedSamples::sample`]): a job works
    /// while it runs, and not while it is paused or waits for an answer. `true` if a graph has
    /// a new sample to show.
    pub fn sample_speeds(&mut self, now: Instant) -> bool {
        let mut sampled = false;
        let graphed = |job: &&mut Tracked| job.started.is_some() && job.progress.total.bytes > 0;
        for job in self.jobs.iter_mut().filter(graphed) {
            let working = job.state == JobState::Running;
            sampled |= job.speeds.sample(now, job.progress.done.bytes, working);
        }
        sampled
    }

    /// The number of jobs that have not ended, queued ones included.
    pub fn active(&self) -> usize {
        self.jobs.len()
    }

    /// Whether a job will end by itself: one that is not paused, and is not queued in a lane
    /// whose job under way is paused. A lane runs one job at a time and passes over only the
    /// queued jobs that are paused, so a paused job a worker took up holds the jobs queued
    /// behind it. The lanes are the engine's ([`JobRows::set_lanes`]); a job whose lane is not
    /// known (its spec has not come) is taken to wait behind any paused one.
    pub fn going_on(&self) -> bool {
        let lane = |job: &Tracked| job.spec.as_deref().map(|spec| self.lanes.0.lane(spec));
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
                speeds: SpeedSamples::default(),
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
        detail: detail(job),
        fraction,
        progress,
        paused,
        waiting,
        current: job
            .progress
            .current
            .as_ref()
            .map(words::job_current)
            .unwrap_or_default(),
        speeds: (total.bytes > 0).then(|| job.speeds.samples()),
        queued: None,
    }
}

/// What `job` works on ([`words::job_detail`]), once its spec is known.
fn detail(job: &Tracked) -> String {
    job.spec
        .as_deref()
        .map(words::job_detail)
        .unwrap_or_default()
}

/// The row of `job`, which waits in the queue and can be moved as `moves` says: `대기 중`, or
/// `일시정지됨` for one its lane passes over, with what it will do and on what.
fn queued_row(job: &Tracked, moves: QueueMoves) -> JobRow {
    JobRow {
        id: job.id,
        title: words::job_title(job.work, job.state),
        detail: words::queued_detail(job.work, &detail(job)),
        fraction: 0.0,
        progress: String::new(),
        paused: job.state == JobState::Paused,
        waiting: false,
        current: String::new(),
        speeds: None,
        queued: Some(moves),
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
        // Nothing has started: both wait, each with a row that says what it will do.
        let shown = rows.rows();
        assert_eq!(shown.len(), 2);
        for row in &shown {
            assert_eq!(row.title, "대기 중");
            assert_eq!(row.detail, "복사 · assets → 백업");
            // Neither moves until the engine has told the queue's order.
            let stays = QueueMoves {
                up: false,
                down: false,
            };
            assert_eq!(row.queued, Some(stays));
            assert_eq!((row.progress.as_str(), row.current.as_str()), ("", ""));
            assert_eq!(row.speeds, None);
        }
        assert_eq!(rows.active(), 2);

        rows.apply(state(1, JobState::Planning));
        let shown = rows.rows();
        // The job a worker took up comes first; the other one still waits.
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0].title, "복사 중");
        assert_eq!(shown[0].progress, "준비 중");
        assert_eq!(shown[0].queued, None);
        // The target folder by its name, as the 화면 사양 shows it.
        assert_eq!(shown[0].detail, "assets → 백업");
        assert!(shown[1].queued.is_some());

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
        // The job that waits has the row after it.
        assert_eq!(rows.row_id(1), Some(JobId(2)));
        assert_eq!(rows.row_id(2), None);
    }

    /// The rows of four copies: job 1 runs, jobs 2, 3 and 4 wait in that order.
    fn one_running_three_waiting() -> JobRows {
        let mut rows = JobRows::default();
        for id in 1..=4 {
            rows.track(started(id, copy_spec()));
            rows.apply(state(id, JobState::Queued));
        }
        rows.apply(state(1, JobState::Planning));
        rows.apply(state(1, JobState::Running));
        rows.apply(JobEvent::Queue {
            order: vec![JobId(2), JobId(3), JobId(4)],
        });
        rows
    }

    /// The ids of the rows, in their order.
    fn ids(rows: &JobRows) -> Vec<u64> {
        rows.rows().iter().map(|row| row.id.0).collect()
    }

    #[test]
    fn the_jobs_that_wait_show_in_the_order_of_the_queue() {
        let mut rows = one_running_three_waiting();
        assert_eq!(ids(&rows), [1, 2, 3, 4]);
        // The engine tells a new order: the rows follow it.
        rows.apply(JobEvent::Queue {
            order: vec![JobId(4), JobId(2), JobId(3)],
        });
        assert_eq!(ids(&rows), [1, 4, 2, 3]);
        // The running job has no place in the queue; of those that wait, the first cannot go
        // up and the last cannot go down.
        let moves = |rows: &JobRows| -> Vec<Option<(bool, bool)>> {
            rows.rows()
                .iter()
                .map(|row| row.queued.map(|moves| (moves.up, moves.down)))
                .collect()
        };
        assert_eq!(
            moves(&rows),
            [
                None,
                Some((false, true)),
                Some((true, true)),
                Some((true, false))
            ]
        );
        assert_eq!(rows.queued_row(JobId(2)), Some(2));
        assert_eq!(rows.queued_row(JobId(1)), None, "it runs");
        // A job submitted that the queue has not told of yet comes last, and neither it nor
        // the job above it moves past the other until it has.
        rows.track(started(5, copy_spec()));
        assert_eq!(ids(&rows), [1, 4, 2, 3, 5]);
        assert_eq!(
            moves(&rows)[3..],
            [Some((true, false)), Some((false, false))]
        );
        rows.apply(JobEvent::Queue {
            order: vec![JobId(4), JobId(2), JobId(3), JobId(5)],
        });
        assert_eq!(moves(&rows)[3..], [Some((true, true)), Some((true, false))]);
        // One that is paused while it waits keeps its place, and says so.
        rows.apply(state(2, JobState::Paused));
        let paused = &rows.rows()[2];
        assert_eq!((paused.id, paused.title.as_str()), (JobId(2), "일시정지됨"));
        assert!(paused.paused && paused.queued.is_some());
    }

    #[test]
    fn only_a_job_that_waits_moves_and_only_within_the_queue() {
        let mut rows = one_running_three_waiting();
        // Rows: 1 runs; 2, 3, 4 wait at the engine's places 0, 1, 2.
        // The running job does not move, either way.
        assert_eq!(rows.move_target(0, true), None);
        assert_eq!(rows.move_target(0, false), None);
        // The first one that waits cannot go up, the last one cannot go down.
        assert_eq!(rows.move_target(1, true), None);
        assert_eq!(rows.move_target(3, false), None);
        // The others go to the place of the job beside them.
        assert_eq!(rows.move_target(1, false), Some((JobId(2), 1)));
        assert_eq!(rows.move_target(2, true), Some((JobId(3), 0)));
        assert_eq!(rows.move_target(2, false), Some((JobId(3), 2)));
        assert_eq!(rows.move_target(3, true), Some((JobId(4), 1)));
        // No row there.
        assert_eq!(rows.move_target(4, true), None);
        assert_eq!(rows.move_target(usize::MAX, false), None);

        // A job paused in mid-run has left the queue too: it does not move.
        rows.apply(state(1, JobState::Paused));
        assert_eq!(rows.move_target(0, false), None);
        // One paused while it waits still does.
        rows.apply(state(3, JobState::Paused));
        assert_eq!(rows.move_target(2, true), Some((JobId(3), 0)));

        // A worker takes job 2 up; the queue's news has not come yet. Job 3 is first of those
        // that wait now and cannot go up, and going down it goes behind job 4, by the place
        // job 4 has in the queue as it was told.
        rows.apply(state(2, JobState::Planning));
        assert_eq!(ids(&rows), [1, 2, 3, 4]);
        assert_eq!(rows.move_target(1, false), None, "job 2 runs now");
        assert_eq!(rows.move_target(2, true), None);
        assert_eq!(rows.move_target(2, false), Some((JobId(3), 2)));
        rows.apply(JobEvent::Queue {
            order: vec![JobId(3), JobId(4)],
        });
        assert_eq!(rows.move_target(2, false), Some((JobId(3), 1)));

        // A job that ended has no row, and nothing moves in its place.
        let spec = copy_spec();
        rows.apply(JobEvent::Queue {
            order: vec![JobId(3)],
        });
        rows.apply(JobEvent::Finished {
            id: JobId(4),
            report: Arc::new(JobReport::new(JobId(4), &spec)),
        });
        assert_eq!(ids(&rows), [1, 2, 3]);
        assert_eq!(rows.move_target(2, true), None);
        assert_eq!(rows.move_target(2, false), None);
        assert_eq!(rows.move_target(3, true), None);

        // A job the engine has not told the place of does not move, nor does one move past it.
        rows.track(started(9, copy_spec()));
        assert_eq!(ids(&rows), [1, 2, 3, 9]);
        assert_eq!(rows.move_target(3, true), None);
        assert_eq!(rows.move_target(2, false), None);
    }

    /// A progress report of job `id` with `bytes` of `total` bytes done, at the file `current`.
    fn progress_at(id: u64, bytes: u64, total: u64, current: Option<VPath>) -> JobEvent {
        JobEvent::Progress {
            id: JobId(id),
            progress: Progress {
                done: Totals {
                    files: 0,
                    dirs: 0,
                    bytes,
                },
                total: Totals {
                    files: 4,
                    dirs: 0,
                    bytes: total,
                },
                current,
                bytes_per_sec: 0,
                eta: None,
            },
        }
    }

    #[test]
    fn a_running_row_names_the_file_it_works_on() {
        let mut rows = JobRows::default();
        rows.track(started(1, copy_spec()));
        rows.apply(state(1, JobState::Planning));
        assert_eq!(rows.rows()[0].current, "", "none while it plans");
        rows.apply(state(1, JobState::Running));
        rows.apply(progress_at(
            1,
            10,
            400,
            Some(path("assets").join("로고.png").expect("a name")),
        ));
        // The name, without its folders.
        assert_eq!(rows.rows()[0].current, "로고.png");
        // A long name keeps its beginning and its extension.
        let long = format!("{}-최종.psd", "아주-긴-이름".repeat(12));
        rows.apply(progress_at(1, 20, 400, Some(path(&long))));
        let shown = rows.rows()[0].current.clone();
        assert!(shown.starts_with("아주-긴-이름"), "{shown}");
        assert!(shown.ends_with("-최종.psd"), "{shown}");
        assert!(shown.contains('…'), "{shown}");
        assert!(
            words::text_cells(&shown) <= words::JOB_FILE_CELLS,
            "{shown}"
        );
        // It stays while the job is paused, and goes with the job's last report.
        rows.apply(state(1, JobState::Paused));
        assert_eq!(rows.rows()[0].current, shown);
        rows.apply(progress_at(1, 400, 400, None));
        assert_eq!(rows.rows()[0].current, "");
    }

    #[test]
    fn the_ring_keeps_the_last_sixty_samples() {
        let mut ring = SpeedSamples::default();
        assert!(ring.is_empty());
        assert_eq!(ring.samples(), Vec::<u64>::new());
        for speed in 1..=u64::try_from(SPEED_SAMPLES + 5).expect("a small number") {
            ring.push(speed);
        }
        assert_eq!(ring.len(), SPEED_SAMPLES);
        let samples = ring.samples();
        // The oldest five have gone; the newest is last.
        assert_eq!(samples.first(), Some(&6));
        assert_eq!(samples.last(), Some(&65));
        assert!(samples.windows(2).all(|pair| pair[1] == pair[0] + 1));
    }

    #[test]
    fn a_sample_is_taken_for_each_second_of_work() {
        const MB: u64 = 1024 * 1024;
        // Every time below counts from this one; none is compared with the clock.
        let start = Instant::now();
        let at = |millis: u64| start + Duration::from_millis(millis);
        let mut ring = SpeedSamples::default();
        // The first look only notes where the job is.
        assert!(!ring.sample(at(0), 0, true));
        assert!(ring.is_empty());
        // A second later: what it moved in that second.
        assert!(ring.sample(at(1_000), 4 * MB, true));
        assert_eq!(ring.samples(), [4 * MB]);
        // A look that comes early, within half a second, takes nothing and loses nothing.
        assert!(!ring.sample(at(1_400), 5 * MB, true));
        assert_eq!(ring.samples(), [4 * MB]);
        assert!(ring.sample(at(2_000), 6 * MB, true));
        assert_eq!(ring.samples(), [4 * MB, 2 * MB]);
        // A timer the system held back for three seconds: three samples of their average.
        assert!(ring.sample(at(5_000), 12 * MB, true));
        assert_eq!(ring.samples(), [4 * MB, 2 * MB, 2 * MB, 2 * MB, 2 * MB]);
        // A tick that is a little late is one sample, of the speed over the time it took.
        assert!(ring.sample(at(6_250), 17 * MB, true));
        assert_eq!(ring.samples().last(), Some(&(4 * MB)));
        // A stall shows as a second of nothing.
        assert!(ring.sample(at(7_250), 17 * MB, true));
        assert_eq!(ring.samples().last(), Some(&0));
        // Bytes taken back after a failed attempt are none moved.
        assert!(ring.sample(at(8_250), 16 * MB, true));
        assert_eq!(ring.samples().last(), Some(&0));
        let before = ring.len();
        // While the job does not work (paused, asking) no sample is taken, however long, and
        // the time does not count toward the next one.
        assert!(!ring.sample(at(60_000), 16 * MB, false));
        assert!(!ring.sample(at(120_000), 16 * MB, false));
        assert_eq!(ring.len(), before);
        assert!(ring.sample(at(121_000), 19 * MB, true));
        assert_eq!(ring.len(), before + 1);
        assert_eq!(ring.samples().last(), Some(&(3 * MB)));
        // A very long gap fills the ring at most.
        assert!(ring.sample(at(1_000_000), 19 * MB, true));
        assert_eq!(ring.len(), SPEED_SAMPLES);
        assert!(ring.samples().iter().all(|&sample| sample == 0));
    }

    #[test]
    fn the_rows_sample_the_jobs_that_run_and_only_copies_have_a_graph() {
        const MB: u64 = 1024 * 1024;
        let start = Instant::now();
        let at = |secs: u64| start + Duration::from_secs(secs);
        let mut rows = JobRows::default();
        rows.track(started(1, copy_spec()));
        rows.track(Started {
            id: JobId(2),
            spec: None,
            work: JobWork::Trash,
            origin: None,
            via_history: false,
        });
        rows.track(started(3, copy_spec()));
        rows.apply(state(1, JobState::Planning));
        rows.apply(state(2, JobState::Running));
        // Before its plan the copy has no bytes to move yet, and no graph; a trash never has.
        assert_eq!(rows.rows()[0].speeds, None);
        assert!(!rows.sample_speeds(at(0)), "nothing to sample yet");
        rows.apply(state(1, JobState::Running));
        rows.apply(progress_at(1, 0, 100 * MB, None));
        assert_eq!(rows.rows()[0].speeds, Some(Vec::new()));
        assert!(
            !rows.sample_speeds(at(1)),
            "the first look of a running job"
        );
        rows.apply(progress_at(1, 8 * MB, 100 * MB, None));
        assert!(rows.sample_speeds(at(2)));
        rows.apply(progress_at(1, 10 * MB, 100 * MB, None));
        assert!(rows.sample_speeds(at(3)));
        let shown = rows.rows();
        assert_eq!(shown[0].speeds, Some(vec![8 * MB, 2 * MB]));
        // A trash moves no bytes: no graph, though it runs.
        assert_eq!((shown[1].id, &shown[1].speeds), (JobId(2), &None));
        // The copy that waits has none either, and is not sampled.
        assert_eq!((shown[2].id, &shown[2].speeds), (JobId(3), &None));
        // Paused, the copy takes no sample; going on, it measures from there.
        rows.apply(state(1, JobState::Paused));
        assert!(!rows.sample_speeds(at(30)));
        assert_eq!(rows.rows()[0].speeds.as_ref().map(Vec::len), Some(2));
        rows.apply(state(1, JobState::Running));
        rows.apply(progress_at(1, 13 * MB, 100 * MB, None));
        assert!(rows.sample_speeds(at(31)));
        assert_eq!(rows.rows()[0].speeds, Some(vec![8 * MB, 2 * MB, 3 * MB]));
    }

    /// Whether `a` and `b` are the same to a hundredth.
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn no_sample_makes_no_point_and_one_sample_one_at_the_right_edge() {
        assert_eq!(speed_points(&[], 120.0, 28.0), []);
        // The one sample is the fastest: at the top, where the newest one always is.
        let points = speed_points(&[5], 120.0, 28.0);
        assert_eq!(points.len(), 1);
        assert!(
            close(points[0].0, 120.0) && close(points[0].1, 0.0),
            "{points:?}"
        );
    }

    #[test]
    fn samples_of_zero_lie_on_the_bottom() {
        let points = speed_points(&[0, 0, 0], 120.0, 28.0);
        assert_eq!(points.len(), 3);
        for (x, y) in &points {
            assert!(y.is_finite() && x.is_finite());
            assert!(close(*y, 28.0), "{points:?}");
        }
        // One of them alone too: nothing divides by a top speed of 0.
        let one = speed_points(&[0], 120.0, 28.0);
        assert!(close(one[0].1, 28.0), "{one:?}");
    }

    #[test]
    fn the_line_is_normalised_to_the_fastest_sample_and_grows_from_the_right() {
        let points = speed_points(&[0, 50, 100, 25], 120.0, 28.0);
        assert_eq!(points.len(), 4);
        // The fastest at the top, half of it halfway, a quarter three quarters down.
        let ys: Vec<f32> = points.iter().map(|point| point.1).collect();
        for (y, wanted) in ys.iter().zip([28.0, 14.0, 0.0, 21.0]) {
            assert!(close(*y, wanted), "{ys:?}");
        }
        // The newest at the right edge, each older one a step of a sixtieth's gap to its left.
        let step = 120.0 / 59.0;
        let xs: Vec<f32> = points.iter().map(|point| point.0).collect();
        for (x, back) in xs.iter().zip([3.0_f32, 2.0, 1.0, 0.0]) {
            assert!(close(*x, 120.0 - back * step), "{xs:?}");
        }
        // The same speeds a thousand times faster draw the same line.
        let faster = speed_points(&[0, 50_000, 100_000, 25_000], 120.0, 28.0);
        for (a, b) in points.iter().zip(&faster) {
            assert!(close(a.0, b.0) && close(a.1, b.1));
        }
        // A full ring spans the box; of more samples, the newest are drawn.
        let full: Vec<u64> = (1..=60).collect();
        let points = speed_points(&full, 120.0, 28.0);
        assert_eq!(points.len(), SPEED_SAMPLES);
        assert!(close(points[0].0, 0.0) && close(points[59].0, 120.0));
        assert!(close(points[59].1, 0.0));
        let more: Vec<u64> = (1..=70).collect();
        let points = speed_points(&more, 120.0, 28.0);
        assert_eq!(points.len(), SPEED_SAMPLES);
        assert!(close(points[0].0, 0.0) && close(points[59].1, 0.0));
        // The oldest drawn is sample 11 of 70, a little above the bottom.
        assert!(
            close(points[0].1, 28.0 * (1.0 - 11.0 / 70.0)),
            "{:?}",
            points[0]
        );
        // A box of another size scales with it.
        let wide = speed_points(&[10, 20], 59.0, 10.0);
        assert!(close(wide[0].0, 58.0) && close(wide[0].1, 5.0), "{wide:?}");
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

    /// Every job in one lane.
    #[derive(Debug)]
    struct OneLane;

    impl LanePolicy for OneLane {
        fn lane(&self, _spec: &JobSpec) -> LaneId {
            LaneId::MAIN
        }
    }

    #[test]
    fn the_jobs_go_on_by_the_lanes_of_the_engine() {
        // A copy under way is paused; a rename waits. The engine's usual lanes run a rename in
        // a lane of its own, so it goes on; an engine that runs everything in one lane holds it
        // behind the copy.
        let rename = JobSpec::Rename {
            from: path("a"),
            to: path("b"),
        };
        let rows_with = |lanes: Option<Arc<dyn LanePolicy>>| {
            let mut rows = JobRows::default();
            if let Some(lanes) = lanes {
                rows.set_lanes(lanes);
            }
            for (id, spec, work) in [
                (1, copy_spec(), JobWork::Copy),
                (2, rename.clone(), JobWork::Rename),
            ] {
                rows.track(Started {
                    id: JobId(id),
                    spec: Some(Arc::new(spec)),
                    work,
                    origin: None,
                    via_history: false,
                });
            }
            rows.apply(state(1, JobState::Running));
            rows.apply(state(1, JobState::Paused));
            rows.apply(state(2, JobState::Queued));
            rows
        };
        assert!(rows_with(None).going_on());
        assert!(rows_with(Some(Arc::new(DefaultLanes))).going_on());
        assert!(!rows_with(Some(Arc::new(OneLane))).going_on());
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
