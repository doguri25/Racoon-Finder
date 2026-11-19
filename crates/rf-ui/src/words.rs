//! The words the app shows for errors, refusals, jobs, the status bar and the sidebar
//! (I18N-02), in the language of the run: each function asks [`crate::i18n`] for its message
//! (`crates/rf-ui/i18n/<language>/*.ftl`), and the tests, which never choose a language, read
//! them in Korean.
//!
//! The `Display` text of `rf_core::Error` and of the job types is English and for the log; the
//! screens show these words instead. They say what happened to the user's item, not how the
//! engine works.

use std::fmt::Write as _;
use std::time::{Duration, SystemTime};

use fluent_bundle::FluentValue;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use rf_core::{EntryKind, ErrorKind, Os, VPath};
use rf_jobs::{
    Compared, ConflictPolicy, CopyFound, CreateKind, DeleteMethod, FailureReason, IssueKind,
    ItemFailure, JobKind, JobSpec, JobState, Outcome, Severity, TransferMode, UndoRecord,
};
use rf_platform::dnd::DropEffect;
use rf_platform::shell::App;
use rf_platform::{KnownFolder, LinkKind};
use rf_vfs::ShellPlace;

use crate::format::{Rounding, format_size, format_size_rounded, short_path};
use crate::hashing::HashFailure;
use crate::i18n::{Arg, OS_ARG, count, number, os_arg, t, t_static};
use crate::jobs::JobRow;
use crate::pattern::{MAX_PATTERN_CHARS, PatternError};
use crate::pick::Validity;
use crate::state::drop_rule::{DropChoice, Refusal};
use crate::state::{ClipboardFailure, Column};

/// How many characters a path takes at most in a row of a recovered job, beside its buttons:
/// the middle of a longer one is left out ([`short_path`]), so that the end of the path stays
/// in view, and the words after it. The window of failed items shows whole paths instead
/// ([`failure_line`]).
const ROW_PLACE_CHARS: usize = 24;

/// How many characters a path takes at most in a notice, which is one line.
const NOTICE_PLACE_CHARS: usize = 60;

/// What a job does, as the user sees it: a [`JobKind`] with a delete told apart by where the
/// items go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JobWork {
    Copy,
    Move,
    /// A delete that sends the items to the trash.
    Trash,
    /// A delete for good.
    Delete,
    Rename,
    Create,
    /// Making links (`JobSpec::MakeLink`).
    Link,
    /// Putting items back from the trash (`JobSpec::RestoreFromTrash`, NAV-16).
    Restore,
    /// An undo (`JobEngine::undo`).
    Undo,
    /// A redo (`JobEngine::redo`), which is an undo of the undo. Its spec is an undo spec too,
    /// so only the caller that asked for the redo can say so.
    Redo,
    /// A kind of job this version has no words for.
    Other,
}

impl JobWork {
    /// The work `spec` asks for. An undo spec gives [`JobWork::Undo`], also for a redo: the
    /// caller that called `JobEngine::redo` keeps the job's id and sets [`JobWork::Redo`] for
    /// it itself.
    pub fn of(spec: &JobSpec) -> JobWork {
        if let JobSpec::Delete(delete) = spec {
            return match delete.method {
                DeleteMethod::Trash => JobWork::Trash,
                DeleteMethod::Permanent => JobWork::Delete,
            };
        }
        match spec.kind() {
            JobKind::Copy => JobWork::Copy,
            JobKind::Move => JobWork::Move,
            JobKind::Delete => JobWork::Delete,
            JobKind::Rename => JobWork::Rename,
            JobKind::Create => JobWork::Create,
            JobKind::Link => JobWork::Link,
            JobKind::Undo => JobWork::Undo,
            JobKind::Restore => JobWork::Restore,
            _ => JobWork::Other,
        }
    }

    /// The work as the messages' `$work` selects it: `copy`, `move`, `trash`, `delete`,
    /// `rename`, `create`, `link`, `restore`, `undo`, `redo`, `other`.
    fn arg(self) -> FluentValue<'static> {
        FluentValue::from(match self {
            JobWork::Copy => "copy",
            JobWork::Move => "move",
            JobWork::Trash => "trash",
            JobWork::Delete => "delete",
            JobWork::Rename => "rename",
            JobWork::Create => "create",
            JobWork::Link => "link",
            JobWork::Restore => "restore",
            JobWork::Undo => "undo",
            JobWork::Redo => "redo",
            JobWork::Other => "other",
        })
    }

    /// The message `key` about this work, with `args`: a sentence the message makes whole in
    /// each language from `$work`.
    fn says(self, key: &str, args: &[Arg<'_>]) -> String {
        let mut all = Vec::with_capacity(args.len() + 1);
        all.push(("work", self.arg()));
        all.extend(args.iter().cloned());
        t(key, &all)
    }

    /// The name of the work on its own, as a recovered job's line starts: `복사`, `이동`.
    pub fn noun(self) -> &'static str {
        t_static(match self {
            JobWork::Copy => "job-noun-copy",
            JobWork::Move => "job-noun-move",
            JobWork::Trash => "job-noun-trash",
            JobWork::Delete => "job-noun-delete",
            JobWork::Rename => "job-noun-rename",
            JobWork::Create => "job-noun-create",
            JobWork::Link => "job-noun-link",
            JobWork::Restore => "job-noun-restore",
            JobWork::Undo => "job-noun-undo",
            JobWork::Redo => "job-noun-redo",
            JobWork::Other => "job-noun-other",
        })
    }
}

/// What went wrong, for an error of the system or of a provider (an item with no
/// [`FailureReason`]).
pub fn error_kind(kind: ErrorKind) -> &'static str {
    t_static(match kind {
        ErrorKind::NotFound => "error-not-found",
        ErrorKind::PermissionDenied => "error-permission-denied",
        ErrorKind::AlreadyExists => "error-already-exists",
        ErrorKind::InvalidPath => "error-invalid-path",
        ErrorKind::NotADirectory => "error-not-a-directory",
        ErrorKind::IsADirectory => "error-is-a-directory",
        ErrorKind::DirectoryNotEmpty => "error-directory-not-empty",
        ErrorKind::StorageFull => "error-storage-full",
        ErrorKind::ReadOnly => "error-read-only",
        ErrorKind::CrossesDevices => "error-crosses-devices",
        ErrorKind::Busy => "error-busy",
        ErrorKind::TimedOut => "error-timed-out",
        ErrorKind::Interrupted => "error-interrupted",
        ErrorKind::FileTooLarge => "error-file-too-large",
        ErrorKind::Unsupported => "error-unsupported",
        ErrorKind::Cancelled => "error-cancelled",
        // `Other`, and any kind added after this was written.
        _ => "error-other",
    })
}

/// The message `key` with the [`error_kind`] of `kind` as its `$reason`.
fn with_reason(key: &str, kind: ErrorKind) -> String {
    t(key, &[("reason", error_kind(kind).into())])
}

/// The notice for an item that could not be opened or a folder that could not be shown:
/// `열 수 없습니다: 권한이 없습니다`.
pub fn cannot_open(kind: ErrorKind) -> String {
    with_reason("error-cannot-open", kind)
}

/// The notice for the address of a remote place typed into a path field (`sftp://…`): the app
/// opens local folders alone for now.
pub fn remote_location() -> &'static str {
    t_static("notice-remote-location")
}

/// What the list of a folder's subfolders under a `›` says while a worker lists the folder.
pub fn listing_folders() -> &'static str {
    t_static("tree-loading")
}

/// What the list of a folder's subfolders says when the folder has none (that the panes show).
pub fn no_subfolders() -> &'static str {
    t_static("tree-no-subfolders")
}

/// Under the folders of a `›`'s list when the listing left links unexamined (past the most it
/// follows) that may lead to more folders, and no folder was left out otherwise.
pub fn links_unexamined() -> &'static str {
    t_static("tree-links-unexamined")
}

/// Under the names a list of the address bar shows: how many more folders there are.
pub fn more_folders(n: usize) -> String {
    t("tree-more-folders", &[("count", count(n))])
}

/// The notice of a drop the window did not make: the rows changed under the pointer (a
/// folder's watch) since it showed where the items would go, so they would go elsewhere.
pub fn drop_moved() -> &'static str {
    t_static("drop-moved")
}

/// The notice of a command chosen in Windows' own menu that the app did not do: the items a
/// command acts on (or the folder of the pane) are no longer the ones the menu was for.
pub fn system_menu_moved() -> &'static str {
    t_static("notice-system-menu-moved")
}

/// Where a pane went when its folder was gone, as the messages' `$to` selects it: `parent` for
/// the nearest folder above it that is left, `home` for the home folder.
fn went_to(to_parent: bool) -> FluentValue<'static> {
    FluentValue::from(if to_parent { "parent" } else { "home" })
}

/// The notice when the folder `name` a pane showed is gone and the pane went to the nearest
/// folder above it that is left (`to_parent`), or home when none is.
pub fn folder_vanished(name: &str, to_parent: bool) -> String {
    t(
        "notice-folder-gone",
        &[("name", name.into()), ("to", went_to(to_parent))],
    )
}

/// What the head of `column` of 자세히 says (the 보기 menu's 열 and 정렬 say it too).
pub fn column_title(column: Column) -> &'static str {
    t_static(match column {
        Column::Name => "column-name",
        Column::Size => "column-size",
        Column::Modified => "column-modified",
        Column::Created => "column-created",
        Column::Kind => "column-kind",
        Column::Extension => "column-extension",
        Column::Attributes => "column-attributes",
        Column::Accessed => "column-accessed",
        Column::Path => "column-path",
        Column::Original => "column-original",
        Column::Deleted => "column-deleted",
    })
}

/// The notice when a view of 내 PC is asked for that it does not have: it shows its drives as
/// tiles only.
pub fn computer_view_fixed() -> &'static str {
    t_static("notice-computer-view-fixed")
}

/// The notice after 하위 폴더에 적용 in the folder `name`.
pub fn view_applied_below(name: &str) -> String {
    t("view-applied-below", &[("name", name.into())])
}

/// The title of the question before 하위 폴더에 적용 makes folders below forget their views.
pub fn view_apply_title() -> &'static str {
    t_static("view-apply-title")
}

/// The question before 하위 폴더에 적용 in the folder `name`, of whose subfolders `n` have
/// views of their own.
pub fn view_apply_below(name: &str, n: usize) -> String {
    t(
        "view-apply-below",
        &[("name", name.into()), ("count", count(n))],
    )
}

/// The label of the button that applies a view below.
pub fn view_apply_button() -> &'static str {
    t_static("view-apply-button")
}

/// The notice after 기본값으로 되돌리기 in the folder `name`.
pub fn view_reset(name: &str) -> String {
    t("view-reset", &[("name", name.into())])
}

/// The notice when a folder's view cannot be remembered (its path is too long to keep).
pub fn view_not_kept() -> &'static str {
    t_static("view-not-kept")
}

/// The notice when the folder `name` a pane showed is gone, the pane went to the nearest folder
/// above it that is left (`to_parent`) or home, and navigation sync, which waited to open
/// folders there, turned off: one notice for both, as the window shows one at a time.
pub fn folder_vanished_sync_off(name: &str, to_parent: bool) -> String {
    t(
        "notice-folder-gone-sync-off",
        &[("name", name.into()), ("to", went_to(to_parent))],
    )
}

/// The notice of 같은 확장자 선택 on a row whose name has no extension (a folder, `README`).
pub fn no_extension(name: &str) -> String {
    t("select-no-extension", &[("name", name.into())])
}

/// The notice when a long folder could not be put in its new order: the rows keep the one they
/// had.
pub fn sort_failed() -> &'static str {
    t_static("notice-sort-failed")
}

/// What the pattern window shows when its worker could not count the rows: 선택 matches them
/// when it is pressed.
pub fn pattern_uncounted() -> &'static str {
    t_static("pattern-uncounted")
}

/// How many rows the pattern window's pattern matches.
pub fn pattern_count(n: usize) -> String {
    t("pattern-count", &[("count", count(n))])
}

/// What the pattern window shows while a worker matches the names, before its first count.
pub fn pattern_counting() -> &'static str {
    t_static("pattern-counting")
}

/// Why the pattern window's text is no pattern.
pub fn pattern_problem(problem: PatternError) -> String {
    match problem {
        PatternError::TooLong => t("pattern-too-long", &[("max", count(MAX_PATTERN_CHARS))]),
        PatternError::Invalid => t_static("pattern-invalid").to_owned(),
        PatternError::TooComplex => t_static("pattern-too-complex").to_owned(),
    }
}

/// The notice when navigation sync is turned on: a move into a folder, up, back or forward in
/// one pane is made in the other one too.
pub fn sync_on() -> &'static str {
    t_static("sync-on")
}

/// The notice when navigation sync is turned off.
pub fn sync_off() -> &'static str {
    t_static("sync-off")
}

/// The notice when one pane went into its folder `name` and the other has no folder of that
/// name: sync is off.
pub fn sync_no_folder(name: &str) -> String {
    t("sync-no-folder", &[("name", name.into())])
}

/// The notice when one pane went up, back or forward and the other cannot (it is at a root,
/// or has nothing to go back or forward to): sync is off.
pub fn sync_cannot_follow() -> &'static str {
    t_static("sync-cannot-follow")
}

/// The notice when the panes were swapped while sync waited for the other pane's listing to
/// follow a move: what it would follow now is the pane with the keys, so sync is off.
pub fn sync_off_by_swap() -> &'static str {
    t_static("sync-off-by-swap")
}

/// The notice when a command for the other pane (the same folder there, navigation sync) is
/// asked for while one pane shows.
pub fn one_pane_only() -> &'static str {
    t_static("pane-one-only")
}

/// The notice when a pane's last tab is to go to the other pane: a pane keeps one tab.
pub fn last_tab_stays() -> &'static str {
    t_static("tab-last-stays")
}

/// The notice when a tab went to the other pane while one pane shows: it is in the hidden
/// pane, which the 보기 menu's 배치 ▸ 듀얼 패널 (F12) shows.
pub fn tab_moved_to_hidden() -> &'static str {
    t_static("tab-moved-to-hidden")
}

/// The notice when the folder the app was asked to open at its start is gone: the tabs of
/// the last run show instead. A long path loses its middle ([`short_path`]).
pub fn start_folder_gone(folder: &VPath) -> String {
    t(
        "notice-start-folder-gone",
        &[(
            "path",
            short_path(&folder.to_string(), NOTICE_PLACE_CHARS).into(),
        )],
    )
}

/// The notice when the system could not start the work behind part of the window (it is out
/// of threads or memory): a listing, a preview, a look at the clipboard, a worker of the app's
/// start. Each of them can be asked for again. "일", not "작업": no file job failed.
pub fn background_failed() -> &'static str {
    t_static("notice-background-failed")
}

/// The notice when the settings (tabs, theme, window size) could not be written: the next
/// start may not show them.
pub fn settings_not_saved() -> &'static str {
    t_static("settings-not-saved")
}

/// The notice at the start when the settings file is there but could not be read (or set
/// aside): the app starts with the defaults and leaves the file alone.
pub fn settings_unreadable() -> &'static str {
    t_static("settings-unreadable")
}

/// The space line of a drive in the sidebar: `128 GB 남음 / 512 GB`. The free space is
/// rounded down, so a full drive never reads as having room.
pub fn drive_space(available: u64, total: u64) -> String {
    t(
        "sidebar-drive-space",
        &[
            (
                "free",
                format_size_rounded(available, Rounding::Down).into(),
            ),
            ("total", format_size(total).into()),
        ],
    )
}

/// What a pane's status line has to tell (NAV-12).
#[derive(Clone, Copy, Debug)]
pub struct StatusFacts<'a> {
    /// The rows that show.
    pub items: usize,
    /// The selected rows, of every kind.
    pub selected: usize,
    /// The sizes of the selected files added up; `None` when no file is selected (a folder, a
    /// link has no size to add: nothing looks at what is inside a folder).
    pub selected_bytes: Option<u64>,
    /// The selected rows that are folders.
    pub selected_folders: usize,
    /// The toolbar's filter, while it hides rows.
    pub filter: Option<StatusFilter<'a>>,
    /// The bytes the user can still write on the folder's drive, once known.
    pub free: Option<u64>,
}

/// The toolbar's filter, as the status line tells of it.
#[derive(Clone, Copy, Debug)]
pub struct StatusFilter<'a> {
    pub text: &'a str,
    /// How many rows show, of `of`.
    pub shown: usize,
    pub of: usize,
}

/// The parts of a pane's status line, the most worth first: what is selected (`3개 선택`, `크기
/// 1.4 MB`, `+폴더 1개`), then `항목 9개`, then `거르기: *.jpg · 4/9` while a filter hides rows,
/// then `남은 공간 212 GB`. [`fit_status`] drops them from the right when the pane is narrow.
///
/// The size is the sum of the selected files; a folder is told apart (`+폴더`, or `폴더` when
/// no file is selected) and its size is never looked for. The room is rounded down like the
/// sidebar's [`drive_space`], so the two never show different numbers for the same volume.
pub fn status_parts(facts: &StatusFacts<'_>) -> Vec<String> {
    let mut parts = Vec::new();
    if facts.selected > 0 {
        parts.push(t("status-selected", &[("count", count(facts.selected))]));
        if let Some(bytes) = facts.selected_bytes {
            parts.push(t("status-size", &[("size", format_size(bytes).into())]));
        }
        if facts.selected_folders > 0 {
            let key = if facts.selected_bytes.is_some() {
                "status-plus-folders"
            } else {
                "status-folders"
            };
            parts.push(t(key, &[("count", count(facts.selected_folders))]));
        }
    }
    parts.push(t("status-items", &[("count", count(facts.items))]));
    if let Some(filter) = facts.filter {
        parts.push(t(
            "status-filter",
            &[
                ("filter", clip_end(filter.text, STATUS_FILTER_CELLS).into()),
                ("shown", count(filter.shown)),
                ("of", count(filter.of)),
            ],
        ));
    }
    if let Some(free) = facts.free {
        parts.push(t(
            "status-free",
            &[("free", format_size_rounded(free, Rounding::Down).into())],
        ));
    }
    parts
}

/// How many cells one character takes on a line: a Hangul syllable or another wide character
/// two, the rest one.
fn char_cells(c: char) -> usize {
    match c {
        '\u{1100}'..='\u{11FF}'
        | '\u{2E80}'..='\u{A4CF}'
        | '\u{AC00}'..='\u{D7A3}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FE30}'..='\u{FE6F}'
        | '\u{FF00}'..='\u{FF60}'
        | '\u{FFE0}'..='\u{FFE6}' => 2,
        _ => 1,
    }
}

/// How many cells `text` takes on a line: a Hangul syllable or another wide character two, the
/// rest one. The pane turns its width into cells by the size of its font.
pub fn text_cells(text: &str) -> usize {
    text.chars().map(char_cells).sum()
}

/// What stands between the parts of a status line.
const STATUS_SEPARATOR: &str = " · ";

/// `parts` joined with ` · `, fewer of them from the right until the line is at most `cells`
/// wide: a part is left out whole, never cut. The first part stays whatever the width.
pub fn fit_status<S: AsRef<str>>(parts: &[S], cells: usize) -> String {
    let separator = text_cells(STATUS_SEPARATOR);
    let mut line = String::new();
    let mut used = 0;
    for part in parts
        .iter()
        .map(AsRef::as_ref)
        .filter(|part| !part.is_empty())
    {
        let width = text_cells(part);
        if line.is_empty() {
            line.push_str(part);
            used = width;
        } else if used + separator + width <= cells {
            line.push_str(STATUS_SEPARATOR);
            line.push_str(part);
            used += separator + width;
        } else {
            break;
        }
    }
    line
}

/// The sidebar name of one of the user's well-known folders.
pub fn favorite(folder: KnownFolder) -> &'static str {
    t_static(match folder {
        KnownFolder::Home => "place-home",
        KnownFolder::Desktop => "place-desktop",
        KnownFolder::Documents => "place-documents",
        KnownFolder::Downloads => "place-downloads",
        KnownFolder::Pictures => "place-pictures",
        KnownFolder::Music => "place-music",
        KnownFolder::Videos => "place-videos",
    })
}

/// The sidebar name of the Windows drive `letter`: `로컬 디스크 (C:)` for the system's usual
/// drive, `드라이브 (D:)` for the others.
pub fn drive_letter(letter: char) -> String {
    let key = if letter == 'C' {
        "place-local-disk"
    } else {
        "place-drive"
    };
    t(key, &[("letter", letter.to_string().into())])
}

/// The sidebar name of the volume that holds `/` on macOS and Linux.
pub fn system_disk() -> &'static str {
    t_static("place-system-disk")
}

/// The name of a place of the shell, in the sidebar, a tab, the path row and the path field,
/// which takes it typed back: `휴지통`, `내 PC`.
pub fn shell_place(place: ShellPlace) -> &'static str {
    t_static(match place {
        ShellPlace::Trash => "place-trash",
        ShellPlace::Computer => "place-computer",
    })
}

