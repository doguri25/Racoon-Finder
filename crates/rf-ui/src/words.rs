//! The Korean words the app shows for errors, refusals, jobs, the status bar and the sidebar
//! (I18N-02).
//!
//! The `Display` text of `rf_core::Error` and of the job types is English and for the log; the
//! screens show these words instead. They say what happened to the user's item, not how the
//! engine works.

use std::fmt::Write as _;
use std::time::Duration;

use rf_core::{EntryKind, ErrorKind, VPath};
use rf_jobs::{
    ConflictPolicy, DeleteMethod, FailureReason, IssueKind, ItemFailure, JobKind, JobSpec,
    JobState, Outcome, Severity, TransferMode, UndoRecord,
};
use rf_platform::KnownFolder;

use crate::format::{Rounding, format_size, format_size_rounded, short_path};
use crate::pattern::{MAX_PATTERN_CHARS, PatternError};
use crate::pick::Validity;
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
            JobKind::Undo => JobWork::Undo,
            _ => JobWork::Other,
        }
    }

    /// The pieces the job sentences are made of.
    fn phrases(self) -> Phrases {
        let (doing, done, object, noun) = match self {
            JobWork::Copy => ("복사 중", "복사했", "복사를", "복사"),
            JobWork::Move => ("이동 중", "이동했", "이동을", "이동"),
            JobWork::Trash => (
                "휴지통으로 보내는 중",
                "휴지통으로 보냈",
                "휴지통으로 보내기를",
                "휴지통으로 보내기",
            ),
            JobWork::Delete => ("삭제 중", "삭제했", "삭제를", "영구 삭제"),
            JobWork::Rename => (
                "이름 바꾸는 중",
                "이름을 바꿨",
                "이름 바꾸기를",
                "이름 바꾸기",
            ),
            JobWork::Create => ("만드는 중", "만들었", "만들기를", "새로 만들기"),
            JobWork::Undo => ("되돌리는 중", "되돌렸", "되돌리기를", "되돌리기"),
            JobWork::Redo => (
                "다시 실행하는 중",
                "다시 실행했",
                "다시 실행을",
                "다시 실행",
            ),
            JobWork::Other => ("작업 중", "작업을 마쳤", "작업을", "작업"),
        };
        Phrases {
            doing,
            done,
            object,
            noun,
        }
    }

    /// The name of the work on its own, as a recovered job's line starts: `복사`, `이동`.
    pub fn noun(self) -> &'static str {
        self.phrases().noun
    }
}

/// How one [`JobWork`] reads in a sentence.
struct Phrases {
    /// The title while it runs: `복사 중`.
    doing: &'static str,
    /// The past form without its ending, for `…습니다` and `…지만`: `복사했`.
    done: &'static str,
    /// The work as the object of a verb, with its particle: `복사를`.
    object: &'static str,
    /// The work on its own: `복사`.
    noun: &'static str,
}

/// What went wrong, for an error of the system or of a provider (an item with no
/// [`FailureReason`]).
pub fn error_kind(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::NotFound => "찾을 수 없습니다",
        ErrorKind::PermissionDenied => "권한이 없습니다",
        ErrorKind::AlreadyExists => "같은 이름의 항목이 이미 있습니다",
        ErrorKind::InvalidPath => "쓸 수 없는 이름이나 경로입니다",
        ErrorKind::NotADirectory => "경로에 폴더가 아닌 항목이 있습니다",
        ErrorKind::IsADirectory => "파일이 아니라 폴더입니다",
        ErrorKind::DirectoryNotEmpty => "폴더가 비어 있지 않습니다",
        ErrorKind::StorageFull => "디스크 공간이 부족합니다",
        ErrorKind::ReadOnly => "읽기 전용 위치입니다",
        ErrorKind::CrossesDevices => "다른 드라이브로는 한 번에 옮길 수 없습니다",
        ErrorKind::Busy => "다른 프로그램이 사용 중입니다",
        ErrorKind::TimedOut => "응답이 너무 늦어 중단했습니다",
        ErrorKind::Interrupted => "작업이 중간에 끊겼으니 다시 해 보세요",
        ErrorKind::FileTooLarge => "파일이 너무 커서 이 위치에 둘 수 없습니다",
        ErrorKind::Unsupported => "이 항목이나 위치에서는 할 수 없는 작업입니다",
        ErrorKind::Cancelled => "취소되었습니다",
        // `Other`, and any kind added after this was written.
        _ => "알 수 없는 오류가 났습니다",
    }
}

/// The notice for an item that could not be opened or a folder that could not be shown:
/// `열 수 없습니다: 권한이 없습니다`.
pub fn cannot_open(kind: ErrorKind) -> String {
    format!("열 수 없습니다: {}", error_kind(kind))
}

/// The notice for the address of a remote place typed into a path field (`sftp://…`): the app
/// opens local folders alone for now.
pub fn remote_location() -> &'static str {
    "아직 지원하지 않는 위치입니다"
}

/// What the list of a folder's subfolders under a `›` says while a worker lists the folder.
pub fn listing_folders() -> &'static str {
    "불러오는 중…"
}

/// What the list of a folder's subfolders says when the folder has none (that the panes show).
pub fn no_subfolders() -> &'static str {
    "하위 폴더가 없습니다"
}

/// Under the names a list of the address bar shows: how many more folders there are.
pub fn more_folders(count: usize) -> String {
    format!("외 {count}개")
}

/// The notice of a drop the window did not make: the rows changed under the pointer (a
/// folder's watch) since it showed where the items would go, so they would go elsewhere.
pub fn drop_moved() -> &'static str {
    "목록이 바뀌어 놓을 곳이 달라졌습니다. 다시 끌어다 놓으세요"
}

/// The notice when the folder `name` a pane showed is gone and the pane went to the nearest
/// folder above it that is left (`to_parent`), or home when none is.
pub fn folder_vanished(name: &str, to_parent: bool) -> String {
    let to = if to_parent {
        "상위 폴더"
    } else {
        "홈 폴더"
    };
    format!("‘{name}’ 폴더가 없어져 {to}로 옮겼습니다")
}

/// What the head of `column` of 자세히 says (the 보기 menu's 열 and 정렬 say it too).
pub fn column_title(column: Column) -> &'static str {
    match column {
        Column::Name => "이름",
        Column::Size => "크기",
        Column::Modified => "수정한 날짜",
        Column::Created => "만든 날짜",
        Column::Kind => "종류",
        Column::Extension => "확장자",
        Column::Attributes => "속성",
    }
}

/// The notice after 하위 폴더에 적용 in the folder `name`.
pub fn view_applied_below(name: &str) -> String {
    format!("‘{name}’ 폴더와 그 하위 폴더를 이 보기로 표시합니다")
}

