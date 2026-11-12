//! The file commands, the jobs, the toasts and the dialogs.

use rf_jobs::{
    ConflictPolicy, DeleteMethod, DeleteSpec, ErrorAction, ErrorAnswer, ErrorPolicy, FailureReason,
    ItemFailure, JobEvent, JobSpec, Outcome, Stage, SymlinkMode, TransferItem, TransferMode,
    TransferOptions, TransferSpec, UndoRecord,
};

use super::*;
use crate::dialogs::{ConfirmPurpose, ConflictChoice, ErrorChoice, Modal};
use crate::jobs::{History, JobEffect, Started};
use crate::resolver::{Asked, Question, SKIP_ERROR};
use crate::settings::OnError;
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
    assert_eq!(press(&mut state, "N", CTRL_SHIFT), Some(vec![]));
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
    // The toolbar's 새 폴더 opens it too, and Esc closes it without a job.
    state.key(Command::NewFolder, NONE, Paging::rows(1), Instant::now());
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
    // Once the user has moved the cursor, F2 renames its row.
    let mut state = ready(vec![dir("docs"), file("설계안.pdf"), file("x")], vec![]);
    press(&mut state, &special(Key::DownArrow), NONE);
    press(&mut state, &special(Key::UpArrow), NONE);
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
fn a_folder_just_opened_changes_no_file_until_the_user_chooses_a_row() {
    // The rows come with the cursor on the first one for the keys to start from, but nothing
    // is chosen: no command that changes files acts on that row.
    let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![dir("백업")]);
    assert_eq!(cursor(&state, LEFT), Some(0));
    for (key, mods) in [
        (Key::Delete, NONE),
        (Key::Delete, SHIFT),
        (Key::F2, NONE),
        (Key::F7, NONE),
        (Key::F8, NONE),
    ] {
        assert_eq!(
            press(&mut state, &special(key), mods),
            Some(vec![]),
            "{key:?} {mods:?}"
        );
        assert_eq!(state.modal(), None, "{key:?} {mods:?}");
    }
    let options = TransferOptions::default();
    assert_eq!(
        state.transfer(LEFT, TransferMode::Copy, &folder("b"), options),
        []
    );

    // An arrow key chooses the row it moves to: Delete and F2 act on it.
    press(&mut state, &special(Key::DownArrow), NONE);
    assert_eq!(
        press(&mut state, &special(Key::Delete), NONE),
        Some(vec![Effect::CheckTrash {
            folder: folder("a"),
            paths: vec![in_a("b.txt")],
        }])
    );
    // So does Up from the first row, which stays there.
    let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
    press(&mut state, &special(Key::UpArrow), NONE);
    press(&mut state, &special(Key::F2), NONE);
    assert!(matches!(state.modal(), Some(Modal::Name(dialog)) if dialog.initial == "a.txt"));

    // A row the screen shows unselected is not acted on, though the user chose it before: once
    // Ctrl+Space took it away again, or Esc did (as in the system's file managers).
    let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
    press(&mut state, &special(Key::Space), CTRL);
    press(&mut state, &special(Key::Space), CTRL);
    assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
    let no_file_command = |state: &mut AppState| {
        for (key, mods) in [
            (Key::Delete, NONE),
            (Key::Delete, SHIFT),
            (Key::F2, NONE),
            (Key::F7, NONE),
            (Key::F8, NONE),
        ] {
            assert_eq!(
                press(state, &special(key), mods),
                Some(vec![]),
                "{key:?} {mods:?}"
            );
            assert_eq!(state.modal(), None, "{key:?} {mods:?}");
        }
    };
    no_file_command(&mut state);
    press(&mut state, &special(Key::DownArrow), NONE);
    assert_eq!(selected(&state, LEFT), [1]);
    press(&mut state, &special(Key::Escape), NONE);
    assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
    no_file_command(&mut state);
    // Ctrl+Space that selects the row again makes it the one acted on.
    press(&mut state, &special(Key::Space), CTRL);
    assert_eq!(
        press(&mut state, &special(Key::Delete), NONE),
        Some(vec![Effect::CheckTrash {
            folder: folder("a"),
            paths: vec![in_a("b.txt")],
        }])
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
pub(super) fn finish(
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
        stage: Stage::Delete,
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
                ..
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
        ..History::default()
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
        ..History::default()
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
        ..History::default()
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
        ..History::default()
    });
    assert!(!state.can_undo() && !state.can_redo());
    assert_eq!(press(&mut state, "z", CTRL), Some(vec![]));
    assert_eq!(press(&mut state, "y", CTRL), Some(vec![]));
    // It ends: the toast names what it undid, and the redo can start.
    state.set_history(History {
        top: None,
        can_redo: true,
        ..History::default()
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
    press(&mut state, "N", CTRL_SHIFT);
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
        submitted: SystemTime::now(),
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
fn a_selection_the_user_makes_before_the_new_item_shows_stays() {
    // The user clicks a row, or types a name, while the new folder is not listed yet: when it
    // shows, the selection the user chose stays, and nothing scrolls to the folder.
    for by_typing in [false, true] {
        let mut state = ready(vec![file("x"), file("y")], vec![]);
        let spec = JobSpec::Create {
            parent: folder("a"),
            name: "새 폴더".into(),
            kind: rf_jobs::CreateKind::Dir,
        };
        finish(&mut state, 1, &spec, JobWork::Create, |report| {
            report.outputs.push(in_a("새 폴더"));
        });
        if by_typing {
            let typed = Command::TypeToSelect("y".into());
            state.key(typed, NONE, Paging::rows(10), Instant::now());
        } else {
            state.click_row(LEFT, 1, false, false);
        }
        assert_eq!(selected(&state, LEFT), [1]);
        let generation = state.panes()[LEFT].active_tab().generation();
        let effects = state.loaded(LoadEvent::Changes {
            generation,
            changes: vec![rf_vfs::Change::Upsert(dir("새 폴더"))],
        });
        assert_eq!(rows(&state, LEFT), ["새 폴더", "x", "y"]);
        assert_eq!(selected(&state, LEFT), [2], "typing: {by_typing}");
        assert_eq!(cursor(&state, LEFT), Some(2), "typing: {by_typing}");
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::ScrollTo { .. })),
            "{effects:?}"
        );
    }
}

