//! The file commands, the jobs, the toasts and the dialogs.

use rf_jobs::{
    DeleteMethod, DeleteSpec, ErrorPolicy, FailureReason, ItemFailure, JobEvent, JobSpec, Outcome,
    TransferMode, TransferOptions, TransferSpec, UndoRecord,
};

use super::*;
use crate::dialogs::{ConfirmPurpose, ConflictChoice, Modal};
use crate::jobs::{History, JobEffect, Started};
use crate::resolver::{Asked, Question};
use crate::words::JobWork;

/// The one job `effects` submit, and the pane that asked for it.
fn the_submit(effects: &[Effect]) -> (JobSpec, Option<usize>) {
    let submits: Vec<(JobSpec, Option<usize>)> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Job(JobEffect::Submit { spec, origin }) => Some((spec.clone(), *origin)),
            _ => None,
        })
        .collect();
    match submits.as_slice() {
        [one] => one.clone(),
        other => panic!("expected one job, got {other:?} in {effects:?}"),
    }
}

fn in_a(name: &str) -> VPath {
    folder("a").join(name).expect("a plain name")
}

fn delete(paths: Vec<VPath>, method: DeleteMethod) -> JobSpec {
    JobSpec::Delete(DeleteSpec {
        paths,
        method,
        errors: ErrorPolicy::Continue,
    })
}

#[test]
fn new_folder_asks_for_a_name_then_creates_it() {
    let mut state = ready(vec![file("x")], vec![]);
    assert_eq!(press(&mut state, &special(Key::F7), NONE), Some(vec![]));
    let Some(Modal::Name(dialog)) = state.modal() else {
        panic!("a name dialog: {:?}", state.modal());
    };
    assert_eq!(dialog.initial, "새 폴더");
    // The dialog has the keys now: a shortcut that comes here anyway does nothing.
    assert_eq!(press(&mut state, &special(Key::Delete), NONE), Some(vec![]));
    // A name that cannot be one keeps the dialog open.
    state.name_edited("a/b");
    assert_eq!(state.name_confirmed("a/b"), []);
    assert!(matches!(state.modal(), Some(Modal::Name(_))));
    let effects = state.name_confirmed("자료");
    assert_eq!(
        the_submit(&effects),
        (
            JobSpec::Create {
                parent: folder("a"),
                name: "자료".into(),
                kind: rf_jobs::CreateKind::Dir,
            },
            Some(LEFT)
        )
    );
    assert_eq!(state.modal(), None);
    // Ctrl+Shift+N opens it too, and Esc closes it without a job.
    press(
        &mut state,
        "N",
        Mods {
            shift: true,
            ..CTRL
        },
    );
    assert!(matches!(state.modal(), Some(Modal::Name(_))));
    assert_eq!(state.dialog_cancelled(), []);
    assert_eq!(state.modal(), None);
}

#[test]
fn rename_starts_from_the_cursor_row() {
    // In an empty folder F2 has nothing to rename.
    let mut empty = ready(vec![], vec![]);
    assert_eq!(press(&mut empty, &special(Key::F2), NONE), Some(vec![]));
    assert_eq!(empty.modal(), None);
    // A folder just listed has the cursor on its first row: F2 renames that.
    let mut state = ready(vec![dir("docs"), file("설계안.pdf")], vec![]);
    press(&mut state, &special(Key::F2), NONE);
    assert!(matches!(state.modal(), Some(Modal::Name(dialog)) if dialog.initial == "docs"));
    state.dialog_cancelled();
    state.click_row(LEFT, 1, false, false);
    press(&mut state, &special(Key::F2), NONE);
    let Some(Modal::Name(dialog)) = state.modal() else {
        panic!("a name dialog");
    };
    assert_eq!(&dialog.initial[..dialog.select_end], "설계안");
    // The name it has closes the dialog with nothing to do.
    assert_eq!(state.name_confirmed("설계안.pdf"), []);
    assert_eq!(state.modal(), None);
    press(&mut state, &special(Key::F2), NONE);
    let effects = state.name_confirmed("최종안.pdf");
    assert_eq!(
        the_submit(&effects),
        (
            JobSpec::Rename {
                from: in_a("설계안.pdf"),
                to: in_a("최종안.pdf"),
            },
            Some(LEFT)
        )
    );
}

