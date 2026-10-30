//! The worker that puts the rows of a long listing in a new order ([`Effect::Sort`]): a
//! [`Newest`] worker with a slot for each tab, which keeps only the newest sort a tab asked for.
//! Clicking a head again and again while a large folder sorts queues no sorts nobody waits for:
//! a sort that has not started gives its place to the tab's next one, and the one under way is
//! dropped when it ends (the tab would drop its rows anyway, `Tab::apply_sort`). A sort that
//! panics, or that no thread can be started for, comes back failed, for the tab to stop waiting
//! (`AppState::sort_failed`).
//!
//! [`Effect::Sort`]: crate::state::Effect::Sort

use std::time::Duration;

use super::newest::{Job, Newest, Outcome};
use crate::pane::{SortRequest, Sorted};

impl Job for SortRequest {
    /// The generation of the tab's listing and the number of the sort.
    type Ticket = (u64, u64);

    fn slot(&self) -> u64 {
        self.tab
    }

    fn ticket(&self) -> (u64, u64) {
        (self.generation, self.serial)
    }
}

/// The worker of the sorts.
pub type Sorter = Newest<SortRequest, Sorted>;

/// A sorter that gives what becomes of each sort to `deliver`.
pub fn sorter(deliver: impl Fn(Outcome<(u64, u64), Sorted>) + Send + Sync + 'static) -> Sorter {
    Newest::new(
        "rf-ui-sort",
        Duration::ZERO,
        |request: SortRequest, _| Some(request.run()),
        deliver,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::mpsc;

    use rf_vfs::SortSpec;

    use super::*;

    fn request(tab: u64, serial: u64) -> SortRequest {
        SortRequest {
            tab,
            generation: 1,
            serial,
            sort: SortSpec::default(),
            entries: Vec::new(),
        }
    }

    #[test]
    fn a_tab_s_newer_sort_takes_the_place_of_the_one_that_waits() {
        // The worker hands each sort over and then waits for `go` before the next one: the
        // sorts asked for meanwhile wait.
        let (done, sorted) = mpsc::channel::<u64>();
        let (go, wait) = mpsc::channel::<()>();
        let wait = Mutex::new(wait);
        let sorter = sorter(move |outcome| {
            let Outcome::Done(rows) = outcome else {
                panic!("a sort that failed");
            };
            done.send(rows.serial).expect("the test listens");
            let _ = wait
                .lock()
                .expect("one worker")
                .recv_timeout(Duration::from_secs(10));
        });
        let next = || sorted.recv_timeout(Duration::from_secs(10)).ok();

        sorter.submit(request(1, 10)).expect("a worker");
        assert_eq!(next(), Some(10));
        // While the worker hands over tab 1's first sort: two more for it, one for tab 2.
        sorter.submit(request(1, 11)).expect("a worker");
        sorter.submit(request(2, 20)).expect("a worker");
        sorter.submit(request(1, 12)).expect("a worker");
        for _ in 0..3 {
            go.send(()).expect("the worker listens");
        }
        // Tab 1's newest sort, in the place of the one it replaced, then tab 2's; 11 never ran.
        assert_eq!(next(), Some(12));
        assert_eq!(next(), Some(20));
        assert_eq!(sorted.recv_timeout(Duration::from_millis(100)).ok(), None);
    }
}
