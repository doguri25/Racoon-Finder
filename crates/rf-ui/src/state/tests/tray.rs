//! The jobs going on behind the tray icon while the window is hidden (XFER-15): where the
//! question before the window closes offers it, the icon's menu, the window coming back to
//! ask, and the app's end after the last job, in the order the app always ends.

use std::time::Duration;

use super::jobs::finish;
use super::*;
use crate::dialogs::{ConfirmPurpose, Modal};
use crate::jobs::Started;
use crate::notify::Notice;
use crate::resolver::{Asked, Question};
use crate::words::{self, JobWork};
use rf_jobs::{
    DeleteMethod, DeleteSpec, ErrorPolicy, FailureReason, ItemFailure, JobEvent, JobSpec, JobState,
    Stage,
};

fn trash(names: &[&str]) -> JobSpec {
    JobSpec::Delete(DeleteSpec {
        paths: names
            .iter()
            .map(|name| folder("a").join(name).expect("a name"))
            .collect(),
        method: DeleteMethod::Trash,
        errors: ErrorPolicy::Continue,
    })
}

/// Follows the trash of `name` as job `id`, which a worker runs now.
fn running(state: &mut AppState, id: u64, name: &str) {
    state.track(Started {
        id: JobId(id),
        spec: Some(Arc::new(trash(&[name]))),
        work: JobWork::Trash,
        origin: None,
        via_history: false,
    });
    set_state(state, id, JobState::Running);
}

fn set_state(state: &mut AppState, id: u64, job: JobState) {
    state.job_event(JobEvent::State {
        id: JobId(id),
        state: job,
    });
}

/// A state with the tray where the system has one, and jobs `ids` running.
fn with_jobs(ids: &[u64]) -> AppState {
    let mut state = ready(vec![], vec![]);
    state.set_tray(true);
    for &id in ids {
        running(&mut state, id, &format!("x{id}"));
    }
    state
}

/// Closes the window and goes on behind the tray icon.
fn to_tray(state: &mut AppState) {
    assert_eq!(state.request_close(), []);
    assert_eq!(state.confirm_alternative(), [Effect::ToTray]);
    assert!(state.background());
}

fn the_confirm(state: &AppState) -> &crate::dialogs::Confirm {
    match state.modal() {
        Some(Modal::Confirm(confirm)) => confirm,
        other => panic!("a confirmation, not {other:?}"),
    }
}

fn question(id: u64, job: u64) -> Question {
    Question {
        id,
        job: JobId(job),
        asked: Asked::Preflight(Vec::new()),
    }
}

#[test]
fn closing_with_jobs_offers_the_tray_where_the_system_has_one() {
    // Without a tray the question is as it was.
    let mut state = with_jobs(&[1]);
    state.set_tray(false);
    state.request_close();
    let confirm = the_confirm(&state);
    assert_eq!((confirm.question.as_str(), confirm.alt.as_str()), ("", ""));
    assert_eq!(state.confirm_alternative(), []);
    state.dialog_cancelled();

    // With one, it asks whether the jobs go on behind it, its main answer.
    state.set_tray(true);
    state.request_close();
    let confirm = the_confirm(&state);
    assert_eq!(confirm.purpose, ConfirmPurpose::Quit);
    assert_eq!(confirm.question, words::quit_or_tray(Os::CURRENT));
    assert_eq!(confirm.alt, words::continue_in_tray(Os::CURRENT));
    assert_eq!(confirm.ok, "끝내기");
    assert_eq!(state.confirm_alternative(), [Effect::ToTray]);
    assert!(state.background());
    assert_eq!(state.modal(), None);
    assert!(!state.quitting());
    // The icon shows how far the job has got, and the app.
    let view = state.tray_view("앱").expect("behind the tray");
    assert!(
        view.status.starts_with("앱\n휴지통으로 보내는 중 0%"),
        "{view:?}"
    );
    assert_eq!(view.label, "0%");
    assert!(!view.all_paused);
}

#[test]
fn the_tray_is_not_offered_while_a_question_waits() {
    let mut state = with_jobs(&[1]);
    state.question(question(7, 1));
    state.request_close();
    // The question waits under the confirmation: going behind the tray would leave it there.
    assert_eq!(the_confirm(&state).alt, "");
    assert_eq!(state.confirm_alternative(), []);
    assert!(!state.background());
}

