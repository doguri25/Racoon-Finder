//! The windows that open over the main window, as plain data: the name of a new folder or of a
//! rename, a confirmation, the failed items of a job, the jobs the last run left unfinished,
//! the questions of the jobs, and the destination picker ([`Picker`]).
//! [`AppState`](crate::state::AppState) keeps the one that shows and the ones waiting behind
//! it; `dialogs.slint`, `conflict-dialog.slint` and `pick-dialog.slint` draw them.
//!
//! Nothing here touches a file: a dialog that is answered gives the job to submit, and the
//! state hands it to the engine.

use std::ffi::OsString;

use rf_core::{EntryKind, Os, VPath};
use rf_jobs::{
    Conflict, ConflictAction, ConflictAnswer, CreateKind, DeleteMethod, DeleteSpec, ErrorPolicy,
    IncompleteJob, Issue, IssueKind, JobId, JobSpec, MTIME_TOLERANCE, PreflightAnswer, Severity,
    UndoRecord, numbered_name,
};

use crate::pick::Picker;
use crate::resolver::QuestionId;
use crate::selection::{NamePattern, PatternError, PatternSyntax};
use crate::state::FolderView;
use crate::words::{self, JobWork};

/// A window over the main window. While one shows, the main window takes no keys or clicks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Modal {
    Name(NameDialog),
    Confirm(Confirm),
    Failures(Failures),
    Recovery(Recovery),
    Conflict(ConflictQuestion),
    Preflight(PreflightQuestion),
    /// Where to copy or move the items to (PICK).
    Pick(Box<Picker>),
    /// Which rows to select by a pattern of their names (NAV-15).
    Pattern(PatternDialog),
}

impl Modal {
    /// The job question this window asks, if it asks one.
    pub fn question(&self) -> Option<QuestionId> {
        match self {
            Modal::Conflict(question) => Some(question.id),
            Modal::Preflight(question) => Some(question.id),
            Modal::Name(_)
            | Modal::Confirm(_)
            | Modal::Failures(_)
            | Modal::Recovery(_)
            | Modal::Pick(_)
            | Modal::Pattern(_) => None,
        }
    }
}

/// What a name window names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamePurpose {
    /// A new folder in `parent`, asked for in pane `origin`.
    NewFolder { parent: VPath, origin: usize },
    /// A new name for `from`, asked for in pane `origin`.
    Rename { from: VPath, origin: usize },
    /// A new folder in `parent`, asked for by the destination picker's 새 폴더, which takes it
    /// as its target once it is made.
    PickFolder { parent: VPath },
}

/// The window that asks for a name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameDialog {
    pub purpose: NamePurpose,
    /// The name the field starts with.
    pub initial: String,
    /// Where the selection the field starts with ends, in bytes from the start: before the
    /// extension of a file, so that typing replaces the name and keeps the extension.
    pub select_end: usize,
    /// What the field holds now.
    pub text: String,
}

/// What [`NameDialog::check`] makes of the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameCheck {
    /// The name can be asked for.
    Ready,
    /// Nothing but spaces.
    Empty,
    /// A rename to the name the item has.
    Unchanged,
    /// The name cannot be a name; the warning says why.
    Invalid(String),
}

impl NameDialog {
    /// The window for a new folder in `parent`, with the default name.
    pub fn new_folder(parent: VPath, origin: usize) -> NameDialog {
        let name = words::new_folder_name().to_owned();
        NameDialog {
            purpose: NamePurpose::NewFolder { parent, origin },
            select_end: name.len(),
            text: name.clone(),
            initial: name,
        }
    }

    /// The window for a new folder in `parent` that the destination picker asks for, with the
    /// default name.
    pub fn pick_folder(parent: VPath) -> NameDialog {
        let name = words::new_folder_name().to_owned();
        NameDialog {
            purpose: NamePurpose::PickFolder { parent },
            select_end: name.len(),
            text: name.clone(),
            initial: name,
        }
    }