/// The title of the question before 하위 폴더에 적용 makes folders below forget their views.
pub fn view_apply_title() -> &'static str {
    "하위 폴더에 적용"
}

/// The question before 하위 폴더에 적용 in the folder `name`, of whose subfolders `count` have
/// views of their own.
pub fn view_apply_below(name: &str, count: usize) -> String {
    format!(
        "‘{name}’ 폴더의 보기를 하위 폴더에도 적용할까요?\n하위 폴더 {count}개에 따로 정한 보기가 지워집니다."
    )
}

/// The label of the button that applies a view below.
pub fn view_apply_button() -> &'static str {
    "적용"
}

/// The notice after 기본값으로 되돌리기 in the folder `name`.
pub fn view_reset(name: &str) -> String {
    format!("‘{name}’ 폴더의 보기를 되돌렸습니다")
}

/// The notice when a folder's view cannot be remembered (its path is too long to keep).
pub fn view_not_kept() -> &'static str {
    "경로가 너무 길어 이 폴더의 보기를 기억할 수 없습니다"
}

/// The notice when the folder `name` a pane showed is gone, the pane went to the nearest folder
/// above it that is left (`to_parent`) or home, and navigation sync, which waited to open
/// folders there, turned off: one notice for both, as the window shows one at a time.
pub fn folder_vanished_sync_off(name: &str, to_parent: bool) -> String {
    let to = if to_parent {
        "상위 폴더"
    } else {
        "홈 폴더"
    };
    format!("‘{name}’ 폴더가 없어져 {to}로 옮겼고 탐색 동기화를 껐습니다")
}

/// The notice of 같은 확장자 선택 on a row whose name has no extension (a folder, `README`).
pub fn no_extension(name: &str) -> String {
    format!("‘{name}’에는 확장자가 없습니다")
}

/// The notice when a long folder could not be put in its new order: the rows keep the one they
/// had.
pub fn sort_failed() -> &'static str {
    "이 폴더를 정렬하지 못해 이전 순서로 둡니다"
}

/// What the pattern window shows when its worker could not count the rows: 선택 matches them
/// when it is pressed.
pub fn pattern_uncounted() -> &'static str {
    "항목 수를 세지 못했습니다. 선택을 누르면 바로 맞춰 고릅니다"
}

/// How many rows the pattern window's pattern matches.
pub fn pattern_count(count: usize) -> String {
    format!("맞는 항목 {count}개")
}

/// What the pattern window shows while a worker matches the names, before its first count.
pub fn pattern_counting() -> &'static str {
    "세는 중…"
}

/// Why the pattern window's text is no pattern.
pub fn pattern_problem(problem: PatternError) -> String {
    match problem {
        PatternError::TooLong => format!("패턴이 너무 깁니다 ({MAX_PATTERN_CHARS}자까지)"),
        PatternError::Invalid => "올바른 정규식이 아닙니다".to_owned(),
        PatternError::TooComplex => "패턴이 너무 복잡합니다. 더 짧게 써 주세요".to_owned(),
    }
}

/// The notice when navigation sync is turned on: a move into a folder, up, back or forward in
/// one pane is made in the other one too.
pub fn sync_on() -> &'static str {
    "탐색 동기화를 켰습니다"
}

/// The notice when navigation sync is turned off.
pub fn sync_off() -> &'static str {
    "탐색 동기화를 껐습니다"
}

/// The notice when one pane went into its folder `name` and the other has no folder of that
/// name: sync is off.
pub fn sync_no_folder(name: &str) -> String {
    format!("반대편 패널에 ‘{name}’ 폴더가 없어 탐색 동기화를 껐습니다")
}

/// The notice when one pane went up, back or forward and the other cannot (it is at a root,
/// or has nothing to go back or forward to): sync is off.
pub fn sync_cannot_follow() -> &'static str {
    "반대편 패널이 따라갈 수 없어 탐색 동기화를 껐습니다"
}

/// The notice when a command for the other pane (the same folder there, navigation sync) is
/// asked for while one pane shows.
pub fn one_pane_only() -> &'static str {
    "한 패널 보기에서는 쓸 수 없습니다"
}

/// The notice when a pane's last tab is to go to the other pane: a pane keeps one tab.
pub fn last_tab_stays() -> &'static str {
    "패널의 마지막 탭은 옮길 수 없습니다"
}

/// The notice when a tab went to the other pane while one pane shows: it is in the hidden
/// pane, which the 보기 menu's 듀얼 패널 shows.
pub fn tab_moved_to_hidden() -> &'static str {
    "탭을 숨은 반대편 패널로 옮겼습니다. 보기 메뉴의 ‘듀얼 패널’로 볼 수 있습니다"
}

/// The notice when the folder the app was asked to open at its start is gone: the tabs of
/// the last run show instead. A long path loses its middle ([`short_path`]).
pub fn start_folder_gone(folder: &VPath) -> String {
    format!(
        "열려던 폴더를 찾을 수 없습니다: {}",
        short_path(&folder.to_string(), NOTICE_PLACE_CHARS)
    )
}

/// The notice when the system could not start the work behind part of the window (it is out
/// of threads or memory): the sidebar, the first folders, or where to go from a vanished
/// folder.
pub fn background_failed() -> &'static str {
    "시스템 자원이 부족해 일부 내용을 불러오지 못했습니다"
}

/// The notice when the settings (tabs, theme, window size) could not be written: the next
/// start may not show them.
pub fn settings_not_saved() -> &'static str {
    "설정을 저장하지 못했습니다. 다음 실행 때 탭과 보기 설정이 돌아오지 않을 수 있습니다"
}

/// The notice at the start when the settings file is there but could not be read (or set
/// aside): the app starts with the defaults and leaves the file alone.
pub fn settings_unreadable() -> &'static str {
    "설정 파일을 읽지 못해 기본 설정으로 시작했습니다. 이번에 바꾼 설정은 저장하지 않습니다"
}

/// The space line of a drive in the sidebar: `128 GB 남음 / 512 GB`. The free space is
/// rounded down, so a full drive never reads as having room.
pub fn drive_space(available: u64, total: u64) -> String {
    format!(
        "{} 남음 / {}",
        format_size_rounded(available, Rounding::Down),
        format_size(total)
    )
}

/// The status bar of a folder: `항목 9개 · 선택 2개 (1.4 MB) · 남은 공간 212 GB`.
///
/// `items` is the number of rows and `selected` the number of selected ones; the selection
/// part shows only when there is one. `selected_bytes` is the size of the selected files, and
/// `None` when only folders or links are selected, which have no size to add up. `free` is
/// the space left on the volume, shown once known and rounded down like the sidebar's
/// [`drive_space`], so the two never show different numbers for the same volume.
pub fn status_line(
    items: usize,
    selected: usize,
    selected_bytes: Option<u64>,
    free: Option<u64>,
) -> String {
    status_parts(
        vec![format!("항목 {items}개")],
        selected,
        selected_bytes,
        free,
    )
}

