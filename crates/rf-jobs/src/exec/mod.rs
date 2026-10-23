//! Running one job to its end on the calling thread: plan, run, report.

mod file;
mod scan;
mod transfer;

use std::any::Any;
use std::ffi::OsStr;
use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Instant;

use rf_core::{Entry, Error, ErrorKind, FileId, Os, PathError, VPath, VolumeId};
use rf_vfs::{Provider, Vfs};

use crate::conflict::{Conflict, ConflictAction};
use crate::control::Control;
use crate::id::JobId;
use crate::naming::temp_name;
use crate::preflight::{FsKind, IssueKind, fold_name};
use crate::progress::{PROGRESS_INTERVAL, Progress, SPEED_WINDOW, SpeedMeter};
use crate::report::{ItemFailure, JobReport, Outcome, Stage, Totals};
use crate::resolver::{ErrorAction, Resolver};
use crate::spec::{ErrorPolicy, JobKind, JobSpec};

/// The size of the buffer a file is copied through, and so of one progress step.
pub(crate) const COPY_CHUNK: usize = 1 << 20;

/// How deep a transfer goes into folders: the items it was given are at depth 0, their
/// children at 1. An item deeper than this is not counted in the plan and fails as
/// [`Stage::Plan`] when the job reaches it; a folder there is not entered. It ends a loop of
/// links that Follow mode would walk forever, well inside the 1,023-byte path limit of macOS.
pub const MAX_DEPTH: usize = 128;

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

/// Why a step failed.
#[derive(Debug)]
pub(crate) enum Cause {
    /// An error of a provider, of a path or of the job.
    Error(Error),
    /// Something the target file system cannot hold.
    Issue(IssueKind),
}

impl Cause {
    /// The kind a failure of this cause has: the error's, or for an issue
    /// [`ErrorKind::FileTooLarge`] when the file is too large and [`ErrorKind::InvalidPath`]
    /// for anything else.
    pub(crate) fn kind(&self) -> ErrorKind {
        match self {
            Cause::Error(error) => error.kind(),
            Cause::Issue(IssueKind::FileTooLarge { .. }) => ErrorKind::FileTooLarge,
            Cause::Issue(_) => ErrorKind::InvalidPath,
        }
    }
}

/// The log text: the error's, or the issue's own text with nothing in front.
impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Cause::Error(error) => error.fmt(f),
            Cause::Issue(issue) => issue.fmt(f),
        }
    }
}

impl From<Error> for Cause {
    fn from(error: Error) -> Cause {
        Cause::Error(error)
    }
}

impl From<PathError> for Cause {
    fn from(error: PathError) -> Cause {
        Cause::Error(error.into())
    }
}

/// Why a step failed, and the step of the job it happened in.
#[derive(Debug)]
pub(crate) struct Failed {
    pub stage: Stage,
    pub cause: Cause,
    /// The path that was being written, when it is not the target the step was run for (a
    /// numbered name). The failure names it as its target. Boxed, so that every `Result` that
    /// fails with a `Failed` stays small.
    pub target: Option<Box<VPath>>,
}

impl Failed {
    pub(crate) fn new(stage: Stage, error: Error) -> Failed {
        Failed {
            stage,
            cause: Cause::Error(error),
            target: None,
        }
    }

    /// The failure of an item the target cannot hold because of `issue`.
    pub(crate) fn issue(stage: Stage, issue: IssueKind) -> Failed {
        Failed {
            stage,
            cause: Cause::Issue(issue),
            target: None,
        }
    }

    /// Names `path` as the path that was being written, unless a path is named already.
    #[must_use]
    pub(crate) fn writing(mut self, path: &VPath) -> Failed {
        self.target.get_or_insert_with(|| Box::new(path.clone()));
        self
    }

