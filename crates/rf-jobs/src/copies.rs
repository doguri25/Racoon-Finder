//! Whether a copy can be deleted for good without losing anything (OPS-12).
//!
//! An undo never deletes for good: where there is no trash (Linux, a USB stick, a network
//! share), the undo of a copy leaves its copies, and only the user may have them deleted for good,
//! through a job of their own ([`JobSpec::DeleteCopies`](crate::JobSpec::DeleteCopies)). Such a
//! copy may go only while it is as its copy job left it and its original still holds everything
//! it holds: [`check_copy`] tells that. A window calls it on a worker thread before it offers
//! the copies, and the delete of the copies calls it again for each one right before it deletes
//! it, whenever it runs (also when a job recovered after a crash runs again). The undo of a copy
//! checks what it trashes against what the copy left in the same way.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::Arc;
use std::time::SystemTime;

use rf_core::{Entry, EntryKind, Error, ErrorKind, VPath};
use rf_vfs::{CancelToken, Provider, Vfs};

use crate::conflict::MTIME_TOLERANCE;
use crate::exec::MAX_DEPTH;
use crate::undo::CreatedItem;

/// How many items [`compare_with_made`] and [`compare_with_original`] each look at, at most,
/// before they give up.
pub const COMPARE_ITEMS: usize = 20_000;

/// How an item is beside what it is compared with: a copy beside what its copy job left
/// ([`compare_with_made`]), or the original of a copy beside the copy
/// ([`compare_with_original`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Compared {
    /// As it was: the copy is as its job left it, or the original holds everything the copy
    /// holds. Deleting the copy loses nothing.
    Same,
    /// It is gone.
    Missing,
    /// It is there, but of another kind or size, written to since, or something of the copy
    /// is not in it.
    Changed,
    /// It could not be found out.
    Unknown,
}

/// What [`check_copy`] found of a copy and its original.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CopyFound {
    /// The copy beside what its job left ([`compare_with_made`]).
    pub copy: Compared,
    /// Its original beside it ([`compare_with_original`]); `Unknown` while the copy is not as
    /// its job left it: it is not the copy any more, so its original is not looked at.
    pub original: Compared,
}

impl CopyFound {
    /// Both as they were: deleting the copy loses nothing.
    pub const SAME: CopyFound = CopyFound {
        copy: Compared::Same,
        original: Compared::Same,
    };

    /// The copy found as `copy`, and its original not looked at.
    const fn copy_only(copy: Compared) -> CopyFound {
        CopyFound {
            copy,
            original: Compared::Unknown,
        }
    }
}

/// How `made`, an item a copy job made that ended at `at`, and its original `original` are
/// now: the copy beside what the job left ([`compare_with_made`]) and, while it is that, its
/// original beside it ([`compare_with_original`]). Only [`CopyFound::SAME`] lets the copy go.
/// `cancel` stops the look: what is not looked at then cannot be told.
///
/// Blocks (a large folder, a slow drive): call it on a worker thread.
pub fn check_copy(
    vfs: &Vfs,
    made: &CreatedItem,
    at: Option<SystemTime>,
    original: &VPath,
    cancel: &CancelToken,
) -> CopyFound {
    match look_at(vfs, &made.path) {
        Ok((provider, now)) => {
            check_found(vfs, provider.as_ref(), made, &now, at, original, cancel)
        }
        Err(copy) => CopyFound::copy_only(copy),
    }
}

/// [`check_copy`] of `made`, which `stat` found as `now` through `provider`. The delete of
/// copies checks the entry it then deletes, so that nothing comes between the two `stat`s.
pub(crate) fn check_found(
    vfs: &Vfs,
    provider: &dyn Provider,
    made: &CreatedItem,
    now: &Entry,
    at: Option<SystemTime>,
    original: &VPath,
    cancel: &CancelToken,
) -> CopyFound {
    let mut budget = COMPARE_ITEMS;
    let copy = as_left(provider, made, now, at, cancel, &mut budget).unwrap_or(Compared::Unknown);
    if copy != Compared::Same {
        return CopyFound::copy_only(copy);
    }
    CopyFound {
        copy,
        original: original_beside(vfs, provider, &made.path, now, original, cancel),
    }
}