/// The status bar of a folder while the toolbar's filter shows only the rows whose names hold
/// `filter`: `거르는 중: ‘보고’ · 항목 2개 / 9개 · 선택 1개 · 남은 공간 212 GB`, where `shown` is
/// how many rows show and of how many there are (`(2, 9)`); the rest as in [`status_line`].
pub fn filtered_status_line(
    filter: &str,
    (shown, of): (usize, usize),
    selected: usize,
    selected_bytes: Option<u64>,
    free: Option<u64>,
) -> String {
    let parts = vec![
        format!("거르는 중: ‘{filter}’"),
        format!("항목 {shown}개 / {of}개"),
    ];
    status_parts(parts, selected, selected_bytes, free)
}

/// `parts`, then the selection and the free space of [`status_line`], joined.
fn status_parts(
    mut parts: Vec<String>,
    selected: usize,
    selected_bytes: Option<u64>,
    free: Option<u64>,
) -> String {
    match (selected, selected_bytes) {
        (0, _) => {}
        (count, Some(bytes)) => parts.push(format!("선택 {count}개 ({})", format_size(bytes))),
        (count, None) => parts.push(format!("선택 {count}개")),
    }
    if let Some(free) = free {
        parts.push(format!(
            "남은 공간 {}",
            format_size_rounded(free, Rounding::Down)
        ));
    }
    parts.join(" · ")
}

/// The sidebar name of one of the user's well-known folders.
pub fn favorite(folder: KnownFolder) -> &'static str {
    match folder {
        KnownFolder::Home => "홈",
        KnownFolder::Desktop => "바탕 화면",
        KnownFolder::Documents => "문서",
        KnownFolder::Downloads => "다운로드",
        KnownFolder::Pictures => "사진",
        KnownFolder::Music => "음악",
        KnownFolder::Videos => "동영상",
    }
}

/// The sidebar name of the Windows drive `letter`: `로컬 디스크 (C:)` for the system's usual
/// drive, `드라이브 (D:)` for the others.
pub fn drive_letter(letter: char) -> String {
    if letter == 'C' {
        "로컬 디스크 (C:)".to_owned()
    } else {
        format!("드라이브 ({letter}:)")
    }
}

/// The sidebar name of the volume that holds `/` on macOS and Linux.
pub fn system_disk() -> &'static str {
    "시스템 디스크"
}

/// Why a job refused an item, or part of one: one sentence for each rule of the jobs, or for
/// a name or size the target cannot hold, the [`issue_kind`] fact and what it meant for the
/// item (a rename or a create refuses every name with an issue, warnings too).
pub fn failure_reason(reason: &FailureReason) -> String {
    let words = match reason {
        FailureReason::IntoItself => "폴더를 자기 안으로 복사하거나 옮길 수 없습니다",
        FailureReason::NoTargetFolder => "대상이 드라이브 자체라 그 자리에 둘 수 없습니다",
        FailureReason::TargetNotAFolder => "대상 위치가 폴더가 아닙니다",
        FailureReason::TooDeep => "폴더 안 너무 깊은 곳에 있어 건너뛰었습니다",
        FailureReason::LinkLoop => "바로 가기가 자기를 담은 폴더를 다시 가리켜 따라갈 수 없습니다",
        FailureReason::TooManyLinks => "바로 가기가 너무 여러 번 이어져 따라갈 수 없습니다",
        FailureReason::LinkChanged => "따라가는 동안 바로 가기가 바뀌었습니다",
        FailureReason::AbsoluteLink => "이 위치의 바로 가기는 따라갈 수 없습니다",
        FailureReason::NotARegularFile => "장치나 파이프 같은 특수 파일이라 건너뛰었습니다",
        FailureReason::SourceCutShort => {
            "복사하는 동안 원본 파일이 줄어들어 사본을 만들지 않았습니다"
        }
        FailureReason::KindMismatch => {
            "같은 이름의 항목이 있는데, 파일과 폴더는 서로 덮어쓸 수 없습니다"
        }
        FailureReason::NoFreeName => "번호를 붙인 이름도 모두 쓰이고 있습니다",
        FailureReason::OntoItself => "대상 폴더가 원본이 있는 바로 그 폴더라 옮기지 않았습니다",
        FailureReason::ChangedWhileMoved => "옮기는 동안 항목이 바뀌어 그대로 두었습니다",
        FailureReason::KeptAsTargetFolder => {
            "원본 폴더가 대상 폴더와 같은 폴더라 지우지 않았습니다"
        }
        FailureReason::CannotTellOwnFolder => {
            "원본 폴더가 대상 폴더와 같은지 알 수 없어 지우지 않았습니다"
        }
        FailureReason::OtherVolume => "다른 드라이브의 항목이라 건너뛰었습니다",
        FailureReason::TrashRefused => "휴지통으로 보낼 수 없습니다",
        FailureReason::DeletedNotTrashed => "휴지통 대신 영구 삭제되어 되살릴 수 없습니다",
        FailureReason::ChangedWhileDeleted => "지우는 동안 항목이 바뀌어 그대로 두었습니다",
        FailureReason::Overwritten => "덮어쓴 파일의 예전 내용은 되살릴 수 없습니다",
        FailureReason::ChangedSinceCopy => "복사한 뒤 바뀌어 되돌리지 않았습니다",
        FailureReason::ChangedSinceCreated => "만든 뒤 다른 종류로 바뀌어 되돌리지 않았습니다",
        FailureReason::NotEmpty => "만든 뒤 내용이 생겨 되돌리지 않았습니다",
        FailureReason::TrashLocationUnknown => {
            "휴지통의 어디에 들어갔는지 알 수 없어 되돌릴 수 없습니다"
        }
        FailureReason::AcrossLocations => "이름 바꾸기로는 다른 위치로 옮길 수 없습니다",
        FailureReason::TempLeftBehind => "임시 파일을 지우지 못해 남아 있습니다",
        FailureReason::Issue(issue) => {
            return format!("{}. {}", issue_kind(issue), refused_because(issue));
        }
        _ => "알 수 없는 이유로 처리하지 못했습니다",
    };
    words.to_owned()
}

