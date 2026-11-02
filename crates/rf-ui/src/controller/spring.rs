//! 머무르면 열기 (DND-08): the place a drag rests on opens once the drag has stayed there for
//! [`SPRING_DELAY`]. Each hover over a place that takes the drag tells where it is
//! ([`Spring::hover`]); a new place, or the same one holding another folder now (the rows of a
//! folder came under a still pointer, a list scrolled under it), counts the time from the start,
//! and the time runs on while the drag stays. A place that takes nothing stops it, and so does
//! the end of the drag ([`Spring::stop`]: its drop, `drag-finished`, the left button let go,
//! the pointer gone from the window, and later the leave and the drop of a drag of another app).
//! What a rest opens is the state's ([`AppState::drag_rests`](crate::state::AppState::drag_rests)).

use std::cell::RefCell;
use std::rc::Rc;

use rf_core::VPath;
use slint::{Timer, TimerMode};

use crate::state::{DropSpot, SPRING_DELAY};

/// A place a drag hovers that takes its items, and the folder they would go into there.
pub(super) type Rest = (DropSpot, VPath);

/// The one timer of the rests of the drags over the window.
pub(super) struct Spring {
    timer: Timer,
    /// Where the drag rests since the time started; it stays after the time is up, so that a
    /// place opens once for each stay.
    at: RefCell<Option<Rest>>,
    /// What the time of a rest calls, with the place.
    due: Rc<dyn Fn(DropSpot)>,
}

impl Spring {
    /// A timer that calls `due` with the place a drag rested on for [`SPRING_DELAY`].
    pub(super) fn new(due: impl Fn(DropSpot) + 'static) -> Spring {
        Spring {
            timer: Timer::default(),
            at: RefCell::new(None),
            due: Rc::new(due),
        }
    }

    /// The drag hovers `place`, a place that takes it, or a place that takes nothing (`None`):
    /// another place starts the time again, the same one lets it run (or, once it is up, opens
    /// nothing more), and no place stops it.
    pub(super) fn hover(&self, place: Option<Rest>) {
        if *self.at.borrow() == place {
            return;
        }
        match &place {
            Some((spot, _)) => {
                let (spot, due) = (*spot, Rc::clone(&self.due));
                self.timer
                    .start(TimerMode::SingleShot, SPRING_DELAY, move || due(spot));
            }
            None => self.timer.stop(),
        }
        *self.at.borrow_mut() = place;
    }

    /// The drag is over, or gone from the window: nothing it rested on opens.
    pub(super) fn stop(&self) {
        self.hover(None);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::state::RIGHT;

    fn tick(ms: u64) {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(ms));
    }

    fn row(row: usize) -> DropSpot {
        DropSpot::Pane {
            side: RIGHT,
            row: Some(row),
        }
    }

    /// Row `index` of the right pane, which holds the folder `name`.
    fn place(index: usize, name: &str) -> Rest {
        let folder = VPath::local(std::env::temp_dir().join(name)).expect("a local path");
        (row(index), folder)
    }

    /// A spring on the testing backend's clock, and the places it opened.
    fn spring() -> (Spring, Rc<RefCell<Vec<DropSpot>>>) {
        i_slint_backend_testing::init_no_event_loop();
        let opened = Rc::new(RefCell::new(Vec::new()));
        let noted = Rc::clone(&opened);
        let spring = Spring::new(move |spot| noted.borrow_mut().push(spot));
        (spring, opened)
    }

    #[test]
    fn a_drag_that_stays_on_a_place_a_second_opens_it_once() {
        let (spring, opened) = spring();
        assert_eq!(SPRING_DELAY, Duration::from_millis(1000));
        spring.hover(Some(place(0, "백업")));
        tick(600);
        // The pointer moving on the same place lets the time run.
        spring.hover(Some(place(0, "백업")));
        tick(399);
        assert_eq!(*opened.borrow(), []);
        tick(1);
        assert_eq!(*opened.borrow(), [row(0)]);
        // Staying there opens it no more.
        spring.hover(Some(place(0, "백업")));
        tick(3000);
        assert_eq!(*opened.borrow(), [row(0)]);
    }

    #[test]
    fn a_move_to_another_place_counts_the_time_from_the_start() {
        let (spring, opened) = spring();
        spring.hover(Some(place(0, "백업")));
        tick(600);
        spring.hover(Some(place(1, "old")));
        tick(600);
        assert_eq!(*opened.borrow(), []);
        tick(400);
        assert_eq!(*opened.borrow(), [row(1)]);
        // The same row holding another folder (rows came under a still pointer) is another
        // place.
        spring.hover(Some(place(1, "new")));
        tick(999);
        assert_eq!(*opened.borrow(), [row(1)]);
        tick(1);
        assert_eq!(*opened.borrow(), [row(1), row(1)]);
    }

    #[test]
    fn the_time_stops_where_nothing_takes_the_drag_and_as_it_ends() {
        let (spring, opened) = spring();
        spring.hover(Some(place(0, "백업")));
        tick(600);
        spring.hover(None);
        tick(3000);
        assert_eq!(*opened.borrow(), []);
        // Back on the place, the time starts again; the drag ends before it is up.
        spring.hover(Some(place(0, "백업")));
        tick(600);
        spring.stop();
        tick(3000);
        assert_eq!(*opened.borrow(), []);
        // The next drag's rest there counts from the start.
        spring.hover(Some(place(0, "백업")));
        tick(999);
        assert_eq!(*opened.borrow(), []);
        tick(1);
        assert_eq!(*opened.borrow(), [row(0)]);
    }
}
