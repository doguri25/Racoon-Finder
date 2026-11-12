//! The job panel's 되돌리기 기록 (OPS-12): the rows, the next one, the buttons that do what the
//! keys do, the times, the rows that cannot run any more, and the keys of the panel.

use std::time::{Duration, SystemTime};

use jiff::tz::TimeZone;
use rf_jobs::{CreateKind, CreatedItem};

use super::*;
use crate::jobs::{History, JobEffect, Started};
use crate::keys::{PanelKey, TreeKey};
use crate::words::{HISTORY_SHOWN, JobWork, Stale};

fn in_a(name: &str) -> VPath {
    folder("a").join(name).expect("a plain name")
}

fn in_b(name: &str) -> VPath {
    folder("b").join(name).expect("a plain name")
}

/// 2026-10-30 05:32:00 UTC.
fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_mins(29_888_972)
}

fn later(minutes: u64) -> SystemTime {
    t0() + Duration::from_mins(minutes)
}

fn renamed(from: &str, to: &str) -> UndoRecord {
    UndoRecord::Renamed {
        from: in_a(from),
        to: in_a(to),
    }
}

/// A history whose lists are `undo` and `redo`, the oldest first, read at `at`.
fn history(undo: &[UndoRecord], redo: &[UndoRecord], at: SystemTime) -> History {
    History {
        top: undo.last().cloned(),
        can_redo: !redo.is_empty(),
        undo: undo.to_vec(),
        redo: redo.to_vec(),
        at,
    }
}

fn view(state: &AppState) -> HistoryView {
    state.history_view(&TimeZone::UTC, later(60))
}

fn texts(lines: &[HistoryLine]) -> Vec<&str> {
    lines.iter().map(|line| line.text.as_str()).collect()
}

/// A window whose left pane lists the items `names` (files) of `a`.
fn listing(names: &[&str]) -> AppState {
    ready(names.iter().map(|name| file(name)).collect(), vec![])
}