#[test]
fn delete_checks_for_a_trash_and_never_deletes_for_good_unasked() {
    let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
    state.click_row(LEFT, 0, false, false);
    state.click_row(LEFT, 1, true, false);
    let paths = vec![in_a("a.txt"), in_a("b.txt")];
    let effects = press(&mut state, &special(Key::Delete), NONE).expect("used");
    assert_eq!(
        effects,
        [Effect::CheckTrash {
            folder: folder("a"),
            paths: paths.clone(),
        }]
    );
    // With a trash, the items go there.
    assert_eq!(
        the_submit(&state.trash_checked(paths.clone(), true)).0,
        delete(paths.clone(), DeleteMethod::Trash)
    );
    // Without one, nothing is submitted until the user says yes (OPS-03).
    assert_eq!(state.trash_checked(paths.clone(), false), []);
    let Some(Modal::Confirm(confirm)) = state.modal() else {
        panic!("a confirmation");
    };
    assert_eq!(
        confirm.message,
        "휴지통을 쓸 수 없는 위치입니다.\n항목 2개를 영구 삭제할까요? 되돌릴 수 없습니다."
    );
    assert!(confirm.danger);
    // No keeps the items.
    assert_eq!(state.dialog_cancelled(), []);
    assert_eq!(state.modal(), None);
    state.trash_checked(paths.clone(), false);
    assert_eq!(
        the_submit(&state.confirm_accepted()).0,
        delete(paths.clone(), DeleteMethod::Permanent)
    );
    assert_eq!(state.modal(), None);

    // Shift+Delete asks first too.
    assert_eq!(
        press(&mut state, &special(Key::Delete), SHIFT),
        Some(vec![])
    );
    let Some(Modal::Confirm(confirm)) = state.modal() else {
        panic!("a confirmation");
    };
    assert_eq!(
        confirm.message,
        "항목 2개를 영구 삭제할까요?\n휴지통을 거치지 않으며 되돌릴 수 없습니다."
    );
    assert_eq!(confirm.purpose, ConfirmPurpose::DeletePermanently(paths));
}

/// Ends the job `id` that `spec` describes and the state followed as `work`, with the
/// report `edit` makes, and gives what the state asks for.
fn finish(
    state: &mut AppState,
    id: u64,
    spec: &JobSpec,
    work: JobWork,
    edit: impl FnOnce(&mut JobReport),
) -> Vec<Effect> {
    without_space_reads(finish_all(state, id, spec, work, edit))
}

/// [`finish`] with every effect, the read of the free space among them.
fn finish_all(
    state: &mut AppState,
    id: u64,
    spec: &JobSpec,
    work: JobWork,
    edit: impl FnOnce(&mut JobReport),
) -> Vec<Effect> {
    state.track(Started {
        id: JobId(id),
        spec: Some(Arc::new(spec.clone())),
        work,
        origin: Some(LEFT),
        via_history: matches!(work, JobWork::Undo | JobWork::Redo),
    });
    let mut report = JobReport::new(JobId(id), spec);
    edit(&mut report);
    state.job_event(JobEvent::Finished {
        id: JobId(id),
        report: Arc::new(report),
    })
}

fn failure(source: VPath, kind: ErrorKind, reason: Option<FailureReason>) -> ItemFailure {
    ItemFailure {
        source: Some(source),
        target: None,
        stage: rf_jobs::Stage::Delete,
        kind,
        message: String::new(),
        reason,
    }
}

#[test]
fn items_the_trash_refused_are_offered_for_a_permanent_delete() {
    let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
    let spec = delete(vec![in_a("a.txt"), in_a("b.txt")], DeleteMethod::Trash);
    let effects = finish(&mut state, 1, &spec, JobWork::Trash, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.failures.push(failure(
            in_a("b.txt"),
            ErrorKind::Unsupported,
            Some(FailureReason::TrashRefused),
        ));
    });
    // Nothing is deleted yet, and the confirmation is all that tells of the refusal.
    assert!(
        effects
            .iter()
            .all(|effect| !matches!(effect, Effect::Job(_) | Effect::Toast { .. })),
        "{effects:?}"
    );
    let Some(Modal::Confirm(confirm)) = state.modal() else {
        panic!("a confirmation");
    };
    assert_eq!(
        confirm.purpose,
        ConfirmPurpose::DeletePermanently(vec![in_a("b.txt")])
    );
    assert!(
        confirm
            .message
            .starts_with("휴지통이 받지 않은 항목이 1개 있습니다"),
        "{}",
        confirm.message
    );
    assert_eq!(
        the_submit(&state.confirm_accepted()).0,
        delete(vec![in_a("b.txt")], DeleteMethod::Permanent)
    );
}

fn moved(from: &str) -> UndoRecord {
    UndoRecord::Moved {
        moves: vec![rf_jobs::MovedItem {
            from: in_a(from),
            to: folder("b").join(from).expect("a plain name"),
        }],
        created_dirs: Vec::new(),
        removed_dirs: Vec::new(),
        overwritten: 0,
        copied_dirs: Vec::new(),
    }
}

/// The token, text and button of the one toast among `effects`.
fn the_toast(effects: &[Effect]) -> (u64, String, String) {
    let toasts: Vec<(u64, String, String)> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Toast {
                token,
                text,
                action,
            } => Some((*token, text.clone(), action.clone())),
            _ => None,
        })
        .collect();
    match toasts.as_slice() {
        [one] => one.clone(),
        other => panic!("expected one toast, got {other:?} in {effects:?}"),
    }
}