    /// The record of this failure for a step on `source` and `target`; the path that was being
    /// written, if one is named, is the target instead.
    pub(crate) fn record(self, source: Option<&VPath>, target: Option<&VPath>) -> ItemFailure {
        ItemFailure {
            source: source.cloned(),
            target: self.target.map(|path| *path).or_else(|| target.cloned()),
            stage: self.stage,
            kind: self.cause.kind(),
            message: self.cause.to_string(),
        }
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

impl<T> At<T> for Result<T, Cause> {
    fn at(self, stage: Stage) -> Result<T, Failed> {
        self.map_err(|cause| Failed {
            stage,
            cause,
            target: None,
        })
    }
}

/// The names of `path` from its root, each as its [`fold_name`] key on `fs`, after its scheme
/// and authority as they are. Two paths of one provider that `fs` takes for one path have the
/// same keys, and the keys of a path inside another start with the other's. Only meaningful on
/// a file system with [`FsKind::case_insensitive`].
pub(crate) fn fold_path(path: &VPath, fs: FsKind) -> Vec<String> {
    let mut keys = vec![
        path.scheme().name().to_owned(),
        path.authority().unwrap_or_default().to_owned(),
    ];
    match path.as_local() {
        Some(local) => keys.extend(local.components().map(|c| fold_name(c.as_os_str(), fs))),
        None => keys.extend(
            path.segments()
                .unwrap_or_default()
                .iter()
                .map(|segment| fold_name(OsStr::new(segment), fs)),
        ),
    }
    keys
}

/// The volume and file ID of an item.
pub(crate) type Identity = (VolumeId, FileId);

/// One folder a transfer's walk has entered, and through `up` the folders above it.
#[derive(Debug)]
pub(crate) struct Above<'a> {
    /// How deep the folder is: an item of the job is at 0.
    depth: usize,
    /// The identity of the folder, when the walk follows links and the provider knows it.
    identity: Option<Identity>,
    up: Option<&'a Above<'a>>,
}

impl Above<'_> {
    /// The level of the items in this folder.
    pub(crate) fn below(&self) -> Level<'_> {
        Level(Some(self))
    }
}

/// Where an item is in a transfer's walk: how deep, and which folders are above it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Level<'a>(Option<&'a Above<'a>>);

impl<'a> Level<'a> {
    /// The level of an item of the job itself.
    pub(crate) const TOP: Level<'static> = Level(None);

    /// How far below an item of the job this is: 0 for the item itself.
    pub(crate) fn depth(self) -> usize {
        self.0.map_or(0, |above| above.depth + 1)
    }

    /// Whether a folder with `identity` is one of the folders above, so that entering it
    /// would lead back up (a loop of links). An unknown identity never matches.
    pub(crate) fn leads_back(self, identity: Option<Identity>) -> bool {
        let Some(identity) = identity else {
            return false;
        };
        let mut above = self.0;
        while let Some(folder) = above {
            if folder.identity == Some(identity) {
                return true;
            }
            above = folder.up;
        }
        false
    }

    /// The folder at this level, with `identity`, as the walk enters it.
    pub(crate) fn enter(self, identity: Option<Identity>) -> Above<'a> {
        Above {
            depth: self.depth(),
            identity,
            up: self.0,
        }
    }
}

/// The identity the folder `path` of `provider`, which `entry` describes, is entered with:
/// none unless links are followed (only a followed link can lead back to a folder above),
/// else the entry's, or the one `stat` gives for a listed folder that has none (a Windows
/// listing).
pub(crate) fn folder_identity(
    follow: bool,
    provider: &dyn Provider,
    path: &VPath,
    entry: &Entry,
) -> Option<Identity> {
    if !follow {
        return None;
    }
    entry
        .identity()
        .or_else(|| provider.stat(path).ok()?.identity())
}