#[test]
fn a_job_that_ends_after_the_user_chose_rows_leaves_them_chosen() {
    // The new folder is listed (the watch told of it) before its job ends, and the user picks
    // another row meanwhile: the job's end selects nothing, so a Delete acts on that row.
    let spec = JobSpec::Create {
        parent: folder("a"),
        name: "새 폴더".into(),
        kind: rf_jobs::CreateKind::Dir,
    };
    for user_picks in [false, true] {
        let mut state = ready(vec![file("x"), file("y")], vec![]);
        state.track(Started {
            id: JobId(1),
            spec: Some(Arc::new(spec.clone())),
            work: JobWork::Create,
            origin: Some(LEFT),
            via_history: false,
        });
        let generation = state.panes()[LEFT].active_tab().generation();
        state.loaded(LoadEvent::Changes {
            generation,
            changes: vec![rf_vfs::Change::Upsert(dir("새 폴더"))],
        });
        if user_picks {
            state.click_row(LEFT, 2, false, false);
        }
        let mut report = JobReport::new(JobId(1), &spec);
        report.outputs.push(in_a("새 폴더"));
        let effects = state.job_event(JobEvent::Finished {
            id: JobId(1),
            report: Arc::new(report),
        });
        if user_picks {
            assert_eq!(selected(&state, LEFT), [2], "{effects:?}");
            assert!(
                !effects
                    .iter()
                    .any(|effect| matches!(effect, Effect::ScrollTo { .. })),
                "{effects:?}"
            );
        } else {
            assert_eq!(selected(&state, LEFT), [0]);
        }
    }
}

