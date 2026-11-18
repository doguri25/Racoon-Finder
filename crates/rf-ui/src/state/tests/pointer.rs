//! The mouse's fourth and fifth buttons, a trackpad's swipe (NAV-04, COMP-04) and the checkbox
//! of a row (NAV-15), from the state's side; the window's events that call them are in the
//! controller.

use super::*;
use crate::settings::{LONG_PRESS_DEFAULT_MS, LONG_PRESS_MAX_MS, LONG_PRESS_MIN_MS};
use crate::state::{SWIPE_DISTANCE, SelectState};

/// The left pane with history: it showed `a`, then `sub` of it, then `b`.
fn travelled() -> AppState {
    let mut state = ready(vec![dir("sub")], vec![file("z")]);
    state.open_row(LEFT, 0);
    state.go_to(folder("b"));
    state
}

fn shown(state: &AppState, side: usize) -> VPath {
    state.panes()[side].active_tab().location().clone()
}

#[test]
fn the_fourth_button_goes_back_and_the_fifth_forward_in_the_active_pane() {
    let mut state = travelled();
    let sub = folder("a").join("sub").expect("a name");
    assert_eq!(the_load(&state.mouse_back()).path, sub);
    assert_eq!(the_load(&state.mouse_back()).path, folder("a"));
    assert_eq!(the_load(&state.mouse_forward()).path, sub);
    assert_eq!(the_load(&state.mouse_forward()).path, folder("b"));
    // The other pane did not move.
    assert_eq!(shown(&state, RIGHT), folder("b"));
}

#[test]
fn the_buttons_follow_the_pane_that_has_the_keyboard() {
    let mut state = ready(vec![file("x")], vec![dir("deep")]);
    state.open_row(RIGHT, 0);
    state.click_row(RIGHT, 0, false, false);
    assert_eq!(state.active(), RIGHT);
    let effects = state.mouse_back();
    assert_eq!(the_load(&effects).path, folder("b"));
    assert_eq!(shown(&state, LEFT), folder("a"));
}

#[test]
fn without_history_a_button_does_nothing() {
    let mut state = ready(vec![file("x")], vec![]);
    assert_eq!(state.mouse_back(), []);
    assert_eq!(state.mouse_forward(), []);
    // And at the end of the history, forward does nothing.
    let mut state = travelled();
    assert_eq!(state.mouse_forward(), []);
}

#[test]
fn a_dialog_a_text_field_a_drag_and_the_end_of_the_app_keep_the_buttons_from_acting() {
    // A window over the main one.
    let mut state = travelled();
    press(&mut state, "n", CTRL_SHIFT).expect("used");
    assert!(state.modal().is_some());
    assert_eq!(state.mouse_back(), []);
    assert_eq!(state.mouse_forward(), []);
    state.dialog_cancelled();
    assert!(state.modal().is_none());
    assert!(!state.mouse_back().is_empty(), "back again once it closed");

    // Quick look.
    let mut state = ready(vec![dir("sub")], vec![]);
    let effects = state.open_row(LEFT, 0);
    let generation = the_load(&effects).generation;
    state.loaded(LoadEvent::Listed {
        generation,
        entries: vec![file("x")],
    });
    state.click_row(LEFT, 0, false, false);
    press(&mut state, " ", NONE).expect("Space is quick look");
    assert!(state.quick_look_open());
    assert_eq!(state.mouse_back(), []);
    state.quick_look_key(keys::QuickLookKey::Close);
    assert!(!state.quick_look_open());

    // A text field that has the keys.
    let mut state = travelled();
    state.set_pointer_context(true, false);
    assert_eq!(state.mouse_back(), []);
    state.set_pointer_context(false, false);
    assert_ne!(state.mouse_back().len(), 0);

    // A drag under way.
    let mut state = travelled();
    state.set_pointer_context(false, true);
    assert_eq!(state.mouse_back(), []);
    assert_eq!(state.swipe(200.0), []);
    state.set_pointer_context(false, false);
    assert_ne!(state.mouse_back().len(), 0);

    // The app ends.
    let mut state = travelled();
    state.begin_quit();
    assert_eq!(state.mouse_back(), []);
}

#[test]
fn a_swipe_adds_up_and_moves_once_at_the_distance() {
    let mut state = travelled();
    // To the right, in steps: nothing until the sum reaches the distance, then Back, once.
    let mut moved = 0;
    for _ in 0..10 {
        let effects = state.swipe(SWIPE_DISTANCE / 4.0);
        if !effects.is_empty() {
            moved += 1;
        }
    }
    assert_eq!(moved, 1, "one move for one gesture");
    assert_eq!(
        shown(&state, LEFT),
        folder("a").join("sub").expect("a name")
    );
}

