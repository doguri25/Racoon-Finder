//! The job journal (REL-02): what the journaled jobs were asked to do and how far they got, so
//! that a job cut off by a crash or a power loss is found again on the next start and the
//! temporary files it left behind are removed.
//!
//! The journal is the file [`JOURNAL_FILE`] in the folder the engine is given (the app's data
//! folder). One thread writes it, in the order the records reach it, and only ever appends:
//! each record opens the file, writes its whole line with one `write_all` and closes the file
//! again, so no handle stays open between records. A durable record is also flushed to the
//! disk before its writer goes on. On the next start, recovery reads the file, finds the jobs
//! that were submitted and never finished, and replaces the file with only their records
//! (compaction).
//!
//! # Format
//!
//! Each line is `{"v":1,"rec":<record>}` and a `\n`: the format number and the serde JSON of
//! one record, with the job's [`JobSpec`] and everything in it, the undo records of undo jobs
//! ([`UndoRecord`](crate::UndoRecord)) included. A line that does not parse, as the torn last
//! line of a crash, and a line of another format number are skipped, so a crash costs at most
//! the record it cut off; the next record written after a torn line starts a line of its own.
//!
//! Every type in a record is part of the format, and the test `format_1_is_pinned` holds it
//! byte for byte. A new field gets `#[serde(default)]`, with a default that means what builds
//! without the field did, so that older lines still parse; a build that does not know the
//! field ignores it when it reads the line, and its compaction keeps the line as it was
//! written. A new enum variant needs no new format number. A build that does not know it skips
//! the line, except a `Finished` line, which still ends its job whatever its outcome says, so
//! that a new outcome does not bring a finished job back. Its compaction keeps only the lines
//! it can read, so it drops a line with an unknown variant even of a job it knows (a new kind
//! of record, or a `TempDir` with a new kind of path): a newer build that comes back to the
//! journal does not find that line again. A build that cannot read a job's `Submitted` line
//! does not know the job: its compaction drops every line of that job, the `TempDir` lines
//! included. That is safe, but the temporary files those lines point to are then never
//! removed. Any other change of shape (a field or a variant renamed or removed, a field's type
//! changed, a new field that a job must not run without) takes a new format number, so that
//! builds skip the lines of the other number instead of misreading them.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::SystemTime;

use rf_core::{EntryKind, Error, VPath};
use rf_vfs::{CancelToken, Vfs};
use serde::{Deserialize, Serialize};

use crate::engine::spawn_thread;
use crate::id::JobId;
use crate::naming::temp_job;
use crate::report::Outcome;
use crate::spec::JobSpec;

/// The name of the journal file in the folder it is kept in.
pub const JOURNAL_FILE: &str = "jobs.journal";

/// The name compaction writes the new journal under, next to the old one, before the rename.
pub(crate) const COMPACTED_FILE: &str = "jobs.journal.tmp";

/// The format number every line carries as `v`; see the module doc for when it changes.
const FORMAT: u32 = 1;

/// A job the journal says was submitted and never finished: the app ended, or crashed, while
/// the job was queued or running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncompleteJob {
    /// The job's ID, as it was submitted. Its temporary files carry it in their names.
    pub id: JobId,
    /// What the job was asked to do.
    pub spec: JobSpec,
    /// When the job was submitted.
    pub submitted: SystemTime,
    /// Whether a worker had taken the job up. One that had not has changed nothing.
    pub started: bool,
    /// The folders the job made temporary files in, in the order it first did.
    pub temp_dirs: Vec<VPath>,
}

/// One record of the journal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Record {
    /// The job was queued.
    Submitted {
        job: JobId,
        spec: JobSpec,
        at: SystemTime,
    },
    /// A worker took the job up.
    Started { job: JobId },
    /// The job is about to make its first temporary file in the folder `dir`. It is written
    /// durably before that file is made.
    TempDir { job: JobId, dir: VPath },
    /// The job ended with `outcome`, or with `None` when the user dismissed it from the list of
    /// recovered jobs.
    Finished {
        job: JobId,
        outcome: Option<Outcome>,
    },
}

impl Record {
    /// The job the record is about.
    fn job(&self) -> JobId {
        match self {
            Record::Submitted { job, .. }
            | Record::Started { job }
            | Record::TempDir { job, .. }
            | Record::Finished { job, .. } => *job,
        }
    }
}

/// One line of the journal: the format number and the record.
#[derive(Serialize, Deserialize)]
struct Line {
    v: u32,
    rec: Record,
}

/// The line of `record`, `\n` included.
fn encode(record: Record) -> serde_json::Result<Vec<u8>> {
    let mut line = serde_json::to_vec(&Line {
        v: FORMAT,
        rec: record,
    })?;
    line.push(b'\n');
    Ok(line)
}

/// The record of one line (without its `\n`), or `None` for a line that does not parse or is
/// of another format.
fn decode(line: &[u8]) -> Option<Record> {
    let line: Line = serde_json::from_slice(line).ok()?;
    (line.v == FORMAT).then_some(line.rec)
}

/// The job of a `Finished` line of this format that [`decode`] cannot read, such as one with an
/// outcome a newer build added: whatever the outcome, the job ended. `None` for any other line.
fn finished_job(line: &[u8]) -> Option<JobId> {
    /// A line read for its `Finished` record's job only; other fields are ignored.
    #[derive(Deserialize)]
    struct FinishedLine {
        v: u32,
        rec: FinishedRecord,
    }
    #[derive(Deserialize)]
    enum FinishedRecord {
        Finished { job: JobId },
    }
    let line: FinishedLine = serde_json::from_slice(line).ok()?;
    let FinishedRecord::Finished { job } = line.rec;
    (line.v == FORMAT).then_some(job)
}

/// The error of a journal step that failed with `error` on `path`.
fn journal_error(path: &Path, error: io::Error) -> Error {
    match VPath::local(path) {
        Ok(path) => Error::io("journal", path, error),
        Err(error) => error.into(),
    }
}

/// What [`recover`] found in the journal.
#[derive(Debug)]
pub(crate) struct Recovered {
    /// The jobs that were submitted and never finished, in the order they were submitted.
    pub jobs: Vec<IncompleteJob>,
    /// Why the journal could not be compacted, if it could not, or why a compacted file put in
    /// its place could not be made durable. The jobs are found all the same, and the journal
    /// on the disk still holds every record of them: the old journal stays until the compacted
    /// one is renamed over it, and when only the flush after that rename fails, the old and the
    /// new journal both hold them, whichever a crash leaves.
    pub compaction: Option<Error>,
}