/// Why a job refused an item, or part of one: one sentence for each rule of the jobs, or for
/// a name or size the target cannot hold, the [`issue_kind`] fact and what it meant for the
/// item (a rename or a create refuses every name with an issue, warnings too).
pub fn failure_reason(reason: &FailureReason) -> String {
    let key = match reason {
        FailureReason::IntoItself => "failure-into-itself",
        FailureReason::NoTargetFolder => "failure-no-target-folder",
        FailureReason::TargetNotAFolder => "failure-target-not-a-folder",
        FailureReason::TooDeep => "failure-too-deep",
        FailureReason::LinkLoop => "failure-link-loop",
        FailureReason::TooManyLinks => "failure-too-many-links",
        FailureReason::LinkChanged => "failure-link-changed",
        FailureReason::AbsoluteLink => "failure-absolute-link",
        FailureReason::NotARegularFile => "failure-not-a-regular-file",
        FailureReason::SourceCutShort => "failure-source-cut-short",
        FailureReason::KindMismatch => "failure-kind-mismatch",
        FailureReason::NoFreeName => "failure-no-free-name",
        FailureReason::OntoItself => "failure-onto-itself",
        FailureReason::ChangedWhileMoved => "failure-changed-while-moved",
        FailureReason::KeptAsTargetFolder => "failure-kept-as-target-folder",
        FailureReason::CannotTellOwnFolder => "failure-cannot-tell-own-folder",
        FailureReason::OtherVolume => "failure-other-volume",
        FailureReason::TrashRefused => "failure-trash-refused",
        FailureReason::DeletedNotTrashed => "failure-deleted-not-trashed",
        FailureReason::ChangedWhileDeleted => "failure-changed-while-deleted",
        FailureReason::Overwritten => "failure-overwritten",
        FailureReason::ChangedSinceCopy => "failure-changed-since-copy",
        // A copy as it was is never kept; the words of one that cannot be told do for it.
        FailureReason::CopyKept(found) => {
            return copy_kept(CopyKept::of(*found).unwrap_or(CopyKept::CopyUnknown)).to_owned();
        }
        // As the engine's message says, it may have been: a change of the folder by anyone looks
        // the same.
        FailureReason::CopyPartlyDeleted => "failure-copy-partly-deleted",
        FailureReason::ChangedSinceCreated => "failure-changed-since-created",
        FailureReason::NotEmpty => "failure-not-empty",
        FailureReason::TrashLocationUnknown => "failure-trash-location-unknown",
        FailureReason::AcrossLocations => "failure-across-locations",
        FailureReason::LinkNotLocal => "failure-link-not-local",
        FailureReason::LinkNeedsPrivilege => "failure-link-needs-privilege",
        FailureReason::LinkTargetMissing => "failure-link-target-missing",
        FailureReason::ChangedSinceLinked => "failure-changed-since-linked",
        FailureReason::TempLeftBehind => "failure-temp-left-behind",
        FailureReason::TrashItemChanged => "failure-trash-item-changed",
        FailureReason::OriginalFolderGone => "failure-original-folder-gone",
        FailureReason::RestoreNeverReplaces => "failure-restore-never-replaces",
        FailureReason::Issue(issue) => {
            return issue_and(issue, refused_because(issue));
        }
        _ => "failure-unknown",
    };
    t_static(key).to_owned()
}

/// The [`issue_kind`] fact of `issue`, then what it meant (`consequence`), as one line.
fn issue_and(issue: &IssueKind, consequence: &str) -> String {
    t(
        "issue-and-consequence",
        &[
            ("issue", issue_kind(issue).into()),
            ("consequence", consequence.into()),
        ],
    )
}

/// What keeps an item from fitting its target as it is, as a fact: the character, the
/// sizes or the other name involved. What follows from it depends on the job, so the words
/// the user reads come from [`failure_reason`] (the item was refused) or [`preflight_issue`]
/// (before a transfer).
pub fn issue_kind(issue: &IssueKind) -> String {
    let sized = |key: &str, up: (&str, u64), down: (&str, u64)| {
        t(
            key,
            &[
                (up.0, format_size_rounded(up.1, Rounding::Up).into()),
                (down.0, format_size_rounded(down.1, Rounding::Down).into()),
            ],
        )
    };
    match issue {
        IssueKind::InvalidCharacter(c) => t(
            "issue-invalid-character",
            &[("char", quote_char(*c).into())],
        ),
        IssueKind::ReservedName => t_static("issue-reserved-name").to_owned(),
        IssueKind::TrailingDotOrSpace => t_static("issue-trailing-dot-or-space").to_owned(),
        // `len` and `max` count UTF-8 bytes or UTF-16 units, and a path may be the temporary
        // one: no number would mean much to the user.
        IssueKind::NameTooLong { .. } => t_static("issue-name-too-long").to_owned(),
        IssueKind::PathTooLong { .. } => t_static("issue-path-too-long").to_owned(),
        IssueKind::FileTooLarge { size, max } => {
            sized("issue-file-too-large", ("size", *size), ("max", *max))
        }
        IssueKind::CaseCollision { other } => t(
            "issue-case-collision",
            &[("other", other.to_string_lossy().into())],
        ),
        IssueKind::NotEnoughSpace { needed, available } => sized(
            "issue-not-enough-space",
            ("needed", *needed),
            ("available", *available),
        ),
        _ => t_static("issue-unknown").to_owned(),
    }
}

/// What it meant for an item that a job refused it for `issue`.
fn refused_because(issue: &IssueKind) -> &'static str {
    t_static(match issue {
        IssueKind::NameTooLong { .. } => "refused-name-too-long",
        IssueKind::PathTooLong { .. } => "refused-path-too-long",
        IssueKind::InvalidCharacter(_)
        | IssueKind::ReservedName
        | IssueKind::TrailingDotOrSpace
        | IssueKind::CaseCollision { .. } => "refused-name",
        IssueKind::NotEnoughSpace { .. } => "refused-no-space",
        // `FileTooLarge`, and any issue added after this was written.
        _ => "refused-other",
    })
}

/// One line of the pre-flight review a transfer shows before it writes anything: the
/// [`issue_kind`] fact and what follows from it. An item with a warning can still be written
/// and may then be hard to open; one with a blocking issue cannot be written as it is; too
/// little space lets the job start but may stop it halfway.
pub fn preflight_issue(issue: &IssueKind) -> String {
    issue_and(issue, preflight_follows(issue))
}

/// What follows from `issue` for a transfer that has not written anything yet.
fn preflight_follows(issue: &IssueKind) -> &'static str {
    t_static(match issue {
        IssueKind::NotEnoughSpace { .. } => "preflight-no-space",
        _ if issue.severity() == Severity::Warning => "preflight-warning",
        _ => "preflight-blocked",
    })
}

/// `c` in quotes, or its code (`U+0001`) when it would not show.
fn quote_char(c: char) -> String {
    if c.is_control() || (c.is_whitespace() && c != ' ') {
        format!("U+{:04X}", u32::from(c))
    } else {
        format!("‘{c}’")
    }
}

/// How a job of `work` ended, as one sentence: `이동했습니다`, `복사했지만 일부 항목을 처리하지
/// 못했습니다`, `이동을 취소했습니다`, `문제가 생겨 삭제를 중단했습니다`.
pub fn outcome(outcome: Outcome, work: JobWork) -> String {
    let key = match outcome {
        Outcome::Completed => "job-done",
        Outcome::CompletedWithErrors => "job-done-with-errors",
        Outcome::Cancelled => "job-cancelled",
        Outcome::Aborted => "job-aborted",
        _ => "job-ended",
    };
    work.says(key, &[])
}

/// The title of a job's row in the job panel: `대기 중` in the queue, `일시정지됨` when paused,
/// what the job does while it plans, runs or waits for an answer (`복사 중`; the panel shows
/// the question itself), and its [`outcome`] once it has ended.
pub fn job_title(work: JobWork, state: JobState) -> String {
    match state {
        JobState::Queued => t_static("job-queued").to_owned(),
        JobState::Paused => t_static("job-paused").to_owned(),
        JobState::Finished(end) => outcome(end, work),
        // `Planning`, `Running`, `Waiting`, and any state added after this was written.
        _ => work.says("job-doing", &[]),
    }
}

/// The notice of a second instance of the app: the first one keeps the job journal, so the
/// second one ends.
pub fn already_running() -> &'static str {
    t_static("startup-already-running")
}

/// `path` as a message's argument, as the system writes it.
fn shown_path(path: &std::path::Path) -> FluentValue<'static> {
    FluentValue::from(path.display().to_string())
}

/// The notice, once the window is open, when the settings file could not be used: it was moved
/// to `backup` and the app started with the defaults (INST-08).
pub fn settings_backed_up(backup: &std::path::Path) -> String {
    t("settings-backed-up", &[("path", shown_path(backup))])
}

/// What `--cleanup` says of a portable install, which has no uninstaller.
pub fn cleanup_portable() -> &'static str {
    t_static("cleanup-portable")
}

/// What `--cleanup` says when the app still runs after the wait (exit code 3).
pub fn cleanup_still_running() -> &'static str {
    t_static("cleanup-still-running")
}

/// What `--cleanup` says when the program runs from inside a folder it would delete (exit code
/// 1): nothing was changed.
pub fn cleanup_runs_from_owned(folder: &std::path::Path) -> String {
    t("cleanup-runs-from-owned", &[("folder", shown_path(folder))])
}

/// What `--cleanup-plan` says when there is nothing to delete.
pub fn cleanup_plan_empty() -> &'static str {
    t_static("cleanup-plan-empty")
}

/// A step of the plan: the settings are copied to a file first.
pub fn cleanup_step_backup(from: &std::path::Path, to: &std::path::Path) -> String {
    t(
        "cleanup-step-backup",
        &[("from", shown_path(from)), ("to", shown_path(to))],
    )
}

/// A step of the plan: a recorded change of the system is put back.
pub fn cleanup_step_revert(change: &str) -> String {
    t("cleanup-step-revert", &[("change", change.into())])
}

/// A step of the plan: the app's preferences are cleared from the system (macOS).
pub fn cleanup_step_clear_domain(domain: &str) -> String {
    t("cleanup-step-clear-domain", &[("domain", domain.into())])
}

/// A step of the plan: a file or folder is deleted.
pub fn cleanup_step_delete(path: &std::path::Path) -> String {
    t("cleanup-step-delete", &[("path", shown_path(path))])
}

/// A step of the plan: a folder is deleted if it is empty.
pub fn cleanup_step_remove_if_empty(path: &std::path::Path) -> String {
    t(
        "cleanup-step-remove-if-empty",
        &[("path", shown_path(path))],
    )
}

/// A step of the plan: the shared temporary folder is listed for what the app left there.
pub fn cleanup_step_list_temp(path: &std::path::Path) -> String {
    t("cleanup-step-list-temp", &[("path", shown_path(path))])
}

/// A step of a kind this wording does not know.
pub fn cleanup_step_other() -> &'static str {
    t_static("cleanup-step-other")
}

/// 완전 삭제… did not start because the app cannot be moved to the trash from where it is: it
/// would have deleted the data and left the app. Nothing was changed.
pub fn removal_blocked(block: rf_platform::TrashBlock) -> &'static str {
    use rf_platform::TrashBlock;
    t_static(match block {
        TrashBlock::NotInBundle => "removal-blocked-not-in-bundle",
        TrashBlock::Translocated => "removal-blocked-translocated",
        TrashBlock::ReadOnlyVolume => "removal-blocked-read-only-volume",
        TrashBlock::FolderNotWritable => "removal-blocked-folder-not-writable",
    })
}

/// The settings could not be written for good before the removal went on.
pub fn removal_save_pending() -> &'static str {
    t_static("removal-save-pending")
}

/// The settings were copied to `copy`.
pub fn cleanup_backup_done(copy: &std::path::Path) -> String {
    t("cleanup-backup-done", &[("path", shown_path(copy))])
}

/// There was no settings file to back up.
pub fn cleanup_backup_none() -> &'static str {
    t_static("cleanup-backup-none")
}

/// The settings could not be copied to `to`; nothing was deleted (exit code 4).
pub fn cleanup_backup_failed(to: &std::path::Path, reason: &str) -> String {
    t(
        "cleanup-backup-failed",
        &[("path", shown_path(to)), ("reason", reason.into())],
    )
}

/// The clean removal could not run (the ledger file is refused, the lock cannot be taken).
pub fn cleanup_failed(reason: &str) -> String {
    t("cleanup-failed", &[("reason", reason.into())])
}

/// `n` changes of the system were put back.
pub fn cleanup_reverted(n: usize) -> String {
    t("cleanup-reverted", &[("count", count(n))])
}

/// A change of the system that could not be put back.
pub fn cleanup_revert_failed(change: &str, reason: &str) -> String {
    t(
        "cleanup-revert-failed",
        &[("change", change.into()), ("reason", reason.into())],
    )
}

/// Nothing was deleted because a change of the system could not be put back.
pub fn cleanup_deletes_skipped() -> &'static str {
    t_static("cleanup-deletes-skipped")
}

/// A step that was done.
pub fn cleanup_done_line(step: &str) -> String {
    t("cleanup-done-line", &[("step", step.into())])
}

/// A step that was not done, and why.
pub fn cleanup_remaining_line(step: &str, reason: &str) -> String {
    t(
        "cleanup-remaining-line",
        &[("step", step.into()), ("reason", reason.into())],
    )
}

/// The clean removal is complete.
pub fn cleanup_complete() -> &'static str {
    t_static("cleanup-complete")
}

/// Part of the clean removal remains (exit code 5).
pub fn cleanup_partial() -> &'static str {
    t_static("cleanup-partial")
}

/// The line of the list in the window of 완전 삭제…: the app itself goes to the trash last.
pub fn removal_step_bundle(bundle: &std::path::Path) -> String {
    t("removal-step-bundle", &[("path", shown_path(bundle))])
}

/// The message `key` with the app's name as its `$app`.
fn of_app(key: &str) -> String {
    t(key, &[("app", rf_core::identity::DISPLAY_NAME.into())])
}

/// The item at the bottom of 보기 ▸ 설정 that removes the app (macOS).
pub fn remove_app_menu() -> String {
    of_app("removal-menu")
}

/// The title of the window of 완전 삭제….
pub fn removal_title() -> String {
    of_app("removal-title")
}

/// What the window of 완전 삭제… says before the list of what goes.
pub fn removal_message() -> String {
    of_app("removal-message")
}

/// The checkbox of the window of 완전 삭제…: the settings are copied to the documents folder
/// first, and nothing is deleted if that fails.
pub fn removal_backup_label() -> &'static str {
    t_static("removal-backup-label")
}

/// Under the list of the window of 완전 삭제…: what the checkbox does.
pub fn removal_backup_note() -> String {
    t(
        "removal-backup-note",
        &[("file", crate::removal::backup_file_name().into())],
    )
}

/// The button of the window of 완전 삭제… that deletes.
pub fn removal_button() -> &'static str {
    t_static("removal-button")
}

/// The title of the question when jobs run as 완전 삭제… is confirmed.
pub fn removal_jobs_title() -> &'static str {
    t_static("removal-jobs-title")
}

/// The question when `n` jobs run as 완전 삭제… is confirmed: they stop for good, or the
/// removal waits (취소).
pub fn removal_with_jobs(n: usize) -> String {
    t("removal-with-jobs", &[("count", count(n))])
}

/// The button that stops the jobs and goes on with 완전 삭제….
pub fn removal_jobs_button() -> &'static str {
    t_static("removal-jobs-button")
}

/// The list of what 완전 삭제… deletes could not be made; nothing was changed.
pub fn removal_plan_failed(reason: &str) -> String {
    t("removal-plan-failed", &[("reason", reason.into())])
}

/// What the window shows while 완전 삭제… works.
pub fn removing() -> String {
    of_app("removal-removing")
}

/// The title of the window that tells how far 완전 삭제… got when it stopped.
pub fn removal_stopped_title() -> &'static str {
    t_static("removal-stopped-title")
}

/// Said above the report when the app's data is partly or wholly gone and the app stays.
pub fn removal_stopped_note() -> &'static str {
    t_static("removal-stopped-note")
}

/// The app's data is gone, but the app could not be moved to the trash.
pub fn removal_not_trashed(reason: &str) -> String {
    t("removal-not-trashed", &[("reason", reason.into())])
}

/// The settings were copied to `copy` before 완전 삭제… went on.
pub fn removal_backup_done(copy: &std::path::Path) -> String {
    t("removal-backup-done", &[("path", shown_path(copy))])
}

/// The uninstaller of an installed app could not be started.
pub fn uninstaller_failed(reason: &str) -> String {
    t("removal-uninstaller-failed", &[("reason", reason.into())])
}

/// What is said of arguments `--cleanup` does not understand (exit code 2).
pub fn cleanup_usage() -> &'static str {
    t_static("cleanup-usage")
}

/// The title of the window that says the app could not start.
pub fn startup_failed() -> &'static str {
    t_static("startup-failed")
}

/// Why the app could not start: the window system or the renderer failed.
pub fn no_window() -> &'static str {
    t_static("startup-no-window")
}

/// Why the app could not start: the folders it keeps its settings and job journal in cannot
/// be found or made.
pub fn no_app_folders(kind: ErrorKind) -> String {
    with_reason("startup-no-app-folders", kind)
}

/// Why the app could not start: the lock of the job journal's folder could not be taken.
pub fn lock_failed(kind: ErrorKind) -> String {
    with_reason("startup-lock-failed", kind)
}

/// The notice when the job journal could not be read or written: the jobs run on, but one
/// that a crash cuts off may not be offered again.
pub fn journal_error() -> &'static str {
    t_static("job-journal-error")
}

/// What the job panel shows under a job's bar: `3 / 12 항목 · 48 MB/s · 남은 시간 약 20초 ·
/// 대기 중인 작업 1개`. Each part shows only when it is known: the items once the plan has
/// counted them, the speed once it is measured, the time left once the speed is, and the
/// queue when jobs wait in it.
pub fn job_progress(
    done_items: u64,
    total_items: u64,
    bytes_per_sec: u64,
    eta: Option<Duration>,
    queued: usize,
) -> String {
    let mut parts = Vec::new();
    if total_items > 0 {
        parts.push(t(
            "job-progress-items",
            &[
                ("done", number(done_items.min(total_items))),
                ("total", number(total_items)),
            ],
        ));
    }
    if bytes_per_sec > 0 {
        parts.push(t(
            "job-speed",
            &[("speed", format_size(bytes_per_sec).into())],
        ));
        if let Some(eta) = eta.filter(|eta| !eta.is_zero()) {
            parts.push(time_left(eta));
        }
    }
    if queued > 0 {
        parts.push(t("job-progress-queued", &[("count", count(queued))]));
    }
    parts.join(" · ")
}

/// `남은 시간 약 20초`, `약 3분`, `약 1시간 5분`: rounded up, so that the time never reads as
/// shorter than it is.
pub fn time_left(eta: Duration) -> String {
    let secs = eta.as_secs() + u64::from(eta.subsec_nanos() > 0);
    let about = if secs < 60 {
        t("duration-seconds", &[("seconds", number(secs))])
    } else if secs < 3600 {
        t(
            "duration-minutes",
            &[("minutes", number(secs.div_ceil(60)))],
        )
    } else {
        let minutes = secs.div_ceil(60);
        hours_minutes(minutes / 60, minutes % 60)
    };
    t("job-time-left", &[("time", about.into())])
}

/// `hours` hours and `minutes` minutes, without the minutes when there are none: `1시간 5분`,
/// `2시간`.
fn hours_minutes(hours: u64, minutes: u64) -> String {
    match minutes {
        0 => t("duration-hours", &[("hours", number(hours))]),
        minutes => t(
            "duration-hours-minutes",
            &[("hours", number(hours)), ("minutes", number(minutes))],
        ),
    }
}

/// How many cells of a file's name the row of a running job shows at most: a longer name is
/// shortened in the middle. It fits the job panel's window of its own at its narrowest
/// (520px, a line of 480px), at the 8px a cell of the small font takes at most.
pub const JOB_FILE_CELLS: usize = 60;

/// The file a job works on now, as its row names it: the name of `path` (not its folders,
/// which the row's first line tells of), a long one shortened in the middle so that its
/// beginning and its extension both stay in view.
pub fn job_current(path: &VPath) -> String {
    clip_middle(&name_of(path), JOB_FILE_CELLS)
}

/// What the row of a job that waits in the queue says after `대기 중`: what it will do and on
/// what (`detail`, [`job_detail`]), as `복사 · assets → 백업`; the work alone for a job whose
/// items are not known.
pub fn queued_detail(work: JobWork, detail: &str) -> String {
    if detail.is_empty() {
        work.noun().to_owned()
    } else {
        format!("{} · {detail}", work.noun())
    }
}

/// The title of the window the job panel shows in when it is not under the panes (LAY-09).
pub fn job_window_title() -> String {
    of_app("job-window-title")
}

/// The notice when the window of the job panel could not be opened: the panel shows under the
/// panes again.
pub fn job_window_failed() -> &'static str {
    t_static("job-window-failed")
}

/// The progress line of a job that is still counting what it will do.
pub fn job_planning() -> &'static str {
    t_static("job-planning")
}

/// The progress line of a job that waits for the user to answer its question.
pub fn job_waiting() -> &'static str {
    t_static("job-waiting")
}

