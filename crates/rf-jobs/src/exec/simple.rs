//! The jobs of one step: renaming an item (OPS-02) and making a new empty folder or file
//! (OPS-01), each with the record that undoes it (OPS-12). Neither ever replaces anything.

use std::ffi::OsStr;
use std::io;

use rf_core::{EntryKind, Error, ErrorKind, Os, VPath};
use rf_vfs::RenameMode;

use super::{At, Attempt, Ctx, Failed};
use crate::naming::{MAX_NUMBERED, numbered_name};
use crate::preflight::{FsKind, IssueKind, check_name, check_path_len};
use crate::report::{Stage, Totals};
use crate::spec::{CreateKind, ErrorPolicy};
use crate::undo::UndoRecord;

/// The error policy of a job of one step: its failure is recorded and ends it, without a
/// question.
const ERRORS: ErrorPolicy = ErrorPolicy::Continue;

/// The first thing wrong with `name` as the name of the new item `path` on the file system
/// `fs` and under the path limit of `os` (see [`check_name`] and [`check_path_len`]), or
/// `None`. Any issue refuses the name, a warning too: a rename or a create never gives an
/// item a name its file system cannot hold or that Windows would not open (a reserved name,
/// a trailing dot or space), which the `\\?\` paths of the local provider would write
/// literally.
fn name_issue(name: &OsStr, path: &VPath, fs: FsKind, os: Os) -> Option<IssueKind> {
    check_name(name, fs)
        .into_iter()
        .next()
        .or_else(|| check_path_len(path, os))
}

/// One folder (`is_dir`) or one file, as a plan of one step counts it.
fn one(is_dir: bool) -> Totals {
    Totals {
        files: u64::from(!is_dir),
        dirs: u64::from(is_dir),
        bytes: 0,
    }
}

/// Renames `from` to `to`, never replacing anything there.
///
/// The plan `stat`s `from` (`Stage::Plan`, a link itself) and counts it as one folder or one
/// file by its kind; no file is read. Two paths of different schemes are served by different
/// providers, which no rename joins: that fails as `Stage::Rename` with `Unsupported`. The new
/// name is checked against the file system of its folder first (see [`name_issue`]): any issue
/// fails as `Stage::Rename` with it (`ErrorKind::InvalidPath`), and nothing is renamed. Then
/// the provider renames with `RenameMode::NoReplace` (`Stage::Rename`), so a taken `to` fails
/// with `AlreadyExists` and both items stay as they are. A change of only the letter case of a
/// name works on a file system that ignores case (the provider sees to it); a change of only
/// the Unicode normalization of a name can fail with `AlreadyExists` on APFS, which takes both
/// spellings for one name.
///
/// Once renamed, `to` is the output and the record that undoes the job is
/// `UndoRecord::Renamed`.
pub(crate) fn run_rename(ctx: &mut Ctx<'_>, from: &VPath, to: &VPath) {
    let planned = ctx.attempt(ERRORS, Some(from), Some(to), |ctx| {
        ctx.check().at(Stage::Plan)?;
        let provider = ctx.provider(from).at(Stage::Plan)?;
        provider.stat(from).at(Stage::Plan)
    });
    let entry = match planned {
        Attempt::Done(entry) => entry,
        Attempt::Skipped => {
            ctx.planned(Totals::default());
            return;
        }
        Attempt::Stop => return,
    };
    let is_dir = entry.kind == EntryKind::Dir;
    ctx.planned(one(is_dir));
    let renamed = ctx.attempt(ERRORS, Some(from), Some(to), |ctx| {
        ctx.check().at(Stage::Rename)?;
        if from.scheme() != to.scheme() {
            let error =
                Error::Unsupported(format!("cannot rename across locations: {from} -> {to}"));
            return Err(Failed::new(Stage::Rename, error));
        }
        let provider = ctx.provider(from).at(Stage::Rename)?;
        if let (Some(folder), Some(name)) = (to.parent(), to.file_name()) {
            let fs = FsKind::from_fs_name(provider.fs_name(&folder).as_deref());
            if let Some(issue) = name_issue(name, to, fs, ctx.env.naming_os) {
                return Err(Failed::issue(Stage::Rename, issue));
            }
        }
        ctx.working_on(from);
        provider
            .rename(from, to, RenameMode::NoReplace)
            .at(Stage::Rename)
    });
    if let Attempt::Done(()) = renamed {
        ctx.report.outputs.push(to.clone());
        ctx.report.undo = Some(UndoRecord::Renamed {
            from: from.clone(),
            to: to.clone(),
        });
        ctx.item_done(!is_dir, is_dir);
    }
}

/// Makes a new empty folder or file (`kind`) called `name` in `parent`, or under the first
/// free numbered name when `name` is taken (see [`make_new`]). The plan counts one folder or
/// one file and touches nothing.
///
/// Once made, its path is the output and the record that undoes the job is
/// `UndoRecord::Created`.
pub(crate) fn run_create(ctx: &mut Ctx<'_>, parent: &VPath, name: &OsStr, kind: CreateKind) {
    let is_dir = kind == CreateKind::Dir;
    ctx.planned(one(is_dir));
    let made = ctx.attempt(ERRORS, None, Some(parent), |ctx| {
        make_new(ctx, parent, name, is_dir)
    });
    if let Attempt::Done(path) = made {
        ctx.report.outputs.push(path.clone());
        ctx.report.undo = Some(UndoRecord::Created { path, kind });
        ctx.item_done(!is_dir, is_dir);
    }
}