#[test]
fn behind_the_tray_a_question_brings_the_window_back() {
    let mut state = with_jobs(&[1]);
    to_tray(&mut state);
    assert_eq!(state.question(question(7, 1)), [Effect::FromTray]);
    assert!(!state.background());
    assert!(matches!(state.modal(), Some(Modal::Preflight(q)) if q.id == 7));
    assert_eq!(state.tray_view("앱"), None);
}

#[test]
fn the_tray_menu_pauses_resumes_opens_and_quits() {
    let mut state = with_jobs(&[1, 2]);
    // Nothing behind the tray: its menu does nothing.
    assert_eq!(state.tray_command(TrayCommand::PauseAll), []);
    to_tray(&mut state);
    assert_eq!(
        state.tray_command(TrayCommand::PauseAll),
        [
            Effect::Job(JobEffect::Pause(JobId(1))),
            Effect::Job(JobEffect::Pause(JobId(2)))
        ]
    );
    set_state(&mut state, 1, JobState::Paused);
    assert!(!state.tray_view("앱").expect("shows").all_paused);
    set_state(&mut state, 2, JobState::Paused);
    assert!(state.tray_view("앱").expect("shows").all_paused);
    assert_eq!(
        state.tray_command(TrayCommand::ResumeAll),
        [
            Effect::Job(JobEffect::Resume(JobId(1))),
            Effect::Job(JobEffect::Resume(JobId(2)))
        ]
    );
    set_state(&mut state, 1, JobState::Running);
    set_state(&mut state, 2, JobState::Running);
    // 창 열기: the window comes back, and the icon goes.
    assert_eq!(state.tray_command(TrayCommand::Open), [Effect::FromTray]);
    assert!(!state.background());
    assert_eq!(state.tray_view("앱"), None);
    // 끝내기 while jobs run: the window comes back with the question before the app ends,
    // without the tray this time; yes ends the app as ever.
    to_tray(&mut state);
    assert_eq!(state.tray_command(TrayCommand::Quit), [Effect::FromTray]);
    let confirm = the_confirm(&state);
    assert_eq!(confirm.purpose, ConfirmPurpose::Quit);
    assert_eq!(confirm.alt, "");
    assert_eq!(state.confirm_accepted(), [Effect::Quit]);
    assert!(state.quitting());
}

#[test]
fn the_last_job_behind_the_tray_is_told_of_and_the_app_ends() {
    let mut state = with_jobs(&[1, 2]);
    to_tray(&mut state);
    // The first one ends quickly: nothing to tell yet, and the jobs go on.
    let effects = finish(&mut state, 1, &trash(&["x1"]), JobWork::Trash, |report| {
        report.elapsed = Duration::from_secs(2);
    });
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Notify(_) | Effect::Quit))
    );
    assert!(state.background());
    // The last one ends: one notification, whatever time it took, then the app's end, which
    // writes the settings, shuts the engine down and then releases the questions (`Quit`).
    let effects = finish(&mut state, 2, &trash(&["x2"]), JobWork::Trash, |report| {
        report.elapsed = Duration::from_secs(1);
    });
    let at = |wanted: fn(&Effect) -> bool| effects.iter().position(wanted);
    let notified = at(|effect| matches!(effect, Effect::Notify(_))).expect("told");
    let quit = at(|effect| *effect == Effect::Quit).expect("the end");
    assert!(notified < quit, "{effects:?}");
    assert_eq!(
        effects[notified],
        Effect::Notify(Notice {
            title: "휴지통으로 보내기를 마쳤습니다 · 항목 1개".to_owned(),
            body: format!("x2\n{}", words::all_jobs_done()),
        })
    );
    assert!(state.quitting());
    assert_eq!(state.tray_view("앱"), None);
}

#[test]
fn a_job_end_that_asks_brings_the_window_back_instead_of_ending() {
    let mut state = with_jobs(&[1]);
    to_tray(&mut state);
    // The trash refused an item: the app asks before deleting it for good.
    let effects = finish(&mut state, 1, &trash(&["x1"]), JobWork::Trash, |report| {
        report.failures.push(ItemFailure {
            source: Some(folder("a").join("x1").expect("a name")),
            target: None,
            stage: Stage::Delete,
            kind: ErrorKind::Unsupported,
            message: String::new(),
            reason: Some(FailureReason::TrashRefused),
        });
    });
    assert!(effects.contains(&Effect::FromTray), "{effects:?}");
    assert!(!effects.contains(&Effect::Quit));
    assert!(!state.background() && !state.quitting());
    assert!(matches!(
        state.modal(),
        Some(Modal::Confirm(confirm)) if matches!(confirm.purpose, ConfirmPurpose::DeletePermanently(_))
    ));
}

