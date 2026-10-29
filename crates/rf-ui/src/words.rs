//! The Korean words the app shows for errors, refusals, jobs, the status bar and the sidebar
//! (I18N-02).
//!
//! The `Display` text of `rf_core::Error` and of the job types is English and for the log; the
//! screens show these words instead. They say what happened to the user's item, not how the
//! engine works.

use rf_core::ErrorKind;
use rf_jobs::{
    DeleteMethod, FailureReason, IssueKind, JobKind, JobSpec, JobState, Outcome, Severity,
};
use rf_platform::KnownFolder;

use crate::format::{Rounding, format_size, format_size_rounded};

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
        let (doing, done, object) = match self {
            JobWork::Copy => ("복사 중", "복사했", "복사를"),
            JobWork::Move => ("이동 중", "이동했", "이동을"),
            JobWork::Trash => (
                "휴지통으로 보내는 중",
                "휴지통으로 보냈",
                "휴지통으로 보내기를",
            ),
            JobWork::Delete => ("삭제 중", "삭제했", "삭제를"),
            JobWork::Rename => ("이름 바꾸는 중", "이름을 바꿨", "이름 바꾸기를"),
            JobWork::Create => ("만드는 중", "만들었", "만들기를"),
            JobWork::Undo => ("되돌리는 중", "되돌렸", "되돌리기를"),
            JobWork::Redo => ("다시 실행하는 중", "다시 실행했", "다시 실행을"),
            JobWork::Other => ("작업 중", "작업을 마쳤", "작업을"),
        };
        Phrases {
            doing,
            done,
            object,
        }
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
/// the space left on the volume, shown once known.
pub fn status_line(
    items: usize,
    selected: usize,
    selected_bytes: Option<u64>,
    free: Option<u64>,
) -> String {
    let mut parts = vec![format!("항목 {items}개")];
    match (selected, selected_bytes) {
        (0, _) => {}
        (count, Some(bytes)) => parts.push(format!("선택 {count}개 ({})", format_size(bytes))),
        (count, None) => parts.push(format!("선택 {count}개")),
    }
    if let Some(free) = free {
        parts.push(format!("남은 공간 {}", format_size(free)));
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
    let follows = match issue {
        IssueKind::NotEnoughSpace { .. } => "계속하면 공간이 모자라 중간에 멈출 수 있습니다",
        _ if issue.severity() == Severity::Warning => {
            "그대로 두면 Windows에서 열기 어려울 수 있습니다"
        }
        _ => "이 항목은 대상 위치에 둘 수 없습니다",
    };
    format!("{}. {follows}", issue_kind(issue))
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::ffi::OsString;

    use rf_core::VPath;
    use rf_jobs::{
        CreateKind, DeleteSpec, ErrorPolicy, TransferMode, TransferOptions, TransferSpec,
        UndoRecord,
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
        assert_eq!(
            status_line(3, 3, Some(0), None),
            "항목 3개 · 선택 3개 (0 바이트)"
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
}
