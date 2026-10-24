//! Running one job to its end on the calling thread: plan, run, report.

mod delete;
mod file;
mod scan;
mod simple;
#[cfg(all(test, unix))]
mod swapping;
mod transfer;
mod undo;

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use rf_core::{Entry, Error, ErrorKind, FileId, Os, PathError, VPath, VolumeId};
use rf_vfs::{Provider, Vfs};

use crate::conflict::{Conflict, ConflictAction};
use crate::control::Control;
use crate::id::JobId;
use crate::journal::{JournalHandle, Record};
use crate::naming::temp_name;
use crate::preflight::{FsKind, IssueKind, fold_name};
use crate::progress::{PROGRESS_INTERVAL, Progress, SPEED_WINDOW, SpeedMeter};
use crate::report::{FailureReason, ItemFailure, JobReport, Outcome, Stage, Totals};
use crate::resolver::{ErrorAction, Resolver};
use crate::spec::{ErrorPolicy, JobKind, JobSpec};

/// The size of the buffer a file is copied through, and so of one progress step.
pub(crate) const COPY_CHUNK: usize = 1 << 20;

/// How deep a transfer or a permanent delete goes into folders: the items it was given are at
/// depth 0, their children at 1. An item deeper than this is not counted in the plan and fails
/// as [`Stage::Plan`] when the job reaches it; a folder there is not entered, and a delete
/// removes neither it nor the folders above it.
///
/// A loop of links that a copy in Follow mode would walk forever is caught by identity where
/// identities are known: a followed folder that is one of the folders above it is not entered.
/// Only where they are unknown (a file system without file IDs) does this depth end such a
/// loop, well inside the 1,023-byte path limit of macOS.
pub const MAX_DEPTH: usize = 128;

/// What a running job tells the one who started it, on the job's own thread.
#[derive(Clone, Debug)]
#[non_exhaustive]
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
/// naming rule it follows, its remote control, who hears about its progress and the journal
/// it notes its temporary folders in.
pub struct ExecEnv<'a> {
    vfs: &'a Vfs,
    resolver: &'a dyn Resolver,
    naming_os: Os,
    control: Control,
    on_event: Option<&'a dyn Fn(ExecEvent)>,
    journal: Option<&'a JournalHandle>,
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
            journal: None,
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

    /// Notes in `journal`, durably, each folder the job makes temporary files in, before the
    /// first one there, so that crash recovery finds what the job left if it never ends.
    #[must_use]
    pub(crate) fn journal(mut self, journal: &'a JournalHandle) -> Self {
        self.journal = Some(journal);
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
            .field("journal", &self.journal)
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
            reason: None,
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
        JobSpec::Delete(delete) => delete::run_delete(ctx, delete),
        JobSpec::Rename { from, to } => simple::run_rename(ctx, from, to),
        JobSpec::Create { parent, name, kind } => simple::run_create(ctx, parent, name, *kind),
        JobSpec::Undo(record) => undo::run_undo(ctx, record),
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
    /// Why the job refused the item itself, when it did (see [`FailureReason`]). A failure of
    /// an issue has the issue as its reason without this. Boxed, as `target` is.
    pub reason: Option<Box<FailureReason>>,
}

impl Failed {
    pub(crate) fn new(stage: Stage, error: Error) -> Failed {
        Failed {
            stage,
            cause: Cause::Error(error),
            target: None,
            reason: None,
        }
    }

    /// The failure of an item the target cannot hold because of `issue`; its reason is the
    /// issue.
    pub(crate) fn issue(stage: Stage, issue: IssueKind) -> Failed {
        Failed {
            stage,
            cause: Cause::Issue(issue),
            target: None,
            reason: None,
        }
    }

    /// The failure of an item the job refuses itself, by the rule `reason`, at `stage`;
    /// `error` gives its kind and its log text.
    pub(crate) fn refused(stage: Stage, error: Error, reason: FailureReason) -> Failed {
        Failed::new(stage, error).because(reason)
    }