#[test]
fn a_swipe_to_the_left_goes_forward_and_a_new_gesture_starts_from_nothing() {
    let mut state = travelled();
    let sub = folder("a").join("sub").expect("a name");
    assert_eq!(the_load(&state.swipe(SWIPE_DISTANCE)).path, sub);
    assert_eq!(
        state.swipe(500.0),
        [],
        "the same gesture goes on: nothing more"
    );
    state.swipe_end();
    // Short of the distance: nothing.
    assert_eq!(state.swipe(-(SWIPE_DISTANCE - 1.0)), []);
    state.swipe_end();
    // A new gesture to the left, the whole distance at once: Forward.
    assert_eq!(the_load(&state.swipe(-SWIPE_DISTANCE)).path, folder("b"));
}

#[test]
fn a_swipe_that_goes_both_ways_adds_up_both_ways() {
    let mut state = travelled();
    assert_eq!(state.swipe(70.0), []);
    assert_eq!(state.swipe(-60.0), []);
    // 10 net: far from the distance.
    assert_eq!(state.swipe(60.0), []);
    assert!(
        !state.swipe(30.0).is_empty(),
        "100 right of the start: back"
    );
}

#[test]
fn a_swipe_is_not_counted_while_nothing_may_move_and_ignores_odd_numbers() {
    let mut state = travelled();
    assert_eq!(state.swipe(f32::NAN), []);
    assert_eq!(state.swipe(f32::INFINITY), []);
    // A gesture while a dialog showed does not count towards the one after it.
    press(&mut state, "n", CTRL_SHIFT).expect("used");
    assert_eq!(state.swipe(500.0), []);
    state.dialog_cancelled();
    assert_eq!(state.swipe(SWIPE_DISTANCE - 1.0), []);
}

#[test]
fn the_checkbox_toggles_its_row_without_ctrl_and_leaves_the_rest() {
    let mut state = ready(
        vec![file("a"), file("b"), file("c"), file("d")],
        vec![file("z")],
    );
    // Clicks on checkboxes add and remove rows one at a time.
    state.click_checkbox(LEFT, 0);
    state.click_checkbox(LEFT, 2);
    assert_eq!(selected(&state, LEFT), [0, 2]);
    state.click_checkbox(LEFT, 0);
    assert_eq!(selected(&state, LEFT), [2]);
    state.click_checkbox(LEFT, 3);
    assert_eq!(selected(&state, LEFT), [2, 3]);
    // The pane that was clicked has the keyboard.
    state.click_checkbox(RIGHT, 0);
    assert_eq!(state.active(), RIGHT);
    assert_eq!(selected(&state, RIGHT), [0]);
    // The other pane's selection stays.
    assert_eq!(selected(&state, LEFT), [2, 3]);
}

#[test]
fn a_plain_click_on_the_row_still_selects_only_that_row() {
    let mut state = ready(vec![file("a"), file("b"), file("c")], vec![]);
    state.click_checkbox(LEFT, 0);
    state.click_checkbox(LEFT, 1);
    assert_eq!(selected(&state, LEFT), [0, 1]);
    state.click_row(LEFT, 2, false, false);
    assert_eq!(selected(&state, LEFT), [2]);
}

#[test]
fn the_head_s_checkbox_selects_every_row_or_none() {
    let mut state = ready(vec![file("a"), file("b"), file("c")], vec![]);
    assert_eq!(state.selection_state(LEFT), SelectState::None);
    state.toggle_all(LEFT);
    assert_eq!(selected(&state, LEFT), [0, 1, 2]);
    assert_eq!(state.selection_state(LEFT), SelectState::All);
    state.toggle_all(LEFT);
    assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
    // With part selected, it selects all (as File Explorer's does).
    state.click_checkbox(LEFT, 1);
    assert_eq!(state.selection_state(LEFT), SelectState::Some);
    state.toggle_all(LEFT);
    assert_eq!(state.selection_state(LEFT), SelectState::All);
    // Unchecking one row of all leaves part.
    state.click_checkbox(LEFT, 1);
    assert_eq!(state.selection_state(LEFT), SelectState::Some);
}

#[test]
fn no_rows_make_no_head_state_of_all() {
    let mut state = ready(vec![], vec![]);
    assert_eq!(state.selection_state(LEFT), SelectState::None);
    state.toggle_all(LEFT);
    assert_eq!(state.selection_state(LEFT), SelectState::None);
}

