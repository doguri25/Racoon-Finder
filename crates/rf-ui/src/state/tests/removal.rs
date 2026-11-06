//! 완전 삭제… and 제거 프로그램 열기 in the settings menu (INST-05): the steps of the question,
//! what each answer does, and what the window is when the removal stops.

use super::jobs::finish;
use super::*;
use crate::dialogs::{ConfirmPurpose, Modal};
use crate::jobs::Started;
use crate::removal::Offer;
use crate::words::{self, JobWork};
use rf_jobs::{DeleteMethod, DeleteSpec, ErrorPolicy, JobEvent, JobSpec, JobState};

fn menu(offer: Option<Offer>) -> AppState {
    let mut state = ready(vec![], vec![]);
    state.set_removal_offer(offer);
    state
}

fn plan() -> Vec<String> {
    vec![
        "삭제: /Users/a/Library/Application Support/io.github.doguri25.RacoonFinder".to_owned(),
        "앱을 휴지통으로 옮기기: /Applications/Racoon Finder.app".to_owned(),
    ]
}

fn the_confirm(state: &AppState) -> &crate::dialogs::Confirm {
    match state.modal() {
        Some(Modal::Confirm(confirm)) => confirm,
        other => panic!("a confirmation, not {other:?}"),
    }
}

/// The menu item chosen and its list come: the confirmation shows.
fn asked_about(state: &mut AppState) {
    assert_eq!(state.removal_asked(), [Effect::Removal(RemovalWork::Plan)]);
    assert_eq!(state.removal_planned(Ok(plan())), []);
}

fn running(state: &mut AppState, id: u64) {
    let spec = JobSpec::Delete(DeleteSpec {
        paths: vec![folder("a").join("x").expect("a name")],
        method: DeleteMethod::Trash,
        errors: ErrorPolicy::Continue,
    });
    state.track(Started {
        id: JobId(id),
        spec: Some(Arc::new(spec)),
        work: JobWork::Trash,
        origin: None,
        via_history: false,
    });
    state.job_event(JobEvent::State {
        id: JobId(id),
        state: JobState::Running,
    });
}

#[test]
fn the_menu_item_does_what_the_system_offers() {
    // Nothing offered (Linux, portable): the item does nothing.
    let mut none = menu(None);
    assert_eq!(none.removal_asked(), []);
    // macOS: a worker lists what goes, once while it is being listed.
    let mut mac = menu(Some(Offer::DeleteCompletely));
    assert_eq!(mac.removal_offer(), Some(Offer::DeleteCompletely));
    assert_eq!(mac.removal_asked(), [Effect::Removal(RemovalWork::Plan)]);
    assert_eq!(mac.removal_asked(), []);
    // Windows, installed: the uninstaller starts, as often as it is asked.
    let mut windows = menu(Some(Offer::OpenUninstaller));
    assert_eq!(
        windows.removal_asked(),
        [Effect::Removal(RemovalWork::OpenUninstaller)]
    );
    assert_eq!(
        windows.removal_asked(),
        [Effect::Removal(RemovalWork::OpenUninstaller)]
    );
    assert_eq!(windows.modal(), None);
}

#[test]
fn the_list_comes_in_a_confirmation_with_a_backup_checkbox_that_starts_off() {
    let mut state = menu(Some(Offer::DeleteCompletely));
    asked_about(&mut state);

    let confirm = the_confirm(&state);
    assert_eq!(confirm.purpose, ConfirmPurpose::RemoveApp);
    assert_eq!(confirm.title, "Racoon Finder 완전 삭제");
    assert_eq!(confirm.lines, plan());
    assert_eq!(confirm.check, "문서 폴더에 설정 백업");
    assert!(!confirm.checked);
    assert!(confirm.danger);
    assert_eq!(confirm.ok, "완전 삭제");
    assert!(state.removal_asked_about());

    state.confirm_check_toggled();
    assert!(the_confirm(&state).checked);
    state.confirm_check_toggled();
    assert!(!the_confirm(&state).checked);
}

#[test]
fn a_list_that_could_not_be_made_is_a_notice_and_the_menu_works_again() {
    let mut state = menu(Some(Offer::DeleteCompletely));
    assert_eq!(state.removal_asked(), [Effect::Removal(RemovalWork::Plan)]);
    assert_eq!(
        state.removal_planned(Err("ledger.json is refused".to_owned())),
        [Effect::Notice(words::removal_plan_failed(
            "ledger.json is refused"
        ))]
    );
    assert_eq!(state.modal(), None);
    assert_eq!(state.removal_asked(), [Effect::Removal(RemovalWork::Plan)]);
}

#[test]
fn cancelling_the_confirmation_deletes_nothing() {
    let mut state = menu(Some(Offer::DeleteCompletely));
    asked_about(&mut state);
    assert_eq!(state.dialog_cancelled(), []);
    assert_eq!(state.modal(), None);
    assert!(!state.quitting() && !state.removing());
}