#[test]
fn jobs_that_end_before_the_answer_end_the_app_at_once() {
    let mut state = with_jobs(&[1]);
    state.request_close();
    finish(&mut state, 1, &trash(&["x1"]), JobWork::Trash, |_| {});
    // Nothing is left to go on behind the tray.
    assert_eq!(state.confirm_alternative(), [Effect::Quit]);
    assert!(state.quitting() && !state.background());
}

#[test]
fn every_job_paused_offers_no_tray() {
    let mut state = with_jobs(&[1, 2]);
    set_state(&mut state, 1, JobState::Paused);
    set_state(&mut state, 2, JobState::Paused);
    // Nothing behind the tray would end them: the plain question.
    state.request_close();
    assert_eq!(the_confirm(&state).alt, "");
    assert_eq!(state.confirm_alternative(), []);
    state.dialog_cancelled();
    // One going on again: the tray is offered.
    set_state(&mut state, 2, JobState::Running);
    state.request_close();
    assert_ne!(the_confirm(&state).alt, "");
}

#[test]
fn only_paused_jobs_left_bring_the_window_back() {
    let mut state = with_jobs(&[1, 2]);
    to_tray(&mut state);
    set_state(&mut state, 2, JobState::Paused);
    // The running one ends; the paused one would never end behind the tray.
    let effects = finish(&mut state, 1, &trash(&["x1"]), JobWork::Trash, |_| {});
    assert!(effects.contains(&Effect::FromTray), "{effects:?}");
    assert!(!effects.contains(&Effect::Quit));
    assert!(!state.background() && !state.quitting());
    assert_eq!(state.active_jobs(), 1);
}

#[test]
fn whatever_asks_the_user_brings_a_hidden_window_back() {
    let mut state = with_jobs(&[1]);
    to_tray(&mut state);
    assert_eq!(state.follow_background(), []);
    // A confirmation, however it came, is never left behind the hidden window.
    state.open(Modal::Confirm(crate::dialogs::Confirm::quit(1, None)));
    assert_eq!(state.follow_background(), [Effect::FromTray]);
    assert!(!state.background());
    assert_eq!(state.follow_background(), []);
}

#[test]
fn a_job_that_failed_out_of_sight_brings_the_window_back_at_the_end() {
    let mut state = with_jobs(&[1, 2]);
    to_tray(&mut state);
    // One fails while the other goes on: the jobs go on hidden.
    let effects = finish(&mut state, 1, &trash(&["x1"]), JobWork::Trash, |report| {
        report.outcome = rf_jobs::Outcome::CompletedWithErrors;
        report.failures.push(ItemFailure {
            source: Some(folder("a").join("x1").expect("a name")),
            target: None,
            stage: Stage::Delete,
            kind: ErrorKind::PermissionDenied,
            message: String::new(),
            reason: None,
        });
    });
    assert!(!effects.contains(&Effect::FromTray), "{effects:?}");
    assert!(state.background());
    // The last one ends well: the notification says the window came back to show what failed,
    // and the app goes on, with 최근 완료 and its 자세히.
    let effects = finish(&mut state, 2, &trash(&["x2"]), JobWork::Trash, |_| {});
    let notified = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Notify(notice) => Some(notice.body.clone()),
            _ => None,
        })
        .expect("told");
    assert_eq!(
        notified,
        format!("x2\n{}", words::all_jobs_done_with_failures())
    );
    assert!(effects.contains(&Effect::FromTray), "{effects:?}");
    assert!(!effects.contains(&Effect::Quit));
    assert!(!state.quitting() && !state.background());
    assert!(state.recent_rows().iter().any(|row| row.details));
}

#[test]
fn a_second_start_shows_the_window() {
    let mut state = with_jobs(&[1]);
    // In front already: it comes forward all the same.
    assert_eq!(state.show_asked(), [Effect::FromTray]);
    to_tray(&mut state);
    assert_eq!(state.show_asked(), [Effect::FromTray]);
    assert!(!state.background());
    assert_eq!(state.tray_view("앱"), None);
}

#[test]
fn a_tray_icon_that_could_not_be_made_leaves_the_window_and_the_offer_goes() {
    let mut state = with_jobs(&[1]);
    to_tray(&mut state);
    let effects = state.tray_unavailable();
    assert_eq!(
        effects,
        [Effect::Notice(words::tray_unavailable().to_owned())]
    );
    assert!(!state.background());
    // From now on the question is the plain one.
    state.request_close();
    assert_eq!(the_confirm(&state).alt, "");
}