    /// The window for a new name of `from`, a folder when `is_dir`; `None` for a path without
    /// a name (a root).
    pub fn rename(from: VPath, is_dir: bool, origin: usize) -> Option<NameDialog> {
        let name = from.file_name()?.to_string_lossy().into_owned();
        Some(NameDialog {
            select_end: stem_end(&name, is_dir),
            purpose: NamePurpose::Rename { from, origin },
            text: name.clone(),
            initial: name,
        })
    }

    pub fn title(&self) -> &'static str {
        match self.purpose {
            NamePurpose::NewFolder { .. } | NamePurpose::PickFolder { .. } => {
                words::new_folder_title()
            }
            NamePurpose::Rename { .. } => words::rename_title(),
        }
    }

    /// What the field holds now, for this system's names.
    pub fn check(&self) -> NameCheck {
        if self.text.trim().is_empty() {
            return NameCheck::Empty;
        }
        if let Some(problem) = name_problem(&self.text, Os::CURRENT) {
            return NameCheck::Invalid(problem);
        }
        if matches!(self.purpose, NamePurpose::Rename { .. }) && self.text == self.initial {
            return NameCheck::Unchanged;
        }
        NameCheck::Ready
    }

    /// The job the field asks for, with the pane that asked (none for the picker's folder);
    /// `Err` with the warning when the name cannot be used, `Ok(None)` when there is nothing
    /// to do (an empty or unchanged name).
    pub fn job(&self) -> Result<Option<(JobSpec, Option<usize>)>, String> {
        match self.check() {
            NameCheck::Ready => {}
            NameCheck::Empty | NameCheck::Unchanged => return Ok(None),
            NameCheck::Invalid(problem) => return Err(problem),
        }
        let name = OsString::from(&self.text);
        match &self.purpose {
            NamePurpose::NewFolder { parent, origin } => Ok(Some((
                JobSpec::Create {
                    parent: parent.clone(),
                    name,
                    kind: CreateKind::Dir,
                },
                Some(*origin),
            ))),
            NamePurpose::PickFolder { parent } => Ok(Some((
                JobSpec::Create {
                    parent: parent.clone(),
                    name,
                    kind: CreateKind::Dir,
                },
                None,
            ))),
            NamePurpose::Rename { from, origin } => {
                let to = from
                    .parent()
                    .and_then(|parent| parent.join(&name).ok())
                    .ok_or_else(|| words::error_kind(rf_core::ErrorKind::InvalidPath).to_owned())?;
                Ok(Some((
                    JobSpec::Rename {
                        from: from.clone(),
                        to,
                    },
                    Some(*origin),
                )))
            }
        }
    }
}

/// What the pattern window asks for: the pattern, how it is written, whether case counts, and
/// whether the rows it matches join the selection or replace it. The next window starts with
/// the choices of the last one confirmed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PatternChoice {
    pub text: String,
    pub syntax: PatternSyntax,
    /// Case counts; off (대소문자 무시) by default.
    pub match_case: bool,
    /// The rows join the selection (선택에 더하기); off (선택 바꾸기) by default.
    pub add: bool,
}

impl PatternChoice {
    /// The pattern of the choices; `Ok(None)` for one with nothing to match.
    pub fn pattern(&self) -> Result<Option<NamePattern>, PatternError> {
        NamePattern::new(&self.text, self.syntax, !self.match_case)
    }
}

/// The window that selects the rows whose names match a pattern (NAV-15 패턴으로 선택). A
/// worker matches the names as they are typed, for the count it shows: the window keeps how
/// far that is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternDialog {
    /// The pane whose rows it selects, and the tab that showed there (its id): a tab that has
    /// left the pane since takes nothing.
    pub side: usize,
    pub tab: u64,
    pub choice: PatternChoice,
    /// The text the field starts with.
    pub initial: String,
    /// Why the text is no pattern; `None` when it is one, or is empty.
    pub problem: Option<PatternError>,
    /// The text has nothing to match: it is empty, or wildcards of nothing but `;` and spaces.
    pub empty: bool,
    /// The request of the worker's match for the choices as they are.
    pub generation: u64,
    /// How many rows the pattern matches: for `generation` once the worker is done, the last
    /// count meanwhile.
    pub count: Option<usize>,
    /// A worker matches the names for `generation`.
    pub counting: bool,
    /// 선택 was chosen while the worker matched: the rows are selected once it is done.
    pub confirming: bool,
}

