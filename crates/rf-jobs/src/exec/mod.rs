//! Running one job to its end on the calling thread: plan, run, report.

mod file;
mod scan;
mod transfer;

use std::any::Any;
use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Instant;

use rf_core::{Error, ErrorKind, Os, VPath};
use rf_vfs::{Provider, Vfs};

use crate::control::Control;
use crate::id::JobId;
use crate::naming::temp_name;
use crate::progress::{PROGRESS_INTERVAL, Progress, SPEED_WINDOW, SpeedMeter};
use crate::report::{ItemFailure, JobReport, Outcome, Stage, Totals};
use crate::resolver::{ErrorAction, Resolver};
use crate::spec::{ErrorPolicy, JobKind, JobSpec};

/// The size of the buffer a file is copied through, and so of one progress step.
pub(crate) const COPY_CHUNK: usize = 1 << 20;

/// What a running job tells the one who started it, on the job's own thread.
#[derive(Clone, Debug)]
pub enum ExecEvent {
    /// The plan is made: what the job expects to process, and how many problems the
    /// pre-flight check found on the target.
    Planned { totals: Totals, issues: usize },
    /// How far the job has got. Sent at most every [`PROGRESS_INTERVAL`], except right after
    /// the first chunk of a big file, and once more when the job ends.
    Progress(Progress),
    /// `true` while the job waits for an answer from its [`Resolver`], `false` once it has one.
    Waiting(bool),
}

/// What a job runs with: where it finds the providers, who answers its questions, which
/// naming rule it follows, its remote control and who hears about its progress.
pub struct ExecEnv<'a> {
    vfs: &'a Vfs,
    resolver: &'a dyn Resolver,
    naming_os: Os,
    control: Control,
    on_event: Option<&'a dyn Fn(ExecEvent)>,
}

impl<'a> ExecEnv<'a> {
    /// The environment that finds providers in `vfs` and asks `resolver`, with the naming rule
    /// of this OS, a new [`Control`] and nobody listening for events.
    pub fn new(vfs: &'a Vfs, resolver: &'a dyn Resolver) -> ExecEnv<'a> {
        ExecEnv {
            vfs,
            resolver,
            naming_os: Os::CURRENT,
            control: Control::new(),
            on_event: None,
        }
    }

    /// Names new items by the rule of `os` (`a (2).txt` or `a 2.txt`).
    #[must_use]
    pub fn naming_os(mut self, os: Os) -> Self {
        self.naming_os = os;
        self
    }

    /// Pauses, resumes and cancels the job through `control`.
    #[must_use]
    pub fn control(mut self, control: Control) -> Self {
        self.control = control;
        self
    }

    /// Calls `f` with every [`ExecEvent`], on the job's thread. The job holds none of its own
    /// locks while it calls `f`, so `f` may pause or cancel the job.
    #[must_use]
    pub fn on_event(mut self, f: &'a dyn Fn(ExecEvent)) -> Self {
        self.on_event = Some(f);
        self
    }
}

impl fmt::Debug for ExecEnv<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecEnv")
            .field("vfs", self.vfs)
            .field("naming_os", &self.naming_os)
            .field("control", &self.control)
            .field("on_event", &self.on_event.is_some())
            .finish_non_exhaustive()
    }
}

/// Runs the job `spec` as job `id` to its end and reports what happened. It blocks: call it
/// on a worker thread, never on the UI thread.
///
/// The job plans first (it counts what it will do and sends [`ExecEvent::Planned`]), then
/// runs, then sends a last [`ExecEvent::Progress`]. A panic anywhere in the job is caught
/// and reported as a [`Stage::Internal`] failure of an aborted job; the temporary file the
/// job was writing is removed as the panic unwinds.
///
/// The outcome is [`Outcome::Cancelled`] if the job was cancelled, [`Outcome::Aborted`] if
/// it stopped on an error, [`Outcome::CompletedWithErrors`] if some items failed, and
/// [`Outcome::Completed`] otherwise.
pub fn execute(id: JobId, spec: &JobSpec, env: &ExecEnv<'_>) -> JobReport {
    let started = Instant::now();
    let mut ctx = Ctx::new(id, env, JobReport::new(id, spec));
    let run = panic::catch_unwind(AssertUnwindSafe(|| run(&mut ctx, spec)));
    if let Err(payload) = run {
        ctx.report.failures.push(ItemFailure {
            source: None,
            target: None,
            stage: Stage::Internal,
            kind: ErrorKind::Other,
            message: format!("internal error: {}", panic_text(payload.as_ref())),
        });
        ctx.aborted = true;
    }
    ctx.current = None;
    ctx.send_progress();
    ctx.finish(started)
}

