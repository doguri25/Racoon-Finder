//! Making links (OPS-08) and taking them away again (OPS-12): a job that makes shortcuts,
//! aliases, symbolic links and junctions, and the undo that removes only the links it made.
//!
//! A link is a new item and never replaces one: its name is `at`, or the first free numbered
//! name when that is taken, and a link job never writes over what is there. The target is only
//! looked at, never changed. A link on the local disk only: the places are local paths.

use std::ffi::OsStr;
use std::io;
use std::path::Path;

use rf_core::{Error, ErrorKind, VPath};
use rf_platform::{LinkKind, file_identity, link_points_to, make_link, remove_link};

use super::simple::name_issue;
use super::{At, Attempt, Ctx, Failed};
use crate::naming::{MAX_NUMBERED, numbered_name};
use crate::preflight::FsKind;
use crate::report::{FailureReason, Stage, Totals};
use crate::spec::ErrorPolicy;
use crate::undo::UndoRecord;

/// A failed link is reported and the job goes on to the next one.
const ERRORS: ErrorPolicy = ErrorPolicy::Continue;

/// One link, as a record keeps it: the link, its kind and the target it points to.
pub(crate) type MadeLink = (VPath, LinkKind, VPath);

/// How a link may be named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Naming {
    /// The first free name from `at` on (`a`, `a (2)`, ...).
    Numbered,
    /// `at` exactly (a redo makes the links again where they were).
    Exact,
}

/// A link that was made.
struct Made {
    path: VPath,
    /// What it points to is not there.
    dangling: bool,
}

/// Makes each of `links` (`target`, `at`, `kind`), one step each, in order. The plan counts one
/// file for each. A link that fails is reported (`Stage::Create`) and the others go on: a
/// target or a place that is not on the local disk (`LinkNotLocal`), a symbolic link that needs
/// rights the process lacks (`LinkNeedsPrivilege`, nothing is tried), a name the file system
/// cannot hold, a kind this system does not make (`ErrorKind::Unsupported`), or an error of the
/// system. A target that is not there still gets its link, with a warning
/// (`LinkTargetMissing`). What was made is in the outputs, and the undo record is
/// `UndoRecord::LinksMade`.
pub(crate) fn run_make_links(ctx: &mut Ctx<'_>, links: &[(VPath, VPath, LinkKind)]) {
    ctx.planned(Totals {
        files: links.len() as u64,
        dirs: 0,
        bytes: 0,
    });
    let mut made = Vec::new();
    for (target, at, kind) in links {
        let step = ctx.attempt(ERRORS, Some(target), Some(at), |ctx| {
            make_link_at(ctx, target, at, *kind, Naming::Numbered, Stage::Create)
        });
        match step {
            Attempt::Done(link) => {
                finish(ctx, &link, target, *kind, &mut made);
            }
            Attempt::Skipped => {}
            Attempt::Stop => break,
        }
    }
    if !made.is_empty() {
        ctx.report.undo = Some(UndoRecord::LinksMade { at: made });
    }
}

/// Notes `link`, which was made for `target`: in the outputs and the record, as one item done,
/// and with a warning when it points to nothing.
fn finish(
    ctx: &mut Ctx<'_>,
    link: &Made,
    target: &VPath,
    kind: LinkKind,
    made: &mut Vec<MadeLink>,
) {
    if link.dangling {
        let missing = Error::io("stat", target.clone(), io::ErrorKind::NotFound.into());
        let warning = Failed::refused(Stage::Create, missing, FailureReason::LinkTargetMissing);
        ctx.warn(warning, Some(target), Some(&link.path));
    }
    ctx.report.outputs.push(link.path.clone());
    made.push((link.path.clone(), kind, target.clone()));
    ctx.item_done(true, false);
}