/// What keeps an item from fitting its target as it is, as a fact: the character, the
/// sizes or the other name involved. What follows from it depends on the job, so the words
/// the user reads come from [`failure_reason`] (the item was refused) or [`preflight_issue`]
/// (before a transfer).
pub fn issue_kind(issue: &IssueKind) -> String {
    match issue {
        IssueKind::InvalidCharacter(c) => {
            format!(
                "대상 위치가 허용하지 않는 {} 문자가 이름에 있습니다",
                quote_char(*c)
            )
        }
        IssueKind::ReservedName => {
            "Windows가 장치 이름으로 쓰는 이름입니다(CON, NUL 등)".to_owned()
        }
        IssueKind::TrailingDotOrSpace => "이름이 마침표나 공백으로 끝납니다".to_owned(),
        // `len` and `max` count UTF-8 bytes or UTF-16 units, and a path may be the temporary
        // one: no number would mean much to the user.
        IssueKind::NameTooLong { .. } => "이름이 너무 깁니다".to_owned(),
        IssueKind::PathTooLong { .. } => "경로가 너무 깁니다".to_owned(),
        IssueKind::FileTooLarge { size, max } => format!(
            "파일이 대상 위치가 담을 수 있는 크기보다 큽니다 ({}, 최대 {})",
            format_size_rounded(*size, Rounding::Up),
            format_size_rounded(*max, Rounding::Down)
        ),
        IssueKind::CaseCollision { other } => format!(
            "대소문자만 다른 ‘{}’ 항목이 이미 있습니다",
            other.to_string_lossy()
        ),
        IssueKind::NotEnoughSpace { needed, available } => format!(
            "대상 드라이브의 공간이 부족합니다 (필요 {}, 남은 공간 {})",
            format_size_rounded(*needed, Rounding::Up),
            format_size_rounded(*available, Rounding::Down)
        ),
        _ => "대상 위치에 맞지 않는 항목입니다".to_owned(),
    }
}

/// What it meant for an item that a job refused it for `issue`.
fn refused_because(issue: &IssueKind) -> &'static str {
    match issue {
        IssueKind::NameTooLong { .. } => "더 짧게 바꿔 주세요",
        IssueKind::PathTooLong { .. } => "더 얕은 폴더나 짧은 이름을 써 주세요",
        IssueKind::InvalidCharacter(_)
        | IssueKind::ReservedName
        | IssueKind::TrailingDotOrSpace
        | IssueKind::CaseCollision { .. } => "이 이름은 쓸 수 없습니다",
        IssueKind::NotEnoughSpace { .. } => "공간을 비우거나 다른 위치를 고르세요",
        // `FileTooLarge`, and any issue added after this was written.
        _ => "이 위치에는 둘 수 없습니다",
    }
}

/// One line of the pre-flight review a transfer shows before it writes anything: the
/// [`issue_kind`] fact and what follows from it. An item with a warning can still be written
/// and may then be hard to open; one with a blocking issue cannot be written as it is; too
/// little space lets the job start but may stop it halfway.
pub fn preflight_issue(issue: &IssueKind) -> String {
    format!("{}. {}", issue_kind(issue), preflight_follows(issue))
}

/// What follows from `issue` for a transfer that has not written anything yet.
fn preflight_follows(issue: &IssueKind) -> &'static str {
    match issue {
        IssueKind::NotEnoughSpace { .. } => "계속하면 공간이 모자라 중간에 멈출 수 있습니다",
        _ if issue.severity() == Severity::Warning => {
            "그대로 두면 Windows에서 열기 어려울 수 있습니다"
        }
        _ => "이 항목은 대상 위치에 둘 수 없습니다",
    }
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
    let Phrases { done, object, .. } = work.phrases();
    match outcome {
        Outcome::Completed => format!("{done}습니다"),
        Outcome::CompletedWithErrors => format!("{done}지만 일부 항목을 처리하지 못했습니다"),
        Outcome::Cancelled => format!("{object} 취소했습니다"),
        Outcome::Aborted => format!("문제가 생겨 {object} 중단했습니다"),
        _ => format!("{object} 끝냈습니다"),
    }
}

/// The title of a job's row in the job panel: `대기 중` in the queue, `일시정지됨` when paused,
/// what the job does while it plans, runs or waits for an answer (`복사 중`; the panel shows
/// the question itself), and its [`outcome`] once it has ended.
pub fn job_title(work: JobWork, state: JobState) -> String {
    match state {
        JobState::Queued => "대기 중".to_owned(),
        JobState::Paused => "일시정지됨".to_owned(),
        JobState::Finished(end) => outcome(end, work),
        // `Planning`, `Running`, `Waiting`, and any state added after this was written.
        _ => work.phrases().doing.to_owned(),
    }
}

/// The notice of a second instance of the app: the first one keeps the job journal, so the
/// second one ends.
pub fn already_running() -> &'static str {
    "이미 실행 중입니다. 먼저 연 창을 쓰세요."
}

/// The title of the window that says the app could not start.
pub fn startup_failed() -> &'static str {
    "앱을 시작하지 못했습니다"
}

/// Why the app could not start: the window system or the renderer failed.
pub fn no_window() -> &'static str {
    "창을 열 수 없습니다. 화면이나 그래픽 드라이버 설정을 확인해 주세요"
}

/// Why the app could not start: the folders it keeps its settings and job journal in cannot
/// be found or made.
pub fn no_app_folders(kind: ErrorKind) -> String {
    format!(
        "설정과 작업 기록을 둘 폴더를 쓸 수 없습니다: {}",
        error_kind(kind)
    )
}

/// Why the app could not start: the lock of the job journal's folder could not be taken.
pub fn lock_failed(kind: ErrorKind) -> String {
    format!("작업 기록 폴더를 잠글 수 없습니다: {}", error_kind(kind))
}

/// The notice when the job journal could not be read or written: the jobs run on, but one
/// that a crash cuts off may not be offered again.
pub fn journal_error() -> &'static str {
    "작업 기록을 저장하지 못했습니다. 작업은 계속되지만 앱이 갑자기 꺼지면 이어서 할 수 없습니다"
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
        parts.push(format!(
            "{} / {total_items} 항목",
            done_items.min(total_items)
        ));
    }
    if bytes_per_sec > 0 {
        parts.push(format!("{}/s", format_size(bytes_per_sec)));
        if let Some(eta) = eta.filter(|eta| !eta.is_zero()) {
            parts.push(time_left(eta));
        }
    }
    if queued > 0 {
        parts.push(format!("대기 중인 작업 {queued}개"));
    }
    parts.join(" · ")
}

/// `남은 시간 약 20초`, `약 3분`, `약 1시간 5분`: rounded up, so that the time never reads as
/// shorter than it is.
pub fn time_left(eta: Duration) -> String {
    let secs = eta.as_secs() + u64::from(eta.subsec_nanos() > 0);
    let about = if secs < 60 {
        format!("{secs}초")
    } else if secs < 3600 {
        format!("{}분", secs.div_ceil(60))
    } else {
        let minutes = secs.div_ceil(60);
        match minutes % 60 {
            0 => format!("{}시간", minutes / 60),
            rest => format!("{}시간 {rest}분", minutes / 60),
        }
    };
    format!("남은 시간 약 {about}")
}