#[test]
fn a_finished_move_offers_its_undo_while_it_is_next() {
    // Nothing to move in an empty folder.
    let options = TransferOptions::default();
    let mut empty = ready(vec![], vec![]);
    assert_eq!(
        empty.transfer(LEFT, TransferMode::Move, &folder("b"), options),
        []
    );
    let mut state = ready(vec![file("a.txt")], vec![]);
    state.click_row(LEFT, 0, false, false);
    let effects = state.transfer(LEFT, TransferMode::Move, &folder("b"), options);
    let (spec, origin) = the_submit(&effects);
    assert_eq!(origin, Some(LEFT));
    let record = moved("a.txt");
    // The engine notes the record before it tells of the end.
    state.set_history(History {
        top: Some(record.clone()),
        can_redo: false,
    });
    let effects = finish(&mut state, 1, &spec, JobWork::Move, |report| {
        report.undo = Some(record.clone());
    });
    let (token, text, action) = the_toast(&effects);
    assert_eq!(
        (text.as_str(), action.as_str()),
        ("항목 1개를 이동했습니다", "되돌리기")
    );
    assert!(state.can_undo());
    assert_eq!(state.toast_clicked(token), [Effect::Job(JobEffect::Undo)]);
    // The toast's button works once.
    assert_eq!(state.toast_clicked(token), []);

    // An undo from the keyboard leaves the toast's button with nothing to do.
    let effects = finish(&mut state, 3, &spec, JobWork::Move, |report| {
        report.undo = Some(record.clone());
    });
    let (token, _, _) = the_toast(&effects);
    assert_eq!(
        press(&mut state, "z", CTRL),
        Some(vec![Effect::Job(JobEffect::Undo)])
    );
    assert_eq!(state.toast_clicked(token), []);

    // A later job is the next to undo: the toast of the move does not undo it.
    let effects = finish(&mut state, 2, &spec, JobWork::Move, |report| {
        report.undo = Some(record.clone());
    });
    let (token, _, _) = the_toast(&effects);
    state.set_history(History {
        top: Some(UndoRecord::Renamed {
            from: in_a("x"),
            to: in_a("y"),
        }),
        can_redo: false,
    });
    assert_eq!(
        state.toast_clicked(token),
        [Effect::Notice(
            "나중에 한 작업이 있어 먼저 그 작업을 되돌려야 합니다".to_owned()
        )]
    );
}

#[test]
fn undo_is_off_while_an_undo_runs() {
    let mut state = ready(vec![], vec![]);
    // Nothing to undo: Ctrl+Z does nothing.
    assert!(!state.can_undo());
    assert_eq!(press(&mut state, "z", CTRL), Some(vec![]));
    let record = moved("a.txt");
    state.set_history(History {
        top: Some(record.clone()),
        can_redo: false,
    });
    assert_eq!(
        press(&mut state, "z", CTRL),
        Some(vec![Effect::Job(JobEffect::Undo)])
    );
    // The engine starts the undo; until it ends, neither undo nor redo starts another.
    state.track(Started {
        id: JobId(9),
        spec: None,
        work: JobWork::Undo,
        origin: None,
        via_history: true,
    });
    state.set_history(History {
        top: None,
        can_redo: false,
    });
    assert!(!state.can_undo() && !state.can_redo());
    assert_eq!(press(&mut state, "z", CTRL), Some(vec![]));
    assert_eq!(press(&mut state, "y", CTRL), Some(vec![]));
    // It ends: the toast names what it undid, and the redo can start.
    state.set_history(History {
        top: None,
        can_redo: true,
    });
    let spec = JobSpec::Undo(record);
    let mut report = JobReport::new(JobId(9), &spec);
    report.outcome = Outcome::Completed;
    let effects = state.job_event(JobEvent::Finished {
        id: JobId(9),
        report: Arc::new(report),
    });
    assert_eq!(the_toast(&effects).1, "이동을 되돌렸습니다");
    assert!(state.can_redo());
    assert_eq!(
        press(
            &mut state,
            "z",
            Mods {
                shift: true,
                ..CTRL
            }
        ),
        Some(vec![Effect::Job(JobEffect::Redo)])
    );
}

fn conflict_question(id: u64) -> Question {
    Question {
        id,
        job: JobId(1),
        asked: Asked::Conflict(Box::new(rf_jobs::Conflict {
            source: folder("b").join("a.txt").expect("a name"),
            target: in_a("a.txt"),
            incoming: Entry::new("a.txt", EntryKind::File),
            existing: Entry::new("a.txt", EntryKind::File),
        })),
    }
}

#[test]
fn questions_wait_behind_the_open_dialog_and_answer_the_job() {
    let mut state = ready(vec![file("x")], vec![]);
    press(&mut state, &special(Key::F7), NONE);
    assert_eq!(state.question(conflict_question(5)), []);
    assert!(matches!(state.modal(), Some(Modal::Name(_))));
    state.dialog_cancelled();
    let Some(Modal::Conflict(question)) = state.modal() else {
        panic!("the conflict shows next");
    };
    assert_eq!(question.id, 5);
    assert_eq!(
        state.conflict_answered(ConflictChoice::Overwrite, true),
        [Effect::Job(JobEffect::AnswerConflict(
            5,
            rf_jobs::ConflictAnswer::all(rf_jobs::ConflictAction::Overwrite)
        ))]
    );
    assert_eq!(state.modal(), None);

    // Esc stops the job of a question: its cancelling answer.
    state.question(conflict_question(7));
    assert_eq!(
        state.dialog_cancelled(),
        [Effect::Job(JobEffect::AnswerConflict(
            7,
            rf_jobs::ConflictAnswer::once(rf_jobs::ConflictAction::Cancel)
        ))]
    );
    let preflight = Question {
        id: 8,
        job: JobId(2),
        asked: Asked::Preflight(Vec::new()),
    };
    state.question(preflight.clone());
    assert_eq!(
        state.preflight_answered(1),
        [Effect::Job(JobEffect::AnswerPreflight(
            8,
            rf_jobs::PreflightAnswer::SkipBlocked
        ))]
    );
    state.question(preflight);
    assert_eq!(
        state.dialog_cancelled(),
        [Effect::Job(JobEffect::AnswerPreflight(
            8,
            rf_jobs::PreflightAnswer::Cancel
        ))]
    );
}