#[test]
fn the_pattern_window_keeps_the_name_waiting_to_be_selected() {
    let mut state = ready(vec![file("x")], vec![]);
    let spec = JobSpec::Create {
        parent: folder("a"),
        name: "새 폴더".into(),
        kind: rf_jobs::CreateKind::Dir,
    };
    finish(&mut state, 1, &spec, JobWork::Create, |report| {
        report.outputs.push(in_a("새 폴더"));
    });
    // The pattern window opens and closes, and a cursor without an extension selects nothing:
    // no row was chosen, so the new folder is still selected when it shows.
    state.key(
        Command::SelectPattern,
        NONE,
        Paging::rows(10),
        Instant::now(),
    );
    assert!(matches!(state.modal(), Some(Modal::Pattern(_))));
    state.dialog_cancelled();
    let generation = state.panes()[LEFT].active_tab().generation();
    state.loaded(LoadEvent::Changes {
        generation,
        changes: vec![rf_vfs::Change::Upsert(dir("새 폴더"))],
    });
    assert_eq!(rows(&state, LEFT), ["새 폴더", "x"]);
    assert_eq!(selected(&state, LEFT), [0]);
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
        ..History::default()
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
        stage: Stage::Delete,
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
        ..History::default()
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
        ..History::default()
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
    assert_eq!(
        state.panes()[RIGHT]
            .active_tab()
            .status_parts(None)
            .last()
            .map(String::as_str),
        Some("남은 공간 5 GB")
    );
}

/// The failure of the copy of `name` from `a` to `b` at `stage`, as a transfer reports it: with
/// both of its paths.
fn copy_failure(name: &str, stage: Stage) -> ItemFailure {
    ItemFailure {
        source: Some(in_a(name)),
        target: Some(folder("b").join(name).expect("a plain name")),
        stage,
        kind: ErrorKind::PermissionDenied,
        message: String::new(),
        reason: None,
    }
}

/// The copy of `names` from `a` into `b` under `options`.
fn copy_of(names: &[&str], options: TransferOptions) -> JobSpec {
    let sources: Vec<VPath> = names.iter().map(|name| in_a(name)).collect();
    JobSpec::Transfer(
        TransferSpec::into_dir(TransferMode::Copy, &sources, &folder("b"), options)
            .expect("a transfer"),
    )
}

#[test]
fn retrying_the_failures_redoes_only_what_failed() {
    let mut state = ready(vec![file("a.txt"), file("b.txt"), file("c.txt")], vec![]);
    // A copy of three files under policies of its own: `a.txt` was copied, the two others
    // failed.
    let options = TransferOptions {
        conflict: ConflictPolicy::Overwrite,
        errors: ErrorPolicy::Continue,
        symlinks: SymlinkMode::Follow,
    };
    let spec = copy_of(&["a.txt", "b.txt", "c.txt"], options);
    let effects = finish(&mut state, 1, &spec, JobWork::Copy, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.done.files = 1;
        report.failures.push(copy_failure("b.txt", Stage::Write));
        report.failures.push(copy_failure("c.txt", Stage::Read));
    });
    let (token, text, _) = the_toast(&effects);
    assert_eq!(text, "항목 2개를 처리하지 못했습니다");
    assert_eq!(state.toast_clicked(token), []);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the failed items");
    };
    assert_eq!(failures.lines.len(), 2);
    assert!(failures.retry.is_some(), "다시 시도 is offered");

    // 다시 시도 submits one new job: the same kind, to the same targets, under the same
    // policies, of the two failed items alone. The file that was copied is not in it.
    let (retry, origin) = the_submit(&state.failures_retried());
    assert_eq!(origin, None);
    let JobSpec::Transfer(transfer) = &retry else {
        panic!("a transfer: {retry:?}");
    };
    assert_eq!(
        (transfer.mode, transfer.options),
        (TransferMode::Copy, options)
    );
    let item = |name: &str| TransferItem {
        source: in_a(name),
        target: folder("b").join(name).expect("a plain name"),
    };
    assert_eq!(transfer.items, [item("b.txt"), item("c.txt")]);
    assert_eq!(state.modal(), None);
    // The window is gone: a second press finds nothing to submit.
    assert_eq!(state.failures_retried(), []);

    // The same job's 자세히 in 최근 완료 lists its failures still, and offers no second retry.
    assert_eq!(state.recent_details(0), []);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the failed items again");
    };
    assert_eq!((failures.lines.len(), &failures.retry), (2, &None));
    assert_eq!(state.failures_retried(), []);
    state.dialog_cancelled();

    // The retry is a job of its own: `b.txt` goes this time, `c.txt` (its source is gone)
    // fails again and stays listed, with a retry of its own that holds it alone.
    let effects = finish(&mut state, 2, &retry, JobWork::Copy, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.done.files = 1;
        let mut gone = copy_failure("c.txt", Stage::Plan);
        gone.kind = ErrorKind::NotFound;
        report.failures.push(gone);
    });
    let (token, text, _) = the_toast(&effects);
    assert_eq!(text, "항목 1개를 처리하지 못했습니다");
    state.toast_clicked(token);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the item that failed again");
    };
    assert_eq!(
        failures.lines,
        [format!("{}: 찾을 수 없습니다", in_a("c.txt"))]
    );
    let (again, _) = the_submit(&state.failures_retried());
    let JobSpec::Transfer(transfer) = again else {
        panic!("a transfer");
    };
    assert_eq!(transfer.items, [item("c.txt")]);
    assert_eq!(transfer.options, options);
}