#[test]
fn confirming_without_jobs_begins_the_removal_with_the_checkbox_as_it_was() {
    for backup in [false, true] {
        let mut state = menu(Some(Offer::DeleteCompletely));
        asked_about(&mut state);
        if backup {
            state.confirm_check_toggled();
        }

        assert_eq!(
            state.confirm_accepted(),
            [Effect::Removal(RemovalWork::Run { backup })]
        );

        // The window is the closing note and takes no question.
        assert!(state.quitting() && state.removing());
        assert_eq!(state.modal(), None);
        assert_eq!(state.removal_asked(), []);
        assert_eq!(state.begin_quit(), []);
    }
}

#[test]
fn confirming_with_jobs_asks_again_and_cancel_is_the_wait() {
    let mut state = menu(Some(Offer::DeleteCompletely));
    running(&mut state, 1);
    running(&mut state, 2);
    asked_about(&mut state);
    state.confirm_check_toggled();

    // Nothing begins: the jobs are asked about, with the choice of the checkbox kept.
    assert_eq!(state.confirm_accepted(), []);
    assert!(!state.quitting());
    let confirm = the_confirm(&state);
    assert_eq!(
        confirm.purpose,
        ConfirmPurpose::RemoveAppWithJobs { backup: true }
    );
    assert_eq!(confirm.message, words::removal_with_jobs(2));
    assert!(confirm.danger);

    // 취소 waits for them: the jobs run on and nothing is stopped.
    assert_eq!(state.dialog_cancelled(), []);
    assert_eq!(state.modal(), None);
    assert!(!state.quitting() && !state.removing());
    assert_eq!(state.active_jobs(), 2);

    // Asked again, and this time the jobs are stopped for good.
    asked_about(&mut state);
    assert_eq!(state.confirm_accepted(), []);
    assert_eq!(
        state.confirm_accepted(),
        [Effect::Removal(RemovalWork::Run { backup: false })]
    );
    assert!(state.removing());
}

#[test]
fn jobs_that_ended_while_the_confirmation_showed_are_not_asked_about() {
    let mut state = menu(Some(Offer::DeleteCompletely));
    running(&mut state, 1);
    asked_about(&mut state);
    finish(
        &mut state,
        1,
        &JobSpec::Delete(DeleteSpec {
            paths: vec![folder("a").join("x").expect("a name")],
            method: DeleteMethod::Trash,
            errors: ErrorPolicy::Continue,
        }),
        JobWork::Trash,
        |_| {},
    );
    assert_eq!(state.active_jobs(), 0);
    assert_eq!(
        state.confirm_accepted(),
        [Effect::Removal(RemovalWork::Run { backup: false })]
    );
}

#[test]
fn a_backup_that_failed_leaves_the_app_as_it_was() {
    let mut state = menu(Some(Offer::DeleteCompletely));
    asked_about(&mut state);
    state.confirm_check_toggled();
    state.confirm_accepted();
    let text = words::cleanup_backup_failed(Path::new("/Users/a/Documents/b.toml"), "full");

    let effects = state.removal_stopped(RemovalStop::BackupFailed(text.clone()));

    assert_eq!(effects, [Effect::Notice(text)]);
    assert!(!state.quitting() && !state.removing());
    assert_eq!(state.modal(), None);
    // The item works again.
    assert_eq!(state.removal_asked(), [Effect::Removal(RemovalWork::Plan)]);
}

#[test]
fn a_removal_that_stopped_part_way_says_how_far_it_got_and_the_app_stays() {
    let mut state = menu(Some(Offer::DeleteCompletely));
    asked_about(&mut state);
    state.confirm_accepted();

    let lines = vec![
        "완료 · 삭제: /a".to_owned(),
        "남음 · 삭제: /b (in use)".to_owned(),
    ];
    assert_eq!(
        state.removal_stopped(RemovalStop::Partway(lines.clone())),
        []
    );

    assert!(!state.quitting() && !state.removing());
    let Some(Modal::Failures(shown)) = state.modal() else {
        panic!("a window that says how far it got");
    };
    assert_eq!(shown.title, "완전 삭제를 마치지 못했습니다");
    assert_eq!(shown.lines[0], words::removal_stopped_note());
    assert_eq!(shown.lines[1..], lines);
}

#[test]
fn a_stop_nobody_asked_for_changes_nothing() {
    let mut state = menu(Some(Offer::DeleteCompletely));
    assert_eq!(
        state.removal_stopped(RemovalStop::Partway(vec!["x".to_owned()])),
        []
    );
    assert_eq!(state.modal(), None);
}

#[test]
fn nothing_is_offered_while_the_app_ends() {
    let mut state = menu(Some(Offer::DeleteCompletely));
    state.begin_quit();
    assert_eq!(state.removal_asked(), []);
    assert_eq!(state.removal_planned(Ok(plan())), []);
    assert_eq!(state.modal(), None);
}
