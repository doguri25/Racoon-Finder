//! Listing a folder in batches.

use std::io::ErrorKind as IoKind;

use rf_core::{Entry, EntryKind, Error, Result, VPath};

use super::{entry_from_metadata, kind_of, native};
use crate::cancel::CancelToken;
use crate::provider::LIST_BATCH_SIZE;

/// Streams the children of `dir` to `sink`, `LIST_BATCH_SIZE` at a time in the order the OS
/// gives them; only the last batch can be smaller.
///
/// An item that vanishes between the directory read and its `stat` is skipped. One whose
/// metadata cannot be read is listed with its name and kind only.
pub(super) fn list(
    dir: &VPath,
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Vec<Entry>),
) -> Result<()> {
    cancel.check()?;
    let read_err = |e| Error::io("read_dir", dir.clone(), e);
    let mut batch = Vec::with_capacity(LIST_BATCH_SIZE);
    for item in std::fs::read_dir(native(dir)?).map_err(read_err)? {
        cancel.check()?;
        let item = item.map_err(read_err)?;
        let name = item.file_name();
        let entry = match item.metadata() {
            Ok(meta) => entry_from_metadata(name, &meta),
            Err(e) if e.kind() == IoKind::NotFound => continue,
            Err(_) => {
                let kind = item.file_type().map_or(EntryKind::File, kind_of);
                Entry::new(name, kind)
            }
        };
        batch.push(entry);
        if batch.len() == LIST_BATCH_SIZE {
            sink(std::mem::replace(
                &mut batch,
                Vec::with_capacity(LIST_BATCH_SIZE),
            ));
        }
    }
    if !batch.is_empty() {
        sink(batch);
    }
    Ok(())
}