/// The progress line of a job that is still counting what it will do.
pub fn job_planning() -> &'static str {
    "준비 중"
}

/// The progress line of a job that waits for the user to answer its question.
pub fn job_waiting() -> &'static str {
    "답을 기다리는 중"
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
        JobSpec::Rename { from, to } => format!("{} → {}", name_of(from), name_of(to)),
        JobSpec::Create { parent, name, .. } => {
            format!("{} → {}", name.to_string_lossy(), folder(parent))
        }
        JobSpec::Undo(record) => item_count(record.item_count()),
        // A kind of job added after this was written.
        _ => String::new(),
    }
}

/// The notice when items cannot be copied or moved at all (a volume root, a name the target
/// cannot take): `복사하거나 옮길 수 없습니다: 쓸 수 없는 이름이나 경로입니다`.
pub fn cannot_transfer(kind: ErrorKind) -> String {
    format!("복사하거나 옮길 수 없습니다: {}", error_kind(kind))
}

/// Why the clipboard did not take or give what was asked: the system has no clipboard for it
/// (text on Linux), or another app held it or the system refused.
pub fn clipboard_failed(failure: ClipboardFailure) -> &'static str {
    match failure {
        ClipboardFailure::Unsupported => "이 시스템에서는 클립보드에 글자를 넣을 수 없습니다",
        ClipboardFailure::Unavailable => {
            "클립보드를 쓸 수 없습니다. 다른 앱이 쓰고 있다면 잠시 뒤에 다시 해 보세요"
        }
    }
}

/// A paste with no files or folders on the clipboard (text, an image, nothing).
pub fn nothing_to_paste() -> &'static str {
    "클립보드에 붙여넣을 파일이나 폴더가 없습니다"
}

/// A paste of a folder into itself or into a folder inside it.
pub fn paste_into_itself() -> &'static str {
    "폴더를 그 폴더 안에 붙여넣을 수 없습니다"
}

/// A paste of items cut, into the folder they are in already: nothing moves.
pub fn paste_already_here() -> &'static str {
    "잘라낸 항목이 이미 이 폴더에 있습니다"
}

/// The same cut pasted again while its first paste still moves the items.
pub fn paste_moving() -> &'static str {
    "잘라낸 항목을 옮기는 중입니다"
}

/// `항목 3개`.
pub fn item_count(count: usize) -> String {
    format!("항목 {count}개")
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

/// The notice of a transfer or a trash that ended as asked, `count` items at its top level:
/// `항목 3개를 이동했습니다`. When its conflict policy left items out, the numbers are of every
/// item it went through (`done` made, `skipped` left out): `항목 3개 중 1개를 복사했습니다 ·
/// 2개 건너뜀`, or `항목 3개를 건너뛰었습니다` when it made none.
pub fn items_done(work: JobWork, count: usize, done: u64, skipped: u64) -> String {
    let made = work.phrases().done;
    match (done, skipped) {
        (_, 0) => format!("항목 {count}개를 {made}습니다"),
        (0, skipped) => format!("항목 {skipped}개를 건너뛰었습니다"),
        (done, skipped) => format!(
            "항목 {}개 중 {done}개를 {made}습니다 · {skipped}개 건너뜀",
            done.saturating_add(skipped)
        ),
    }
}

/// The notice of a trash job that ended: what went to the trash (`trashed` items), and what
/// the location deleted for good instead (`gone`), which nothing brings back.
pub fn trash_done(trashed: usize, gone: usize) -> String {
    match (trashed, gone) {
        (trashed, 0) => items_done(JobWork::Trash, trashed, 0, 0),
        (0, gone) => format!("항목 {gone}개가 휴지통에 들어가지 않고 영구 삭제되었습니다"),
        (trashed, gone) => format!(
            "항목 {trashed}개를 휴지통으로 보냈고, {gone}개는 휴지통 대신 영구 삭제되었습니다"
        ),
    }
}

/// The notice of a job some items of which failed: `항목 2개를 처리하지 못했습니다`.
pub fn items_failed(count: usize) -> String {
    format!("항목 {count}개를 처리하지 못했습니다")
}

/// `text`, the notice of a job that ended, and how many of its items (`gone`) the location
/// deleted for good when they were to go to the trash, which nothing brings back: `복사를
/// 되돌렸습니다 · 1개는 휴지통 대신 영구 삭제되었습니다`. `text` as it is for none.
pub fn and_gone(text: String, gone: usize) -> String {
    if gone == 0 {
        text
    } else {
        format!("{text} · {gone}개는 휴지통 대신 영구 삭제되었습니다")
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
        // The record of an undo, or one added after this was written.
        _ => JobWork::Other,
    };
    format!("{} 되돌렸습니다", work.phrases().object)
}

/// The notice of a rename or a create that failed, with why: `이름을 바꾸지 못했습니다: 같은
/// 이름의 항목이 이미 있습니다`.
pub fn name_refused(work: JobWork, failure: &ItemFailure) -> String {
    let what = match work {
        JobWork::Create => "새 폴더를 만들지 못했습니다",
        _ => "이름을 바꾸지 못했습니다",
    };
    format!("{what}: {}", item_failure(failure))
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
    "처리하지 못한 항목"
}

/// The label of the toast's button that undoes the job it tells of.
pub fn undo_action() -> &'static str {
    "되돌리기"
}

/// The label of the toast's button that shows the failed items.
pub fn details_action() -> &'static str {
    "자세히"
}

/// The notice when the toast's job is no longer the next one to undo.
pub fn undo_order() -> &'static str {
    "나중에 한 작업이 있어 먼저 그 작업을 되돌려야 합니다"
}

/// The notice when an undo or a redo is asked for while one is under way: one runs at a time.
pub fn undo_busy() -> &'static str {
    "되돌리기나 다시 실행이 진행 중입니다. 끝난 뒤에 다시 해 보세요"
}

/// The default name of a new folder.
pub fn new_folder_name() -> &'static str {
    "새 폴더"
}

/// The title of the window that asks for a new folder's name.
pub fn new_folder_title() -> &'static str {
    "새 폴더"
}

/// The title of the window that asks for an item's new name.
pub fn rename_title() -> &'static str {
    "이름 바꾸기"
}

/// The warning under a name that holds a character no name may have on this system.
pub fn name_has_character(c: char) -> String {
    format!("이름에 {} 문자를 쓸 수 없습니다", quote_char(c))
}

/// The warning under `.` or `..`, which name the folder itself and its parent.
pub fn name_is_dots() -> &'static str {
    "‘.’과 ‘..’은 이름으로 쓸 수 없습니다"
}

/// The title of the confirmation before a permanent delete.
pub fn delete_permanently_title() -> &'static str {
    "영구 삭제"
}

