//! Running a copy or a move: plan it, then transfer the planned items one by one, each folder
//! with everything in it. A move renames where it can, and where it cannot it copies, then
//! deletes the source. Either leaves the record that undoes it.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io;
use std::ops::ControlFlow;

use rf_core::{Entry, EntryKind, Error, ErrorKind, Os, VPath};
use rf_vfs::{DeleteMode, MetadataPatch, Provider, RenameMode};

use super::file::{Source, copy_file};
use super::scan::{follow_link, follows_links, scan_transfer};
use super::{
    At, Attempt, Cause, Ctx, Failed, Identity, Level, MAX_DEPTH, fold_path, folder_identity,
    link_loop,
};
use crate::conflict::{Conflict, ConflictAction, Decision, decide};
use crate::naming::{MAX_NUMBERED, numbered_name};
use crate::preflight::{FsKind, IssueKind, check_name, check_path_len, fold_name};
use crate::report::Stage;
use crate::spec::{ConflictPolicy, ErrorPolicy, TransferMode, TransferSpec};
use crate::undo::{CreatedItem, MovedItem, UndoRecord};

/// Plans `spec` and transfers each planned item to its target under the job's conflict and
/// error policies; a folder goes with everything in it. A copy, and a move that the plan
/// turned into a copy, goes through [`transfer_entry`]; a move through [`move_entry`].
///
/// A target name that is free gets the file with a commit that never replaces. A taken one
/// is settled by [`place`] first: the item is skipped, written under a numbered name, or
/// written over the existing file, which is replaced only by the commit of the finished
/// temporary file, so the old content survives every failure before it. The conflict is
/// settled again each time the item is tried again, since the target may have changed. A
/// target that appears between that check and the commit fails the item as `Stage::Commit`;
/// it is never replaced.
///
/// At the end, also when the job stopped early, the report gets the record that undoes what
/// the job did (`report.undo`): for a move what it moved and the folders it made and removed,
/// for a copy what it made and how many files it replaced. `None` when it changed nothing.
pub(crate) fn run_transfer(ctx: &mut Ctx<'_>, spec: &TransferSpec) {
    let Some(plan) = scan_transfer(ctx, spec) else {
        return;
    };
    let moving = spec.mode == TransferMode::Move && !plan.moved_as_copy;
    let mut log = Log::default();
    for planned in &plan.items {
        let walk = Walk {
            conflict: spec.options.conflict,
            errors: spec.options.errors,
            follow: follows_links(spec),
            blocked: &plan.blocked,
            fs: planned.fs,
        };
        let (source, target) = (&planned.item.source, &planned.item.target);
        let to = To {
            path: target,
            planned: target,
            folder: Folder::Existing,
        };
        let stopped = if moving {
            move_entry(ctx, &walk, source, &planned.entry, to, Level::TOP, &mut log).is_break()
        } else {
            let entry = planned.entry.clone();
            transfer_entry(ctx, &walk, source, entry, to, Level::TOP, &mut log).is_break()
        };
        if stopped {
            break;
        }
    }
    ctx.report.undo = log.into_record(moving);
}

/// What every item under one planned item is transferred with.
#[derive(Debug)]
struct Walk<'p> {
    conflict: ConflictPolicy,
    errors: ErrorPolicy,
    /// Whether links are followed (a copy in Follow mode).
    follow: bool,
    /// The targets the pre-flight review left out, with the issue of each.
    blocked: &'p HashMap<VPath, IssueKind>,
    /// The kind of file system the targets are on.
    fs: FsKind,
}

/// Where one item goes.
#[derive(Clone, Copy, Debug)]
struct To<'a> {
    /// The path it is written to: under the folder its parent was really written to, which
    /// can be a numbered one.
    path: &'a VPath,
    /// The path the plan gave it, under the names of the sources: what its pre-flight issues
    /// are found under.
    planned: &'a VPath,
    /// What the folder it goes into is to the job.
    folder: Folder,
}

/// What the folder an item goes into is to the job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Folder {
    /// It was there before the job: the target folder of an item of the job, or a folder
    /// merged into.
    Existing,
    /// The job made it. An undo of a copy removes it with what is in it, so what a copy puts
    /// in it is not recorded on its own; a move fills it by copying, because the source folder
    /// it was made for is on another volume.
    Made,
}

/// What a transfer has changed so far, for the record that undoes it.
#[derive(Debug, Default)]
struct Log {
    /// The items a copy made, as `UndoRecord::Copied` lists them.
    created: Vec<CreatedItem>,
    /// How many files a copy replaced.
    overwritten: u64,
    /// The items a move put at their targets, each with everything in it.
    moves: Vec<MovedItem>,
    /// The folders a move made at the target, in the order it made them: parents first.
    created_dirs: Vec<VPath>,
    /// The source folders a move removed, in the order it removed them: children first.
    removed_dirs: Vec<VPath>,
}

impl Log {
    /// Notes what a copy committed at `path`, in a folder that is `folder` to the job: a file
    /// that `replaced` another is counted; an item in a folder the job made goes with that
    /// folder; any other is recorded as `stat` sees it now, after its commit (a folder after
    /// its children and metadata). One that `stat` cannot see is left out, so an undo leaves
    /// it where it is.
    fn copied(&mut self, ctx: &Ctx<'_>, path: &VPath, replaced: bool, folder: Folder) {
        if replaced {
            self.overwritten += 1;
            return;
        }
        if folder == Folder::Made {
            return;
        }
        if let Ok(entry) = ctx.provider(path).and_then(|dst| dst.stat(path)) {
            self.created
                .push(CreatedItem::from_entry(path.clone(), &entry));
        }
    }