#[test]
fn closing_with_jobs_asks_first_then_leaves_questions_for_the_release() {
    // Without a job the app ends at once.
    let mut idle = ready(vec![], vec![]);
    assert_eq!(idle.request_close(), [Effect::Quit]);
    assert!(idle.quitting());
    assert_eq!(idle.request_close(), []);

    let mut state = ready(vec![file("x")], vec![]);
    let spec = delete(vec![in_a("x")], DeleteMethod::Trash);
    state.track(Started {
        id: JobId(1),
        spec: Some(Arc::new(spec)),
        work: JobWork::Trash,
        origin: None,
        via_history: false,
    });
    state.question(conflict_question(3));
    // The confirmation shows over the question.
    assert_eq!(state.request_close(), []);
    let Some(Modal::Confirm(confirm)) = state.modal() else {
        panic!("a confirmation");
    };
    assert_eq!(confirm.purpose, ConfirmPurpose::Quit);
    assert_eq!(
        confirm.message,
        "진행 중인 작업 1개가 있습니다.\n끝내면 다음 실행 때 이어서 할 수 있습니다."
    );
    // Staying brings the question back, still unanswered.
    assert_eq!(state.dialog_cancelled(), []);
    assert!(matches!(state.modal(), Some(Modal::Conflict(q)) if q.id == 3));
    state.request_close();
    assert_eq!(state.confirm_accepted(), [Effect::Quit]);
    assert!(state.quitting());
    assert_eq!(state.modal(), None);
    // From now on nothing shows and nothing is answered here: the engine shuts down first,
    // then the release answers the open questions (carry-m1-03 "quit order").
    assert_eq!(state.question(conflict_question(4)), []);
    assert_eq!(state.modal(), None);
    assert_eq!(press(&mut state, "z", CTRL), Some(vec![]));
    assert_eq!(state.request_close(), []);
}

fn incomplete(id: u64) -> rf_jobs::IncompleteJob {
    rf_jobs::IncompleteJob {
        id: JobId(id),
        spec: delete(vec![in_a("x")], DeleteMethod::Trash),
        submitted: std::time::SystemTime::now(),
        started: true,
        temp_dirs: Vec::new(),
    }
}

#[test]
fn unfinished_jobs_are_offered_again() {
    let mut state = ready(vec![], vec![]);
    let effects = state.job_event(JobEvent::Recovered {
        jobs: Vec::new(),
        temp_files_removed: 0,
    });
    assert_eq!(effects, []);
    assert_eq!(state.modal(), None);
    state.job_event(JobEvent::Recovered {
        jobs: vec![incomplete(1), incomplete(2)],
        temp_files_removed: 3,
    });
    assert!(matches!(state.modal(), Some(Modal::Recovery(r)) if r.jobs.len() == 2));
    assert_eq!(
        state.recovery_chosen(0, true),
        [Effect::Job(JobEffect::Resubmit {
            id: JobId(1),
            spec: incomplete(1).spec,
        })]
    );
    assert!(matches!(state.modal(), Some(Modal::Recovery(r)) if r.jobs.len() == 1));
    assert_eq!(state.recovery_chosen(5, false), []);
    assert_eq!(
        state.recovery_chosen(0, false),
        [Effect::Job(JobEffect::Dismiss(JobId(2)))]
    );
    assert_eq!(state.modal(), None);
}

#[test]
fn the_new_folder_is_selected_when_it_shows() {
    let mut state = ready(vec![file("x")], vec![]);
    let spec = JobSpec::Create {
        parent: folder("a"),
        name: "새 폴더".into(),
        kind: rf_jobs::CreateKind::Dir,
    };
    let effects = finish(&mut state, 1, &spec, JobWork::Create, |report| {
        report.outputs.push(in_a("새 폴더"));
    });
    // Not listed yet: nothing to scroll to, and no toast for a create.
    assert!(effects.is_empty(), "{effects:?}");
    let generation = state.panes()[LEFT].active_tab().generation();
    let effects = state.loaded(LoadEvent::Changes {
        generation,
        changes: vec![rf_vfs::Change::Upsert(dir("새 폴더"))],
    });
    assert_eq!(effects, [Effect::ScrollTo { side: LEFT, row: 0 }]);
    assert_eq!(rows(&state, LEFT), ["새 폴더", "x"]);
    assert_eq!(selected(&state, LEFT), [0]);
    assert_eq!(cursor(&state, LEFT), Some(0));

    // A rename that is listed already is selected at once.
    let spec = JobSpec::Rename {
        from: in_a("x"),
        to: in_a("x"),
    };
    let effects = finish(&mut state, 2, &spec, JobWork::Rename, |report| {
        report.outputs.push(in_a("x"));
    });
    assert_eq!(effects, [Effect::ScrollTo { side: LEFT, row: 1 }]);
    assert_eq!(selected(&state, LEFT), [1]);
}

