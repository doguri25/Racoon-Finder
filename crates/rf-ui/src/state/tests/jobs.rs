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

#[test]
fn copy_and_move_keys_wait_for_the_picker() {
    let mut state = ready(vec![file("x")], vec![]);
    state.click_row(LEFT, 0, false, false);
    for text in [special(Key::F5), special(Key::F6)] {
        assert_eq!(press(&mut state, &text, NONE), None, "{text:?}");
    }
    assert_eq!(state.modal(), None);
}

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
    let mut state = ready(vec![dir("docs"), file("설계안.pdf")], vec![]);
    // Nothing under the cursor: F2 does nothing.
    assert_eq!(press(&mut state, &special(Key::F2), NONE), Some(vec![]));
    assert_eq!(state.modal(), None);
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
        "휴지통을 쓸 수 없는 위치입니다. 항목 2개를 영구 삭제할까요? 되돌릴 수 없습니다."
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
        "항목 2개를 영구 삭제할까요? 휴지통을 거치지 않으며 되돌릴 수 없습니다."
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
    let mut state = ready(vec![file("a.txt")], vec![]);
    // Nothing to move without a selection or a cursor.
    let options = TransferOptions::default();
    assert_eq!(
        state.transfer(LEFT, TransferMode::Move, &folder("b"), options),
        []
    );
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
        "진행 중인 작업 1개가 있습니다. 끝내면 다음 실행 때 이어서 할 수 있습니다."
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