/// The jobs the journal in the folder `dir` holds that were submitted and never finished, in
/// the order they were submitted. The journal then keeps only their records.
///
/// No journal file is no job, and nothing is made then. A line that does not parse (the torn
/// last line of a crash) or is of another format is skipped; a `Finished` line this build
/// cannot read in full still ends its job. The lines of the jobs found are written, as they
/// were, to `jobs.journal.tmp`, flushed to the disk and renamed over the journal, and the
/// rename is made durable too, so that a crash leaves the old journal or the new one, never a
/// mix; with no job left the journal is empty. A journal that would not change is left as it
/// is.
///
/// Only a journal that cannot be read fails it. A compaction that fails (a full disk, a file
/// another program holds, something in the way of `jobs.journal.tmp`) gives the jobs all the
/// same, with the error in [`Recovered::compaction`], so that their temporary files can still
/// be removed.
///
/// A record appended while this runs can be lost with the old file, so it runs before the
/// journal thread writes anything: the engine runs it as that thread's `on_start`.
pub(crate) fn recover(dir: &Path) -> rf_core::Result<Recovered> {
    let path = dir.join(JOURNAL_FILE);
    let Some((content, promoted)) = read_journal(dir, &path)? else {
        return Ok(Recovered {
            jobs: Vec::new(),
            compaction: None,
        });
    };
    let mut records: Vec<(&[u8], Record)> = Vec::new();
    let mut finished = HashSet::new();
    // Split at bytes, not as text: a line torn inside a character is not UTF-8.
    for line in content.split(|&byte| byte == b'\n') {
        match decode(line) {
            Some(record) => {
                if let Record::Finished { job, .. } = record {
                    finished.insert(job);
                }
                records.push((line, record));
            }
            None => finished.extend(finished_job(line)),
        }
    }
    let mut jobs = Vec::new();
    let mut index = HashMap::new();
    for (_, record) in &records {
        let Record::Submitted { job, spec, at } = record else {
            continue;
        };
        if finished.contains(job) || index.contains_key(job) {
            continue;
        }
        index.insert(*job, jobs.len());
        jobs.push(IncompleteJob {
            id: *job,
            spec: spec.clone(),
            submitted: *at,
            started: false,
            temp_dirs: Vec::new(),
        });
    }
    let mut kept = Vec::new();
    for (line, record) in &records {
        let Some(&at) = index.get(&record.job()) else {
            continue;
        };
        let job: &mut IncompleteJob = &mut jobs[at];
        match record {
            Record::Started { .. } => job.started = true,
            Record::TempDir { dir, .. } if !job.temp_dirs.contains(dir) => {
                job.temp_dirs.push(dir.clone());
            }
            _ => {}
        }
        kept.extend_from_slice(line);
        kept.push(b'\n');
    }
    let compaction = if kept != content {
        // Its flush after the rename makes a promoted journal's rename durable too.
        compact(dir, &path, &kept).err()
    } else if promoted {
        // Durable before the journal thread appends to it under its new name.
        sync_entry(dir, &path)
            .map_err(|error| journal_error(&path, error))
            .err()
    } else {
        None
    };
    Ok(Recovered { jobs, compaction })
}

/// The content of the journal `path` in the folder `dir`, or `None` when there is none, and
/// whether it was a compacted file this call renamed into place (its rename is not durable
/// yet).
///
/// A file system whose rename over a file deletes the old file before it renames the new one
/// (FAT, exFAT) can lose the journal to a crash in the middle of a compaction and leave only
/// the compacted file. That file is complete, since it is renamed only after it is flushed, so
/// a compacted file found without a journal is renamed into place first and read. A folder or
/// a link of that name is not.
fn read_journal(dir: &Path, path: &Path) -> rf_core::Result<Option<(Vec<u8>, bool)>> {
    match fs::read(path) {
        Ok(content) => return Ok(Some((content, false))),
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(journal_error(path, error));
        }
        Err(_) => {}
    }
    let compacted = dir.join(COMPACTED_FILE);
    if !fs::symlink_metadata(&compacted).is_ok_and(|meta| meta.is_file()) {
        return Ok(None);
    }
    // A crash before this rename is on the disk leaves the same state, which the next recovery
    // puts right in the same way; `recover` makes it durable before anything is appended.
    fs::rename(&compacted, path).map_err(|error| journal_error(path, error))?;
    fs::read(path)
        .map(|content| Some((content, true)))
        .map_err(|error| journal_error(path, error))
}

/// Replaces the journal `path` in the folder `dir` with `content`, as [`recover`] describes.
fn compact(dir: &Path, path: &Path, content: &[u8]) -> rf_core::Result<()> {
    let tmp = dir.join(COMPACTED_FILE);
    let written = File::create(&tmp).and_then(|mut file| {
        file.write_all(content)?;
        rf_platform::sync_file(&file)
    });
    if let Err(error) = written {
        // `remove_file` never removes a folder that might be in the way.
        let _ = fs::remove_file(&tmp);
        return Err(journal_error(&tmp, error));
    }
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(journal_error(path, error));
    }
    sync_entry(dir, path).map_err(|error| journal_error(path, error))
}

/// Makes the entry of the file `file` in its folder `dir` durable, after the file was renamed
/// into the folder or made there.
///
/// On Unix that is a flush of the folder (`rf_platform::sync_dir`, which takes a file system
/// that cannot flush a folder as being as durable as it gets).
#[cfg(unix)]
fn sync_entry(dir: &Path, file: &Path) -> io::Result<()> {
    let _ = file;
    rf_platform::sync_dir(dir)
}

/// Makes the entry of the file `file` in its folder `dir` durable, after the file was renamed
/// into the folder or made there.
///
/// On Windows NTFS writes its log of the change lazily, so a power loss soon after could undo
/// it. Flushing the file itself (`FlushFileBuffers`, which needs write access) commits that
/// log, the change included.
#[cfg(windows)]
fn sync_entry(dir: &Path, file: &Path) -> io::Result<()> {
    let _ = dir;
    rf_platform::sync_file(&OpenOptions::new().write(true).open(file)?)
}

/// Removes what `job` left in its temporary folders: each file or link there whose name is a
/// temporary name of this job ([`temp_job`]), with `Provider::delete_leaf`, and nothing else.
/// A folder stays whatever its name, and so does every item of another job or of the user.
/// Returns how many items it removed. A folder that cannot be listed and an item that cannot
/// be removed are skipped.
pub(crate) fn remove_orphans(vfs: &Vfs, job: &IncompleteJob) -> usize {
    let mut removed = 0;
    for dir in &job.temp_dirs {
        let Ok(provider) = vfs.provider(dir) else {
            continue;
        };
        let mut entries = Vec::new();
        let listed = provider.list(dir, &CancelToken::new(), &mut |batch| {
            entries.extend(batch);
        });
        if listed.is_err() {
            continue;
        }
        for entry in entries {
            if entry.kind == EntryKind::Dir || temp_job(&entry.name) != Some(job.id) {
                continue;
            }
            let gone = dir
                .join(&entry.name)
                .is_ok_and(|path| provider.delete_leaf(&path).is_ok());
            removed += usize::from(gone);
        }
    }
    removed
}

/// A way to the journal thread. Clones share the thread.
#[derive(Clone, Debug)]
pub(crate) struct JournalHandle {
    /// `None` for a handle that writes nothing.
    queue: Option<Queue>,
}

/// The queue of a journal thread, the thread, and the file it writes.
#[derive(Clone, Debug)]
struct Queue {
    sender: Sender<Message>,
    thread: ThreadId,
    file: PathBuf,
}

/// What the journal thread is asked to do.
enum Message {
    /// Writes `record`. With `done`, flushes it to the disk and sends there how that went.
    Write {
        record: Record,
        done: Option<Sender<rf_core::Result<()>>>,
    },
    /// Ends the thread once everything queued before is written.
    Stop,
}

impl JournalHandle {
    /// A handle whose records all go nowhere, for a job or an engine without a journal.
    pub(crate) fn disabled() -> JournalHandle {
        JournalHandle { queue: None }
    }

