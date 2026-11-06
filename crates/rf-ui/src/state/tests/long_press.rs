//! The long press's own machine ([`LongPress`]): a press held still opens a menu at its time, and
//! is a click or a drag in every other case (PICK-02).

use std::time::{Duration, Instant};

use crate::state::long_press::{Cancel, LongPress, Open, Outcome, RING_AFTER, Spot, THRESHOLD_PX};

const DELAY: Duration = Duration::from_millis(500);

fn machine() -> LongPress {
    LongPress::new(DELAY, THRESHOLD_PX)
}

fn at(x: f32, y: f32) -> Spot {
    Spot::new(x, y)
}

fn after(start: Instant, ms: u64) -> Instant {
    start + Duration::from_millis(ms)
}

#[test]
fn a_release_before_the_delay_is_an_ordinary_click() {
    let (mut press, start) = (machine(), Instant::now());
    press.press(at(100.0, 80.0), start);
    assert!(press.is_armed());
    assert_eq!(press.tick(after(start, 300)), None);
    assert_eq!(press.release(after(start, 300)), Outcome::Click);
    assert!(!press.is_armed());
    // Nothing opens afterwards either.
    assert_eq!(press.tick(after(start, 900)), None);
}

#[test]
fn a_move_past_the_threshold_before_the_delay_makes_it_a_drag_not_a_menu() {
    let (mut press, start) = (machine(), Instant::now());
    press.press(at(100.0, 80.0), start);
    // Within 8 px (here 5 and 5, 7.07 apart) it stays armed.
    assert_eq!(press.moved(at(105.0, 85.0), after(start, 100)), None);
    assert!(press.is_armed());
    // Past it, 9 px sideways: cancelled.
    assert_eq!(
        press.moved(at(109.0, 80.0), after(start, 200)),
        Some(Cancel)
    );
    assert!(!press.is_armed());
    // The time passing opens no menu, and the release is the drag's.
    assert_eq!(press.tick(after(start, 700)), None);
    assert_eq!(press.release(after(start, 800)), Outcome::Click);
}

#[test]
fn the_threshold_is_the_distance_from_where_it_was_pressed_not_from_the_last_move() {
    let (mut press, start) = (machine(), Instant::now());
    press.press(at(0.0, 0.0), start);
    // Each step is small, the sum is not.
    for step in 1..=3 {
        let spot = at(f32::from(u8::try_from(step).expect("small")) * 3.0, 0.0);
        let cancelled = press.moved(spot, after(start, 50 * step)).is_some();
        assert_eq!(cancelled, step == 3, "step {step}");
    }
}

#[test]
fn it_opens_once_when_the_delay_has_passed() {
    let (mut press, start) = (machine(), Instant::now());
    press.press(at(40.0, 60.0), start);
    assert_eq!(press.tick(after(start, 499)), None);
    assert_eq!(
        press.tick(after(start, 500)),
        Some(Open { at: at(40.0, 60.0) })
    );
    // Once: the next looks find nothing, and it no longer waits.
    assert_eq!(press.tick(after(start, 530)), None);
    assert!(!press.is_armed());
}

#[test]
fn a_small_wobble_does_not_stop_it_opening() {
    let (mut press, start) = (machine(), Instant::now());
    press.press(at(40.0, 60.0), start);
    assert_eq!(press.moved(at(43.0, 62.0), after(start, 200)), None);
    assert_eq!(press.moved(at(38.0, 57.0), after(start, 400)), None);
    let open = press.tick(after(start, 520)).expect("it opens");
    // Where it was pressed, not where it wobbled to.
    assert_eq!(open.at, at(40.0, 60.0));
}

#[test]
fn the_release_after_the_menu_opened_is_not_a_click() {
    let (mut press, start) = (machine(), Instant::now());
    press.press(at(40.0, 60.0), start);
    assert!(press.tick(after(start, 510)).is_some());
    // A move after the menu opened cancels nothing and changes nothing.
    assert_eq!(press.moved(at(300.0, 300.0), after(start, 600)), None);
    assert_eq!(press.release(after(start, 700)), Outcome::Opened);
    // The next press is a press again.
    press.press(at(1.0, 1.0), after(start, 800));
    assert_eq!(press.release(after(start, 850)), Outcome::Click);
}

#[test]
fn a_reset_forgets_a_menu_whose_release_never_came() {
    let (mut press, start) = (machine(), Instant::now());
    press.press(at(40.0, 60.0), start);
    assert!(press.tick(after(start, 510)).is_some());
    // An OS menu's loop took the release: the next press, on the toolbar, never armed the
    // machine. Reset first, and its release is a click.
    press.reset();
    assert!(!press.is_armed());
    assert_eq!(press.release(after(start, 900)), Outcome::Click);
    // A reset also ends a press that waits.
    press.press(at(1.0, 1.0), after(start, 1_000));
    press.reset();
    assert_eq!(press.tick(after(start, 2_000)), None);
}

#[test]
fn a_second_button_cancels_a_press_that_waits_but_not_one_that_opened() {
    let (mut press, start) = (machine(), Instant::now());
    press.press(at(40.0, 60.0), start);
    press.cancel();
    assert!(!press.is_armed());
    assert_eq!(press.tick(after(start, 600)), None);
    assert_eq!(press.release(after(start, 650)), Outcome::Click);

    // After the menu opened, a cancel leaves the end of the press as it is.
    press.press(at(40.0, 60.0), after(start, 700));
    assert!(press.tick(after(start, 1_300)).is_some());
    press.cancel();
    assert_eq!(press.release(after(start, 1_400)), Outcome::Opened);
}

#[test]
fn a_release_with_no_press_is_a_click_and_a_new_press_starts_afresh() {
    let (mut press, start) = (machine(), Instant::now());
    assert_eq!(press.release(start), Outcome::Click);
    assert_eq!(press.tick(start), None);
    assert_eq!(press.moved(at(1.0, 1.0), start), None);
    press.press(at(0.0, 0.0), start);
    // A press while one waits starts the time again.
    press.press(at(5.0, 5.0), after(start, 400));
    assert_eq!(press.tick(after(start, 600)), None);
    assert_eq!(
        press.tick(after(start, 900)),
        Some(Open { at: at(5.0, 5.0) })
    );
}

#[test]
fn the_ring_shows_after_a_moment_and_fills_to_one_at_the_delay() {
    let (mut press, start) = (machine(), Instant::now());
    assert_eq!(press.progress(start), None);
    press.press(at(0.0, 0.0), start);
    // A click shows no ring.
    assert_eq!(press.progress(after(start, 0)), None);
    assert_eq!(
        press.progress(
            (start + RING_AFTER)
                .checked_sub(Duration::from_millis(1))
                .expect("after the start")
        ),
        None
    );
    let third = press.progress(after(start, 250)).expect("a ring");
    assert!((third - 0.5).abs() < 1e-4, "{third}");
    let all = press.progress(after(start, 5_000)).expect("a ring");
    assert!((all - 1.0).abs() < f32::EPSILON, "{all}");
    // None once it opened, was cancelled or ended.
    assert!(press.tick(after(start, 500)).is_some());
    assert_eq!(press.progress(after(start, 510)), None);
}

#[test]
fn the_delay_of_the_next_press_can_be_changed() {
    let (mut press, start) = (machine(), Instant::now());
    press.set_delay(Duration::from_millis(1_200));
    assert_eq!(press.delay(), Duration::from_millis(1_200));
    press.press(at(0.0, 0.0), start);
    assert_eq!(press.tick(after(start, 1_199)), None);
    assert!(press.tick(after(start, 1_200)).is_some());
}
