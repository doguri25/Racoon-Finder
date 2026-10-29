//! The windows that open over the main window, as plain data: the name of a new folder or of a
//! rename, a confirmation, the failed items of a job, the jobs the last run left unfinished,
//! and the questions of the jobs. [`AppState`](crate::state::AppState) keeps the one that
//! shows and the ones waiting behind it; `dialogs.slint` and `conflict-dialog.slint` draw them.
//!
//! Nothing here touches a file: a dialog that is answered gives the job to submit, and the
//! state hands it to the engine.

use std::ffi::OsString;

use rf_core::{EntryKind, Os, VPath};
use rf_jobs::{
    Conflict, ConflictAction, ConflictAnswer, CreateKind, DeleteMethod, DeleteSpec, ErrorPolicy,
    IncompleteJob, Issue, JobId, JobSpec, MTIME_TOLERANCE, PreflightAnswer, Severity,
    numbered_name,
};

use crate::resolver::QuestionId;
use crate::words;

/// A window over the main window. While one shows, the main window takes no keys or clicks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Modal {
    Name(NameDialog),
    Confirm(Confirm),
    Failures(Failures),
    Recovery(Recovery),
    Conflict(ConflictQuestion),
    Preflight(PreflightQuestion),
}

impl Modal {
    /// The job question this window asks, if it asks one.
    pub fn question(&self) -> Option<QuestionId> {
        match self {
            Modal::Conflict(question) => Some(question.id),
            Modal::Preflight(question) => Some(question.id),
            Modal::Name(_) | Modal::Confirm(_) | Modal::Failures(_) | Modal::Recovery(_) => None,
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
            NamePurpose::NewFolder { .. } => words::new_folder_title(),
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

    /// The job the field asks for, with the pane that asked; `Err` with the warning when the
    /// name cannot be used, `Ok(None)` when there is nothing to do (an empty or unchanged
    /// name).
    pub fn job(&self) -> Result<Option<(JobSpec, usize)>, String> {
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
                *origin,
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
                    *origin,
                )))
            }
        }
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
    /// End the app while jobs run.
    Quit,
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
            ok: words::delete_permanently_button().to_owned(),
            danger: true,
            purpose: ConfirmPurpose::DeletePermanently(paths),
        }
    }

    /// The question before the app ends while `running` jobs run.
    pub fn quit(running: usize) -> Confirm {
        Confirm {
            purpose: ConfirmPurpose::Quit,
            title: words::quit_title().to_owned(),
            message: words::quit_with_jobs(running),
            detail: String::new(),
            ok: words::quit_button().to_owned(),
            danger: false,
        }
    }

    /// The job the confirmation asks for, if it asks for one.
    pub fn job(&self) -> Option<JobSpec> {
        match &self.purpose {
            ConfirmPurpose::DeletePermanently(paths) => Some(JobSpec::Delete(DeleteSpec {
                paths: paths.clone(),
                method: DeleteMethod::Permanent,
                errors: ErrorPolicy::Continue,
            })),
            ConfirmPurpose::Quit => None,
        }
    }
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
    pub fn has_blocked(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.kind.severity() == Severity::Blocking)
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
                1
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
                0
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
            "항목 4개를 영구 삭제할까요? 휴지통을 거치지 않으며 되돌릴 수 없습니다."
        );
        assert_eq!(asked.detail, "‘a.txt’, ‘b’, ‘c’ 외 1개");
        let no_trash = Confirm::delete_permanently(paths[..1].to_vec(), DeleteWhy::NoTrash);
        assert_eq!(
            no_trash.message,
            "휴지통을 쓸 수 없는 위치입니다. 항목 1개를 영구 삭제할까요? 되돌릴 수 없습니다."
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

        let quit = Confirm::quit(2);
        assert!(!quit.danger);
        assert_eq!(quit.job(), None);
        assert_eq!(
            quit.message,
            "진행 중인 작업 2개가 있습니다. 끝내면 다음 실행 때 이어서 할 수 있습니다."
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
                kind: rf_jobs::IssueKind::ReservedName,
            }],
        };
        assert!(!question.has_blocked());
        assert_eq!(
            question.lines(),
            [
                "CON: Windows가 장치 이름으로 쓰는 이름입니다(CON, NUL 등). 그대로 두면 Windows에서 열기 어려울 수 있습니다"
            ]
        );
    }
}
