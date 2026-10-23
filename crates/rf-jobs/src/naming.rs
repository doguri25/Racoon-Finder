//! Names the jobs engine invents: numbered names for items that must not replace an existing
//! one (`report (2).txt`), and the temporary file names a transfer writes before it commits.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use rf_core::Os;

use crate::id::JobId;

/// The ending of every temporary file a transfer writes. Crash recovery finds leftovers by it.
pub const TEMP_SUFFIX: &str = ".rfpart";

/// The highest number given to a numbered name.
pub const MAX_NUMBERED: u32 = 9_999;

/// The start of every temporary file name.
const TEMP_PREFIX: &str = ".rf-";

/// `name` with the number `n` added, for an item whose own name is taken.
///
/// A folder, a name without an extension and a name whose extension is empty (`a.`) keep the
/// whole name as the stem. Any other name is split at its last dot with [`Path::file_stem`] and
/// [`Path::extension`], so a dot file such as `.bashrc` has no extension and `a.tar.gz` has the
/// extension `gz`. Windows and other systems write `stem (n).ext`; macOS writes `stem n.ext`, the
/// Finder rule for keeping both items. An existing number is not recognized: `a (2).txt` becomes
/// `a (2) (3).txt`.
///
/// The name is assembled as an [`OsString`], so a name that is not valid UTF-8 keeps every byte.
/// A `name` that is not a single path component (it holds a separator) is not split at all.
pub fn numbered_name(name: &OsStr, n: u32, is_dir: bool, os: Os) -> OsString {
    let (stem, extension) = split_name(name, is_dir);
    let mut numbered = OsString::from(stem);
    numbered.push(match os {
        Os::MacOs => format!(" {n}"),
        Os::Windows | Os::Other => format!(" ({n})"),
    });
    if let Some(extension) = extension {
        numbered.push(".");
        numbered.push(extension);
    }
    numbered
}

/// The stem and the extension [`numbered_name`] works with.
fn split_name(name: &OsStr, is_dir: bool) -> (&OsStr, Option<&OsStr>) {
    let path = Path::new(name);
    // `Path` only looks at the last component, so a name that holds a separator (or, on
    // Windows, a drive prefix) would lose its front part. It is no file name; keep it whole.
    if is_dir || path.file_name() != Some(name) {
        return (name, None);
    }
    match (path.file_stem(), path.extension()) {
        (Some(stem), Some(extension)) if !extension.is_empty() => (stem, Some(extension)),
        _ => (name, None),
    }
}

/// The temporary file a transfer writes item number `seq` of job `job` to:
/// `.rf-{job:016x}-{seq}.rfpart`. It holds no character of the user's names, so every file
/// system accepts it and it stays far below the 255-unit name limit.
pub fn temp_name(job: JobId, seq: u64) -> OsString {
    OsString::from(format!("{TEMP_PREFIX}{job}-{seq}{TEMP_SUFFIX}"))
}

/// The job a temporary file name belongs to, or `None` for any name [`temp_name`] cannot have
/// produced: other letter case, other lengths, a signed or zero padded sequence number, a
/// sequence number that overflows, text before or after, or a name that is not text at all.
/// Crash recovery deletes what this matches, so it must never match a user's file.
pub fn temp_job(name: &OsStr) -> Option<JobId> {
    let body = name
        .to_str()?
        .strip_prefix(TEMP_PREFIX)?
        .strip_suffix(TEMP_SUFFIX)?;
    let (id, seq) = body.split_once('-')?;
    let job = JobId::from_hex(id)?;
    // `parse` also takes `+7` and `007`; only the exact spelling `temp_name` writes counts.
    let seq: u64 = seq.parse().ok()?;
    (temp_name(job, seq) == name).then_some(job)
}
