//! Copying one file or link: write a temporary file (or link) next to the target, make it
//! durable, commit it under the final name with one rename, then copy the metadata. No partly
//! written file ever carries the final name.

use std::io::{self, Write};

use rf_core::{Entry, EntryKind, Error, VPath};
use rf_vfs::{DeleteMode, MetadataPatch, Provider, ReadStream, RenameMode, WriteMode, WriteStream};

use super::{At, COPY_CHUNK, Ctx, Failed};
use crate::preflight::check_path_len;
use crate::report::Stage;

/// One item a transfer reads: where it is and what it is.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Source<'a> {
    /// The provider of `path`.
    pub provider: &'a dyn Provider,
    /// The item itself, never a link that is followed: in Follow mode the path the link leads
    /// to, with every link on the way resolved. Its content and its extras
    /// (`Provider::copy_extras`) are what the copy gets.
    pub path: &'a VPath,
    /// What the item at `path` is: its kind, size, times and attributes, and its identity when
    /// the provider knows it.
    pub entry: &'a Entry,
}

/// Copies the file or link `from` to `target` of `dst`.
///
/// The content goes to a new temporary file in the target's folder, which is flushed and
/// synced, given the source's extras (when both sides are of one provider) and times, and
/// renamed to `target` with `commit`. On an error, a cancel or a panic the temporary file is
/// removed; a file that was at the temporary path before is not ours and is never touched.
/// The attributes (read-only among them) are put on `target` only after the commit, because
/// Windows cannot remove a read-only temporary file. A metadata failure is a warning; the
/// copy itself has worked.
///
/// A link is copied as a link: a new link at the temporary path stores what the source link
/// stores, and is committed like a file. It gets no metadata, because on Unix changing its
/// mode would change the item it leads to; the link is never opened, and the temporary link
/// that a failure leaves is removed itself, never what it leads to.
///
/// A source that is neither a regular file nor a link is refused before anything is opened
/// or created: a FIFO blocks a reader until a writer comes, and a device can be read without
/// end. A temporary path longer than the OS allows is refused the same way, as
/// `Stage::Preflight`: its name is longer than a short final name, so the final path alone may
/// fit where it does not.
pub(crate) fn copy_file(
    ctx: &mut Ctx<'_>,
    from: Source<'_>,
    dst: &dyn Provider,
    target: &VPath,
    commit: RenameMode,
) -> Result<(), Failed> {
    let Source {
        provider: src,
        path: source,
        entry,
    } = from;
    match entry.kind {
        EntryKind::File | EntryKind::Symlink => {}
        EntryKind::Dir | EntryKind::Other => {
            let error = Error::Unsupported(format!("not a regular file: {source}"));
            return Err(Failed::new(Stage::Plan, error));
        }
    }
    ctx.working_on(source);
    let tmp = ctx.temp_for(target).at(Stage::Write)?;
    // The temporary name is at least 29 units, longer than a short final name, so the final
    // path can fit where this one does not.
    if let Some(issue) = check_path_len(&tmp, ctx.env.naming_os) {
        return Err(Failed::issue(Stage::Preflight, issue));
    }
    if entry.kind == EntryKind::Symlink {
        let link = src.read_link(source).at(Stage::Read)?;
        dst.create_symlink(&tmp, &link).at(Stage::Write)?;
        // As for a file: only a link this call made is removed, and only the link.
        let mut guard = TempGuard {
            provider: dst,
            path: &tmp,
            armed: true,
        };
        dst.rename(&tmp, target, commit).at(Stage::Commit)?;
        guard.armed = false;
        return Ok(());
    }
    let mut reader = src.open_read(source).at(Stage::Read)?;
    let writer = dst
        .open_write(&tmp, WriteMode::CreateNew)
        .at(Stage::Write)?;
    // Only now is there a file of ours at `tmp`. If `CreateNew` failed, whatever is at that
    // path is not ours and stays.
    let mut guard = TempGuard {
        provider: dst,
        path: &tmp,
        armed: true,
    };
    // `write_content` owns the writer, so the handle is closed before the guard removes the
    // file (Windows does not remove an open file).
    write_content(ctx, &mut *reader, writer, source, &tmp, entry.size)?;
    drop(reader);

    if src.scheme() == dst.scheme() {
        if let Err(error) = dst.copy_extras(source, &tmp) {
            ctx.warn(
                Failed::new(Stage::Metadata, error),
                Some(source),
                Some(target),
            );
        }
    }
    if let Err(error) = dst.set_metadata(&tmp, &MetadataPatch::times_of(entry)) {
        ctx.warn(
            Failed::new(Stage::Metadata, error),
            Some(source),
            Some(target),
        );
    }

    dst.rename(&tmp, target, commit).at(Stage::Commit)?;
    guard.armed = false;

    if let Err(error) = dst.set_metadata(target, &MetadataPatch::attributes_of(entry)) {
        ctx.warn(
            Failed::new(Stage::Metadata, error),
            Some(source),
            Some(target),
        );
    }
    Ok(())
}

