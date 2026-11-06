//! Links on a system that has only symbolic links (Linux, which is for development): no
//! shortcut, no junction.

use std::io;
use std::path::{Path, PathBuf};

pub(super) fn make_shortcut(_target: &Path, _at: &Path) -> io::Result<()> {
    Err(unsupported("shortcuts"))
}

pub(super) fn make_junction(_target: &Path, _at: &Path) -> io::Result<()> {
    Err(unsupported("junctions"))
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "the signature of the systems that read shortcuts"
)]
pub(super) fn read_shortcut(_at: &Path) -> io::Result<Option<PathBuf>> {
    Ok(None)
}

pub(super) fn symlink_needs_privilege() -> bool {
    false
}

fn unsupported(what: &'static str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("this system makes no {what}"),
    )
}