/// The error of a followed folder that is one of the folders above it.
pub(crate) fn link_loop(source: &VPath) -> Error {
    Error::Unsupported(format!(
        "folder link loop: {source} leads back to a folder above it"
    ))
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
    /// The answer to every later conflict, once the user chose "apply to all".
    conflict_for_all: Option<ConflictAction>,
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
            conflict_for_all: None,
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
            if failed.cause.kind() == ErrorKind::Cancelled {
                self.stop(Stopped::Cancelled);
                return Attempt::Stop;
            }
            let failure = failed.record(source, target);
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

    /// What to do about `conflict`, an item whose target name is taken and whose policy is to
    /// ask.
    ///
    /// The answer the user gave with "apply to all" comes back for every later conflict of the
    /// job without a question. Otherwise an interactive job asks its resolver (between
    /// `Waiting(true)` and `Waiting(false)`), and a job that may not ask (an undo) answers
    /// [`ConflictAction::Skip`]. The answer is only what the user wants: the caller still
    /// refuses an `Overwrite` that would replace a folder or put a file over one.
    ///
    /// A `Cancel` answer cancels the job's control and fails with `Error::Cancelled`, so the
    /// step that asked stops the job.
    pub(crate) fn resolve_conflict(
        &mut self,
        conflict: &Conflict,
    ) -> rf_core::Result<ConflictAction> {
        let action = if let Some(action) = self.conflict_for_all {
            action
        } else if self.interactive {
            let answer = self.waiting(|| self.env.resolver.conflict(self.id, conflict));
            if answer.apply_to_all {
                self.conflict_for_all = Some(answer.action);
            }
            answer.action
        } else {
            ConflictAction::Skip
        };
        if action == ConflictAction::Cancel {
            self.env.control.cancel();
            return Err(Error::Cancelled);
        }
        Ok(action)
    }

    /// Records that a step went wrong without failing its item, such as copying metadata.
    pub(crate) fn warn(&mut self, failed: Failed, source: Option<&VPath>, target: Option<&VPath>) {
        let warning = failed.record(source, target);
        self.report.warnings.push(warning);
    }

    /// Every child of the folder `dir` of `provider`. All the batches are gathered before the
    /// caller uses any of them, so that a change to the folder while the job works through it
    /// cannot shake the list. A cancel of the job stops the listing with `Error::Cancelled`.
    pub(crate) fn list(&self, provider: &dyn Provider, dir: &VPath) -> rf_core::Result<Vec<Entry>> {
        let mut children = Vec::new();
        let cancel = self.env.control.cancel_token();
        provider.list(dir, &cancel, &mut |batch| children.extend(batch))?;
        Ok(children)
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

    use rf_core::{Entry, EntryKind};

    use super::*;
    use crate::conflict::ConflictAnswer;
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
    fn a_failure_on_an_issue_has_the_issue_text_and_its_own_kind() {
        let dir = VPath::local(std::env::temp_dir()).unwrap();
        let (taken, numbered) = (dir.join("a.txt").unwrap(), dir.join("a (2).txt").unwrap());
        let too_large = IssueKind::FileTooLarge { size: 5, max: 4 };
        let failure = Failed::issue(Stage::Preflight, too_large.clone()).record(None, Some(&taken));
        assert_eq!(
            (failure.kind, failure.message, failure.target),
            (
                ErrorKind::FileTooLarge,
                too_large.to_string(),
                Some(taken.clone())
            )
        );
        let bad = IssueKind::InvalidCharacter('?');
        let failure = Failed::issue(Stage::Preflight, bad.clone())
            .writing(&numbered)
            .writing(&taken)
            .record(None, Some(&taken));
        // The first path named is the one that was being written.
        assert_eq!(
            (failure.kind, failure.message, failure.target),
            (ErrorKind::InvalidPath, bad.to_string(), Some(numbered))
        );
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

    /// A conflict over one name, as a file meeting a file.
    fn clash() -> Conflict {
        let path = VPath::local(std::env::temp_dir())
            .unwrap()
            .join("a.txt")
            .unwrap();
        Conflict {
            source: path.clone(),
            target: path,
            incoming: Entry::new("a.txt", EntryKind::File),
            existing: Entry::new("a.txt", EntryKind::File),
        }
    }

    #[test]
    fn a_job_that_may_not_ask_skips_every_conflict() {
        let (vfs, spec) = (Vfs::with_local(), empty_copy());
        // It has no answer, so it would panic if it were asked.
        let resolver = ScriptedResolver::new();
        let env = ExecEnv::new(&vfs, &resolver);
        let mut ctx = Ctx::new(JobId(1), &env, JobReport::new(JobId(1), &spec));
        ctx.interactive = false;
        for _ in 0..2 {
            assert_eq!(
                ctx.resolve_conflict(&clash()).unwrap(),
                ConflictAction::Skip
            );
        }
        assert_eq!(resolver.calls(), Vec::new());
    }

    #[test]
    fn an_answer_for_all_settles_the_later_conflicts_unasked() {
        let (vfs, spec) = (Vfs::with_local(), empty_copy());
        let resolver = ScriptedResolver::new()
            .on_conflict(ConflictAnswer::once(ConflictAction::Overwrite))
            .on_conflict(ConflictAnswer::all(ConflictAction::Rename));
        let env = ExecEnv::new(&vfs, &resolver);
        let mut ctx = Ctx::new(JobId(1), &env, JobReport::new(JobId(1), &spec));
        let answers: Vec<_> = (0..4)
            .map(|_| ctx.resolve_conflict(&clash()).unwrap())
            .collect();
        // An answer for one conflict is asked again; the one for all is not.
        assert_eq!(
            answers,
            [
                ConflictAction::Overwrite,
                ConflictAction::Rename,
                ConflictAction::Rename,
                ConflictAction::Rename
            ]
        );
        assert_eq!(resolver.calls().len(), 2);
    }

    #[test]
    fn a_cancel_answer_cancels_the_control_and_the_step() {
        let (vfs, spec) = (Vfs::with_local(), empty_copy());
        let resolver =
            ScriptedResolver::new().on_conflict(ConflictAnswer::once(ConflictAction::Cancel));
        let env = ExecEnv::new(&vfs, &resolver);
        let mut ctx = Ctx::new(JobId(1), &env, JobReport::new(JobId(1), &spec));
        let error = ctx.resolve_conflict(&clash()).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Cancelled);
        assert!(env.control.is_cancelled());
    }
}
