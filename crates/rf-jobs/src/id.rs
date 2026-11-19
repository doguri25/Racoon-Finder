//! Job identifiers.

use std::fmt;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The identity of one job. It names the job's journal entry and its temporary files
/// (`.rf-{job:016x}-{seq}.rfpart`), so it must not repeat across processes either.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct JobId(pub u64);

impl JobId {
    /// A fresh id. It hashes the clock, the process id and a process-wide counter with a
    /// randomly keyed hasher: the counter keeps ids from one process apart, and the random
    /// keys, the process id and the clock keep other processes apart.
    pub fn generate() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let mut hasher = RandomState::new().build_hasher();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        hasher.write_u128(nanos);
        hasher.write_u32(std::process::id());
        hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        Self(hasher.finish())
    }

    /// Reads the 16 lowercase hex digits [`Display`](fmt::Display) writes, and nothing else:
    /// a shorter or longer string, an upper-case digit, a sign, whitespace or a non-ASCII digit
    /// is `None`. One id has one spelling, so a name made from it is matched exactly.
    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 16
            || !s
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return None;
        }
        u64::from_str_radix(s, 16).ok().map(Self)
    }
}

/// Sixteen lowercase hex digits, zero padded.
impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}