impl PatternDialog {
    /// The window for the rows of tab `tab` of pane `side`, starting with `choice`.
    pub fn new(side: usize, tab: u64, choice: PatternChoice) -> PatternDialog {
        PatternDialog {
            side,
            tab,
            initial: choice.text.clone(),
            choice,
            problem: None,
            empty: true,
            generation: 0,
            count: None,
            counting: false,
            confirming: false,
        }
    }

    /// Whether 선택 can select now: the text is a pattern with something to match, and 선택
    /// was not chosen already.
    pub fn can_confirm(&self) -> bool {
        self.problem.is_none() && !self.empty && !self.confirming
    }
}

/// Why `text` can be no name of an item on `os`, as the warning the name window shows while
/// it is typed; `None` when it can. A name the target's file system refuses for other reasons
/// (a reserved name, a trailing dot, a length) is the job's to refuse, with its own words.
pub fn name_problem(text: &str, os: Os) -> Option<String> {
    if text == "." || text == ".." {
        return Some(words::name_is_dots().to_owned());
    }
    let forbidden: &[char] = match os {
        Os::Windows => &['\\', '/', ':', '*', '?', '"', '<', '>', '|'],
        Os::MacOs | Os::Other => &['/'],
    };
    text.chars()
        .find(|c| forbidden.contains(c) || *c == '\0' || (os == Os::Windows && c.is_control()))
        .map(words::name_has_character)
}

/// The byte offset where the stem of `name` ends: before its last dot for a file with an
/// extension, the whole name for a folder, for a name without a dot and for a dot file
/// (`.bashrc`).
pub fn stem_end(name: &str, is_dir: bool) -> usize {
    if is_dir {
        return name.len();
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => dot,
        _ => name.len(),
    }
}

/// Why a permanent delete is asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteWhy {
    /// The user asked for it (Shift+Delete, or the menu).
    Asked,
    /// The folder's location has no trash (OPS-03).
    NoTrash,
    /// The trash refused the items.
    TrashRefused,
}

/// What a confirmation is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmPurpose {
    /// Delete these items for good.
    DeletePermanently(Vec<VPath>),
    /// Delete for good the copies an undo of the copy `record` could not send to the trash,
    /// whose originals (`originals`, one for each copy) a worker has found as they were.
    DeleteCopies {
        copies: Vec<VPath>,
        originals: Vec<VPath>,
        record: Box<UndoRecord>,
    },
    /// End the app while jobs run.
    Quit,
    /// 하위 폴더에 적용: `view` for `folder` and the folders below it, which forget their own
    /// views.
    ApplyViewBelow { folder: VPath, view: FolderView },
}

/// A window that asks before something happens that cannot be taken back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirm {
    pub purpose: ConfirmPurpose,
    pub title: String,
    /// What will happen, in full.
    pub message: String,
    /// The items it happens to, when there are some.
    pub detail: String,
    /// The items it happens to, one whole path a line, for a list; empty for none.
    pub lines: Vec<String>,
    /// What it asks, after the list; empty when the message asks it.
    pub question: String,
    /// The label of the button that does it.
    pub ok: String,
    /// Whether it destroys something: then Enter does not confirm, only the button does.
    pub danger: bool,
}

impl Confirm {
    /// The question before `paths` are deleted for good.
    pub fn delete_permanently(paths: Vec<VPath>, why: DeleteWhy) -> Confirm {
        let count = paths.len();
        let message = match why {
            DeleteWhy::Asked => words::delete_permanently(count),
            DeleteWhy::NoTrash => words::trash_unavailable(count),
            DeleteWhy::TrashRefused => words::trash_refused(count),
        };
        Confirm {
            title: words::delete_permanently_title().to_owned(),
            message,
            detail: words::names_line(&paths),
            lines: Vec::new(),
            question: String::new(),
            ok: words::delete_permanently_button().to_owned(),
            danger: true,
            purpose: ConfirmPurpose::DeletePermanently(paths),
        }
    }