    /// The record that undoes a move (`moving`) or a copy that did what this log holds, or
    /// `None` when it changed nothing. A move that only made and removed folders changed
    /// something: an empty folder moved across volumes.
    fn into_record(self, moving: bool) -> Option<UndoRecord> {
        let Log {
            created,
            overwritten,
            moves,
            created_dirs,
            removed_dirs,
        } = self;
        if moving {
            let changed = !(moves.is_empty() && created_dirs.is_empty() && removed_dirs.is_empty());
            changed.then_some(UndoRecord::Moved {
                moves,
                created_dirs,
                removed_dirs,
            })
        } else {
            let changed = !created.is_empty() || overwritten > 0;
            changed.then_some(UndoRecord::Copied {
                created,
                overwritten,
            })
        }
    }
}

/// Whether the job goes on after an item: `Break` once it is cancelled or aborted.
type Flow = ControlFlow<()>;

/// [`Flow`] for an item of a move, which also says whether the item has gone from the source
/// with everything in it (`Continue(true)`): a folder is removed only once every child has.
type Moved = ControlFlow<(), bool>;

/// The flow after an attempt that ends no differently whether it worked or not.
fn flow<T>(attempt: &Attempt<T>) -> Flow {
    match attempt {
        Attempt::Stop => ControlFlow::Break(()),
        Attempt::Done(_) | Attempt::Skipped => ControlFlow::Continue(()),
    }
}

/// The checks an item meets before anything is done with it, `Some` with the flow when it
/// ends there:
///
/// - Deeper than [`MAX_DEPTH`]: fails as `Stage::Plan` and is not entered.
/// - A target the pre-flight review left out (found under its planned path, also below a
///   folder written under a numbered name): fails as `Stage::Preflight` with its issue,
///   without a question, since the user chose to skip it.
fn refused(
    ctx: &mut Ctx<'_>,
    walk: &Walk<'_>,
    source: &VPath,
    to: To<'_>,
    level: Level<'_>,
) -> Option<Flow> {
    let target = to.path;
    if level.depth() > MAX_DEPTH {
        let too_deep = ctx.attempt(walk.errors, Some(source), Some(target), |_| {
            let error =
                Error::Unsupported(format!("folder nesting deeper than {MAX_DEPTH} levels"));
            Err::<(), _>(Failed::new(Stage::Plan, error))
        });
        return Some(flow(&too_deep));
    }
    if let Some(issue) = walk.blocked.get(to.planned) {
        let failure =
            Failed::issue(Stage::Preflight, issue.clone()).record(Some(source), Some(target));
        ctx.report.failures.push(failure);
        return Some(ControlFlow::Continue(()));
    }
    None
}

/// Copies `source`, which `entry` describes, to `to`; `level` says how far below an item of
/// the job it is (the items themselves are at 0) and which folders are above it. What the copy
/// makes goes to `log`.
///
/// - First the checks of every item (see [`refused`]): too deep, or left out by the
///   pre-flight review.
/// - A link in Follow mode: what it leads to is transferred in its place, under the link's
///   name. It is read, listed and given extras at its own path with every link resolved (see
///   `follow_link`), which also names it in its later failures, so a loop of links never makes
///   a source path longer. A link that cannot be followed fails as `Stage::Read`. A followed
///   folder that is one of the folders above it (by identity, when both are known) is a loop
///   of links: it fails as `Stage::Plan` and is not entered. Without identities, the depth
///   limit ends such a loop.
/// - A folder: see [`transfer_dir`]. A file, a link and anything else: see [`transfer_file`].
fn transfer_entry(
    ctx: &mut Ctx<'_>,
    walk: &Walk<'_>,
    source: &VPath,
    entry: Entry,
    to: To<'_>,
    level: Level<'_>,
    log: &mut Log,
) -> Flow {
    let target = to.path;
    if let Some(flow) = refused(ctx, walk, source, to, level) {
        return flow;
    }
    let (link, (source, entry)) = if entry.kind == EntryKind::Symlink && walk.follow {
        let followed = ctx.attempt(walk.errors, Some(source), Some(target), |ctx| {
            ctx.check().at(Stage::Read)?;
            let src = ctx.provider(source).at(Stage::Read)?;
            follow_link(&*src, source).at(Stage::Read)
        });
        match followed {
            Attempt::Done(followed) => (Some(source), followed),
            other => return flow(&other),
        }
    } else {
        (None, (source.clone(), entry))
    };
    if entry.kind != EntryKind::Dir {
        return transfer_file(ctx, walk, &source, &entry, to, level.depth() == 0, log);
    }
    if let Some(link) = link.filter(|_| level.leads_back(entry.identity())) {
        let looped = ctx.attempt(walk.errors, Some(link), Some(target), |_| {
            Err::<(), _>(Failed::new(Stage::Plan, link_loop(link)))
        });
        return flow(&looped);
    }
    transfer_dir(ctx, walk, &source, &entry, to, level, log)
}

/// Copies the file or link `source`, which `entry` describes, to `to`, or settles its
/// conflict first (see [`place`]); a `top` item that is written goes to the outputs. A FIFO,
/// socket or device goes to `copy_file` too, which refuses it before it opens or creates
/// anything; nobody is asked about it.
fn transfer_file(
    ctx: &mut Ctx<'_>,
    walk: &Walk<'_>,
    source: &VPath,
    entry: &Entry,
    to: To<'_>,
    top: bool,
    log: &mut Log,
) -> Flow {
    let target = to.path;
    let copied = ctx.attempt(walk.errors, Some(source), Some(target), |ctx| {
        // A cancel between items creates nothing more.
        ctx.check().at(Stage::Write)?;
        let src = ctx.provider(source).at(Stage::Read)?;
        let dst = ctx.provider(target).at(Stage::Write)?;
        let from = Source {
            provider: &*src,
            path: source,
            entry,
        };
        let (path, commit) = if matches!(entry.kind, EntryKind::File | EntryKind::Symlink) {
            match place(ctx, walk.conflict, from, &*dst, target, walk.fs)? {
                Place::Skip => return Ok(None),
                Place::At { path, commit } => (path, commit),
                // Only two folders merge; a file is never written into one.
                Place::Merge => return Err(Failed::new(Stage::Conflict, taken(target))),
            }
        } else {
            (target.clone(), RenameMode::NoReplace)
        };
        copy_file(ctx, from, &*dst, &path, commit).map_err(|failed| failed.writing(&path))?;
        Ok(Some((path, commit)))
    });
    match copied {
        Attempt::Done(Some((path, commit))) => {
            // Before `item_done`, which may call the listener: a committed file is in the
            // outputs and the log whatever the listener does.
            if top {
                ctx.report.outputs.push(path.clone());
            }
            log.copied(ctx, &path, commit == RenameMode::Replace, to.folder);
            ctx.item_done(true, false);
        }
        Attempt::Done(None) => ctx.report.skipped += 1,
        Attempt::Skipped => {}
        Attempt::Stop => return ControlFlow::Break(()),
    }
    ControlFlow::Continue(())
}