/// Makes one link of `kind` to `target` at `at` (see [`Naming`]) and gives where it ended up.
/// Every failure is at `stage` and names `at` as its target, whatever number it had got to (its
/// text names the path). Before the file system is asked about a name, it is checked against the
/// file system of the folder and the path limit of the OS, as a new folder's is (`name_issue`):
/// a name that file system cannot hold fails with that issue rather than move on. Only a
/// `NotFound` from `stat` makes a name free; a name that turns out taken when the link is made
/// (another program made one since the `stat`) moves on to the next number.
fn make_link_at(
    ctx: &mut Ctx<'_>,
    target: &VPath,
    at: &VPath,
    kind: LinkKind,
    naming: Naming,
    stage: Stage,
) -> Result<Made, Failed> {
    ctx.check().at(stage)?;
    let Some(target_path) = target.as_local() else {
        return Err(not_local(stage, target));
    };
    if at.as_local().is_none() {
        return Err(not_local(stage, at));
    }
    if kind == LinkKind::Symlink && rf_platform::symlink_needs_privilege() {
        let error = Error::Unsupported(
            "a symbolic link needs Developer Mode or an elevated process".to_owned(),
        );
        return Err(Failed::refused(
            stage,
            error,
            FailureReason::LinkNeedsPrivilege,
        ));
    }
    let (Some(parent), Some(name)) = (at.parent(), at.file_name()) else {
        let error = Error::Unsupported(format!("no folder to make a link in: {at}"));
        return Err(Failed::refused(stage, error, FailureReason::NoTargetFolder));
    };
    let provider = ctx.provider(at).at(stage)?;
    let fs = FsKind::from_fs_name(provider.fs_name(&parent).as_deref());
    let os = ctx.env.naming_os;
    let dangling = ctx
        .provider(target)
        .and_then(|provider| provider.stat(target))
        .is_err_and(|error| error.kind() == ErrorKind::NotFound);
    let numbered = (2..=MAX_NUMBERED).map(|n| numbered_name(name, n, false, os));
    let tried = if naming == Naming::Exact {
        1
    } else {
        usize::MAX
    };
    for candidate in std::iter::once(name.to_owned()).chain(numbered).take(tried) {
        let path = parent
            .join(&candidate)
            .map_err(|error| Failed::new(stage, error.into()))?;
        if let Some(issue) = name_issue(&candidate, &path, fs, os) {
            return Err(Failed::issue(stage, issue));
        }
        match provider.stat(&path) {
            Ok(_) => {
                if naming == Naming::Exact {
                    return Err(taken(stage, &path));
                }
                continue;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(Failed::new(stage, error)),
        }
        let Some(link_path) = path.as_local() else {
            return Err(not_local(stage, &path));
        };
        ctx.working_on(target);
        match make_link(kind, target_path, link_path) {
            Ok(()) => return Ok(Made { path, dangling }),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if naming == Naming::Exact {
                    return Err(taken(stage, &path));
                }
            }
            Err(error) => {
                let error = Error::io("make_link", path.clone(), error);
                return Err(Failed::new(stage, error));
            }
        }
    }
    let error = Error::io("make_link", at.clone(), io::ErrorKind::AlreadyExists.into());
    Err(Failed::refused(stage, error, FailureReason::NoFreeName))
}

/// The failure of a place that is taken, for a link that may be made nowhere else.
fn taken(stage: Stage, path: &VPath) -> Failed {
    let error = Error::io(
        "make_link",
        path.clone(),
        io::ErrorKind::AlreadyExists.into(),
    );
    Failed::new(stage, error)
}

/// The failure of a path that is not on the local disk.
fn not_local(stage: Stage, path: &VPath) -> Failed {
    let error = Error::Unsupported(format!("a link is made on the local disk only: {path}"));
    Failed::refused(stage, error, FailureReason::LinkNotLocal)
}

/// Whether the target `found` that a link says is the one `made_for`, which the job made it
/// for: the same path (on Windows in any letter case, with or without the extended prefix), or
/// the same file. A shortcut may keep its target's path in another form, and an alias follows its
/// target to where it was moved.
pub(crate) fn same_target(found: &Path, made_for: &Path) -> bool {
    if found == made_for || spelled_alike(found.as_os_str(), made_for.as_os_str()) {
        return true;
    }
    let identity = |path: &Path| {
        let path = VPath::local(path).ok()?;
        file_identity(&path).ok()
    };
    matches!((identity(found), identity(made_for)), (Some(a), Some(b)) if a == b)
}

