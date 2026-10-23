//! What to do when a name already exists at the target.

use std::time::Duration;

use rf_core::{Entry, EntryKind, VPath};

use crate::spec::ConflictPolicy;

/// How far apart two modification times must be before one counts as newer. FAT32 stores
/// times in 2 s steps, so a copy made there reads back up to 2 s off; without the margin
/// "overwrite if newer" would copy the same file again every time.
pub const MTIME_TOLERANCE: Duration = Duration::from_secs(2);

/// A name that exists at the target, with both sides' metadata. It is what an "ask" window
/// needs to show size and date next to each other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// Where the incoming item comes from.
    pub source: VPath,
    /// The path both items want.
    pub target: VPath,
    pub incoming: Entry,
    pub existing: Entry,
}

/// What the user chose for a conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConflictAction {
    Overwrite,
    Skip,
    /// Keep both: the incoming item gets a numbered name.
    Rename,
    /// Stop the whole job.
    Cancel,
}

/// A [`ConflictAction`] and whether it also settles the job's remaining conflicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConflictAnswer {
    pub action: ConflictAction,
    /// Use `action` for every later conflict of the job without asking again.
    pub apply_to_all: bool,
}

impl ConflictAnswer {
    /// `action` for this conflict only.
    pub const fn once(action: ConflictAction) -> Self {
        Self {
            action,
            apply_to_all: false,
        }
    }

    /// `action` for this conflict and every later one of the job.
    pub const fn all(action: ConflictAction) -> Self {
        Self {
            action,
            apply_to_all: true,
        }
    }
}

/// What a [`ConflictPolicy`] makes of one conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Decision {
    /// Replace the existing item.
    Overwrite,
    /// Leave the existing item and do not transfer the incoming one.
    Skip,
    /// Keep both: the incoming item gets a numbered name.
    Rename,
    /// Let the user choose, through the job's `Resolver`.
    Ask,
    /// The policy would replace a folder, or replace something with a folder. Touch neither
    /// side and report the item as failed.
    Refuse,
}

/// Applies `policy` to an `incoming` item whose name is taken by `existing`.
///
/// The overwrite policies (`Overwrite`, `OverwriteIfNewer`, `OverwriteIfSizeDiffers`) fail
/// closed: when either side is a folder the answer is [`Decision::Refuse`], so a file and a
/// folder never replace each other. That includes two folders. Callers merge two folders
/// (copy their children into the existing one) and do not call this function for them; if
/// one does anyway, nothing is overwritten.
///
/// `OverwriteIfNewer` overwrites only when both modification times are known and the
/// incoming one is later than the existing one by more than [`MTIME_TOLERANCE`]. A missing
/// time, or a time too large to add the tolerance to, gives [`Decision::Skip`].
///
/// `OverwriteIfSizeDiffers` compares the byte sizes only.
pub fn decide(policy: ConflictPolicy, incoming: &Entry, existing: &Entry) -> Decision {
    let overwrites = matches!(
        policy,
        ConflictPolicy::Overwrite
            | ConflictPolicy::OverwriteIfNewer
            | ConflictPolicy::OverwriteIfSizeDiffers
    );
    if overwrites && (incoming.kind == EntryKind::Dir || existing.kind == EntryKind::Dir) {
        return Decision::Refuse;
    }
    match policy {
        ConflictPolicy::Ask => Decision::Ask,
        ConflictPolicy::Skip => Decision::Skip,
        ConflictPolicy::AutoRename => Decision::Rename,
        ConflictPolicy::Overwrite => Decision::Overwrite,
        ConflictPolicy::OverwriteIfNewer => overwrite_if(is_newer(incoming, existing)),
        ConflictPolicy::OverwriteIfSizeDiffers => overwrite_if(incoming.size != existing.size),
    }
}

fn overwrite_if(wanted: bool) -> Decision {
    if wanted {
        Decision::Overwrite
    } else {
        Decision::Skip
    }
}

/// Whether `incoming` was modified more than [`MTIME_TOLERANCE`] after `existing`.
fn is_newer(incoming: &Entry, existing: &Entry) -> bool {
    match (incoming.modified, existing.modified) {
        (Some(new), Some(old)) => old
            .checked_add(MTIME_TOLERANCE)
            .is_some_and(|limit| new > limit),
        _ => false,
    }
}