/// Copies the folder `source`, which `entry` describes, to `to` with everything in it.
///
/// One step lists all the children first, then settles where the folder goes (see
/// [`place`]): a new folder is made there, an existing folder is merged into without a
/// question and keeps its own metadata, or the policy skips it, renames it or refuses it. A
/// folder copied onto itself is made under a numbered name, never merged into itself. Each
/// child is then its own step, so a child that has been written stays counted once whatever
/// happens after it. A folder this job made gets the extras, times and attributes of the
/// source last, after its children are in it: its modification time stays, and a read-only
/// folder still takes its children. A metadata failure is a warning. Only a top-level folder
/// this job made goes to the outputs; one merged into was there before.
fn transfer_dir(
    ctx: &mut Ctx<'_>,
    walk: &Walk<'_>,
    source: &VPath,
    entry: &Entry,
    to: To<'_>,
    level: Level<'_>,
    log: &mut Log,
) -> Flow {
    let opened = ctx.attempt(walk.errors, Some(source), Some(to.path), |ctx| {
        open_dir(ctx, walk, source, entry, to.path)
    });
    let opened = match opened {
        Attempt::Done(Some(opened)) => opened,
        Attempt::Done(None) => {
            ctx.report.skipped += 1;
            return ControlFlow::Continue(());
        }
        other => return flow(&other),
    };
    if opened.made && level.depth() == 0 {
        ctx.report.outputs.push(opened.path.clone());
    }
    let folder = if opened.made { Folder::Made } else { to.folder };
    let here = level.enter(opened.identity);
    for child in opened.children {
        let Some((child_source, child_target, child_planned)) = child_paths(
            ctx,
            walk.errors,
            source,
            &opened.path,
            to.planned,
            &child.name,
        )?
        else {
            continue;
        };
        let child_to = To {
            path: &child_target,
            planned: &child_planned,
            folder,
        };
        transfer_entry(ctx, walk, &child_source, child, child_to, here.below(), log)?;
    }
    if opened.made {
        finish_dir(ctx, source, entry, &opened.path);
        log.copied(ctx, &opened.path, false, to.folder);
    }
    ctx.item_done(false, true);
    ControlFlow::Continue(())
}

/// The source, target and planned target of the child `name` of the folder `source`, whose
/// children go to `path` and were planned to go to `planned`. A name that cannot be joined
/// to one of them fails the child, as `Stage::Read` on the source side and as `Stage::Write`
/// on the target side: `Continue(None)`, or `Break` once that stops the job.
fn child_paths(
    ctx: &mut Ctx<'_>,
    errors: ErrorPolicy,
    source: &VPath,
    path: &VPath,
    planned: &VPath,
    name: &OsStr,
) -> ControlFlow<(), Option<(VPath, VPath, VPath)>> {
    let child_source = match source.join(name) {
        Ok(child) => child,
        Err(error) => {
            // No path names the child on the source side; the error names it.
            let named = path.join(name).ok();
            let target = named.as_ref().unwrap_or(path);
            let failed = ctx.attempt(errors, Some(source), Some(target), |_| {
                Err::<(), _>(Failed::new(Stage::Read, error.clone().into()))
            });
            flow(&failed)?;
            return ControlFlow::Continue(None);
        }
    };
    match (path.join(name), planned.join(name)) {
        (Ok(target), Ok(planned)) => ControlFlow::Continue(Some((child_source, target, planned))),
        (Err(error), _) | (_, Err(error)) => {
            let failed = ctx.attempt(errors, Some(&child_source), Some(path), |_| {
                Err::<(), _>(Failed::new(Stage::Write, error.clone().into()))
            });
            flow(&failed)?;
            ControlFlow::Continue(None)
        }
    }
}

/// A folder the job has entered.
#[derive(Debug)]
struct Opened {
    /// Where its children go.
    path: VPath,
    /// Its children, as they were when it was listed.
    children: Vec<Entry>,
    /// Whether this job made the folder at `path`, rather than merge into one that was there.
    made: bool,
    /// The identity of the source folder (see `folder_identity`).
    identity: Option<Identity>,
}

/// Lists the folder `source` and settles where its children go; `None` when the folder is
/// skipped.
fn open_dir(
    ctx: &mut Ctx<'_>,
    walk: &Walk<'_>,
    source: &VPath,
    entry: &Entry,
    target: &VPath,
) -> Result<Option<Opened>, Failed> {
    ctx.check().at(Stage::Write)?;
    let src = ctx.provider(source).at(Stage::Read)?;
    let dst = ctx.provider(target).at(Stage::Write)?;
    ctx.working_on(source);
    let children = ctx.list(&*src, source).at(Stage::Read)?;
    let identity = folder_identity(walk.follow, &*src, source, entry);
    let from = Source {
        provider: &*src,
        path: source,
        entry,
    };
    let (path, made) = match place(ctx, walk.conflict, from, &*dst, target, walk.fs)? {
        Place::Skip => return Ok(None),
        Place::Merge => (target.clone(), false),
        Place::At { path, .. } => {
            dst.create_dir(&path)
                .map_err(|error| Failed::new(Stage::Write, error).writing(&path))?;
            (path, true)
        }
    };
    Ok(Some(Opened {
        path,
        children,
        made,
        identity,
    }))
}

