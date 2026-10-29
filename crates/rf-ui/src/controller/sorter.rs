//! The worker that puts the rows of a long listing in a new order ([`Effect::Sort`]): one
//! thread for every tab, which keeps only the newest sort a tab asked for. Clicking a head again
//! and again while a large folder sorts queues no sorts nobody waits for: a sort that has not
//! started gives its place to the tab's next one (the one under way runs to its end, and the tab
//! drops its rows, `Tab::apply_sort`).
//!
//! [`Effect::Sort`]: crate::state::Effect::Sort

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use crate::pane::{SortRequest, Sorted};

/// Hands the sorts to the worker, which gives each sorted listing to `deliver` on its own
/// thread. The worker starts with the first sort and ends when the sorter goes.
pub struct Sorter {
    shared: Arc<Shared>,
    deliver: Arc<dyn Fn(Sorted) + Send + Sync>,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

#[derive(Default)]
struct Queue {
    /// The sorts that wait, the oldest first, at most one for each tab.
    waiting: VecDeque<SortRequest>,
    /// The worker runs.
    started: bool,
    /// The sorter is gone: the worker ends.
    closed: bool,
}

/// The worker could not be started (the system has no thread to give).
#[derive(Debug)]
pub struct NoWorker;

impl Sorter {
    pub fn new(deliver: impl Fn(Sorted) + Send + Sync + 'static) -> Sorter {
        Sorter {
            shared: Arc::new(Shared {
                queue: Mutex::new(Queue::default()),
                wake: Condvar::new(),
            }),
            deliver: Arc::new(deliver),
        }
    }

    /// Queues `request`, in the place of the tab's sort that waits, if one does.
    pub fn sort(&self, request: SortRequest) -> Result<(), NoWorker> {
        let mut queue = self.lock();
        match queue
            .waiting
            .iter_mut()
            .find(|known| known.tab == request.tab)
        {
            Some(known) => *known = request,
            None => queue.waiting.push_back(request),
        }
        if !queue.started {
            let shared = Arc::clone(&self.shared);
            let deliver = Arc::clone(&self.deliver);
            let spawned = std::thread::Builder::new()
                .name("rf-ui-sort".to_owned())
                .spawn(move || work(&shared, &*deliver));
            if spawned.is_err() {
                queue.waiting.clear();
                return Err(NoWorker);
            }
            queue.started = true;
        }
        drop(queue);
        self.shared.wake.notify_one();
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Sorter {
    fn drop(&mut self) {
        let mut queue = self.lock();
        queue.closed = true;
        queue.waiting.clear();
        drop(queue);
        self.shared.wake.notify_one();
    }
}

/// The worker: the sorts one after another, the oldest first, until the sorter goes.
fn work(shared: &Shared, deliver: &(dyn Fn(Sorted) + Send + Sync)) {
    loop {
        let request = {
            let mut queue = shared.queue.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if queue.closed {
                    return;
                }
                if let Some(request) = queue.waiting.pop_front() {
                    break request;
                }
                queue = shared
                    .wake
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        deliver(request.run());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

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
        let sorter = Sorter::new(move |rows| {
            done.send(rows.serial).expect("the test listens");
            let _ = wait
                .lock()
                .expect("one worker")
                .recv_timeout(Duration::from_secs(10));
        });
        let next = || sorted.recv_timeout(Duration::from_secs(10)).ok();

        sorter.sort(request(1, 10)).expect("a worker");
        assert_eq!(next(), Some(10));
        // While the worker hands over tab 1's first sort: two more for it, one for tab 2.
        sorter.sort(request(1, 11)).expect("a worker");
        sorter.sort(request(2, 20)).expect("a worker");
        sorter.sort(request(1, 12)).expect("a worker");
        for _ in 0..3 {
            go.send(()).expect("the worker listens");
        }
        // Tab 1's newest sort, in the place of the one it replaced, then tab 2's; 11 never ran.
        assert_eq!(next(), Some(12));
        assert_eq!(next(), Some(20));
        assert_eq!(sorted.recv_timeout(Duration::from_millis(100)).ok(), None);
    }
}