#[test]
fn a_failed_rename_says_why_and_a_failed_copy_lists_its_items() {
    let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
    let spec = JobSpec::Rename {
        from: in_a("a.txt"),
        to: in_a("b.txt"),
    };
    let effects = finish(&mut state, 1, &spec, JobWork::Rename, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report
            .failures
            .push(failure(in_a("a.txt"), ErrorKind::AlreadyExists, None));
    });
    assert_eq!(
        effects,
        [Effect::Notice(
            "이름을 바꾸지 못했습니다: 같은 이름의 항목이 이미 있습니다".to_owned()
        )]
    );

    let spec = JobSpec::Transfer(
        TransferSpec::into_dir(
            TransferMode::Copy,
            &[in_a("a.txt"), in_a("b.txt")],
            &folder("b"),
            TransferOptions::default(),
        )
        .expect("a transfer"),
    );
    let effects = finish(&mut state, 2, &spec, JobWork::Copy, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report
            .failures
            .push(failure(in_a("a.txt"), ErrorKind::PermissionDenied, None));
        report.failures.push(failure(
            in_a("b.txt"),
            ErrorKind::Unsupported,
            Some(FailureReason::NotARegularFile),
        ));
    });
    let (token, text, action) = the_toast(&effects);
    assert_eq!(
        (text.as_str(), action.as_str()),
        ("항목 2개를 처리하지 못했습니다", "자세히")
    );
    assert_eq!(state.toast_clicked(token), []);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the failed items");
    };
    assert_eq!(failures.title, "처리하지 못한 항목");
    assert_eq!(
        failures.lines,
        [
            format!("{}: 권한이 없습니다", in_a("a.txt")),
            format!(
                "{}: 장치나 파이프 같은 특수 파일이라 건너뛰었습니다",
                in_a("b.txt")
            ),
        ]
    );
    // A job the user cancelled tells nothing.
    state.dialog_cancelled();
    let effects = finish(&mut state, 3, &spec, JobWork::Copy, |report| {
        report.outcome = Outcome::Cancelled;
    });
    assert_eq!(effects, []);
}

#[test]
fn the_job_panel_buttons_reach_their_job() {
    let mut state = ready(vec![], vec![]);
    assert_eq!(state.job_control(0, 0), []);
    let spec = delete(vec![in_a("x")], DeleteMethod::Trash);
    state.track(Started {
        id: JobId(4),
        spec: Some(Arc::new(spec)),
        work: JobWork::Trash,
        origin: None,
        via_history: false,
    });
    state.job_event(JobEvent::State {
        id: JobId(4),
        state: rf_jobs::JobState::Running,
    });
    assert_eq!(state.job_rows().len(), 1);
    assert_eq!(
        state.job_control(0, 0),
        [Effect::Job(JobEffect::Pause(JobId(4)))]
    );
    assert_eq!(
        state.job_control(0, 1),
        [Effect::Job(JobEffect::Resume(JobId(4)))]
    );
    assert_eq!(
        state.job_control(0, 2),
        [Effect::Job(JobEffect::Cancel(JobId(4)))]
    );
}

fn trashed(paths: &[VPath]) -> UndoRecord {
    UndoRecord::Trashed {
        items: paths
            .iter()
            .map(|path| rf_jobs::TrashedItem {
                original: path.clone(),
                location: None,
            })
            .collect(),
    }
}

#[test]
fn an_undo_never_offers_a_permanent_delete_by_itself() {
    // An undo of a copy trashes the copies; where the trash refuses one, the undo has failed
    // for it. An undo never deletes anything for good: only a confirmation the user answers,
    // offered once the copies' originals are known and found as they were (see `copies`),
    // which this run cannot tell for a copy it did not make.
    let mut state = ready(vec![file("a.txt")], vec![]);
    let spec = JobSpec::Undo(UndoRecord::Copied {
        created: Vec::new(),
        overwritten: 0,
        at: None,
    });
    let effects = finish(&mut state, 1, &spec, JobWork::Undo, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.failures.push(failure(
            in_a("a.txt"),
            ErrorKind::Unsupported,
            Some(FailureReason::TrashRefused),
        ));
    });
    assert_eq!(state.modal(), None);
    let (token, text, action) = the_toast(&effects);
    assert_eq!(
        (text.as_str(), action.as_str()),
        ("항목 1개를 처리하지 못했습니다", "자세히")
    );
    state.toast_clicked(token);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the failed item and why");
    };
    assert!(
        failures.lines[0].ends_with(
            "휴지통으로 보낼 수 없습니다. 원본을 확인할 수 없어 복사본을 지우지 않았습니다"
        ),
        "{:?}",
        failures.lines
    );
}