/// The label of the button that deletes for good.
pub fn delete_permanently_button() -> &'static str {
    "영구 삭제"
}

/// The question before a permanent delete the user asked for (Shift+Delete): what goes and
/// that nothing brings it back.
pub fn delete_permanently(count: usize) -> String {
    format!("항목 {count}개를 영구 삭제할까요?\n휴지통을 거치지 않으며 되돌릴 수 없습니다.")
}

/// The question when the folder's location has no trash (OPS-03): the items would go for
/// good, which only the user may choose.
pub fn trash_unavailable(count: usize) -> String {
    format!(
        "휴지통을 쓸 수 없는 위치입니다.\n항목 {count}개를 영구 삭제할까요? 되돌릴 수 없습니다."
    )
}

/// The question when the trash refused some items of a trash job: they are still there, and
/// would go for good.
pub fn trash_refused(count: usize) -> String {
    format!(
        "휴지통이 받지 않은 항목이 {count}개 있습니다.\n이 항목을 영구 삭제할까요? 되돌릴 수 없습니다."
    )
}

/// What the confirmation after an undo of a copy says first, once a worker has found every
/// copy's original as it was: the trash refused the copies, and nothing is lost without them.
pub fn copies_refused() -> &'static str {
    "복사본을 휴지통으로 보낼 수 없습니다. 원본은 그대로 있습니다."
}

/// The question of that confirmation, under the copies' paths.
pub fn delete_copies(count: usize) -> String {
    format!("복사본 {count}개를 영구 삭제할까요? 되돌릴 수 없습니다.")
}

/// The toast once the copies an undo could not trash are deleted for good: the copy is undone.
pub fn copies_deleted(count: usize) -> String {
    format!("복사본 {count}개를 영구 삭제해 복사를 되돌렸습니다")
}

/// The toast of the copies an undo could not trash, when something of them or their
/// originals changed after the user was asked about them: nothing was deleted.
pub fn copies_not_deleted(count: usize) -> String {
    format!("복사본 {count}개를 지우지 않았습니다")
}

/// The notice while a worker looks at the copies an undo could not trash and their originals.
pub fn checking_originals() -> &'static str {
    "원본을 확인하는 중…"
}

/// Why the copies an undo could not trash were not offered for a permanent delete, or not
/// deleted, after [`failure_line`] of a copy: its original is gone, it differs from the copy
/// (another kind, another size, something of the copy missing in it), or it could not be found
/// out; or the copy itself was written to, is gone, or could not be looked at.
pub fn copy_kept(why: CopyKept) -> &'static str {
    match why {
        CopyKept::OriginalMissing => "원본이 없어 복사본을 지우지 않았습니다",
        CopyKept::OriginalChanged => "원본이 바뀌어 복사본을 지우지 않았습니다",
        CopyKept::OriginalUnknown => "원본을 확인할 수 없어 복사본을 지우지 않았습니다",
        CopyKept::CopyChanged => "복사본이 바뀌어 지우지 않았습니다",
        CopyKept::CopyMissing => "복사본이 이미 없습니다",
        CopyKept::CopyUnknown => "복사본을 확인할 수 없어 지우지 않았습니다",
    }
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

/// The names of the items a confirmation is about: up to three, in quotes, and how many more.
pub fn names_line(paths: &[VPath]) -> String {
    const SHOWN: usize = 3;
    let names: Vec<String> = paths
        .iter()
        .take(SHOWN)
        .map(|path| format!("‘{}’", name_of(path)))
        .collect();
    let mut line = names.join(", ");
    if paths.len() > SHOWN {
        let _ = write!(line, " 외 {}개", paths.len() - SHOWN);
    }
    line
}

/// The title of the confirmation before the window closes while jobs run.
pub fn quit_title() -> &'static str {
    "끝낼까요?"
}

/// The label of the button that ends the app while jobs run.
pub fn quit_button() -> &'static str {
    "끝내기"
}

/// The question before the window closes while jobs run: they stop, and the next start offers
/// them again.
pub fn quit_with_jobs(count: usize) -> String {
    format!("진행 중인 작업 {count}개가 있습니다.\n끝내면 다음 실행 때 이어서 할 수 있습니다.")
}

/// What the window shows while the jobs stop before the app ends.
pub fn closing() -> &'static str {
    "작업을 멈추고 있습니다…"
}

/// The title of the window of the jobs that the last run did not finish.
pub fn recovery_title(count: usize) -> String {
    format!("지난번에 끝나지 않은 작업이 {count}개 있습니다")
}

/// What running a recovered job again may do (carry-m1-03 2), and what closing the window
/// does: a job starts over from its first item and meets what it had finished, and a job
/// neither run again nor dropped is offered again at the next start. One sentence a line, so
/// that a line breaks between words.
pub fn recovery_note() -> &'static str {
    "다시 실행하면 처음부터 다시 합니다.\n\
     이미 끝난 항목은 건너뛰거나 번호를 붙인 사본이 생길 수 있습니다.\n\
     이미 옮긴 항목은 찾을 수 없다고 나올 수 있습니다.\n\
     닫으면 다음 실행 때 다시 묻습니다."
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
        line.push_str(" (시작 전)");
    }
    line
}

/// The title of the conflict window, by what is at the target already.
pub fn conflict_title(existing: EntryKind) -> &'static str {
    match existing {
        EntryKind::File => "같은 이름의 파일이 이미 있습니다",
        EntryKind::Dir => "같은 이름의 폴더가 이미 있습니다",
        EntryKind::Symlink | EntryKind::Other => "같은 이름의 항목이 이미 있습니다",
    }
}

/// What the conflict is about: `‘설계안.pdf’가 C:\…\백업 에 이미 있습니다.`
pub fn conflict_description(name: &str, folder: &str) -> String {
    format!(
        "‘{name}’{} {folder} 에 이미 있습니다.",
        subject_particle(name)
    )
}

/// The head of a conflict card: the item that would come, by what `work` does with it (`복사해
/// 올 파일`, `옮겨 올 폴더`), or the one that is there (`이미 있는 파일`), by its `kind`.
pub fn conflict_card(incoming: bool, kind: EntryKind, work: JobWork) -> String {
    let what = item_kind(kind);
    if incoming {
        format!("{} {what}", coming(work))
    } else {
        format!("이미 있는 {what}")
    }
}

/// The hint of the skip choice of a conflict: what `work` leaves out (`이 파일은 복사하지
/// 않습니다`), by the `kind` of the item that would come.
pub fn skip_hint(kind: EntryKind, work: JobWork) -> String {
    let (what, particle) = match kind {
        EntryKind::File => ("파일", "은"),
        EntryKind::Dir => ("폴더", "는"),
        EntryKind::Symlink | EntryKind::Other => ("항목", "은"),
    };
    let verb = match work {
        JobWork::Copy => "복사하지",
        JobWork::Move => "옮기지",
        _ => "가져오지",
    };
    format!("이 {what}{particle} {verb} 않습니다")
}