#[test]
fn only_the_failures_a_transfer_can_take_up_again_offer_a_retry() {
    let mut state = ready(vec![file("a.txt")], vec![]);
    let details = |state: &mut AppState| {
        assert_eq!(state.recent_details(0), []);
        let Some(Modal::Failures(failures)) = state.modal().cloned() else {
            panic!("the failed items");
        };
        state.dialog_cancelled();
        failures
    };

    // A move whose copy is in place but whose source stayed: asking for it again would meet
    // the copy as a conflict, so there is nothing to try again.
    let moving = JobSpec::Transfer(
        TransferSpec::into_dir(
            TransferMode::Move,
            &[in_a("a.txt")],
            &folder("b"),
            TransferOptions::default(),
        )
        .expect("a transfer"),
    );
    finish(&mut state, 1, &moving, JobWork::Move, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report
            .failures
            .push(copy_failure("a.txt", Stage::DeleteSource));
    });
    assert_eq!(details(&mut state).retry, None);

    // A warning is no failure: a copy that only warned has no retry.
    let copy = copy_of(&["a.txt"], TransferOptions::default());
    finish(&mut state, 2, &copy, JobWork::Copy, |report| {
        report.warnings.push(copy_failure("a.txt", Stage::Metadata));
    });
    assert_eq!(details(&mut state).retry, None);

    // A trash that failed is not tried again from the window: only copies and moves are.
    let trash = delete(vec![in_a("a.txt")], DeleteMethod::Trash);
    finish(&mut state, 3, &trash, JobWork::Trash, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report
            .failures
            .push(failure(in_a("a.txt"), ErrorKind::PermissionDenied, None));
    });
    assert_eq!(details(&mut state).retry, None);

    // A copy that failed before its content was in place has one, from 최근 완료 too; once it
    // is taken there, neither that line nor the job's toast offers it again.
    let effects = finish(&mut state, 4, &copy, JobWork::Copy, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.failures.push(copy_failure("a.txt", Stage::Commit));
    });
    let (token, _, _) = the_toast(&effects);
    assert_eq!(state.recent_details(0), []);
    assert!(matches!(
        the_submit(&state.failures_retried()).0,
        JobSpec::Transfer(_)
    ));
    assert_eq!(details(&mut state).retry, None);
    // Nor does the window of the job's toast, which was made before the retry.
    assert_eq!(state.toast_clicked(token), []);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the failed items");
    };
    assert_eq!((failures.lines.len(), &failures.retry), (1, &None));
    assert_eq!(state.failures_retried(), []);
}

fn error_question(id: u64) -> Question {
    Question {
        id,
        job: JobId(1),
        asked: Asked::Error(Box::new(copy_failure("a.txt", Stage::Write))),
    }
}