/// Sends `spec` to the executor of its kind.
fn run(ctx: &mut Ctx<'_>, spec: &JobSpec) {
    match spec {
        JobSpec::Transfer(transfer) => transfer::run_transfer(ctx, transfer),
        JobSpec::Delete(_) => ctx.not_yet("delete jobs are not implemented yet"),
        JobSpec::Rename { .. } => ctx.not_yet("rename jobs are not implemented yet"),
        JobSpec::Create { .. } => ctx.not_yet("create jobs are not implemented yet"),
    }
}

/// The text of a panic payload: the message of `panic!` with a `&str` or a `String`.
fn panic_text(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic")
}

/// An error and the step of the job it happened in.
#[derive(Debug)]
pub(crate) struct Failed {
    pub stage: Stage,
    pub error: Error,
}

impl Failed {
    pub(crate) fn new(stage: Stage, error: Error) -> Failed {
        Failed { stage, error }
    }
}

/// Tags the error of a result with the step it happened in.
pub(crate) trait At<T> {
    fn at(self, stage: Stage) -> Result<T, Failed>;
}

impl<T> At<T> for rf_core::Result<T> {
    fn at(self, stage: Stage) -> Result<T, Failed> {
        self.map_err(|error| Failed::new(stage, error))
    }
}

/// How [`Ctx::attempt`] ended.
#[derive(Debug)]
pub(crate) enum Attempt<T> {
    /// The step worked.
    Done(T),
    /// The step failed and the job goes on without it; the failure is in the report.
    Skipped,
    /// The job stops: it was cancelled or aborted.
    Stop,
}

/// The state of one running job, handed to every step.
pub(crate) struct Ctx<'e> {
    pub id: JobId,
    pub env: &'e ExecEnv<'e>,
    pub report: JobReport,
    /// Whether the job may ask its resolver. When it may not, an `Ask` error policy skips the
    /// failed item, as `Continue` does. Every job but an undo is interactive.
    pub interactive: bool,
    /// The source being worked on, for the progress events.
    current: Option<VPath>,
    /// When the last progress event was sent; `None` before the first.
    last_progress: Option<Instant>,
    meter: SpeedMeter,
    /// Every later failure is skipped without asking ("skip all").
    skip_failures: bool,
    /// The job stopped on an error (a policy or an answer said so, or a panic).
    aborted: bool,
    /// The number the next temporary file gets.
    temp_seq: u64,
}

