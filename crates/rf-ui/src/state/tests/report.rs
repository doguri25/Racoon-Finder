//! The jobs that ended (XFER-14): the job panel's 최근 완료 and its limit, its lines from the
//! keyboard, the log saved where the destination picker chooses (PICK-09), and the
//! notification of a long job that ended while the window was not in front.

use std::time::Duration;

use super::jobs::finish;
use super::*;
use crate::dialogs::{ConfirmPurpose, Modal};
use crate::jobs::RECENT_JOBS;
use crate::keys::{PanelKey, TreeKey};
use crate::notify::Notice;
use crate::pick::PickPurpose;
use crate::words::JobWork;
use rf_jobs::{
    DeleteMethod, DeleteSpec, ErrorPolicy, FailureReason, ItemFailure, JobSpec, Outcome, Stage,
    TransferMode, TransferOptions, TransferSpec,
};

fn in_a(name: &str) -> VPath {
    folder("a").join(name).expect("a plain name")
}

fn copy(names: &[&str]) -> JobSpec {
    let sources: Vec<VPath> = names.iter().map(|name| in_a(name)).collect();
    JobSpec::Transfer(
        TransferSpec::into_dir(
            TransferMode::Copy,
            &sources,
            &folder("b"),
            TransferOptions::default(),
        )
        .expect("a transfer"),
    )
}

fn trash(names: &[&str]) -> JobSpec {
    JobSpec::Delete(DeleteSpec {
        paths: names.iter().map(|name| in_a(name)).collect(),
        method: DeleteMethod::Trash,
        errors: ErrorPolicy::Continue,
    })
}

fn failed(name: &str) -> ItemFailure {
    ItemFailure {
        source: Some(in_a(name)),
        target: None,
        stage: Stage::Read,
        kind: ErrorKind::PermissionDenied,
        message: String::new(),
        reason: None,
    }
}

/// Ends job `id`, a copy of `names` that took `secs` seconds, as `outcome`.
fn copied(state: &mut AppState, id: u64, names: &[&str], secs: u64) -> Vec<Effect> {
    finish(state, id, &copy(names), JobWork::Copy, |report| {
        report.elapsed = Duration::from_secs(secs);
        report.done.files = u64::try_from(names.len()).unwrap_or(0);
    })
}

fn notices(effects: &[Effect]) -> Vec<&Notice> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Notify(notice) => Some(notice),
            _ => None,
        })
        .collect()
}

#[test]
fn finished_jobs_go_into_the_recent_list_twenty_at_most() {
    let mut state = ready(vec![], vec![]);
    assert_eq!(state.recent_rows(), []);
    for id in 1..=22 {
        copied(&mut state, id, &["x"], 1);
    }
    let rows = state.recent_rows();
    assert_eq!(rows.len(), RECENT_JOBS);
    assert_eq!(rows[0].title, "복사했습니다");
    assert_eq!(rows[0].detail, "x → b");
    // A new folder and a rename are not listed: the list shows the item itself at once.
    let create = JobSpec::Create {
        parent: folder("a"),
        name: "새 폴더".into(),
        kind: rf_jobs::CreateKind::Dir,
    };
    finish(&mut state, 30, &create, JobWork::Create, |_| {});
    let rename = JobSpec::Rename {
        from: in_a("x"),
        to: in_a("y"),
    };
    finish(&mut state, 31, &rename, JobWork::Rename, |_| {});
    assert_eq!(state.recent_rows().len(), RECENT_JOBS);
    // A cancelled job is listed with how it ended; the newest comes first, the oldest goes.
    finish(&mut state, 32, &trash(&["x"]), JobWork::Trash, |report| {
        report.outcome = Outcome::Cancelled;
    });
    let rows = state.recent_rows();
    assert_eq!(rows.len(), RECENT_JOBS);
    assert_eq!(rows[0].title, "휴지통으로 보내기를 취소했습니다");
    assert!(!rows[0].ok);
    // Nothing is listed once the app ends.
    state.begin_quit();
    copied(&mut state, 33, &["x"], 1);
    assert_eq!(state.recent_rows()[0].title, rows[0].title);
}