#[test]
fn a_partly_refused_trash_tells_what_went_and_asks_about_the_rest() {
    let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
    let spec = delete(vec![in_a("a.txt"), in_a("b.txt")], DeleteMethod::Trash);
    let record = trashed(&[in_a("a.txt")]);
    state.set_history(History {
        top: Some(record.clone()),
        can_redo: false,
    });
    let effects = finish(&mut state, 1, &spec, JobWork::Trash, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.undo = Some(record.clone());
        report.failures.push(failure(
            in_a("b.txt"),
            ErrorKind::Unsupported,
            Some(FailureReason::TrashRefused),
        ));
    });
    // What went can come back...
    let (token, text, action) = the_toast(&effects);
    assert_eq!(
        (text.as_str(), action.as_str()),
        ("항목 1개를 휴지통으로 보냈습니다", "되돌리기")
    );
    // ...and what stayed is asked about.
    let Some(Modal::Confirm(confirm)) = state.modal() else {
        panic!("the refused item is asked about");
    };
    assert_eq!(
        confirm.purpose,
        ConfirmPurpose::DeletePermanently(vec![in_a("b.txt")])
    );
    // Once the question is answered, the toast's button still undoes the trash.
    state.dialog_cancelled();
    assert!(state.toast_button_live());
    assert_eq!(state.toast_clicked(token), [Effect::Job(JobEffect::Undo)]);
}

/// The warning of an item at `path` the location deleted for good instead of trashing it.
fn gone(path: VPath) -> ItemFailure {
    ItemFailure {
        source: Some(path),
        target: None,
        stage: rf_jobs::Stage::Delete,
        kind: ErrorKind::Unsupported,
        message: String::new(),
        reason: Some(FailureReason::DeletedNotTrashed),
    }
}

#[test]
fn what_was_deleted_instead_of_trashed_has_no_undo() {
    let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
    let spec = delete(vec![in_a("a.txt"), in_a("b.txt")], DeleteMethod::Trash);
    let effects = finish(&mut state, 1, &spec, JobWork::Trash, |report| {
        report.warnings.push(gone(in_a("a.txt")));
        report.warnings.push(gone(in_a("b.txt")));
    });
    let (_, text, action) = the_toast(&effects);
    assert_eq!(
        (text.as_str(), action.as_str()),
        ("항목 2개가 휴지통에 들어가지 않고 영구 삭제되었습니다", "")
    );
    assert!(!state.toast_button_live());

    // One trashed, one deleted for good: the button brings back the one that can come back.
    let record = trashed(&[in_a("a.txt")]);
    let effects = finish(&mut state, 2, &spec, JobWork::Trash, |report| {
        report.undo = Some(record.clone());
        report.warnings.push(gone(in_a("b.txt")));
    });
    let (_, text, action) = the_toast(&effects);
    assert_eq!(
        (text.as_str(), action.as_str()),
        (
            "항목 1개를 휴지통으로 보냈고, 1개는 휴지통 대신 영구 삭제되었습니다",
            "되돌리기"
        )
    );
}

#[test]
fn a_trash_with_failures_also_lists_what_it_deleted_for_good() {
    let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
    let spec = delete(vec![in_a("a.txt"), in_a("b.txt")], DeleteMethod::Trash);
    let effects = finish(&mut state, 1, &spec, JobWork::Trash, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report
            .failures
            .push(failure(in_a("a.txt"), ErrorKind::PermissionDenied, None));
        report.warnings.push(gone(in_a("b.txt")));
    });
    let (token, text, action) = the_toast(&effects);
    assert_eq!(
        (text.as_str(), action.as_str()),
        (
            "항목 1개를 처리하지 못했습니다 · 1개는 휴지통 대신 영구 삭제되었습니다",
            "자세히"
        )
    );
    state.toast_clicked(token);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the failed items");
    };
    assert_eq!(
        failures.lines,
        [
            format!("{}: 권한이 없습니다", in_a("a.txt")),
            format!(
                "{}: 휴지통 대신 영구 삭제되어 되살릴 수 없습니다",
                in_a("b.txt")
            ),
        ]
    );
}

#[test]
fn an_undo_tells_what_it_deleted_for_good() {
    // An undo of a copy trashes the copies; a location that deletes them for good instead
    // leaves warnings, which the toast tells of.
    let mut state = ready(vec![file("a.txt")], vec![]);
    let spec = JobSpec::Undo(UndoRecord::Copied {
        created: Vec::new(),
        overwritten: 0,
        at: None,
    });
    let effects = finish(&mut state, 1, &spec, JobWork::Undo, |report| {
        report
            .warnings
            .push(gone(folder("b").join("a.txt").expect("a name")));
    });
    let (_, text, action) = the_toast(&effects);
    assert_eq!(
        (text.as_str(), action.as_str()),
        (
            "복사를 되돌렸습니다 · 1개는 휴지통 대신 영구 삭제되었습니다",
            ""
        )
    );
    // Without such warnings, as before.
    let effects = finish(&mut state, 2, &spec, JobWork::Undo, |_| {});
    assert_eq!(the_toast(&effects).1, "복사를 되돌렸습니다");
    // A redo of a trash, which trashes again, the same.
    let spec = JobSpec::Undo(trashed(&[in_a("a.txt")]));
    let effects = finish(&mut state, 3, &spec, JobWork::Redo, |report| {
        report.warnings.push(gone(in_a("a.txt")));
    });
    assert_eq!(
        the_toast(&effects).1,
        "다시 실행했습니다 · 1개는 휴지통 대신 영구 삭제되었습니다"
    );
}