#[test]
fn the_keys_do_what_they_always_do_with_the_checkboxes_on() {
    let mut state = ready(vec![file("a"), file("b"), file("c")], vec![]);
    state.toggle_checkbox_mode();
    assert!(state.checkbox_select());
    press(&mut state, "a", CTRL).expect("used");
    assert_eq!(selected(&state, LEFT), [0, 1, 2]);
    press(&mut state, &special(Key::Escape), NONE).expect("used");
    assert_eq!(selected(&state, LEFT), Vec::<usize>::new());
    press(&mut state, &special(Key::DownArrow), NONE).expect("used");
    assert_eq!(selected(&state, LEFT), [1]);
    state.toggle_checkbox_mode();
    assert!(!state.checkbox_select());
    // The mode changes no selection.
    assert_eq!(selected(&state, LEFT), [1]);
}

#[test]
fn the_long_press_settings_start_on_at_500_ms_and_stay_in_range() {
    let mut state = ready(vec![], vec![]);
    assert!(state.long_press());
    assert_eq!(state.long_press_ms(), LONG_PRESS_DEFAULT_MS);
    assert!(!state.checkbox_select());
    state.set_long_press(false);
    assert!(!state.long_press());
    state.set_long_press_ms(800);
    assert_eq!(state.long_press_ms(), 800);
    state.set_long_press_ms(1);
    assert_eq!(state.long_press_ms(), LONG_PRESS_MIN_MS);
    state.set_long_press_ms(u16::MAX);
    assert_eq!(state.long_press_ms(), LONG_PRESS_MAX_MS);
}

mod slow_click {
    //! The slow second click (OPS-02): the pure machine, with the times it is given.

    use std::time::Duration;

    use super::*;
    use crate::state::long_press::Spot;
    use crate::state::{DOUBLE_CLICK, RowPress, SlowClick, SlowTarget};

    const MS: Duration = Duration::from_millis(1);

    fn target() -> SlowTarget {
        SlowTarget {
            side: LEFT,
            name: "설계안.pdf".into(),
        }
    }

    /// A press on the name of the item that is the only selection, with no modifier key.
    fn on_name() -> RowPress {
        RowPress {
            target: target(),
            on_name: true,
            modified: false,
            sole: true,
        }
    }

    fn here() -> Spot {
        Spot::new(100.0, 40.0)
    }

    /// A machine whose last press came at `start`, long enough before `start + 2 s`.
    fn machine() -> (SlowClick, Instant) {
        let mut slow = SlowClick::new(DOUBLE_CLICK, 8.0);
        let start = Instant::now();
        // The click that selected the item, a second before the presses of the tests.
        slow.press(
            RowPress {
                sole: false,
                ..on_name()
            },
            here(),
            start,
        );
        assert_eq!(slow.release(start + 50 * MS), None);
        (slow, start + Duration::from_secs(1))
    }

    #[test]
    fn a_click_on_the_name_of_the_only_selection_renames_once_no_second_click_came() {
        let (mut slow, now) = machine();
        slow.press(on_name(), here(), now);
        // Let go before the double-click time: the rename waits for it, in case a second
        // click makes this a double click.
        assert_eq!(slow.release(now + 80 * MS), None);
        assert_eq!(slow.waiting_until(), Some(now + DOUBLE_CLICK));
        assert_eq!(slow.tick(now + DOUBLE_CLICK.saturating_sub(MS)), None);
        assert_eq!(slow.tick(now + DOUBLE_CLICK), Some(target()));
        // Once.
        assert_eq!(slow.tick(now + DOUBLE_CLICK + MS), None);
        assert_eq!(slow.waiting_until(), None);
        // Held past the double-click time, it renames as it is let go.
        let later = now + Duration::from_secs(2);
        slow.press(on_name(), here(), later);
        assert_eq!(slow.release(later + DOUBLE_CLICK + MS), Some(target()));
    }

