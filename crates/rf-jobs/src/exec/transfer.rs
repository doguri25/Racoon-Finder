//! Running a copy or a move: plan it, then transfer the planned items one by one, each folder
//! with everything in it.

use std::collections::HashMap;
use std::io;
use std::ops::ControlFlow;

use rf_core::{Entry, EntryKind, Error, ErrorKind, Os, VPath};
use rf_vfs::{MetadataPatch, Provider, RenameMode};

use super::file::{Source, copy_file};
use super::scan::{follow_link, follows_links, scan_transfer};
use super::{
    At, Attempt, Cause, Ctx, Failed, Identity, Level, MAX_DEPTH, fold_path, folder_identity,
    link_loop,
};
use crate::conflict::{Conflict, ConflictAction, Decision, decide};
use crate::naming::{MAX_NUMBERED, numbered_name};
use crate::preflight::{FsKind, IssueKind, check_name, check_path_len};
use crate::report::Stage;
use crate::spec::{ConflictPolicy, ErrorPolicy, TransferMode, TransferSpec};

/// Plans `spec` and copies each planned item to its target under the job's conflict and error
/// policies; a folder goes with everything in it (see [`transfer_entry`]).
///
/// A target name that is free gets the file with a commit that never replaces. A taken one
/// is settled by [`place`] first: the item is skipped, written under a numbered name, or
/// written over the existing file, which is replaced only by the commit of the finished
/// temporary file, so the old content survives every failure before it. The conflict is
/// settled again each time the item is tried again, since the target may have changed. A
/// target that appears between that check and the commit fails the item as `Stage::Commit`;
/// it is never replaced.
pub(crate) fn run_transfer(ctx: &mut Ctx<'_>, spec: &TransferSpec) {
    if spec.mode == TransferMode::Move {
        ctx.not_yet("move is not implemented yet");
        return;
    }
    let Some(plan) = scan_transfer(ctx, spec) else {
        return;
    };
    for planned in &plan.items {
        let walk = Walk {
            conflict: spec.options.conflict,
            errors: spec.options.errors,
            follow: follows_links(spec),
            blocked: &plan.blocked,
            fs: planned.fs,
        };
        let (source, target) = (&planned.item.source, &planned.item.target);
        let entry = planned.entry.clone();
        let to = To {
            path: target,
            planned: target,
        };
        if transfer_entry(ctx, &walk, source, entry, to, Level::TOP).is_break() {
            return;
        }
    }
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
}

/// Whether the job goes on after an item: `Break` once it is cancelled or aborted.
type Flow = ControlFlow<()>;

/// The flow after an attempt that ends no differently whether it worked or not.
fn flow<T>(attempt: &Attempt<T>) -> Flow {
    match attempt {
        Attempt::Stop => ControlFlow::Break(()),
        Attempt::Done(_) | Attempt::Skipped => ControlFlow::Continue(()),
    }
}

/// Transfers `source`, which `entry` describes, to `to`; `level` says how far below an item of
/// the job it is (the items themselves are at 0) and which folders are above it.
///
/// - Deeper than [`MAX_DEPTH`]: fails as `Stage::Plan` and is not entered.
/// - A target the pre-flight review left out (found under its planned path, also below a
///   folder written under a numbered name): fails as `Stage::Preflight` with its issue,
///   without a question, since the user chose to skip it.
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
) -> Flow {
    let target = to.path;
    if level.depth() > MAX_DEPTH {
        let too_deep = ctx.attempt(walk.errors, Some(source), Some(target), |_| {
            let error =
                Error::Unsupported(format!("folder nesting deeper than {MAX_DEPTH} levels"));
            Err::<(), _>(Failed::new(Stage::Plan, error))
        });
        return flow(&too_deep);
    }
    if let Some(issue) = walk.blocked.get(to.planned) {
        let failure =
            Failed::issue(Stage::Preflight, issue.clone()).record(Some(source), Some(target));
        ctx.report.failures.push(failure);
        return ControlFlow::Continue(());
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
        return transfer_file(ctx, walk, &source, &entry, target, level.depth() == 0);
    }
    if let Some(link) = link.filter(|_| level.leads_back(entry.identity())) {
        let looped = ctx.attempt(walk.errors, Some(link), Some(target), |_| {
            Err::<(), _>(Failed::new(Stage::Plan, link_loop(link)))
        });
        return flow(&looped);
    }
    transfer_dir(ctx, walk, &source, &entry, to, level)
}