/// What a job that ended did, in the job panel's 최근 완료 (XFER-14): `복사 12개 · 건너뜀 1개 ·
/// 오류 0개 · 평균 48 MB/s · 걸린 시간 1분 20초`. The speed, the bytes moved over the whole
/// job's time, shows only for a job that moved bytes.
pub fn job_report(
    work: JobWork,
    done: u64,
    skipped: u64,
    failed: usize,
    average_speed: Option<u64>,
    elapsed: Duration,
) -> String {
    let mut parts = vec![
        work.says("job-report-done", &[("count", number(done))]),
        t("job-report-skipped", &[("count", number(skipped))]),
        t("job-report-failed", &[("count", count(failed))]),
    ];
    if let Some(speed) = average_speed {
        parts.push(t(
            "job-report-speed",
            &[("speed", format_size(speed).into())],
        ));
    }
    parts.push(t("job-report-took", &[("time", took(elapsed).into())]));
    parts.join(" · ")
}

/// How long a job took: `1초 미만`, `12초`, `1분 20초`, `2분`, `1시간 5분`, in whole seconds.
pub fn took(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0 => t_static("duration-under-a-second").to_owned(),
        1..60 => t("duration-seconds", &[("seconds", number(secs))]),
        60..3600 => match secs % 60 {
            0 => t("duration-minutes", &[("minutes", number(secs / 60))]),
            rest => t(
                "duration-minutes-seconds",
                &[("minutes", number(secs / 60)), ("seconds", number(rest))],
            ),
        },
        _ => hours_minutes(secs / 3600, secs / 60 % 60),
    }
}

/// The notice of a log saved as `name` (XFER-14's 로그 저장).
pub fn log_saved(name: &str) -> String {
    t("job-log-saved", &[("name", name.into())])
}

/// The notice of a log that could not be saved, for an error of `kind`.
pub fn log_failed(kind: ErrorKind) -> String {
    with_reason("job-log-failed", kind)
}

/// The title of the destination picker that chooses where a job's log goes (PICK-09).
pub fn pick_log_title() -> &'static str {
    t_static("pick-log-title")
}

/// The head of the quick target of the app's own folder for logs.
pub fn pick_log_folder() -> &'static str {
    t_static("pick-log-folder")
}

/// The picker's confirm button when it chooses where a log goes.
pub fn pick_log_confirm() -> &'static str {
    t_static("pick-log-confirm")
}

/// The picker's line under its title when it chooses where the log of a job goes: how the job
/// ended (`title`) and what it worked on (`detail`).
pub fn pick_log_summary(title: &str, detail: &str) -> String {
    if detail.is_empty() {
        title.to_owned()
    } else {
        format!("{title} · {detail}")
    }
}

/// The title of the system's notification of a job that ended while the window was not in
/// front (XFER-14): `복사를 마쳤습니다 · 항목 12개`, the job having been asked to act on `items`
/// items, `failed` of which it could not process; `None` for a job the user cancelled.
pub fn job_notice(work: JobWork, outcome: Outcome, items: usize, failed: usize) -> Option<String> {
    Some(match outcome {
        Outcome::Completed => work.says("job-notice-done", &[("count", count(items))]),
        Outcome::CompletedWithErrors => {
            work.says("job-notice-done-with-errors", &[("failed", count(failed))])
        }
        Outcome::Cancelled => return None,
        Outcome::Aborted => work.says("job-aborted", &[]),
        _ => work.says("job-ended", &[]),
    })
}

/// What the last notification adds after the window's jobs went on in the tray and all of them
/// have ended: the app has ended too.
pub fn all_jobs_done() -> &'static str {
    t_static("tray-all-done")
}

/// What the last notification adds after the window's jobs went on in the tray and all of them
/// have ended, some of them with items they could not process: the window shows them.
pub fn all_jobs_done_with_failures() -> &'static str {
    t_static("tray-all-done-with-failures")
}

/// The notice of a window that stays shown because it could not make the tray icon.
pub fn tray_unavailable() -> &'static str {
    t_static("tray-unavailable")
}

/// The button of the question before the window closes while jobs run that keeps them going
/// behind the system's tray icon (Windows) or menu bar icon (macOS), on `os`.
pub fn continue_in_tray(os: Os) -> &'static str {
    t_static(match os {
        Os::MacOs => "tray-continue-menu-bar",
        Os::Windows | Os::Other => "tray-continue",
    })
}

/// What the question before the window closes while jobs run asks when the jobs can go on
/// behind the tray icon (XFER-15), on `os`.
pub fn quit_or_tray(os: Os) -> &'static str {
    t_static(match os {
        Os::MacOs => "tray-ask-menu-bar",
        Os::Windows | Os::Other => "tray-ask",
    })
}

/// The tooltip of the tray icon while the jobs go on behind it: `app`, then each job a worker
/// has taken up, three at most, with how far it has got (`복사 중 25%`), and the first one's
/// progress line; `대기 중` before any has started.
pub fn tray_status(app: &str, rows: &[JobRow]) -> String {
    let mut lines = vec![app.to_owned()];
    if rows.is_empty() {
        lines.push(t_static("job-queued").to_owned());
    }
    for row in rows.iter().take(3) {
        lines.push(format!("{} {}", row.title, percent(row.fraction)));
    }
    if let Some(row) = rows.first().filter(|row| !row.progress.is_empty()) {
        lines.push(row.progress.clone());
    }
    lines.join("\n")
}

/// `fraction` (0 to 1) as a whole percent, rounded down: `25%`.
pub fn percent(fraction: f32) -> String {
    let hundredths = fraction.clamp(0.0, 1.0) * 100.0;
    let whole = (0..=100_u8)
        .rev()
        .find(|&step| f32::from(step) <= hundredths)
        .unwrap_or(0);
    format!("{whole}%")
}

/// What a job works on, beside its title in the job panel: `assets → 백업` for a transfer (the
/// item, or `항목 3개`, and the name of the target folder, as the 화면 사양 shows it), the item
/// or the count for a delete, `a.txt → b.txt` for a rename, `새 폴더 → 작업` for a create, and
/// the count for an undo.
pub fn job_detail(spec: &JobSpec) -> String {
    detail(spec, name_of)
}

/// [`job_detail`] with each folder shown by `folder`.
fn detail(spec: &JobSpec, folder: impl Fn(&VPath) -> String) -> String {
    match spec {
        JobSpec::Transfer(transfer) => {
            let names: Vec<&VPath> = transfer.items.iter().map(|item| &item.source).collect();
            let target = transfer
                .items
                .first()
                .and_then(|item| item.target.parent())
                .map(|target| folder(&target));
            match target {
                Some(target) => format!("{} → {target}", items_named(&names)),
                None => items_named(&names),
            }
        }
        JobSpec::Delete(delete) => items_named(&delete.paths.iter().collect::<Vec<_>>()),
        JobSpec::DeleteCopies(delete) => items_named(
            &delete
                .copies
                .iter()
                .map(|c| &c.made.path)
                .collect::<Vec<_>>(),
        ),
        JobSpec::Rename { from, to } => format!("{} → {}", name_of(from), name_of(to)),
        JobSpec::Create { parent, name, .. } => {
            format!("{} → {}", name.to_string_lossy(), folder(parent))
        }
        JobSpec::Undo(record) => item_count(record.item_count()),
        // The items of the trash by the names they had (NAV-16).
        JobSpec::RestoreFromTrash { items } => {
            trash_items_named(&items.iter().map(|(item, _)| item).collect::<Vec<_>>())
        }
        JobSpec::DeleteFromTrash { items } => trash_items_named(&items.iter().collect::<Vec<_>>()),
        JobSpec::EmptyTrash => shell_place(ShellPlace::Trash).to_owned(),
        // A kind of job added after this was written.
        _ => String::new(),
    }
}

/// [`items_named`] of items of the trash, by the names they had there.
fn trash_items_named(items: &[&rf_vfs::TrashItem]) -> String {
    match items {
        [one] => one.name.to_string_lossy().into_owned(),
        many => item_count(many.len()),
    }
}

/// The notice when items cannot be copied or moved at all (a volume root, a name the target
/// cannot take): `복사하거나 옮길 수 없습니다: 쓸 수 없는 이름이나 경로입니다`.
pub fn cannot_transfer(kind: ErrorKind) -> String {
    with_reason("error-cannot-transfer", kind)
}

/// Why the clipboard did not take or give what was asked: the system has no clipboard for it
/// (text on Linux), or another app held it or the system refused.
pub fn clipboard_failed(failure: ClipboardFailure) -> &'static str {
    t_static(match failure {
        ClipboardFailure::Unsupported => "clipboard-unsupported",
        ClipboardFailure::Unavailable => "clipboard-unavailable",
    })
}

/// A paste with no files or folders on the clipboard (text, an image, nothing).
pub fn nothing_to_paste() -> &'static str {
    t_static("paste-nothing")
}

/// A paste of a cut the user cancelled with Esc, which the clipboard still holds: nothing
/// moves.
pub fn cut_cancelled() -> &'static str {
    t_static("paste-cut-cancelled")
}

/// A paste whose `n` paths of the clipboard all name no folder or file it can take (a device
/// or a volume of Windows, a file URL of another computer on macOS): nothing is pasted.
pub fn clipboard_skipped(n: usize) -> String {
    t("paste-all-skipped", &[("count", count(n))])
}

/// A paste of a folder into itself or into a folder inside it.
pub fn paste_into_itself() -> &'static str {
    t_static("paste-into-itself")
}

/// A paste of items cut, into the folder they are in already: nothing moves.
pub fn paste_already_here() -> &'static str {
    t_static("paste-already-here")
}

/// The same cut pasted again while its first paste still moves the items.
pub fn paste_moving() -> &'static str {
    t_static("paste-moving")
}

/// A cut cancelled with Esc that the clipboard could not drop (another app held it): a paste
/// in another app could still move the items.
pub fn clipboard_not_emptied() -> &'static str {
    t_static("clipboard-not-emptied")
}

/// A paste that left out `n` paths of the clipboard, which name no folder or file it can take
/// (a device or a volume of Windows, a file URL of another computer on macOS).
pub fn paste_left_out(n: usize) -> String {
    t("paste-left-out", &[("count", count(n))])
}

/// What the app asked of the system ([`ShellWork`](crate::state::ShellWork)), for the notice
/// when it was not done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShellAction {
    /// 연결 프로그램으로 열기.
    OpenWith,
    /// 관리자 권한으로 실행.
    RunAsAdmin,
    /// 터미널에서 열기.
    Terminal,
    /// OS 속성 창 열기.
    SystemProperties,
    /// The system's own menu of items (Windows': Shift+F10, 더 많은 옵션 표시).
    SystemMenu,
    /// 공유… (macOS).
    Share,
}

/// The notice when the system did not do `action` on `os`: it does not do it there
/// (`이 시스템에서는 OS 속성 창을 지원하지 않습니다`; for a terminal, none was found), macOS did
/// not let the app control Finder, or else what went wrong
/// (`터미널을 열지 못했습니다: 찾을 수 없습니다`).
pub fn shell_failed(action: ShellAction, kind: ErrorKind, os: Os) -> String {
    let unsupported = match action {
        ShellAction::OpenWith => "shell-open-with-unsupported",
        ShellAction::RunAsAdmin => "shell-run-as-admin-unsupported",
        ShellAction::Terminal => "shell-terminal-unsupported",
        ShellAction::SystemProperties => "shell-properties-unsupported",
        ShellAction::SystemMenu => "shell-system-menu-unsupported",
        ShellAction::Share => "shell-share-unsupported",
    };
    match (action, kind) {
        (_, ErrorKind::Unsupported) => t(unsupported, &[(OS_ARG, os_arg(os))]),
        (ShellAction::SystemProperties, ErrorKind::PermissionDenied) if os == Os::MacOs => {
            t("shell-finder-not-allowed", &[(OS_ARG, os_arg(os))])
        }
        (action, kind) => {
            let failed = match action {
                ShellAction::OpenWith => "shell-open-with-failed",
                ShellAction::RunAsAdmin => "shell-run-as-admin-failed",
                ShellAction::Terminal => "shell-terminal-failed",
                ShellAction::SystemProperties => "shell-properties-failed",
                ShellAction::SystemMenu => "shell-system-menu-failed",
                ShellAction::Share => "shell-share-failed",
            };
            t(
                failed,
                &[(OS_ARG, os_arg(os)), ("reason", error_kind(kind).into())],
            )
        }
    }
}

/// 관리자 권한으로 실행 refused for a batch file whose path `cmd.exe` would read as a command
/// (`rf_platform::shell::elevation`).
pub fn batch_path_refused() -> &'static str {
    t_static("shell-batch-path-refused")
}

/// The link chosen leads elsewhere than the command wants: to no folder, for a terminal
/// (`folder`), or to no file, for 연결 프로그램 and 관리자 권한으로 실행. A dangling link leads to
/// neither.
pub fn link_elsewhere(folder: bool) -> &'static str {
    t_static(if folder {
        "shell-link-not-folder"
    } else {
        "shell-link-not-file"
    })
}

/// The system knows no app that opens the file (연결 프로그램, macOS).
pub fn no_apps() -> &'static str {
    t_static("shell-no-apps")
}

/// An app of the menu of 연결 프로그램: its name, and `(기본)` after the system's default one.
pub fn app_title(app: &App) -> String {
    if app.default {
        t("shell-app-default", &[("name", app.name.as_str().into())])
    } else {
        app.name.clone()
    }
}

/// `항목 3개`.
pub fn item_count(n: usize) -> String {
    t("count-items", &[("count", count(n))])
}

/// The name of the one item in `paths`, or the count of them.
fn items_named(paths: &[&VPath]) -> String {
    match paths {
        [one] => name_of(one),
        many => item_count(many.len()),
    }
}

/// The last part of `path`, or the whole path for a root.
fn name_of(path: &VPath) -> String {
    path.file_name().map_or_else(
        || path.to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The notice of a transfer or a trash that ended as asked, `n` items at its top level:
/// `항목 3개를 이동했습니다`. When its conflict policy left items out, the numbers are of every
/// item it went through (`done` made, `skipped` left out): `항목 3개 중 1개를 복사했습니다 ·
/// 2개 건너뜀`, or `항목 3개를 건너뛰었습니다` when it made none.
pub fn items_done(work: JobWork, n: usize, done: u64, skipped: u64) -> String {
    done_with(work.arg(), n, done, skipped)
}

/// The notice of a move that a drag made and that ended without failures (DND-07), as
/// [`items_done`] says the others: `항목 3개를 옮겼습니다`.
pub fn dragged_move_done(n: usize, done: u64, skipped: u64) -> String {
    done_with(FluentValue::from("drag"), n, done, skipped)
}

/// [`items_done`] of the work `work` as the messages' `$work` selects it, which may also be
/// `drag` for a move a drag made (`옮겼습니다`).
fn done_with(work: FluentValue<'static>, n: usize, done: u64, skipped: u64) -> String {
    match (done, skipped) {
        (_, 0) => t("job-items-done", &[("work", work), ("count", count(n))]),
        (0, skipped) => t("job-items-skipped", &[("count", number(skipped))]),
        (done, skipped) => t(
            "job-items-partly-done",
            &[
                ("work", work),
                ("count", number(done.saturating_add(skipped))),
                ("done", number(done)),
                ("skipped", number(skipped)),
            ],
        ),
    }
}

/// The notice of a trash job that ended: what went to the trash (`trashed` items), and what
/// the location deleted for good instead (`gone`), which nothing brings back.
pub fn trash_done(trashed: usize, gone: usize) -> String {
    match (trashed, gone) {
        (trashed, 0) => items_done(JobWork::Trash, trashed, 0, 0),
        (0, gone) => t("job-trash-all-gone", &[("count", count(gone))]),
        (trashed, gone) => t(
            "job-trash-some-gone",
            &[("count", count(trashed)), ("gone", count(gone))],
        ),
    }
}

/// The notice of a job some items of which failed: `항목 2개를 처리하지 못했습니다`.
pub fn items_failed(n: usize) -> String {
    t("job-items-failed", &[("count", count(n))])
}

/// The notice of a job that was stopped at a failed item (중단 in the window that asks about
/// it, XFER-09) or by an error: that it was stopped, how many items failed, and that the
/// `left` items after them were not processed (`None`: how many cannot be told; `Some(0)`: it
/// stopped at its last item). `복사를 중단했습니다 · 실패 1개 · 남은 항목 2개는 처리하지 않았습니다`.
pub fn job_stopped(work: JobWork, failed: usize, left: Option<u64>) -> String {
    let mut parts = vec![
        work.says("job-stopped", &[]),
        t("job-stopped-failed", &[("count", count(failed))]),
    ];
    match left {
        Some(0) => {}
        Some(left) => parts.push(t("job-stopped-left", &[("count", number(left))])),
        None => parts.push(t_static("job-stopped-left-unknown").to_owned()),
    }
    parts.join(" · ")
}

/// The first line of the window of failed items of such a job: the items below are where it
/// stopped, what came after them was not processed, and the window has no 다시 시도, which
/// would try the listed items alone.
pub fn job_stopped_note(work: JobWork, left: Option<u64>) -> String {
    match left {
        Some(0) => work.says("job-stopped-note-last", &[]),
        Some(left) => work.says("job-stopped-note-left", &[("count", number(left))]),
        None => work.says("job-stopped-note", &[]),
    }
}

/// `text`, the notice of a job that ended, and how many of its items (`gone`) the location
/// deleted for good when they were to go to the trash, which nothing brings back: `복사를
/// 되돌렸습니다 · 1개는 휴지통 대신 영구 삭제되었습니다`. `text` as it is for none.
pub fn and_gone(text: String, gone: usize) -> String {
    if gone == 0 {
        text
    } else {
        format!(
            "{text} · {}",
            t("job-gone-instead", &[("count", count(gone))])
        )
    }
}

/// The notice of an undo that ended as asked, naming the work it reversed from its record:
/// `이동을 되돌렸습니다`.
pub fn undone(record: &UndoRecord) -> String {
    let work = match record {
        UndoRecord::Copied { .. } => JobWork::Copy,
        UndoRecord::Moved { .. } => JobWork::Move,
        UndoRecord::Trashed { .. } => JobWork::Trash,
        UndoRecord::Renamed { .. } => JobWork::Rename,
        UndoRecord::Created { .. } => JobWork::Create,
        UndoRecord::LinksMade { .. } => JobWork::Link,
        // The record of an undo, or one added after this was written.
        _ => JobWork::Other,
    };
    work.says("job-undone", &[])
}

/// The notice of a rename or a create that failed, with why: `이름을 바꾸지 못했습니다: 같은
/// 이름의 항목이 이미 있습니다`.
pub fn name_refused(work: JobWork, failure: &ItemFailure) -> String {
    let key = match work {
        JobWork::Create => "job-create-refused",
        _ => "job-rename-refused",
    };
    t(key, &[("reason", item_failure(failure).into())])
}

/// Why one item failed: its [`failure_reason`] when the job refused it, else the
/// [`error_kind`] of the error.
pub fn item_failure(failure: &ItemFailure) -> String {
    match &failure.reason {
        Some(reason) => failure_reason(reason),
        None => error_kind(failure.kind).to_owned(),
    }
}

/// One line of the window of failed items: the item and why (`/home/me/a.txt: 권한이
/// 없습니다`). The item is the source, or the target when there is no source, with its whole
/// path: two failed items can differ only in the middle of their paths, so nothing is left out
/// (the window wraps a long line).
pub fn failure_line(failure: &ItemFailure) -> String {
    let why = item_failure(failure);
    match failure.source.as_ref().or(failure.target.as_ref()) {
        Some(path) => format!("{path}: {why}"),
        None => why,
    }
}

/// The title of the window of failed items.
pub fn failures_title() -> &'static str {
    t_static("job-failures-title")
}

/// The label of the toast's button that undoes the job it tells of.
pub fn undo_action() -> &'static str {
    t_static("toast-undo")
}

/// The label of the toast's button that shows the failed items.
pub fn details_action() -> &'static str {
    t_static("toast-details")
}

/// The notice when the toast's job is no longer the next one to undo.
pub fn undo_order() -> &'static str {
    t_static("undo-order")
}

/// The notice when an undo or a redo is asked for while one is under way: one runs at a time.
pub fn undo_busy() -> &'static str {
    t_static("undo-busy")
}

// ---- 되돌리기 기록 (OPS-12) ----

/// How many rows of each list the 되돌리기 기록 shows at most; the engine keeps more
/// ([`rf_jobs::UNDO_LIMIT`]).
pub const HISTORY_SHOWN: usize = 50;

/// Why a record of the history can no longer be undone (or done again), as far as the folders
/// on screen tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stale {
    /// An item the undo works on is gone.
    Missing,
    /// A copied item is not as the copy left it.
    Changed,
}

/// The head of the group in the job panel.
pub fn history_head(n: usize) -> String {
    t("history-head", &[("count", count(n))])
}

/// The head of the list of what can be undone.
pub fn history_undo_section(n: usize) -> String {
    t("history-undo-section", &[("count", count(n))])
}

/// The head of the list of what can be done again.
pub fn history_redo_section(n: usize) -> String {
    t("history-redo-section", &[("count", count(n))])
}

/// How many cells the name of an item takes at most in a row of the history.
const HISTORY_NAME_CELLS: usize = 28;

