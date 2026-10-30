//! The job panel from the keyboard (KEY-02): F6 goes from the tree to the panel while it shows,
//! and on to the panes; the keys move among its buttons and press them.

use super::*;
use crate::jobs::Started;
use crate::keys::{PanelKey, TreeKey};
use crate::words::JobWork;
use rf_jobs::{DeleteMethod, DeleteSpec, ErrorPolicy, JobEvent, JobSpec, JobState};

/// Follows the trash of `name` as job `id`, which a worker runs now.
fn running(state: &mut AppState, id: u64, name: &str) {
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