/// The smallest buffer a file is copied through.
const MIN_BUFFER: usize = 64 << 10;

/// Copies `reader` into `writer` one chunk at a time, stopping at a pause or a cancel between
/// chunks, then finishes (flushes and syncs) the writer. `size` is the planned size of the
/// source: it decides whether the first chunk gets its own progress event, and the buffer is
/// no larger than it needs (at least [`MIN_BUFFER`], at most [`COPY_CHUNK`]). A source that
/// grew after the plan only takes more chunks.
fn write_content(
    ctx: &mut Ctx<'_>,
    reader: &mut dyn ReadStream,
    mut writer: Box<dyn WriteStream>,
    source: &VPath,
    tmp: &VPath,
    size: u64,
) -> Result<(), Failed> {
    let large = size > bytes(COPY_CHUNK);
    let buffer =
        usize::try_from(size).map_or(COPY_CHUNK, |size| size.clamp(MIN_BUFFER, COPY_CHUNK));
    let mut buf = vec![0; buffer];
    let mut first = true;
    loop {
        ctx.check().at(Stage::Write)?;
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(Failed::new(
                    Stage::Read,
                    Error::io("read", source.clone(), e),
                ));
            }
        };
        writer
            .write_all(&buf[..n])
            .map_err(|e| Failed::new(Stage::Write, Error::io("write", tmp.clone(), e)))?;
        ctx.add_bytes(bytes(n), source, first && large);
        first = false;
    }
    writer.finish().at(Stage::Write)
}

/// A byte count as a `u64`; no `usize` this code sees is larger.
fn bytes(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// Removes the temporary file at `path` when it goes out of scope still armed: after an
/// error, a cancel, or while a panic unwinds. The removal is best effort; a file it cannot
/// remove keeps its temporary name, which crash recovery looks for.
struct TempGuard<'a> {
    provider: &'a dyn Provider,
    path: &'a VPath,
    armed: bool,
}

