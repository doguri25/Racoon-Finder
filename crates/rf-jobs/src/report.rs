//! What a finished job hands back: how it ended, what it did and what it could not do.

use std::borrow::Cow;
use std::collections::HashSet;
use std::io::{self, BufWriter, Write};
use std::time::{Duration, SystemTime};

use rf_core::{ErrorKind, VPath};
use serde::{Deserialize, Serialize};

use crate::id::JobId;
use crate::preflight::Issue;
use crate::spec::{DeleteSpec, JobKind, JobSpec, TransferItem, TransferSpec};
use crate::undo::UndoRecord;

/// The step of a job an item failed in, for the UI to pick the wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Stage {
    /// Walking the sources to count and list what the job will touch.
    Plan,
    /// Checking the target file system's limits before anything is written.
    Preflight,
    /// Looking for an existing item at the target and applying the conflict policy.
    Conflict,
    /// Opening or reading a source file or link.
    Read,
    /// Creating a target folder or writing a temporary file.
    Write,
    /// Renaming the finished temporary file to its final name.
    Commit,
    /// Copying times, attributes and the other metadata onto the target.
    Metadata,
    /// Removing a source after a move has committed its copy.
    DeleteSource,
    /// Deleting or trashing an item in a delete job.
    Delete,
    /// Renaming an item.
    Rename,
    /// Creating a new empty file or folder.
    Create,
    /// Reversing an earlier job.
    Undo,
    /// A fault in the engine itself, such as a panic in a worker.
    Internal,
}

impl Stage {
    /// The word the log writes for this step: `plan`, `preflight`, `conflict`, `read`,
    /// `write`, `commit`, `metadata`, `delete_source`, `delete`, `rename`, `create`, `undo` or
    /// `internal`. It is English whatever language the UI shows.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Preflight => "preflight",
            Self::Conflict => "conflict",
            Self::Read => "read",
            Self::Write => "write",
            Self::Commit => "commit",
            Self::Metadata => "metadata",
            Self::DeleteSource => "delete_source",
            Self::Delete => "delete",
            Self::Rename => "rename",
            Self::Create => "create",
            Self::Undo => "undo",
            Self::Internal => "internal",
        }
    }
}

/// An item a job could not process, or processed only in part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemFailure {
    /// The item that was read. Below a link that a copy in Follow mode followed, it is the real
    /// path of the item with every link resolved (`…/outside/sub/x`), not the path through the
    /// link (`…/dir/dlink/x`).
    pub source: Option<VPath>,
    pub target: Option<VPath>,
    pub stage: Stage,
    pub kind: ErrorKind,
    /// The error's log text, in English. The UI words its own message from `stage` and `kind`.
    pub message: String,
}

impl ItemFailure {
    /// The failure of `stage` on `source` and `target`, described by `error`.
    pub fn new(
        stage: Stage,
        source: Option<&VPath>,
        target: Option<&VPath>,
        error: &rf_core::Error,
    ) -> ItemFailure {
        ItemFailure {
            source: source.cloned(),
            target: target.cloned(),
            stage,
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

/// How a job ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Outcome {
    /// Every item was processed (skips the user chose count as processed).
    Completed,
    /// The job ran to the end but some items failed; see `JobReport::failures`.
    CompletedWithErrors,
    /// The user cancelled the job.
    Cancelled,
    /// The job stopped early on an error or a refused precondition.
    Aborted,
}

impl Outcome {
    /// The word the log writes for this outcome: `completed`, `completed_with_errors`,
    /// `cancelled` or `aborted`. It is English whatever language the UI shows.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::CompletedWithErrors => "completed_with_errors",
            Self::Cancelled => "cancelled",
            Self::Aborted => "aborted",
        }
    }
}

/// A count of files, folders and bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Totals {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
}