    /// The question before `copies` are deleted for good: the copies an undo of the copy
    /// `record` could not send to the trash, whose originals (`originals`, one for each copy)
    /// are as they were. It lists every copy by its whole path.
    pub fn delete_copies(copies: Vec<VPath>, originals: Vec<VPath>, record: UndoRecord) -> Confirm {
        Confirm {
            title: words::delete_permanently_title().to_owned(),
            message: words::copies_refused().to_owned(),
            detail: String::new(),
            lines: copies.iter().map(ToString::to_string).collect(),
            question: words::delete_copies(copies.len()),
            ok: words::delete_permanently_button().to_owned(),
            danger: true,
            purpose: ConfirmPurpose::DeleteCopies {
                copies,
                originals,
                record: Box::new(record),
            },
        }
    }

    /// The question before the app ends while `running` jobs run.
    pub fn quit(running: usize) -> Confirm {
        Confirm {
            purpose: ConfirmPurpose::Quit,
            title: words::quit_title().to_owned(),
            message: words::quit_with_jobs(running),
            detail: String::new(),
            lines: Vec::new(),
            question: String::new(),
            ok: words::quit_button().to_owned(),
            danger: false,
        }
    }

    /// The question before 하위 폴더에 적용 gives `view` to `folder`, called `name`, and the
    /// folders below it, of which `below` have views of their own that they would forget.
    pub fn apply_view_below(folder: VPath, view: FolderView, name: &str, below: usize) -> Confirm {
        Confirm {
            purpose: ConfirmPurpose::ApplyViewBelow { folder, view },
            title: words::view_apply_title().to_owned(),
            message: words::view_apply_below(name, below),
            detail: String::new(),
            lines: Vec::new(),
            question: String::new(),
            ok: words::view_apply_button().to_owned(),
            danger: false,
        }
    }

    /// The job the confirmation asks for, if it asks for one. The copies an undo could not
    /// trash give none: they go only once a worker has found them as they were once more
    /// ([`AppState::copies_checked`](crate::state::AppState::copies_checked)), so a yes alone
    /// never deletes them.
    pub fn job(&self) -> Option<JobSpec> {
        match &self.purpose {
            ConfirmPurpose::DeletePermanently(paths) => Some(permanent_delete(paths.clone())),
            ConfirmPurpose::DeleteCopies { .. }
            | ConfirmPurpose::Quit
            | ConfirmPurpose::ApplyViewBelow { .. } => None,
        }
    }
}

/// The job that deletes `paths` for good, going on past an item that fails.
pub fn permanent_delete(paths: Vec<VPath>) -> JobSpec {
    JobSpec::Delete(DeleteSpec {
        paths,
        method: DeleteMethod::Permanent,
        errors: ErrorPolicy::Continue,
    })
}

/// The items a job could not process, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failures {
    pub title: String,
    pub lines: Vec<String>,
}

/// The jobs the last run of the app left unfinished, still to be run again or dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovery {
    pub jobs: Vec<IncompleteJob>,
}

/// A conflict question of a job: the name of an item is taken at the target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictQuestion {
    pub id: QuestionId,
    pub job: JobId,
    pub conflict: Box<Conflict>,
    /// The items the job has still to do after this one.
    pub items_left: u64,
    /// What the job does with the items (a copy or a move), as far as the window knows it.
    pub work: JobWork,
}

/// What the user chose in the conflict window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    Overwrite,
    Skip,
    Rename,
}

impl ConflictQuestion {
    /// Whether the overwrite choice is offered: a file and a folder never replace each other,
    /// and two folders merge without a question.
    pub fn can_overwrite(&self) -> bool {
        self.conflict.incoming.kind != EntryKind::Dir
            && self.conflict.existing.kind != EntryKind::Dir
    }

