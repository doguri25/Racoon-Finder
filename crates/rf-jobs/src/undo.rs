//! What a finished job records so it can be reversed (OPS-12). The journal stores these
//! records, so their serialized shape is part of the on-disk format.

use std::time::SystemTime;

use rf_core::{Entry, EntryKind, VPath};
use serde::{Deserialize, Serialize};

use crate::spec::CreateKind;

/// An item a copy made, with what it looked like when the job finished: an undo trashes it
/// only if it still looks like that.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedItem {
    pub path: VPath,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// The volume and file ids (`VolumeId` and `FileId`) when the file system gave both.
    pub identity: Option<(u64, u128)>,
}

impl CreatedItem {
    /// The item at `path`, as `entry` describes it. A modification time before 1970 is left
    /// out (`None`), since the journal cannot store it; an undo then cannot tell that the item
    /// is unchanged, and leaves it.
    pub fn from_entry(path: VPath, entry: &Entry) -> CreatedItem {
        CreatedItem {
            path,
            is_dir: entry.kind == EntryKind::Dir,
            size: entry.size,
            modified: entry.modified.filter(|t| *t >= SystemTime::UNIX_EPOCH),
            identity: entry.identity().map(|(volume, file)| (volume.0, file.0)),
        }
    }
}

/// One item a move carried from `from` to `to`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedItem {
    pub from: VPath,
    pub to: VPath,
}

/// One item sent to the trash, and where the trash holds it when the platform says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashedItem {
    pub original: VPath,
    pub location: Option<VPath>,
}

/// How to reverse one finished job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UndoRecord {
    Copied {
        /// The top-level items the job made. A folder the job created is listed once and
        /// what is inside it is not; for a folder merged into one that already existed, the
        /// items the job added inside it are listed.
        created: Vec<CreatedItem>,
        /// How many existing files the job replaced. Their old content is gone, so undo
        /// cannot bring it back.
        overwritten: u64,
    },
    Moved {
        moves: Vec<MovedItem>,
        /// Folders the job made at the target, parents first.
        created_dirs: Vec<VPath>,
        /// Source folders the job removed once they were empty, children first.
        removed_dirs: Vec<VPath>,
    },
    Renamed {
        from: VPath,
        to: VPath,
    },
    Created {
        path: VPath,
        kind: CreateKind,
    },
    Removed {
        path: VPath,
        kind: CreateKind,
    },
    Trashed {
        items: Vec<TrashedItem>,
    },
    Restored {
        items: Vec<TrashedItem>,
    },
}

impl UndoRecord {
    /// The number of items the record covers: the length of its list for a copy, a move, a
    /// trash and a restore, and 1 for the others.
    pub fn item_count(&self) -> usize {
        match self {
            Self::Copied { created, .. } => created.len(),
            Self::Moved { moves, .. } => moves.len(),
            Self::Trashed { items } | Self::Restored { items } => items.len(),
            Self::Renamed { .. } | Self::Created { .. } | Self::Removed { .. } => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rf_core::{FileId, VolumeId};

    use super::*;

    #[test]
    fn a_time_before_1970_is_not_recorded() {
        let path = VPath::local(std::env::temp_dir().join("a.txt")).unwrap();
        let mut entry = Entry::new("a.txt", EntryKind::File);
        (entry.size, entry.volume_id, entry.file_id) = (3, Some(VolumeId(1)), Some(FileId(7)));
        let at = |offset: i64| {
            let step = Duration::from_secs(offset.unsigned_abs());
            if offset < 0 {
                SystemTime::UNIX_EPOCH - step
            } else {
                SystemTime::UNIX_EPOCH + step
            }
        };
        for (modified, kept) in [(-1, None), (0, Some(at(0))), (1, Some(at(1)))] {
            entry.modified = Some(at(modified));
            let item = CreatedItem::from_entry(path.clone(), &entry);
            assert_eq!(
                item,
                CreatedItem {
                    path: path.clone(),
                    is_dir: false,
                    size: 3,
                    modified: kept,
                    identity: Some((1, 7)),
                }
            );
            // What the record keeps, the journal can store.
            serde_json::to_string(&item).unwrap();
        }
    }
}
