//! Running a copy or a move: plan it, then transfer the planned items one by one.

use std::io;

use rf_core::{Entry, EntryKind, Error, ErrorKind, Os, VPath};
use rf_vfs::{Provider, RenameMode};

use super::file::copy_file;
use super::scan::scan_transfer;
use super::{At, Attempt, Ctx, Failed, issue_error};
use crate::conflict::{Conflict, ConflictAction, Decision, decide};
use crate::naming::{MAX_NUMBERED, numbered_name};
use crate::preflight::{FsKind, IssueKind, check_name, check_path_len};
use crate::report::Stage;
use crate::spec::{ConflictPolicy, TransferMode, TransferSpec};

/// Plans `spec` and copies each planned item to its target under the job's conflict and error
/// policies.
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
        let (source, target) = (&planned.item.source, &planned.item.target);
        let copied = ctx.attempt(spec.options.errors, Some(source), Some(target), |ctx| {
            // A cancel between items creates nothing more.
            ctx.check().at(Stage::Write)?;
            let src = ctx.provider(source).at(Stage::Read)?;
            let dst = ctx.provider(target).at(Stage::Write)?;
            // Only a regular file can be copied (and so can clash): `copy_file` refuses any
            // other source before it touches anything, and nobody is asked about those.
            let (to, commit) = if planned.entry.kind == EntryKind::File {
                let policy = spec.options.conflict;
                match place(ctx, policy, source, &planned.entry, &*dst, target)? {
                    Place::Skip => return Ok(None),
                    Place::At { path, commit } => (path, commit),
                }
            } else {
                (target.clone(), RenameMode::NoReplace)
            };
            copy_file(ctx, &src, source, &planned.entry, &dst, &to, commit)?;
            Ok(Some(to))
        });
        match copied {
            Attempt::Done(Some(to)) => {
                // Before `item_done`, which may call the listener: a committed file is in
                // the outputs whatever the listener does.
                ctx.report.outputs.push(to);
                ctx.item_done(true, false);
            }
            Attempt::Done(None) => ctx.report.skipped += 1,
            Attempt::Skipped => {}
            Attempt::Stop => return,
        }
    }
}

/// Where a file goes.
#[derive(Debug)]
enum Place {
    /// Nowhere: the existing item stays and the file is left out.
    Skip,
    /// To `path`, committed with `commit`.
    At { path: VPath, commit: RenameMode },
}

/// Decides where the file `source`, which `incoming` describes, is written: what happens
/// when `target` is taken by something already.
///
/// 1. `stat` says `NotFound`: the name is free. Any other `stat` error fails the item, since a
///    target that cannot be examined may well exist.
/// 2. The existing item is the source itself (the same path, or two identities that are both
///    known and equal): the file gets a numbered name whatever the policy says, so that a
///    copy onto itself never replaces or empties its own source.
/// 3. Otherwise the policy decides. [`Decision::Ask`] goes to the resolver, and its
///    `Overwrite` answer is refused when either side is a folder, as it would be for the
///    overwrite policies: a file and a folder never replace each other. A refused conflict
///    fails the item as `Stage::Conflict` with `AlreadyExists` and touches neither side.
fn place(
    ctx: &mut Ctx<'_>,
    policy: ConflictPolicy,
    source: &VPath,
    incoming: &Entry,
    dst: &dyn Provider,
    target: &VPath,
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
    if is_same_item(source, incoming, target, &existing) {
        return numbered(dst, target, os);
    }
    let decision = match decide(policy, incoming, &existing) {
        Decision::Ask => {
            let conflict = Conflict {
                source: source.clone(),
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
        Decision::Rename => numbered(dst, target, os),
        // An `Ask` is answered by now; one that is not fails closed like a refusal.
        Decision::Refuse | Decision::Ask => Err(Failed::new(Stage::Conflict, taken(target))),
    }
}

/// Whether the item `existing` at `target` is the source `source` that `incoming` describes.
/// Two identities count only when both are known: a file system without file IDs gives none,
/// and two unknowns are not proof of the same file.
fn is_same_item(source: &VPath, incoming: &Entry, target: &VPath, existing: &Entry) -> bool {
    source == target
        || matches!(
            (incoming.identity(), existing.identity()),
            (Some(a), Some(b)) if a == b
        )
}

/// A free numbered name next to `target`, to commit a file to without replacing anything.
fn numbered(dst: &dyn Provider, target: &VPath, os: Os) -> Result<Place, Failed> {
    let path = free_name(dst, target, false, os).at(Stage::Conflict)?;
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
/// with [`ErrorKind::InvalidPath`] before the file system is asked, since every later number
/// is at least as long. When all the names are taken, fails with `AlreadyExists` for
/// `target`.
pub(crate) fn free_name(
    provider: &dyn Provider,
    target: &VPath,
    is_dir: bool,
    os: Os,
) -> rf_core::Result<VPath> {
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        return Err(Error::Unsupported(format!("no name to number: {target}")));
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
            return Err(issue_error(&issue));
        }
        match provider.stat(&candidate) {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(candidate),
            Err(error) => return Err(error),
        }
    }
    Err(taken(target))
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
        let same =
            |incoming: &Entry, existing: &Entry| is_same_item(&source, incoming, &target, existing);
        let known = entry(Some(1), Some(7));
        assert!(same(&known, &entry(Some(1), Some(7))));
        assert!(!same(&known, &entry(Some(1), Some(8))));
        assert!(!same(&known, &entry(Some(2), Some(7))));
        // Unknown is not proof of anything: two files without IDs are not one file.
        assert!(!same(&entry(None, None), &entry(None, None)));
        assert!(!same(&known, &entry(Some(1), None)));
        assert!(!same(&entry(None, Some(7)), &entry(None, Some(7))));
        // The same path is the same item whatever the IDs say.
        assert!(is_same_item(
            &source,
            &entry(None, None),
            &source,
            &entry(None, None)
        ));
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
        assert!(free_name(&local, &target, false, Os::MacOs).is_ok());
    }
}
