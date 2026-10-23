//! The plan of a transfer: what each source is, whether its target folder exists, how much the
//! job will copy, and what the target file systems cannot hold (XFER-12).

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Component, PathBuf};
use std::sync::Arc;

use rf_core::{Entry, EntryKind, Error, ErrorKind, Os, VPath};
use rf_vfs::{Capabilities, Provider};

use super::{At, Attempt, Ctx, Failed, Level, MAX_DEPTH, Stopped, fold_path, folder_identity};
use crate::preflight::{
    CaseIndex, FsKind, Issue, IssueKind, Severity, check_name, check_path_len, check_size,
};
use crate::report::{Stage, Totals};
use crate::resolver::PreflightAnswer;
use crate::spec::{SymlinkMode, TransferItem, TransferMode, TransferSpec};

/// One item the plan keeps, with what the source was when it was planned.
#[derive(Debug)]
pub(crate) struct Planned {
    pub item: TransferItem,
    /// The source as `stat` saw it (a link is described itself, not what it leads to).
    pub entry: Entry,
    /// The kind of file system the target folder is on; every item under this one is written
    /// to the same.
    pub fs: FsKind,
    /// A move the plan expects to end with one rename, because the source and the target
    /// folder are on one volume of one provider. It is counted alone, as one item of no bytes,
    /// and the tree under it is neither walked nor checked: the names in it are on that file
    /// system already.
    pub rename: bool,
}

/// What a transfer will do. What it expects to process is in the report (`planned`).
#[derive(Debug)]
pub(crate) struct Plan {
    pub items: Vec<Planned>,
    /// Targets the pre-flight review left out, each with the first of its blocking issues; each
    /// fails as `Stage::Preflight` and is not written.
    pub blocked: HashMap<VPath, IssueKind>,
    /// A move that copies instead and deletes no source, because the folder of one of its
    /// sources cannot be written to (DND-06).
    pub moved_as_copy: bool,
}

/// Whether the links of `spec` are followed: in a copy that asks for it. A move always moves
/// links as links, so that it never deletes anything through one.
pub(crate) fn follows_links(spec: &TransferSpec) -> bool {
    spec.mode == TransferMode::Copy && spec.options.symlinks == SymlinkMode::Follow
}

/// Plans `spec` and puts its pre-flight review to the resolver.
///
/// Each source is `stat`ed (a link itself, not what it leads to) under the job's error policy;
/// its target folder must exist, and a folder may not be copied into itself or one of its
/// subfolders (on a target that ignores case, spelled in any case). Then the tree under it is
/// walked, `MAX_DEPTH` levels deep at most. A file counts with its size; a link copied as a
/// link counts as a file of no bytes; a folder counts as a folder. In Follow mode a link counts
/// as what it leads to, and a folder it leads to is walked, unless it is one of the folders
/// above it (a loop of links, known by identity; without identities the depth limit ends it).
/// A FIFO, socket or device is not counted, checked or opened: the run reports it. Nothing
/// found below a source fails the plan; the run meets it again and reports it then.
///
/// A move whose sources include one in a folder that cannot be written to (no
/// `Capabilities::WRITE`) is planned as a copy, all of it (`moved_as_copy`, and
/// `copied_instead_of_moved` in the report, DND-06). Otherwise a source of a move that is on
/// the volume of its target folder (both `stat`s give the same volume ID, of one provider) is
/// expected to end with one rename (`Planned::rename`): it counts as one file or folder of no
/// bytes, and its tree is not walked, so moving a large folder within a volume never lists
/// it. A source whose volume is another or unknown is walked as for a copy.
///
/// Every target name, path and file size is checked against the file system of its top-level
/// target folder, names that only differ in case within one folder are looked for, and each
/// such folder must have room for the bytes copied into it. The issues go to the report, and
/// the totals are announced. If there are issues and the job may ask, the resolver decides:
/// go on as planned, leave out the targets with a blocking issue (a lack of space only warns
/// here), or cancel the job.
///
/// `None` when the job was cancelled or aborted.
pub(crate) fn scan_transfer(ctx: &mut Ctx<'_>, spec: &TransferSpec) -> Option<Plan> {
    let mut survey = Survey::new(ctx.env.naming_os, follows_links(spec));
    let mut found = Vec::with_capacity(spec.items.len());
    for item in &spec.items {
        let (source, target) = (&item.source, &item.target);
        match ctx.attempt(spec.options.errors, Some(source), Some(target), |ctx| {
            scan_item(ctx, &mut survey, item)
        }) {
            Attempt::Done(scanned) => found.push((item, scanned)),
            Attempt::Skipped => {}
            Attempt::Stop => return None,
        }
    }
    let moving = spec.mode == TransferMode::Move;
    // Many sources of one job are in one folder, which is asked about once.
    let mut writable = HashMap::new();
    let moved_as_copy = moving
        && found
            .iter()
            .any(|(item, _)| !source_folder_writable(ctx, &item.source, &mut writable));
    ctx.report.copied_instead_of_moved = moved_as_copy;
    let mut items = Vec::with_capacity(found.len());
    for (item, scanned) in found {
        let planned = Planned {
            item: item.clone(),
            entry: scanned.entry,
            fs: survey.folders[scanned.folder].fs,
            rename: moving && !moved_as_copy && scanned.same_volume,
        };
        if planned.rename {
            survey.count_alone(&planned.entry, &item.target, scanned.folder);
        } else {
            let walked = survey.walk(
                ctx,
                &*scanned.src,
                &item.source,
                planned.entry.clone(),
                &item.target,
                scanned.folder,
                Level::TOP,
            );
            if walked.is_err() {
                ctx.stop(Stopped::Cancelled);
                return None;
            }
        }
        items.push(planned);
    }
    survey.check_space(ctx);
    ctx.report.issues = survey.issues;
    ctx.planned(survey.totals);
    let blocked = review(ctx)?;
    Some(Plan {
        items,
        blocked,
        moved_as_copy,
    })
}