/// The record of one job: what it was asked, how it ended and what it left behind.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct JobReport {
    pub id: JobId,
    pub kind: JobKind,
    pub spec: JobSpec,
    pub outcome: Outcome,
    /// What the plan expected to process.
    pub planned: Totals,
    /// What was processed.
    pub done: Totals,
    /// Items left alone by the conflict policy or because they were the same item.
    pub skipped: u64,
    /// Items that could not be processed.
    pub failures: Vec<ItemFailure>,
    /// Items that were processed except for some part of them, such as their metadata.
    pub warnings: Vec<ItemFailure>,
    /// What the target file system cannot hold, found before the job wrote anything.
    pub issues: Vec<Issue>,
    /// The top-level paths the job made, or moved its items to, with any automatic rename
    /// applied. The UI selects them and starts a rename on a new item.
    pub outputs: Vec<VPath>,
    /// A move that copied instead and left every source in place, because the folder of one
    /// of its sources cannot be written to (DND-06). A move to another volume copies and then
    /// deletes, and is still a move.
    pub copied_instead_of_moved: bool,
    pub started: SystemTime,
    pub elapsed: Duration,
    /// How to reverse the job, when it can be reversed.
    pub undo: Option<UndoRecord>,
}

impl JobReport {
    /// The report of a job that has not done anything yet: completed, with nothing counted.
    pub(crate) fn new(id: JobId, spec: &JobSpec) -> JobReport {
        JobReport {
            id,
            kind: spec.kind(),
            spec: spec.clone(),
            outcome: Outcome::Completed,
            planned: Totals::default(),
            done: Totals::default(),
            skipped: 0,
            failures: Vec::new(),
            warnings: Vec::new(),
            issues: Vec::new(),
            outputs: Vec::new(),
            copied_instead_of_moved: false,
            started: SystemTime::now(),
            elapsed: Duration::ZERO,
            undo: None,
        }
    }

