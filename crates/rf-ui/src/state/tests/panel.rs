//! The job panel from the keyboard (KEY-02): F6 goes from the tree to the panel while it shows,
//! and on to the panes; the keys move among its buttons and press them. And the order of its
//! queue (XFER-01): the jobs that wait move up and down it, by their buttons and by the panel's
//! own keys; and the place of the panel (LAY-09).

use super::*;
use crate::jobs::Started;
use crate::keys::{PanelKey, TreeKey};
use crate::words::JobWork;
use rf_jobs::{DeleteMethod, DeleteSpec, ErrorPolicy, JobEvent, JobSpec, JobState};

/// Follows the trash of `name` as job `id`, which a worker runs now.
pub(super) fn running(state: &mut AppState, id: u64, name: &str) {
    let spec = JobSpec::Delete(DeleteSpec {
        paths: vec![folder("a").join(name).expect("a name")],
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
    set_state(state, id, JobState::Running);
}

fn set_state(state: &mut AppState, id: u64, job: JobState) {
    let effects = state.job_event(JobEvent::State {
        id: JobId(id),
        state: job,
    });
    assert_eq!(effects, []);
}

fn end(state: &mut AppState, id: u64) {
    let spec = JobSpec::Delete(DeleteSpec {
        paths: Vec::new(),
        method: DeleteMethod::Trash,
        errors: ErrorPolicy::Continue,
    });
    state.job_event(JobEvent::Finished {
        id: JobId(id),
        report: Arc::new(JobReport::new(JobId(id), &spec)),
    });
}

#[test]
fn f6_goes_from_the_tree_to_the_job_panel_while_it_shows() {
    let mut state = ready(vec![], vec![]);
    // Without a job the panel does not show: F6 in the tree gives the keys back to the panes,
    // as before, and so does a key of the panel that comes anyway.
    assert_eq!(state.panel_lines(), Vec::<Vec<PanelButton>>::new());
    assert_eq!(state.tree_key(TreeKey::Next), [Effect::FocusPanes]);
    assert_eq!(state.panel_cursor(), None);
    assert_eq!(state.panel_key(PanelKey::Down), [Effect::FocusPanes]);

    running(&mut state, 1, "x");
    assert_eq!(
        state.panel_lines(),
        [[PanelButton::PauseOrResume(0), PanelButton::Cancel(0)]]
    );
    assert_eq!(state.tree_key(TreeKey::Next), [Effect::FocusPanel]);
    assert_eq!(state.panel_cursor(), Some((0, 0)));
    // Esc, Tab and F6 in the panel give the keys to the panes; Esc in the tree still does.
    assert_eq!(state.panel_key(PanelKey::Leave), [Effect::FocusPanes]);
    assert_eq!(state.tree_key(TreeKey::Leave), [Effect::FocusPanes]);
    // F6 from the panes still goes to the tree first.
    let effects = press(&mut state, &special(Key::F6), NONE).expect("a shortcut");
    assert!(effects.contains(&Effect::FocusTree), "{effects:?}");
}

#[test]
fn the_panel_keys_move_among_the_buttons_and_press_them() {
    let mut state = ready(vec![], vec![]);
    running(&mut state, 1, "x");
    running(&mut state, 2, "y");
    state.tree_key(TreeKey::Next);
    let moves = [
        (PanelKey::Right, (0, 1)),
        // The last button of a line stays the last.
        (PanelKey::Right, (0, 1)),
        (PanelKey::Down, (1, 1)),
        (PanelKey::Down, (1, 1)),
        (PanelKey::Left, (1, 0)),
        (PanelKey::Left, (1, 0)),
        (PanelKey::Up, (0, 0)),
        (PanelKey::Up, (0, 0)),
        (PanelKey::Down, (1, 0)),
    ];
    for (key, at) in moves {
        assert_eq!(state.panel_key(key), [], "{key:?}");
        assert_eq!(state.panel_cursor(), Some(at), "{key:?}");
    }
    // Space and Enter press the button: 일시정지 of the second job, then, paused, its 계속.
    assert_eq!(
        state.panel_key(PanelKey::Press),
        [Effect::Job(JobEffect::Pause(JobId(2)))]
    );
    set_state(&mut state, 2, JobState::Paused);
    assert_eq!(
        state.panel_key(PanelKey::Press),
        [Effect::Job(JobEffect::Resume(JobId(2)))]
    );
    state.panel_key(PanelKey::Right);
    assert_eq!(
        state.panel_key(PanelKey::Press),
        [Effect::Job(JobEffect::Cancel(JobId(2)))]
    );
    // The keys stay with the panel.
    assert_eq!(state.panel_cursor(), Some((1, 1)));
}

#[test]
fn the_panel_cursor_stays_on_the_lines_there_are() {
    let mut state = ready(vec![], vec![]);
    running(&mut state, 1, "x");
    running(&mut state, 2, "y");
    state.tree_key(TreeKey::Next);
    state.panel_key(PanelKey::Down);
    state.panel_key(PanelKey::Right);
    assert_eq!(state.panel_cursor(), Some((1, 1)));
    // The second job ends and goes into 최근 완료, whose head takes its line: the keys are on
    // its 지우기 now.
    end(&mut state, 2);
    assert_eq!(
        state.panel_lines(),
        [
            vec![PanelButton::PauseOrResume(0), PanelButton::Cancel(0)],
            vec![PanelButton::ToggleRecent, PanelButton::ClearRecent],
        ]
    );
    assert_eq!(state.panel_cursor(), Some((1, 1)));
    // 지우기 empties the list: the keys are on the line that is left, on the same button.
    assert_eq!(state.panel_key(PanelKey::Press), []);
    assert_eq!(state.recent_rows(), []);
    assert_eq!(state.panel_cursor(), Some((0, 1)));
    assert_eq!(
        state.panel_key(PanelKey::Press),
        [Effect::Job(JobEffect::Cancel(JobId(1)))]
    );
    // With the last one gone and the list cleared, the panel goes, and its keys go back to the
    // panes.
    end(&mut state, 1);
    state.recent_cleared();
    assert_eq!(state.panel_cursor(), None);
    assert_eq!(state.panel_key(PanelKey::Press), [Effect::FocusPanes]);
}

/// Follows the trash of `name` as job `id`, which waits in the queue.
fn waiting(state: &mut AppState, id: u64, name: &str) {
    let spec = JobSpec::Delete(DeleteSpec {
        paths: vec![folder("a").join(name).expect("a name")],
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
    set_state(state, id, JobState::Queued);
}

/// The engine tells the order of the queue.
fn queue(state: &mut AppState, order: &[u64]) {
    let effects = state.job_event(JobEvent::Queue {
        order: order.iter().copied().map(JobId).collect(),
    });
    assert_eq!(effects, []);
}

/// A state where job 1 runs and jobs 2, 3 and 4 wait in that order.
fn one_running_three_waiting() -> AppState {
    let mut state = ready(vec![], vec![]);
    running(&mut state, 1, "w");
    for (id, name) in [(2, "x"), (3, "y"), (4, "z")] {
        waiting(&mut state, id, name);
    }
    queue(&mut state, &[2, 3, 4]);
    state
}

/// The ids of the panel's rows, in their order.
fn row_ids(state: &AppState) -> Vec<u64> {
    state.job_rows().iter().map(|row| row.id.0).collect()
}

fn move_to(id: u64, position: usize) -> Vec<Effect> {
    vec![Effect::Job(JobEffect::MoveTo {
        id: JobId(id),
        position,
    })]
}

#[test]
fn a_job_that_waits_moves_up_and_down_and_no_other_does() {
    let mut state = one_running_three_waiting();
    assert_eq!(row_ids(&state), [1, 2, 3, 4]);
    // 위로 and 아래로 of a job that waits: one place, past the job beside it.
    assert_eq!(state.job_move(2, true), move_to(3, 0));
    assert_eq!(state.job_move(2, false), move_to(3, 2));
    assert_eq!(state.job_move(1, false), move_to(2, 1));
    assert_eq!(state.job_move(3, true), move_to(4, 1));
    // The ends of the queue.
    assert_eq!(state.job_move(1, true), []);
    assert_eq!(state.job_move(3, false), []);
    // The running job, a row that is not there.
    assert_eq!(state.job_move(0, true), []);
    assert_eq!(state.job_move(0, false), []);
    assert_eq!(state.job_move(9, true), []);
    // Nothing moved here: the rows change only when the engine tells the new order.
    assert_eq!(row_ids(&state), [1, 2, 3, 4]);
    queue(&mut state, &[3, 2, 4]);
    assert_eq!(row_ids(&state), [1, 3, 2, 4]);

    // A job paused in mid-run does not move; one paused while it waits does.
    set_state(&mut state, 1, JobState::Paused);
    assert_eq!(state.job_move(0, false), []);
    set_state(&mut state, 2, JobState::Paused);
    assert_eq!(state.job_move(2, true), move_to(2, 0));
    // A job that ended is in 최근 완료, where nothing moves: its old row is another job's now.
    queue(&mut state, &[3, 2]);
    end(&mut state, 4);
    assert_eq!(row_ids(&state), [1, 3, 2]);
    assert_eq!(state.job_move(3, true), []);
    assert_eq!(state.job_move(2, false), []);
}

#[test]
fn the_lines_of_the_jobs_that_wait_have_the_buttons_that_move_them() {
    let mut state = one_running_three_waiting();
    assert_eq!(
        state.panel_lines(),
        [
            vec![PanelButton::PauseOrResume(0), PanelButton::Cancel(0)],
            vec![
                PanelButton::PauseOrResume(1),
                PanelButton::Cancel(1),
                PanelButton::MoveUp(1),
                PanelButton::MoveDown(1),
            ],
            vec![
                PanelButton::PauseOrResume(2),
                PanelButton::Cancel(2),
                PanelButton::MoveUp(2),
                PanelButton::MoveDown(2),
            ],
            vec![
                PanelButton::PauseOrResume(3),
                PanelButton::Cancel(3),
                PanelButton::MoveUp(3),
                PanelButton::MoveDown(3),
            ],
        ]
    );
    state.tree_key(TreeKey::Next);
    // ↓ to the second job that waits, → to its 위로: pressed, it moves that job.
    for key in [
        PanelKey::Down,
        PanelKey::Down,
        PanelKey::Right,
        PanelKey::Right,
    ] {
        assert_eq!(state.panel_key(key), []);
    }
    assert_eq!(state.panel_cursor(), Some((2, 2)));
    assert_eq!(state.panel_key(PanelKey::Press), move_to(3, 0));
    // The keys go with the job as the queue's new order comes, on the same button.
    queue(&mut state, &[3, 2, 4]);
    assert_eq!(row_ids(&state), [1, 3, 2, 4]);
    assert_eq!(state.panel_cursor(), Some((1, 2)));
    // It is first now: its 위로 does nothing.
    assert_eq!(state.panel_key(PanelKey::Press), []);
    // The pause and the cancel of a job that waits reach that job.
    state.panel_key(PanelKey::Left);
    assert_eq!(
        state.panel_key(PanelKey::Press),
        [Effect::Job(JobEffect::Cancel(JobId(3)))]
    );
    // From a waiting job's last button, ↑ to the running job's line takes its last one.
    state.panel_key(PanelKey::Right);
    state.panel_key(PanelKey::Right);
    assert_eq!(state.panel_cursor(), Some((1, 3)));
    state.panel_key(PanelKey::Up);
    assert_eq!(state.panel_cursor(), Some((0, 1)));
}

#[test]
fn the_panels_own_keys_move_the_job_the_keys_are_on() {
    let mut state = one_running_three_waiting();
    state.tree_key(TreeKey::Next);
    // On the running job's line the keys move nothing.
    assert_eq!(state.panel_key(PanelKey::MoveDown), []);
    assert_eq!(state.panel_key(PanelKey::MoveUp), []);
    assert_eq!(state.panel_cursor(), Some((0, 0)));
    // On the last job that waits: up twice, the keys staying with it.
    for _ in 0..3 {
        state.panel_key(PanelKey::Down);
    }
    assert_eq!(state.panel_cursor(), Some((3, 0)));
    assert_eq!(state.panel_key(PanelKey::MoveDown), [], "it is the last");
    assert_eq!(state.panel_key(PanelKey::MoveUp), move_to(4, 1));
    queue(&mut state, &[2, 4, 3]);
    assert_eq!(state.panel_cursor(), Some((2, 0)));
    assert_eq!(state.panel_key(PanelKey::MoveUp), move_to(4, 0));
    queue(&mut state, &[4, 2, 3]);
    assert_eq!(state.panel_cursor(), Some((1, 0)));
    assert_eq!(state.panel_key(PanelKey::MoveUp), [], "it is the first");
    assert_eq!(state.panel_key(PanelKey::MoveDown), move_to(4, 1));
    queue(&mut state, &[2, 4, 3]);
    assert_eq!(row_ids(&state), [1, 2, 4, 3]);
    assert_eq!(state.panel_cursor(), Some((2, 0)));

    // Once the keys move on, they stay where they are put: a later change of the queue takes
    // them nowhere.
    state.panel_key(PanelKey::Up);
    assert_eq!(state.panel_cursor(), Some((1, 0)));
    queue(&mut state, &[4, 3, 2]);
    assert_eq!(state.panel_cursor(), Some((1, 0)));

    // A job the keys moved that a worker then takes up: the keys stay on their line.
    state.panel_key(PanelKey::Down);
    assert_eq!(state.panel_key(PanelKey::MoveDown), move_to(3, 2));
    set_state(&mut state, 3, JobState::Running);
    queue(&mut state, &[4, 2]);
    assert_eq!(row_ids(&state), [1, 3, 4, 2]);
    assert_eq!(state.panel_cursor(), Some((2, 0)));
    // A button pressed by the pointer moves the job and leaves the keys where they are.
    assert_eq!(state.job_move(3, true), move_to(2, 0));
    queue(&mut state, &[2, 4]);
    assert_eq!(state.panel_cursor(), Some((2, 0)));
    // On a line that is no job's (the head of 최근 완료, were there one) the keys move nothing;
    // with no line at all they go back to the panes, as every key of the panel does.
    let mut empty = ready(vec![], vec![]);
    assert_eq!(empty.panel_key(PanelKey::MoveUp), [Effect::FocusPanes]);
}

#[test]
fn the_running_row_names_its_file_and_the_waiting_rows_say_what_they_will_do() {
    let mut state = one_running_three_waiting();
    let file = folder("a").join("사진-0412.jpg").expect("a name");
    state.job_event(JobEvent::Progress {
        id: JobId(1),
        progress: rf_jobs::Progress {
            current: Some(file),
            ..rf_jobs::Progress::default()
        },
    });
    let rows = state.job_rows();
    assert_eq!(rows[0].current, "사진-0412.jpg");
    assert_eq!(rows[0].queued, None);
    // A trash moves no bytes: no graph.
    assert_eq!(rows[0].speeds, None);
    assert_eq!(
        (rows[1].title.as_str(), rows[1].detail.as_str()),
        ("대기 중", "휴지통으로 보내기 · x")
    );
    assert!(rows[1..].iter().all(|row| row.current.is_empty()));
    // The tray icon tells of the job a worker has taken up alone; its line counts the others.
    state.set_tray(true);
    state.request_close();
    state.confirm_alternative();
    let view = state.tray_view("앱").expect("behind the tray");
    assert_eq!(
        view.status,
        "앱\n휴지통으로 보내는 중 0%\n대기 중인 작업 3개"
    );
    assert_eq!(view.label, "0%");
}

#[test]
fn the_speed_samples_come_with_the_time_the_window_gives() {
    use rf_jobs::{Progress, Totals, TransferMode, TransferOptions, TransferSpec};
    use std::time::Duration;

    const MB: u64 = 1024 * 1024;
    let mut state = ready(vec![file("big.bin")], vec![]);
    let spec = JobSpec::Transfer(
        TransferSpec::into_dir(
            TransferMode::Copy,
            &[folder("a").join("big.bin").expect("a name")],
            &folder("b"),
            TransferOptions::default(),
        )
        .expect("a transfer"),
    );
    state.track(Started {
        id: JobId(7),
        spec: Some(Arc::new(spec)),
        work: JobWork::Copy,
        origin: None,
        via_history: false,
    });
    set_state(&mut state, 7, JobState::Running);
    let progress = |state: &mut AppState, bytes: u64| {
        let total = Totals {
            files: 1,
            dirs: 0,
            bytes: 64 * MB,
        };
        state.job_event(JobEvent::Progress {
            id: JobId(7),
            progress: Progress {
                done: Totals {
                    bytes,
                    ..Totals::default()
                },
                total,
                ..Progress::default()
            },
        });
    };
    // The window's clock, handed in: every time counts from this one.
    let start = Instant::now();
    let at = |secs: u64| start + Duration::from_secs(secs);
    progress(&mut state, 0);
    assert!(!state.sample_speeds(at(0)));
    progress(&mut state, 16 * MB);
    assert!(state.sample_speeds(at(1)));
    progress(&mut state, 20 * MB);
    assert!(state.sample_speeds(at(2)));
    assert_eq!(state.job_rows()[0].speeds, Some(vec![16 * MB, 4 * MB]));
    // No job, nothing to sample.
    let mut idle = ready(vec![], vec![]);
    assert!(!idle.sample_speeds(at(3)));
}

#[test]
fn the_job_panel_shows_under_the_panes_or_in_its_own_window() {
    let mut state = ready(vec![], vec![]);
    // Under the panes at first, and at every start that the settings do not say otherwise.
    assert!(!state.layout().jobs_window);
    assert!(!state.session().layout.jobs_window);
    state.set_jobs_window(true);
    assert!(state.layout().jobs_window);
    // The settings keep it with the rest of the layout, and a start gives it back.
    let session = state.session();
    assert!(session.layout.jobs_window);
    let (restored, _) = AppState::restore(folder("a"), folder("b"), session, &[], None);
    assert!(restored.layout().jobs_window);
    // The other choices of the layout are as they were.
    let mut docked = restored.layout();
    docked.jobs_window = false;
    assert_eq!(docked, Layout::default());
    // The panel's lines and its keys are the same wherever it shows.
    running(&mut state, 1, "x");
    let lines = state.panel_lines();
    assert_eq!(state.tree_key(TreeKey::Next), [Effect::FocusPanel]);
    state.set_jobs_window(false);
    assert_eq!(state.panel_lines(), lines);
    assert!(!state.layout().jobs_window);
}
