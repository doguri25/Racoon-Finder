//! How far a running job has got: what is done, how fast it goes and how long is left.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rf_core::VPath;

use crate::report::Totals;

/// The least time between two progress reports of one job.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// The span the transfer speed is averaged over.
pub const SPEED_WINDOW: Duration = Duration::from_secs(3);

/// A snapshot of a running job, sent to the UI at most every [`PROGRESS_INTERVAL`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// What has been processed so far.
    pub done: Totals,
    /// What the plan expects to process in all.
    pub total: Totals,
    /// The item being worked on, if there is one.
    pub current: Option<VPath>,
    /// The speed over the last [`SPEED_WINDOW`]; 0 while it is not known.
    pub bytes_per_sec: u64,
    /// The time left at that speed; `None` while the speed is not known.
    pub eta: Option<Duration>,
}

/// Measures the transfer speed from the byte counts a job reports, averaged over a sliding
/// window so a short stall or burst does not make the figure jump.
#[derive(Clone, Debug)]
pub struct SpeedMeter {
    window: Duration,
    /// The samples still in the window, oldest first, with the total bytes done at each.
    samples: VecDeque<(Instant, u64)>,
}

impl SpeedMeter {
    pub fn new(window: Duration) -> SpeedMeter {
        SpeedMeter {
            window,
            samples: VecDeque::new(),
        }
    }

    /// Adds the sample "`bytes_done` bytes in all at `now`" and drops the samples older than
    /// `now - window`. The newest two samples always stay, so a pause longer than the window
    /// still leaves two points to measure between.
    ///
    /// A count lower than the last one (bytes taken back after a failed attempt) starts the
    /// window again from this sample, so the speed is measured from there on at once instead
    /// of reading 0 until the higher counts have left the window.
    pub fn record(&mut self, now: Instant, bytes_done: u64) {
        if self
            .samples
            .back()
            .is_some_and(|&(_, last)| bytes_done < last)
        {
            self.samples.clear();
        }
        self.samples.push_back((now, bytes_done));
        // A window longer than the time since the clock started has no start: keep everything.
        let Some(oldest_kept) = now.checked_sub(self.window) else {
            return;
        };
        while self.samples.len() > 2
            && self
                .samples
                .front()
                .is_some_and(|&(taken, _)| taken < oldest_kept)
        {
            self.samples.pop_front();
        }
    }

    /// The bytes per second between the oldest and the newest sample in the window, rounded
    /// down. 0 with fewer than two samples or when they were taken at the same millisecond.
    pub fn bytes_per_sec(&self) -> u64 {
        let (Some(&(first_at, first_bytes)), Some(&(last_at, last_bytes))) =
            (self.samples.front(), self.samples.back())
        else {
            return 0;
        };
        let millis = last_at.saturating_duration_since(first_at).as_millis();
        if millis == 0 {
            return 0;
        }
        let bytes = u128::from(last_bytes.saturating_sub(first_bytes));
        u64::try_from(bytes * 1_000 / millis).unwrap_or(u64::MAX)
    }

    /// The time `remaining` bytes take at the current speed, to the millisecond: zero when
    /// nothing remains, `None` when the speed is 0. A time longer than `u64::MAX` milliseconds
    /// is cut there.
    pub fn eta(&self, remaining: u64) -> Option<Duration> {
        if remaining == 0 {
            return Some(Duration::ZERO);
        }
        let speed = self.bytes_per_sec();
        if speed == 0 {
            return None;
        }
        let millis = u128::from(remaining) * 1_000 / u128::from(speed);
        Some(Duration::from_millis(
            u64::try_from(millis).unwrap_or(u64::MAX),
        ))
    }
}