/// The cells the text of a filter takes at most in the status line.
const STATUS_FILTER_CELLS: usize = 20;

/// What the record does, short. With `redo` false it tells what the job did, as the list of
/// what can be undone shows it (`이름 바꾸기: a.txt → b.txt`); with `redo` true it tells what
/// the record's own undo does, which is what a row of the list of what can be done again
/// stands for: its button runs that undo.
pub fn history_action(record: &UndoRecord, redo: bool) -> String {
    fn named(path: &VPath) -> String {
        clip_middle(&name_of(path), HISTORY_NAME_CELLS)
    }
    fn folder_of(path: &VPath) -> String {
        path.parent().map_or_else(String::new, |parent| {
            clip_middle(&name_of(&parent), HISTORY_NAME_CELLS)
        })
    }
    /// The one name among `paths`, or the count of them.
    fn items(paths: &[&VPath]) -> String {
        match paths {
            [one] => named(one),
            many => t("history-items", &[("count", count(many.len()))]),
        }
    }
    // The head of the row (a message), then what it was done to.
    let put = |label: &'static str, what: String| format!("{}: {what}", t_static(label));
    let make = |kind: &CreateKind, path: &VPath| {
        let label = match kind {
            CreateKind::File => "history-new-file",
            CreateKind::Dir => "history-new-folder",
        };
        put(label, named(path))
    };
    match (record, redo) {
        (UndoRecord::Renamed { from, to }, false) => {
            put("history-rename", format!("{} → {}", named(from), named(to)))
        }
        (UndoRecord::Renamed { from, to }, true) => {
            put("history-rename", format!("{} → {}", named(to), named(from)))
        }
        (UndoRecord::Moved { moves, .. }, false) => {
            let moved: Vec<&VPath> = moves.iter().map(|m| &m.to).collect();
            let into = moves.first().map(|m| folder_of(&m.to)).unwrap_or_default();
            put("history-move", format!("{} → {into}", items(&moved)))
        }
        (UndoRecord::Moved { moves, .. }, true) => {
            let moved: Vec<&VPath> = moves.iter().map(|m| &m.from).collect();
            let into = moves
                .first()
                .map(|m| folder_of(&m.from))
                .unwrap_or_default();
            put("history-move", format!("{} → {into}", items(&moved)))
        }
        (UndoRecord::Copied { created, .. }, false) => {
            let made: Vec<&VPath> = created.iter().map(|item| &item.path).collect();
            let into = created
                .first()
                .map(|item| folder_of(&item.path))
                .unwrap_or_default();
            put("history-copy", format!("{} → {into}", items(&made)))
        }
        (UndoRecord::Copied { created, .. }, true) => put(
            "history-to-trash",
            items(&created.iter().map(|item| &item.path).collect::<Vec<_>>()),
        ),
        (UndoRecord::Created { path, kind }, false)
        | (UndoRecord::Removed { path, kind }, true) => make(kind, path),
        (UndoRecord::Created { path, .. }, true) | (UndoRecord::Removed { path, .. }, false) => {
            put("history-delete", named(path))
        }
        (UndoRecord::Trashed { items: trashed }, false)
        | (UndoRecord::Restored { items: trashed }, true) => put(
            "history-to-trash",
            items(
                &trashed
                    .iter()
                    .map(|item| &item.original)
                    .collect::<Vec<_>>(),
            ),
        ),
        (UndoRecord::Trashed { items: trashed }, true)
        | (UndoRecord::Restored { items: trashed }, false) => put(
            "history-from-trash",
            items(
                &trashed
                    .iter()
                    .map(|item| &item.original)
                    .collect::<Vec<_>>(),
            ),
        ),
        (UndoRecord::LinksMade { at: links }, false)
        | (UndoRecord::LinksRemoved { links }, true) => put(
            "history-link",
            items(&links.iter().map(|(link, _, _)| link).collect::<Vec<_>>()),
        ),
        (UndoRecord::LinksMade { at: links }, true)
        | (UndoRecord::LinksRemoved { links }, false) => put(
            "history-unlink",
            items(&links.iter().map(|(link, _, _)| link).collect::<Vec<_>>()),
        ),
        // A record added after this was written.
        _ => t_static("history-other").to_owned(),
    }
}

/// `text` shortened in the middle to at most `max` cells with `…`, at whole characters, so that
/// the end of a name (its extension) stays in view.
fn clip_middle(text: &str, max: usize) -> String {
    if text_cells(text) <= max {
        return text.to_owned();
    }
    let room = max.saturating_sub(1);
    let (head_room, tail_room) = (room.div_ceil(2), room / 2);
    let mut head = String::new();
    let mut used = 0;
    for c in text.chars() {
        let width = char_cells(c);
        if used + width > head_room {
            break;
        }
        head.push(c);
        used += width;
    }
    let mut tail: Vec<char> = Vec::new();
    let mut used = 0;
    for c in text.chars().rev() {
        let width = char_cells(c);
        if used + width > tail_room {
            break;
        }
        tail.push(c);
        used += width;
    }
    tail.reverse();
    format!("{head}…{}", tail.into_iter().collect::<String>())
}

/// `text` shortened at the end to at most `max` cells with `…`, at whole characters.
fn clip_end(text: &str, max: usize) -> String {
    if text_cells(text) <= max {
        return text.to_owned();
    }
    let mut kept = String::new();
    let mut used = 0;
    for c in text.chars() {
        let width = char_cells(c);
        if used + width > max.saturating_sub(1) {
            break;
        }
        kept.push(c);
        used += width;
    }
    kept.push('…');
    kept
}

/// The second line of a row of the history: why it may not run (`stale`), else that it is the
/// `next` one of its list, else why it waits. A row that may not run still has its button when it
/// is the next one: the folders on screen can lag behind the disk, and the undo looks again
/// itself.
pub fn history_note(redo: bool, next: bool, stale: Option<Stale>) -> String {
    let key = match (stale, next, redo) {
        (Some(Stale::Missing), _, false) => "history-missing-undo",
        (Some(Stale::Missing), _, true) => "history-missing-redo",
        (Some(Stale::Changed), _, false) => "history-changed-undo",
        (Some(Stale::Changed), _, true) => "history-changed-redo",
        (None, true, false) => "history-next-undo",
        (None, true, true) => "history-next-redo",
        (None, false, false) => return undo_order().to_owned(),
        (None, false, true) => "history-redo-first",
    };
    t_static(key).to_owned()
}

/// The time of a row: `14:32` for today (the day of `now` in `tz`), `10월 30일 14:32` before.
/// Nothing for a record whose time is not known (the epoch).
pub fn history_time(at: SystemTime, now: SystemTime, tz: &TimeZone) -> String {
    if at <= SystemTime::UNIX_EPOCH {
        return String::new();
    }
    let Ok(stamp) = Timestamp::try_from(at) else {
        return String::new();
    };
    let when = tz.to_datetime(stamp);
    let hour_minute = format!("{:02}:{:02}", when.hour(), when.minute());
    let today = Timestamp::try_from(now).map(|now| tz.to_datetime(now).date());
    if today.is_ok_and(|today| today == when.date()) {
        hour_minute
    } else {
        t(
            "history-time-day",
            &[
                ("month", number(u64::from(when.month().unsigned_abs()))),
                ("day", number(u64::from(when.day().unsigned_abs()))),
                ("time", hour_minute.into()),
            ],
        )
    }
}

/// The label of a row's button.
pub fn history_button(redo: bool) -> &'static str {
    t_static(if redo {
        "history-redo-button"
    } else {
        "history-undo-button"
    })
}

/// The default name of a new folder.
pub fn new_folder_name() -> &'static str {
    t_static("name-new-folder")
}

/// The title of the window that asks for a new folder's name.
pub fn new_folder_title() -> &'static str {
    t_static("name-new-folder-title")
}

/// The default name of a new empty text file (새 파일 ▸ 빈 텍스트 파일, OPS-01): File Explorer's
/// "새 텍스트 문서.txt" in the words of this app's menu.
pub fn new_file_name() -> &'static str {
    t_static("name-new-file")
}

/// The title of the window that asks for a new file's name.
pub fn new_file_title() -> &'static str {
    t_static("name-new-file-title")
}

/// The line of a folder in the visit history's menu (NAV-04): its name, and the folder it is in
/// (`parent`, its middle left out when it is long), so that two folders of one name tell apart;
/// a root, which is in no folder, by its name alone. A menu item has no description of its own
/// for a screen reader, so the folder it is in is in the line itself.
pub fn history_entry(name: &str, parent: Option<&str>) -> String {
    match parent {
        Some(parent) => format!("{name} — {parent}"),
        None => name.to_owned(),
    }
}

/// The title of the window that asks for an item's new name.
pub fn rename_title() -> &'static str {
    t_static("name-rename-title")
}

/// The warning under a name that holds a character no name may have on this system.
pub fn name_has_character(c: char) -> String {
    t("name-has-character", &[("char", quote_char(c).into())])
}

/// The warning under `.` or `..`, which name the folder itself and its parent.
pub fn name_is_dots() -> &'static str {
    t_static("name-is-dots")
}

/// The title of the confirmation before a permanent delete.
pub fn delete_permanently_title() -> &'static str {
    t_static("delete-permanently-title")
}

/// The label of the button that deletes for good.
pub fn delete_permanently_button() -> &'static str {
    t_static("delete-permanently-button")
}

/// The question before a permanent delete the user asked for (Shift+Delete): what goes and
/// that nothing brings it back.
pub fn delete_permanently(n: usize) -> String {
    t("delete-permanently", &[("count", count(n))])
}

/// The question before items of the trash are deleted for good (휴지통 ▸ 영구 삭제…, NAV-16):
/// how many, their size when the trash knows it for every one, and that nothing brings them
/// back.
pub fn delete_from_trash(n: usize, size: Option<u64>) -> String {
    sized("trash-delete-items", "trash-delete-items-sized", n, size)
}

/// The title and the button of the question before the trash is emptied.
pub fn empty_trash_title() -> &'static str {
    t_static("trash-empty-title")
}

/// The question before the trash is emptied (휴지통 비우기…): how many items it holds as the
/// list was read for the question, their size when the trash knows it for every one, and that
/// nothing brings them back.
pub fn empty_trash(n: usize, size: Option<u64>) -> String {
    sized("trash-empty", "trash-empty-sized", n, size)
}

/// The message `key` of `n` items, or `with_size` with their size (`1.2 MB`) when it is known.
fn sized(key: &str, with_size: &str, n: usize, size: Option<u64>) -> String {
    match size {
        Some(size) => t(
            with_size,
            &[("count", count(n)), ("size", format_size(size).into())],
        ),
        None => t(key, &[("count", count(n))]),
    }
}

/// The notice when 휴지통 비우기… finds the trash empty.
pub fn trash_already_empty() -> &'static str {
    t_static("trash-already-empty")
}

/// The notice when the trash could not be read for 휴지통 비우기…, with why.
pub fn trash_unreadable(kind: ErrorKind) -> String {
    with_reason("trash-unreadable", kind)
}

/// The notice when 복원 is asked for items whose original place the trash does not keep (macOS,
/// or a record that cannot be read): only 위치를 골라 꺼내기… can take them out.
pub fn restore_place_unknown() -> &'static str {
    t_static("trash-place-unknown")
}

/// The notice after a restore that put every item back: how many, and where to (`to`, the name
/// of the one folder they went into; none when they went to several, each to its own).
pub fn restored(n: usize, to: Option<&str>) -> String {
    match to {
        Some(folder) => t(
            "trash-restored-to",
            &[("count", count(n)), ("folder", folder.into())],
        ),
        None => t("trash-restored", &[("count", count(n))]),
    }
}

/// The notice after a restore of which some items found their original folder gone: they stay
/// in the trash, and the toast's button finds them another place.
pub fn restore_folders_gone(n: usize) -> String {
    t("trash-folders-gone", &[("count", count(n))])
}

/// The label of the button that takes items out of the trash to a folder the user chooses.
pub fn restore_elsewhere_action() -> &'static str {
    t_static("trash-restore-elsewhere")
}

/// The notice when an item of the trash is opened (Enter, a double click): it is opened once it
/// is back.
pub fn trash_item_not_opened() -> &'static str {
    t_static("trash-item-not-opened")
}

/// The notice when a drive of 내 PC cannot be opened: its row stands for no drive the sidebar
/// knows (any more).
pub fn drive_gone() -> &'static str {
    t_static("computer-drive-gone")
}

/// What a command could not do in a place of the shell (NAV-16): the trash and the computer are
/// lists, not folders, and nothing is made, renamed, copied, read or moved in them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blocked {
    NewFolder,
    NewFile,
    Paste,
    Rename,
    Link,
    Copy,
    Cut,
    Transfer,
    CopyPath,
    Properties,
    OpenWith,
    Terminal,
    Preview,
    Share,
    Delete,
    Favorite,
}

/// The notice when a command does nothing in the trash or the computer, and what is possible
/// there: `휴지통에서는 새 폴더를 만들 수 없습니다. 항목은 복원하거나 영구 삭제할 수 있습니다.`
pub fn shell_blocked(place: ShellPlace, what: Blocked) -> String {
    // What could not be done, as the message's `$what` selects it.
    let what = match what {
        Blocked::NewFolder => "new_folder",
        Blocked::NewFile => "new_file",
        Blocked::Paste => "paste",
        Blocked::Rename => "rename",
        Blocked::Link => "link",
        Blocked::Copy => "copy",
        Blocked::Cut => "cut",
        Blocked::Transfer => "transfer",
        Blocked::CopyPath => "copy_path",
        Blocked::Properties => "properties",
        Blocked::OpenWith => "open_with",
        Blocked::Terminal => "terminal",
        Blocked::Preview => "preview",
        Blocked::Share => "share",
        Blocked::Delete => "delete",
        Blocked::Favorite => "favorite",
    };
    let place = match place {
        ShellPlace::Trash => "trash",
        ShellPlace::Computer => "computer",
    };
    t(
        "shell-blocked",
        &[("place", place.into()), ("what", what.into())],
    )
}

/// What shows beside the pointer of a drag over the 휴지통 of the sidebar: a drop there sends
/// the items to the trash.
pub fn drop_to_trash() -> &'static str {
    t_static("drop-to-trash")
}

/// The question when the folder's location has no trash (OPS-03): the items would go for
/// good, which only the user may choose.
pub fn trash_unavailable(n: usize) -> String {
    t("trash-unavailable", &[("count", count(n))])
}

/// The question when the trash refused some items of a trash job: they are still there, and
/// would go for good.
pub fn trash_refused(n: usize) -> String {
    t("trash-refused", &[("count", count(n))])
}

/// What the confirmation after an undo of a copy says first, once a worker has found every
/// copy's original as it was: the trash refused the copies, and nothing is lost without them.
pub fn copies_refused() -> &'static str {
    t_static("copies-refused")
}

/// The question of that confirmation, under the copies' paths.
pub fn delete_copies(n: usize) -> String {
    t("copies-delete", &[("count", count(n))])
}

/// The toast once the copies an undo could not trash are deleted for good: the copy is undone.
pub fn copies_deleted(n: usize) -> String {
    t("copies-deleted", &[("count", count(n))])
}

/// The toast when the copies an undo could not trash were all gone by the time their delete
/// ran: nothing was deleted, and the copy is undone all the same.
pub fn copies_were_gone() -> &'static str {
    t_static("copies-were-gone")
}

/// The notice while a worker looks at the copies an undo could not trash and their originals.
pub fn checking_originals() -> &'static str {
    t_static("copies-checking")
}

/// Why the copies an undo could not trash were not offered for a permanent delete, or not
/// deleted, after [`failure_line`] of a copy: its original is gone, it differs from the copy
/// (another kind, another size, something of the copy missing in it), or it could not be found
/// out; or the copy itself was written to, is gone, or could not be looked at.
pub fn copy_kept(why: CopyKept) -> &'static str {
    t_static(match why {
        CopyKept::OriginalMissing => "copy-kept-original-missing",
        CopyKept::OriginalChanged => "copy-kept-original-changed",
        CopyKept::OriginalUnknown => "copy-kept-original-unknown",
        CopyKept::CopyChanged => "copy-kept-copy-changed",
        CopyKept::CopyMissing => "copy-kept-copy-missing",
        CopyKept::CopyUnknown => "copy-kept-copy-unknown",
    })
}

/// Why a copy an undo could not trash stays ([`copy_kept`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyKept {
    OriginalMissing,
    OriginalChanged,
    OriginalUnknown,
    CopyChanged,
    CopyMissing,
    CopyUnknown,
}

impl CopyKept {
    /// Why a copy found as `found` stays: by the copy while it is not as its job left it, else
    /// by its original. `None` for a copy and an original as they were.
    pub fn of(found: CopyFound) -> Option<CopyKept> {
        Some(match found {
            CopyFound::SAME => return None,
            CopyFound {
                copy: Compared::Same,
                original,
            } => match original {
                Compared::Missing => CopyKept::OriginalMissing,
                Compared::Changed => CopyKept::OriginalChanged,
                Compared::Same | Compared::Unknown => CopyKept::OriginalUnknown,
            },
            CopyFound { copy, .. } => match copy {
                Compared::Missing => CopyKept::CopyMissing,
                Compared::Changed => CopyKept::CopyChanged,
                Compared::Same | Compared::Unknown => CopyKept::CopyUnknown,
            },
        })
    }
}

/// The names of the items a confirmation is about: up to three, in quotes, and how many more.
pub fn names_line(paths: &[VPath]) -> String {
    let names: Vec<String> = paths.iter().map(name_of).collect();
    shown_names_line(&names)
}

/// [`names_line`] of items by the names they show (an item of the trash by the name it had).
pub fn shown_names_line(names: &[String]) -> String {
    const SHOWN: usize = 3;
    let shown: Vec<String> = names
        .iter()
        .take(SHOWN)
        .map(|name| t("names-quoted", &[("name", name.as_str().into())]))
        .collect();
    let mut line = shown.join(", ");
    if names.len() > SHOWN {
        let more = t("names-more", &[("count", count(names.len() - SHOWN))]);
        let _ = write!(line, " {more}");
    }
    line
}

/// The title of the confirmation before the window closes while jobs run.
pub fn quit_title() -> &'static str {
    t_static("quit-title")
}

/// The label of the button that ends the app while jobs run.
pub fn quit_button() -> &'static str {
    t_static("quit-button")
}

/// The question before the window closes while jobs run: they stop, and the next start offers
/// them again.
pub fn quit_with_jobs(n: usize) -> String {
    t("quit-with-jobs", &[("count", count(n))])
}

/// What the window shows while the jobs stop before the app ends.
pub fn closing() -> &'static str {
    t_static("quit-closing")
}

/// The title of the window of the jobs that the last run did not finish.
pub fn recovery_title(n: usize) -> String {
    t("recovery-title", &[("count", count(n))])
}

/// What running a recovered job again may do (carry-m1-03 2), and what closing the window
/// does: a job starts over from its first item and meets what it had finished, and a job
/// neither run again nor dropped is offered again at the next start. One sentence a line, so
/// that a line breaks between words.
pub fn recovery_note() -> &'static str {
    t_static("recovery-note")
}

/// One recovered job: `복사 · assets → /…/작업/백업 · 2026-10-28 22:08`, the target with the
/// middle of a long path left out, and `(시작 전)` when it never started and so changed nothing.
pub fn recovered_job(spec: &JobSpec, submitted: &str, started: bool) -> String {
    let folder = |path: &VPath| short_path(&path.to_string(), ROW_PLACE_CHARS);
    let mut line = format!("{} · {}", JobWork::of(spec).noun(), detail(spec, folder));
    if !submitted.is_empty() {
        let _ = write!(line, " · {submitted}");
    }
    if !started {
        line.push(' ');
        line.push_str(t_static("recovery-not-started"));
    }
    line
}

/// The title of the conflict window, by what is at the target already.
pub fn conflict_title(existing: EntryKind) -> &'static str {
    t_static(match existing {
        EntryKind::File => "conflict-title-file",
        EntryKind::Dir => "conflict-title-folder",
        EntryKind::Symlink | EntryKind::Other => "conflict-title-item",
    })
}

/// What the conflict is about: `‘설계안.pdf’가 C:\…\백업 에 이미 있습니다.`
pub fn conflict_description(name: &str, folder: &str) -> String {
    t(
        "conflict-description",
        &[
            ("name", name.into()),
            ("folder", folder.into()),
            ("ko-final", ko_final(name)),
        ],
    )
}

/// Whether `word` ends in a final consonant as it is read, as the Korean messages' `$ko-final`
/// selects it (`yes` or `no`) to choose the particle after it: `이` after a final consonant,
/// else `가`. Hangul is read by its last syllable, digits and Latin letters by their Korean
/// names (`1` 일, `L` 엘, `M` 엠, `N` 엔, `R` 알); anything else, and nothing, has none.
fn ko_final(word: &str) -> FluentValue<'static> {
    /// The first Hangul syllable, 가: the syllables after it come in runs of 28, the first of
    /// each with no final consonant.
    const FIRST_SYLLABLE: u32 = 0xAC00;
    let final_consonant = word.chars().last().is_some_and(|last| match last {
        '\u{AC00}'..='\u{D7A3}' => (u32::from(last) - FIRST_SYLLABLE) % 28 != 0,
        '0' | '1' | '3' | '6' | '7' | '8' => true,
        c if c.is_ascii_alphabetic() => matches!(c.to_ascii_lowercase(), 'l' | 'm' | 'n' | 'r'),
        _ => false,
    });
    FluentValue::from(if final_consonant { "yes" } else { "no" })
}