    /// The bytes processed per second over the whole job: `done.bytes` over `elapsed`, in
    /// whole milliseconds and at least one, so a job that took no time at all has no division
    /// by zero. The product saturates, so huge counts give `u64::MAX` and no panic.
    pub fn average_speed(&self) -> u64 {
        let millis = u64::try_from(self.elapsed.as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        self.done.bytes.saturating_mul(1_000) / millis
    }

    /// The job to submit to try again what went wrong (XFER-09: review the errors at the end,
    /// then retry); `None` when there is nothing to retry. Only failures count, never
    /// warnings, and the retry keeps the original's mode, method and policies.
    ///
    /// - A transfer retries the items that failed before their content was in place, in the
    ///   step `Plan`, `Preflight`, `Conflict`, `Read`, `Write` or `Commit`, and only when the
    ///   failure names both a source and a target. A failure of `DeleteSource` is left out:
    ///   its copy is in place already, and a retry would meet that copy as a conflict. The
    ///   failure's own paths are used, so an item below a folder is retried by itself, to the
    ///   path it was being written to (a numbered name included), and a move leaves the
    ///   source folder it came from where it is.
    /// - A delete retries the paths that failed, with the same method and error policy. A
    ///   trash is never turned into a permanent delete here. The UI may offer one, and only
    ///   for a failure of kind [`ErrorKind::Unsupported`] (the trash refused the item) and
    ///   only after the user has said yes: it then sets `DeleteSpec::method` itself on the
    ///   spec this gives. A folder a permanent delete kept because of a failure below it is
    ///   not in the retry.
    /// - A rename, a create or an undo is retried as it was asked, when it has any failure.
    ///
    /// Items that a cancelled or aborted job never reached are not failures and are not in the
    /// retry.
    pub fn retry_spec(&self) -> Option<JobSpec> {
        match &self.spec {
            JobSpec::Transfer(spec) => self.retry_transfer(spec),
            JobSpec::Delete(spec) => self.retry_delete(spec),
            JobSpec::Rename { .. } | JobSpec::Create { .. } | JobSpec::Undo(_) => {
                (!self.failures.is_empty()).then(|| self.spec.clone())
            }
        }
    }

    fn retry_transfer(&self, spec: &TransferSpec) -> Option<JobSpec> {
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        for failure in &self.failures {
            let before_the_copy_was_in_place = matches!(
                failure.stage,
                Stage::Plan
                    | Stage::Preflight
                    | Stage::Conflict
                    | Stage::Read
                    | Stage::Write
                    | Stage::Commit
            );
            if !before_the_copy_was_in_place {
                continue;
            }
            let (Some(source), Some(target)) = (&failure.source, &failure.target) else {
                continue;
            };
            let item = TransferItem {
                source: source.clone(),
                target: target.clone(),
            };
            if seen.insert(item.clone()) {
                items.push(item);
            }
        }
        (!items.is_empty()).then_some(JobSpec::Transfer(TransferSpec {
            mode: spec.mode,
            items,
            options: spec.options,
        }))
    }

    fn retry_delete(&self, spec: &DeleteSpec) -> Option<JobSpec> {
        let mut seen = HashSet::new();
        let paths: Vec<VPath> = self
            .failures
            .iter()
            .filter_map(|failure| failure.source.as_ref())
            .filter(|path| seen.insert(*path))
            .cloned()
            .collect();
        (!paths.is_empty()).then_some(JobSpec::Delete(DeleteSpec {
            paths,
            method: spec.method,
            errors: spec.errors,
        }))
    }

    /// Writes the log of the job (XFER-14, "save the log") to `out`, one record per line, in
    /// English, whatever language the UI shows. The UI picks the file and writes it off the
    /// UI thread; the records are buffered here, so `out` need not be.
    ///
    /// The lines, in this order:
    ///
    /// 1. `job {id} {kind} {outcome}`
    /// 2. `planned files={} dirs={} bytes={}`
    /// 3. `done files={} dirs={} bytes={} skipped={} failed={} warnings={}`
    /// 4. `elapsed_ms={} average_bps={}`
    /// 5. for each issue, `issue target={target} {text}` (see [`Issue::log_line`])
    /// 6. for each failure, `failure stage={stage} kind={kind:?} source={source} target={target}
    ///    message={message}`, where a missing path is `-`; then the same for each warning, as
    ///    `warning …`
    ///
    /// Every path, name and text in a line has its control characters written as escapes
    /// (`\n`, `\t`, `\u{7f}`), so a file name with a line break in it cannot end a record
    /// early or forge another one.
    pub fn write_log(&self, out: &mut dyn Write) -> io::Result<()> {
        let mut out = BufWriter::new(out);
        writeln!(
            out,
            "job {} {} {}",
            self.id,
            self.kind.name(),
            self.outcome.name()
        )?;
        writeln!(
            out,
            "planned files={} dirs={} bytes={}",
            self.planned.files, self.planned.dirs, self.planned.bytes
        )?;
        writeln!(
            out,
            "done files={} dirs={} bytes={} skipped={} failed={} warnings={}",
            self.done.files,
            self.done.dirs,
            self.done.bytes,
            self.skipped,
            self.failures.len(),
            self.warnings.len()
        )?;
        writeln!(
            out,
            "elapsed_ms={} average_bps={}",
            self.elapsed.as_millis(),
            self.average_speed()
        )?;
        for issue in &self.issues {
            writeln!(out, "{}", issue.log_line())?;
        }
        for failure in &self.failures {
            write_item(&mut out, "failure", failure)?;
        }
        for warning in &self.warnings {
            write_item(&mut out, "warning", warning)?;
        }
        out.flush()
    }
}

impl Issue {
    /// The line the log (see [`JobReport::write_log`]) and a command-line question write for
    /// this issue: `issue target={target} {text}`, with `text` the English log text of the
    /// issue (the `Display` of [`IssueKind`](crate::IssueKind)). Control characters in either
    /// are written as escapes, so the line is one line whatever the names in it.
    pub fn log_line(&self) -> String {
        format!(
            "issue target={} {}",
            one_line(&self.target.to_string()),
            one_line(&self.kind.to_string())
        )
    }
}

/// `text` for one line of a log: every control character is written as its escape (`\n`,
/// `\t`, `\u{7f}`), every other character as it is.
fn one_line(text: &str) -> Cow<'_, str> {
    if !text.chars().any(char::is_control) {
        return Cow::Borrowed(text);
    }
    let mut line = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if c.is_control() {
            line.extend(c.escape_debug());
        } else {
            line.push(c);
        }
    }
    Cow::Owned(line)
}