#[test]
fn an_error_question_is_answered_for_its_item_or_for_the_rest() {
    let mut state = ready(vec![file("a.txt")], vec![]);
    state.track(Started {
        id: JobId(1),
        spec: Some(Arc::new(copy_of(&["a.txt"], TransferOptions::default()))),
        work: JobWork::Copy,
        origin: Some(LEFT),
        via_history: false,
    });
    let answered = |action, apply_to_all| ErrorAnswer {
        action,
        apply_to_all,
    };
    // The question shows with the item that failed and what its job does.
    assert_eq!(state.question(error_question(4)), []);
    let Some(Modal::Error(question)) = state.modal() else {
        panic!("the error shows");
    };
    assert_eq!((question.id, question.work), (4, JobWork::Copy));
    assert_eq!(question.failure.source, Some(in_a("a.txt")));
    // 다시 시도 answers for this item, and the window closes.
    assert_eq!(
        state.error_answered(ErrorChoice::Retry, false),
        [Effect::Job(JobEffect::AnswerError(
            4,
            answered(ErrorAction::Retry, false)
        ))]
    );
    assert_eq!(state.modal(), None);
    // An answer with no question open answers nothing.
    assert_eq!(state.error_answered(ErrorChoice::Skip, false), []);

    // 남은 오류에도 같은 선택 with 건너뛰기: the answer the engine remembers, so that it asks no
    // more.
    state.question(error_question(5));
    assert_eq!(
        state.error_answered(ErrorChoice::Skip, true),
        [Effect::Job(JobEffect::AnswerError(
            5,
            answered(ErrorAction::Skip, true)
        ))]
    );
    // With 다시 시도 it is not offered: nothing is answered, and the question stays.
    state.question(error_question(6));
    assert_eq!(state.error_answered(ErrorChoice::Retry, true), []);
    assert!(matches!(state.modal(), Some(Modal::Error(question)) if question.id == 6));
    assert_eq!(
        state.error_answered(ErrorChoice::Abort, false),
        [Effect::Job(JobEffect::AnswerError(
            6,
            answered(ErrorAction::Abort, false)
        ))]
    );

    // Closing the window (Esc) leaves the item out, this once: the job goes on.
    state.question(error_question(7));
    assert_eq!(
        state.dialog_cancelled(),
        [Effect::Job(JobEffect::AnswerError(7, SKIP_ERROR))]
    );
    assert_eq!(SKIP_ERROR, answered(ErrorAction::Skip, false));
    assert_eq!(state.modal(), None);

    // It waits behind a window that is open, as a conflict does, and is left unanswered once
    // the app ends (the release after the engine's shutdown answers it).
    press(&mut state, "N", CTRL_SHIFT);
    state.question(error_question(8));
    assert!(matches!(state.modal(), Some(Modal::Name(_))));
    state.dialog_cancelled();
    assert!(matches!(state.modal(), Some(Modal::Error(question)) if question.id == 8));
    state.request_close();
    assert_eq!(state.confirm_accepted(), [Effect::Quit]);
    assert_eq!(state.question(error_question(9)), []);
    assert_eq!(state.modal(), None);
}

#[test]
fn the_error_setting_chooses_how_a_transfer_takes_a_failure() {
    let mut state = ready(vec![file("a.txt")], vec![dir("백업")]);
    // By default a transfer carries on, as the demos before did.
    assert_eq!(state.on_error(), OnError::Continue);
    let errors_of = |effects: &[Effect]| match the_submit(effects).0 {
        JobSpec::Transfer(transfer) => transfer.options.errors,
        other => panic!("a transfer: {other:?}"),
    };
    let copy_by_picker = |state: &mut AppState| {
        state.click_row(LEFT, 0, false, false);
        press(state, &special(Key::F7), NONE);
        state.pick_confirmed()
    };
    assert_eq!(
        errors_of(&copy_by_picker(&mut state)),
        ErrorPolicy::Continue
    );

    // 파일마다 묻기: the transfers submitted from then on ask.
    state.set_on_error(OnError::Ask);
    assert_eq!(state.on_error(), OnError::Ask);
    assert_eq!(errors_of(&copy_by_picker(&mut state)), ErrorPolicy::Ask);
    // A delete has no such question: it carries on whatever the setting says.
    let trashed = state.trash_checked(vec![in_a("a.txt")], true);
    assert_eq!(
        the_submit(&trashed).0,
        delete(vec![in_a("a.txt")], DeleteMethod::Trash)
    );

    // Back to the default.
    state.set_on_error(OnError::Continue);
    assert_eq!(
        errors_of(&copy_by_picker(&mut state)),
        ErrorPolicy::Continue
    );
}