    #[test]
    fn a_second_click_within_the_double_click_time_opens_and_renames_nothing() {
        let (mut slow, now) = machine();
        slow.press(on_name(), here(), now);
        assert_eq!(slow.release(now + 60 * MS), None);
        // The second click of a double click: the row opens (Slint's double click), and the
        // rename that waited is dropped.
        slow.press(on_name(), here(), now + 200 * MS);
        assert_eq!(slow.waiting_until(), None);
        assert_eq!(slow.release(now + 260 * MS), None);
        assert_eq!(slow.tick(now + Duration::from_secs(3)), None);
        // And a click right after the selecting one is a double click's second click too.
        let mut slow = SlowClick::new(DOUBLE_CLICK, 8.0);
        let start = Instant::now();
        slow.press(
            RowPress {
                sole: false,
                ..on_name()
            },
            here(),
            start,
        );
        slow.release(start + 50 * MS);
        slow.press(on_name(), here(), start + 300 * MS);
        assert_eq!(slow.release(start + 350 * MS), None);
        assert_eq!(slow.tick(start + Duration::from_secs(3)), None);
    }

    #[test]
    fn a_press_that_moves_past_the_threshold_is_a_drag() {
        let (mut slow, now) = machine();
        slow.press(on_name(), here(), now);
        // Within the threshold it is still a click.
        slow.moved(Spot::new(104.0, 43.0));
        assert_eq!(slow.release(now + DOUBLE_CLICK), Some(target()));
        let later = now + Duration::from_secs(2);
        slow.press(on_name(), here(), later);
        slow.moved(Spot::new(109.0, 40.0));
        // A drag ends with no click; were a release told all the same, it renames nothing.
        assert_eq!(slow.release(later + DOUBLE_CLICK), None);
        assert_eq!(slow.tick(later + Duration::from_secs(3)), None);
    }

    #[test]
    fn a_modifier_another_place_or_another_selection_only_selects() {
        for press in [
            // Ctrl or Shift (or Alt, the system key) changes the selection.
            RowPress {
                modified: true,
                ..on_name()
            },
            // The icon, or another column.
            RowPress {
                on_name: false,
                ..on_name()
            },
            // An item that was not the only selection: it becomes it.
            RowPress {
                sole: false,
                ..on_name()
            },
        ] {
            let (mut slow, now) = machine();
            slow.press(press.clone(), here(), now);
            assert_eq!(slow.release(now + 60 * MS), None, "{press:?}");
            assert_eq!(slow.tick(now + Duration::from_secs(3)), None, "{press:?}");
        }
    }

    #[test]
    fn a_key_the_wheel_or_a_menu_drops_the_rename_that_waits() {
        let (mut slow, now) = machine();
        slow.press(on_name(), here(), now);
        slow.release(now + 60 * MS);
        slow.cancel();
        assert_eq!(slow.waiting_until(), None);
        assert_eq!(slow.tick(now + Duration::from_secs(3)), None);
        // Cancelled while it is pressed, its click renames nothing.
        let later = now + Duration::from_secs(2);
        slow.press(on_name(), here(), later);
        slow.cancel();
        assert_eq!(slow.release(later + DOUBLE_CLICK), None);
        // The press's time stays: one right after the cancelled one is a double click's.
        slow.press(on_name(), here(), later + 100 * MS);
        assert_eq!(slow.release(later + DOUBLE_CLICK + MS), None);
    }

    #[test]
    fn the_state_says_what_a_press_is_on_and_renames_only_the_only_selection() {
        let mut state = ready(vec![file("a.txt"), dir("b")], vec![]);
        // Before the press selects it, the row is not the only selection.
        let press = state.row_press(LEFT, 1, true, false).expect("a row");
        assert!(!press.sole && press.on_name && !press.modified);
        assert_eq!(press.target.name, "b");
        state.click_row(LEFT, 1, false, false);
        let press = state.row_press(LEFT, 1, true, false).expect("a row");
        assert!(press.sole);
        assert_eq!(state.row_press(LEFT, 5, true, false), None);
        // A modifier the window heard of counts, whatever the press said.
        state.modifiers_changed(DragMods {
            alt: true,
            ..DragMods::default()
        });
        assert!(
            state
                .row_press(LEFT, 1, true, false)
                .expect("a row")
                .modified
        );
        state.modifiers_changed(DragMods::default());

        // Due, it opens the name window of the folder, its whole name selected.
        assert_eq!(state.slow_rename(&press.target), []);
        let Some(Modal::Name(dialog)) = state.modal() else {
            panic!("a rename: {:?}", state.modal());
        };
        assert_eq!(dialog.initial, "b");
        assert_eq!(dialog.select_end, 1);
        state.dialog_cancelled();
        // Not when the selection changed meanwhile, nor for an item that left the list.
        state.click_row(LEFT, 0, false, true);
        assert_eq!(state.slow_rename(&press.target), []);
        assert_eq!(state.modal(), None);
        let gone = SlowTarget {
            side: LEFT,
            name: "없음".into(),
        };
        state.click_row(LEFT, 1, false, false);
        assert_eq!(state.slow_rename(&gone), []);
        assert_eq!(state.modal(), None);
        // Nor over another window.
        state.new_file();
        state.slow_rename(&press.target);
        assert!(matches!(state.modal(), Some(Modal::Name(dialog)) if dialog.renames().is_none()));
    }
}