/// A source of the job as `scan_item` found it.
#[derive(Debug)]
struct Scanned {
    /// The provider of the source.
    src: Arc<dyn Provider>,
    /// The source as `stat` sees it.
    entry: Entry,
    /// The number of its top-level target folder in the survey.
    folder: usize,
    /// Whether the source and its target folder are of one provider and, by the volume IDs of
    /// their `stat`s, on one volume. An unknown volume ID is not the same volume.
    same_volume: bool,
}

/// The source of `item` with its provider and entry, and the number of its target folder in
/// `survey`, once that folder is known to exist and to lie outside the source.
fn scan_item(
    ctx: &mut Ctx<'_>,
    survey: &mut Survey,
    item: &TransferItem,
) -> Result<Scanned, Failed> {
    let (source, target) = (&item.source, &item.target);
    ctx.check().at(Stage::Plan)?;
    let src = ctx.provider(source).at(Stage::Plan)?;
    let entry = src.stat(source).at(Stage::Plan)?;
    let (parent, folder_entry) = check_target_folder(ctx, target).at(Stage::Plan)?;
    let dst = ctx.provider(&parent).at(Stage::Plan)?;
    let folder = survey.folder(&*dst, &parent);
    if lands_inside(source, target, survey.folders[folder].fs) {
        let error = Error::Unsupported(format!("cannot copy or move {source} into itself"));
        return Err(Failed::new(Stage::Plan, error));
    }
    let same_volume = src.scheme() == dst.scheme()
        && matches!(
            (entry.volume_id, folder_entry.volume_id),
            (Some(a), Some(b)) if a == b
        );
    Ok(Scanned {
        src,
        entry,
        folder,
        same_volume,
    })
}

/// Whether the folder that holds `source` can be written to, so that a move can take the
/// source out of it. A source in no folder (a volume root) cannot be taken out of one. `known`
/// keeps the answer for each folder asked about, so that its provider is asked once.
fn source_folder_writable(ctx: &Ctx<'_>, source: &VPath, known: &mut HashMap<VPath, bool>) -> bool {
    let Some(parent) = source.parent() else {
        return false;
    };
    *known.entry(parent).or_insert_with_key(|parent| {
        ctx.provider(parent)
            .is_ok_and(|src| src.capabilities(parent).contains(Capabilities::WRITE))
    })
}

/// The folder `target` goes into, and what `stat` says it is. Fails with `NotFound` unless it
/// exists; a link to a folder counts as a folder (macOS `/tmp`, a Windows junction), and the
/// entry is then that of the folder it leads to.
fn check_target_folder(ctx: &Ctx<'_>, target: &VPath) -> rf_core::Result<(VPath, Entry)> {
    let parent = target
        .parent()
        .ok_or_else(|| Error::Unsupported(format!("no folder to copy into: {target}")))?;
    let dst = ctx.provider(&parent)?;
    let mut folder = dst.stat(&parent)?;
    if folder.kind == EntryKind::Symlink {
        folder = dst.stat_target(&parent)?;
    }
    if folder.kind == EntryKind::Dir {
        Ok((parent, folder))
    } else {
        let error = io::Error::new(io::ErrorKind::NotFound, "not a folder");
        Err(Error::io("stat", parent, error))
    }
}