/// The path of a failure for a log line: `-` when there is none.
fn path_field(path: Option<&VPath>) -> String {
    path.map_or_else(
        || "-".to_owned(),
        |path| one_line(&path.to_string()).into_owned(),
    )
}

/// The record of a failure or a warning, as its `label`.
fn write_item(out: &mut dyn Write, label: &str, item: &ItemFailure) -> io::Result<()> {
    writeln!(
        out,
        "{label} stage={} kind={:?} source={} target={} message={}",
        item.stage.name(),
        item.kind,
        path_field(item.source.as_ref()),
        path_field(item.target.as_ref()),
        one_line(&item.message)
    )
}

#[cfg(test)]
mod tests {
    use rf_core::{Error, VPath};

    use super::*;
    use crate::preflight::IssueKind;
    use crate::spec::{
        ConflictPolicy, CreateKind, DeleteMethod, ErrorPolicy, SymlinkMode, TransferMode,
        TransferOptions,
    };

    #[test]
    fn a_new_report_is_complete_and_empty() {
        let before = SystemTime::now();
        let dir = VPath::local(std::env::temp_dir()).unwrap();
        let spec = JobSpec::Create {
            parent: dir,
            name: "x".into(),
            kind: CreateKind::Dir,
        };
        let id = JobId(7);
        let report = JobReport::new(id, &spec);
        assert_eq!(
            (report.id, report.kind, report.outcome),
            (id, JobKind::Create, Outcome::Completed)
        );
        assert_eq!(report.spec, spec);
        assert_eq!(
            (report.planned, report.done, report.skipped),
            (Totals::default(), Totals::default(), 0)
        );
        assert_eq!(report.failures, Vec::new());
        assert_eq!(report.warnings, Vec::new());
        assert_eq!(report.issues, Vec::new());
        assert_eq!(report.outputs, Vec::<VPath>::new());
        assert!(!report.copied_instead_of_moved);
        assert_eq!(report.undo, None);
        assert_eq!(report.elapsed, Duration::ZERO);
        assert!(report.started >= before);
    }

    fn path(name: &str) -> VPath {
        VPath::local(std::env::temp_dir())
            .unwrap()
            .join(name)
            .unwrap()
    }

    fn failed(stage: Stage, source: Option<&str>, target: Option<&str>) -> ItemFailure {
        ItemFailure::new(
            stage,
            source.map(path).as_ref(),
            target.map(path).as_ref(),
            &Error::Unsupported("no".into()),
        )
    }

    fn transfer_options() -> TransferOptions {
        TransferOptions {
            conflict: ConflictPolicy::AutoRename,
            errors: ErrorPolicy::Abort,
            symlinks: SymlinkMode::Follow,
        }
    }

    fn report_of(spec: &JobSpec) -> JobReport {
        JobReport::new(JobId(0x2a), spec)
    }

    fn move_report() -> JobReport {
        report_of(&JobSpec::Transfer(TransferSpec {
            mode: TransferMode::Move,
            items: Vec::new(),
            options: transfer_options(),
        }))
    }

    fn delete_report(method: DeleteMethod, errors: ErrorPolicy) -> JobReport {
        report_of(&JobSpec::Delete(DeleteSpec {
            paths: vec![path("p1"), path("p2"), path("p3")],
            method,
            errors,
        }))
    }

