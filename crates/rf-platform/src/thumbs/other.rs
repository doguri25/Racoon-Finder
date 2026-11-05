//! No thumbnails of the system's here (Linux, which is for development): every file keeps its
//! kind's icon unless the app decodes it itself.

use std::path::Path;

use super::ThumbOutcome;

pub(super) const SUPPORTED: bool = false;

pub(super) fn thumbnail(_path: &Path, _size: u32) -> ThumbOutcome {
    ThumbOutcome::None
}
