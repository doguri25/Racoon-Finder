//! The Korean words the app shows for errors, refusals and jobs (I18N-02).
//!
//! The `Display` text of `rf_core::Error` and of the job types is English and for the log; the
//! screens show these words instead. They say what happened to the user's item, not how the
//! engine works.

use rf_core::ErrorKind;
use rf_jobs::{DeleteMethod, FailureReason, IssueKind, JobKind, JobSpec, JobState, Outcome};

use crate::format::format_size;

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
    /// An undo, or a redo (which undoes the undo).
    Undo,
    /// A kind of job this version has no words for.
    Other,
}

impl JobWork {
    /// The work `spec` asks for.
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
        ErrorKind::NotADirectory => "경로 중간에 폴더가 아닌 항목이 있습니다",
        ErrorKind::IsADirectory => "파일이 아니라 폴더입니다",
        ErrorKind::DirectoryNotEmpty => "폴더가 비어 있지 않습니다",
        ErrorKind::StorageFull => "디스크 공간이 부족합니다",
        ErrorKind::ReadOnly => "읽기 전용 위치입니다",
        ErrorKind::CrossesDevices => "다른 드라이브로는 한 번에 옮길 수 없습니다",
        ErrorKind::Busy => "다른 프로그램이 사용 중입니다",
        ErrorKind::TimedOut => "응답이 너무 늦어 중단했습니다",
        ErrorKind::Interrupted => "작업이 중간에 끊겼습니다. 다시 해 보세요",
        ErrorKind::FileTooLarge => "파일이 너무 커서 이 위치에 둘 수 없습니다",
        ErrorKind::Unsupported => "이 항목이나 위치에서는 할 수 없는 작업입니다",
        ErrorKind::Cancelled => "취소되었습니다",
        // `Other`, and any kind added after this was written.
        _ => "알 수 없는 오류가 났습니다",
    }
}

/// Why a job refused an item, or part of one: one sentence for each rule of the jobs, or the
/// words of the [`IssueKind`] for a name or size the target cannot hold.
pub fn failure_reason(reason: &FailureReason) -> String {
    let words = match reason {
        FailureReason::IntoItself => "폴더를 자기 안으로 옮길 수 없습니다",
        FailureReason::NoTargetFolder => "대상이 드라이브 자체라 그 자리에 둘 수 없습니다",
        FailureReason::TargetNotAFolder => "대상 위치가 폴더가 아닙니다",
        FailureReason::TooDeep => "폴더 안 너무 깊은 곳에 있어 건너뛰었습니다",
        FailureReason::LinkLoop => "바로 가기가 위쪽 폴더로 되돌아가는 고리라 들어가지 않았습니다",
        FailureReason::TooManyLinks => "바로 가기가 너무 여러 번 이어져 따라갈 수 없습니다",
        FailureReason::LinkChanged => "따라가는 동안 바로 가기가 바뀌었습니다",
        FailureReason::AbsoluteLink => "이 위치의 바로 가기는 따라갈 수 없습니다",
        FailureReason::NotARegularFile => "장치나 파이프 같은 특수 파일이라 건너뛰었습니다",
        FailureReason::SourceCutShort => {
            "복사하는 동안 다른 프로그램이 원본을 줄여 사본을 남기지 않았습니다"
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
        FailureReason::Issue(issue) => return issue_kind(issue),
        _ => "알 수 없는 이유로 처리하지 못했습니다",
    };
    words.to_owned()
}

/// What keeps an item from fitting its target as it is, with the numbers or the name involved.
pub fn issue_kind(issue: &IssueKind) -> String {
    match issue {
        IssueKind::InvalidCharacter(c) => {
            format!(
                "대상 위치에서는 이름에 {} 문자를 쓸 수 없습니다",
                quote_char(*c)
            )
        }
        IssueKind::ReservedName => {
            "Windows가 장치에 쓰는 이름(CON, NUL 등)이라 나중에 열기 어려울 수 있습니다".to_owned()
        }
        IssueKind::TrailingDotOrSpace => {
            "이름이 마침표나 공백으로 끝나 Windows에서 열기 어려울 수 있습니다".to_owned()
        }
        IssueKind::NameTooLong { len, max } => {
            format!("이름이 너무 깁니다 (길이 {len}, 최대 {max})")
        }
        IssueKind::PathTooLong { len, max } => {
            format!("전체 경로가 너무 깁니다 (길이 {len}, 최대 {max})")
        }
        IssueKind::FileTooLarge { size, max } => format!(
            "파일이 대상 위치에 둘 수 있는 크기보다 큽니다 ({}, 최대 {})",
            format_size(*size),
            format_size(*max)
        ),
        IssueKind::CaseCollision { other } => format!(
            "대소문자만 다른 ‘{}’ 항목이 이미 있습니다",
            other.to_string_lossy()
        ),
        IssueKind::NotEnoughSpace { needed, available } => format!(
            "대상 드라이브의 공간이 부족합니다 (필요 {}, 남은 공간 {})",
            format_size(*needed),
            format_size(*available)
        ),
        _ => "대상 위치에 이 항목을 둘 수 없습니다".to_owned(),
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
/// 못했습니다`, `이동을 취소했습니다`, `문제가 생겨 삭제를 멈췄습니다`.
pub fn outcome(outcome: Outcome, work: JobWork) -> String {
    let Phrases { done, object, .. } = work.phrases();
    match outcome {
        Outcome::Completed => format!("{done}습니다"),
        Outcome::CompletedWithErrors => format!("{done}지만 일부 항목을 처리하지 못했습니다"),
        Outcome::Cancelled => format!("{object} 취소했습니다"),
        Outcome::Aborted => format!("문제가 생겨 {object} 멈췄습니다"),
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

        let mut seen = HashSet::new();
        for kind in ERROR_KINDS {
            let words = error_kind(kind);
            assert!(is_korean(words), "{kind:?}: {words:?}");
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
            "폴더를 자기 안으로 옮길 수 없습니다"
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
        // An issue reads as the issue itself.
        let issue = IssueKind::NameTooLong { len: 300, max: 255 };
        assert_eq!(
            failure_reason(&FailureReason::Issue(issue.clone())),
            issue_kind(&issue)
        );
    }

    #[test]
    fn every_issue_kind_has_words() {
        let issues = [
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
        ];
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
        assert!(issue_kind(&issues[3]).contains("300") && issue_kind(&issues[3]).contains("255"));
        assert!(issue_kind(&issues[4]).contains("260"));
        assert!(issue_kind(&issues[5]).contains("5 GB") && issue_kind(&issues[5]).contains("4 GB"));
        assert!(issue_kind(&issues[6]).contains("Readme.md"));
        assert!(issue_kind(&issues[7]).contains("3 MB") && issue_kind(&issues[7]).contains("1 MB"));
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
        let undo = JobSpec::Undo(UndoRecord::Trashed { items: Vec::new() });
        assert_eq!(JobWork::of(&undo), JobWork::Undo);
    }

    const WORKS: [JobWork; 8] = [
        JobWork::Copy,
        JobWork::Move,
        JobWork::Trash,
        JobWork::Delete,
        JobWork::Rename,
        JobWork::Create,
        JobWork::Undo,
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
            "문제가 생겨 삭제를 멈췄습니다"
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