/// Copies the file or link `source`, which `entry` describes, to `target`, or settles its
/// conflict first (see [`place`]); a `top` item that is written goes to the outputs. A FIFO,
/// socket or device goes to `copy_file` too, which refuses it before it opens or creates
/// anything; nobody is asked about it.
fn transfer_file(
    ctx: &mut Ctx<'_>,
    walk: &Walk<'_>,
    source: &VPath,
    entry: &Entry,
    target: &VPath,
    top: bool,
) -> Flow {
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
        let (to, commit) = if matches!(entry.kind, EntryKind::File | EntryKind::Symlink) {
            match place(ctx, walk.conflict, from, &*dst, target, walk.fs)? {
                Place::Skip => return Ok(None),
                Place::At { path, commit } => (path, commit),
                // Only two folders merge; a file is never written into one.
                Place::Merge => return Err(Failed::new(Stage::Conflict, taken(target))),
            }
        } else {
            (target.clone(), RenameMode::NoReplace)
        };
        copy_file(ctx, from, &*dst, &to, commit).map_err(|failed| failed.writing(&to))?;
        Ok(Some(to))
    });
    match copied {
        Attempt::Done(Some(to)) => {
            // Before `item_done`, which may call the listener: a committed file is in the
            // outputs whatever the listener does.
            if top {
                ctx.report.outputs.push(to);
            }
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
    let here = level.enter(opened.identity);
    for child in opened.children {
        let name = &child.name;
        let child_source = match source.join(name) {
            Ok(path) => path,
            Err(error) => {
                // No path names the child on the source side; the error names it.
                let named = opened.path.join(name).ok();
                let target = named.as_ref().unwrap_or(&opened.path);
                let failed = ctx.attempt(walk.errors, Some(source), Some(target), |_| {
                    Err::<(), _>(Failed::new(Stage::Read, error.clone().into()))
                });
                flow(&failed)?;
                continue;
            }
        };
        let (child_target, child_planned) = match (opened.path.join(name), to.planned.join(name)) {
            (Ok(target), Ok(planned)) => (target, planned),
            (Err(error), _) | (_, Err(error)) => {
                let failed =
                    ctx.attempt(walk.errors, Some(&child_source), Some(&opened.path), |_| {
                        Err::<(), _>(Failed::new(Stage::Write, error.clone().into()))
                    });
                flow(&failed)?;
                continue;
            }
        };
        let child_to = To {
            path: &child_target,
            planned: &child_planned,
        };
        transfer_entry(ctx, walk, &child_source, child, child_to, here.below())?;
    }
    if opened.made {
        finish_dir(ctx, source, entry, &opened.path);
    }
    ctx.item_done(false, true);
    ControlFlow::Continue(())
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
/// 3. A folder meets a folder (not a link to one): they merge, without a question.
/// 4. Otherwise the policy decides. [`Decision::Ask`] goes to the resolver, and its
///    `Overwrite` answer is refused when either side is a folder, as it would be for the
///    overwrite policies: a file and a folder never replace each other. A refused conflict
///    fails the item as `Stage::Conflict` with `AlreadyExists` and touches neither side.
fn place(
    ctx: &mut Ctx<'_>,
    policy: ConflictPolicy,
    from: Source<'_>,
    dst: &dyn Provider,
    target: &VPath,
    fs: FsKind,
) -> Result<Place, Failed> {
    let existing = match dst.stat(target) {
        Ok(existing) => existing,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(Place::At {
                path: target.clone(),
                commit: RenameMode::NoReplace,
            });
        }
        Err(error) => return Err(Failed::new(Stage::Conflict, error)),
    };
    let os = ctx.env.naming_os;
    let incoming = from.entry;
    let is_dir = incoming.kind == EntryKind::Dir;
    if is_same_item(from, target, &existing, fs) {
        return numbered(dst, target, is_dir, os);
    }
    if is_dir && existing.kind == EntryKind::Dir {
        return Ok(Place::Merge);
    }
    let decision = match decide(policy, incoming, &existing) {
        Decision::Ask => {
            let conflict = Conflict {
                source: from.path.clone(),
                target: target.clone(),
                incoming: incoming.clone(),
                existing: existing.clone(),
            };
            match ctx.resolve_conflict(&conflict).at(Stage::Conflict)? {
                // The same rule the overwrite policies follow.
                ConflictAction::Overwrite => decide(ConflictPolicy::Overwrite, incoming, &existing),
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
        Decision::Rename => numbered(dst, target, is_dir, os),
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