#[test]
fn history_lists_the_latest_first_and_marks_the_next_row_of_each_list() {
    let mut state = listing(&["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]);
    state.set_history(history(
        &[renamed("a.txt", "b.txt"), renamed("c.txt", "d.txt")],
        &[renamed("e.txt", "a.txt")],
        t0(),
    ));
    assert!(state.history_any());
    let rows = view(&state);
    assert_eq!(
        texts(&rows.undo),
        ["이름 바꾸기: c.txt → d.txt", "이름 바꾸기: a.txt → b.txt"]
    );
    // What a redo does is the undo's record the other way round.
    assert_eq!(texts(&rows.redo), ["이름 바꾸기: a.txt → e.txt"]);
    assert_eq!((rows.undo_total, rows.redo_total), (2, 1));
    // Only the first of each list is next, and only it can be pressed.
    assert_eq!(
        rows.undo
            .iter()
            .map(|l| (l.next, l.runnable))
            .collect::<Vec<_>>(),
        [(true, true), (false, false)]
    );
    assert!(rows.redo[0].next && rows.redo[0].runnable);
    assert_eq!(rows.undo[0].note, words::history_note(false, true, None));
    assert_eq!(rows.undo[1].note, words::history_note(false, false, None));
    assert_eq!(rows.redo[0].note, words::history_note(true, true, None));
    assert!(rows.undo.iter().chain(&rows.redo).all(|line| !line.dimmed));
}

#[test]
fn history_is_empty_without_records() {
    let state = listing(&[]);
    assert!(!state.history_any());
    assert_eq!(view(&state), HistoryView::default());
    // The toolbar's two flags alone (a history that tells no lists) show no rows.
    let mut state = listing(&[]);
    state.set_history(History {
        top: Some(renamed("x", "y")),
        can_redo: true,
        ..History::default()
    });
    assert!(!state.history_any());
}

#[test]
fn history_shows_at_most_fifty_rows_of_each_list_and_counts_them_all() {
    let many: Vec<UndoRecord> = (0..60)
        .map(|i| renamed(&format!("f{i}"), &format!("g{i}")))
        .collect();
    let mut state = listing(&[]);
    state.set_history(history(&many, &many[..55], t0()));
    let rows = view(&state);
    assert_eq!(
        (rows.undo.len(), rows.redo.len()),
        (HISTORY_SHOWN, HISTORY_SHOWN)
    );
    assert_eq!((rows.undo_total, rows.redo_total), (60, 55));
    // The latest is the first row of the undo list.
    assert_eq!(rows.undo[0].text, "이름 바꾸기: f59 → g59");
    assert_eq!(rows.undo[49].text, "이름 바꾸기: f10 → g10");
}

#[test]
fn history_buttons_do_what_the_keys_do_and_only_for_the_next_row() {
    let build = || {
        let mut state = listing(&["a.txt", "b.txt", "c.txt", "d.txt"]);
        state.set_history(history(
            &[renamed("a.txt", "b.txt"), renamed("c.txt", "d.txt")],
            &[renamed("c.txt", "a.txt"), renamed("a.txt", "d.txt")],
            t0(),
        ));
        state
    };
    // Ctrl+Z and the first row's 되돌리기 are the same effects.
    let (mut by_key, mut by_button) = (build(), build());
    let keys = press(&mut by_key, "z", CTRL).expect("a shortcut");
    assert_eq!(keys, [Effect::Job(JobEffect::Undo)]);
    assert_eq!(by_button.history_run(false, 0), keys);
    // So are Ctrl+Y and the first row's 다시 실행.
    let (mut by_key, mut by_button) = (build(), build());
    let keys = press(&mut by_key, "y", CTRL).expect("a shortcut");
    assert_eq!(keys, [Effect::Job(JobEffect::Redo)]);
    assert_eq!(by_button.history_run(true, 0), keys);
    // A row in the middle is never undone alone: its button does nothing.
    let mut state = build();
    assert_eq!(state.history_run(false, 1), []);
    assert_eq!(state.history_run(true, 1), []);
    assert_eq!(state.history_run(false, 7), []);
    // While an undo or a redo runs, neither button starts another, as the keys do not.
    state.track(Started {
        id: JobId(9),
        spec: None,
        work: JobWork::Undo,
        origin: None,
        via_history: true,
    });
    assert_eq!(press(&mut state, "z", CTRL), Some(vec![]));
    assert_eq!(state.history_run(false, 0), []);
    assert_eq!(state.history_run(true, 0), []);
    assert!(view(&state).undo.iter().all(|line| !line.runnable));
    assert!(view(&state).redo.iter().all(|line| !line.runnable));
}

#[test]
fn history_button_clears_the_toast_as_the_keys_do() {
    let mut state = listing(&["a.txt", "b.txt"]);
    let record = renamed("a.txt", "b.txt");
    state.set_history(history(std::slice::from_ref(&record), &[], t0()));
    let effects = state.toast(
        "이름을 바꿨습니다".to_owned(),
        Some(ToastAction::Undo(record)),
    );
    assert!(matches!(effects, Effect::Toast { .. }));
    assert!(state.toast_button_live());
    assert_eq!(state.history_run(false, 0), [Effect::Job(JobEffect::Undo)]);
    assert!(!state.toast_button_live());
}

#[test]
fn history_times_are_those_of_the_first_time_a_record_was_seen() {
    let mut state = listing(&["a.txt", "b.txt", "c.txt", "d.txt"]);
    let first = renamed("a.txt", "b.txt");
    let second = renamed("c.txt", "d.txt");
    state.set_history(history(std::slice::from_ref(&first), &[], later(0)));
    state.set_history(history(&[first.clone(), second.clone()], &[], later(10)));
    // Read again later with nothing new: the times stay.
    state.set_history(history(&[first.clone(), second.clone()], &[], later(20)));
    let rows = state.history_view(&TimeZone::UTC, later(30));
    assert_eq!(
        rows.undo
            .iter()
            .map(|l| l.time.as_str())
            .collect::<Vec<_>>(),
        ["05:42", "05:32"]
    );
    // A record that leaves and one that comes: only the new one takes the new time.
    state.set_history(history(std::slice::from_ref(&first), &[second], later(40)));
    let rows = state.history_view(&TimeZone::UTC, later(50));
    assert_eq!(rows.undo[0].time, "05:32");
    assert_eq!(rows.redo[0].time, "05:42");
    // A history that does not tell the time shows none.
    let mut state = listing(&[]);
    state.set_history(History {
        undo: vec![first],
        ..History::default()
    });
    assert_eq!(view(&state).undo[0].time, "");
}

#[test]
fn history_dims_a_row_whose_items_are_gone_and_says_why() {
    // `y` is not in the folder the rename made it in: the undo has nothing to rename back.
    let mut state = listing(&["a.txt"]);
    state.set_history(history(&[renamed("a.txt", "y.txt")], &[], t0()));
    let rows = view(&state);
    let line = &rows.undo[0];
    // Dimmed, with its reason, but its button stays: the listing may lag behind the disk for a
    // moment after the rename, and the undo looks again itself.
    assert!(line.dimmed && line.runnable && line.next);
    assert_eq!(
        line.note,
        words::history_note(false, true, Some(Stale::Missing))
    );
    assert_eq!(state.history_run(false, 0), [Effect::Job(JobEffect::Undo)]);
    // It is there: the row is as any other.
    let mut state = listing(&["y.txt"]);
    state.set_history(history(&[renamed("a.txt", "y.txt")], &[], t0()));
    assert!(!view(&state).undo[0].dimmed);
    // The folder is not on screen: nothing tells, so the row stays live.
    let elsewhere = UndoRecord::Renamed {
        from: folder("other").join("p").expect("a plain name"),
        to: folder("other").join("q").expect("a plain name"),
    };
    let mut state = listing(&[]);
    state.set_history(history(&[elsewhere], &[], t0()));
    assert!(!view(&state).undo[0].dimmed);
}

#[test]
fn history_dims_a_copy_that_changed_and_a_move_that_is_gone() {
    let copy = |size: u64| UndoRecord::Copied {
        created: vec![CreatedItem {
            path: in_a("c.txt"),
            is_dir: false,
            size,
            modified: None,
            identity: None,
        }],
        overwritten: 0,
        at: None,
    };
    let mut changed = file("c.txt");
    changed.size = 7;
    let mut state = ready(vec![changed], vec![]);
    state.set_history(history(&[copy(5)], &[], t0()));
    let line = view(&state).undo.remove(0);
    assert!(line.dimmed);
    assert_eq!(
        line.note,
        words::history_note(false, true, Some(Stale::Changed))
    );
    // As the copy left it: live.
    state.set_history(history(&[copy(7)], &[], t0()));
    assert!(!view(&state).undo[0].dimmed);
    // A move undoes by moving each item back from where it went: one that is gone is stale.
    let moved = UndoRecord::Moved {
        moves: vec![
            rf_jobs::MovedItem {
                from: in_a("m1"),
                to: in_b("m1"),
            },
            rf_jobs::MovedItem {
                from: in_a("m2"),
                to: in_b("m2"),
            },
        ],
        created_dirs: Vec::new(),
        removed_dirs: Vec::new(),
        overwritten: 0,
        copied_dirs: Vec::new(),
    };
    let mut state = ready(vec![], vec![file("m1")]);
    state.set_history(history(std::slice::from_ref(&moved), &[], t0()));
    assert_eq!(
        view(&state).undo[0].note,
        words::history_note(false, true, Some(Stale::Missing))
    );
    let mut state = ready(vec![], vec![file("m1"), file("m2")]);
    state.set_history(history(&[moved], &[], t0()));
    assert!(!view(&state).undo[0].dimmed);
}

#[test]
fn history_dims_a_made_folder_that_is_gone_and_leaves_the_trash_alone() {
    let made = UndoRecord::Created {
        path: in_a("새 폴더"),
        kind: CreateKind::Dir,
    };
    let mut state = listing(&[]);
    state.set_history(history(&[made], &[], t0()));
    assert!(view(&state).undo[0].dimmed);
    // What the trash holds cannot be told from a listing: the row stays live.
    let trashed = UndoRecord::Trashed {
        items: vec![rf_jobs::TrashedItem {
            original: in_a("gone.txt"),
            location: None,
        }],
    };
    state.set_history(history(&[trashed], &[], t0()));
    assert!(!view(&state).undo[0].dimmed);
}

#[test]
fn history_panel_lines_come_after_the_jobs_and_the_recent_ones() {
    let mut state = listing(&["a.txt", "b.txt", "c.txt", "d.txt"]);
    assert_eq!(state.panel_lines(), Vec::<Vec<PanelButton>>::new());
    state.set_history(history(
        &[renamed("a.txt", "b.txt"), renamed("c.txt", "d.txt")],
        &[renamed("c.txt", "a.txt")],
        t0(),
    ));
    // The group shows by its head alone while it is closed (the default).
    assert!(!state.history_open());
    assert_eq!(state.panel_lines(), [[PanelButton::ToggleHistory]]);
    assert_eq!(state.tree_key(TreeKey::Next), [Effect::FocusPanel]);
    // Enter on the head opens it: one line for each row, the undo list first.
    assert_eq!(state.panel_key(PanelKey::Press), []);
    assert!(state.history_open());
    assert_eq!(
        state.panel_lines(),
        [
            vec![PanelButton::ToggleHistory],
            vec![PanelButton::RunHistory {
                redo: false,
                index: 0
            }],
            vec![PanelButton::RunHistory {
                redo: false,
                index: 1
            }],
            vec![PanelButton::RunHistory {
                redo: true,
                index: 0
            }],
        ]
    );
    // Arrows move among the rows; Enter on the first runs it as Ctrl+Z does, on the second
    // does nothing.
    state.panel_key(PanelKey::Down);
    assert_eq!(state.panel_cursor(), Some((1, 0)));
    assert_eq!(
        state.panel_key(PanelKey::Press),
        [Effect::Job(JobEffect::Undo)]
    );
    state.panel_key(PanelKey::Down);
    assert_eq!(state.panel_key(PanelKey::Press), []);
    state.panel_key(PanelKey::Down);
    assert_eq!(
        state.panel_key(PanelKey::Press),
        [Effect::Job(JobEffect::Redo)]
    );
    // Closed again by the head.
    state.panel_key(PanelKey::Up);
    state.panel_key(PanelKey::Up);
    state.panel_key(PanelKey::Up);
    assert_eq!(state.panel_cursor(), Some((0, 0)));
    state.panel_key(PanelKey::Press);
    assert!(!state.history_open());
    assert_eq!(state.panel_lines().len(), 1);
}

#[test]
fn history_lines_follow_a_running_job_row() {
    let mut state = listing(&["a.txt", "b.txt"]);
    panel::running(&mut state, 1, "x");
    state.set_history(history(&[renamed("a.txt", "b.txt")], &[], t0()));
    state.history_toggled();
    assert_eq!(
        state.panel_lines(),
        [
            vec![PanelButton::PauseOrResume(0), PanelButton::Cancel(0)],
            vec![PanelButton::ToggleHistory],
            vec![PanelButton::RunHistory {
                redo: false,
                index: 0
            }],
        ]
    );
}

#[test]
fn the_status_line_borrows_the_room_of_the_deepest_known_drive() {
    use crate::places::Place;
    use rf_platform::DiskSpace;

    let mut state = listing(&[]);
    let (outer, inner) = (folder("a"), in_a("mount"));
    let drive = |label: &str, path: &VPath, available: u64| Place {
        label: label.to_owned(),
        path: path.clone(),
        kind: PlaceKind::Drive,
        space: Some(DiskSpace {
            total: 10 * available,
            free: available,
            available,
        }),
    };
    state.set_places(&[drive("바깥", &outer, 5), drive("안쪽", &inner, 7)]);
    // The drive that holds the folder most closely tells.
    assert_eq!(
        state.drive_free_of(&in_a("mount").join("x").expect("a name")),
        Some(7)
    );
    assert_eq!(state.drive_free_of(&in_a("other")), Some(5));
    // A folder on none of them, or a drive whose room is not read yet: nothing.
    assert_eq!(state.drive_free_of(&folder("elsewhere")), None);
    let unread = Place {
        space: None,
        ..drive("읽기 전", &outer, 5)
    };
    state.set_places(&[unread]);
    assert_eq!(state.drive_free_of(&in_a("other")), None);
}