/// Gives the folder `to`, which this job made, what the source folder `source` has besides its
/// children: its extras when both are of one provider, then its times and attributes.
fn finish_dir(ctx: &mut Ctx<'_>, source: &VPath, entry: &Entry, to: &VPath) {
    let (src, dst) = match (ctx.provider(source), ctx.provider(to)) {
        (Ok(src), Ok(dst)) => (src, dst),
        (Err(error), _) | (_, Err(error)) => {
            ctx.warn(Failed::new(Stage::Metadata, error), Some(source), Some(to));
            return;
        }
    };
    if src.scheme() == dst.scheme() {
        if let Err(error) = dst.copy_extras(source, to) {
            ctx.warn(Failed::new(Stage::Metadata, error), Some(source), Some(to));
        }
    }
    let mut patch = MetadataPatch::times_of(entry);
    patch.attributes = MetadataPatch::attributes_of(entry).attributes;
    if let Err(error) = dst.set_metadata(to, &patch) {
        ctx.warn(Failed::new(Stage::Metadata, error), Some(source), Some(to));
    }
}

/// Moves `source`, which `entry` describes, to `to`; `level` says how far below an item of the
/// job it is. What the move does goes to `log`.
///
/// A move never follows a link, whatever the link mode says: a link is moved as the link
/// itself, by a rename or by a copy of the link, and only the link is ever deleted.
///
/// - First the checks of every item (see [`refused`]): too deep, or left out by the pre-flight
///   review. Such an item stays where it is.
/// - A folder: see [`move_dir`]. A file, a link and anything else: see [`move_file`]. Both
///   settle where the item goes with [`settle_move`], which renames it there when it can.
fn move_entry(
    ctx: &mut Ctx<'_>,
    walk: &Walk<'_>,
    source: &VPath,
    entry: &Entry,
    to: To<'_>,
    level: Level<'_>,
    log: &mut Log,
) -> Moved {
    if let Some(flow) = refused(ctx, walk, source, to, level) {
        return flow.map_continue(|()| false);
    }
    if entry.kind == EntryKind::Dir {
        move_dir(ctx, walk, source, entry, to, level, log)
    } else {
        move_file(ctx, walk, source, entry, to, level.depth() == 0, log)
    }
}

/// Moves the file or link `source`, which `entry` describes, to `to`: with one rename where it
/// can (see [`settle_move`]), else by a copy (see `copy_file`) and a delete of the source once
/// the copy is committed (see [`delete_source`]). A `top` item that reaches its target goes to
/// the outputs.
///
/// The delete is a step of its own, so that trying it again after a failure does not copy the
/// file again. It comes right after the commit, with no checkpoint between them: a pause or a
/// cancel there would leave the file in two places. A failed delete keeps both the source and
/// its copy. A FIFO, socket or device that cannot be renamed goes to `copy_file`, which
/// refuses it, and stays.
fn move_file(
    ctx: &mut Ctx<'_>,
    walk: &Walk<'_>,
    source: &VPath,
    entry: &Entry,
    to: To<'_>,
    top: bool,
    log: &mut Log,
) -> Moved {
    let target = to.path;
    let placed = ctx.attempt(walk.errors, Some(source), Some(target), |ctx| {
        // A cancel between items moves nothing more.
        ctx.check().at(Stage::Write)?;
        let src = ctx.provider(source).at(Stage::Read)?;
        let dst = ctx.provider(target).at(Stage::Write)?;
        let from = Source {
            provider: &*src,
            path: source,
            entry,
        };
        let (path, commit) = match settle_move(ctx, walk, from, &*dst, to)? {
            Way::Stay => return Ok(None),
            Way::Renamed(path) => return Ok(Some((path, false))),
            Way::Copy { path, commit } => (path, commit),
            // Only two folders merge; a file is never written into one.
            Way::Merge(_) => return Err(Failed::new(Stage::Conflict, taken(target))),
        };
        copy_file(ctx, from, &*dst, &path, commit).map_err(|failed| failed.writing(&path))?;
        Ok(Some((path, true)))
    });
    let (path, copied) = match placed {
        Attempt::Done(Some(placed)) => placed,
        Attempt::Done(None) => {
            ctx.report.skipped += 1;
            return ControlFlow::Continue(false);
        }
        Attempt::Skipped => return ControlFlow::Continue(false),
        Attempt::Stop => return ControlFlow::Break(()),
    };
    if top {
        ctx.report.outputs.push(path.clone());
    }
    let deleted = if copied {
        ctx.attempt(walk.errors, Some(source), Some(&path), |ctx| {
            delete_source(ctx, source, entry)
        })
    } else {
        Attempt::Done(())
    };
    let gone = matches!(deleted, Attempt::Done(()));
    if gone {
        log.moves.push(MovedItem {
            from: source.clone(),
            to: path,
        });
    }
    // The file is at its target, whether or not the delete of its source worked.
    ctx.item_done(true, false);
    flow(&deleted)?;
    ControlFlow::Continue(gone)
}