/// Whether `made`, an item a copy job made that ended at `at`, is still as the job left it, by
/// the rule the undo of a copy checks it by: its recorded identity and modification time, the
/// recorded size for a file, and nothing newer than `at` under a folder. Gone, it is `Missing`;
/// without a recorded time (or `at`, for a folder), past [`COMPARE_ITEMS`] items, after
/// `cancel`, or for anything that cannot be looked at, it cannot be told.
///
/// Blocks (a large folder, a slow drive): call it on a worker thread.
pub fn compare_with_made(
    vfs: &Vfs,
    made: &CreatedItem,
    at: Option<SystemTime>,
    cancel: &CancelToken,
) -> Compared {
    if cancel.is_cancelled() {
        return Compared::Unknown;
    }
    let (provider, now) = match look_at(vfs, &made.path) {
        Ok(found) => found,
        Err(compared) => return compared,
    };
    let mut budget = COMPARE_ITEMS;
    as_left(provider.as_ref(), made, &now, at, cancel, &mut budget).unwrap_or(Compared::Unknown)
}

/// How `original` is now beside `copy`, a copy of it: whether deleting the copy for good would
/// lose nothing that is not still in the original.
///
/// Each is looked at as it is, a link as the link. They must be of one kind (a copy that
/// followed a link is compared with what the link leads to). A file must be of the copy's size,
/// and changed when the copy says (a copy keeps its source's time; within [`MTIME_TOLERANCE`]),
/// and a link lead where the copy's does. A folder must hold, under the same names, everything
/// the copy holds, each the same way, down to [`MAX_DEPTH`] levels; what the original has more
/// does not matter. An original that is, at any level, the copy's own item (the same volume
/// and file ID: a link to the copy, a linked folder above it, a hard link) cannot be told, as
/// deleting the copy would lose it. Past [`COMPARE_ITEMS`] items, after `cancel`, or for
/// anything that cannot be looked at (the copy among them), it cannot be told either.
///
/// Blocks (a large folder, a slow drive): call it on a worker thread.
pub fn compare_with_original(
    vfs: &Vfs,
    copy: &VPath,
    original: &VPath,
    cancel: &CancelToken,
) -> Compared {
    let Ok(at_copy) = vfs.provider(copy) else {
        return Compared::Unknown;
    };
    let Ok(copied) = at_copy.stat(copy) else {
        return Compared::Unknown;
    };
    original_beside(vfs, at_copy.as_ref(), copy, &copied, original, cancel)
}

/// The provider of `path` and what `stat` finds there; `Missing` when nothing is there, and
/// `Unknown` when it cannot be told.
fn look_at(vfs: &Vfs, path: &VPath) -> Result<(Arc<dyn Provider>, Entry), Compared> {
    let provider = vfs.provider(path).map_err(|_| Compared::Unknown)?;
    match provider.stat(path) {
        Ok(entry) => Ok((provider, entry)),
        Err(error) if error.kind() == ErrorKind::NotFound => Err(Compared::Missing),
        Err(_) => Err(Compared::Unknown),
    }
}

/// [`compare_with_original`] of the copy `copy` of `at_copy`, which `stat` found as `copied`.
pub(crate) fn original_beside(
    vfs: &Vfs,
    at_copy: &dyn Provider,
    copy: &VPath,
    copied: &Entry,
    original: &VPath,
    cancel: &CancelToken,
) -> Compared {
    let Ok(at_original) = vfs.provider(original) else {
        return Compared::Unknown;
    };
    let compare = Compare {
        copy: at_copy,
        original: at_original.as_ref(),
        cancel,
    };
    let mut budget = COMPARE_ITEMS;
    match at_original.stat(original) {
        Ok(entry) => compare.items(copy, copied, original, &entry, 0, &mut budget),
        Err(error) if error.kind() == ErrorKind::NotFound => Compared::Missing,
        Err(_) => Compared::Unknown,
    }
}