#[test]
fn the_recent_list_opens_and_its_lines_reach_the_keys() {
    let mut state = ready(vec![], vec![]);
    copied(&mut state, 1, &["x"], 1);
    finish(&mut state, 2, &copy(&["y", "z"]), JobWork::Copy, |report| {
        report.outcome = Outcome::CompletedWithErrors;
        report.done.files = 1;
        report.failures.push(failed("z"));
    });
    // The head alone while the list is closed.
    assert!(!state.recent_open());
    assert_eq!(
        state.panel_lines(),
        [[PanelButton::ToggleRecent, PanelButton::ClearRecent]]
    );
    // F6 from the tree reaches it with no job running, and 펼치기 opens the list.
    assert_eq!(state.tree_key(TreeKey::Next), [Effect::FocusPanel]);
    assert_eq!(state.panel_key(PanelKey::Press), []);
    assert!(state.recent_open());
    // The newest first: the copy that failed has its 자세히; both have 로그 저장….
    assert_eq!(
        state.panel_lines(),
        [
            vec![PanelButton::ToggleRecent, PanelButton::ClearRecent],
            vec![PanelButton::Details(0), PanelButton::SaveLog(0)],
            vec![PanelButton::SaveLog(1)],
        ]
    );
    let rows = state.recent_rows();
    assert!(rows[0].details && !rows[0].ok);
    assert!(!rows[1].details && rows[1].ok);
    // 자세히, as the toast of that job had it: the item and why.
    state.panel_key(PanelKey::Down);
    assert_eq!(state.panel_cursor(), Some((1, 0)));
    assert_eq!(state.panel_key(PanelKey::Press), []);
    let Some(Modal::Failures(failures)) = state.modal() else {
        panic!("the items that failed");
    };
    assert_eq!(failures.title, "처리하지 못한 항목");
    assert_eq!(failures.lines, [format!("{}: 권한이 없습니다", in_a("z"))]);
    state.dialog_cancelled();
    // A job without failures has nothing to list.
    assert_eq!(state.recent_details(1), []);
    assert_eq!(state.modal(), None);
    // 접기 closes the list: the keys stay within the lines that show.
    state.panel_key(PanelKey::Up);
    state.panel_key(PanelKey::Press);
    assert!(!state.recent_open());
    assert_eq!(state.panel_cursor(), Some((0, 0)));
}