    /// Whether the item that would come was changed later than the one that is there (by more
    /// than the clock tolerance of the jobs), and the other way round.
    pub fn newer(&self) -> (bool, bool) {
        let (Some(incoming), Some(existing)) = (
            self.conflict.incoming.modified,
            self.conflict.existing.modified,
        ) else {
            return (false, false);
        };
        let after = |a: std::time::SystemTime, b: std::time::SystemTime| {
            b.checked_add(MTIME_TOLERANCE)
                .is_some_and(|limit| a > limit)
        };
        (after(incoming, existing), after(existing, incoming))
    }

    /// The first numbered name a rename gives the incoming item.
    pub fn numbered_name(&self) -> String {
        let name = self
            .conflict
            .target
            .file_name()
            .map(std::ffi::OsStr::to_os_string)
            .unwrap_or_default();
        let is_dir = self.conflict.incoming.kind == EntryKind::Dir;
        numbered_name(&name, 2, is_dir, Os::CURRENT)
            .to_string_lossy()
            .into_owned()
    }

    /// The answer for `choice`, for this conflict or (`apply_all`) every later one of the job
    /// too; `None` for an overwrite that is not offered.
    pub fn answer(&self, choice: ConflictChoice, apply_all: bool) -> Option<ConflictAnswer> {
        let action = match choice {
            ConflictChoice::Overwrite if !self.can_overwrite() => return None,
            ConflictChoice::Overwrite => ConflictAction::Overwrite,
            ConflictChoice::Skip => ConflictAction::Skip,
            ConflictChoice::Rename => ConflictAction::Rename,
        };
        Some(ConflictAnswer {
            action,
            apply_to_all: apply_all,
        })
    }
}

/// The pre-flight question of a transfer: problems found on the target before anything was
/// written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreflightQuestion {
    pub id: QuestionId,
    pub job: JobId,
    pub issues: Vec<Issue>,
}

impl PreflightQuestion {
    /// The lines of the window: each item and what its problem means.
    pub fn lines(&self) -> Vec<String> {
        self.issues
            .iter()
            .map(|issue| words::preflight_line(&issue.target, &issue.kind))
            .collect()
    }

    /// Whether an item cannot be written at all, which makes "skip the blocked items" a choice.
    /// Too little space is no such item: the engine leaves nothing out for it, so the choice
    /// would do what "go on" does.
    pub fn has_blocked(&self) -> bool {
        self.issues.iter().any(|issue| {
            issue.kind.severity() == Severity::Blocking
                && !matches!(issue.kind, IssueKind::NotEnoughSpace { .. })
        })
    }

    /// The answer for the button `choice`: 0 go on as it is, 1 leave out the blocked items,
    /// anything else cancel.
    pub fn answer(choice: i32) -> PreflightAnswer {
        match choice {
            0 => PreflightAnswer::Proceed,
            1 => PreflightAnswer::SkipBlocked,
            _ => PreflightAnswer::Cancel,
        }
    }
}

#[cfg(test)]
mod tests {
    use rf_core::Entry;

    use super::*;