    /// Gives the failure the reason `reason`.
    #[must_use]
    pub(crate) fn because(mut self, reason: FailureReason) -> Failed {
        self.reason = Some(Box::new(reason));
        self
    }

    /// Names `path` as the path that was being written, unless a path is named already.
    #[must_use]
    pub(crate) fn writing(mut self, path: &VPath) -> Failed {
        self.target.get_or_insert_with(|| Box::new(path.clone()));
        self
    }

    /// The record of this failure for a step on `source` and `target`; the path that was being
    /// written, if one is named, is the target instead. The reason is the one given, else the
    /// issue of a failure of an issue, else none.
    pub(crate) fn record(self, source: Option<&VPath>, target: Option<&VPath>) -> ItemFailure {
        let reason = self
            .reason
            .map(|reason| *reason)
            .or_else(|| match &self.cause {
                Cause::Issue(issue) => Some(FailureReason::Issue(issue.clone())),
                Cause::Error(_) => None,
            });
        ItemFailure {
            source: source.cloned(),
            target: self.target.map(|path| *path).or_else(|| target.cloned()),
            stage: self.stage,
            kind: self.cause.kind(),
            message: self.cause.to_string(),
            reason,
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
            reason: None,
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

/// The failure of a followed folder that is one of the folders above it.
pub(crate) fn link_loop(source: &VPath) -> Failed {
    let error = Error::Unsupported(format!(
        "folder link loop: {source} leads back to a folder above it"
    ));
    Failed::refused(Stage::Plan, error, FailureReason::LinkLoop)
}

/// Whether the item a fresh `stat` finds, `now`, is no longer the one `was` described: of
/// another kind (a file turned into a folder, a folder into a link to one) or, when both
/// identities are known, another item. A walk about to delete or enter an item it found
/// earlier leaves it in place then.
pub(crate) fn replaced(was: &Entry, now: &Entry) -> bool {
    now.kind != was.kind || matches!((now.identity(), was.identity()), (Some(a), Some(b)) if a != b)
}

/// What a walk that takes items away was doing to an item it found changed: the words of the
/// failure that leaves it in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Walking {
    /// A move, which takes each item away once its copy is committed.
    Moving,
    /// A permanent delete.
    Deleting,
}

impl Walking {
    /// The failure, at `stage`, of `path`, which changed under the walk (see [`replaced`]):
    /// `Unsupported` with "{path} changed while it was moved (or deleted); left in place".
    pub(crate) fn changed(self, stage: Stage, path: &VPath) -> Failed {
        let (done, reason) = match self {
            Walking::Moving => ("moved", FailureReason::ChangedWhileMoved),
            Walking::Deleting => ("deleted", FailureReason::ChangedWhileDeleted),
        };
        let error =
            Error::Unsupported(format!("{path} changed while it was {done}; left in place"));
        Failed::refused(stage, error, reason)
    }
}

/// The folder an item of a walk is in, as the walk found it when it listed it: its path, and
/// what `stat` gave for it then (its kind, and its identity where the file system has one,
/// also on Windows, where a listing carries none).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Parent<'a> {
    pub path: &'a VPath,
    pub entry: &'a Entry,
}

/// `stat`s `path` of `provider` again and gives what it finds. Fails at `stage`, leaving the
/// item in place, when it is no longer the item `entry` described (see [`replaced`]): of
/// another kind (a file turned into a folder, a folder into a link to one) or, when both
/// identities are known, another item. A `stat` that fails fails at `stage` too.
pub(crate) fn restat(
    provider: &dyn Provider,
    path: &VPath,
    entry: &Entry,
    stage: Stage,
    walking: Walking,
) -> Result<Entry, Failed> {
    let now = provider.stat(path).at(stage)?;
    if replaced(entry, &now) {
        return Err(walking.changed(stage, path));
    }
    Ok(now)
}

/// Checks that the folder an item is in, `parent` (`None` for an item of the job), is still the
/// folder the walk listed (see [`restat`]). A swap of that folder for a link makes it a link
/// (another kind) on every system; a swap of a folder further above makes its path lead to
/// another folder, which has another identity where the file system has identities.
pub(crate) fn check_parent(
    provider: &dyn Provider,
    parent: Option<Parent<'_>>,
    stage: Stage,
    walking: Walking,
) -> Result<(), Failed> {
    match parent {
        Some(parent) => restat(provider, parent.path, parent.entry, stage, walking).map(drop),
        None => Ok(()),
    }
}

/// The volume of the folder `path` of `provider`, which `entry` describes: the entry's, or the
/// one `stat` gives for a listed folder that has none (a Windows listing). `None` when neither
/// tells.
pub(crate) fn volume_of(provider: &dyn Provider, path: &VPath, entry: &Entry) -> Option<VolumeId> {
    entry
        .volume_id
        .or_else(|| provider.stat(path).ok()?.volume_id)
}

/// Whether a folder on `volume` in a folder on `above` is on another volume (a mount point):
/// only when both are known and differ.
pub(crate) fn on_another_volume(above: Option<VolumeId>, volume: Option<VolumeId>) -> bool {
    matches!((above, volume), (Some(a), Some(b)) if a != b)
}

/// The failure, at `stage`, of the folder `path`, which is on another volume than the folder it
/// is in and is not entered: `Unsupported` with "{path} is on another volume".
pub(crate) fn other_volume(stage: Stage, path: &VPath) -> Failed {
    let error = Error::Unsupported(format!("{path} is on another volume"));
    Failed::refused(stage, error, FailureReason::OtherVolume)
}

/// Fails the item `source`, which goes to `target` if anything is written, as deeper than
/// [`MAX_DEPTH`] when `level` is that deep: a [`Stage::Plan`] failure under `policy`, and the
/// item is neither entered, copied, moved nor deleted. `None` when it is not that deep.
pub(crate) fn too_deep(
    ctx: &mut Ctx<'_>,
    level: Level<'_>,
    policy: ErrorPolicy,
    source: &VPath,
    target: Option<&VPath>,
) -> Option<Attempt<()>> {
    if level.depth() <= MAX_DEPTH {
        return None;
    }
    Some(ctx.attempt(policy, Some(source), target, |_| {
        let error = Error::Unsupported(format!("folder nesting deeper than {MAX_DEPTH} levels"));
        Err(Failed::refused(Stage::Plan, error, FailureReason::TooDeep))
    }))
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
    /// The folders of temporary files noted in the journal so far.
    temp_dirs: HashSet<VPath>,
    /// The temporary files and markers the job could not remove (see [`Leftovers`]).
    leftovers: Leftovers,
    /// Whether a metadata step has failed with `Unsupported` before, which is warned about
    /// once per job (see [`warn_metadata`](Self::warn_metadata)).
    metadata_unsupported: bool,
}

/// The temporary files and markers of a job that their guards could not remove, as warnings
/// for the job's report. A guard notes one as it is dropped, also while a panic unwinds, when
/// nothing else can reach the report; the job hands them over to the report as it ends. A
/// leftover keeps its temporary name, and crash recovery looks only at the folders of jobs
/// that never ended, so without this nobody would hear of it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Leftovers(Rc<RefCell<Vec<ItemFailure>>>);

impl Leftovers {
    /// Notes that `path`, a temporary file or marker of the job made for `source`, could not be
    /// removed because of `error`: a `Stage::Write` warning that names it as its target, with
    /// the reason `TempLeftBehind`. A `NotFound` is nothing to note: the file is gone.
    pub(crate) fn note(&self, error: Error, source: Option<&VPath>, path: &VPath) {
        if error.kind() == ErrorKind::NotFound {
            return;
        }
        let failed = Failed::refused(Stage::Write, error, FailureReason::TempLeftBehind);
        self.0.borrow_mut().push(failed.record(source, Some(path)));
    }