#[test]
fn a_failure_no_retry_can_help_is_listed_and_not_tried_again() {
    let mut state = ready(vec![dir("dir"), file("b.txt")], vec![]);
    let following = TransferOptions {
        symlinks: SymlinkMode::Follow,
        ..TransferOptions::default()
    };
    let spec = copy_of(&["dir", "b.txt"], following);
    // A link in `dir` that leads back to `dir`, as a copy that follows links reports it.
    let looped = || ItemFailure {
        source: Some(in_a("dir").join("loop").expect("a plain name")),
        target: Some(
            folder("b")
                .join("dir")
                .and_then(|dir| dir.join("loop"))
                .expect("plain names"),
        ),
        stage: Stage::Plan,
        kind: ErrorKind::Unsupported,
        message: String::new(),
        reason: Some(FailureReason::LinkLoop),
    };
    let details = |state: &mut AppState| {
        assert_eq!(state.recent_details(0), []);
        let Some(Modal::Failures(failures)) = state.modal().cloned() else {
            panic!("the failed items");
        };
        state.dialog_cancelled();
        failures
    };

    // The loop alone: the window lists it with its reason, and has no 다시 시도 (asked for by
    // itself, the link would be followed and the folder copied again below it).
    finish(&mut state, 1, &spec, JobWork::Copy, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.failures.push(looped());
    });
    let failures = details(&mut state);
    assert_eq!(
        failures.lines,
        [format!(
            "{}: 바로 가기가 자기를 담은 폴더를 다시 가리켜 따라갈 수 없습니다",
            in_a("dir").join("loop").expect("a plain name")
        )]
    );
    assert_eq!(failures.retry, None);

    // With a failure that can be tried again beside it, both are listed and the retry holds
    // that one alone.
    finish(&mut state, 2, &spec, JobWork::Copy, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.failures.push(looped());
        report.failures.push(copy_failure("b.txt", Stage::Write));
    });
    let failures = details(&mut state);
    assert_eq!(failures.lines.len(), 2);
    let Some(JobSpec::Transfer(retry)) = failures.retry.map(|retry| retry.spec) else {
        panic!("a retry of the other item");
    };
    let sources: Vec<VPath> = retry.items.into_iter().map(|item| item.source).collect();
    assert_eq!(sources, [in_a("b.txt")]);
    assert_eq!(retry.options, following);
}

#[test]
fn a_stopped_job_says_so_and_offers_no_retry() {
    let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
    let spec = copy_of(
        &["a.txt", "b.txt", "c.txt", "d.txt"],
        TransferOptions::default(),
    );
    // Four files planned: one copied, the second failed, and the user pressed 중단 there.
    let stopped = |report: &mut JobReport| {
        report.outcome = Outcome::Aborted;
        report.planned.files = 4;
        report.done.files = 1;
        report.failures.push(copy_failure("b.txt", Stage::Write));
    };
    let effects = finish(&mut state, 1, &spec, JobWork::Copy, stopped);
    // The notice says that the job was stopped and how many items it never came to, not only
    // that one item failed.
    let (token, text, action) = the_toast(&effects);
    assert_eq!(
        (text.as_str(), action.as_str()),
        (
            "복사를 중단했습니다 · 실패 1개 · 남은 항목 2개는 처리하지 않았습니다",
            "자세히"
        )
    );
    let note = "복사를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목 2개는 처리하지 않았습니다. \
                마저 하려면 복사를 다시 시작하세요.";
    let line = format!("{}: 권한이 없습니다", in_a("b.txt"));
    // So does the first line of the window, before the item it stopped at. It has no 다시
    // 시도: that would try `b.txt` alone, and `c.txt` and `d.txt` would still be missing.
    assert_eq!(state.toast_clicked(token), []);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the failed items");
    };
    assert_eq!(failures.lines, [note.to_owned(), line.clone()]);
    assert_eq!(failures.retry, None);
    assert_eq!(state.failures_retried(), []);
    state.dialog_cancelled();
    // The same from 최근 완료.
    assert_eq!(state.recent_details(0), []);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the failed items again");
    };
    assert_eq!(failures.lines, [note.to_owned(), line]);
    assert_eq!(failures.retry, None);
    state.dialog_cancelled();

    // Stopped while it was still counting: how many are left is not known, and not made up.
    let effects = finish(&mut state, 2, &spec, JobWork::Move, |report| {
        report.outcome = Outcome::Aborted;
        report.failures.push(copy_failure("a.txt", Stage::Plan));
    });
    assert_eq!(
        the_toast(&effects).1,
        "이동을 중단했습니다 · 실패 1개 · 남은 항목은 처리하지 않았습니다"
    );
    // A job that ran to its end with the same failure is told as before, with its retry.
    let effects = finish(&mut state, 3, &spec, JobWork::Copy, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.planned.files = 4;
        report.done.files = 3;
        report.failures.push(copy_failure("b.txt", Stage::Write));
    });
    let (token, text, _) = the_toast(&effects);
    assert_eq!(text, "항목 1개를 처리하지 못했습니다");
    state.toast_clicked(token);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the failed items");
    };
    assert_eq!(failures.lines.len(), 1);
    assert!(failures.retry.is_some());
}