/// Deletes `source`, which a move has copied and committed and `entry` describes, for good.
///
/// It is `stat`ed first, and left with a `Stage::DeleteSource` failure when it is no longer
/// what was copied: of another kind (above all a folder, which `delete` would remove with
/// everything in it) or, when both identities are known, another file, as when a program
/// replaced it while it was copied. A link is deleted itself, never what it leads to.
fn delete_source(ctx: &Ctx<'_>, source: &VPath, entry: &Entry) -> Result<(), Failed> {
    let src = ctx.provider(source).at(Stage::DeleteSource)?;
    let now = src.stat(source).at(Stage::DeleteSource)?;
    let replaced = now.kind != entry.kind
        || matches!((now.identity(), entry.identity()), (Some(a), Some(b)) if a != b);
    if replaced {
        let error = Error::Unsupported(format!(
            "{source} changed while it was moved; left in place"
        ));
        return Err(Failed::new(Stage::DeleteSource, error));
    }
    src.delete(source, DeleteMode::Permanent)
        .map(drop)
        .at(Stage::DeleteSource)
}

/// How a move got a folder to its target in its first step.
#[derive(Debug)]
enum MovedDir {
    /// It stays where it is.
    Stayed,
    /// It is at this path, renamed there with everything in it.
    Renamed(VPath),
    /// It is listed, and its children go into the folder made or merged into at `path`.
    Opened(Opened),
}

/// Moves the folder `source`, which `entry` describes, to `to`.
///
/// One step settles where it goes (see [`settle_move`]). Renamed there, it is moved with
/// everything in it. Otherwise the same step lists its children and, unless it merges into the
/// folder at the target, makes a folder there, which its children are copied into. Each child
/// is then moved as its own step.
///
/// The source folder is removed after its children, with `remove_dir`, and only once each of
/// them has gone with everything in it: nothing failed, was skipped, was left out by the
/// pre-flight review or was too deep. A file that another program put in it meanwhile keeps
/// it, since `remove_dir` removes only an empty folder; that and any other failed removal is a
/// `Stage::DeleteSource` warning. A folder this job made gets the extras, times and attributes
/// of the source before that, while the source is still there to give them. Only a top-level
/// folder that was renamed or made goes to the outputs; one merged into was there before.
fn move_dir(
    ctx: &mut Ctx<'_>,
    walk: &Walk<'_>,
    source: &VPath,
    entry: &Entry,
    to: To<'_>,
    level: Level<'_>,
    log: &mut Log,
) -> Moved {
    let top = level.depth() == 0;
    let opened = ctx.attempt(walk.errors, Some(source), Some(to.path), |ctx| {
        open_moved_dir(ctx, walk, source, entry, to)
    });
    let opened = match opened {
        Attempt::Done(MovedDir::Opened(opened)) => opened,
        Attempt::Done(MovedDir::Renamed(path)) => {
            if top {
                ctx.report.outputs.push(path.clone());
            }
            log.moves.push(MovedItem {
                from: source.clone(),
                to: path,
            });
            ctx.item_done(false, true);
            return ControlFlow::Continue(true);
        }
        Attempt::Done(MovedDir::Stayed) => {
            ctx.report.skipped += 1;
            return ControlFlow::Continue(false);
        }
        Attempt::Skipped => return ControlFlow::Continue(false),
        Attempt::Stop => return ControlFlow::Break(()),
    };
    if opened.made {
        if top {
            ctx.report.outputs.push(opened.path.clone());
        }
        log.created_dirs.push(opened.path.clone());
    }
    let folder = if opened.made { Folder::Made } else { to.folder };
    let here = level.enter(None);
    let mut all_gone = true;
    for child in opened.children {
        let Some((child_source, child_target, child_planned)) = child_paths(
            ctx,
            walk.errors,
            source,
            &opened.path,
            to.planned,
            &child.name,
        )?
        else {
            all_gone = false;
            continue;
        };
        let child_to = To {
            path: &child_target,
            planned: &child_planned,
            folder,
        };
        all_gone &= move_entry(
            ctx,
            walk,
            &child_source,
            &child,
            child_to,
            here.below(),
            log,
        )?;
    }
    if opened.made {
        finish_dir(ctx, source, entry, &opened.path);
    }
    let gone = all_gone && remove_source_dir(ctx, source, &opened.path, log);
    ctx.item_done(false, true);
    ControlFlow::Continue(gone)
}

/// The first step of moving the folder `source`, which `entry` describes, to `to`: where it
/// goes, and unless it stays or was renamed, its children and the folder they go into.
fn open_moved_dir(
    ctx: &mut Ctx<'_>,
    walk: &Walk<'_>,
    source: &VPath,
    entry: &Entry,
    to: To<'_>,
) -> Result<MovedDir, Failed> {
    ctx.check().at(Stage::Write)?;
    let src = ctx.provider(source).at(Stage::Read)?;
    let dst = ctx.provider(to.path).at(Stage::Write)?;
    let from = Source {
        provider: &*src,
        path: source,
        entry,
    };
    let (path, made) = match settle_move(ctx, walk, from, &*dst, to)? {
        Way::Stay => return Ok(MovedDir::Stayed),
        Way::Renamed(path) => return Ok(MovedDir::Renamed(path)),
        Way::Merge(path) => (path, false),
        Way::Copy { path, .. } => (path, true),
    };
    let children = ctx.list(&*src, source).at(Stage::Read)?;
    if made {
        dst.create_dir(&path)
            .map_err(|error| Failed::new(Stage::Write, error).writing(&path))?;
    }
    Ok(MovedDir::Opened(Opened {
        path,
        children,
        made,
        identity: None,
    }))
}

/// Removes the folder `source`, whose children a move has all taken to `target`, with
/// `remove_dir`, and says whether it is gone. A failure is a `Stage::DeleteSource` warning:
/// the folder stays, with whatever is still in it.
fn remove_source_dir(ctx: &mut Ctx<'_>, source: &VPath, target: &VPath, log: &mut Log) -> bool {
    match ctx.provider(source).and_then(|src| src.remove_dir(source)) {
        Ok(()) => {
            log.removed_dirs.push(source.clone());
            true
        }
        Err(error) => {
            ctx.warn(
                Failed::new(Stage::DeleteSource, error),
                Some(source),
                Some(target),
            );
            false
        }
    }
}