/// Whether `target` lies inside `source`, a copy into itself (DND-09). The paths are compared
/// name by name, and on a file system `fs` that ignores case by their folded keys too, so
/// `C:\Work\A` and `C:\work\a\sub` count. A target that only spells `source` in another case
/// is not inside it.
fn lands_inside(source: &VPath, target: &VPath, fs: FsKind) -> bool {
    if target != source && target.starts_with(source) {
        return true;
    }
    if !fs.case_insensitive() {
        return false;
    }
    let (source, target) = (fold_path(source, fs), fold_path(target, fs));
    target.len() > source.len() && target.starts_with(&source)
}

/// Asks the resolver about the issues in the report, if there are any and the job may ask
/// (a job that may not goes on as planned). The answer gives the targets to leave out: none to
/// go on, those with a blocking issue other than a lack of space to skip them. `None` when the
/// answer cancels the job, which cancels its control.
fn review(ctx: &mut Ctx<'_>) -> Option<HashMap<VPath, IssueKind>> {
    let mut blocked = HashMap::new();
    if ctx.report.issues.is_empty() || !ctx.interactive {
        return Some(blocked);
    }
    let answer = ctx.waiting(|| ctx.env.resolver.preflight(ctx.id, &ctx.report.issues));
    match answer {
        PreflightAnswer::Proceed => {}
        PreflightAnswer::SkipBlocked => {
            let skipped = ctx.report.issues.iter().filter(|issue| {
                issue.kind.severity() == Severity::Blocking
                    && !matches!(issue.kind, IssueKind::NotEnoughSpace { .. })
            });
            for issue in skipped {
                blocked
                    .entry(issue.target.clone())
                    .or_insert_with(|| issue.kind.clone());
            }
        }
        PreflightAnswer::Cancel => {
            ctx.env.control.cancel();
            return None;
        }
    }
    Some(blocked)
}

/// A top-level target folder: the file system it is on and the bytes the job copies into it.
#[derive(Debug)]
struct Folder {
    path: VPath,
    fs: FsKind,
    needed: u64,
}

/// What the walk of the sources has found so far.
#[derive(Debug)]
struct Survey {
    /// The naming rule of the job, whose OS limits the length of a path.
    os: Os,
    /// Whether links are followed (see [`follows_links`]).
    follow: bool,
    folders: Vec<Folder>,
    /// The names going into each target folder, for each kind of file system.
    cases: HashMap<FsKind, CaseIndex>,
    issues: Vec<Issue>,
    totals: Totals,
}

impl Survey {
    fn new(os: Os, follow: bool) -> Survey {
        Survey {
            os,
            follow,
            folders: Vec::new(),
            cases: HashMap::new(),
            issues: Vec::new(),
            totals: Totals::default(),
        }
    }

    /// The number of the top-level target folder `path` of `dst`. The file system of a folder
    /// is asked once, the first time it is seen.
    fn folder(&mut self, dst: &dyn Provider, path: &VPath) -> usize {
        if let Some(known) = self.folders.iter().position(|f| f.path == *path) {
            return known;
        }
        self.folders.push(Folder {
            path: path.clone(),
            fs: FsKind::from_fs_name(dst.fs_name(path).as_deref()),
            needed: 0,
        });
        self.folders.len() - 1
    }