    fn log_of(report: &JobReport) -> String {
        let mut out = Vec::new();
        report.write_log(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn names_are_the_words_of_the_log() {
        let kinds = [
            (JobKind::Copy, "copy"),
            (JobKind::Move, "move"),
            (JobKind::Delete, "delete"),
            (JobKind::Rename, "rename"),
            (JobKind::Create, "create"),
            (JobKind::Undo, "undo"),
        ];
        for (kind, name) in kinds {
            assert_eq!(kind.name(), name);
        }
        let outcomes = [
            (Outcome::Completed, "completed"),
            (Outcome::CompletedWithErrors, "completed_with_errors"),
            (Outcome::Cancelled, "cancelled"),
            (Outcome::Aborted, "aborted"),
        ];
        for (outcome, name) in outcomes {
            assert_eq!(outcome.name(), name);
        }
        let stages = [
            (Stage::Plan, "plan"),
            (Stage::Preflight, "preflight"),
            (Stage::Conflict, "conflict"),
            (Stage::Read, "read"),
            (Stage::Write, "write"),
            (Stage::Commit, "commit"),
            (Stage::Metadata, "metadata"),
            (Stage::DeleteSource, "delete_source"),
            (Stage::Delete, "delete"),
            (Stage::Rename, "rename"),
            (Stage::Create, "create"),
            (Stage::Undo, "undo"),
            (Stage::Internal, "internal"),
        ];
        for (stage, name) in stages {
            assert_eq!(stage.name(), name);
        }
    }

    #[test]
    fn the_average_speed_neither_divides_by_zero_nor_overflows() {
        let mut report = move_report();
        report.done.bytes = 5;
        // No time at all counts as one millisecond.
        assert_eq!(report.average_speed(), 5_000);
        report.elapsed = Duration::from_millis(2_500);
        assert_eq!(report.average_speed(), 2);
        // Less than a millisecond is no time at all.
        report.elapsed = Duration::from_micros(900);
        assert_eq!(report.average_speed(), 5_000);
        report.done.bytes = u64::MAX;
        report.elapsed = Duration::from_millis(1);
        assert_eq!(report.average_speed(), u64::MAX);
        // A span that does not fit 64 bits of milliseconds.
        report.done.bytes = 5;
        report.elapsed = Duration::MAX;
        assert_eq!(report.average_speed(), 0);
    }

    #[test]
    fn a_transfer_retry_holds_the_items_that_failed_before_they_were_in_place() {
        let mut report = move_report();
        report.failures = vec![
            failed(Stage::Plan, Some("a"), Some("A")),
            // The copy is in place already.
            failed(Stage::DeleteSource, Some("b"), Some("B")),
            failed(Stage::Metadata, Some("c"), Some("C")),
            // Without both a source and a target there is nothing to retry.
            failed(Stage::Read, Some("d"), None),
            failed(Stage::Commit, None, Some("E")),
            failed(Stage::Internal, None, None),
            // The same item again.
            failed(Stage::Write, Some("a"), Some("A")),
            failed(Stage::Conflict, Some("f"), Some("F")),
            failed(Stage::Preflight, Some("g"), Some("G")),
            failed(Stage::Commit, Some("h"), Some("H")),
            failed(Stage::Read, Some("i"), Some("I")),
            failed(Stage::Write, Some("j"), Some("J")),
            failed(Stage::Undo, Some("k"), Some("K")),
        ];
        report.warnings = vec![failed(Stage::Write, Some("w"), Some("W"))];
        let Some(JobSpec::Transfer(retry)) = report.retry_spec() else {
            panic!("{:?}", report.retry_spec())
        };
        assert_eq!(
            (retry.mode, retry.options),
            (TransferMode::Move, transfer_options())
        );
        let pairs: Vec<(VPath, VPath)> = retry
            .items
            .into_iter()
            .map(|item| (item.source, item.target))
            .collect();
        let expected: Vec<(VPath, VPath)> = [
            ("a", "A"),
            ("f", "F"),
            ("g", "G"),
            ("h", "H"),
            ("i", "I"),
            ("j", "J"),
        ]
        .map(|(source, target)| (path(source), path(target)))
        .into();
        assert_eq!(pairs, expected);
    }

    #[test]
    fn a_transfer_whose_failures_cannot_be_retried_has_nothing_to_retry() {
        let mut report = move_report();
        assert_eq!(report.retry_spec(), None);
        report.failures = vec![
            failed(Stage::DeleteSource, Some("b"), Some("B")),
            failed(Stage::Internal, None, None),
        ];
        report.warnings = vec![failed(Stage::Write, Some("w"), Some("W"))];
        assert_eq!(report.retry_spec(), None);
    }

    #[test]
    fn a_delete_retry_keeps_the_method_and_the_policy() {
        for (method, errors) in [
            (DeleteMethod::Trash, ErrorPolicy::Ask),
            (DeleteMethod::Permanent, ErrorPolicy::Abort),
        ] {
            let mut report = delete_report(method, errors);
            assert_eq!(report.retry_spec(), None);
            report.failures = vec![
                failed(Stage::Delete, Some("p3"), None),
                failed(Stage::Internal, None, None),
                failed(Stage::Plan, Some("p1"), None),
                failed(Stage::Delete, Some("p3"), None),
            ];
            report.warnings = vec![failed(Stage::Delete, Some("p2"), None)];
            assert_eq!(
                report.retry_spec(),
                Some(JobSpec::Delete(DeleteSpec {
                    paths: vec![path("p3"), path("p1")],
                    method,
                    errors,
                }))
            );
        }
    }

    #[test]
    fn a_rename_a_create_and_an_undo_are_retried_as_they_were_asked() {
        let specs = [
            JobSpec::Rename {
                from: path("a"),
                to: path("b"),
            },
            JobSpec::Create {
                parent: path("a"),
                name: "x".into(),
                kind: CreateKind::File,
            },
            JobSpec::Undo(UndoRecord::Renamed {
                from: path("a"),
                to: path("b"),
            }),
        ];
        for spec in specs {
            let mut report = report_of(&spec);
            assert_eq!(report.retry_spec(), None);
            report.warnings = vec![failed(Stage::Rename, Some("a"), Some("b"))];
            assert_eq!(report.retry_spec(), None);
            report.failures = vec![failed(Stage::Rename, None, None)];
            assert_eq!(report.retry_spec(), Some(spec));
        }
    }

    #[test]
    fn the_log_has_a_record_for_everything_the_report_holds() {
        let mut report = move_report();
        report.outcome = Outcome::CompletedWithErrors;
        report.planned = Totals {
            files: 4,
            dirs: 2,
            bytes: 9_000,
        };
        report.done = Totals {
            files: 3,
            dirs: 1,
            bytes: 3_000,
        };
        report.skipped = 5;
        report.elapsed = Duration::from_millis(1_500);
        report.issues = vec![
            Issue {
                target: path("t1"),
                kind: IssueKind::InvalidCharacter('?'),
            },
            Issue {
                target: path("t2"),
                kind: IssueKind::NotEnoughSpace {
                    needed: 9,
                    available: 4,
                },
            },
        ];
        report.failures = vec![
            failed(Stage::Commit, Some("a"), Some("b")),
            failed(Stage::Internal, None, None),
        ];
        report.warnings = vec![failed(Stage::Metadata, Some("c"), None)];
        let shown = |name: &str| path(name).to_string();
        let expected = format!(
            "job 000000000000002a move completed_with_errors\n\
             planned files=4 dirs=2 bytes=9000\n\
             done files=3 dirs=1 bytes=3000 skipped=5 failed=2 warnings=1\n\
             elapsed_ms=1500 average_bps=2000\n\
             issue target={t1} name contains '?', which the target file system does not allow\n\
             issue target={t2} needs 9 bytes; 4 available\n\
             failure stage=commit kind=Unsupported source={a} target={b} message=not supported: no\n\
             failure stage=internal kind=Unsupported source=- target=- message=not supported: no\n\
             warning stage=metadata kind=Unsupported source={c} target=- message=not supported: no\n",
            t1 = shown("t1"),
            t2 = shown("t2"),
            a = shown("a"),
            b = shown("b"),
            c = shown("c"),
        );
        assert_eq!(log_of(&report), expected);
        assert_eq!(
            report.issues[0].log_line(),
            expected.lines().nth(4).unwrap()
        );
    }

    #[test]
    fn control_characters_are_escaped_in_every_field_of_a_record() {
        let mut report = move_report();
        report.issues = vec![
            Issue {
                target: path("t"),
                kind: IssueKind::InvalidCharacter('\n'),
            },
            Issue {
                target: path("u"),
                kind: IssueKind::CaseCollision {
                    other: "a\tb\r\nc".into(),
                },
            },
        ];
        let error = Error::Unsupported("first\nfailure: x\r\nforged line\u{7f}".into());
        report.failures = vec![ItemFailure::new(Stage::Write, None, None, &error)];
        report.warnings = vec![ItemFailure::new(Stage::Metadata, None, None, &error)];
        let log = log_of(&report);
        assert_eq!(log.matches('\n').count(), 8, "{log:?}");
        assert!(log.chars().all(|c| !c.is_control() || c == '\n'), "{log:?}");
        let lines: Vec<&str> = log.lines().collect();
        assert!(
            lines[4].ends_with("name contains '\\n', which the target file system does not allow"),
            "{}",
            lines[4]
        );
        assert!(
            lines[5].ends_with("name differs only in case from a\\tb\\r\\nc"),
            "{}",
            lines[5]
        );
        let message = "message=not supported: first\\nfailure: x\\r\\nforged line\\u{7f}";
        assert!(
            lines[6].starts_with("failure stage=write ") && lines[6].ends_with(message),
            "{}",
            lines[6]
        );
        assert!(
            lines[7].starts_with("warning stage=metadata ") && lines[7].ends_with(message),
            "{}",
            lines[7]
        );
    }

    #[cfg(unix)]
    #[test]
    fn control_characters_in_a_path_are_escaped_too() {
        use std::os::unix::ffi::OsStrExt;

        let name = std::ffi::OsStr::from_bytes(b"a\nb\tc");
        let odd = VPath::local(std::env::temp_dir().join(name)).unwrap();
        let mut report = move_report();
        report.issues = vec![Issue {
            target: odd.clone(),
            kind: IssueKind::ReservedName,
        }];
        let error = Error::Unsupported("x".into());
        report.failures = vec![ItemFailure::new(
            Stage::Read,
            Some(&odd),
            Some(&odd),
            &error,
        )];
        let log = log_of(&report);
        assert_eq!(log.matches('\n').count(), 6, "{log:?}");
        let base = VPath::local(std::env::temp_dir()).unwrap();
        let shown = format!("{base}/a\\nb\\tc");
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(
            lines[4],
            format!("issue target={shown} name is reserved on Windows")
        );
        assert!(
            lines[5].contains(&format!("source={shown} target={shown} ")),
            "{}",
            lines[5]
        );
    }

    #[test]
    fn one_line_escapes_control_characters_and_nothing_else() {
        let plain = "tab\u{2003}and 한글 'quoted' back\\slash \u{2028}";
        assert!(matches!(one_line(plain), Cow::Borrowed(_)));
        assert_eq!(one_line(plain), plain);
        assert_eq!(
            one_line("a\nb\tc\rd\0e\u{7f}f\u{85}g"),
            "a\\nb\\tc\\rd\\0e\\u{7f}f\\u{85}g"
        );
    }

    #[test]
    fn a_log_that_cannot_be_written_is_an_error() {
        struct Full;
        impl Write for Full {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::StorageFull))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let error = move_report().write_log(&mut Full).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::StorageFull);
    }
}