/// Where a move puts an item, as [`settle_move`] settles it.
#[derive(Debug)]
enum Way {
    /// Nowhere: it is at its target already, or the conflict policy leaves it.
    Stay,
    /// It is at this path, renamed there in one step with everything in it.
    Renamed(VPath),
    /// A folder goes into the existing folder at this path, one child at a time.
    Merge(VPath),
    /// It is copied to `path` and committed with `commit` (a folder is made there), since it
    /// cannot be renamed there: it is on another volume, or of another provider.
    Copy { path: VPath, commit: RenameMode },
}

/// Settles where the move of `from` to `to` puts it, and renames it there when it can.
///
/// 1. The target is the source's own path: it stays (DND-09).
/// 2. `stat` of the target says `NotFound`: the name is free. Any other `stat` error fails the
///    item as `Stage::Conflict`, since a target that cannot be examined may well exist.
/// 3. The item there is the source itself (see [`same_item`]): a change of only the letter
///    case of its name in its own folder is renamed, and a failure of that rename fails the
///    item, as `Stage::Commit`; it never turns into a copy of the item onto itself and a
///    delete. Any other pair (a second name of the same file at the target, a hard link)
///    stays, whatever the policy says.
/// 4. Another item: the conflict rules of a copy (see [`settle`]) leave the item, merge a
///    folder into a folder (never into a link to one), or give the path and the commit mode
///    (`Replace` when the policy overwrites) to put it at.
/// 5. Of one provider, and unless the folder it goes into was made for a copy across volumes
///    (`Folder::Made`), it is renamed there with that mode. A `CrossesDevices` error makes it
///    a copy. `AlreadyExists`, a target that appeared since step 2, merges two real folders
///    (by `stat`) and fails anything else. Any other error fails the item as `Stage::Commit`;
///    nothing is tried again on its own, since the next try would meet the same error (a move
///    into a subfolder of itself through a link, say).
fn settle_move(
    ctx: &mut Ctx<'_>,
    walk: &Walk<'_>,
    from: Source<'_>,
    dst: &dyn Provider,
    to: To<'_>,
) -> Result<Way, Failed> {
    let target = to.path;
    if from.path == target {
        return Ok(Way::Stay);
    }
    ctx.working_on(from.path);
    let (path, commit) = match existing_at(dst, target)? {
        None => (target.clone(), RenameMode::NoReplace),
        Some(existing) => match same_item(from, target, &existing, walk.fs) {
            Same::Other => match settle(ctx, walk.conflict, from, dst, target, &existing)? {
                Place::Skip => return Ok(Way::Stay),
                Place::Merge => return Ok(Way::Merge(target.clone())),
                Place::At { path, commit } => (path, commit),
            },
            Same::Itself => return Ok(Way::Stay),
            Same::Spelling(renamed) => {
                dst.rename(from.path, &renamed, RenameMode::NoReplace)
                    .map_err(|error| Failed::new(Stage::Commit, error).writing(&renamed))?;
                return Ok(Way::Renamed(renamed));
            }
        },
    };
    if to.folder == Folder::Made || from.provider.scheme() != dst.scheme() {
        return Ok(Way::Copy { path, commit });
    }
    match dst.rename(from.path, &path, commit) {
        Ok(()) => Ok(Way::Renamed(path)),
        Err(error) if error.kind() == ErrorKind::CrossesDevices => Ok(Way::Copy { path, commit }),
        Err(error)
            if error.kind() == ErrorKind::AlreadyExists && both_folders(from, dst, &path) =>
        {
            Ok(Way::Merge(path))
        }
        Err(error) => Err(Failed::new(Stage::Commit, error).writing(&path)),
    }
}

/// Whether the source `from` and the item at `path` of `dst` are both real folders by `stat`.
/// A link to a folder is no folder to merge into.
fn both_folders(from: Source<'_>, dst: &dyn Provider, path: &VPath) -> bool {
    from.entry.kind == EntryKind::Dir
        && dst
            .stat(path)
            .is_ok_and(|entry| entry.kind == EntryKind::Dir)
}

/// What the item at the target of a move is to its source.
#[derive(Debug)]
enum Same {
    /// Another item.
    Other,
    /// The source itself, where it already is.
    Itself,
    /// The source itself, under its name in another letter case: renamed to this path, the
    /// new name in the source's own folder, it changes only the case of its name.
    Spelling(VPath),
}

/// What the item `existing` at `target` is to the source `from` of a move, `target` not being
/// the source's own path.
///
/// Whether it is the source itself, [`is_same_item`] tells: by identity when both are known,
/// so a second name of the same file (a hard link) is the source too; without them, on a
/// target file system `fs` that ignores case, by the folded paths. The source itself is
/// renamed only to change the case of its own name within its own folder: on a file system
/// that ignores case, the two folders fold to one and the two names differ but fold to one.
/// Anything else is the source where it already is.
fn same_item(from: Source<'_>, target: &VPath, existing: &Entry, fs: FsKind) -> Same {
    if !is_same_item(from, target, existing, fs) {
        return Same::Other;
    }
    let (Some(folder), Some(old), Some(new)) = (
        from.path.parent(),
        from.path.file_name(),
        target.file_name(),
    ) else {
        return Same::Itself;
    };
    let one_folder = target
        .parent()
        .is_some_and(|other| fold_path(&other, fs) == fold_path(&folder, fs));
    if fs.case_insensitive() && one_folder && old != new && fold_name(old, fs) == fold_name(new, fs)
    {
        if let Ok(renamed) = folder.join(new) {
            return Same::Spelling(renamed);
        }
    }
    Same::Itself
}