    fn path(name: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(name)).expect("a local path")
    }

    #[test]
    fn rename_selects_the_stem() {
        assert_eq!(stem_end("설계안.pdf", false), "설계안".len());
        assert_eq!(stem_end("a.tar.gz", false), "a.tar".len());
        assert_eq!(stem_end(".bashrc", false), ".bashrc".len());
        assert_eq!(stem_end("README", false), 6);
        assert_eq!(stem_end("photos.2026", true), "photos.2026".len());

        let dialog = NameDialog::rename(path("설계안.pdf"), false, 0).expect("a name");
        assert_eq!(dialog.title(), "이름 바꾸기");
        assert_eq!(dialog.initial, "설계안.pdf");
        assert_eq!(&dialog.initial[..dialog.select_end], "설계안");
        // The name as it is, or nothing at all, asks for nothing.
        assert_eq!(dialog.check(), NameCheck::Unchanged);
        assert_eq!(dialog.job(), Ok(None));
    }

    #[test]
    fn names_are_checked_while_typed() {
        let mut dialog = NameDialog::new_folder(path("parent"), 1);
        assert_eq!(dialog.title(), "새 폴더");
        assert_eq!(dialog.text, "새 폴더");
        assert_eq!(dialog.check(), NameCheck::Ready);
        dialog.text = "   ".to_owned();
        assert_eq!(dialog.check(), NameCheck::Empty);
        dialog.text = "a/b".to_owned();
        assert_eq!(
            dialog.check(),
            NameCheck::Invalid("이름에 ‘/’ 문자를 쓸 수 없습니다".to_owned())
        );
        assert!(dialog.job().is_err());
        dialog.text = "..".to_owned();
        assert!(matches!(dialog.check(), NameCheck::Invalid(_)));

        // Windows refuses more characters; other systems take them.
        assert!(name_problem("a:b", Os::Windows).is_some());
        assert!(name_problem("a?b", Os::Windows).is_some());
        assert_eq!(name_problem("a:b", Os::Other), None);
        assert_eq!(name_problem("a:b", Os::MacOs), None);
        assert!(name_problem("a\\b", Os::Windows).is_some());
        assert!(name_problem("a\u{1}b", Os::Windows).is_some());
        assert_eq!(name_problem("보고서 (최종).txt", Os::Windows), None);

        dialog.text = "자료".to_owned();
        assert_eq!(
            dialog.job(),
            Ok(Some((
                JobSpec::Create {
                    parent: path("parent"),
                    name: OsString::from("자료"),
                    kind: CreateKind::Dir,
                },
                Some(1)
            )))
        );
        // The picker's folder belongs to no pane.
        let mut picked = NameDialog::pick_folder(path("parent"));
        assert_eq!(picked.title(), "새 폴더");
        picked.text = "자료".to_owned();
        assert_eq!(
            picked.job(),
            Ok(Some((
                JobSpec::Create {
                    parent: path("parent"),
                    name: OsString::from("자료"),
                    kind: CreateKind::Dir,
                },
                None
            )))
        );
    }

    #[test]
    fn a_rename_stays_in_its_folder() {
        let mut dialog = NameDialog::rename(path("a.txt"), false, 0).expect("a name");
        dialog.text = "b.txt".to_owned();
        assert_eq!(
            dialog.job(),
            Ok(Some((
                JobSpec::Rename {
                    from: path("a.txt"),
                    to: path("b.txt"),
                },
                Some(0)
            )))
        );
    }

    #[test]
    fn a_permanent_delete_says_what_happens() {
        let paths = vec![path("a.txt"), path("b"), path("c"), path("d")];
        let asked = Confirm::delete_permanently(paths.clone(), DeleteWhy::Asked);
        assert!(asked.danger);
        assert_eq!(asked.ok, "영구 삭제");
        assert_eq!(
            asked.message,
            "항목 4개를 영구 삭제할까요?\n휴지통을 거치지 않으며 되돌릴 수 없습니다."
        );
        assert_eq!(asked.detail, "‘a.txt’, ‘b’, ‘c’ 외 1개");
        let no_trash = Confirm::delete_permanently(paths[..1].to_vec(), DeleteWhy::NoTrash);
        assert_eq!(
            no_trash.message,
            "휴지통을 쓸 수 없는 위치입니다.\n항목 1개를 영구 삭제할까요? 되돌릴 수 없습니다."
        );
        assert_eq!(no_trash.detail, "‘a.txt’");
        assert_eq!(
            no_trash.job(),
            Some(JobSpec::Delete(DeleteSpec {
                paths: vec![path("a.txt")],
                method: DeleteMethod::Permanent,
                errors: ErrorPolicy::Continue,
            }))
        );
        let refused = Confirm::delete_permanently(paths, DeleteWhy::TrashRefused);
        assert!(refused.message.contains("휴지통이 받지 않은 항목이 4개"));
        assert!(refused.message.ends_with("되돌릴 수 없습니다."));

        // The copies an undo could not trash go only after a worker has looked at them once
        // more: their confirmation submits no job itself.
        let record = UndoRecord::Copied {
            created: Vec::new(),
            overwritten: 0,
            at: None,
        };
        let copies = Confirm::delete_copies(vec![path("a.txt")], vec![path("b.txt")], record);
        assert!(copies.danger);
        assert_eq!(copies.job(), None);

        let quit = Confirm::quit(2);
        assert!(!quit.danger);
        assert_eq!(quit.job(), None);
        assert_eq!(
            quit.message,
            "진행 중인 작업 2개가 있습니다.\n끝내면 다음 실행 때 이어서 할 수 있습니다."
        );
    }

    fn conflict(incoming: EntryKind, existing: EntryKind) -> ConflictQuestion {
        ConflictQuestion {
            id: 1,
            job: JobId(9),
            conflict: Box::new(Conflict {
                source: path("src/설계안.pdf"),
                target: path("설계안.pdf"),
                incoming: Entry::new("설계안.pdf", incoming),
                existing: Entry::new("설계안.pdf", existing),
            }),
            items_left: 0,
            work: JobWork::Copy,
        }
    }

    #[test]
    fn a_file_never_overwrites_a_folder() {
        let files = conflict(EntryKind::File, EntryKind::File);
        assert!(files.can_overwrite());
        assert_eq!(
            files.answer(ConflictChoice::Overwrite, true),
            Some(ConflictAnswer::all(ConflictAction::Overwrite))
        );
        assert_eq!(
            files.answer(ConflictChoice::Rename, false),
            Some(ConflictAnswer::once(ConflictAction::Rename))
        );
        let mixed = conflict(EntryKind::File, EntryKind::Dir);
        assert!(!mixed.can_overwrite());
        assert_eq!(mixed.answer(ConflictChoice::Overwrite, false), None);
        assert_eq!(
            mixed.answer(ConflictChoice::Skip, false),
            Some(ConflictAnswer::once(ConflictAction::Skip))
        );
        assert!(!conflict(EntryKind::Dir, EntryKind::File).can_overwrite());
    }

    #[test]
    fn the_newer_side_is_marked() {
        let mut question = conflict(EntryKind::File, EntryKind::File);
        assert_eq!(question.newer(), (false, false));
        let now = std::time::SystemTime::now();
        question.conflict.incoming.modified = Some(now);
        question.conflict.existing.modified = Some(now - std::time::Duration::from_secs(60));
        assert_eq!(question.newer(), (true, false));
        // Within the clock tolerance neither is newer.
        question.conflict.existing.modified = Some(now - std::time::Duration::from_secs(1));
        assert_eq!(question.newer(), (false, false));
        question.conflict.existing.modified = Some(now + std::time::Duration::from_secs(60));
        assert_eq!(question.newer(), (false, true));
        let numbered = question.numbered_name();
        assert!(
            numbered == "설계안 (2).pdf" || numbered == "설계안 2.pdf",
            "{numbered}"
        );
    }

    #[test]
    fn preflight_answers_follow_the_buttons() {
        assert_eq!(PreflightQuestion::answer(0), PreflightAnswer::Proceed);
        assert_eq!(PreflightQuestion::answer(1), PreflightAnswer::SkipBlocked);
        assert_eq!(PreflightQuestion::answer(2), PreflightAnswer::Cancel);
        let question = PreflightQuestion {
            id: 1,
            job: JobId(1),
            issues: vec![Issue {
                target: path("CON"),
                kind: IssueKind::ReservedName,
            }],
        };
        assert!(!question.has_blocked());
        // Too little space lets the job start; leaving out "blocked" items changes nothing.
        let space = PreflightQuestion {
            issues: vec![Issue {
                target: path("백업"),
                kind: IssueKind::NotEnoughSpace {
                    needed: 2048,
                    available: 1024,
                },
            }],
            ..question.clone()
        };
        assert!(!space.has_blocked());
        let long = PreflightQuestion {
            issues: vec![Issue {
                target: path("a"),
                kind: IssueKind::NameTooLong { len: 300, max: 255 },
            }],
            ..question.clone()
        };
        assert!(long.has_blocked());
        assert_eq!(
            question.lines(),
            [
                "CON: Windows가 장치 이름으로 쓰는 이름입니다(CON, NUL 등).\n그대로 두면 Windows에서 열기 어려울 수 있습니다"
            ]
        );
    }
}