    fn take(&self) -> Vec<ItemFailure> {
        std::mem::take(&mut *self.0.borrow_mut())
    }
}

/// A temporary file or marker that a job made at `path` of `provider`, removed when the guard
/// goes out of scope still armed: after an error, a cancel, or while a panic unwinds. It goes
/// with `Provider::delete_leaf`, so a folder that took its name meanwhile is never removed with
/// it. A file it cannot remove keeps its temporary name and is noted in the job's
/// [`Leftovers`], so the report names it.
pub(crate) struct TempGuard<'a> {
    provider: &'a dyn Provider,
    path: &'a VPath,
    source: Option<&'a VPath>,
    leftovers: Leftovers,
    armed: bool,
}

impl<'a> TempGuard<'a> {
    /// The guard of the item this job made at `path`, for the item `source`.
    pub(crate) fn new(
        ctx: &Ctx<'_>,
        provider: &'a dyn Provider,
        path: &'a VPath,
        source: Option<&'a VPath>,
    ) -> TempGuard<'a> {
        TempGuard {
            provider,
            path,
            source,
            leftovers: ctx.leftovers.clone(),
            armed: true,
        }
    }

    /// Leaves the item where it is: it was committed.
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            if let Err(error) = self.provider.delete_leaf(self.path) {
                self.leftovers.note(error, self.source, self.path);
            }
        }
    }
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
            temp_dirs: HashSet::new(),
            leftovers: Leftovers::default(),
            metadata_unsupported: false,
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

    /// Records that copying metadata (`Provider::set_metadata`, `Provider::copy_extras`) onto
    /// `target` failed with `error`, as a `Stage::Metadata` warning; the item itself is done.
    ///
    /// An `Unsupported` error means that the target cannot keep that metadata at all: a
    /// provider without `set_metadata` (its default implementation), or a file system that has
    /// no place for it. That is nothing to do for each item, so it is warned about once per
    /// job, for the first item it happens to, and every later one is passed over. Any other
    /// error is warned about for each item.
    pub(crate) fn warn_metadata(
        &mut self,
        error: Error,
        source: Option<&VPath>,
        target: Option<&VPath>,
    ) {
        if error.kind() == ErrorKind::Unsupported {
            if self.metadata_unsupported {
                return;
            }
            self.metadata_unsupported = true;
        }
        self.warn(Failed::new(Stage::Metadata, error), source, target);
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
    /// `seq` counting from 0 in each job. A target in no folder (a volume root) fails as
    /// `Stage::Write` with the reason `NoTargetFolder`.
    pub(crate) fn temp_for(&mut self, target: &VPath) -> Result<VPath, Failed> {
        let Some(parent) = target.parent() else {
            let error =
                Error::Unsupported(format!("no folder to write a temporary file in: {target}"));
            return Err(Failed::refused(
                Stage::Write,
                error,
                FailureReason::NoTargetFolder,
            ));
        };
        self.temp_in(&parent).at(Stage::Write)
    }

    /// A new temporary name in the folder `folder`, numbered on from the job's other ones (see
    /// [`temp_for`](Self::temp_for)).
    ///
    /// With a journal, the job's first name in a folder first notes the folder in the journal
    /// and waits until the note is on the disk, so that crash recovery knows where to look for
    /// what the job leaves.
    /// A note that cannot be written does not stop the job: the journal reports it itself, and
    /// a leftover temporary file is never mistaken for a user's file.
    pub(crate) fn temp_in(&mut self, folder: &VPath) -> rf_core::Result<VPath> {
        let seq = self.temp_seq;
        self.temp_seq += 1;
        let temp = folder.join(temp_name(self.id, seq))?;
        if let Some(journal) = self.env.journal {
            if self.temp_dirs.insert(folder.clone()) {
                let note = Record::TempDir {
                    job: self.id,
                    dir: folder.clone(),
                };
                // The journal's own error handler has the failure.
                let _ = journal.append_durable(note);
            }
        }
        Ok(temp)
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

    /// The report of the finished job, with its outcome and the time since `started`. The
    /// temporary files the job could not remove go to its warnings.
    fn finish(mut self, started: Instant) -> JobReport {
        let leftovers = self.leftovers.take();
        self.report.warnings.extend(leftovers);
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