/// Where an item goes.
#[derive(Debug)]
enum Place {
    /// Nowhere: the existing item stays and the new one is left out.
    Skip,
    /// To `path`, committed with `commit` (a folder is made there).
    At { path: VPath, commit: RenameMode },
    /// Into the existing folder at the target, which a folder joins.
    Merge,
}

/// Decides where the item `from` is written: what happens when `target` is taken by something
/// already. `fs` is the kind of file system of the target.
///
/// 1. `stat` says `NotFound`: the name is free. Any other `stat` error fails the item, since a
///    target that cannot be examined may well exist.
/// 2. The existing item is the source itself (see [`is_same_item`]): the item gets a numbered
///    name whatever the policy says, so that a copy onto itself never replaces or empties its
///    own source, and a folder is never merged into itself.
/// 3. Another item: see [`settle`].
fn place(
    ctx: &mut Ctx<'_>,
    policy: ConflictPolicy,
    from: Source<'_>,
    dst: &dyn Provider,
    target: &VPath,
    fs: FsKind,
) -> Result<Place, Failed> {
    let Some(existing) = existing_at(dst, target)? else {
        return Ok(Place::At {
            path: target.clone(),
            commit: RenameMode::NoReplace,
        });
    };
    if is_same_item(from, target, &existing, fs) {
        let is_dir = from.entry.kind == EntryKind::Dir;
        return numbered(dst, target, is_dir, ctx.env.naming_os);
    }
    settle(ctx, policy, from, dst, target, &existing)
}

/// What is at `target` of `dst`, or `None` when `stat` says `NotFound`. Any other error fails
/// as `Stage::Conflict`: a target that cannot be examined may well exist.
fn existing_at(dst: &dyn Provider, target: &VPath) -> Result<Option<Entry>, Failed> {
    match dst.stat(target) {
        Ok(existing) => Ok(Some(existing)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Failed::new(Stage::Conflict, error)),
    }
}

/// Where the item `from` goes when `target` is taken by `existing`, another item.
///
/// 1. A folder meets a folder (not a link to one): they merge, without a question.
/// 2. Otherwise the policy decides. [`Decision::Ask`] goes to the resolver, and its
///    `Overwrite` answer is refused when either side is a folder, as it would be for the
///    overwrite policies: a file and a folder never replace each other. A refused conflict
///    fails the item as `Stage::Conflict` with `AlreadyExists` and touches neither side.
fn settle(
    ctx: &mut Ctx<'_>,
    policy: ConflictPolicy,
    from: Source<'_>,
    dst: &dyn Provider,
    target: &VPath,
    existing: &Entry,
) -> Result<Place, Failed> {
    let incoming = from.entry;
    let is_dir = incoming.kind == EntryKind::Dir;
    if is_dir && existing.kind == EntryKind::Dir {
        return Ok(Place::Merge);
    }
    let decision = match decide(policy, incoming, existing) {
        Decision::Ask => {
            let conflict = Conflict {
                source: from.path.clone(),
                target: target.clone(),
                incoming: incoming.clone(),
                existing: existing.clone(),
            };
            match ctx.resolve_conflict(&conflict).at(Stage::Conflict)? {
                // The same rule the overwrite policies follow.
                ConflictAction::Overwrite => decide(ConflictPolicy::Overwrite, incoming, existing),
                ConflictAction::Skip => Decision::Skip,
                ConflictAction::Rename => Decision::Rename,
                // `resolve_conflict` has failed already; this only keeps the match closed.
                ConflictAction::Cancel => {
                    return Err(Failed::new(Stage::Conflict, Error::Cancelled));
                }
            }
        }
        decided => decided,
    };
    match decision {
        Decision::Overwrite => Ok(Place::At {
            path: target.clone(),
            commit: RenameMode::Replace,
        }),
        Decision::Skip => Ok(Place::Skip),
        Decision::Rename => numbered(dst, target, is_dir, ctx.env.naming_os),
        // An `Ask` is answered by now; one that is not fails closed like a refusal.
        Decision::Refuse | Decision::Ask => Err(Failed::new(Stage::Conflict, taken(target))),
    }
}

/// Whether the item `existing` at `target` is the source `from` itself.
///
/// The same path is the same item. Otherwise two identities decide, but only when both are
/// known: a file system without file IDs gives none, and two unknowns are not proof of the
/// same file. A source listed without an identity (a Windows listing) is `stat`ed for it.
/// Without both identities, a target file system `fs` that ignores case takes two paths that
/// differ only in case for one item, as it would open them.
fn is_same_item(from: Source<'_>, target: &VPath, existing: &Entry, fs: FsKind) -> bool {
    if from.path == target {
        return true;
    }
    let incoming = from.entry.identity().or_else(|| {
        let entry = from.provider.stat(from.path).ok()?;
        entry.identity()
    });
    match (incoming, existing.identity()) {
        (Some(a), Some(b)) => a == b,
        _ => fs.case_insensitive() && fold_path(from.path, fs) == fold_path(target, fs),
    }
}

/// A free numbered name next to `target`, to commit a file to or make a folder at without
/// replacing anything.
fn numbered(dst: &dyn Provider, target: &VPath, is_dir: bool, os: Os) -> Result<Place, Failed> {
    let path = free_name(dst, target, is_dir, os).at(Stage::Conflict)?;
    Ok(Place::At {
        path,
        commit: RenameMode::NoReplace,
    })
}

/// The error of an item whose target name is taken and may not be replaced.
fn taken(target: &VPath) -> Error {
    Error::io(
        "rename",
        target.clone(),
        io::ErrorKind::AlreadyExists.into(),
    )
}