#[test]
fn a_copy_into_a_folder_of_the_same_name_lists_that_folder_again() {
    let mut state = ready(vec![dir("docs")], vec![dir("docs")]);
    let temp = VPath::local(std::env::temp_dir()).expect("a local path");
    state.set_places(&[Place {
        label: "임시".to_owned(),
        path: temp.clone(),
        kind: PlaceKind::Drive,
        space: None,
    }]);
    // The tree lists the temporary folder, `b` and `b/docs`.
    let list = |state: &mut AppState, row: usize, entries: Vec<Entry>| {
        let effects = state.toggle_tree_row(row);
        let [Effect::LoadChildren(request)] = effects.as_slice() else {
            panic!("one tree listing in {effects:?}");
        };
        state.loaded(LoadEvent::Listed {
            generation: request.generation,
            entries,
        });
    };
    list(&mut state, 0, vec![dir("a"), dir("b")]);
    list(&mut state, 2, vec![dir("docs")]);
    list(&mut state, 3, vec![dir("old")]);

    // `a/docs` is copied into `b`, which has a `docs`: the copy merges into it. The panes show
    // `a` and `b` and follow them; `b/docs`, which no pane shows, is listed again.
    let spec = JobSpec::Transfer(
        TransferSpec::into_dir(
            TransferMode::Copy,
            &[in_a("docs")],
            &folder("b"),
            TransferOptions::default(),
        )
        .expect("a transfer"),
    );
    let effects = finish(&mut state, 1, &spec, JobWork::Copy, |_| {});
    let relisted: Vec<&VPath> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::LoadChildren(request) => Some(&request.path),
            _ => None,
        })
        .collect();
    assert_eq!(relisted, [&folder("b").join("docs").expect("a name")]);
}

#[test]
fn skipped_items_are_not_counted_as_done() {
    let mut state = ready(vec![file("a"), file("b"), file("c")], vec![]);
    let spec = JobSpec::Transfer(
        TransferSpec::into_dir(
            TransferMode::Copy,
            &[in_a("a"), in_a("b"), in_a("c")],
            &folder("b"),
            TransferOptions::default(),
        )
        .expect("a transfer"),
    );
    let effects = finish(&mut state, 1, &spec, JobWork::Copy, |report| {
        report.done.files = 1;
        report.skipped = 2;
    });
    assert_eq!(
        the_toast(&effects).1,
        "항목 3개 중 1개를 복사했습니다 · 2개 건너뜀"
    );
    // All of them skipped: nothing was copied, and there is nothing to undo.
    let effects = finish(&mut state, 2, &spec, JobWork::Copy, |report| {
        report.skipped = 3;
    });
    let (_, text, action) = the_toast(&effects);
    assert_eq!(
        (text.as_str(), action.as_str()),
        ("항목 3개를 건너뛰었습니다", "")
    );
}

#[test]
fn the_toast_button_hides_once_it_would_not_undo() {
    let mut state = ready(vec![file("a.txt")], vec![]);
    let spec = delete(vec![in_a("a.txt")], DeleteMethod::Trash);
    let record = moved("a.txt");
    state.set_history(History {
        top: Some(record.clone()),
        can_redo: false,
    });
    let effects = finish(&mut state, 1, &spec, JobWork::Move, |report| {
        report.undo = Some(record.clone());
    });
    let (token, _, _) = the_toast(&effects);
    assert!(state.toast_button_live());

    // An undo under way (a recovered one, say): the button hides; a click that came first is
    // told why.
    state.track(Started {
        id: JobId(2),
        spec: None,
        work: JobWork::Undo,
        origin: None,
        via_history: true,
    });
    assert!(!state.toast_button_live());
    assert_eq!(
        state.toast_clicked(token),
        [Effect::Notice(
            "되돌리기나 다시 실행이 진행 중입니다. 끝난 뒤에 다시 해 보세요".to_owned()
        )]
    );

    // A later job is next to undo: hidden too, and told so.
    let effects = finish(&mut state, 3, &spec, JobWork::Move, |report| {
        report.undo = Some(record.clone());
    });
    state.job_event(JobEvent::Finished {
        id: JobId(2),
        report: Arc::new(JobReport::new(JobId(2), &spec)),
    });
    let (token, _, _) = the_toast(&effects);
    state.set_history(History {
        top: Some(moved("other.txt")),
        can_redo: false,
    });
    assert!(!state.toast_button_live());
    assert_eq!(
        state.toast_clicked(token),
        [Effect::Notice(
            "나중에 한 작업이 있어 먼저 그 작업을 되돌려야 합니다".to_owned()
        )]
    );
}

#[test]
fn the_same_confirmation_asks_once() {
    let mut state = ready(vec![file("a.txt")], vec![]);
    let paths = vec![in_a("a.txt")];
    // Delete pressed twice before the first answer came from the worker.
    state.trash_checked(paths.clone(), false);
    state.trash_checked(paths.clone(), false);
    assert!(matches!(state.modal(), Some(Modal::Confirm(_))));
    assert_eq!(state.dialog_cancelled(), []);
    assert_eq!(state.modal(), None);
    // Another question about other items still comes.
    state.trash_checked(paths, false);
    state.trash_checked(vec![in_a("b.txt")], false);
    state.dialog_cancelled();
    let Some(Modal::Confirm(confirm)) = state.modal() else {
        panic!("the second question");
    };
    assert_eq!(
        confirm.purpose,
        ConfirmPurpose::DeletePermanently(vec![in_a("b.txt")])
    );
}

