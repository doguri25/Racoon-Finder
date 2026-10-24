//! What a finished job records so it can be reversed (OPS-12), and the history of those
//! records the engine keeps for undo and redo. The journal stores the records, so their
//! serialized shape is part of the on-disk format.

use std::collections::VecDeque;
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
        /// When the job finished, after it wrote its last item: a copied folder is unchanged
        /// only while nothing in it is newer. `None` in a record written before this was kept
        /// (or for a clock before 1970), and an undo then leaves every copied folder where it
        /// is.
        ///
        /// It is one time for the whole job, so it leaves a window: an item changed in a
        /// copied folder after that folder was done but before the job ended has a time before
        /// `at`, and the change goes unseen. The folder then goes to the trash with it, where
        /// it can still be got back.
        #[serde(default)]
        at: Option<SystemTime>,
    },
    Moved {
        moves: Vec<MovedItem>,
        /// Folders the job made at the target, parents first.
        created_dirs: Vec<VPath>,
        /// Source folders the job removed once they were empty, children first.
        removed_dirs: Vec<VPath>,
        /// How many existing files the job replaced, under an overwrite policy. Their old
        /// content is gone, so undo cannot bring it back; it says so once for each. `0` in a
        /// record written before this was kept.
        #[serde(default)]
        overwritten: u64,
        /// Each folder of `created_dirs` (`to`) with the source folder it was made for
        /// (`from`), parents first: a folder moved across volumes, whose children were copied
        /// into a new folder that took the source folder's times and attributes. An undo that
        /// makes such a source folder again gives it those of `to` before it removes `to`.
        /// Empty in a record written before this was kept, and an undo then makes the source
        /// folders again with new times.
        #[serde(default)]
        copied_dirs: Vec<MovedItem>,
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

/// How many jobs the engine can undo: the history keeps the latest this many.
pub const UNDO_LIMIT: usize = 100;

/// The records of the jobs that can be undone and of the undos that can be done again (redo),
/// each list holding at most `limit` of them: the oldest goes first.
#[derive(Clone, Debug)]
pub struct UndoHistory {
    limit: usize,
    undo: VecDeque<UndoRecord>,
    redo: VecDeque<UndoRecord>,
    /// How many records of jobs the user ran were noted ([`push`](Self::push)).
    generation: u64,
}

impl UndoHistory {
    /// An empty history that keeps at most `limit` records in each list.
    pub fn new(limit: usize) -> UndoHistory {
        UndoHistory {
            limit,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            generation: 0,
        }
    }

    /// Notes `record`, the record of a job the user ran: it can be undone next. A new action
    /// makes what was undone before it impossible to redo, so the redo list is emptied.
    pub fn push(&mut self, record: UndoRecord) {
        self.redo.clear();
        self.generation += 1;
        Self::bounded_push(&mut self.undo, record, self.limit);
    }

    /// How many records of jobs the user ran were noted so far ([`push`](Self::push)). The
    /// engine puts a record it took off a list back, and notes what its undo or redo left,
    /// only in the generation it took the record in (see
    /// [`put_back_undo`](Self::put_back_undo)).
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Puts `record`, which [`pop_undo`](Self::pop_undo) took in the generation `popped_at`,
    /// back at the end of the undo list, for an undo that never ran; the redo list stays as it
    /// is. Only while no record of a job the user ran was noted since: that newer record must
    /// be undone first, so the old one is dropped then. Says whether it was put back.
    pub(crate) fn put_back_undo(&mut self, record: UndoRecord, popped_at: u64) -> bool {
        let current = popped_at == self.generation;
        if current {
            Self::bounded_push(&mut self.undo, record, self.limit);
        }
        current
    }

    /// Puts `record`, which [`pop_redo`](Self::pop_redo) took in the generation `popped_at`,
    /// back at the end of the redo list, for a redo that never ran. Only while no record of a
    /// job the user ran was noted since, after which nothing can be redone: the record is
    /// dropped then. Says whether it was put back.
    pub(crate) fn put_back_redo(&mut self, record: UndoRecord, popped_at: u64) -> bool {
        let current = popped_at == self.generation;
        if current {
            Self::bounded_push(&mut self.redo, record, self.limit);
        }
        current
    }

    /// Takes the record of the job to undo next, the last one noted.
    pub fn pop_undo(&mut self) -> Option<UndoRecord> {
        self.undo.pop_back()
    }

    /// Notes `reverse`, the record an undo left: it redoes what was undone, next.
    pub fn push_undone(&mut self, reverse: UndoRecord) {
        Self::bounded_push(&mut self.redo, reverse, self.limit);
    }

    /// Takes the record of the undo to redo next, the last one undone.
    pub fn pop_redo(&mut self) -> Option<UndoRecord> {
        self.redo.pop_back()
    }

    /// Notes `reverse`, the record a redo left: it can be undone next. The redo list stays as
    /// it is, so the undos before it can still be redone.
    pub fn push_redone(&mut self, reverse: UndoRecord) {
        Self::bounded_push(&mut self.undo, reverse, self.limit);
    }

    /// What can be undone, the oldest first: the last one is undone next.
    pub fn undo_list(&self) -> Vec<UndoRecord> {
        self.undo.iter().cloned().collect()
    }

    /// What can be redone: the last one is redone next.
    pub fn redo_list(&self) -> Vec<UndoRecord> {
        self.redo.iter().cloned().collect()
    }

    /// Adds `record` at the end of `list` and drops the oldest records beyond `limit`.
    fn bounded_push(list: &mut VecDeque<UndoRecord>, record: UndoRecord, limit: usize) {
        list.push_back(record);
        while list.len() > limit {
            list.pop_front();
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

    #[test]
    fn a_record_goes_back_only_in_the_generation_it_was_taken_in() {
        let r = |n: &str| UndoRecord::Renamed {
            from: VPath::local(std::env::temp_dir().join(n)).unwrap(),
            to: VPath::local(std::env::temp_dir().join(format!("{n}2"))).unwrap(),
        };
        let mut h = UndoHistory::new(10);
        h.push(r("a"));
        h.push(r("b"));
        h.push_undone(r("x"));
        let at = h.generation();
        let b = h.pop_undo().unwrap();
        assert!(h.put_back_undo(b, at));
        // Back on top, and the redo list as it was.
        assert_eq!(
            (h.undo_list(), h.redo_list()),
            (vec![r("a"), r("b")], vec![r("x")])
        );
        let x = h.pop_redo().unwrap();
        assert!(h.put_back_redo(x, at));
        assert_eq!(h.redo_list(), [r("x")]);
        let (b, x) = (h.pop_undo().unwrap(), h.pop_redo().unwrap());
        h.push(r("c"));
        assert_eq!(h.generation(), at + 1);
        assert!(!h.put_back_undo(b, at) && !h.put_back_redo(x, at));
        assert_eq!(
            (h.undo_list(), h.redo_list()),
            (vec![r("a"), r("c")], Vec::new())
        );
    }
}