/// The first of `target`'s numbered names (`name (2)`, `name (3)`, ..., up to
/// [`MAX_NUMBERED`]; `name 2` on macOS) that `provider` has nothing at. `is_dir` is whether
/// the item to name is a folder, which keeps its whole name as the stem.
///
/// Only `NotFound` from `stat` makes a name free; any other error is returned. A name that
/// would be too long for the file system or the OS (a number adds up to seven units) fails
/// with that issue ([`ErrorKind::InvalidPath`]) before the file system is asked, since every
/// later number is at least as long. When all the names are taken, fails with `AlreadyExists`
/// for `target`.
pub(crate) fn free_name(
    provider: &dyn Provider,
    target: &VPath,
    is_dir: bool,
    os: Os,
) -> Result<VPath, Cause> {
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        return Err(Error::Unsupported(format!("no name to number: {target}")).into());
    };
    let fs = FsKind::from_fs_name(provider.fs_name(&parent).as_deref());
    for n in 2..=MAX_NUMBERED {
        let numbered = numbered_name(name, n, is_dir, os);
        let candidate = parent.join(&numbered)?;
        let too_long = check_name(&numbered, fs)
            .into_iter()
            .find(|issue| matches!(issue, IssueKind::NameTooLong { .. }))
            .or_else(|| check_path_len(&candidate, os));
        if let Some(issue) = too_long {
            return Err(Cause::Issue(issue));
        }
        match provider.stat(&candidate) {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(candidate),
            Err(error) => return Err(error.into()),
        }
    }
    Err(taken(target).into())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use rf_core::{FileId, VolumeId};
    use rf_vfs::LocalProvider;

    use super::*;

    fn v(path: &Path) -> VPath {
        VPath::local(path).unwrap()
    }

    fn name_of(path: &VPath) -> OsString {
        path.file_name().unwrap().to_owned()
    }

    fn entry(volume: Option<u64>, file: Option<u128>) -> Entry {
        let mut entry = Entry::new("a", EntryKind::File);
        entry.volume_id = volume.map(VolumeId);
        entry.file_id = file.map(FileId);
        entry
    }

    #[test]
    fn two_identities_make_one_item_only_when_both_are_known() {
        let base = std::env::temp_dir();
        let (source, target) = (v(&base.join("s")), v(&base.join("t")));
        let local = LocalProvider::new();
        let same = |incoming: &Entry, existing: &Entry| {
            let from = Source {
                provider: &local,
                path: &source,
                entry: incoming,
            };
            is_same_item(from, &target, existing, FsKind::Other)
        };
        let known = entry(Some(1), Some(7));
        assert!(same(&known, &entry(Some(1), Some(7))));
        assert!(!same(&known, &entry(Some(1), Some(8))));
        assert!(!same(&known, &entry(Some(2), Some(7))));
        // Unknown is not proof of anything: two files without IDs are not one file.
        assert!(!same(&entry(None, None), &entry(None, None)));
        assert!(!same(&known, &entry(Some(1), None)));
        assert!(!same(&entry(None, Some(7)), &entry(None, Some(7))));
        // The same path is the same item whatever the IDs say.
        let from = Source {
            provider: &local,
            path: &source,
            entry: &entry(None, None),
        };
        assert!(is_same_item(
            from,
            &source,
            &entry(None, None),
            FsKind::Other
        ));
    }

    #[test]
    fn without_identities_a_target_that_ignores_case_compares_folded_paths() {
        let base = std::env::temp_dir();
        let (source, upper) = (v(&base.join("a.txt")), v(&base.join("A.TXT")));
        let local = LocalProvider::new();
        let unknown = entry(None, None);
        let same = |incoming: &Entry, existing: &Entry, fs| {
            let from = Source {
                provider: &local,
                path: &source,
                entry: incoming,
            };
            is_same_item(from, &upper, existing, fs)
        };
        assert!(same(&unknown, &unknown, FsKind::Ntfs));
        assert!(!same(&unknown, &unknown, FsKind::Other));
        // Two known identities that differ are two items, in a folder that tells case apart.
        assert!(!same(
            &entry(Some(1), Some(7)),
            &entry(Some(1), Some(8)),
            FsKind::Ntfs
        ));
        let other = v(&base.join("b.txt"));
        let from = Source {
            provider: &local,
            path: &source,
            entry: &unknown,
        };
        assert!(!is_same_item(from, &other, &unknown, FsKind::Ntfs));
    }

    #[test]
    fn free_name_takes_the_first_number_nobody_has() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.txt", "a (2).txt", "a (4).txt"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        let target = v(&dir.path().join("a.txt"));
        let local = LocalProvider::new();
        let free = free_name(&local, &target, false, Os::Windows).unwrap();
        assert_eq!(name_of(&free), "a (3).txt");
        let free = free_name(&local, &target, false, Os::MacOs).unwrap();
        assert_eq!(name_of(&free), "a 2.txt");
        // A folder keeps its dots.
        let folder = v(&dir.path().join("v1.2"));
        let free = free_name(&local, &folder, true, Os::Other).unwrap();
        assert_eq!(name_of(&free), "v1.2 (2)");
    }

    #[test]
    fn free_name_gives_up_after_the_last_number() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "").unwrap();
        for n in 2..=MAX_NUMBERED {
            std::fs::write(dir.path().join(format!("a ({n})")), "").unwrap();
        }
        let target = v(&dir.path().join("a"));
        let error = free_name(&LocalProvider::new(), &target, false, Os::Other).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists);
        assert!(error.to_string().contains(&target.to_string()), "{error}");
    }

    #[test]
    fn free_name_stops_at_a_name_too_long_for_the_file_system() {
        let dir = tempfile::tempdir().unwrap();
        let target = v(&dir.path().join("n".repeat(253)));
        let local = LocalProvider::new();
        // ` (2)` takes the name to 257 units and ` 2` to 255.
        let error = free_name(&local, &target, false, Os::Windows).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidPath);
        // The issue's own text, with no prefix.
        let issue = IssueKind::NameTooLong { len: 257, max: 255 };
        assert_eq!(error.to_string(), issue.to_string());
        assert!(free_name(&local, &target, false, Os::MacOs).is_ok());
    }
}
