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
/// runs, then sends a last [`ExecEvent::Progress`]. A panic anywhere in the job, the event
/// listener's and the resolver's included, is caught and reported as a [`Stage::Internal`]
/// failure of an aborted job; the temporary file the job was writing is removed as the panic
/// unwinds. A panic of the listener on the last event is ignored. No panic leaves `execute`.
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
        ctx.stop(Stopped::Aborted);
    }
    ctx.current = None;
    // The job is over and its report is what counts; a listener that panics here loses only
    // this event.
    let _ = panic::catch_unwind(AssertUnwindSafe(|| ctx.send_progress()));
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

/// Why a job stopped before its end, if it did. A later variant outranks an earlier one, as
/// in the outcome: a job both cancelled and aborted ends as cancelled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum Stopped {
    #[default]
    No,
    /// On an error: a policy or an answer said so, or a panic.
    Aborted,
    /// A step ended with `Error::Cancelled`, whatever cancelled it.
    Cancelled,
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
    /// Whether, and why, the job stopped early.
    stopped: Stopped,
    /// The bytes [`add_bytes`](Self::add_bytes) counted for the step in progress. They stay
    /// in `report.done` only if the step works; a failed or cancelled step takes them back.
    step_bytes: u64,
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
            stopped: Stopped::No,
            step_bytes: 0,
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
    ///
    /// `report.done.bytes` keeps only the bytes of steps that worked: a step that fails or is
    /// cancelled takes back what it counted, so a file tried again is counted once. The
    /// progress shown can step back by that much. What a step run inside this one counted
    /// stays once that inner step has worked.
    pub(crate) fn attempt<T>(
        &mut self,
        policy: ErrorPolicy,
        source: Option<&VPath>,
        target: Option<&VPath>,
        op: impl FnMut(&mut Ctx<'e>) -> Result<T, Failed>,
    ) -> Attempt<T> {
        // An outer step's own count is set aside while this one runs.
        let outer = std::mem::take(&mut self.step_bytes);
        let attempt = self.attempt_step(policy, source, target, op);
        self.step_bytes = outer;
        attempt
    }

    /// [`attempt`](Self::attempt), with the outer step's byte count set aside.
    fn attempt_step<T>(
        &mut self,
        policy: ErrorPolicy,
        source: Option<&VPath>,
        target: Option<&VPath>,
        mut op: impl FnMut(&mut Ctx<'e>) -> Result<T, Failed>,
    ) -> Attempt<T> {
        loop {
            let result = op(self);
            let counted = std::mem::take(&mut self.step_bytes);
            let failed = match result {
                Ok(value) => return Attempt::Done(value),
                Err(failed) => failed,
            };
            self.report.done.bytes = self.report.done.bytes.saturating_sub(counted);
            if failed.error.kind() == ErrorKind::Cancelled {
                self.stop(Stopped::Cancelled);
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
                    self.stop(Stopped::Aborted);
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
        self.step_bytes += n;
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
    /// `Waiting(false)` is sent even when `f` panics; the panic then goes on.
    pub(crate) fn waiting<T>(&self, f: impl FnOnce() -> T) -> T {
        self.emit(ExecEvent::Waiting(true));
        // Caught and resumed rather than sent from a drop guard: the event is sent outside
        // any unwinding, so a listener that panics too is one plain panic, not a nested one.
        let answer = panic::catch_unwind(AssertUnwindSafe(f));
        self.emit(ExecEvent::Waiting(false));
        answer.unwrap_or_else(|payload| panic::resume_unwind(payload))
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
        if self.env.control.is_cancelled() {
            self.stop(Stopped::Cancelled);
        }
        self.report.outcome = match self.stopped {
            Stopped::Cancelled => Outcome::Cancelled,
            Stopped::Aborted => Outcome::Aborted,
            Stopped::No if self.report.failures.is_empty() => Outcome::Completed,
            Stopped::No => Outcome::CompletedWithErrors,
        };
        self.report.elapsed = started.elapsed();
        self.report
    }

    /// Notes that the job stopped early, and why; a cancel outranks an abort.
    fn stop(&mut self, why: Stopped) {
        self.stopped = self.stopped.max(why);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::resolver::NonInteractive;
    use crate::spec::{TransferMode, TransferOptions, TransferSpec};
    use crate::testkit::ScriptedResolver;

    /// A copy of `missing.txt`, which does not exist, under `errors`.
    fn copy_of_a_missing_file(errors: ErrorPolicy) -> (tempfile::TempDir, JobSpec) {
        let dir = tempfile::tempdir().unwrap();
        let root = VPath::local(dir.path()).unwrap();
        let options = TransferOptions {
            errors,
            ..TransferOptions::default()
        };
        let sources = [root.join("missing.txt").unwrap()];
        let spec = TransferSpec::into_dir(TransferMode::Copy, &sources, &root, options).unwrap();
        (dir, JobSpec::Transfer(spec))
    }

    fn empty_copy() -> JobSpec {
        JobSpec::Transfer(TransferSpec {
            mode: TransferMode::Copy,
            items: Vec::new(),
            options: TransferOptions::default(),
        })
    }

    fn unsupported(what: &str) -> Failed {
        Failed::new(Stage::Write, Error::Unsupported(what.to_owned()))
    }

    #[test]
    fn a_panicking_listener_never_escapes_execute() {
        let (_dir, job) = copy_of_a_missing_file(ErrorPolicy::Continue);
        let vfs = Vfs::with_local();
        let on = |_: ExecEvent| panic!("listener");
        let env = ExecEnv::new(&vfs, &NonInteractive).on_event(&on);
        let report = execute(JobId(1), &job, &env);
        assert_eq!(report.outcome, Outcome::Aborted);
        let last = report.failures.last().unwrap();
        assert_eq!(
            (last.stage, last.message.as_str()),
            (Stage::Internal, "internal error: listener")
        );
    }

    #[test]
    fn a_panicking_resolver_still_ends_the_wait() {
        let (_dir, job) = copy_of_a_missing_file(ErrorPolicy::Ask);
        let vfs = Vfs::with_local();
        // It has no answer, so the question panics.
        let resolver = ScriptedResolver::new();
        let waits = Mutex::new(Vec::new());
        let on = |event: ExecEvent| {
            if let ExecEvent::Waiting(waiting) = event {
                waits.lock().unwrap().push(waiting);
            }
        };
        let report = execute(JobId(1), &job, &ExecEnv::new(&vfs, &resolver).on_event(&on));
        assert_eq!(waits.into_inner().unwrap(), [true, false]);
        assert_eq!(
            (report.outcome, report.failures[0].stage),
            (Outcome::Aborted, Stage::Internal)
        );
    }

    #[test]
    fn a_cancelled_step_cancels_the_job_whatever_cancelled_it() {
        let (vfs, spec) = (Vfs::with_local(), empty_copy());
        let env = ExecEnv::new(&vfs, &NonInteractive);
        let mut ctx = Ctx::new(JobId(1), &env, JobReport::new(JobId(1), &spec));
        let attempt = ctx.attempt(ErrorPolicy::Continue, None, None, |_| {
            Err::<(), _>(Failed::new(Stage::Write, Error::Cancelled))
        });
        assert!(matches!(attempt, Attempt::Stop));
        assert!(!env.control.is_cancelled());
        assert_eq!(ctx.finish(Instant::now()).outcome, Outcome::Cancelled);
    }

    #[test]
    fn a_failed_step_takes_back_only_its_own_bytes() {
        let (vfs, spec) = (Vfs::with_local(), empty_copy());
        let env = ExecEnv::new(&vfs, &NonInteractive);
        let mut ctx = Ctx::new(JobId(1), &env, JobReport::new(JobId(1), &spec));
        let path = VPath::local(std::env::temp_dir()).unwrap();
        let outer = ctx.attempt(ErrorPolicy::Continue, None, None, |ctx| {
            ctx.add_bytes(5, &path, false);
            let worked = ctx.attempt(ErrorPolicy::Continue, None, None, |ctx| {
                ctx.add_bytes(3, &path, false);
                Ok(())
            });
            assert!(matches!(worked, Attempt::Done(())));
            let failed = ctx.attempt(ErrorPolicy::Continue, None, None, |ctx| {
                ctx.add_bytes(7, &path, false);
                Err::<(), _>(unsupported("inner"))
            });
            assert!(matches!(failed, Attempt::Skipped));
            assert_eq!(ctx.report.done.bytes, 5 + 3);
            Err::<(), _>(unsupported("outer"))
        });
        assert!(matches!(outer, Attempt::Skipped));
        assert_eq!(ctx.report.done.bytes, 3);
    }
}