/// How the item that would come is called by what `work` does with it.
fn coming(work: JobWork) -> &'static str {
    match work {
        JobWork::Copy => "복사해 올",
        JobWork::Move => "옮겨 올",
        // A job this window did not start.
        _ => "가져올",
    }
}

/// An item of `kind` as a conflict names it.
fn item_kind(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "파일",
        EntryKind::Dir => "폴더",
        EntryKind::Symlink | EntryKind::Other => "항목",
    }
}

/// The size of a conflict card: `1.2 MB (1,258,291 바이트)`, and the rounded size alone
/// below 1 KB.
pub fn exact_size(bytes: u64) -> String {
    let rounded = format_size(bytes);
    if bytes < 1024 {
        return rounded;
    }
    let digits = bytes.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{rounded} ({grouped} 바이트)")
}

/// The hint of the overwrite choice, the file that would come named by what `work` does with
/// it, or why there is none: a file and a folder never replace each other.
pub fn overwrite_hint(can_overwrite: bool, work: JobWork) -> String {
    if can_overwrite {
        format!("있던 파일을 {} 파일로 바꿉니다", coming(work))
    } else {
        "파일과 폴더는 서로 덮어쓸 수 없습니다".to_owned()
    }
}

/// The hint of the rename choice, with the first numbered name the job tries.
pub fn rename_hint(numbered: &str) -> String {
    format!("‘{numbered}’처럼 번호를 붙여 저장합니다")
}

/// The warning while overwrite is chosen: what it does to the file that is there.
pub fn overwrite_warning(apply_all: bool) -> &'static str {
    if apply_all {
        "남은 충돌도 모두 덮어씁니다. 있던 파일의 내용은 되살릴 수 없습니다."
    } else {
        "있던 파일의 내용은 되살릴 수 없습니다."
    }
}

/// The items still to come in a job that asks about a conflict.
pub fn items_left(count: u64) -> String {
    format!("이 작업에 남은 항목 {count}개")
}

/// The title of the window of the pre-flight check.
pub fn preflight_title() -> &'static str {
    "대상 위치에 맞지 않는 항목이 있습니다"
}

/// One line of the pre-flight window: the item's name and its [`preflight_issue`].
pub fn preflight_line(target: &VPath, issue: &IssueKind) -> String {
    format!(
        "{}: {}.\n{}",
        name_of(target),
        issue_kind(issue),
        preflight_follows(issue)
    )
}

/// The title of the destination picker.
pub fn pick_title(mode: TransferMode) -> &'static str {
    match mode {
        TransferMode::Copy => "어디로 복사할까요?",
        TransferMode::Move => "어디로 이동할까요?",
    }
}

/// The confirm button of the destination picker.
pub fn pick_confirm(mode: TransferMode) -> &'static str {
    match mode {
        TransferMode::Copy => "여기로 복사",
        TransferMode::Move => "여기로 이동",
    }
}

/// What the picker copies or moves: `항목 3개 · 1.2 GB` for `count` items whose `files`
/// files are `bytes` together, `항목 3개 · 파일 1.2 MB와 폴더 1개` with `folders` folders
/// among them (whose size is not known before the job looks inside), `항목 2개 · 폴더 2개`
/// without files, and `항목 1개` for links alone, which have no size. Every unit of a size ends
/// in a vowel when read, so `와` always follows it.
pub fn pick_summary(count: usize, files: usize, bytes: u64, folders: usize) -> String {
    let what = match (files, folders) {
        (0, 0) => return item_count(count),
        (_, 0) => format_size(bytes),
        (0, folders) => format!("폴더 {folders}개"),
        (_, folders) => format!("파일 {}와 폴더 {folders}개", format_size(bytes)),
    };
    format!("{} · {what}", item_count(count))
}

/// After the names of the first items the picker shows: how many more there are.
pub fn pick_more(count: usize) -> String {
    format!("외 {count}개")
}

/// The heads of the picker's quick targets.
pub fn pick_opposite() -> &'static str {
    "반대편 패널"
}

pub fn pick_tabs() -> &'static str {
    "열린 탭"
}

pub fn pick_recent() -> &'static str {
    "최근 위치"
}

pub fn pick_favorites() -> &'static str {
    "즐겨찾기"
}

/// Why a folder cannot be chosen, or what to beware of when it is; empty for a folder with
/// no problem. A need for room is rounded up and the room free rounded down, so that the two
/// never read as the same number; `at_least` when folders are among the items, whose content
/// adds to the need.
pub fn pick_problem(validity: Validity, mode: TransferMode, at_least: bool) -> String {
    match validity {
        Validity::Ok => String::new(),
        Validity::IntoSource if mode == TransferMode::Copy => {
            "복사하는 항목 안이라 고를 수 없음".to_owned()
        }
        Validity::IntoSource => "옮기는 항목 안이라 고를 수 없음".to_owned(),
        Validity::SameFolder => "원래 있던 폴더라 고를 수 없음".to_owned(),
        Validity::NotAFolder => "폴더를 찾을 수 없어 고를 수 없음".to_owned(),
        Validity::NotWritable => "읽기 전용 위치라 작업이 실패할 수 있습니다".to_owned(),
        Validity::NotEnoughSpace { need, available } => format!(
            "공간이 부족할 수 있습니다: 필요 {}{}, 남은 공간 {}",
            format_size_rounded(need, Rounding::Up),
            if at_least { " 이상" } else { "" },
            format_size_rounded(available, Rounding::Down)
        ),
    }
}

/// The free room at the picker's target: `남은 공간 212 GB`, rounded down like the status
/// bar's.
pub fn pick_space(available: u64) -> String {
    format!(
        "남은 공간 {}",
        format_size_rounded(available, Rounding::Down)
    )
}

/// The picker's space line while a worker looks at the target.
pub fn pick_space_pending() -> &'static str {
    "남은 공간을 확인하는 중…"
}

/// What the picker says while a worker looks at a path typed into it.
pub fn pick_path_pending() -> &'static str {
    "입력한 경로를 확인하는 중…"
}

/// What the picker says while its path field holds a path typed and not entered: its button
/// is off until the path is entered (or another folder is chosen).
pub fn pick_path_unentered() -> &'static str {
    "Enter로 경로를 먼저 확인하세요"
}

/// What the picker says while its path field has been cleared: there is nothing to enter, and
/// its button is off until a path is entered or a folder is chosen.
pub fn pick_path_blank() -> &'static str {
    "경로를 입력하거나 폴더를 고르세요"
}

/// What the picker says while its policy overwrites: Enter does not confirm it.
pub fn pick_overwrite_by_button() -> &'static str {
    "덮어쓰기는 버튼으로만 확인합니다"
}