    /// Starts the `rf-jobs-journal` thread for the journal in the folder `dir`. The thread runs
    /// `on_start` first (the engine's recovery) and then writes the records it is sent, in the
    /// order they were sent, until [`shutdown`](Self::shutdown). The folder is made with the
    /// first record that finds it missing. Every record that cannot be written is passed to
    /// `on_error`, on the thread; a failure is `Error::io("journal", file, …)`.
    ///
    /// Records sent while `on_start` runs wait for it, and so do the writers of durable ones.
    /// `on_start` and `on_error` may [`append`](Self::append), but a durable record from them
    /// would have the thread wait for itself, so [`append_durable`](Self::append_durable)
    /// refuses it there.
    ///
    /// Fails with `Error::io("journal", file, …)` when the system cannot start the thread;
    /// `on_start` and `on_error` are then dropped without being called.
    ///
    /// A relative `dir` is made absolute first ([`absolute_dir`]), so that the thread never
    /// depends on the working folder.
    pub(crate) fn spawn(
        dir: PathBuf,
        on_start: Box<dyn FnOnce() + Send>,
        on_error: Box<dyn Fn(&Error) + Send>,
    ) -> rf_core::Result<(JournalHandle, JoinHandle<()>)> {
        let dir = absolute_dir(dir);
        let file = dir.join(JOURNAL_FILE);
        let mut writer = Writer::new(dir);
        let (sender, messages) = mpsc::channel();
        let thread = spawn_thread("rf-jobs-journal".to_owned(), move || {
            on_start();
            for message in messages {
                let Message::Write { record, done } = message else {
                    break;
                };
                let written = writer.write(record, done.is_some());
                if let Err(error) = &written {
                    on_error(error);
                }
                if let Some(done) = done {
                    // A writer that stopped waiting needs no answer.
                    let _ = done.send(written);
                }
            }
        });
        let thread = thread.map_err(|error| {
            let text = format!("the journal thread could not be started: {error}");
            journal_error(&file, io::Error::new(error.kind(), text))
        })?;
        let queue = Queue {
            sender,
            thread: thread.thread().id(),
            file,
        };
        Ok((JournalHandle { queue: Some(queue) }, thread))
    }

    /// Queues `record` and returns at once. A failure to write it goes to the thread's
    /// `on_error`. After [`shutdown`](Self::shutdown) the record is dropped.
    pub(crate) fn append(&self, record: Record) {
        if let Some(queue) = &self.queue {
            // A thread that has ended takes nothing more.
            let _ = queue.sender.send(Message::Write { record, done: None });
        }
    }

    /// Writes `record` and waits until it is flushed to the disk. A failure is returned, and
    /// passed to the thread's `on_error` as well; after [`shutdown`](Self::shutdown) the record
    /// fails without being written. A disabled handle writes nothing and returns `Ok`.
    ///
    /// On the journal thread itself (in `on_start` or `on_error`) the record would wait for
    /// the thread that waits for it: it is not written, and the error's I/O kind is
    /// `io::ErrorKind::Deadlock`. `on_error` is not called for it.
    pub(crate) fn append_durable(&self, record: Record) -> rf_core::Result<()> {
        let Some(queue) = &self.queue else {
            return Ok(());
        };
        if thread::current().id() == queue.thread {
            let deadlock = io::Error::new(
                io::ErrorKind::Deadlock,
                "the journal thread cannot wait for its own durable record",
            );
            return Err(journal_error(&queue.file, deadlock));
        }
        let (done, answer) = mpsc::channel();
        let message = Message::Write {
            record,
            done: Some(done),
        };
        let written = queue
            .sender
            .send(message)
            .ok()
            .and_then(|()| answer.recv().ok());
        written.unwrap_or_else(|| {
            let ended = io::Error::other("the journal thread has ended");
            Err(journal_error(&queue.file, ended))
        })
    }

    /// Lets the journal thread write everything queued so far and end. It does not wait for
    /// that; join the thread to.
    pub(crate) fn shutdown(&self) {
        if let Some(queue) = &self.queue {
            let _ = queue.sender.send(Message::Stop);
        }
    }
}

/// `dir` as an absolute path (`std::path::absolute`), or as it is when it cannot be made one
/// (the working folder is gone). A journal folder given relative to the working folder is
/// taken in the working folder of the time the journal starts: every folder its writer makes
/// and syncs has a real path then, and every error names the file by its full path, where a
/// relative one cannot be named at all (`VPath::local` refuses it).
pub(crate) fn absolute_dir(dir: PathBuf) -> PathBuf {
    std::path::absolute(&dir).unwrap_or(dir)
}

/// The folders whose entries the making of the folder `dir` changes: the folder each missing
/// folder of `dir` (it included) is made in, the lowest first. A relative `dir` (one
/// [`absolute_dir`] could not make absolute) has the empty path as the folder of its first
/// folder, and that path names no folder to sync: it is left out.
fn unsynced_parents(dir: &Path) -> Vec<PathBuf> {
    dir.ancestors()
        .take_while(|folder| !folder.exists())
        .filter_map(Path::parent)
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .collect()
}

/// How the journal file ends, as the journal thread finds it before a record.
enum Tail {
    /// There is no file.
    Missing,
    /// The file is empty or its last line is whole.
    Whole,
    /// The last line was cut off, as a crash or a failed write leaves it.
    Torn,
}

/// The journal file as its thread writes it.
struct Writer {
    dir: PathBuf,
    file: PathBuf,
    /// Whether the next record looks at the end of the file first: at the start, and after a
    /// failed write, which may have left a torn line.
    look: bool,
    /// Whether this writer made the file and its entry in the folder is not flushed yet.
    entry_unsynced: bool,
    /// The folders this writer made the journal's folder in, or a missing folder above it:
    /// the entries of the new folders in them are not flushed yet.
    dirs_unsynced: Vec<PathBuf>,
}

impl Writer {
    fn new(dir: PathBuf) -> Writer {
        Writer {
            file: dir.join(JOURNAL_FILE),
            dir,
            look: true,
            entry_unsynced: false,
            dirs_unsynced: Vec::new(),
        }
    }

    /// Makes the journal's folder and the missing folders above it, and notes the folders they
    /// were made in, and the journal file to be made in the new folder, so that the next
    /// durable record makes their entries durable too.
    fn create_dirs(&mut self) -> io::Result<()> {
        let parents = unsynced_parents(&self.dir);
        fs::create_dir_all(&self.dir)?;
        self.dirs_unsynced.extend(parents);
        self.entry_unsynced = true;
        Ok(())
    }

    /// Makes the entries of the folders this writer made durable, after the journal file's
    /// own: each folder they were made in is flushed.
    #[cfg(unix)]
    fn sync_dirs(&mut self) -> io::Result<()> {
        while let Some(parent) = self.dirs_unsynced.last() {
            rf_platform::sync_dir(parent)?;
            self.dirs_unsynced.pop();
        }
        Ok(())
    }

    /// Makes the entries of the folders this writer made durable, after the journal file's
    /// own: the flush of the journal file, made in the newest of them, has committed the log
    /// of NTFS up to it, their entries included.
    #[cfg(windows)]
    fn sync_dirs(&mut self) {
        self.dirs_unsynced.clear();
    }

    /// Appends the line of `record`, and with `durable` flushes it to the disk.
    fn write(&mut self, record: Record, durable: bool) -> rf_core::Result<()> {
        let line = encode(record).map_err(|error| journal_error(&self.file, error.into()))?;
        let written = self.append(&line, durable);
        self.look = written.is_err();
        written.map_err(|error| journal_error(&self.file, error))
    }

    fn append(&mut self, line: &[u8], durable: bool) -> io::Result<()> {
        let mut torn = false;
        if self.look {
            match tail(&self.file)? {
                Tail::Missing => self.entry_unsynced = true,
                Tail::Whole => {}
                Tail::Torn => torn = true,
            }
        }
        let open = |file: &Path| OpenOptions::new().create(true).append(true).open(file);
        let mut file = match open(&self.file) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.create_dirs()?;
                open(&self.file)?
            }
            opened => opened?,
        };
        if torn {
            // The record starts a line of its own, so that the torn one costs only itself.
            let mut whole = Vec::with_capacity(line.len() + 1);
            whole.push(b'\n');
            whole.extend_from_slice(line);
            file.write_all(&whole)?;
        } else {
            file.write_all(line)?;
        }
        if durable {
            // The whole file, not only its data: on macOS only `F_FULLFSYNC` gets past the
            // drive's own cache.
            rf_platform::sync_file(&file)?;
            drop(file);
            if self.entry_unsynced {
                sync_entry(&self.dir, &self.file)?;
                self.entry_unsynced = false;
            }
            #[cfg(unix)]
            self.sync_dirs()?;
            #[cfg(windows)]
            self.sync_dirs();
        }
        Ok(())
    }
}