/// Makes a new empty folder (`is_dir`) or file in `parent` and gives its path. Every failure is
/// `Stage::Create` and names the path it was for.
///
/// The names tried are `name`, then its numbered names (`name (2)`, ..., up to
/// [`MAX_NUMBERED`]; `name 2` on macOS) by the job's naming rule. Before the file system is
/// asked about one, it is checked against the file system of `parent` and the path length of
/// the OS: a name that file system cannot hold, or that Windows would not open (a reserved name,
/// a trailing dot or space), fails with that issue (`ErrorKind::InvalidPath`) rather than move
/// on: `name` itself is not one to make, and a later number is only longer than one that is
/// too long already. Then only a `NotFound` from `stat` makes a name free; any other `stat`
/// error fails, since a name that cannot be examined may well be taken. A name that turns out
/// taken when the item is made (`AlreadyExists`, another program made one since the `stat`)
/// moves on to the next number. When every name is taken, fails with `AlreadyExists` for
/// `name`.
fn make_new(
    ctx: &mut Ctx<'_>,
    parent: &VPath,
    name: &OsStr,
    is_dir: bool,
) -> Result<VPath, Failed> {
    ctx.check().at(Stage::Create)?;
    let provider = ctx.provider(parent).at(Stage::Create)?;
    let fs = FsKind::from_fs_name(provider.fs_name(parent).as_deref());
    let os = ctx.env.naming_os;
    let numbered = (2..=MAX_NUMBERED).map(|n| numbered_name(name, n, is_dir, os));
    for candidate in std::iter::once(name.to_owned()).chain(numbered) {
        let path = parent
            .join(&candidate)
            .map_err(|error| Failed::new(Stage::Create, error.into()))?;
        if let Some(issue) = name_issue(&candidate, &path, fs, os) {
            return Err(Failed::issue(Stage::Create, issue).writing(&path));
        }
        match provider.stat(&path) {
            Ok(_) => continue,
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(Failed::new(Stage::Create, error).writing(&path)),
        }
        let made = if is_dir {
            provider.create_dir(&path)
        } else {
            provider.create_file(&path)
        };
        match made {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(Failed::new(Stage::Create, error).writing(&path)),
        }
    }
    let wanted = parent
        .join(name)
        .map_err(|error| Failed::new(Stage::Create, error.into()))?;
    let op = if is_dir { "create_dir" } else { "create_file" };
    let taken = Error::io(op, wanted.clone(), io::ErrorKind::AlreadyExists.into());
    Err(Failed::new(Stage::Create, taken).writing(&wanted))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rf_core::{Entry, Result, Scheme};
    use rf_vfs::{
        CancelToken, Capabilities, DeleteMode, Deleted, DiskSpace, LocalProvider, Provider,
        ReadStream, Vfs, WatchHandle, WatchSink, WriteMode, WriteStream,
    };

    use super::*;
    use crate::exec::{ExecEnv, execute};
    use crate::id::JobId;
    use crate::report::Outcome;
    use crate::resolver::NonInteractive;
    use crate::spec::JobSpec;

    /// The local provider, but its `stat` finds nothing, as when another program makes an item
    /// right after a name was found free.
    #[derive(Debug)]
    struct Blind(LocalProvider);

    impl Provider for Blind {
        fn scheme(&self) -> Scheme {
            self.0.scheme()
        }
        fn capabilities(&self, path: &VPath) -> Capabilities {
            self.0.capabilities(path)
        }
        fn list(
            &self,
            dir: &VPath,
            cancel: &CancelToken,
            sink: &mut dyn FnMut(Vec<Entry>),
        ) -> Result<()> {
            self.0.list(dir, cancel, sink)
        }
        fn stat(&self, path: &VPath) -> Result<Entry> {
            let missing = io::Error::from(io::ErrorKind::NotFound);
            Err(Error::io("stat", path.clone(), missing))
        }
        fn create_dir(&self, path: &VPath) -> Result<()> {
            self.0.create_dir(path)
        }
        fn create_file(&self, path: &VPath) -> Result<()> {
            self.0.create_file(path)
        }
        fn rename(&self, from: &VPath, to: &VPath, mode: RenameMode) -> Result<()> {
            self.0.rename(from, to, mode)
        }
        fn delete(&self, path: &VPath, mode: DeleteMode) -> Result<Deleted> {
            self.0.delete(path, mode)
        }
        fn open_read(&self, path: &VPath) -> Result<Box<dyn ReadStream>> {
            self.0.open_read(path)
        }
        fn open_write(&self, path: &VPath, mode: WriteMode) -> Result<Box<dyn WriteStream>> {
            self.0.open_write(path, mode)
        }
        fn space(&self, path: &VPath) -> Result<Option<DiskSpace>> {
            self.0.space(path)
        }
        fn watch(&self, dir: &VPath, sink: WatchSink) -> Result<WatchHandle> {
            self.0.watch(dir, sink)
        }
    }

    #[test]
    fn a_name_taken_after_its_stat_moves_on_to_the_next_number() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.txt", "a (2).txt"] {
            std::fs::write(dir.path().join(name), name).unwrap();
        }
        let mut vfs = Vfs::new();
        vfs.register(Arc::new(Blind(LocalProvider::new())));
        let parent = VPath::local(dir.path()).unwrap();
        let job = JobSpec::Create {
            parent: parent.clone(),
            name: "a.txt".into(),
            kind: CreateKind::File,
        };
        let env = ExecEnv::new(&vfs, &NonInteractive).naming_os(Os::Windows);
        let report = execute(JobId(1), &job, &env);
        assert_eq!(report.outcome, Outcome::Completed, "{:?}", report.failures);
        assert_eq!(report.outputs, [parent.join("a (3).txt").unwrap()]);
        // The items that were there are untouched.
        for name in ["a.txt", "a (2).txt"] {
            assert_eq!(
                std::fs::read_to_string(dir.path().join(name)).unwrap(),
                name
            );
        }
    }
}
