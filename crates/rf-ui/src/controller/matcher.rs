//! The worker that matches the names of the pattern window's rows ([`Effect::MatchNames`]): a
//! [`Newest`] worker with one slot. A request that comes while another one runs stops that one
//! (it looks at its token every thousand names or so) and takes the place of one that waits, so
//! a pattern typed key by key over 100,000 names matches only the text as it is. A match that
//! panics, or that no thread can be started for, comes back failed
//! (`AppState::pattern_failed`).
//!
//! [`Effect::MatchNames`]: crate::state::Effect::MatchNames

use std::time::Duration;

use super::newest::{Job, Newest, Outcome};
use crate::state::{MatchRequest, Matched};

impl Job for MatchRequest {
    /// The generation of the request.
    type Ticket = u64;

    fn slot(&self) -> u64 {
        0
    }

    fn ticket(&self) -> u64 {
        self.generation
    }
}

/// The worker of the pattern window.
pub type Matcher = Newest<MatchRequest, Matched>;

/// A matcher that gives what becomes of each request to `deliver`.
pub fn matcher(deliver: impl Fn(Outcome<u64, Matched>) + Send + Sync + 'static) -> Matcher {
    Newest::new(
        "rf-ui-match",
        Duration::ZERO,
        |request: MatchRequest, token| request.run(&|| !token.is_cancelled()),
        deliver,
    )
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::pattern::{NamePattern, PatternSyntax};

    fn request(generation: u64, rows: usize) -> MatchRequest {
        let names: Arc<[OsString]> = (0..rows)
            .map(|i| OsString::from(format!("f{i}.txt")))
            .collect();
        MatchRequest {
            generation,
            rows: names,
            pattern: NamePattern::new("*.txt", PatternSyntax::Wildcard, true, rf_core::Os::CURRENT)
                .expect("a pattern")
                .expect("something to match"),
        }
    }

    #[test]
    fn a_newer_request_takes_the_place_of_the_one_that_waits() {
        // The worker hands each answer over and then waits for `go` before the next request:
        // the requests made meanwhile wait.
        let (done, found) = mpsc::channel::<(u64, usize)>();
        let (go, wait) = mpsc::channel::<()>();
        let wait = Mutex::new(wait);
        let matcher = matcher(move |outcome| {
            let Outcome::Done(matched) = outcome else {
                panic!("a match that failed");
            };
            done.send((matched.generation, matched.hits.len()))
                .expect("the test listens");
            let _ = wait
                .lock()
                .expect("one worker")
                .recv_timeout(Duration::from_secs(10));
        });
        let next = || found.recv_timeout(Duration::from_secs(10)).ok();

        matcher.submit(request(1, 3)).expect("a worker");
        assert_eq!(next(), Some((1, 3)));
        // While the worker hands over the first answer: two more; only the newest runs.
        matcher.submit(request(2, 5)).expect("a worker");
        matcher.submit(request(3, 7)).expect("a worker");
        go.send(()).expect("the worker listens");
        assert_eq!(next(), Some((3, 7)));
        go.send(()).expect("the worker listens");
        assert_eq!(found.recv_timeout(Duration::from_millis(100)).ok(), None);
    }

    #[test]
    fn a_run_stops_once_a_newer_request_is_made() {
        let stopped = request(4, 5_000).run(&|| false);
        assert_eq!(stopped, None);
        let whole = request(4, 5_000).run(&|| true).expect("not stopped");
        assert_eq!(whole.hits.len(), 5_000);
        // A short run has no stop to look at.
        assert!(request(5, 10).run(&|| false).is_some());
    }
}