/// How `made`, an item a copy job made that ended at `at`, is beside what the job left, `now`
/// being what `stat` finds at its path through `provider`.
///
/// Without a recorded modification time (a time before 1970 is not recorded), it cannot be
/// told. Otherwise its identity must be the recorded one when both are known, and its
/// modification time the recorded one:
///
/// - A file (or a link): it must be no folder, of the recorded size.
/// - A folder: it must be a folder, `at` must be known (else it cannot be told), and nothing
///   under it may be newer than `at` (see [`untouched_since`]). The folder's own time catches
///   an item added to it or taken from it; an item changed, added or taken deeper down leaves
///   a time after `at` on itself or on its folder. A file that the copy gave a time after
///   `at`, because its source has a time in the future, makes its folder count as changed too.
///
/// Each item looked at under a folder takes one from `budget`; once none is left, it cannot be
/// told. Fails with the error of a listing under the folder, and with `Error::Cancelled` once
/// `cancel` is.
pub(crate) fn as_left(
    provider: &dyn Provider,
    made: &CreatedItem,
    now: &Entry,
    at: Option<SystemTime>,
    cancel: &CancelToken,
    budget: &mut usize,
) -> rf_core::Result<Compared> {
    let Some(was) = made.modified else {
        return Ok(Compared::Unknown);
    };
    let other_item = matches!(
        (made.identity, now.identity()),
        (Some(was), Some((volume, file))) if was != (volume.0, file.0)
    );
    if other_item || now.modified != Some(was) {
        return Ok(Compared::Changed);
    }
    if !made.is_dir {
        let same = now.kind != EntryKind::Dir && now.size == made.size;
        return Ok(if same {
            Compared::Same
        } else {
            Compared::Changed
        });
    }
    if now.kind != EntryKind::Dir {
        return Ok(Compared::Changed);
    }
    let Some(at) = at else {
        return Ok(Compared::Unknown);
    };
    untouched_since(provider, &made.path, now, at, 1, cancel, budget)
}

/// Whether nothing under the folder `dir` of `provider`, which `entry` describes, is newer than
/// `at`: every item it lists has a known modification time, none of them after `at`, and every
/// real folder among them (`EntryKind::Dir`, as the listing gives it: a link is never followed)
/// holds the same. `depth` is how far below the copied folder the listed items are (its own
/// items are at 1); each item looked at takes one from `budget`, and once none is left it
/// cannot be told.
///
/// The copy went no deeper than [`MAX_DEPTH`], so a folder whose items would be deeper must be
/// empty. A folder on another volume than the one it is in (both volume IDs known) is not
/// entered and counts as a change. Fails with the listing's error, and with `Error::Cancelled`
/// once `cancel` is.
fn untouched_since(
    provider: &dyn Provider,
    dir: &VPath,
    entry: &Entry,
    at: SystemTime,
    depth: usize,
    cancel: &CancelToken,
    budget: &mut usize,
) -> rf_core::Result<Compared> {
    let mut children = Vec::new();
    provider.list(dir, cancel, &mut |batch| children.extend(batch))?;
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if depth > MAX_DEPTH {
        return Ok(if children.is_empty() {
            Compared::Same
        } else {
            Compared::Changed
        });
    }
    for child in &children {
        if *budget == 0 {
            return Ok(Compared::Unknown);
        }
        *budget -= 1;
        if child.modified.is_none_or(|modified| modified > at) {
            return Ok(Compared::Changed);
        }
        if child.kind != EntryKind::Dir {
            continue;
        }
        let elsewhere = matches!(
            (entry.volume_id, child.volume_id),
            (Some(here), Some(there)) if here != there
        );
        if elsewhere {
            return Ok(Compared::Changed);
        }
        let path = dir.join(&child.name)?;
        let below = untouched_since(provider, &path, child, at, depth + 1, cancel, budget)?;
        if below != Compared::Same {
            return Ok(below);
        }
    }
    Ok(Compared::Same)
}

/// The two providers [`compare_with_original`] compares a copy and its original through, until
/// `cancel`.
struct Compare<'p> {
    copy: &'p dyn Provider,
    original: &'p dyn Provider,
    cancel: &'p CancelToken,
}