/// Whether two paths are one on Windows once the extended prefix and the letter case are set
/// aside; never elsewhere.
fn spelled_alike(a: &OsStr, b: &OsStr) -> bool {
    if !cfg!(windows) {
        return false;
    }
    let plain = |path: &OsStr| {
        let text = path.to_string_lossy().to_lowercase();
        text.strip_prefix(r"\\?\")
            .map_or(text.clone(), str::to_owned)
    };
    plain(a) == plain(b)
}

/// Undoes `LinksMade`: takes away each link (`at`, `kind`, `target`) that is still a link of
/// that kind to that target, one step each. The target is never touched. A link that is gone is
/// skipped. Anything else at its path (a file or a folder that took its place, a link of
/// another kind, a link pointed elsewhere) stays, and fails as `Stage::Undo` with the reason
/// `ChangedSinceLinked`. The plan counts one file for each, and the undo's record makes the
/// links that were removed again (`UndoRecord::LinksRemoved`).
pub(crate) fn undo_links(ctx: &mut Ctx<'_>, links: &[MadeLink]) {
    ctx.planned(Totals {
        files: links.len() as u64,
        dirs: 0,
        bytes: 0,
    });
    let mut removed = Vec::new();
    for (at, kind, target) in links {
        let step = ctx.attempt(ERRORS, Some(at), None, |ctx| unmake(ctx, at, *kind, target));
        match step {
            Attempt::Done(true) => {
                removed.push((at.clone(), *kind, target.clone()));
                ctx.item_done(true, false);
            }
            Attempt::Done(false) => ctx.report.skipped += 1,
            Attempt::Skipped => {}
            Attempt::Stop => break,
        }
    }
    if !removed.is_empty() {
        ctx.report.undo = Some(UndoRecord::LinksRemoved { links: removed });
    }
}

/// Removes the link `at`, if it is a link of `kind` to `target`; `false` when it is gone.
fn unmake(ctx: &Ctx<'_>, at: &VPath, kind: LinkKind, target: &VPath) -> Result<bool, Failed> {
    ctx.check().at(Stage::Undo)?;
    let (Some(at_path), Some(target_path)) = (at.as_local(), target.as_local()) else {
        return Err(not_local(Stage::Undo, at));
    };
    let io = |op: &'static str, error: io::Error| {
        Failed::new(Stage::Undo, Error::io(op, at.clone(), error))
    };
    match link_points_to(at_path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io("link_points_to", error)),
        Ok(Some((found, to))) if found == kind && same_target(&to, target_path) => {}
        Ok(_) => {
            let error = Error::Unsupported(format!(
                "{at} is no longer the link the job made; left in place"
            ));
            return Err(Failed::refused(
                Stage::Undo,
                error,
                FailureReason::ChangedSinceLinked,
            ));
        }
    }
    // No wait for a pause between the last look and the removal.
    ctx.check_cancelled().at(Stage::Undo)?;
    match remove_link(at_path, kind) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io("remove_link", error)),
    }
}

/// Undoes `LinksRemoved` (a redo): makes each link (`at`, `kind`, `target`) again under the very
/// same name, one step each. Anything now at that name keeps it and the link fails
/// (`AlreadyExists`, `Stage::Undo`). The plan counts one file for each, and the undo's record
/// is `UndoRecord::LinksMade` again.
pub(crate) fn remake_links(ctx: &mut Ctx<'_>, links: &[MadeLink]) {
    ctx.planned(Totals {
        files: links.len() as u64,
        dirs: 0,
        bytes: 0,
    });
    let mut made = Vec::new();
    for (at, kind, target) in links {
        let step = ctx.attempt(ERRORS, None, Some(at), |ctx| {
            make_link_at(ctx, target, at, *kind, Naming::Exact, Stage::Undo)
        });
        match step {
            Attempt::Done(link) => finish(ctx, &link, target, *kind, &mut made),
            Attempt::Skipped => {}
            Attempt::Stop => break,
        }
    }
    if !made.is_empty() {
        ctx.report.undo = Some(UndoRecord::LinksMade { at: made });
    }
}