/// An item of `kind` as the conflict window's messages' `$kind` selects it: `file`, `folder`, or
/// `item` for a link or anything else.
fn entry_arg(kind: EntryKind) -> FluentValue<'static> {
    FluentValue::from(match kind {
        EntryKind::File => "file",
        EntryKind::Dir => "folder",
        EntryKind::Symlink | EntryKind::Other => "item",
    })
}

/// The head of a conflict card: the item that would come, by what `work` does with it (`복사해
/// 올 파일`, `옮겨 올 폴더`), or the one that is there (`이미 있는 파일`), by its `kind`.
pub fn conflict_card(incoming: bool, kind: EntryKind, work: JobWork) -> String {
    let kind = [("kind", entry_arg(kind))];
    if incoming {
        work.says("conflict-card-incoming", &kind)
    } else {
        t("conflict-card-existing", &kind)
    }
}

/// The hint of the skip choice of a conflict: what `work` leaves out (`이 파일은 복사하지
/// 않습니다`), by the `kind` of the item that would come.
pub fn skip_hint(kind: EntryKind, work: JobWork) -> String {
    work.says("conflict-skip", &[("kind", entry_arg(kind))])
}

/// The size of a conflict card: `1.2 MB (1,258,291 바이트)`, and the rounded size alone
/// below 1 KB.
pub fn exact_size(bytes: u64) -> String {
    let rounded = format_size(bytes);
    if bytes < 1024 {
        return rounded;
    }
    t(
        "size-exact",
        &[("size", rounded.into()), ("bytes", grouped(bytes).into())],
    )
}

/// `number` with a comma between each three digits: `1,258,291`.
pub fn grouped(number: u64) -> String {
    let digits = number.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// The hint of the overwrite choice, the file that would come named by what `work` does with
/// it, or why there is none: a file and a folder never replace each other.
pub fn overwrite_hint(can_overwrite: bool, work: JobWork) -> String {
    if can_overwrite {
        work.says("conflict-overwrite", &[])
    } else if work == JobWork::Restore {
        failure_reason(&FailureReason::RestoreNeverReplaces)
    } else {
        t_static("conflict-overwrite-kind-mismatch").to_owned()
    }
}

/// The hint of the rename choice, with the first numbered name the job tries.
pub fn rename_hint(numbered: &str) -> String {
    t("conflict-rename", &[("name", numbered.into())])
}

/// The warning while overwrite is chosen: what it does to the file that is there.
pub fn overwrite_warning(apply_all: bool) -> &'static str {
    t_static(if apply_all {
        "conflict-overwrite-warning-all"
    } else {
        "conflict-overwrite-warning"
    })
}

/// The hint of the conflict window's 더 새 파일만 덮어쓰기: what it does to this file, which it
/// replaces (`overwrites`) when the file that would come, named by what `work` does with it, is
/// the newer one. Empty where no overwrite is offered: the overwrite choice says why.
pub fn newer_hint(can_overwrite: bool, overwrites: bool, work: JobWork) -> String {
    if !can_overwrite {
        return String::new();
    }
    let key = if overwrites {
        "conflict-newer-overwrites"
    } else {
        "conflict-newer-skips"
    };
    work.says(key, &[])
}

/// The hint of the conflict window's 크기가 다를 때만 덮어쓰기: what it does to this file, which
/// it replaces (`overwrites`) when the two sizes differ. Empty where no overwrite is offered.
pub fn size_differs_hint(can_overwrite: bool, overwrites: bool) -> &'static str {
    match (can_overwrite, overwrites) {
        (false, _) => "",
        (true, true) => t_static("conflict-size-differs-overwrites"),
        (true, false) => t_static("conflict-size-same-skips"),
    }
}

/// The warning while a conditional overwrite is chosen for the remaining conflicts too: each
/// of them is decided by the same condition.
pub fn conditional_warning_all() -> &'static str {
    t_static("conflict-conditional-warning-all")
}

/// The title of the window that asks about an item a job could not process (XFER-09, 파일마다
/// 묻기), by what the job does: `복사 중 오류가 났습니다`.
pub fn error_ask_title(work: JobWork) -> String {
    work.says("error-ask-title", &[])
}

/// The item the error window asks about, by its whole path (the source, or the target when
/// there is no source), as [`failure_line`] names it; empty for a failure that names no item.
pub fn error_ask_item(failure: &ItemFailure) -> String {
    failure
        .source
        .as_ref()
        .or(failure.target.as_ref())
        .map(ToString::to_string)
        .unwrap_or_default()
}

/// Why the item the error window asks about failed, said while the job still waits for the
/// answer: the reasons the list at a job's end words as done ("…건너뛰었습니다", "…그대로
/// 두었습니다", [`failure_reason`]) say here only what is wrong, since whether the item is left
/// out is what the window asks. Any other failure reads as in that list ([`item_failure`]).
pub fn error_ask_reason(failure: &ItemFailure) -> String {
    let key = match &failure.reason {
        Some(FailureReason::TooDeep) => "error-ask-too-deep",
        Some(FailureReason::NotARegularFile) => "error-ask-not-a-regular-file",
        Some(FailureReason::SourceCutShort) => "error-ask-source-cut-short",
        Some(FailureReason::OntoItself) => "error-ask-onto-itself",
        Some(FailureReason::ChangedWhileMoved) => "error-ask-changed-while-moved",
        Some(FailureReason::OtherVolume) => "error-ask-other-volume",
        _ => return item_failure(failure),
    };
    t_static(key).to_owned()
}

/// What the error window says while 남은 오류에도 같은 선택 is on: why 다시 시도 is off.
pub fn error_ask_retry_once() -> &'static str {
    t_static("error-ask-retry-once")
}

/// The items still to come in a job that asks about a conflict.
pub fn items_left(n: u64) -> String {
    t("job-items-left", &[("count", number(n))])
}

/// The title of the window of the pre-flight check.
pub fn preflight_title() -> &'static str {
    t_static("preflight-title")
}

/// One line of the pre-flight window: the item's name and its [`preflight_issue`].
pub fn preflight_line(target: &VPath, issue: &IssueKind) -> String {
    t(
        "preflight-line",
        &[
            ("name", name_of(target).into()),
            ("issue", issue_kind(issue).into()),
            ("follows", preflight_follows(issue).into()),
        ],
    )
}

/// The title of the destination picker.
pub fn pick_title(mode: TransferMode) -> &'static str {
    t_static(match mode {
        TransferMode::Copy => "pick-title-copy",
        TransferMode::Move => "pick-title-move",
    })
}

/// The confirm button of the destination picker.
pub fn pick_confirm(mode: TransferMode) -> &'static str {
    t_static(match mode {
        TransferMode::Copy => "pick-confirm-copy",
        TransferMode::Move => "pick-confirm-move",
    })
}

/// What the picker copies or moves: `항목 3개 · 1.2 GB` for `n` items whose `files`
/// files are `bytes` together, `항목 3개 · 파일 1.2 MB와 폴더 1개` with `folders` folders
/// among them (whose size is not known before the job looks inside), `항목 2개 · 폴더 2개`
/// without files, and `항목 1개` for links alone, which have no size. Every unit of a size ends
/// in a vowel when read, so `와` always follows it.
pub fn pick_summary(n: usize, files: usize, bytes: u64, folders: usize) -> String {
    let what = match (files, folders) {
        (0, 0) => return item_count(n),
        (_, 0) => format_size(bytes),
        (0, folders) => t("pick-summary-folders", &[("count", count(folders))]),
        (_, folders) => t(
            "pick-summary-files-and-folders",
            &[
                ("size", format_size(bytes).into()),
                ("count", count(folders)),
            ],
        ),
    };
    format!("{} · {what}", item_count(n))
}

/// After the names of the first items the picker shows: how many more there are.
pub fn pick_more(n: usize) -> String {
    t("pick-more", &[("count", count(n))])
}

/// The heads of the picker's quick targets.
pub fn pick_opposite() -> &'static str {
    t_static("pick-opposite")
}

pub fn pick_tabs() -> &'static str {
    t_static("pick-tabs")
}

pub fn pick_recent() -> &'static str {
    t_static("pick-recent")
}

pub fn pick_favorites() -> &'static str {
    t_static("pick-favorites")
}

/// Why a folder cannot be chosen, or what to beware of when it is; empty for a folder with
/// no problem. A need for room is rounded up and the room free rounded down, so that the two
/// never read as the same number; `at_least` when folders are among the items, whose content
/// adds to the need.
pub fn pick_problem(validity: Validity, mode: TransferMode, at_least: bool) -> String {
    let key = match validity {
        Validity::Ok => return String::new(),
        Validity::IntoSource if mode == TransferMode::Copy => "pick-problem-into-copied",
        Validity::IntoSource => "pick-problem-into-moved",
        Validity::SameFolder => "pick-problem-same-folder",
        Validity::NotAFolder => "pick-problem-not-a-folder",
        Validity::ShellPlace | Validity::ForeignToTrash => "pick-problem-shell-place",
        Validity::NotWritable => "pick-problem-not-writable",
        Validity::NotEnoughSpace { need, available } => {
            let key = if at_least {
                "pick-problem-space-at-least"
            } else {
                "pick-problem-space"
            };
            return t(
                key,
                &[
                    ("need", format_size_rounded(need, Rounding::Up).into()),
                    (
                        "available",
                        format_size_rounded(available, Rounding::Down).into(),
                    ),
                ],
            );
        }
    };
    t_static(key).to_owned()
}

/// The free room at the picker's target: `남은 공간 212 GB`, rounded down like the status
/// bar's.
pub fn pick_space(available: u64) -> String {
    t(
        "pick-space",
        &[(
            "free",
            format_size_rounded(available, Rounding::Down).into(),
        )],
    )
}

/// The picker's space line while a worker looks at the target.
pub fn pick_space_pending() -> &'static str {
    t_static("pick-space-pending")
}

/// What the picker says while a worker looks at a path typed into it.
pub fn pick_path_pending() -> &'static str {
    t_static("pick-path-pending")
}

/// What the picker says while its path field holds a path typed and not entered: its button
/// is off until the path is entered (or another folder is chosen).
pub fn pick_path_unentered() -> &'static str {
    t_static("pick-path-unentered")
}

/// What the picker says while its path field has been cleared: there is nothing to enter, and
/// its button is off until a path is entered or a folder is chosen.
pub fn pick_path_blank() -> &'static str {
    t_static("pick-path-blank")
}

/// What the picker says while its policy overwrites: Enter does not confirm it.
pub fn pick_overwrite_by_button() -> &'static str {
    t_static("pick-overwrite-by-button")
}

/// Why a path typed into the picker cannot be the target.
pub fn pick_path_error(kind: ErrorKind) -> String {
    match kind {
        ErrorKind::NotADirectory => t_static("pick-path-not-a-folder").to_owned(),
        kind => error_kind(kind).to_owned(),
    }
}

/// What the picker's conflict policy does, said under its buttons; `true` with it when the
/// policy destroys something, which the picker then shows in the text's color and never
/// confirms with Enter alone.
pub fn pick_policy(policy: ConflictPolicy) -> (&'static str, bool) {
    // One sentence a line, so that a narrow window breaks the line between them.
    let (key, destroys) = match policy {
        ConflictPolicy::Overwrite => ("pick-policy-overwrite", true),
        ConflictPolicy::OverwriteIfNewer => ("pick-policy-if-newer", true),
        ConflictPolicy::OverwriteIfSizeDiffers => ("pick-policy-if-size-differs", true),
        ConflictPolicy::Skip => ("pick-policy-skip", false),
        ConflictPolicy::AutoRename => ("pick-policy-auto-rename", false),
        ConflictPolicy::Ask => ("pick-policy-ask", false),
    };
    (t_static(key), destroys)
}

/// What the picker's folder tree shows when its filter finds nothing: it looks only among
/// the folders listed so far.
pub fn pick_no_match() -> &'static str {
    t_static("pick-no-match")
}

/// What a drop at the place under the pointer would do, beside the pointer while a drag hovers
/// there (DND-03): `이동 → 백업`, with `folder`, the name of the folder the items would go
/// into, or why the keys held refuse the drop.
pub fn drop_badge(choice: DropChoice, folder: &str) -> String {
    let key = match choice {
        DropChoice::Do(DropEffect::Move) => "drop-badge-move",
        DropChoice::Do(DropEffect::Copy) => "drop-badge-copy",
        DropChoice::Ask => "drop-badge-ask",
        DropChoice::Do(DropEffect::Link) => "drop-badge-link",
        DropChoice::Refuse(Refusal::NotAllowed) => {
            return t_static("drop-badge-refused").to_owned();
        }
    };
    t(key, &[("folder", folder.into())])
}

/// The name of a kind of link in the item menu's 링크 만들기 ▸: the shortcut of Windows (바로
/// 가기), the alias of macOS (별칭), a symbolic link, a junction.
pub fn link_kind(kind: LinkKind, os: Os) -> &'static str {
    t_static(match (kind, os) {
        (LinkKind::Shortcut, Os::MacOs) => "link-kind-alias",
        (LinkKind::Shortcut, _) => "link-kind-shortcut",
        (LinkKind::Symlink, _) => "link-kind-symlink",
        (LinkKind::Junction, _) => "link-kind-junction",
    })
}

/// The word a new shortcut's name takes after the name of its target on `os`, which
/// [`rf_platform::link_name`] puts in the system's form: `바로 가기` on Windows (`보고서.docx -
/// 바로 가기.lnk`), `별칭` on macOS (`사진 별칭`).
pub fn link_name_word(os: Os) -> &'static str {
    t_static(match os {
        Os::MacOs => "link-name-alias",
        Os::Windows | Os::Other => "link-name-shortcut",
    })
}

/// The entry of a kind of link in 링크 만들기 ▸: its name, and for one that is off (`enabled`
/// false: a symbolic link without the rights for one) the reason, since a menu item has no
/// tooltip to carry it.
pub fn link_entry(kind: LinkKind, enabled: bool, os: Os) -> String {
    let name = link_kind(kind, os);
    if enabled {
        name.to_owned()
    } else {
        t("link-entry-off", &[("name", name.into())])
    }
}

/// What a click on a symbolic link that is off says: why, and what to do.
pub fn link_needs_privilege() -> &'static str {
    t_static("link-needs-privilege")
}

/// The notice of links made: `링크 3개를 만들었습니다`, with how many of them (`dangling`)
/// point to nothing: `링크 3개를 만들었습니다 · 대상이 없는 링크 1개`.
pub fn links_made(n: u64, dangling: usize) -> String {
    let made = t("links-made", &[("count", number(n))]);
    if dangling == 0 {
        made
    } else {
        format!(
            "{made} · {}",
            t("links-dangling", &[("count", count(dangling))])
        )
    }
}

/// The notice of a link that could not be made, with why: `링크를 만들지 못했습니다: 같은 이름의
/// 항목이 이미 있습니다`.
pub fn link_refused(failure: &ItemFailure) -> String {
    t("link-refused", &[("reason", item_failure(failure).into())])
}

/// The title of the picker that finds where links go (PICK-09).
pub fn pick_link_title() -> &'static str {
    t_static("pick-link-title")
}

/// The picker's confirm button when it finds where links go.
pub fn pick_link_confirm() -> &'static str {
    t_static("pick-link-confirm")
}

/// The title of the picker that finds where items of the trash are taken out to
/// (위치를 골라 꺼내기…).
pub fn pick_restore_title() -> &'static str {
    t_static("pick-restore-title")
}

/// The picker's line under its title when it finds where `n` items of the trash go.
pub fn pick_restore_summary(n: usize) -> String {
    t("pick-restore-summary", &[("count", count(n))])
}

/// The confirm button of the picker that takes items out of the trash.
pub fn pick_restore_confirm() -> &'static str {
    t_static("pick-restore-confirm")
}

/// The picker's line under its title when it finds where `n` links of `kind` go.
pub fn pick_link_summary(kind: LinkKind, n: usize, os: Os) -> String {
    t(
        "pick-link-summary",
        &[("count", count(n)), ("kind", link_kind(kind, os).into())],
    )
}

/// Why the picker cannot find a folder for links: one of the items, or a folder inside one, or
/// else what [`pick_problem`] says.
pub fn pick_link_problem(validity: Validity) -> String {
    match validity {
        Validity::IntoSource => t_static("pick-problem-into-linked").to_owned(),
        other => pick_problem(other, TransferMode::Copy, false),
    }
}

/// A drop from another app that left out `n` of its paths, which name no folder or file it
/// can take (a device or a volume of Windows, a relative path): the others go.
pub fn dropped_left_out(n: usize) -> String {
    t("dropped-left-out", &[("count", count(n))])
}

/// A drop from another app whose `n` paths all name no folder or file it can take: nothing
/// goes.
pub fn dropped_skipped(n: usize) -> String {
    t("dropped-skipped", &[("count", count(n))])
}

/// What the window says while a drag of another app whose paths all name no folder or file it
/// can take hovers over a place (the pointer says so too).
pub fn drop_no_paths() -> &'static str {
    t_static("drop-no-paths")
}

/// The head of the sidebar's group of the folders visited last.
pub fn recent_folders() -> &'static str {
    t_static("sidebar-recent")
}

/// What a drop between the favorites, or in the empty part of their list, would do, beside the
/// pointer (NAV-05, DND-01): the folders are kept as favorites, and nothing is moved.
pub fn drop_keeps() -> &'static str {
    t_static("drop-keeps")
}

/// Why the list of favorites takes the drag: it holds files, and the list takes folders only.
pub fn drop_only_folders() -> &'static str {
    t_static("drop-only-folders")
}

/// Why the list of favorites takes nothing of a drag whose keys ask for a link: it makes none
/// (OPS-08): the drop is refused, and the words say what is taken, without the link key.
pub fn drop_favorites_no_links() -> &'static str {
    t_static("drop-favorites-no-links")
}

/// The notice for a folder the user asked to keep as a favorite that is one already.
pub fn favorite_already(name: &str) -> String {
    t("favorite-already", &[("name", name.into())])
}

/// The title of the window that asks for a favorite's new name.
pub fn favorite_rename_title() -> &'static str {
    t_static("favorite-rename-title")
}

/// What the window says while a drag of items hovers over a place that does not take them
/// (the pointer says so too): why, for the reason `validity` gives.
pub fn drop_refused(validity: Validity) -> String {
    t_static(match validity {
        Validity::IntoSource => "drop-refused-into-source",
        Validity::SameFolder => "drop-refused-same-folder",
        Validity::NotAFolder => "drop-refused-not-a-folder",
        Validity::ShellPlace => "drop-refused-shell-place",
        Validity::ForeignToTrash => "drop-refused-foreign-to-trash",
        // A place that takes them with a warning is no refusal.
        Validity::Ok | Validity::NotWritable | Validity::NotEnoughSpace { .. } => "drop-refused",
    })
    .to_owned()
}

/// What the preview panel and quick look say of an item that cannot be shown: it could not be
/// read, or its content is broken (a damaged image, bytes not valid in their encoding).
pub fn preview_unavailable() -> &'static str {
    t_static("preview-unavailable")
}

/// What the preview says of an image file too large to decode (over 50 MB): its size shows
/// beside it.
pub fn preview_too_large() -> &'static str {
    t_static("preview-too-large")
}

/// How many items a folder in the preview shows: `항목 12개`, or `항목 10000개 이상` past the
/// count's cap (`more`).
pub fn preview_items(items: usize, more: bool) -> String {
    if more {
        t("preview-items-at-least", &[("count", count(items))])
    } else {
        item_count(items)
    }
}

/// The size of an image in the preview, as its file has it: `3200 × 800`.
pub fn preview_image_size(width: u32, height: u32) -> String {
    format!("{width} × {height}")
}

/// What the preview says of a text: the encoding it was read in, and when the file goes on past
/// its first 256 KB, that only those show: `EUC-KR · 앞 256 KB만 보입니다`.
pub fn preview_text_note(encoding: &str, truncated: bool) -> String {
    if truncated {
        let shown = format_size(u64::try_from(crate::preview::TEXT_BYTES).unwrap_or(u64::MAX));
        format!(
            "{encoding} · {}",
            t("preview-text-first", &[("size", shown.into())])
        )
    } else {
        encoding.to_owned()
    }
}

/// The facts the preview shows under an item: its kind, its size (`size`, for a file) and when
/// it was last changed (`modified`, when known): `PNG 이미지 · 1.2 MB · 2026-10-30 09:12`. Each
/// fact keeps its words together with no-break spaces, so that a narrow panel breaks the line
/// only between the facts (never between a date and its time).
pub fn preview_facts(kind: &str, size: Option<&str>, modified: Option<&str>) -> String {
    let mut parts = vec![kind];
    parts.extend(size);
    parts.extend(modified.filter(|modified| !modified.is_empty()));
    parts
        .iter()
        .map(|part| part.replace(' ', "\u{a0}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// What the preview says of a file stored online only (a cloud drive's placeholder): it is not
/// downloaded to be shown.
pub fn preview_online_only() -> &'static str {
    t_static("preview-online-only")
}

/// The notice when the preview panel is hidden because the window is too narrow for it; it
/// comes back when the window is wide enough.
pub fn preview_no_room() -> &'static str {
    t_static("preview-no-room")
}

/// The labels of the facts of the properties window (OPS-06), in their order.
pub fn fact_kind() -> &'static str {
    t_static("fact-kind")
}