#[test]
fn a_stopped_job_counts_only_what_it_never_came_to() {
    let mut state = ready(vec![file("a.txt"), file("b.txt")], vec![]);
    let spec = copy_of(
        &["a.txt", "b.txt", "c.txt", "d.txt"],
        TransferOptions::default(),
    );
    // A move of three files to another drive: `a` moved, `b` was copied but its source could
    // not be removed (which counts it as done), and the user stopped there. `c` is the one
    // item it never came to.
    let moving = JobSpec::Transfer(
        TransferSpec::into_dir(
            TransferMode::Move,
            &[in_a("a.txt"), in_a("b.txt"), in_a("c.txt")],
            &folder("b"),
            TransferOptions::default(),
        )
        .expect("a transfer"),
    );
    let effects = finish(&mut state, 10, &moving, JobWork::Move, |report| {
        report.outcome = Outcome::Aborted;
        report.planned.files = 3;
        report.done.files = 2;
        report
            .failures
            .push(copy_failure("b.txt", Stage::DeleteSource));
    });
    let (token, text, _) = the_toast(&effects);
    assert_eq!(
        text,
        "이동을 중단했습니다 · 실패 1개 · 남은 항목 1개는 처리하지 않았습니다"
    );
    state.toast_clicked(token);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the failed items");
    };
    assert_eq!(
        failures.lines[0],
        "이동을 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목 1개는 처리하지 않았습니다. \
         마저 하려면 이동을 다시 시작하세요."
    );
    state.dialog_cancelled();

    // A copy of four: `a` had vanished (it was never in the plan of three) and was left out
    // in the window that asks, `b` was copied, `c` failed and the user stopped there. `d` is
    // the one item it never came to.
    let effects = finish(&mut state, 11, &spec, JobWork::Copy, |report| {
        report.outcome = Outcome::Aborted;
        report.planned.files = 3;
        report.done.files = 1;
        let mut gone = copy_failure("a.txt", Stage::Plan);
        gone.kind = ErrorKind::NotFound;
        report.failures.push(gone);
        report.failures.push(copy_failure("c.txt", Stage::Write));
    });
    assert_eq!(
        the_toast(&effects).1,
        "복사를 중단했습니다 · 실패 2개 · 남은 항목 1개는 처리하지 않았습니다"
    );
    // A failure whose place in the plan is not known (a fault of the engine itself): that
    // items were left is said, and no number is made up.
    let effects = finish(&mut state, 12, &spec, JobWork::Copy, |report| {
        report.outcome = Outcome::Aborted;
        report.planned.files = 4;
        report.done.files = 1;
        report.failures.push(copy_failure("b.txt", Stage::Internal));
    });
    assert_eq!(
        the_toast(&effects).1,
        "복사를 중단했습니다 · 실패 1개 · 남은 항목은 처리하지 않았습니다"
    );
}

#[test]
fn a_retry_is_offered_once_even_after_the_recent_list_is_cleared() {
    let mut state = ready(vec![file("a.txt")], vec![]);
    let spec = copy_of(&["a.txt"], TransferOptions::default());
    let effects = finish(&mut state, 1, &spec, JobWork::Copy, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.failures.push(copy_failure("a.txt", Stage::Write));
    });
    let (token, _, _) = the_toast(&effects);
    // Tried again from 최근 완료, whose list is then cleared, while the toast still shows.
    assert_eq!(state.recent_details(0), []);
    assert!(matches!(
        the_submit(&state.failures_retried()).0,
        JobSpec::Transfer(_)
    ));
    state.recent_cleared();
    assert_eq!(state.recent_rows(), []);
    // The toast's window lists the item still, and offers no second retry.
    assert_eq!(state.toast_clicked(token), []);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the failed items");
    };
    assert_eq!((failures.lines.len(), &failures.retry), (1, &None));
    assert_eq!(state.failures_retried(), []);
}