impl Drop for TempGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.provider.delete(self.path, DeleteMode::Permanent);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use rf_core::{ErrorKind, VPath};
    use rf_vfs::Vfs;

    #[cfg(unix)]
    use crate::testkit::Node;
    use crate::testkit::{Fault, FaultyProvider, temp_files, tree};
    use crate::{
        ExecEnv, JobId, JobReport, JobSpec, NonInteractive, Outcome, Stage, TransferMode,
        TransferOptions, TransferSpec, execute, temp_name,
    };

    struct Dirs {
        _root: tempfile::TempDir,
        src: PathBuf,
        dst: PathBuf,
    }

    fn dirs() -> Dirs {
        let root = tempfile::tempdir().unwrap();
        let (src, dst) = (root.path().join("src"), root.path().join("dst"));
        std::fs::create_dir(&src).unwrap();
        std::fs::create_dir(&dst).unwrap();
        Dirs {
            _root: root,
            src,
            dst,
        }
    }

    fn v(path: &Path) -> VPath {
        VPath::local(path).unwrap()
    }

    fn copy(id: JobId, source: &Path, dest_dir: &Path, vfs: &Vfs) -> JobReport {
        copy_all(id, &[source], dest_dir, vfs)
    }

    fn copy_all(id: JobId, sources: &[&Path], dest_dir: &Path, vfs: &Vfs) -> JobReport {
        let sources: Vec<VPath> = sources.iter().map(|source| v(source)).collect();
        let spec = TransferSpec::into_dir(
            TransferMode::Copy,
            &sources,
            &v(dest_dir),
            TransferOptions::default(),
        )
        .unwrap();
        let env = ExecEnv::new(vfs, &NonInteractive);
        execute(id, &JobSpec::Transfer(spec), &env)
    }

    fn first_failure(report: &JobReport) -> (Stage, ErrorKind) {
        (report.failures[0].stage, report.failures[0].kind)
    }

    #[test]
    fn a_failed_file_adds_no_bytes() {
        let d = dirs();
        let (a, b) = (d.src.join("a.bin"), d.src.join("b.bin"));
        std::fs::write(&a, vec![1; 3 << 20]).unwrap();
        std::fs::write(&b, vec![2; 5_000]).unwrap();
        // Two whole chunks of `a` are written and counted before its third write fails.
        let p = FaultyProvider::local(vec![Fault::FailWrite {
            file: 0,
            after: 2 << 20,
        }]);
        let report = copy_all(JobId::generate(), &[&a, &b], &d.dst, &p.vfs());
        assert_eq!(report.outcome, Outcome::CompletedWithErrors);
        assert_eq!(first_failure(&report), (Stage::Write, ErrorKind::Other));
        assert_eq!((report.done.files, report.done.bytes), (1, 5_000));
        assert_eq!(report.planned.bytes, (3 << 20) + 5_000);
        assert_eq!(tree(&d.dst).into_keys().collect::<Vec<_>>(), ["b.bin"]);
    }

    #[test]
    fn a_taken_temp_name_is_never_removed() {
        let d = dirs();
        std::fs::write(d.src.join("a.txt"), "new").unwrap();
        let id = JobId(7);
        let taken = d.dst.join(temp_name(id, 0));
        std::fs::write(&taken, "not ours").unwrap();
        let report = copy(id, &d.src.join("a.txt"), &d.dst, &Vfs::with_local());
        assert_eq!(
            first_failure(&report),
            (Stage::Write, ErrorKind::AlreadyExists)
        );
        assert_eq!(std::fs::read_to_string(&taken).unwrap(), "not ours");
        assert!(!d.dst.join("a.txt").exists());
    }

    /// The target is free when the item is planned and taken by another program before the
    /// commit: the commit must fail instead of replacing it, and the copy's temporary file
    /// must go.
    #[test]
    fn a_target_made_during_the_copy_fails_the_commit_and_keeps_its_content() {
        let d = dirs();
        std::fs::write(d.src.join("a.txt"), "new").unwrap();
        let p = FaultyProvider::local(vec![Fault::Hold { file: 0 }]);
        let vfs = p.vfs();
        let report = std::thread::scope(|scope| {
            let job = scope.spawn(|| copy(JobId::generate(), &d.src.join("a.txt"), &d.dst, &vfs));
            let held = p.wait_held(Duration::from_secs(10));
            std::fs::write(d.dst.join("a.txt"), "theirs").unwrap();
            p.release();
            assert!(held, "the copy never reached its first write");
            job.join().unwrap()
        });
        assert_eq!(report.outcome, Outcome::CompletedWithErrors);
        assert_eq!(
            first_failure(&report),
            (Stage::Commit, ErrorKind::AlreadyExists)
        );
        assert_eq!(
            std::fs::read_to_string(d.dst.join("a.txt")).unwrap(),
            "theirs"
        );
        assert_eq!(temp_files(&d.dst), Vec::<String>::new());
        assert_eq!((report.done.files, report.skipped), (0, 0));
    }

    #[test]
    fn a_panic_is_reported_and_its_temp_file_removed() {
        let d = dirs();
        std::fs::write(d.src.join("a.txt"), "a").unwrap();
        let p = FaultyProvider::local(vec![Fault::Panic { file: 0 }]);
        let report = copy(JobId::generate(), &d.src.join("a.txt"), &d.dst, &p.vfs());
        assert_eq!(report.outcome, Outcome::Aborted);
        assert_eq!(first_failure(&report), (Stage::Internal, ErrorKind::Other));
        assert_eq!(report.failures[0].message, "internal error: injected panic");
        assert_eq!(tree(&d.dst), BTreeMap::new());
    }

    /// A socket stands in for every item that is no file, folder or link: opening it for data
    /// would fail, so a refusal at `Stage::Plan` shows it was never opened.
    #[cfg(unix)]
    #[test]
    fn a_special_file_is_never_opened() {
        let d = dirs();
        let socket = d.src.join("sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let report = copy(JobId::generate(), &socket, &d.dst, &Vfs::with_local());
        assert_eq!(
            first_failure(&report),
            (Stage::Plan, ErrorKind::Unsupported)
        );
        assert_eq!(report.planned, crate::Totals::default());
        assert_eq!(tree(&d.dst), BTreeMap::new());
    }

    /// A link is counted as a file of no bytes and copied as a link: nothing is opened for
    /// writing, and what it leads to is not read.
    #[cfg(unix)]
    #[test]
    fn a_link_source_is_copied_as_a_link_without_opening_anything() {
        let d = dirs();
        std::fs::write(d.src.join("a.txt"), "a").unwrap();
        std::os::unix::fs::symlink("a.txt", d.src.join("link")).unwrap();
        let p = FaultyProvider::local(vec![]);
        let report = copy(JobId::generate(), &d.src.join("link"), &d.dst, &p.vfs());
        assert_eq!(report.outcome, Outcome::Completed, "{:?}", report.failures);
        let one_link = crate::Totals {
            files: 1,
            dirs: 0,
            bytes: 0,
        };
        assert_eq!((report.planned, report.done), (one_link, one_link));
        assert_eq!(p.opened_writes(), 0);
        assert_eq!(
            tree(&d.dst),
            BTreeMap::from([("link".to_string(), Node::Link("a.txt".into()))])
        );
        assert_eq!(report.outputs, [v(&d.dst.join("link"))]);
    }

    /// A link that cannot be committed is removed, and only the link: the file it leads to is
    /// still there.
    #[cfg(unix)]
    #[test]
    fn a_link_that_fails_its_commit_is_removed_itself() {
        let d = dirs();
        std::fs::write(d.dst.join("keep.txt"), "keep").unwrap();
        std::os::unix::fs::symlink(d.dst.join("keep.txt"), d.src.join("link")).unwrap();
        let p = FaultyProvider::local(vec![Fault::FailRename { call: 0 }]);
        let report = copy(JobId::generate(), &d.src.join("link"), &d.dst, &p.vfs());
        assert_eq!(first_failure(&report), (Stage::Commit, ErrorKind::Other));
        assert_eq!(
            tree(&d.dst),
            BTreeMap::from([("keep.txt".to_string(), Node::File(b"keep".to_vec()))])
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_target_folder_reached_through_a_link_is_accepted() {
        let d = dirs();
        std::fs::write(d.src.join("a.txt"), "a").unwrap();
        let link = d.dst.with_file_name("dst-link");
        std::os::unix::fs::symlink(&d.dst, &link).unwrap();
        let report = copy(
            JobId::generate(),
            &d.src.join("a.txt"),
            &link,
            &Vfs::with_local(),
        );
        assert_eq!(report.outcome, Outcome::Completed, "{:?}", report.failures);
        assert_eq!(std::fs::read_to_string(d.dst.join("a.txt")).unwrap(), "a");
    }
}