pub fn fact_place() -> &'static str {
    t_static("fact-place")
}

pub fn fact_size() -> &'static str {
    t_static("fact-size")
}

/// What a folder holds, everything under it.
pub fn fact_contents() -> &'static str {
    t_static("fact-contents")
}

pub fn fact_link_target() -> &'static str {
    t_static("fact-link-target")
}

pub fn fact_created() -> &'static str {
    t_static("fact-created")
}

pub fn fact_modified() -> &'static str {
    t_static("fact-modified")
}

pub fn fact_accessed() -> &'static str {
    t_static("fact-accessed")
}

pub fn fact_attributes() -> &'static str {
    t_static("fact-attributes")
}

/// What a fact of the properties window says while a worker reads it.
pub fn reading() -> &'static str {
    t_static("properties-reading")
}

/// A time the system does not keep for the item (the creation time on some Linux file
/// systems).
pub fn unknown_time() -> &'static str {
    t_static("properties-unknown-time")
}

/// `n` as a count argument that shows as [`grouped`] does: a number below 1000, where there
/// is no comma to show, which gives English its plural; from 1000 on its grouped text, which
/// selects the `other` plural, its own.
fn grouped_count(n: u64) -> FluentValue<'static> {
    if n < 1000 {
        number(n)
    } else {
        FluentValue::from(grouped(n))
    }
}

/// The kinds of several items in the properties window, as File Explorer counts them: `파일 2개,
/// 폴더 1개` (a link or a special file is a file there).
pub fn chosen_kinds(files: u64, folders: u64) -> String {
    match (files, folders) {
        (files, 0) => t("properties-files", &[("files", grouped_count(files))]),
        (0, folders) => t("properties-folders", &[("folders", grouped_count(folders))]),
        (files, folders) => contents(files, folders),
    }
}

/// What the folders of the properties window hold: `파일 12,345개, 폴더 678개`.
pub fn contents(files: u64, folders: u64) -> String {
    t(
        "properties-files-and-folders",
        &[
            ("files", grouped_count(files)),
            ("folders", grouped_count(folders)),
        ],
    )
}

/// The size of the properties window: `1.2 MB (1,258,291 바이트)`, and `이상` after it while it
/// is not the whole size (`whole` off: the count goes on, stopped or failed).
pub fn size_fact(bytes: u64, whole: bool) -> String {
    let size = exact_size(bytes);
    if whole {
        size
    } else {
        t("properties-size-at-least", &[("size", size.into())])
    }
}

/// The read-only, hidden, system and archive attributes of the properties window: the names
/// of those that `flagged` (how many of `of` items have each) has, `(일부)` after one that
/// only some of them have, or `없음`.
pub fn attribute_names(flagged: &[usize; 4], of: usize) -> String {
    const NAMES: [&str; 4] = [
        "attribute-read-only",
        "attribute-hidden",
        "attribute-system",
        "attribute-archive",
    ];
    let names: Vec<String> = NAMES
        .iter()
        .zip(flagged)
        .filter(|(_, flagged)| **flagged > 0)
        .map(|(key, flagged)| {
            let name = t_static(key);
            if *flagged < of {
                t("attribute-partly", &[("name", name.into())])
            } else {
                name.to_owned()
            }
        })
        .collect();
    if names.is_empty() {
        t_static("attribute-none").to_owned()
    } else {
        names.join(", ")
    }
}

/// The count of the properties window's folders goes on.
pub fn counting() -> &'static str {
    t_static("properties-counting")
}

/// 중지 stopped the count: the size is what it had found.
pub fn count_stopped() -> &'static str {
    t_static("properties-count-stopped")
}

/// The count failed (its worker did).
pub fn count_failed() -> &'static str {
    t_static("properties-count-failed")
}

/// The folders under the properties window's that could not be listed.
pub fn unreadable_folders(n: u64) -> String {
    t(
        "properties-unreadable-folders",
        &[("count", grouped_count(n))],
    )
}

/// The folders under the properties window's that are on another file system (a drive mounted
/// in a folder), which the count does not enter.
pub fn folders_elsewhere(n: u64) -> String {
    t(
        "properties-folders-elsewhere",
        &[("count", grouped_count(n))],
    )
}

/// The size of the properties window's folder that could not be listed: why, in place of a size
/// of nothing.
pub fn folder_unreadable(kind: ErrorKind) -> String {
    with_reason("properties-folder-unreadable", kind)
}

/// A file whose checksum 중지 stopped before it was made.
pub fn hash_stopped() -> &'static str {
    t_static("hash-stopped")
}

/// 중지 stopped the checksums, and how to go on.
pub fn hashes_stopped() -> &'static str {
    t_static("hashes-stopped")
}

/// A file whose checksum waits for the files before it.
pub fn hash_waiting() -> &'static str {
    t_static("hash-waiting")
}

/// How far a file's checksum is: `42%`.
pub fn hash_percent(fraction: f32) -> String {
    // A whole percentage: the cast takes what the clamp left.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "0 to 100"
    )]
    let percent = (fraction.clamp(0.0, 1.0) * 100.0).floor() as u32;
    format!("{percent}%")
}

/// Why a file has no checksum.
pub fn hash_failure(failure: HashFailure) -> String {
    match failure {
        HashFailure::NotAFile => t_static("hash-not-a-file").to_owned(),
        HashFailure::Unreadable(kind) => with_reason("hash-unreadable", kind),
    }
}

/// A file stored online only, which reading would download, has no checksum.
pub fn hash_online_only() -> &'static str {
    t_static("hash-online-only")
}

/// The properties window shows no file to hash (folders, links, special files).
pub fn only_files_hashed() -> &'static str {
    t_static("hash-only-files")
}

/// Before an algorithm is chosen.
pub fn choose_algorithm() -> &'static str {
    t_static("hash-choose-algorithm")
}

/// How far the checksums of several files are: `계산 중… 파일 3/10`.
pub fn hashing_files(finished: usize, total: usize) -> String {
    t(
        "hash-progress",
        &[("done", count(finished)), ("total", count(total))],
    )
}

/// The files past the first `n` have no checksum.
pub fn hashed_first(n: usize) -> String {
    t(
        "hash-first-files",
        &[("count", grouped_count(u64::try_from(n).unwrap_or(u64::MAX)))],
    )
}

/// What the comparison with a pasted hash says when a checksum is it: `일치`, with the name of
/// its file among several (`일치: a.txt`) or how many are (`일치: 파일 2개`).
pub fn verdict_match(name: Option<&str>, n: usize) -> String {
    match name {
        Some(name) => t("hash-match-named", &[("name", name.into())]),
        None if n > 1 => t(
            "hash-match-files",
            &[("count", grouped_count(u64::try_from(n).unwrap_or(u64::MAX)))],
        ),
        None => t_static("hash-match").to_owned(),
    }
}

/// No checksum is the pasted hash.
pub fn verdict_mismatch() -> &'static str {
    t_static("hash-mismatch")
}