/// How the file `path` ends.
fn tail(path: &Path) -> io::Result<Tail> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Tail::Missing),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() == 0 {
        return Ok(Tail::Whole);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0];
    file.read_exact(&mut last)?;
    Ok(if last[0] == b'\n' {
        Tail::Whole
    } else {
        Tail::Torn
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::temp_name;
    use crate::{DeleteMethod, DeleteSpec, ErrorPolicy};
    use std::ffi::OsString;
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use rf_core::{Error, Scheme};

    use crate::exec::{ExecEnv, execute};
    use crate::resolver::NonInteractive;
    use crate::spec::{
        ConflictPolicy, CreateKind, SymlinkMode, TransferItem, TransferMode, TransferOptions,
        TransferSpec,
    };
    use crate::testkit::{Fault, FaultyProvider, temp_files};
    use crate::undo::{CreatedItem, MovedItem, TrashedItem, UndoRecord};

    const A: JobId = JobId(1);
    const B: JobId = JobId(2);
    const C: JobId = JobId(3);
    const WAIT: Duration = Duration::from_secs(10);

    fn spec_for(p: &Path) -> JobSpec {
        JobSpec::Delete(DeleteSpec {
            paths: vec![VPath::local(p).unwrap()],
            method: DeleteMethod::Trash,
            errors: ErrorPolicy::Continue,
        })
    }

    fn write(dir: &Path, records: Vec<Record>) {
        let (h, t) = JournalHandle::spawn(
            dir.to_path_buf(),
            Box::new(|| {}),
            Box::new(|e| panic!("{e}")),
        )
        .unwrap();
        for r in records {
            h.append_durable(r).unwrap();
        }
        h.shutdown();
        t.join().unwrap();
    }

    fn lines(dir: &Path) -> usize {
        fs::read_to_string(dir.join(JOURNAL_FILE))
            .unwrap()
            .lines()
            .count()
    }

    fn append_raw(dir: &Path, text: impl AsRef<[u8]>) {
        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.join(JOURNAL_FILE))
            .unwrap();
        file.write_all(text.as_ref()).unwrap();
    }

    /// A finished; B started and made a temp file in the journal folder itself.
    fn a_done_b_running() -> (tempfile::TempDir, VPath, SystemTime) {
        let t = tempfile::tempdir().unwrap();
        let d = VPath::local(t.path()).unwrap();
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        write(
            t.path(),
            vec![
                Record::Submitted {
                    job: A,
                    spec: spec_for(t.path()),
                    at,
                },
                Record::Started { job: A },
                Record::Finished {
                    job: A,
                    outcome: Some(Outcome::Completed),
                },
                Record::Submitted {
                    job: B,
                    spec: spec_for(t.path()),
                    at,
                },
                Record::Started { job: B },
                Record::TempDir {
                    job: B,
                    dir: d.clone(),
                },
            ],
        );
        (t, d, at)
    }

    /// The jobs `recover` finds in `dir`, after a compaction that worked.
    fn jobs(dir: &Path) -> Vec<IncompleteJob> {
        let recovered = recover(dir).unwrap();
        assert!(recovered.compaction.is_none(), "{:?}", recovered.compaction);
        recovered.jobs
    }

    fn ids(dir: &Path) -> Vec<JobId> {
        jobs(dir).iter().map(|j| j.id).collect()
    }

    #[test]
    fn finished_jobs_are_not_recovered() {
        let (t, d, at) = a_done_b_running();
        let want = IncompleteJob {
            id: B,
            spec: spec_for(t.path()),
            submitted: at,
            started: true,
            temp_dirs: vec![d],
        };
        assert_eq!(jobs(t.path()), [want]);
    }

    #[test]
    fn recovery_compacts_the_file() {
        let (t, _, _) = a_done_b_running();
        let first = jobs(t.path());
        assert_eq!(lines(t.path()), 3);
        assert_eq!(jobs(t.path()), first);
    }

    #[test]
    fn a_torn_last_line_is_ignored() {
        let (t, _, _) = a_done_b_running();
        append_raw(t.path(), r#"{"v":1,"rec":{"Star"#);
        assert_eq!(ids(t.path()), [B]);
    }

    #[test]
    fn a_torn_finished_line_does_not_end_its_job() {
        let (t, _, _) = a_done_b_running();
        // A crash in the middle of B's `Finished`: the job did not end as far as the disk knows.
        append_raw(t.path(), r#"{"v":1,"rec":{"Finished":{"job":2,"outc"#);
        assert_eq!(ids(t.path()), [B]);
        let (t, _, _) = a_done_b_running();
        append_raw(t.path(), r#"{"v":1,"rec":{"Finished":{"job":2"#);
        assert_eq!(ids(t.path()), [B]);
    }

    #[test]
    fn a_durable_record_in_folders_the_journal_made_is_written() {
        let t = tempfile::tempdir().unwrap();
        let state = t.path().join("a").join("b").join("state");
        let (h, thread) =
            JournalHandle::spawn(state.clone(), Box::new(|| {}), Box::new(|e| panic!("{e}")))
                .unwrap();
        h.append_durable(Record::Started { job: A }).unwrap();
        h.append_durable(Record::Started { job: B }).unwrap();
        h.shutdown();
        thread.join().unwrap();
        let want = [Record::Started { job: A }, Record::Started { job: B }];
        assert_eq!(records(&state), want);
    }

    /// Set for [`a_relative_journal_folder_in_the_working_folder`], which this test process
    /// runs in a working folder of its own.
    const IN_OWN_WORKING_FOLDER: &str = "RF_JOBS_TEST_IN_OWN_WORKING_FOLDER";

    /// A journal folder given relative to the working folder, `a/b/state` with no `a`, is made
    /// there, and its durable records are written without an error: no folder of an empty
    /// path is ever synced. The test runs [`a_relative_journal_folder_in_the_working_folder`]
    /// in a new process with a temporary working folder, because the working folder is the
    /// whole process's and the other tests run in parallel.
    #[test]
    fn a_relative_journal_folder_is_made_in_the_working_folder() {
        let t = tempfile::tempdir().unwrap();
        let test = "journal::tests::a_relative_journal_folder_in_the_working_folder";
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([test, "--exact", "--include-ignored", "--test-threads=1"])
            .env(IN_OWN_WORKING_FOLDER, "1")
            .current_dir(t.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stdout}\n{stderr}");
        assert!(stdout.contains("1 passed"), "{stdout}");
        let state = t.path().join("a").join("b").join("state");
        let want = [Record::Started { job: A }, Record::Started { job: B }];
        assert_eq!(records(&state), want);
    }

    /// The folders to sync after making a journal folder are those the missing folders are
    /// made in, and never the empty path, even for a relative folder whose first folder is
    /// missing too.
    #[test]
    fn the_folders_to_sync_are_real_ones() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("a").join("b"));
        assert_eq!(
            unsynced_parents(&b.join("state")),
            [b.clone(), a, t.path().to_path_buf()]
        );
        let first = PathBuf::from(format!("rf-jobs-no-such-folder-{}", std::process::id()));
        let relative = first.join("b");
        assert_eq!(unsynced_parents(&relative.join("state")), [relative, first]);
        assert_eq!(unsynced_parents(Path::new("")), Vec::<PathBuf>::new());
        assert_eq!(
            unsynced_parents(&b),
            [t.path().join("a"), t.path().to_path_buf()]
        );
        // A relative journal folder is made absolute, in the working folder.
        let state = std::env::current_dir().unwrap().join("state");
        assert_eq!(absolute_dir(PathBuf::from("state")), state);
        assert_eq!(absolute_dir(state.clone()), state);
    }

    /// Run by [`a_relative_journal_folder_is_made_in_the_working_folder`] only, in a working
    /// folder of its own; anywhere else it does nothing.
    #[test]
    #[ignore = "run by a_relative_journal_folder_is_made_in_the_working_folder"]
    fn a_relative_journal_folder_in_the_working_folder() {
        if std::env::var_os(IN_OWN_WORKING_FOLDER).is_none() {
            return;
        }
        let state = PathBuf::from("a").join("b").join("state");
        let (h, thread) =
            JournalHandle::spawn(state, Box::new(|| {}), Box::new(|e| panic!("{e}"))).unwrap();
        h.append_durable(Record::Started { job: A }).unwrap();
        h.append_durable(Record::Started { job: B }).unwrap();
        h.shutdown();
        thread.join().unwrap();
    }

    #[test]
    fn unknown_versions_are_skipped() {
        let (t, _, _) = a_done_b_running();
        append_raw(t.path(), "{\"v\":2,\"rec\":{}}\n");
        assert_eq!(ids(t.path()), [B]);
    }

    #[test]
    fn dismissed_jobs_are_gone() {
        let (t, _, _) = a_done_b_running();
        write(
            t.path(),
            vec![Record::Finished {
                job: B,
                outcome: None,
            }],
        );
        assert_eq!(ids(t.path()), Vec::<JobId>::new());
    }

    #[test]
    fn nothing_is_created_before_the_first_record() {
        let t = tempfile::tempdir().unwrap();
        let state = t.path().join("state");
        assert_eq!(jobs(&state), Vec::<IncompleteJob>::new());
        assert!(!state.exists());
        let (h, j) =
            JournalHandle::spawn(state.clone(), Box::new(|| {}), Box::new(|e| panic!("{e}")))
                .unwrap();
        h.append(Record::Started { job: A });
        h.shutdown();
        j.join().unwrap();
        assert!(state.join(JOURNAL_FILE).is_file());
    }

    #[test]
    fn orphan_cleanup_removes_only_this_jobs_temps() {
        let t = tempfile::tempdir().unwrap();
        let names: [OsString; 4] = [
            temp_name(B, 0),
            temp_name(B, 1),
            temp_name(A, 0),
            "keep.txt".into(),
        ];
        for name in &names {
            fs::write(t.path().join(name), b"x").unwrap();
        }
        let dir = VPath::local(t.path()).unwrap();
        let job = IncompleteJob {
            id: B,
            spec: spec_for(t.path()),
            submitted: SystemTime::now(),
            started: true,
            temp_dirs: vec![dir],
        };
        assert_eq!(remove_orphans(&Vfs::with_local(), &job), 2);
        let mut left: Vec<OsString> = fs::read_dir(t.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        left.sort();
        let mut want = vec![names[2].clone(), names[3].clone()];
        want.sort();
        assert_eq!(left, want);
    }

    // Beyond the plan: the shape on disk, robustness of reading and writing, and the
    // executor's records.

    fn v(path: &Path) -> VPath {
        VPath::local(path).unwrap()
    }

    /// The records in the journal in `dir`, every line of format 1.
    fn records(dir: &Path) -> Vec<Record> {
        fs::read(dir.join(JOURNAL_FILE))
            .unwrap()
            .split(|&byte| byte == b'\n')
            .filter_map(decode)
            .collect()
    }

    fn incomplete(id: JobId, dir: &Path) -> IncompleteJob {
        IncompleteJob {
            id,
            spec: spec_for(dir),
            submitted: SystemTime::now(),
            started: true,
            temp_dirs: vec![v(dir)],
        }
    }

    /// A journal thread whose write failures are counted.
    fn counting(dir: PathBuf) -> (JournalHandle, JoinHandle<()>, Arc<AtomicUsize>) {
        let errors = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&errors);
        let (h, t) = JournalHandle::spawn(
            dir,
            Box::new(|| {}),
            Box::new(move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .unwrap();
        (h, t, errors)
    }

    fn is_journal_error(error: &Error) -> bool {
        matches!(error, Error::Io { op: "journal", .. })
    }

    /// A source folder `src` with the files `names`, an empty target folder `dst`, and a
    /// copy of the files into `dst`.
    fn copy_fixture(names: &[&str], size: usize) -> (tempfile::TempDir, PathBuf, JobSpec) {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        fs::create_dir(&src).unwrap();
        fs::create_dir(&dst).unwrap();
        let mut sources = Vec::new();
        for name in names {
            fs::write(src.join(name), vec![7; size]).unwrap();
            sources.push(v(&src.join(name)));
        }
        let spec = TransferSpec::into_dir(
            TransferMode::Copy,
            &sources,
            &v(&dst),
            TransferOptions::default(),
        )
        .unwrap();
        (t, dst, JobSpec::Transfer(spec))
    }

    fn remote(segments: &[&str]) -> VPath {
        VPath::new(Scheme::Remote, "p", segments).unwrap()
    }

    fn golden_time() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::new(1_000_000_000, 500)
    }

    #[cfg(unix)]
    const CREATE_LINE: &str = r#"{"v":1,"rec":{"Submitted":{"job":4,"spec":{"Create":{"parent":"remote://p/d","name":{"Unix":[110,101,119]},"kind":"Dir"}},"at":{"secs_since_epoch":1000000000,"nanos_since_epoch":500}}}}"#;
    #[cfg(windows)]
    const CREATE_LINE: &str = r#"{"v":1,"rec":{"Submitted":{"job":4,"spec":{"Create":{"parent":"remote://p/d","name":{"Windows":[110,101,119]},"kind":"Dir"}},"at":{"secs_since_epoch":1000000000,"nanos_since_epoch":500}}}}"#;

    /// One record of every kind, one submitted spec of every kind, and the line format 1
    /// writes for each. A file name is stored as the OS gives it (bytes on Unix, UTF-16 units
    /// on Windows), so the line of the create differs by OS.
    fn golden() -> Vec<(Record, &'static str)> {
        let at = golden_time();
        let transfer = JobSpec::Transfer(TransferSpec {
            mode: TransferMode::Move,
            items: vec![TransferItem {
                source: remote(&["a.txt"]),
                target: remote(&["d", "a.txt"]),
            }],
            options: TransferOptions {
                conflict: ConflictPolicy::AutoRename,
                errors: ErrorPolicy::Ask,
                symlinks: SymlinkMode::Follow,
            },
        });
        let delete = JobSpec::Delete(DeleteSpec {
            paths: vec![remote(&["a.txt"])],
            method: DeleteMethod::Permanent,
            errors: ErrorPolicy::Abort,
        });
        let rename = JobSpec::Rename {
            from: remote(&["a.txt"]),
            to: remote(&["b.txt"]),
        };
        let create = JobSpec::Create {
            parent: remote(&["d"]),
            name: "new".into(),
            kind: CreateKind::Dir,
        };
        vec![
            (
                Record::Submitted {
                    job: JobId(1),
                    spec: transfer,
                    at,
                },
                r#"{"v":1,"rec":{"Submitted":{"job":1,"spec":{"Transfer":{"mode":"Move","items":[{"source":"remote://p/a.txt","target":"remote://p/d/a.txt"}],"options":{"conflict":"AutoRename","errors":"Ask","symlinks":"Follow"}}},"at":{"secs_since_epoch":1000000000,"nanos_since_epoch":500}}}}"#,
            ),
            (
                Record::Submitted {
                    job: JobId(2),
                    spec: delete,
                    at,
                },
                r#"{"v":1,"rec":{"Submitted":{"job":2,"spec":{"Delete":{"paths":["remote://p/a.txt"],"method":"Permanent","errors":"Abort"}},"at":{"secs_since_epoch":1000000000,"nanos_since_epoch":500}}}}"#,
            ),
            (
                Record::Submitted {
                    job: JobId(3),
                    spec: rename,
                    at,
                },
                r#"{"v":1,"rec":{"Submitted":{"job":3,"spec":{"Rename":{"from":"remote://p/a.txt","to":"remote://p/b.txt"}},"at":{"secs_since_epoch":1000000000,"nanos_since_epoch":500}}}}"#,
            ),
            (
                Record::Submitted {
                    job: JobId(4),
                    spec: create,
                    at,
                },
                CREATE_LINE,
            ),
            (
                Record::Started { job: JobId(1) },
                r#"{"v":1,"rec":{"Started":{"job":1}}}"#,
            ),
            (
                Record::TempDir {
                    job: JobId(1),
                    dir: remote(&["d"]),
                },
                r#"{"v":1,"rec":{"TempDir":{"job":1,"dir":"remote://p/d"}}}"#,
            ),
            (
                Record::Finished {
                    job: JobId(1),
                    outcome: Some(Outcome::CompletedWithErrors),
                },
                r#"{"v":1,"rec":{"Finished":{"job":1,"outcome":"CompletedWithErrors"}}}"#,
            ),
            (
                Record::Finished {
                    job: JobId(2),
                    outcome: None,
                },
                r#"{"v":1,"rec":{"Finished":{"job":2,"outcome":null}}}"#,
            ),
        ]
    }

    /// Format 1 byte for byte. A field or variant renamed by accident fails here; one changed
    /// on purpose follows the rule of the module doc (a default, or a new format number).
    #[test]
    fn format_1_is_pinned() {
        let t = tempfile::tempdir().unwrap();
        let (want, lines): (Vec<Record>, Vec<&str>) = golden().into_iter().unzip();
        write(t.path(), want.clone());
        let text = fs::read_to_string(t.path().join(JOURNAL_FILE)).unwrap();
        assert_eq!(text.lines().collect::<Vec<_>>(), lines);
        assert!(text.ends_with('\n'));
        let read: Vec<Option<Record>> = lines.iter().map(|line| decode(line.as_bytes())).collect();
        assert_eq!(read, want.into_iter().map(Some).collect::<Vec<_>>());
    }

    /// An undo record written before a field of it was kept still reads, with the field's
    /// default. The shape of today of every record is pinned in `every_undo_record_is_pinned`.
    #[test]
    fn older_undo_records_still_read() {
        // A copy's record from before `at` was kept.
        let older = r#"{"Copied":{"created":[{"path":"remote://p/d/a.txt","is_dir":false,"size":3,"modified":{"secs_since_epoch":1000000000,"nanos_since_epoch":500},"identity":[1,7]}],"overwritten":2}}"#;
        let without = UndoRecord::Copied {
            created: vec![CreatedItem {
                path: remote(&["d", "a.txt"]),
                is_dir: false,
                size: 3,
                modified: Some(golden_time()),
                identity: Some((1, 7)),
            }],
            overwritten: 2,
            at: None,
        };
        assert_eq!(serde_json::from_str::<UndoRecord>(older).unwrap(), without);
        // A move's record from before the files it replaced and the folders it made for the
        // folders it moved across volumes were kept.
        let older = r#"{"Moved":{"moves":[{"from":"remote://p/a.txt","to":"remote://p/d/a.txt"}],"created_dirs":["remote://p/d/f"],"removed_dirs":["remote://p/f"]}}"#;
        let without = UndoRecord::Moved {
            moves: vec![MovedItem {
                from: remote(&["a.txt"]),
                to: remote(&["d", "a.txt"]),
            }],
            created_dirs: vec![remote(&["d", "f"])],
            removed_dirs: vec![remote(&["f"])],
            overwritten: 0,
            copied_dirs: Vec::new(),
        };
        assert_eq!(serde_json::from_str::<UndoRecord>(older).unwrap(), without);
    }

    /// `value` is written as `want`, and `want` reads back as `value`.
    fn pinned<T>(value: &T, want: &str)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        assert_eq!(serde_json::to_string(value).unwrap(), want);
        assert_eq!(&serde_json::from_str::<T>(want).unwrap(), value);
    }

    /// Every name of a variant that a record can hold, once. The matches make a new variant a
    /// compile error here until it is pinned too.
    #[test]
    fn every_variant_name_is_pinned() {
        use ConflictPolicy as P;
        use Outcome as O;
        for p in [
            P::Ask,
            P::Overwrite,
            P::Skip,
            P::OverwriteIfNewer,
            P::OverwriteIfSizeDiffers,
            P::AutoRename,
        ] {
            let name = match p {
                P::Ask => "Ask",
                P::Overwrite => "Overwrite",
                P::Skip => "Skip",
                P::OverwriteIfNewer => "OverwriteIfNewer",
                P::OverwriteIfSizeDiffers => "OverwriteIfSizeDiffers",
                P::AutoRename => "AutoRename",
            };
            pinned(&p, &format!("\"{name}\""));
        }
        for e in [ErrorPolicy::Continue, ErrorPolicy::Ask, ErrorPolicy::Abort] {
            let name = match e {
                ErrorPolicy::Continue => "Continue",
                ErrorPolicy::Ask => "Ask",
                ErrorPolicy::Abort => "Abort",
            };
            pinned(&e, &format!("\"{name}\""));
        }
        for s in [SymlinkMode::CopyLink, SymlinkMode::Follow] {
            let name = match s {
                SymlinkMode::CopyLink => "CopyLink",
                SymlinkMode::Follow => "Follow",
            };
            pinned(&s, &format!("\"{name}\""));
        }
        for m in [DeleteMethod::Trash, DeleteMethod::Permanent] {
            let name = match m {
                DeleteMethod::Trash => "Trash",
                DeleteMethod::Permanent => "Permanent",
            };
            pinned(&m, &format!("\"{name}\""));
        }
        for k in [CreateKind::Dir, CreateKind::File] {
            let name = match k {
                CreateKind::Dir => "Dir",
                CreateKind::File => "File",
            };
            pinned(&k, &format!("\"{name}\""));
        }
        for m in [TransferMode::Copy, TransferMode::Move] {
            let name = match m {
                TransferMode::Copy => "Copy",
                TransferMode::Move => "Move",
            };
            pinned(&m, &format!("\"{name}\""));
        }
        for o in [
            O::Completed,
            O::CompletedWithErrors,
            O::Cancelled,
            O::Aborted,
        ] {
            let name = match o {
                O::Completed => "Completed",
                O::CompletedWithErrors => "CompletedWithErrors",
                O::Cancelled => "Cancelled",
                O::Aborted => "Aborted",
            };
            pinned(&o, &format!("\"{name}\""));
        }
    }

    /// One undo record of every kind with every field it has, as the specs of undo jobs hold
    /// them: the one pinned literal of each kind.
    fn undo_golden() -> Vec<(UndoRecord, &'static str)> {
        let (a, b, d) = (remote(&["a.txt"]), remote(&["d", "a.txt"]), remote(&["d"]));
        let trashed = vec![
            TrashedItem {
                original: a.clone(),
                location: Some(remote(&["bin", "a.txt"])),
            },
            TrashedItem {
                original: b.clone(),
                location: None,
            },
        ];
        // A file with every field set, and a folder without the ones it may lack.
        let copied = vec![
            CreatedItem {
                path: b.clone(),
                is_dir: false,
                size: 3,
                modified: Some(golden_time()),
                identity: Some((1, 7)),
            },
            CreatedItem {
                path: d.clone(),
                is_dir: true,
                size: 0,
                modified: None,
                identity: None,
            },
        ];
        let (f, d_f) = (remote(&["f"]), remote(&["d", "f"]));
        vec![
            (
                UndoRecord::Copied {
                    created: copied,
                    overwritten: 2,
                    at: Some(golden_time()),
                },
                r#"{"Copied":{"created":[{"path":"remote://p/d/a.txt","is_dir":false,"size":3,"modified":{"secs_since_epoch":1000000000,"nanos_since_epoch":500},"identity":[1,7]},{"path":"remote://p/d","is_dir":true,"size":0,"modified":null,"identity":null}],"overwritten":2,"at":{"secs_since_epoch":1000000000,"nanos_since_epoch":500}}}"#,
            ),
            (
                UndoRecord::Moved {
                    moves: vec![MovedItem {
                        from: a.clone(),
                        to: b.clone(),
                    }],
                    created_dirs: vec![d_f.clone()],
                    removed_dirs: vec![f.clone()],
                    overwritten: 1,
                    copied_dirs: vec![MovedItem { from: f, to: d_f }],
                },
                r#"{"Moved":{"moves":[{"from":"remote://p/a.txt","to":"remote://p/d/a.txt"}],"created_dirs":["remote://p/d/f"],"removed_dirs":["remote://p/f"],"overwritten":1,"copied_dirs":[{"from":"remote://p/f","to":"remote://p/d/f"}]}}"#,
            ),
            (
                UndoRecord::Renamed {
                    from: a.clone(),
                    to: b.clone(),
                },
                r#"{"Renamed":{"from":"remote://p/a.txt","to":"remote://p/d/a.txt"}}"#,
            ),
            (
                UndoRecord::Created {
                    path: d.clone(),
                    kind: CreateKind::Dir,
                },
                r#"{"Created":{"path":"remote://p/d","kind":"Dir"}}"#,
            ),
            (
                UndoRecord::Removed {
                    path: a,
                    kind: CreateKind::File,
                },
                r#"{"Removed":{"path":"remote://p/a.txt","kind":"File"}}"#,
            ),
            (
                UndoRecord::Trashed {
                    items: trashed.clone(),
                },
                r#"{"Trashed":{"items":[{"original":"remote://p/a.txt","location":"remote://p/bin/a.txt"},{"original":"remote://p/d/a.txt","location":null}]}}"#,
            ),
            (
                UndoRecord::Restored { items: trashed },
                r#"{"Restored":{"items":[{"original":"remote://p/a.txt","location":"remote://p/bin/a.txt"},{"original":"remote://p/d/a.txt","location":null}]}}"#,
            ),
        ]
    }

    #[test]
    fn every_undo_record_is_pinned() {
        let golden = undo_golden();
        for (record, want) in &golden {
            pinned(record, want);
            // An undo job's spec holds its record as it is.
            pinned(
                &JobSpec::Undo(record.clone()),
                &format!(r#"{{"Undo":{want}}}"#),
            );
        }
        // One of every kind, which the match keeps true when a kind is added.
        let kind = |record: &UndoRecord| match record {
            UndoRecord::Copied { .. } => 0,
            UndoRecord::Moved { .. } => 1,
            UndoRecord::Renamed { .. } => 2,
            UndoRecord::Created { .. } => 3,
            UndoRecord::Removed { .. } => 4,
            UndoRecord::Trashed { .. } => 5,
            UndoRecord::Restored { .. } => 6,
        };
        let kinds: Vec<_> = golden.iter().map(|(record, _)| kind(record)).collect();
        assert_eq!(kinds, (0..7).collect::<Vec<_>>());
    }

    #[test]
    fn a_folder_with_this_jobs_temp_name_is_left_alone() {
        let t = tempfile::tempdir().unwrap();
        let folder = t.path().join(temp_name(B, 0));
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("inside.txt"), b"x").unwrap();
        fs::write(t.path().join(temp_name(A, 0)), b"x").unwrap();
        fs::write(t.path().join("keep.txt"), b"x").unwrap();
        assert_eq!(
            remove_orphans(&Vfs::with_local(), &incomplete(B, t.path())),
            0
        );
        assert!(folder.join("inside.txt").is_file());
        assert!(t.path().join(temp_name(A, 0)).is_file());
        assert!(t.path().join("keep.txt").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_with_this_jobs_temp_name_goes_but_not_what_it_leads_to() {
        let t = tempfile::tempdir().unwrap();
        let target = t.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("inside.txt"), b"x").unwrap();
        let link = t.path().join(temp_name(B, 0));
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            remove_orphans(&Vfs::with_local(), &incomplete(B, t.path())),
            1
        );
        assert!(fs::symlink_metadata(&link).is_err());
        assert!(target.join("inside.txt").is_file());
    }

    #[test]
    fn a_missing_temp_folder_is_skipped() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join(temp_name(B, 0)), b"x").unwrap();
        let mut job = incomplete(B, t.path());
        job.temp_dirs.insert(0, v(&t.path().join("gone")));
        assert_eq!(remove_orphans(&Vfs::with_local(), &job), 1);
    }

    #[test]
    fn a_record_after_a_torn_line_starts_a_line_of_its_own() {
        let (t, _, _) = a_done_b_running();
        append_raw(t.path(), r#"{"v":1,"rec":{"Star"#);
        write(
            t.path(),
            vec![Record::Finished {
                job: B,
                outcome: Some(Outcome::Cancelled),
            }],
        );
        assert_eq!(ids(t.path()), Vec::<JobId>::new());
    }

    #[test]
    fn a_line_torn_inside_a_character_is_ignored() {
        let (t, _, _) = a_done_b_running();
        // The first of the three UTF-8 bytes of a Hangul syllable, and nothing after it.
        let mut torn = br#"{"v":1,"rec":{"TempDir":{"job":2,"dir":"remote://p/"#.to_vec();
        torn.push("\u{d55c}".as_bytes()[0]);
        append_raw(t.path(), torn);
        assert_eq!(ids(t.path()), [B]);
    }

    #[test]
    fn a_failed_compaction_keeps_every_record() {
        let (t, _, _) = a_done_b_running();
        let file = t.path().join(JOURNAL_FILE);
        let before = fs::read(&file).unwrap();
        // A folder where the compacted copy is written makes the compaction fail.
        let blocker = t.path().join(COMPACTED_FILE);
        fs::create_dir(&blocker).unwrap();
        let recovered = recover(t.path()).unwrap();
        // The jobs are found all the same, and the failure is not hidden.
        let found: Vec<JobId> = recovered.jobs.iter().map(|j| j.id).collect();
        assert_eq!(found, [B]);
        let error = recovered.compaction.expect("the compaction failed");
        assert!(is_journal_error(&error), "{error}");
        assert_eq!(fs::read(&file).unwrap(), before);
        assert!(blocker.is_dir());
        fs::remove_dir(&blocker).unwrap();
        assert_eq!(ids(t.path()), [B]);
        assert_eq!(lines(t.path()), 3);
    }

    #[test]
    fn a_job_records_each_folder_of_its_temporary_files_once() {
        let (t, dst, spec) = copy_fixture(&["a.txt", "b.txt"], 3);
        let state = t.path().join("state");
        let (h, thread) =
            JournalHandle::spawn(state.clone(), Box::new(|| {}), Box::new(|e| panic!("{e}")))
                .unwrap();
        let vfs = Vfs::with_local();
        let report = execute(C, &spec, &ExecEnv::new(&vfs, &NonInteractive).journal(&h));
        assert_eq!((report.outcome, report.done.files), (Outcome::Completed, 2));
        h.shutdown();
        thread.join().unwrap();
        assert_eq!(
            records(&state),
            [Record::TempDir {
                job: C,
                dir: v(&dst)
            }]
        );
    }

    #[test]
    fn a_journal_that_cannot_be_written_does_not_stop_the_job() {
        let (t, dst, spec) = copy_fixture(&["a.txt"], 3);
        // A folder cannot be made under a file.
        let blocker = t.path().join("file");
        fs::write(&blocker, b"x").unwrap();
        let (h, thread, errors) = counting(blocker.join("state"));
        let vfs = Vfs::with_local();
        let report = execute(C, &spec, &ExecEnv::new(&vfs, &NonInteractive).journal(&h));
        assert_eq!(report.outcome, Outcome::Completed);
        assert_eq!(fs::read(dst.join("a.txt")).unwrap(), [7; 3]);
        h.shutdown();
        thread.join().unwrap();
        assert_eq!(errors.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_temporary_file_of_a_crashed_job_is_found_and_removed() {
        let (t, dst, spec) = copy_fixture(&["a.bin"], 2 << 20);
        let state = t.path().join("state");
        let (h, thread) =
            JournalHandle::spawn(state.clone(), Box::new(|| {}), Box::new(|e| panic!("{e}")))
                .unwrap();
        let at = golden_time();
        h.append(Record::Submitted {
            job: C,
            spec: spec.clone(),
            at,
        });
        h.append_durable(Record::Started { job: C }).unwrap();
        let p = FaultyProvider::local(vec![Fault::Crash {
            file: 0,
            after: 1 << 20,
        }]);
        // The crashed thread never returns, like a killed process.
        let _crashed = {
            let (p, h, spec) = (Arc::clone(&p), h.clone(), spec.clone());
            thread::spawn(move || {
                let vfs = p.vfs();
                execute(C, &spec, &ExecEnv::new(&vfs, &NonInteractive).journal(&h))
            })
        };
        assert!(p.wait_crashed(WAIT));
        h.shutdown();
        thread.join().unwrap();
        let found = jobs(&state);
        let want = IncompleteJob {
            id: C,
            spec,
            submitted: at,
            started: true,
            temp_dirs: vec![v(&dst)],
        };
        assert_eq!(found, std::slice::from_ref(&want));
        assert_eq!(temp_files(&dst).len(), 1);
        assert_eq!(remove_orphans(&Vfs::with_local(), &want), 1);
        assert_eq!(temp_files(&dst), Vec::<String>::new());
        assert_eq!(
            fs::read(t.path().join("src/a.bin")).unwrap(),
            vec![7; 2 << 20]
        );
    }

    #[test]
    fn a_disabled_or_stopped_journal_writes_nothing() {
        let t = tempfile::tempdir().unwrap();
        let off = JournalHandle::disabled();
        off.append(Record::Started { job: A });
        off.append_durable(Record::Started { job: A }).unwrap();
        off.shutdown();
        let (h, thread) = JournalHandle::spawn(
            t.path().to_path_buf(),
            Box::new(|| {}),
            Box::new(|e| panic!("{e}")),
        )
        .unwrap();
        h.shutdown();
        thread.join().unwrap();
        h.append(Record::Started { job: A });
        let error = h.append_durable(Record::Started { job: B }).unwrap_err();
        assert!(is_journal_error(&error), "{error}");
        assert!(!t.path().join(JOURNAL_FILE).exists());
    }

    #[test]
    fn the_start_step_runs_before_any_record() {
        let t = tempfile::tempdir().unwrap();
        let (dir, seen) = (
            t.path().to_path_buf(),
            Arc::new(AtomicUsize::new(usize::MAX)),
        );
        let lines_at_start = Arc::clone(&seen);
        let (h, thread) = JournalHandle::spawn(
            dir.clone(),
            Box::new(move || {
                let found = fs::read_to_string(dir.join(JOURNAL_FILE))
                    .map_or(0, |text| text.lines().count());
                lines_at_start.store(found, Ordering::SeqCst);
            }),
            Box::new(|e| panic!("{e}")),
        )
        .unwrap();
        h.append(Record::Started { job: A });
        h.shutdown();
        thread.join().unwrap();
        assert_eq!((seen.load(Ordering::SeqCst), lines(t.path())), (0, 1));
    }

    #[test]
    fn a_record_that_cannot_be_encoded_fails_without_writing() {
        let t = tempfile::tempdir().unwrap();
        let (h, thread, errors) = counting(t.path().to_path_buf());
        // Serde cannot store a time before 1970.
        let at = SystemTime::UNIX_EPOCH - Duration::from_secs(1);
        let error = h
            .append_durable(Record::Submitted {
                job: A,
                spec: spec_for(t.path()),
                at,
            })
            .unwrap_err();
        assert!(is_journal_error(&error), "{error}");
        h.shutdown();
        thread.join().unwrap();
        assert_eq!(errors.load(Ordering::SeqCst), 1);
        assert!(!t.path().join(JOURNAL_FILE).exists());
    }

    #[test]
    fn a_finished_line_with_an_unknown_outcome_still_ends_its_job() {
        let (t, _, _) = a_done_b_running();
        // Another format number is not read, whatever it says.
        append_raw(
            t.path(),
            "{\"v\":2,\"rec\":{\"Finished\":{\"job\":2,\"outcome\":\"Later\"}}}\n",
        );
        assert_eq!(ids(t.path()), [B]);
        // An outcome a newer build added.
        append_raw(
            t.path(),
            "{\"v\":1,\"rec\":{\"Finished\":{\"job\":2,\"outcome\":\"Later\"}}}\n",
        );
        assert_eq!(ids(t.path()), Vec::<JobId>::new());
    }

    #[test]
    fn a_compaction_cut_off_between_its_delete_and_rename_is_completed() {
        let (t, _, _) = a_done_b_running();
        assert_eq!(ids(t.path()), [B]);
        // A rename over a file on FAT or exFAT deletes the old file first; the crash came
        // between the two.
        fs::rename(t.path().join(JOURNAL_FILE), t.path().join(COMPACTED_FILE)).unwrap();
        assert_eq!(ids(t.path()), [B]);
        assert!(t.path().join(JOURNAL_FILE).is_file());
        assert!(!t.path().join(COMPACTED_FILE).exists());
        assert_eq!(lines(t.path()), 3);
    }

    #[test]
    fn a_compaction_folder_without_a_journal_is_no_journal() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join(COMPACTED_FILE)).unwrap();
        assert_eq!(ids(t.path()), Vec::<JobId>::new());
        assert!(t.path().join(COMPACTED_FILE).is_dir());
        assert!(!t.path().join(JOURNAL_FILE).exists());
    }

    #[test]
    fn a_durable_record_from_the_journal_thread_fails_instead_of_waiting() {
        let t = tempfile::tempdir().unwrap();
        let (give, take) = mpsc::channel::<JournalHandle>();
        let (tell, told) = mpsc::channel();
        let (h, thread) = JournalHandle::spawn(
            t.path().to_path_buf(),
            Box::new(move || {
                let h = take.recv().unwrap();
                let refused = h.append_durable(Record::Started { job: A });
                // A record that is not waited for is fine; it is queued before the test
                // hears back, so before its shutdown.
                h.append(Record::Started { job: B });
                tell.send(refused).unwrap();
            }),
            Box::new(|e| panic!("{e}")),
        )
        .unwrap();
        give.send(h.clone()).unwrap();
        let error = told
            .recv_timeout(WAIT)
            .expect("the journal thread waits for itself")
            .unwrap_err();
        assert!(
            matches!(&error, Error::Io { op: "journal", source, .. } if source.kind() == io::ErrorKind::Deadlock),
            "{error}"
        );
        h.shutdown();
        thread.join().unwrap();
        assert_eq!(records(t.path()), [Record::Started { job: B }]);
    }

    #[test]
    fn compaction_keeps_a_line_as_it_was_written() {
        let (t, _, _) = a_done_b_running();
        // A field this build does not know, as a newer build may add.
        let line = r#"{"v":1,"rec":{"Started":{"job":2,"later":1}}}"#;
        append_raw(t.path(), format!("{line}\n"));
        assert_eq!(ids(t.path()), [B]);
        let text = fs::read_to_string(t.path().join(JOURNAL_FILE)).unwrap();
        assert_eq!(text.lines().count(), 4);
        assert_eq!(text.lines().skip(3).collect::<Vec<_>>(), [line]);
    }
}
