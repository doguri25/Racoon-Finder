//! The types a provider uses to report live changes of one folder.

use std::any::Any;
use std::ffi::OsString;
use std::fmt;
use std::time::Duration;

use rf_core::Entry;

/// How long a provider collects raw notifications before it sends them as one
/// `WatchEvent::Changes`.
pub const WATCH_DEBOUNCE: Duration = Duration::from_millis(100);

/// The most changes a provider holds back for one `WatchEvent::Changes`. When more pile up it
/// sends `WatchEvent::Rescan` instead.
pub const MAX_PENDING_CHANGES: usize = 10_000;

/// One change to a direct child of the watched folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// The entry was added or changed; this is how it looks now.
    Upsert(Entry),
    /// The entry with this name is gone.
    Removed(OsString),
    /// The entry `from` is now `to`.
    Renamed { from: OsString, to: Entry },
}

/// What a watch reports to its sink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchEvent {
    /// Changes since the last event, in the order they happened.
    Changes(Vec<Change>),
    /// Changes may have been lost; the receiver lists the folder again.
    Rescan,
    /// The watch cannot go on and no event follows. The receiver drops the `WatchHandle`.
    Ended,
}

/// Receives the events of one watch, on a background thread of the provider. It must not block
/// for long.
pub type WatchSink = Box<dyn FnMut(WatchEvent) + Send>;

/// A running watch. Dropping it stops the watch, and the sink is not called after the drop.
///
/// Never drop it from inside the sink: stopping can wait for the thread that runs the sink.
pub struct WatchHandle {
    _guard: Box<dyn Any + Send>,
}

impl WatchHandle {
    /// A handle that keeps `guard` alive until it is dropped. A provider passes whatever stops
    /// its watch when dropped (an OS watcher, a thread join guard).
    pub fn new(guard: impl Send + 'static) -> WatchHandle {
        WatchHandle {
            _guard: Box::new(guard),
        }
    }
}

impl fmt::Debug for WatchHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WatchHandle")
    }
}