#[test]
fn a_conflict_question_knows_what_its_job_does() {
    let mut state = ready(vec![file("a.txt")], vec![]);
    let spec = JobSpec::Transfer(
        TransferSpec::into_dir(
            TransferMode::Copy,
            &[in_a("a.txt")],
            &folder("b"),
            TransferOptions::default(),
        )
        .expect("a transfer"),
    );
    state.track(Started {
        id: JobId(1),
        spec: Some(Arc::new(spec)),
        work: JobWork::Copy,
        origin: Some(LEFT),
        via_history: false,
    });
    state.question(conflict_question(5));
    let Some(Modal::Conflict(question)) = state.modal() else {
        panic!("the question shows");
    };
    // The window words its cards and choices for a copy ("복사해 올 파일").
    assert_eq!(question.work, JobWork::Copy);
    // A job the window does not follow is named in neutral words.
    state.dialog_cancelled();
    let mut other = conflict_question(6);
    other.job = JobId(9);
    state.question(other);
    let Some(Modal::Conflict(question)) = state.modal() else {
        panic!("the question shows");
    };
    assert_eq!(question.work, JobWork::Other);
}

#[test]
fn an_undo_or_redo_selects_what_it_brought_back() {
    let mut state = ready(vec![file("x"), file("작업폴더")], vec![]);
    // The undo of a rename gives the item its old name back; the window's pane shows it.
    let record = UndoRecord::Renamed {
        from: in_a("작업폴더"),
        to: in_a("보관함"),
    };
    let spec = JobSpec::Undo(record);
    let effects = finish(&mut state, 1, &spec, JobWork::Undo, |report| {
        report.outputs.push(in_a("작업폴더"));
    });
    assert!(
        effects.contains(&Effect::ScrollTo { side: LEFT, row: 1 }),
        "{effects:?}"
    );
    assert_eq!(selected(&state, LEFT), [1]);
    // A redo the same, also of an item not listed yet.
    let spec = JobSpec::Undo(UndoRecord::Removed {
        path: in_a("새 폴더"),
        kind: rf_jobs::CreateKind::Dir,
    });
    finish(&mut state, 2, &spec, JobWork::Redo, |report| {
        report.outputs.push(in_a("새 폴더"));
    });
    let generation = state.panes()[LEFT].active_tab().generation();
    state.loaded(LoadEvent::Changes {
        generation,
        changes: vec![rf_vfs::Change::Upsert(dir("새 폴더"))],
    });
    assert_eq!(rows(&state, LEFT)[selected(&state, LEFT)[0]], "새 폴더");
}

#[test]
fn a_job_that_ends_reads_the_free_space_of_the_drives_it_touched() {
    let mut state = ready(vec![file("a.txt")], vec![]);
    let temp = VPath::local(std::env::temp_dir()).expect("a local path");
    let elsewhere = VPath::local(if cfg!(windows) {
        r"Q:\"
    } else {
        "/rf-ui-other-drive"
    })
    .expect("a local path");
    let drive = |label: &str, path: &VPath| Place {
        label: label.to_owned(),
        path: path.clone(),
        kind: PlaceKind::Drive,
        space: None,
    };
    state.set_places(&[drive("임시", &temp), drive("다른 드라이브", &elsewhere)]);
    // A third tab, on the other drive, is left out.
    state.add_tab(RIGHT);
    let request = the_load(&state.go_to(elsewhere.join("x").expect("a name")));
    state.loaded(LoadEvent::Listed {
        generation: request.generation,
        entries: Vec::new(),
    });
    let other_tab = state.panes()[RIGHT].tabs()[1].generation();
    state.select_tab(RIGHT, 0);

    let spec = JobSpec::Transfer(
        TransferSpec::into_dir(
            TransferMode::Copy,
            &[in_a("a.txt")],
            &folder("b"),
            TransferOptions::default(),
        )
        .expect("a transfer"),
    );
    let effects = finish_all(&mut state, 1, &spec, JobWork::Copy, |_| {});
    let reads: Vec<&Effect> = effects
        .iter()
        .filter(|effect| matches!(effect, Effect::ReadSpace { .. }))
        .collect();
    let [Effect::ReadSpace { tabs, drives }] = reads.as_slice() else {
        panic!("one read of the free space: {effects:?}");
    };
    let left = state.panes()[LEFT].active_tab().generation();
    let right = state.panes()[RIGHT].active_tab().generation();
    assert_eq!(tabs, &[(left, folder("a")), (right, folder("b"))]);
    assert_eq!(drives, &[temp]);
    assert!(!tabs.iter().any(|(generation, _)| *generation == other_tab));
    // What the worker finds is taken in by the tab that asked, while it still shows that folder.
    state.loaded(LoadEvent::Space {
        generation: right,
        available: 5 * 1024 * 1024 * 1024,
    });
    assert!(
        state.panes()[RIGHT]
            .active_tab()
            .status_line()
            .ends_with("남은 공간 5 GB")
    );
}
