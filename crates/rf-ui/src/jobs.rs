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

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use rf_jobs::{
    ConflictAnswer, EngineConfig, JobEngine, JobEvent, JobId, JobReport, JobSpec, JobState,
    PreflightAnswer, Progress, Totals, UndoRecord,
};
use rf_vfs::Vfs;

use crate::resolver::{Question, QuestionId, UiResolver};
use crate::words::{self, JobWork};

/// How long the app's end waits for the running jobs to stop before it releases their
/// questions and ends (see [`close_down`]).
pub const QUIT_TIMEOUT: Duration = Duration::from_secs(5);

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
    ) -> std::io::Result<Jobs> {
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
    /// Ends every job and waits up to `timeout` for the workers; `true` if they all ended.
    fn shutdown(&self, timeout: Duration) -> bool;
}

impl Shutdown for JobEngine {
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
/// that waits for an answer cannot stop until it has one, so with a question open the
/// shutdown waits its whole `timeout`; the job is then left in the journal, to be recovered.
pub fn close_down(
    engine: &dyn Shutdown,
    questions: &dyn ReleaseQuestions,
    timeout: Duration,
) -> bool {
    let ended = engine.shutdown(timeout);
    questions.release_all();
    ended
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

    /// The id of row `row` of [`JobRows::rows`].
    pub fn row_id(&self, row: usize) -> Option<JobId> {
        self.rows().get(row).map(|view| view.id)
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
    let progress = if waiting {
        words::job_waiting().to_owned()
    } else if job.state == JobState::Planning {
        words::job_planning().to_owned()
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
        paused: job.state == JobState::Paused,
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

        // Pausing changes the title and the button; a question shows as waiting.
        rows.apply(state(1, JobState::Paused));
        assert_eq!(rows.rows()[0].title, "일시정지됨");
        assert!(rows.rows()[0].paused);
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

    /// Records the order of the calls [`close_down`] makes.
    struct Recorder<'a>(&'a RefCell<Vec<&'static str>>);

    impl Shutdown for Recorder<'_> {
        fn shutdown(&self, timeout: Duration) -> bool {
            assert_eq!(timeout, QUIT_TIMEOUT);
            self.0.borrow_mut().push("shutdown");
            true
        }
    }

    impl ReleaseQuestions for Recorder<'_> {
        fn release_all(&self) {
            self.0.borrow_mut().push("release");
        }
    }

    #[test]
    fn close_shuts_engine_down_before_releasing_questions() {
        let calls = RefCell::new(Vec::new());
        let recorder = Recorder(&calls);
        assert!(close_down(&recorder, &recorder, QUIT_TIMEOUT));
        assert_eq!(calls.into_inner(), ["shutdown", "release"]);
    }
}