#[test]
fn a_log_is_saved_where_the_picker_chooses_and_the_notice_says_where() {
    let mut state = ready(vec![], vec![]);
    copied(&mut state, 1, &["x"], 1);
    // Without the app's folder for logs nothing opens.
    assert_eq!(state.save_log_asked(0), []);
    let logs = folder("logs");
    state.set_log_dir(logs.clone());
    let effects = state.save_log_asked(0);
    let Some(Modal::Pick(picker)) = state.modal() else {
        panic!("the destination picker");
    };
    let PickPurpose::SaveLog { title, detail, .. } = &picker.purpose else {
        panic!("a picker for a log");
    };
    assert_eq!((title.as_str(), detail.as_str()), ("복사했습니다", "x → b"));
    // The app's folder for logs first and chosen, then the open tabs.
    assert_eq!(picker.groups[0].title, "로그 폴더");
    assert_eq!(picker.groups[0].items[0].path, logs);
    assert_eq!(picker.groups[1].title, "열린 탭");
    assert_eq!(picker.target, Some(logs.clone()));
    assert!(effects.contains(&Effect::InspectTarget {
        generation: picker.looking,
        path: logs.clone(),
        from_dir: logs.clone(),
    }));
    // It has no move/copy toggle and no conflict policy: a log never goes over a file.
    state.pick_mode(TransferMode::Move);
    state.pick_policy(rf_jobs::ConflictPolicy::Overwrite);
    let picker = state.shown_picker().expect("still there");
    assert_eq!(picker.request.mode, TransferMode::Copy);
    assert_eq!(picker.policy, rf_jobs::ConflictPolicy::Ask);
    // Another folder of the quick targets, then the button: the worker writes it there.
    let other = picker.groups[1].items[0].path.clone();
    state.pick_clicked(1);
    let effects = state.pick_confirmed();
    let [Effect::SaveLog { report, folder: to }] = effects.as_slice() else {
        panic!("the log to write: {effects:?}");
    };
    assert_eq!(*to, other);
    assert_eq!(report.0.id, JobId(1));
    assert_eq!(state.modal(), None);
    // The folder chosen for a log is no target for items.
    assert_eq!(state.recent_targets(), []);
    let effects = state.log_saved(Ok("job-copy-20261030-143207.log".to_owned()));
    assert_eq!(
        effects,
        [Effect::Toast {
            token: state_toast_token(&effects),
            text: "로그를 저장했습니다: job-copy-20261030-143207.log".to_owned(),
            action: String::new(),
        }]
    );
    let effects = state.log_saved(Err(ErrorKind::PermissionDenied));
    assert!(matches!(
        effects.as_slice(),
        [Effect::Toast { text, .. }] if text == "로그를 저장하지 못했습니다: 권한이 없습니다"
    ));
}

/// The token of the one toast of `effects`.
fn state_toast_token(effects: &[Effect]) -> u64 {
    match effects {
        [Effect::Toast { token, .. }] => *token,
        other => panic!("a toast: {other:?}"),
    }
}

#[test]
fn a_long_job_that_ends_out_of_sight_is_notified() {
    let mut state = ready(vec![], vec![]);
    // In front, nothing is notified, however long the job took.
    assert_eq!(
        notices(&copied(&mut state, 1, &["x", "y"], 60)),
        Vec::<&Notice>::new()
    );
    state.set_window_active(false);
    // Ten seconds or less: nothing.
    assert_eq!(
        notices(&copied(&mut state, 2, &["x"], 10)),
        Vec::<&Notice>::new()
    );
    let effects = copied(&mut state, 3, &["x", "y"], 11);
    assert_eq!(
        notices(&effects),
        [&Notice {
            title: "복사를 마쳤습니다 · 항목 2개".to_owned(),
            body: "항목 2개 → b".to_owned(),
        }]
    );
    // A job the user cancelled is not told of.
    let effects = finish(&mut state, 4, &copy(&["x"]), JobWork::Copy, |report| {
        report.outcome = Outcome::Cancelled;
        report.elapsed = Duration::from_secs(30);
    });
    assert_eq!(notices(&effects), Vec::<&Notice>::new());
    // One with failures says how many.
    let effects = finish(
        &mut state,
        5,
        &trash(&["x", "y"]),
        JobWork::Trash,
        |report| {
            report.outcome = Outcome::CompletedWithErrors;
            report.elapsed = Duration::from_secs(12);
            report.failures.push(ItemFailure {
                reason: Some(FailureReason::TrashRefused),
                ..failed("y")
            });
        },
    );
    assert_eq!(
        notices(&effects)[0].title,
        "휴지통으로 보내기를 마쳤지만 항목 1개를 처리하지 못했습니다"
    );
    // The window comes back in front: no more.
    state.set_window_active(true);
    assert_eq!(
        notices(&copied(&mut state, 6, &["x"], 30)),
        Vec::<&Notice>::new()
    );
    // The refusal of job 5 asks before a permanent delete, as before.
    assert!(matches!(
        state.modal(),
        Some(Modal::Confirm(confirm)) if matches!(confirm.purpose, ConfirmPurpose::DeletePermanently(_))
    ));
}
