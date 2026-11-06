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