mod visits {
    //! The visit history's menu (NAV-04), from the state's side.

    use super::*;
    use crate::history::{HISTORY_MENU, Way};
    use crate::state::MenuShown;

    #[test]
    fn the_menu_lists_each_way_nearest_first_and_a_choice_jumps_there() {
        let mut state = travelled();
        let sub = folder("a").join("sub").expect("a name");
        // At b, after a and its sub: behind are sub, then a; nothing ahead.
        assert_eq!(state.history_entries(Way::Back), [sub.clone(), folder("a")]);
        assert_eq!(state.history_entries(Way::Forward), []);
        // The farther one, in one move: two steps back.
        let effects = state.history_chosen(Way::Back, 1);
        assert_eq!(the_load(&effects).path, folder("a"));
        assert_eq!(shown(&state, LEFT), folder("a"));
        // The folders passed over are ahead, nearest first, as a browser keeps them.
        assert_eq!(
            state.history_entries(Way::Forward),
            [sub.clone(), folder("b")]
        );
        assert_eq!(state.history_entries(Way::Back), []);
        let effects = state.history_chosen(Way::Forward, 1);
        assert_eq!(the_load(&effects).path, folder("b"));
        assert_eq!(state.history_entries(Way::Back), [sub, folder("a")]);
        // Past the end, or past what the menu lists, nothing moves.
        assert_eq!(state.history_chosen(Way::Back, 2), []);
        assert_eq!(state.history_chosen(Way::Back, HISTORY_MENU), []);
        assert_eq!(shown(&state, LEFT), folder("b"));
        // Not over a window either.
        state.new_file();
        assert_eq!(state.history_chosen(Way::Back, 0), []);
    }

    #[test]
    fn the_menu_lists_at_most_fifteen_folders() {
        let mut state = ready(vec![], vec![]);
        for i in 0..20 {
            state.go_to(folder(&format!("f{i}")));
        }
        let back = state.history_entries(Way::Back);
        assert_eq!(back.len(), HISTORY_MENU);
        assert_eq!(back[0], folder("f18"));
        assert_eq!(back[HISTORY_MENU - 1], folder("f4"));
    }

    #[test]
    fn its_key_shows_it_while_there_is_somewhere_to_go() {
        let mut state = ready(vec![], vec![]);
        assert_eq!(
            state.key(Command::HistoryMenu, NONE, Paging::rows(1), Instant::now()),
            Some(vec![])
        );
        let mut state = travelled();
        assert_eq!(
            state.key(Command::HistoryMenu, NONE, Paging::rows(1), Instant::now()),
            Some(vec![Effect::ShowMenu(MenuShown::History)])
        );
        // With the pane at the start of its history, ahead is enough.
        state.history_chosen(Way::Back, 1);
        assert_eq!(
            state.key(Command::HistoryMenu, NONE, Paging::rows(1), Instant::now()),
            Some(vec![Effect::ShowMenu(MenuShown::History)])
        );
    }

    #[test]
    fn with_sync_on_the_other_pane_makes_the_same_jump() {
        // Both panes went two folders on from where they started.
        let mut state = ready(vec![], vec![]);
        state.go_to(folder("c"));
        state.go_to(folder("d"));
        state.activate(RIGHT);
        state.go_to(folder("e"));
        state.go_to(folder("f"));
        state.activate(LEFT);
        press(&mut state, &special(Key::F9), SHIFT).expect("sync");
        assert!(state.syncing());
        // Two places back in the left pane: the right one makes the same jump.
        let effects = state.history_chosen(Way::Back, 1);
        let paths: Vec<VPath> = loads(&effects)
            .iter()
            .map(|request| request.path.clone())
            .collect();
        assert_eq!(paths, [folder("a"), folder("b")]);
        assert!(state.syncing());
        // One the other pane's history is too short for turns sync off, with a notice.
        state.activate(RIGHT);
        state.go_to(folder("g"));
        state.activate(LEFT);
        state.history_chosen(Way::Forward, 1);
        assert_eq!(shown(&state, LEFT), folder("d"));
        assert!(!state.syncing());
    }
}