impl<'e> Ctx<'e> {
    fn new(id: JobId, env: &'e ExecEnv<'e>, report: JobReport) -> Ctx<'e> {
        Ctx {
            id,
            env,
            interactive: report.kind != JobKind::Undo,
            report,
            current: None,
            last_progress: None,
            meter: SpeedMeter::new(SPEED_WINDOW),
            skip_failures: false,
            aborted: false,
            temp_seq: 0,
        }
    }

    /// Waits while the job is paused; fails with `Error::Cancelled` once it is cancelled.
    pub(crate) fn check(&self) -> rf_core::Result<()> {
        self.env.control.checkpoint()
    }

    /// The provider that serves `path`.
    pub(crate) fn provider(&self, path: &VPath) -> rf_core::Result<Arc<dyn Provider>> {
        self.env.vfs.provider(path)
    }

    /// Runs one step of the job and applies the error policy when it fails.
    ///
    /// A cancelled step stops the job. Any other failure is recorded and skipped under
    /// `Continue` (or after the user chose "skip all"), recorded and stops the job under
    /// `Abort`, and under `Ask` is put to the resolver (if the job is interactive; otherwise
    /// it is skipped): `Retry` runs `op` again without recording the failure, `Skip` records
    /// and skips it (for every later failure too with `apply_to_all`), `Abort` records it and
    /// stops the job.
    pub(crate) fn attempt<T>(
        &mut self,
        policy: ErrorPolicy,
        source: Option<&VPath>,
        target: Option<&VPath>,
        mut op: impl FnMut(&mut Ctx<'e>) -> Result<T, Failed>,
    ) -> Attempt<T> {
        loop {
            let failed = match op(self) {
                Ok(value) => return Attempt::Done(value),
                Err(failed) => failed,
            };
            if failed.error.kind() == ErrorKind::Cancelled {
                return Attempt::Stop;
            }
            let failure = ItemFailure::new(failed.stage, source, target, &failed.error);
            let action = match policy {
                _ if self.skip_failures => ErrorAction::Skip,
                ErrorPolicy::Continue => ErrorAction::Skip,
                ErrorPolicy::Abort => ErrorAction::Abort,
                ErrorPolicy::Ask if !self.interactive => ErrorAction::Skip,
                ErrorPolicy::Ask => {
                    let answer = self.waiting(|| self.env.resolver.error(self.id, &failure));
                    if answer.action == ErrorAction::Skip && answer.apply_to_all {
                        self.skip_failures = true;
                    }
                    answer.action
                }
            };
            match action {
                ErrorAction::Retry => {}
                ErrorAction::Skip => {
                    self.report.failures.push(failure);
                    return Attempt::Skipped;
                }
                ErrorAction::Abort => {
                    self.report.failures.push(failure);
                    self.aborted = true;
                    return Attempt::Stop;
                }
            }
        }
    }

    /// Records that a step went wrong without failing its item, such as copying metadata.
    pub(crate) fn warn(&mut self, failed: Failed, source: Option<&VPath>, target: Option<&VPath>) {
        let Failed { stage, error } = failed;
        let warning = ItemFailure::new(stage, source, target, &error);
        self.report.warnings.push(warning);
    }

    /// A new temporary file next to `target`: `.rf-{job}-{seq}.rfpart` in its folder, with
    /// `seq` counting from 0 in each job.
    pub(crate) fn temp_for(&mut self, target: &VPath) -> rf_core::Result<VPath> {
        let parent = target.parent().ok_or_else(|| {
            Error::Unsupported(format!("no folder to write a temporary file in: {target}"))
        })?;
        let seq = self.temp_seq;
        self.temp_seq += 1;
        Ok(parent.join(temp_name(self.id, seq))?)
    }

    /// Records what the plan expects to process and announces it.
    pub(crate) fn planned(&mut self, totals: Totals) {
        self.report.planned = totals;
        self.emit(ExecEvent::Planned {
            totals,
            issues: self.report.issues.len(),
        });
    }

    /// Counts `n` more bytes written from `current`. Sends a progress event right away for the
    /// first chunk of a file larger than one chunk, and otherwise when one is due.
    pub(crate) fn add_bytes(&mut self, n: u64, current: &VPath, first_chunk_of_large: bool) {
        self.report.done.bytes += n;
        self.working_on(current);
        if first_chunk_of_large || self.progress_due() {
            self.send_progress();
        }
    }

    /// Counts a finished file (a file or a link) or folder, and sends a progress event when
    /// one is due.
    pub(crate) fn item_done(&mut self, file: bool, dir: bool) {
        self.report.done.files += u64::from(file);
        self.report.done.dirs += u64::from(dir);
        if self.progress_due() {
            self.send_progress();
        }
    }

    /// Runs `f`, a question to the resolver, between `Waiting(true)` and `Waiting(false)`.
    pub(crate) fn waiting<T>(&self, f: impl FnOnce() -> T) -> T {
        self.emit(ExecEvent::Waiting(true));
        let answer = f();
        self.emit(ExecEvent::Waiting(false));
        answer
    }

    /// Names `source` in the progress events from now on.
    fn working_on(&mut self, source: &VPath) {
        if self.current.as_ref() != Some(source) {
            self.current = Some(source.clone());
        }
    }

    /// Records a part of the job this version cannot do yet as a failure of its plan.
    fn not_yet(&mut self, what: &str) {
        let error = Error::Unsupported(what.to_owned());
        let failure = ItemFailure::new(Stage::Plan, None, None, &error);
        self.report.failures.push(failure);
    }

    /// Whether [`PROGRESS_INTERVAL`] has passed since the last progress event.
    fn progress_due(&self) -> bool {
        self.last_progress
            .is_none_or(|sent| sent.elapsed() >= PROGRESS_INTERVAL)
    }

    /// Sends the progress as it is now. The total never falls below what is done, so a
    /// source that grew after the plan does not show more than 100 %.
    fn send_progress(&mut self) {
        let now = Instant::now();
        self.last_progress = Some(now);
        let (done, planned) = (self.report.done, self.report.planned);
        let total = Totals {
            files: planned.files.max(done.files),
            dirs: planned.dirs.max(done.dirs),
            bytes: planned.bytes.max(done.bytes),
        };
        self.meter.record(now, done.bytes);
        let progress = Progress {
            done,
            total,
            current: self.current.clone(),
            bytes_per_sec: self.meter.bytes_per_sec(),
            eta: self.meter.eta(total.bytes - done.bytes),
        };
        self.emit(ExecEvent::Progress(progress));
    }

    fn emit(&self, event: ExecEvent) {
        if let Some(on_event) = self.env.on_event {
            on_event(event);
        }
    }

    /// The report of the finished job, with its outcome and the time since `started`.
    fn finish(mut self, started: Instant) -> JobReport {
        self.report.outcome = if self.env.control.is_cancelled() {
            Outcome::Cancelled
        } else if self.aborted {
            Outcome::Aborted
        } else if !self.report.failures.is_empty() {
            Outcome::CompletedWithErrors
        } else {
            Outcome::Completed
        };
        self.report.elapsed = started.elapsed();
        self.report
    }
}