impl Compare<'_> {
    /// How the item `original`, which `stat` found as `found`, is beside `copied`, the copy at
    /// `copy`, at `depth` levels below the items compared first; each item looked at takes one
    /// from `budget`.
    fn items(
        &self,
        copy: &VPath,
        copied: &Entry,
        original: &VPath,
        found: &Entry,
        depth: usize,
        budget: &mut usize,
    ) -> Compared {
        if *budget == 0 || depth > MAX_DEPTH || self.cancel.is_cancelled() {
            return Compared::Unknown;
        }
        *budget -= 1;
        // A copy that followed a link is what the link leads to.
        let followed;
        let found = if found.kind == EntryKind::Symlink && copied.kind != EntryKind::Symlink {
            match self.original.stat_target(original) {
                Ok(target) => {
                    followed = target;
                    &followed
                }
                Err(error) if error.kind() == ErrorKind::NotFound => return Compared::Missing,
                Err(_) => return Compared::Unknown,
            }
        } else {
            found
        };
        if found.kind != copied.kind {
            return Compared::Changed;
        }
        if self.is_the_copy(copy, copied, found) {
            // The original leads to the copy itself (a link to it, a linked folder above it, a
            // hard link): deleting the copy would lose what the original holds.
            return Compared::Unknown;
        }
        match copied.kind {
            EntryKind::File => same_file(copied, found),
            EntryKind::Symlink => {
                match (self.copy.read_link(copy), self.original.read_link(original)) {
                    (Ok(a), Ok(b)) if a.target == b.target => Compared::Same,
                    (Ok(_), Ok(_)) => Compared::Changed,
                    _ => Compared::Unknown,
                }
            }
            EntryKind::Dir => self.folders(copy, original, depth, budget),
            // A device or a pipe is never copied.
            EntryKind::Other => Compared::Unknown,
        }
    }

    /// Whether `found`, the original as [`Compare::items`] looks at it, is `copied`, the copy
    /// at `copy`, itself: both have one identity (volume and file ID). A listing that gives no
    /// IDs (Windows) leaves the copy's to a `stat`, which only an original with a known
    /// identity needs.
    fn is_the_copy(&self, copy: &VPath, copied: &Entry, found: &Entry) -> bool {
        let Some(original) = found.identity() else {
            return false;
        };
        let copy_identity = copied
            .identity()
            .or_else(|| self.copy.stat(copy).ok()?.identity());
        copy_identity == Some(original)
    }

    /// Whether the folder `original` holds everything the folder `copy` holds, as
    /// [`Compare::items`] tells for each.
    fn folders(
        &self,
        copy: &VPath,
        original: &VPath,
        depth: usize,
        budget: &mut usize,
    ) -> Compared {
        let (Some(copied), Some(found)) = (
            self.listing(self.copy, copy),
            self.listing(self.original, original),
        ) else {
            return Compared::Unknown;
        };
        let found: HashMap<&OsStr, &Entry> = found
            .iter()
            .map(|entry| (entry.name.as_os_str(), entry))
            .collect();
        for child in &copied {
            let Some(&there) = found.get(child.name.as_os_str()) else {
                return Compared::Changed;
            };
            let (Ok(copy_child), Ok(original_child)) =
                (copy.join(&child.name), original.join(&child.name))
            else {
                return Compared::Unknown;
            };
            let compared = self.items(
                &copy_child,
                child,
                &original_child,
                there,
                depth + 1,
                budget,
            );
            if compared != Compared::Same {
                // Gone below a folder that is there is a change of the folder.
                return if compared == Compared::Missing {
                    Compared::Changed
                } else {
                    compared
                };
            }
        }
        Compared::Same
    }

    /// Everything in the folder `dir` of `provider`; `None` when it cannot be listed.
    fn listing(&self, provider: &dyn Provider, dir: &VPath) -> Option<Vec<Entry>> {
        let mut entries = Vec::new();
        provider
            .list(dir, self.cancel, &mut |batch| entries.extend(batch))
            .ok()?;
        (!self.cancel.is_cancelled()).then_some(entries)
    }
}

/// How the file `found` is beside `copied`, a copy of it: the same when it has the copy's size
/// and was last changed when the copy says (a copy keeps its source's time, within
/// [`MTIME_TOLERANCE`]); changed when not; not known without both times.
fn same_file(copied: &Entry, found: &Entry) -> Compared {
    let (Some(copy_time), Some(original_time)) = (copied.modified, found.modified) else {
        return Compared::Unknown;
    };
    let apart = copy_time
        .duration_since(original_time)
        .or_else(|_| original_time.duration_since(copy_time))
        .unwrap_or_default();
    if found.size == copied.size && apart <= MTIME_TOLERANCE {
        Compared::Same
    } else {
        Compared::Changed
    }
}