    /// Counts and checks `source` of `src`, which `entry` describes and which goes to `target`
    /// in the top-level folder number `folder`, and everything under it; `level` says how far
    /// below a source of the job it is and which folders are above it. An item deeper than
    /// [`MAX_DEPTH`], a followed folder that is one of the folders above it (a loop of links),
    /// and an item that cannot be followed or listed are left uncounted. Fails only when the
    /// job is cancelled.
    #[expect(
        clippy::too_many_arguments,
        reason = "one walk step: the item, where it goes and where it is in the walk"
    )]
    fn walk(
        &mut self,
        ctx: &Ctx<'_>,
        src: &dyn Provider,
        source: &VPath,
        entry: Entry,
        target: &VPath,
        folder: usize,
        level: Level<'_>,
    ) -> rf_core::Result<()> {
        if level.depth() > MAX_DEPTH {
            return Ok(());
        }
        let (source, entry) = if entry.kind == EntryKind::Symlink && self.follow {
            match follow_link(src, source) {
                Ok((_, found))
                    if found.kind == EntryKind::Dir && level.leads_back(found.identity()) =>
                {
                    return Ok(());
                }
                Ok(followed) => followed,
                Err(_) => return Ok(()),
            }
        } else {
            (source.clone(), entry)
        };
        let fs = self.folders[folder].fs;
        match entry.kind {
            EntryKind::Other => {}
            EntryKind::File | EntryKind::Symlink => {
                let size = if entry.kind == EntryKind::File {
                    entry.size
                } else {
                    0
                };
                self.totals.files += 1;
                self.totals.bytes += size;
                self.folders[folder].needed += size;
                self.check(target, fs);
                if let Some(kind) = check_size(size, fs) {
                    self.issue(target, kind);
                }
            }
            EntryKind::Dir => {
                self.totals.dirs += 1;
                self.check(target, fs);
                ctx.check()?;
                let children = match ctx.list(src, &source) {
                    Ok(children) => children,
                    Err(error) if error.kind() == ErrorKind::Cancelled => return Err(error),
                    Err(_) => return Ok(()),
                };
                let here = level.enter(folder_identity(self.follow, src, &source, &entry));
                for child in children {
                    let (Ok(child_source), Ok(child_target)) =
                        (source.join(&child.name), target.join(&child.name))
                    else {
                        continue;
                    };
                    self.walk(
                        ctx,
                        src,
                        &child_source,
                        child,
                        &child_target,
                        folder,
                        here.below(),
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Counts `entry`, which a move takes to `target` in the top-level folder number `folder`
    /// with one rename, as one file or folder of no bytes, and checks its name. What is under
    /// it is on that file system already, and is not walked.
    fn count_alone(&mut self, entry: &Entry, target: &VPath, folder: usize) {
        if entry.kind == EntryKind::Dir {
            self.totals.dirs += 1;
        } else {
            self.totals.files += 1;
        }
        let fs = self.folders[folder].fs;
        self.check(target, fs);
    }

    /// Checks the name and the whole path of `target` on `fs`, and its name against the other
    /// names that go into the same folder.
    fn check(&mut self, target: &VPath, fs: FsKind) {
        let Some(name) = target.file_name() else {
            return;
        };
        let mut found = check_name(name, fs);
        found.extend(check_path_len(target, self.os));
        if let Some(parent) = target.parent() {
            let index = self.cases.entry(fs).or_insert_with(|| CaseIndex::new(fs));
            if let Some(other) = index.insert(&parent, name) {
                found.push(IssueKind::CaseCollision { other });
            }
        }
        for kind in found {
            self.issue(target, kind);
        }
    }

    fn issue(&mut self, target: &VPath, kind: IssueKind) {
        self.issues.push(Issue {
            target: target.clone(),
            kind,
        });
    }

    /// Adds [`IssueKind::NotEnoughSpace`] for each top-level target folder that has less space
    /// available than the job copies into it. A folder whose space cannot be told is not
    /// checked.
    fn check_space(&mut self, ctx: &Ctx<'_>) {
        for folder in &self.folders {
            if folder.needed == 0 {
                continue;
            }
            let space = ctx
                .provider(&folder.path)
                .and_then(|dst| dst.space(&folder.path));
            if let Ok(Some(space)) = space {
                if folder.needed > space.available {
                    self.issues.push(Issue {
                        target: folder.path.clone(),
                        kind: IssueKind::NotEnoughSpace {
                            needed: folder.needed,
                            available: space.available,
                        },
                    });
                }
            }
        }
    }
}

/// What the link at `path` of `provider` leads to: the path of that item, with every link on
/// the way resolved (see [`resolve_links`]), and the entry `Provider::stat_target` gives for it,
/// under the name of the link. The entry is never of kind `Symlink`; one of kind `Other` is
/// never opened. A provider that cannot follow links fails with `Unsupported`.
pub(crate) fn follow_link(
    provider: &dyn Provider,
    path: &VPath,
) -> rf_core::Result<(VPath, Entry)> {
    let entry = provider.stat_target(path)?;
    let (resolved, found) = resolve_links(provider, path)?;
    // Both describe one item, unless a link changed between the two.
    let changed = matches!(
        (entry.identity(), found.identity()),
        (Some(a), Some(b)) if a != b
    );
    if changed {
        let error = io::Error::other("the link changed while it was followed");
        return Err(Error::io("stat", path.clone(), error));
    }
    Ok((resolved, entry))
}

/// The most links [`resolve_links`] follows for one path, as Linux does (`MAXSYMLINKS`).
const MAX_LINKS: usize = 40;

/// One name, or one step up, still to be taken by [`resolve_links`].
#[derive(Debug)]
enum Step {
    Name(OsString),
    Up,
}

/// The path that `path` of `provider` leads to with every link on the way resolved, the last
/// name too, and the entry of the item there, which is no link: the item `stat_target`
/// describes, under a path of its own.
///
/// The names are taken from the root one at a time. A name that is a link is replaced by what
/// the link stores: an absolute target starts again at its root, a relative one goes on from
/// the folder resolved so far. That folder is reached through no link, so a `..` in a target
/// goes up from the folder the link is really in, as Unix resolves it. More than
/// [`MAX_LINKS`] links fail like a loop of links.
fn resolve_links(provider: &dyn Provider, path: &VPath) -> rf_core::Result<(VPath, Entry)> {
    let mut steps = Vec::new();
    let mut at = path.clone();
    while let (Some(parent), Some(name)) = (at.parent(), at.file_name()) {
        steps.push(Step::Name(name.to_owned()));
        at = parent;
    }
    // `at` is the root; the next step to take is the last one in `steps`.
    let mut links = 0;
    while let Some(step) = steps.pop() {
        let name = match step {
            Step::Up => {
                at = at.parent().unwrap_or(at);
                continue;
            }
            Step::Name(name) => name,
        };
        let next = at.join(&name)?;
        if provider.stat(&next)?.kind != EntryKind::Symlink {
            at = next;
            continue;
        }
        links += 1;
        if links > MAX_LINKS {
            let error = io::Error::other("too many levels of links");
            return Err(Error::io("stat", path.clone(), error));
        }
        let target = provider.read_link(&next)?.target;
        let root: PathBuf = target
            .components()
            .take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
            .collect();
        if !root.as_os_str().is_empty() {
            // An absolute target is a host path; another provider would have to say what one
            // means.
            if at.as_local().is_none() {
                let what = format!("following an absolute link: {next}");
                return Err(Error::Unsupported(what));
            }
            at = VPath::local(&root)?;
        }
        for component in target.components().rev() {
            match component {
                Component::Normal(name) => steps.push(Step::Name(name.to_owned())),
                Component::ParentDir => steps.push(Step::Up),
                Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            }
        }
    }
    let entry = provider.stat(&at)?;
    Ok((at, entry))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(path: impl AsRef<std::path::Path>) -> VPath {
        VPath::local(path).unwrap()
    }

    #[test]
    fn into_itself_is_name_by_name_and_folds_case_only_where_the_target_does() {
        let base = std::env::temp_dir();
        let (a, sub) = (
            v(base.join("Work").join("A")),
            v(base.join("work").join("a").join("sub")),
        );
        assert!(lands_inside(&a, &a.join("sub").unwrap(), FsKind::Other));
        assert!(!lands_inside(&a, &a, FsKind::Ntfs));
        assert!(!lands_inside(
            &a,
            &v(base.join("Work").join("AB")),
            FsKind::Ntfs
        ));
        assert!(!lands_inside(&a, &sub, FsKind::Other));
        assert!(lands_inside(&a, &sub, FsKind::Ntfs));
        // Only the case differs: the same folder, not one inside it.
        assert!(!lands_inside(
            &a,
            &v(base.join("work").join("a")),
            FsKind::Ntfs
        ));
    }

    #[cfg(unix)]
    #[test]
    fn links_resolve_from_the_folder_they_are_really_in() {
        use std::os::unix::fs::symlink;

        use rf_vfs::LocalProvider;
        let dir = tempfile::tempdir().unwrap();
        // Through the link `top/in`, the `..` of `x` goes up to `real`; as text it would be
        // `top`.
        let root = dir.path().canonicalize().unwrap();
        for d in ["top", "real/deep"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("real").join("y.txt"), "y").unwrap();
        symlink("../real/deep", root.join("top").join("in")).unwrap();
        symlink("../y.txt", root.join("real").join("deep").join("x")).unwrap();
        symlink(root.join("top"), root.join("abs")).unwrap();
        symlink("loop", root.join("loop")).unwrap();
        let local = LocalProvider::new();
        let resolve = |p: PathBuf| resolve_links(&local, &v(p)).map(|(path, _)| path);
        let x = root.join("abs").join("in").join("x");
        assert_eq!(
            resolve(x.clone()).unwrap(),
            v(root.join("real").join("y.txt"))
        );
        let (path, entry) = follow_link(&local, &v(&x)).unwrap();
        assert_eq!(
            (path, entry.name, entry.size),
            (v(root.join("real").join("y.txt")), "x".into(), 1)
        );
        assert_eq!(resolve(root.join("top")).unwrap(), v(root.join("top")));
        assert!(resolve(root.join("loop")).is_err());
    }
}