/// No checksum is the pasted hash so far, and some are still to come.
pub fn verdict_waiting() -> &'static str {
    t_static("hash-waiting-to-compare")
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::ffi::OsString;

    use rf_core::VPath;
    use rf_jobs::{
        CreateKind, DeleteSpec, ErrorPolicy, TransferMode, TransferOptions, TransferSpec,
    };

    use super::*;

    /// Whether `text` reads as Korean: it holds at least one Hangul syllable.
    fn is_korean(text: &str) -> bool {
        text.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c))
    }

    const ERROR_KINDS: [ErrorKind; 17] = [
        ErrorKind::NotFound,
        ErrorKind::PermissionDenied,
        ErrorKind::AlreadyExists,
        ErrorKind::InvalidPath,
        ErrorKind::NotADirectory,
        ErrorKind::IsADirectory,
        ErrorKind::DirectoryNotEmpty,
        ErrorKind::StorageFull,
        ErrorKind::ReadOnly,
        ErrorKind::CrossesDevices,
        ErrorKind::Busy,
        ErrorKind::TimedOut,
        ErrorKind::Interrupted,
        ErrorKind::FileTooLarge,
        ErrorKind::Unsupported,
        ErrorKind::Cancelled,
        ErrorKind::Other,
    ];

    #[test]
    fn cannot_open_names_the_reason() {
        assert_eq!(
            cannot_open(ErrorKind::PermissionDenied),
            "열 수 없습니다: 권한이 없습니다"
        );
        assert_eq!(
            cannot_open(ErrorKind::NotFound),
            "열 수 없습니다: 찾을 수 없습니다"
        );
        assert_eq!(
            background_failed(),
            "시스템 자원이 부족해 일을 시작하지 못했습니다. 잠시 뒤에 다시 해 보세요"
        );
    }

    #[test]
    fn the_status_line_lists_its_parts_in_the_order_of_their_worth() {
        const GB: u64 = 1 << 30;
        let facts = |selected, bytes, folders, filter, free| StatusFacts {
            items: 9,
            selected,
            selected_bytes: bytes,
            selected_folders: folders,
            filter,
            free,
        };
        // Nothing selected: the count, then the room.
        assert_eq!(
            status_parts(&facts(0, None, 0, None, Some(212 * GB))),
            ["항목 9개", "남은 공간 212 GB"]
        );
        // The room is left out while it is not known.
        assert_eq!(status_parts(&facts(0, None, 0, None, None)), ["항목 9개"]);
        // Files and a folder: the folder adds to the count but not to the size.
        assert_eq!(
            status_parts(&facts(3, Some(1_477_427), 1, None, Some(212 * GB))),
            [
                "3개 선택",
                "크기 1.4 MB",
                "+폴더 1개",
                "항목 9개",
                "남은 공간 212 GB"
            ]
        );
        // Files alone have no folder note; folders alone have no size.
        assert_eq!(
            status_parts(&facts(2, Some(0), 0, None, None)),
            ["2개 선택", "크기 0 바이트", "항목 9개"]
        );
        assert_eq!(
            status_parts(&facts(2, None, 2, None, None)),
            ["2개 선택", "폴더 2개", "항목 9개"]
        );
        // A link or another item alone: counted, with nothing to add up.
        assert_eq!(
            status_parts(&facts(1, None, 0, None, None)),
            ["1개 선택", "항목 9개"]
        );
        // A filter comes after the count and says how many rows show of how many.
        let filter = Some(StatusFilter {
            text: "*.jpg",
            shown: 4,
            of: 9,
        });
        assert_eq!(
            status_parts(&facts(0, None, 0, filter, Some(212 * GB))),
            ["항목 9개", "거르기: *.jpg · 4/9", "남은 공간 212 GB"]
        );
        // Every part at once, in order.
        assert_eq!(
            status_parts(&facts(3, Some(1_477_427), 1, filter, Some(212 * GB))),
            [
                "3개 선택",
                "크기 1.4 MB",
                "+폴더 1개",
                "항목 9개",
                "거르기: *.jpg · 4/9",
                "남은 공간 212 GB"
            ]
        );
        // An empty folder.
        let empty = StatusFacts {
            items: 0,
            ..facts(0, None, 0, None, None)
        };
        assert_eq!(status_parts(&empty), ["항목 0개"]);
        // The room is rounded down, as in the sidebar.
        let free = 4 * GB + 2 * GB / 3;
        assert_eq!(
            status_parts(&facts(0, None, 0, None, Some(free)))[1],
            "남은 공간 4.6 GB"
        );
        assert!(drive_space(free, 64 * GB).starts_with("4.6 GB 남음"));
    }

    #[test]
    fn a_long_filter_is_cut_at_a_whole_character() {
        let text = "아주아주아주아주아주아주아주아주 긴 거르기 글";
        let parts = status_parts(&StatusFacts {
            items: 3,
            selected: 0,
            selected_bytes: None,
            selected_folders: 0,
            filter: Some(StatusFilter {
                text,
                shown: 1,
                of: 3,
            }),
            free: None,
        });
        let filter = &parts[1];
        assert!(filter.starts_with("거르기: 아주아주"), "{filter}");
        assert!(filter.ends_with("… · 1/3"), "{filter}");
        assert!(text_cells(filter) < text_cells(text), "{filter}");
    }

    #[test]
    fn text_cells_count_a_hangul_syllable_as_two() {
        assert_eq!(text_cells(""), 0);
        assert_eq!(text_cells("abc 12"), 6);
        assert_eq!(text_cells("선택 2개"), 4 + 1 + 1 + 2);
        assert_eq!(text_cells("…"), 1);
    }

    #[test]
    fn a_narrow_status_line_drops_its_parts_from_the_right() {
        let parts = [
            "3개 선택",
            "크기 1.4 MB",
            "+폴더 1개",
            "항목 9개",
            "남은 공간 212 GB",
        ];
        let at = |cells| fit_status(&parts, cells);
        let all = "3개 선택 · 크기 1.4 MB · +폴더 1개 · 항목 9개 · 남은 공간 212 GB";
        assert_eq!(text_cells(all), 64);
        assert_eq!(at(64), all);
        assert_eq!(at(1000), all);
        // The room goes first, then the count, then the folders, then the size.
        assert_eq!(at(63), "3개 선택 · 크기 1.4 MB · +폴더 1개 · 항목 9개");
        assert_eq!(at(45), "3개 선택 · 크기 1.4 MB · +폴더 1개 · 항목 9개");
        assert_eq!(at(44), "3개 선택 · 크기 1.4 MB · +폴더 1개");
        assert_eq!(at(34), "3개 선택 · 크기 1.4 MB · +폴더 1개");
        assert_eq!(at(33), "3개 선택 · 크기 1.4 MB");
        assert_eq!(at(22), "3개 선택 · 크기 1.4 MB");
        assert_eq!(at(21), "3개 선택");
        // What comes first stays, however narrow, and is never cut in the middle.
        assert_eq!(at(8), "3개 선택");
        assert_eq!(at(1), "3개 선택");
        assert_eq!(at(0), "3개 선택");
        assert_eq!(fit_status(&[] as &[&str], 10), "");
    }

    #[test]
    fn the_status_line_of_a_filtered_folder_fits_a_narrow_pane() {
        const GB: u64 = 1 << 30;
        let parts = status_parts(&StatusFacts {
            items: 4,
            selected: 0,
            selected_bytes: None,
            selected_folders: 0,
            filter: Some(StatusFilter {
                text: "*.jpg",
                shown: 4,
                of: 9,
            }),
            free: Some(212 * GB),
        });
        let line = |cells| fit_status(&parts, cells);
        assert_eq!(
            line(80),
            "항목 4개 · 거르기: *.jpg · 4/9 · 남은 공간 212 GB"
        );
        // 8 + 3 + 19 = 30 cells for the first two.
        assert_eq!(line(40), "항목 4개 · 거르기: *.jpg · 4/9");
        assert_eq!(line(29), "항목 4개");
    }

    #[test]
    fn history_rows_say_what_was_done() {
        let names = |from: &str, to: &str| (path(from), path(to));
        let (from, to) = names("a.txt", "b.txt");
        let renamed = UndoRecord::Renamed { from, to };
        assert_eq!(
            history_action(&renamed, false),
            "이름 바꾸기: a.txt → b.txt"
        );
        // Doing it again after an undo renames the other way round: the record is the undo's.
        assert_eq!(history_action(&renamed, true), "이름 바꾸기: b.txt → a.txt");

        let moved = |count: usize| UndoRecord::Moved {
            moves: (0..count)
                .map(|i| rf_jobs::MovedItem {
                    from: path(&format!("src/f{i}.txt")),
                    to: path(&format!("dest/f{i}.txt")),
                })
                .collect(),
            created_dirs: Vec::new(),
            removed_dirs: Vec::new(),
            overwritten: 0,
            copied_dirs: Vec::new(),
        };
        assert_eq!(history_action(&moved(3), false), "이동: 3개 항목 → dest");
        assert_eq!(history_action(&moved(1), false), "이동: f0.txt → dest");
        assert_eq!(history_action(&moved(3), true), "이동: 3개 항목 → src");

        let copied = UndoRecord::Copied {
            created: vec![rf_jobs::CreatedItem {
                path: path("dest/c.txt"),
                is_dir: false,
                size: 1,
                modified: None,
                identity: None,
            }],
            overwritten: 0,
            at: None,
        };
        assert_eq!(history_action(&copied, false), "복사: c.txt → dest");
        // Doing a copy's undo again takes the copy out of the trash.
        assert_eq!(history_action(&copied, true), "휴지통으로: c.txt");

        let made = |kind| UndoRecord::Created {
            path: path("새 폴더"),
            kind,
        };
        assert_eq!(
            history_action(&made(CreateKind::Dir), false),
            "새 폴더: 새 폴더"
        );
        assert_eq!(
            history_action(&made(CreateKind::File), false),
            "새 파일: 새 폴더"
        );
        // Doing the creation again after its undo is a record of a removal.
        let removed = UndoRecord::Removed {
            path: path("새 폴더"),
            kind: CreateKind::Dir,
        };
        assert_eq!(history_action(&removed, true), "새 폴더: 새 폴더");
        assert_eq!(
            history_action(&made(CreateKind::Dir), true),
            "삭제: 새 폴더"
        );

        let trashed = |count: usize| UndoRecord::Trashed {
            items: (0..count)
                .map(|i| rf_jobs::TrashedItem {
                    original: path(&format!("t{i}.txt")),
                    location: None,
                    stamp: None,
                })
                .collect(),
        };
        assert_eq!(history_action(&trashed(1), false), "휴지통으로: t0.txt");
        assert_eq!(history_action(&trashed(4), false), "휴지통으로: 4개 항목");
        assert_eq!(
            history_action(&trashed(4), true),
            "휴지통에서 꺼내기: 4개 항목"
        );

        let links = UndoRecord::LinksMade {
            at: vec![(path("l.lnk"), LinkKind::Shortcut, path("t"))],
        };
        assert_eq!(history_action(&links, false), "링크 만들기: l.lnk");
        assert_eq!(history_action(&links, true), "링크 지우기: l.lnk");
    }

    #[test]
    fn a_long_name_in_a_history_row_is_cut_in_the_middle() {
        let long = "가".repeat(40);
        let renamed = UndoRecord::Renamed {
            from: path(&long),
            to: path("b.txt"),
        };
        let text = history_action(&renamed, false);
        assert!(text.starts_with("이름 바꾸기: 가가"), "{text}");
        assert!(text.contains('…') && text.ends_with(" → b.txt"), "{text}");
        assert!(text_cells(&text) < 70, "{text}");
    }

    #[test]
    fn history_notes_mark_the_next_row_and_say_why_a_row_waits_or_may_not_run() {
        assert_eq!(history_note(false, true, None), "다음에 되돌릴 작업");
        assert_eq!(history_note(true, true, None), "다음에 다시 실행할 작업");
        assert_eq!(history_note(false, false, None), undo_order());
        assert_eq!(
            history_note(true, false, None),
            "먼저 다시 실행할 작업이 있습니다"
        );
        assert_eq!(
            history_note(false, true, Some(Stale::Missing)),
            "항목이 목록에 보이지 않아 되돌리지 못할 수 있습니다"
        );
        assert_eq!(
            history_note(false, true, Some(Stale::Changed)),
            "복사한 뒤 항목이 바뀌어 되돌리지 못할 수 있습니다"
        );
        assert_eq!(
            history_note(true, true, Some(Stale::Missing)),
            "항목이 목록에 보이지 않아 다시 실행하지 못할 수 있습니다"
        );
        // Why it may not run comes before where it stands in the order.
        assert_eq!(
            history_note(false, false, Some(Stale::Missing)),
            "항목이 목록에 보이지 않아 되돌리지 못할 수 있습니다"
        );
        assert_eq!(history_button(false), "되돌리기");
        assert_eq!(history_button(true), "다시 실행");
        assert_eq!(history_head(3), "되돌리기 기록 3개");
        assert_eq!(history_undo_section(2), "되돌릴 수 있음 2개");
        assert_eq!(history_redo_section(1), "다시 실행할 수 있음 1개");
    }

    #[test]
    fn history_times_show_the_day_only_when_it_is_not_today() {
        use std::time::{Duration, SystemTime};
        let seoul = TimeZone::get("Asia/Seoul").expect("Asia/Seoul is in the database");
        // 2026-10-30 05:32:00 UTC is 14:32 in Seoul.
        let at = SystemTime::UNIX_EPOCH + Duration::from_mins(29_888_972);
        let same_day = at + Duration::from_hours(3);
        let next_day = at + Duration::from_hours(24);
        assert_eq!(history_time(at, same_day, &seoul), "14:32");
        assert_eq!(history_time(at, next_day, &seoul), "10월 30일 14:32");
        // A row that was never stamped shows no time.
        assert_eq!(history_time(SystemTime::UNIX_EPOCH, same_day, &seoul), "");
    }

    #[test]
    fn places_have_sidebar_names() {
        let names: Vec<&str> = KnownFolder::ALL.into_iter().map(favorite).collect();
        assert_eq!(
            names,
            [
                "홈",
                "바탕 화면",
                "문서",
                "다운로드",
                "사진",
                "음악",
                "동영상"
            ]
        );
        assert_eq!(drive_letter('C'), "로컬 디스크 (C:)");
        assert_eq!(drive_letter('D'), "드라이브 (D:)");
        assert_eq!(system_disk(), "시스템 디스크");
    }

    #[test]
    fn drive_space_matches_mockup() {
        const GB: u64 = 1 << 30;
        assert_eq!(drive_space(128 * GB, 512 * GB), "128 GB 남음 / 512 GB");
        assert_eq!(drive_space(212 * GB, 1024 * GB), "212 GB 남음 / 1 TB");
        // Free space just below 10 GB does not round up to 10 GB.
        assert_eq!(drive_space(10 * GB - 1, 64 * GB), "9.9 GB 남음 / 64 GB");
    }

    #[test]
    fn a_gone_start_folder_is_named_by_its_end() {
        // Eight folders of ten characters: far past the 60 a notice shows of a path, wherever
        // the system's temporary folder is.
        let names: Vec<String> = (0..8).map(|i| format!("긴-폴더-이름-{i:02}")).collect();
        let gone = names
            .iter()
            .try_fold(path("작업"), |folder, name| folder.join(name))
            .expect("plain names");
        let notice = start_folder_gone(&gone);
        let shown = notice
            .strip_prefix("열려던 폴더를 찾을 수 없습니다: ")
            .unwrap_or_else(|| panic!("{notice}"));
        // The middle goes; the root, and the last five folders that fit, stay.
        let sep = std::path::MAIN_SEPARATOR.to_string();
        let end = names[3..].join(&sep);
        assert!(shown.ends_with(&format!("…{sep}{end}")), "{shown}");
        assert!(!shown.contains(&names[2]), "{shown}");
        assert!(shown.chars().count() <= 60, "{shown}");
    }

    #[test]
    fn every_error_kind_has_words() {
        assert_eq!(error_kind(ErrorKind::NotFound), "찾을 수 없습니다");
        assert_eq!(error_kind(ErrorKind::PermissionDenied), "권한이 없습니다");
        assert_eq!(
            error_kind(ErrorKind::AlreadyExists),
            "같은 이름의 항목이 이미 있습니다"
        );
        assert_eq!(
            error_kind(ErrorKind::StorageFull),
            "디스크 공간이 부족합니다"
        );
        assert_eq!(error_kind(ErrorKind::ReadOnly), "읽기 전용 위치입니다");
        assert_eq!(error_kind(ErrorKind::Busy), "다른 프로그램이 사용 중입니다");
        assert_eq!(error_kind(ErrorKind::Other), "알 수 없는 오류가 났습니다");
        // The item that is no folder may be the last one of the path, too.
        assert_eq!(
            error_kind(ErrorKind::NotADirectory),
            "경로에 폴더가 아닌 항목이 있습니다"
        );

        let mut seen = HashSet::new();
        for kind in ERROR_KINDS {
            let words = error_kind(kind);
            assert!(is_korean(words), "{kind:?}: {words:?}");
            assert!(!words.contains(". "), "{kind:?} is one sentence: {words:?}");
            assert!(seen.insert(words), "{kind:?} shares its words: {words:?}");
        }
    }

    fn all_reasons() -> Vec<FailureReason> {
        vec![
            FailureReason::IntoItself,
            FailureReason::NoTargetFolder,
            FailureReason::TargetNotAFolder,
            FailureReason::TooDeep,
            FailureReason::LinkLoop,
            FailureReason::TooManyLinks,
            FailureReason::LinkChanged,
            FailureReason::AbsoluteLink,
            FailureReason::NotARegularFile,
            FailureReason::SourceCutShort,
            FailureReason::KindMismatch,
            FailureReason::NoFreeName,
            FailureReason::OntoItself,
            FailureReason::ChangedWhileMoved,
            FailureReason::KeptAsTargetFolder,
            FailureReason::CannotTellOwnFolder,
            FailureReason::OtherVolume,
            FailureReason::TrashRefused,
            FailureReason::DeletedNotTrashed,
            FailureReason::ChangedWhileDeleted,
            FailureReason::Overwritten,
            FailureReason::ChangedSinceCopy,
            FailureReason::ChangedSinceCreated,
            FailureReason::NotEmpty,
            FailureReason::TrashLocationUnknown,
            FailureReason::AcrossLocations,
            FailureReason::TempLeftBehind,
            FailureReason::Issue(IssueKind::ReservedName),
        ]
        .into_iter()
        .chain(
            [
                (Compared::Same, Compared::Missing),
                (Compared::Same, Compared::Changed),
                (Compared::Same, Compared::Unknown),
                (Compared::Changed, Compared::Unknown),
                (Compared::Missing, Compared::Unknown),
                (Compared::Unknown, Compared::Unknown),
            ]
            .map(|(copy, original)| FailureReason::CopyKept(CopyFound { copy, original })),
        )
        .chain([FailureReason::CopyPartlyDeleted])
        .collect()
    }

    #[test]
    fn every_failure_reason_has_words() {
        assert_eq!(
            failure_reason(&FailureReason::IntoItself),
            "폴더를 자기 안으로 복사하거나 옮길 수 없습니다"
        );
        assert_eq!(
            failure_reason(&FailureReason::TrashRefused),
            "휴지통으로 보낼 수 없습니다"
        );
        assert_eq!(
            failure_reason(&FailureReason::OtherVolume),
            "다른 드라이브의 항목이라 건너뛰었습니다"
        );
        assert_eq!(
            failure_reason(&FailureReason::ChangedSinceCopy),
            "복사한 뒤 바뀌어 되돌리지 않았습니다"
        );
        // A copy that a delete of copies kept, by why it stays.
        let kept =
            |copy, original| failure_reason(&FailureReason::CopyKept(CopyFound { copy, original }));
        assert_eq!(
            kept(Compared::Same, Compared::Changed),
            "원본이 바뀌어 복사본을 지우지 않았습니다"
        );
        assert_eq!(
            kept(Compared::Changed, Compared::Unknown),
            "복사본이 바뀌어 지우지 않았습니다"
        );
        // The engine cannot tell its own delete from another change: it may have been one.
        assert_eq!(
            failure_reason(&FailureReason::CopyPartlyDeleted),
            "복사본의 일부를 이미 지웠을 수 있어 나머지는 안전하게 비교할 수 없으므로 남겨 두었습니다"
        );

        let mut seen = HashSet::new();
        for reason in all_reasons() {
            let words = failure_reason(&reason);
            assert!(is_korean(&words), "{reason:?}: {words:?}");
            assert!(
                seen.insert(words.clone()),
                "{reason:?} shares its words: {words:?}"
            );
        }
        assert_eq!(
            failure_reason(&FailureReason::LinkLoop),
            "바로 가기가 자기를 담은 폴더를 다시 가리켜 따라갈 수 없습니다"
        );
        assert_eq!(
            failure_reason(&FailureReason::SourceCutShort),
            "복사하는 동안 원본 파일이 줄어들어 사본을 만들지 않았습니다"
        );
    }

    #[test]
    fn a_refused_name_says_it_was_not_used() {
        // A rename or a create refuses every name with an issue, warnings too.
        let reserved = failure_reason(&FailureReason::Issue(IssueKind::ReservedName));
        assert_eq!(
            reserved,
            "Windows가 장치 이름으로 쓰는 이름입니다(CON, NUL 등). 이 이름은 쓸 수 없습니다"
        );
        assert!(!reserved.contains("열기 어려울"), "{reserved}");
        assert_eq!(
            failure_reason(&FailureReason::Issue(IssueKind::TrailingDotOrSpace)),
            "이름이 마침표나 공백으로 끝납니다. 이 이름은 쓸 수 없습니다"
        );
        assert_eq!(
            failure_reason(&FailureReason::Issue(IssueKind::NameTooLong {
                len: 300,
                max: 255
            })),
            "이름이 너무 깁니다. 더 짧게 바꿔 주세요"
        );
        assert_eq!(
            failure_reason(&FailureReason::Issue(IssueKind::PathTooLong {
                len: 300,
                max: 260
            })),
            "경로가 너무 깁니다. 더 얕은 폴더나 짧은 이름을 써 주세요"
        );
        // Every issue starts with its fact.
        for issue in all_issues() {
            let words = failure_reason(&FailureReason::Issue(issue.clone()));
            assert!(words.starts_with(&issue_kind(&issue)), "{issue:?}: {words}");
            assert!(words.len() > issue_kind(&issue).len(), "{issue:?}: {words}");
        }
    }

    #[test]
    fn the_review_says_what_follows_from_an_issue() {
        assert_eq!(
            preflight_issue(&IssueKind::ReservedName),
            "Windows가 장치 이름으로 쓰는 이름입니다(CON, NUL 등). 그대로 두면 Windows에서 열기 어려울 수 있습니다"
        );
        assert_eq!(
            preflight_issue(&IssueKind::TrailingDotOrSpace),
            "이름이 마침표나 공백으로 끝납니다. 그대로 두면 Windows에서 열기 어려울 수 있습니다"
        );
        assert_eq!(
            preflight_issue(&IssueKind::NameTooLong { len: 300, max: 255 }),
            "이름이 너무 깁니다. 이 항목은 대상 위치에 둘 수 없습니다"
        );
        let space = preflight_issue(&IssueKind::NotEnoughSpace {
            needed: 2048,
            available: 1024,
        });
        assert!(
            space.ends_with("계속하면 공간이 모자라 중간에 멈출 수 있습니다"),
            "{space}"
        );
        let mut seen = HashSet::new();
        for issue in all_issues() {
            let words = preflight_issue(&issue);
            assert!(words.starts_with(&issue_kind(&issue)), "{issue:?}: {words}");
            assert!(
                seen.insert(words.clone()),
                "{issue:?} shares its words: {words:?}"
            );
        }
    }

    fn all_issues() -> [IssueKind; 8] {
        [
            IssueKind::InvalidCharacter(':'),
            IssueKind::ReservedName,
            IssueKind::TrailingDotOrSpace,
            IssueKind::NameTooLong { len: 300, max: 255 },
            IssueKind::PathTooLong { len: 300, max: 260 },
            IssueKind::FileTooLarge {
                size: 5 * 1024 * 1024 * 1024,
                max: 4 * 1024 * 1024 * 1024 - 1,
            },
            IssueKind::CaseCollision {
                other: OsString::from("Readme.md"),
            },
            IssueKind::NotEnoughSpace {
                needed: 3 * 1024 * 1024,
                available: 1024 * 1024,
            },
        ]
    }

    #[test]
    fn every_issue_kind_has_words() {
        let issues = all_issues();
        let mut seen = HashSet::new();
        for issue in &issues {
            let words = issue_kind(issue);
            assert!(is_korean(&words), "{issue:?}: {words:?}");
            assert!(
                seen.insert(words.clone()),
                "{issue:?} shares its words: {words:?}"
            );
        }
        // The details of the issue are part of the words.
        assert!(issue_kind(&issues[0]).contains("‘:’"));
        // The issue states a fact; what follows from it depends on the job.
        assert_eq!(
            issue_kind(&issues[1]),
            "Windows가 장치 이름으로 쓰는 이름입니다(CON, NUL 등)"
        );
        // A length may count bytes or UTF-16 units, or the temporary path: no numbers.
        assert_eq!(issue_kind(&issues[3]), "이름이 너무 깁니다");
        assert_eq!(issue_kind(&issues[4]), "경로가 너무 깁니다");
        assert!(issue_kind(&issues[5]).contains("(5 GB, 최대 3.9 GB)"));
        assert!(issue_kind(&issues[6]).contains("Readme.md"));
        assert!(issue_kind(&issues[7]).contains("(필요 3 MB, 남은 공간 1 MB)"));
        // A size over a limit never reads as the limit: 4 GiB on FAT32.
        let fat32 = issue_kind(&IssueKind::FileTooLarge {
            size: 4 * 1024 * 1024 * 1024,
            max: 4 * 1024 * 1024 * 1024 - 1,
        });
        assert!(fat32.contains("(4 GB, 최대 3.9 GB)"), "{fat32}");
        // Nor a need a few bytes over what is free.
        let space = issue_kind(&IssueKind::NotEnoughSpace {
            needed: 1_288_490_192,
            available: 1_288_490_189,
        });
        assert!(space.contains("(필요 1.3 GB, 남은 공간 1.2 GB)"), "{space}");
        // A control character is spelled out, not printed.
        assert!(issue_kind(&IssueKind::InvalidCharacter('\u{1}')).contains("U+0001"));
    }

    fn path(text: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(text)).expect("a local path")
    }

    #[test]
    fn work_follows_the_spec() {
        let transfer = |mode| {
            JobSpec::Transfer(TransferSpec {
                mode,
                items: Vec::new(),
                options: TransferOptions::default(),
            })
        };
        let delete = |method| {
            JobSpec::Delete(DeleteSpec {
                paths: Vec::new(),
                method,
                errors: ErrorPolicy::default(),
            })
        };
        assert_eq!(JobWork::of(&transfer(TransferMode::Copy)), JobWork::Copy);
        assert_eq!(JobWork::of(&transfer(TransferMode::Move)), JobWork::Move);
        assert_eq!(JobWork::of(&delete(DeleteMethod::Trash)), JobWork::Trash);
        assert_eq!(
            JobWork::of(&delete(DeleteMethod::Permanent)),
            JobWork::Delete
        );
        let rename = JobSpec::Rename {
            from: path("a"),
            to: path("b"),
        };
        assert_eq!(JobWork::of(&rename), JobWork::Rename);
        let create = JobSpec::Create {
            parent: path("a"),
            name: OsString::from("새 폴더"),
            kind: CreateKind::Dir,
        };
        assert_eq!(JobWork::of(&create), JobWork::Create);
        // A redo is an undo spec too; only the caller knows it asked for a redo.
        let undo = JobSpec::Undo(UndoRecord::Trashed { items: Vec::new() });
        assert_eq!(JobWork::of(&undo), JobWork::Undo);
    }

    const WORKS: [JobWork; 10] = [
        JobWork::Copy,
        JobWork::Move,
        JobWork::Trash,
        JobWork::Delete,
        JobWork::Rename,
        JobWork::Create,
        JobWork::Restore,
        JobWork::Undo,
        JobWork::Redo,
        JobWork::Other,
    ];

    #[test]
    fn a_job_s_row_names_its_file_and_a_waiting_one_what_it_will_do() {
        let file =
            |name: &str| VPath::local(std::env::temp_dir().join(name)).expect("a local path");
        // The name alone, whatever its folders.
        assert_eq!(job_current(&file("보고서.pdf")), "보고서.pdf");
        // A name as wide as the line stays whole.
        let fits = format!("{}.txt", "a".repeat(JOB_FILE_CELLS - 4));
        assert_eq!(job_current(&file(&fits)), fits);
        // A longer one loses its middle: the beginning and the extension stay.
        let long = format!("{}.tar.gz", "backup-2026-11-18-".repeat(6));
        let shown = job_current(&file(&long));
        assert!(text_cells(&shown) <= JOB_FILE_CELLS, "{shown}");
        assert!(shown.starts_with("backup-2026-11-18-"), "{shown}");
        assert!(shown.ends_with(".tar.gz"), "{shown}");
        assert_eq!(shown.matches('…').count(), 1, "{shown}");
        // Hangul counts two cells a syllable.
        let korean = format!("{}.hwp", "회의록".repeat(15));
        let shown = job_current(&file(&korean));
        assert!(text_cells(&shown) <= JOB_FILE_CELLS, "{shown}");
        assert!(
            shown.starts_with("회의록") && shown.ends_with("회의록.hwp"),
            "{shown}"
        );

        // A job that waits says what it will do, and on what.
        assert_eq!(
            queued_detail(JobWork::Copy, "assets → 백업"),
            "복사 · assets → 백업"
        );
        assert_eq!(
            queued_detail(JobWork::Move, "항목 3개 → 백업"),
            "이동 · 항목 3개 → 백업"
        );
        assert_eq!(
            queued_detail(JobWork::Trash, "a.txt"),
            "휴지통으로 보내기 · a.txt"
        );
        // One whose items are not known yet says the work alone.
        assert_eq!(queued_detail(JobWork::Other, ""), "작업");
        for work in WORKS {
            assert!(is_korean(&queued_detail(work, "")), "{work:?}");
        }

        // The window of the job panel is named after the app, wherever the name comes from.
        assert_eq!(
            job_window_title(),
            format!("작업 - {}", rf_core::identity::DISPLAY_NAME)
        );
        assert!(is_korean(job_window_failed()));
    }

    #[test]
    fn job_titles_follow_state() {
        let running = [
            (JobWork::Copy, "복사 중"),
            (JobWork::Move, "이동 중"),
            (JobWork::Trash, "휴지통으로 보내는 중"),
            (JobWork::Delete, "삭제 중"),
            (JobWork::Rename, "이름 바꾸는 중"),
            (JobWork::Create, "만드는 중"),
            (JobWork::Undo, "되돌리는 중"),
            (JobWork::Redo, "다시 실행하는 중"),
            (JobWork::Other, "작업 중"),
        ];
        for (work, title) in running {
            assert_eq!(job_title(work, JobState::Running), title);
            assert_eq!(job_title(work, JobState::Planning), title);
            // A question does not change the title; the panel shows the question.
            assert_eq!(job_title(work, JobState::Waiting), title);
            assert_eq!(job_title(work, JobState::Paused), "일시정지됨");
            assert_eq!(job_title(work, JobState::Queued), "대기 중");
        }
        assert_eq!(
            job_title(JobWork::Move, JobState::Finished(Outcome::Completed)),
            "이동했습니다"
        );
    }

    #[test]
    fn job_progress_shows_what_is_known() {
        assert_eq!(
            job_progress(3, 12, 48 * 1024 * 1024, Some(Duration::from_secs(20)), 1),
            "3 / 12 항목 · 48 MB/s · 남은 시간 약 20초 · 대기 중인 작업 1개"
        );
        // Before the speed is measured there is no time left either.
        assert_eq!(
            job_progress(0, 12, 0, Some(Duration::from_secs(20)), 0),
            "0 / 12 항목"
        );
        assert_eq!(job_progress(0, 0, 0, None, 2), "대기 중인 작업 2개");
        assert_eq!(job_progress(0, 0, 0, None, 0), "");
        // Never more done than planned.
        assert_eq!(job_progress(13, 12, 0, None, 0), "12 / 12 항목");
        assert_eq!(
            time_left(Duration::from_millis(19_200)),
            "남은 시간 약 20초"
        );
        assert_eq!(time_left(Duration::from_secs(150)), "남은 시간 약 3분");
        assert_eq!(time_left(Duration::from_hours(1)), "남은 시간 약 1시간");
        assert_eq!(time_left(Duration::from_mins(65)), "남은 시간 약 1시간 5분");
    }

    #[test]
    fn a_finished_job_tells_what_it_did_and_how_long_it_took() {
        assert_eq!(
            job_report(
                JobWork::Copy,
                12,
                1,
                0,
                Some(48 * 1024 * 1024),
                Duration::from_secs(80)
            ),
            "복사 12개 · 건너뜀 1개 · 오류 0개 · 평균 48 MB/s · 걸린 시간 1분 20초"
        );
        assert_eq!(
            job_report(JobWork::Delete, 3, 0, 2, None, Duration::from_millis(400)),
            "삭제 3개 · 건너뜀 0개 · 오류 2개 · 걸린 시간 1초 미만"
        );
        for (secs, text) in [
            (0, "1초 미만"),
            (12, "12초"),
            (60, "1분"),
            (80, "1분 20초"),
            (3600, "1시간"),
            (3900, "1시간 5분"),
        ] {
            assert_eq!(took(Duration::from_secs(secs)), text);
        }
    }

    #[test]
    fn a_job_that_ended_unseen_is_told_unless_it_was_cancelled() {
        assert_eq!(
            job_notice(JobWork::Copy, Outcome::Completed, 12, 0).as_deref(),
            Some("복사를 마쳤습니다 · 항목 12개")
        );
        assert_eq!(
            job_notice(JobWork::Move, Outcome::CompletedWithErrors, 5, 2).as_deref(),
            Some("이동을 마쳤지만 항목 2개를 처리하지 못했습니다")
        );
        assert_eq!(
            job_notice(JobWork::Trash, Outcome::Aborted, 1, 0).as_deref(),
            Some("문제가 생겨 휴지통으로 보내기를 중단했습니다")
        );
        assert_eq!(job_notice(JobWork::Copy, Outcome::Cancelled, 1, 0), None);
    }

    #[test]
    fn the_tray_is_named_as_the_system_names_it() {
        assert_eq!(continue_in_tray(Os::Windows), "트레이에서 계속");
        assert_eq!(continue_in_tray(Os::MacOs), "메뉴 막대에서 계속");
        assert_eq!(
            quit_or_tray(Os::Windows),
            "작업이 끝날 때까지 트레이에서 계속할까요?"
        );
        assert_eq!(
            quit_or_tray(Os::MacOs),
            "작업이 끝날 때까지 메뉴 막대에서 계속할까요?"
        );
    }

    #[test]
    fn the_tray_tooltip_shows_how_far_the_jobs_have_got() {
        let row = |title: &str, fraction: f32, progress: &str| JobRow {
            id: rf_jobs::JobId(1),
            title: title.to_owned(),
            detail: String::new(),
            fraction,
            progress: progress.to_owned(),
            paused: false,
            waiting: false,
            current: String::new(),
            speeds: None,
            queued: None,
        };
        assert_eq!(tray_status("앱", &[]), "앱\n대기 중");
        assert_eq!(
            tray_status(
                "앱",
                &[
                    row("복사 중", 0.257, "3 / 12 항목 · 대기 중인 작업 1개"),
                    row("일시정지됨", 1.0, "")
                ]
            ),
            "앱\n복사 중 25%\n일시정지됨 100%\n3 / 12 항목 · 대기 중인 작업 1개"
        );
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(0.999), "99%");
        assert_eq!(percent(2.0), "100%");
    }

    #[test]
    fn the_log_picker_and_its_notices_say_what_they_are_for() {
        assert_eq!(pick_log_title(), "로그를 어디에 저장할까요?");
        assert_eq!(
            pick_log_summary("복사했습니다", "assets → 백업"),
            "복사했습니다 · assets → 백업"
        );
        assert_eq!(pick_log_summary("복사했습니다", ""), "복사했습니다");
        assert_eq!(
            log_saved("job-copy-20261030-143207.log"),
            "로그를 저장했습니다: job-copy-20261030-143207.log"
        );
        assert_eq!(
            log_failed(ErrorKind::PermissionDenied),
            "로그를 저장하지 못했습니다: 권한이 없습니다"
        );
    }

    #[test]
    fn the_trash_s_jobs_are_named_by_their_items() {
        let item = |name: &str| {
            rf_vfs::TrashItem::at(std::env::temp_dir().join("Trash").join(name), false)
        };
        let one = JobSpec::DeleteFromTrash {
            items: vec![item("a.txt")],
        };
        assert_eq!(job_detail(&one), "a.txt");
        let two = JobSpec::RestoreFromTrash {
            items: vec![
                (item("a.txt"), std::env::temp_dir().join("a.txt")),
                (item("b.txt"), std::env::temp_dir().join("b.txt")),
            ],
        };
        assert_eq!(job_detail(&two), item_count(2));
        assert_eq!(job_detail(&JobSpec::EmptyTrash), "휴지통");
        assert_eq!(JobWork::of(&two), JobWork::Restore);
        assert_eq!(queued_detail(JobWork::Restore, "a.txt"), "복원 · a.txt");
    }

    #[test]
    fn job_details_name_the_items_and_the_target() {
        let copy = JobSpec::Transfer(
            TransferSpec::into_dir(
                TransferMode::Copy,
                &[path("assets")],
                &path("백업"),
                TransferOptions::default(),
            )
            .expect("a transfer"),
        );
        // The panel names the target folder, as the 화면 사양 shows it.
        assert_eq!(job_detail(&copy), "assets → 백업");
        let delete = JobSpec::Delete(DeleteSpec {
            paths: vec![path("a"), path("b")],
            method: DeleteMethod::Trash,
            errors: ErrorPolicy::default(),
        });
        assert_eq!(job_detail(&delete), "항목 2개");
        let rename = JobSpec::Rename {
            from: path("a.txt"),
            to: path("b.txt"),
        };
        assert_eq!(job_detail(&rename), "a.txt → b.txt");
        let create = JobSpec::Create {
            parent: path("작업"),
            name: "새 폴더".into(),
            kind: CreateKind::Dir,
        };
        assert_eq!(job_detail(&create), "새 폴더 → 작업");
        // A recovered job shows where, the middle of a long path left out.
        assert_eq!(
            recovered_job(&copy, "2026-10-28 22:08", true),
            format!(
                "복사 · assets → {} · 2026-10-28 22:08",
                short_path(&path("백업").to_string(), ROW_PLACE_CHARS)
            )
        );
        let deep = std::env::temp_dir()
            .join("a-long-folder-name")
            .join("another-long-folder")
            .join("작업")
            .join("백업");
        let deep = VPath::local(deep).expect("a local path");
        let line = recovered_job(
            &JobSpec::Transfer(
                TransferSpec::into_dir(
                    TransferMode::Move,
                    &[path("assets")],
                    &deep,
                    TransferOptions::default(),
                )
                .expect("a transfer"),
            ),
            "",
            true,
        );
        let sep = std::path::MAIN_SEPARATOR;
        assert!(line.contains('…'), "{line}");
        assert!(line.ends_with(&format!("작업{sep}백업")), "{line}");
        // Closing the window keeps the jobs for the next start, and says so; one sentence a
        // line, so that no line breaks inside a word.
        assert!(recovery_note().ends_with("닫으면 다음 실행 때 다시 묻습니다."));
        assert_eq!(recovery_note().lines().count(), 4);
        assert_eq!(
            recovered_job(&delete, "", false),
            "휴지통으로 보내기 · 항목 2개 (시작 전)"
        );
    }

    #[test]
    fn toasts_tell_what_happened() {
        assert_eq!(
            items_done(JobWork::Move, 3, 3, 0),
            "항목 3개를 이동했습니다"
        );
        // Skipped items are not counted as done.
        assert_eq!(
            items_done(JobWork::Copy, 3, 1, 2),
            "항목 3개 중 1개를 복사했습니다 · 2개 건너뜀"
        );
        assert_eq!(
            items_done(JobWork::Copy, 3, 0, 3),
            "항목 3개를 건너뛰었습니다"
        );
        assert_eq!(
            items_done(JobWork::Trash, 1, 1, 0),
            "항목 1개를 휴지통으로 보냈습니다"
        );
        // What the location deleted for good instead of trashing it is said so.
        assert_eq!(trash_done(2, 0), "항목 2개를 휴지통으로 보냈습니다");
        assert_eq!(
            trash_done(0, 1),
            "항목 1개가 휴지통에 들어가지 않고 영구 삭제되었습니다"
        );
        assert_eq!(
            trash_done(2, 1),
            "항목 2개를 휴지통으로 보냈고, 1개는 휴지통 대신 영구 삭제되었습니다"
        );
        assert_eq!(items_failed(2), "항목 2개를 처리하지 못했습니다");
        let moved = UndoRecord::Moved {
            moves: Vec::new(),
            created_dirs: Vec::new(),
            removed_dirs: Vec::new(),
            overwritten: 0,
            copied_dirs: Vec::new(),
        };
        assert_eq!(undone(&moved), "이동을 되돌렸습니다");
        assert_eq!(
            undone(&UndoRecord::Trashed { items: Vec::new() }),
            "휴지통으로 보내기를 되돌렸습니다"
        );
        let failure = ItemFailure {
            source: Some(path("a.txt")),
            target: Some(path("b.txt")),
            stage: rf_jobs::Stage::Rename,
            kind: ErrorKind::AlreadyExists,
            message: String::new(),
            reason: None,
        };
        assert_eq!(
            name_refused(JobWork::Rename, &failure),
            "이름을 바꾸지 못했습니다: 같은 이름의 항목이 이미 있습니다"
        );
        assert_eq!(
            failure_line(&failure),
            format!("{}: 같은 이름의 항목이 이미 있습니다", path("a.txt"))
        );
        // A long path is shown whole: the window wraps it.
        let deep = VPath::local(
            std::env::temp_dir()
                .join("a-long-folder-name")
                .join("another-long-folder")
                .join("작업")
                .join("a.txt"),
        )
        .expect("a local path");
        let deep_failure = ItemFailure {
            source: Some(deep.clone()),
            ..failure.clone()
        };
        assert_eq!(
            failure_line(&deep_failure),
            format!("{deep}: 같은 이름의 항목이 이미 있습니다")
        );
        assert!(!failure_line(&deep_failure).contains('…'));
        let refused = ItemFailure {
            reason: Some(FailureReason::Issue(IssueKind::ReservedName)),
            kind: ErrorKind::InvalidPath,
            ..failure
        };
        assert_eq!(
            name_refused(JobWork::Create, &refused),
            "새 폴더를 만들지 못했습니다: Windows가 장치 이름으로 쓰는 이름입니다(CON, NUL 등). 이 이름은 쓸 수 없습니다"
        );
    }

    #[test]
    fn the_window_that_asks_about_a_failure_does_not_say_it_was_already_left_out() {
        let failure = |reason: Option<FailureReason>| ItemFailure {
            source: None,
            target: None,
            stage: rf_jobs::Stage::Read,
            kind: ErrorKind::PermissionDenied,
            message: String::new(),
            reason,
        };
        // The reasons the list at a job's end words as done have lines of their own here.
        for reason in [
            FailureReason::TooDeep,
            FailureReason::NotARegularFile,
            FailureReason::SourceCutShort,
            FailureReason::OntoItself,
            FailureReason::ChangedWhileMoved,
            FailureReason::OtherVolume,
        ] {
            let listed = failure_reason(&reason);
            let asked = error_ask_reason(&failure(Some(reason.clone())));
            assert_ne!(asked, listed, "{reason:?}");
            for done in ["건너뛰었습니다", "두었습니다", "않았습니다"] {
                assert!(!asked.contains(done), "{reason:?}: {asked}");
            }
        }
        assert_eq!(
            error_ask_reason(&failure(Some(FailureReason::NotARegularFile))),
            "장치나 파이프 같은 특수 파일이라 처리할 수 없습니다"
        );
        // Any other failure reads as in the list: an error of the system, a rule that says
        // what is wrong.
        assert_eq!(error_ask_reason(&failure(None)), "권한이 없습니다");
        let mismatch = failure(Some(FailureReason::KindMismatch));
        assert_eq!(error_ask_reason(&mismatch), item_failure(&mismatch));
    }

    #[test]
    fn a_stopped_job_says_that_it_stopped_and_what_it_left() {
        assert_eq!(
            job_stopped(JobWork::Copy, 1, Some(2)),
            "복사를 중단했습니다 · 실패 1개 · 남은 항목 2개는 처리하지 않았습니다"
        );
        assert_eq!(
            job_stopped(JobWork::Move, 3, None),
            "이동을 중단했습니다 · 실패 3개 · 남은 항목은 처리하지 않았습니다"
        );
        assert_eq!(
            job_stopped(JobWork::Copy, 1, Some(0)),
            "복사를 중단했습니다 · 실패 1개"
        );
        assert_eq!(
            job_stopped_note(JobWork::Copy, Some(2)),
            "복사를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목 2개는 처리하지 않았습니다. \
             마저 하려면 복사를 다시 시작하세요."
        );
        assert_eq!(
            job_stopped_note(JobWork::Move, None),
            "이동을 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목은 처리하지 않았습니다. \
             마저 하려면 이동을 다시 시작하세요."
        );
        assert_eq!(
            job_stopped_note(JobWork::Copy, Some(0)),
            "복사를 중단했습니다. 아래 항목에서 멈췄습니다."
        );
    }

    #[test]
    fn conflict_words_follow_the_mockup() {
        assert_eq!(
            conflict_description("설계안.pdf", "C:\\사용자\\문서\\백업"),
            "‘설계안.pdf’가 C:\\사용자\\문서\\백업 에 이미 있습니다."
        );
        // The particle follows how the name ends.
        assert!(conflict_description("사진", "x").starts_with("‘사진’이"));
        assert!(conflict_description("보고서", "x").starts_with("‘보고서’가"));
        assert!(conflict_description("data.xml", "x").starts_with("‘data.xml’이"));
        assert!(conflict_description("v3", "x").starts_with("‘v3’이"));
        assert!(conflict_description("v2", "x").starts_with("‘v2’가"));
        assert_eq!(exact_size(1_258_291), "1.2 MB (1,258,291 바이트)");
        assert_eq!(exact_size(1_003_520), "980 KB (1,003,520 바이트)");
        assert_eq!(exact_size(512), "512 바이트");
        assert_eq!(
            conflict_title(EntryKind::File),
            "같은 이름의 파일이 이미 있습니다"
        );
        assert_eq!(
            conflict_title(EntryKind::Dir),
            "같은 이름의 폴더가 이미 있습니다"
        );
        // The item that would come is named by what the job does with it, and by its kind.
        assert_eq!(
            conflict_card(true, EntryKind::File, JobWork::Move),
            "옮겨 올 파일"
        );
        assert_eq!(
            conflict_card(true, EntryKind::File, JobWork::Copy),
            "복사해 올 파일"
        );
        assert_eq!(
            conflict_card(true, EntryKind::Dir, JobWork::Copy),
            "복사해 올 폴더"
        );
        assert_eq!(
            conflict_card(true, EntryKind::Symlink, JobWork::Move),
            "옮겨 올 항목"
        );
        assert_eq!(
            conflict_card(false, EntryKind::Dir, JobWork::Copy),
            "이미 있는 폴더"
        );
        assert_eq!(
            conflict_card(false, EntryKind::File, JobWork::Move),
            "이미 있는 파일"
        );
        assert_eq!(
            skip_hint(EntryKind::File, JobWork::Copy),
            "이 파일은 복사하지 않습니다"
        );
        assert_eq!(
            skip_hint(EntryKind::Dir, JobWork::Move),
            "이 폴더는 옮기지 않습니다"
        );
        assert_eq!(
            skip_hint(EntryKind::Dir, JobWork::Copy),
            "이 폴더는 복사하지 않습니다"
        );
        assert_eq!(
            skip_hint(EntryKind::Symlink, JobWork::Move),
            "이 항목은 옮기지 않습니다"
        );
        assert_eq!(
            overwrite_hint(true, JobWork::Copy),
            "있던 파일을 복사해 올 파일로 바꿉니다"
        );
        assert_eq!(
            rename_hint("설계안 (2).pdf"),
            "‘설계안 (2).pdf’처럼 번호를 붙여 저장합니다"
        );
        assert_eq!(
            overwrite_hint(true, JobWork::Move),
            "있던 파일을 옮겨 올 파일로 바꿉니다"
        );
        assert!(overwrite_warning(true).contains("남은 충돌도 모두 덮어씁니다"));
        assert_eq!(names_line(&[path("a")]), "‘a’");
    }

    #[test]
    fn outcomes_name_the_work() {
        assert_eq!(outcome(Outcome::Completed, JobWork::Copy), "복사했습니다");
        assert_eq!(outcome(Outcome::Completed, JobWork::Move), "이동했습니다");
        assert_eq!(
            outcome(Outcome::Completed, JobWork::Trash),
            "휴지통으로 보냈습니다"
        );
        assert_eq!(
            outcome(Outcome::Completed, JobWork::Rename),
            "이름을 바꿨습니다"
        );
        assert_eq!(
            outcome(Outcome::CompletedWithErrors, JobWork::Copy),
            "복사했지만 일부 항목을 처리하지 못했습니다"
        );
        assert_eq!(
            outcome(Outcome::Cancelled, JobWork::Move),
            "이동을 취소했습니다"
        );
        assert_eq!(
            outcome(Outcome::Cancelled, JobWork::Trash),
            "휴지통으로 보내기를 취소했습니다"
        );
        assert_eq!(
            outcome(Outcome::Aborted, JobWork::Delete),
            "문제가 생겨 삭제를 중단했습니다"
        );
        assert_eq!(outcome(Outcome::Completed, JobWork::Undo), "되돌렸습니다");
        assert_eq!(
            outcome(Outcome::Completed, JobWork::Redo),
            "다시 실행했습니다"
        );
        let outcomes = [
            Outcome::Completed,
            Outcome::CompletedWithErrors,
            Outcome::Cancelled,
            Outcome::Aborted,
        ];
        let mut seen = HashSet::new();
        for work in WORKS {
            for end in outcomes {
                let words = outcome(end, work);
                assert!(is_korean(&words), "{end:?} {work:?}: {words:?}");
                assert!(
                    seen.insert(words.clone()),
                    "{end:?} {work:?} repeats {words:?}"
                );
            }
        }
    }

    #[test]
    fn a_drag_says_what_its_drop_would_do() {
        assert_eq!(
            drop_badge(DropChoice::Do(DropEffect::Move), "백업"),
            "이동 → 백업"
        );
        assert_eq!(
            drop_badge(DropChoice::Do(DropEffect::Copy), "Downloads"),
            "복사 → Downloads"
        );
        assert_eq!(drop_badge(DropChoice::Ask, "백업"), "놓을 때 묻기 → 백업");
        assert_eq!(
            drop_badge(DropChoice::Do(DropEffect::Link), "백업"),
            "링크 만들기: 백업"
        );
        assert_eq!(
            drop_badge(DropChoice::Refuse(Refusal::NotAllowed), "백업"),
            "여기에는 놓을 수 없습니다"
        );
        // A drag of another app that carries no path to take.
        assert_eq!(
            drop_no_paths(),
            "여기에 놓을 수 없습니다: 받을 수 있는 경로가 없습니다"
        );
    }

    #[test]
    fn a_dragged_move_says_what_it_moved() {
        assert_eq!(dragged_move_done(3, 7, 0), "항목 3개를 옮겼습니다");
        // What it skipped, as the other jobs say it.
        assert_eq!(
            dragged_move_done(3, 1, 2),
            "항목 3개 중 1개를 옮겼습니다 · 2개 건너뜀"
        );
        assert_eq!(dragged_move_done(2, 0, 2), "항목 2개를 건너뛰었습니다");
    }

    #[test]
    fn the_facts_of_a_preview_break_only_between_them() {
        let facts = preview_facts("JPG 이미지", Some("98 KB"), Some("2026-10-30 01:29"));
        assert_eq!(
            facts,
            "JPG\u{a0}이미지 · 98\u{a0}KB · 2026-10-30\u{a0}01:29"
        );
        // A date that cannot be told is left out.
        assert_eq!(preview_facts("파일 폴더", None, Some("")), "파일\u{a0}폴더");
    }

    #[test]
    fn shell_failures_say_what_was_not_done() {
        use ShellAction as A;
        assert_eq!(
            shell_failed(A::OpenWith, ErrorKind::Unsupported, Os::Other),
            "이 시스템에서는 연결 프로그램 고르기를 지원하지 않습니다"
        );
        assert_eq!(
            shell_failed(A::SystemProperties, ErrorKind::Unsupported, Os::Other),
            "이 시스템에서는 OS 속성 창을 지원하지 않습니다"
        );
        // No terminal program, and a folder gone since.
        assert_eq!(
            shell_failed(A::Terminal, ErrorKind::Unsupported, Os::Other),
            "터미널 프로그램을 찾지 못했습니다"
        );
        assert_eq!(
            shell_failed(A::Terminal, ErrorKind::NotFound, Os::Windows),
            "터미널을 열지 못했습니다: 찾을 수 없습니다"
        );
        // Only macOS asks the user to let the app control Finder.
        let refused = shell_failed(A::SystemProperties, ErrorKind::PermissionDenied, Os::MacOs);
        assert!(refused.contains("자동화"), "{refused}");
        assert_eq!(
            shell_failed(
                A::SystemProperties,
                ErrorKind::PermissionDenied,
                Os::Windows
            ),
            "OS 속성 창을 열지 못했습니다: 권한이 없습니다"
        );
        assert_eq!(
            shell_failed(A::RunAsAdmin, ErrorKind::Other, Os::Windows),
            "관리자 권한으로 실행하지 못했습니다: 알 수 없는 오류가 났습니다"
        );
        // Windows' own menu, and 공유… of macOS.
        assert_eq!(
            shell_failed(A::SystemMenu, ErrorKind::NotFound, Os::Windows),
            "Windows 메뉴를 띄우지 못했습니다: 찾을 수 없습니다"
        );
        assert_eq!(
            shell_failed(A::SystemMenu, ErrorKind::Unsupported, Os::Other),
            "이 시스템에는 Windows 메뉴가 없습니다"
        );
        assert_eq!(
            shell_failed(A::Share, ErrorKind::Other, Os::MacOs),
            "공유하지 못했습니다: 알 수 없는 오류가 났습니다"
        );
        assert_eq!(
            shell_failed(A::Share, ErrorKind::Unsupported, Os::Other),
            "이 시스템에서는 공유를 지원하지 않습니다"
        );
        for action in [
            A::OpenWith,
            A::RunAsAdmin,
            A::Terminal,
            A::SystemProperties,
            A::SystemMenu,
            A::Share,
        ] {
            for kind in ERROR_KINDS {
                for os in [Os::Windows, Os::MacOs, Os::Other] {
                    let words = shell_failed(action, kind, os);
                    assert!(is_korean(&words), "{action:?} {kind:?}: {words:?}");
                }
            }
        }
    }

    #[test]
    fn the_default_app_says_so() {
        let app = |default| App {
            name: "미리보기".to_owned(),
            path: std::path::PathBuf::from("/System/Applications/Preview.app"),
            default,
        };
        assert_eq!(app_title(&app(true)), "미리보기 (기본)");
        assert_eq!(app_title(&app(false)), "미리보기");
        assert!(is_korean(no_apps()));
        assert!(batch_path_refused().starts_with("이 경로의 배치 파일은"));
        assert_eq!(link_elsewhere(true), "링크가 폴더를 가리키지 않습니다");
        assert_eq!(link_elsewhere(false), "링크가 파일을 가리키지 않습니다");
    }
}