/// Why a path typed into the picker cannot be the target.
pub fn pick_path_error(kind: ErrorKind) -> String {
    match kind {
        ErrorKind::NotADirectory => "폴더가 아닙니다".to_owned(),
        kind => error_kind(kind).to_owned(),
    }
}

/// What the picker's conflict policy does, said under its buttons; `true` with it when the
/// policy destroys something, which the picker then shows in the text's color and never
/// confirms with Enter alone.
pub fn pick_policy(policy: ConflictPolicy) -> (&'static str, bool) {
    match policy {
        // One sentence a line, so that a narrow window breaks the line between them.
        ConflictPolicy::Overwrite => (
            "같은 이름의 파일을 바꿉니다.\n있던 파일의 내용은 되살릴 수 없습니다.",
            true,
        ),
        ConflictPolicy::Skip => ("같은 이름이 있는 항목은 건너뜁니다", false),
        ConflictPolicy::AutoRename => ("같은 이름이 있으면 번호를 붙여 둘 다 둡니다", false),
        // `Ask`, and the policies the picker does not offer.
        _ => ("같은 이름이 있으면 그때 묻습니다", false),
    }
}

/// What the picker's folder tree shows when its filter finds nothing: it looks only among
/// the folders listed so far.
pub fn pick_no_match() -> &'static str {
    "펼쳐 본 폴더 중에 맞는 이름이 없습니다"
}

/// What a drop of `count` dragged items on the list under the pointer would do, shown at the
/// foot of that list while the drag hovers (DND-03): `항목 3개 이동 → 백업`, with `folder`, the
/// name of the folder they would go into.
pub fn drag_label(count: usize, mode: TransferMode, folder: &str) -> String {
    let what = match mode {
        TransferMode::Copy => "복사",
        TransferMode::Move => "이동",
    };
    format!("{} {what} → {folder}", item_count(count))
}

/// What the window says while a drag of items hovers over a place that does not take them
/// (the pointer says so too): why, for the reason `validity` gives.
pub fn drop_refused(validity: Validity) -> String {
    let why = match validity {
        Validity::IntoSource => "끌고 있는 폴더 자신이거나 그 안입니다",
        Validity::SameFolder => "항목이 이미 이 폴더에 있습니다",
        Validity::NotAFolder => "폴더가 아닙니다",
        // A place that takes them with a warning is no refusal.
        Validity::Ok | Validity::NotWritable | Validity::NotEnoughSpace { .. } => {
            return "여기에 놓을 수 없습니다".to_owned();
        }
    };
    format!("여기에 놓을 수 없습니다: {why}")
}

/// What the preview panel and quick look say of an item that cannot be shown: it could not be
/// read, or its content is broken (a damaged image, bytes not valid in their encoding).
pub fn preview_unavailable() -> &'static str {
    "미리 볼 수 없습니다"
}

/// What the preview says of an image file too large to decode (over 50 MB): its size shows
/// beside it.
pub fn preview_too_large() -> &'static str {
    "파일이 커서 미리 보지 않습니다"
}

/// How many items a folder in the preview shows: `항목 12개`, or `항목 10000개 이상` past the
/// count's cap (`more`).
pub fn preview_items(items: usize, more: bool) -> String {
    let count = item_count(items);
    if more {
        format!("{count} 이상")
    } else {
        count
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
        format!("{encoding} · 앞 {shown}만 보입니다")
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
    "온라인 전용 파일은 미리 보지 않습니다"
}

/// The notice when the preview panel is hidden because the window is too narrow for it; it
/// comes back when the window is wide enough.
pub fn preview_no_room() -> &'static str {
    "창이 좁아 미리 보기 패널을 숨겼습니다. 창을 넓히면 다시 보입니다"
}

/// The particle `이` or `가` after `word`, as it is read: `이` after a final consonant.
/// Hangul is read by its last syllable, digits and Latin letters by their Korean names
/// (`1` 일, `L` 엘, `M` 엠, `N` 엔, `R` 알); anything else takes `가`.
fn subject_particle(word: &str) -> &'static str {
    let Some(last) = word.chars().last() else {
        return "가";
    };
    let final_consonant = match last {
        '가'..='힣' => (u32::from(last) - u32::from('가')) % 28 != 0,
        '0' | '1' | '3' | '6' | '7' | '8' => true,
        c if c.is_ascii_alphabetic() => matches!(c.to_ascii_lowercase(), 'l' | 'm' | 'n' | 'r'),
        _ => false,
    };
    if final_consonant { "이" } else { "가" }
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
            "시스템 자원이 부족해 일부 내용을 불러오지 못했습니다"
        );
    }

    #[test]
    fn status_line_matches_mockup() {
        const GB: u64 = 1 << 30;
        assert_eq!(
            status_line(9, 2, Some(1_477_427), Some(212 * GB)),
            "항목 9개 · 선택 2개 (1.4 MB) · 남은 공간 212 GB"
        );
        // Folders alone have no size; nothing selected shows no selection.
        assert_eq!(
            status_line(9, 1, None, Some(212 * GB)),
            "항목 9개 · 선택 1개 · 남은 공간 212 GB"
        );
        assert_eq!(
            status_line(9, 0, None, Some(212 * GB)),
            "항목 9개 · 남은 공간 212 GB"
        );
        assert_eq!(status_line(0, 0, None, None), "항목 0개");
        // Free space rounds down, as in the sidebar: 4.66 GB is 4.6 GB in both places.
        let free = 4 * GB + 2 * GB / 3;
        assert_eq!(
            status_line(1, 0, None, Some(free)),
            "항목 1개 · 남은 공간 4.6 GB"
        );
        assert!(drive_space(free, 64 * GB).starts_with("4.6 GB 남음"));
        assert_eq!(
            status_line(3, 3, Some(0), None),
            "항목 3개 · 선택 3개 (0 바이트)"
        );
        // While a filter hides rows, the line says so and counts both.
        assert_eq!(
            filtered_status_line("보고", (2, 9), 1, None, Some(212 * GB)),
            "거르는 중: ‘보고’ · 항목 2개 / 9개 · 선택 1개 · 남은 공간 212 GB"
        );
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

    const WORKS: [JobWork; 9] = [
        JobWork::Copy,
        JobWork::Move,
        JobWork::Trash,
        JobWork::Delete,
        JobWork::Rename,
        JobWork::Create,
        JobWork::Undo,
        JobWork::Redo,
        JobWork::Other,
    ];

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
            drag_label(3, TransferMode::Move, "백업"),
            "항목 3개 이동 → 백업"
        );
        assert_eq!(
            drag_label(1, TransferMode::Copy, "Downloads"),
            "항목 1개 복사 → Downloads"
        );
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
}
