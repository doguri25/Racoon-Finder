//! Sorting of directory entries.
//!
//! Names compare with the OS natural order (`rf_platform::NaturalKey`). The OS rules are not
//! guaranteed to be total orders, and since Rust 1.81 `slice::sort_by` may panic on one, so
//! every sort here goes through `stable_order_by`, a merge sort that never panics.

use std::cmp::Ordering;
use std::ffi::OsStr;

use rf_core::{Entry, EntryKind};
use rf_platform::NaturalKey;

/// What a column sorts by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SortKey {
    /// Natural order of the name: `file2` before `file10`, case-insensitive.
    Name,
    /// The byte length.
    Size,
    /// The modification time; an unknown time sorts first.
    Modified,
    /// The kind (folder, file, symlink, other), then the lowercase extension of a file.
    Kind,
    /// The creation time; an unknown time (a file system that keeps none) sorts first.
    Created,
    /// The lowercase extension alone; none sorts first. Only a file has one, as the 확장자
    /// column shows: not a folder, nor a link, which may lead to a folder (`앨범.2025`).
    Extension,
}

/// Which way a key sorts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SortOrder {
    Ascending,
    Descending,
}

/// One sort column and its direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SortBy {
    pub key: SortKey,
    pub order: SortOrder,
}

/// How a folder listing is ordered.
///
/// Entries compare in this order, and the first difference decides: folders first (when
/// `folders_first`, whatever the direction), `primary`, `secondary`, the natural order of the
/// name ascending, and finally the name bytes ascending.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SortSpec {
    pub primary: SortBy,
    pub secondary: Option<SortBy>,
    pub folders_first: bool,
}

impl Default for SortSpec {
    /// Name ascending, no secondary key, folders first.
    fn default() -> SortSpec {
        SortSpec {
            primary: SortBy {
                key: SortKey::Name,
                order: SortOrder::Ascending,
            },
            secondary: None,
            folders_first: true,
        }
    }
}

/// What comparing an entry needs besides the entry itself, built once per entry.
struct Keys {
    name: NaturalKey,
    /// The lowercase extension of a file; empty for one without, and for any other kind.
    extension: String,
}

impl Keys {
    fn new(entry: &Entry) -> Keys {
        Keys {
            name: NaturalKey::new(&entry.name),
            extension: entry
                .extension()
                .filter(|_| entry.kind == EntryKind::File)
                .map(|ext| ext.to_string_lossy().to_lowercase())
                .unwrap_or_default(),
        }
    }
}

/// The position of an entry kind in a `SortKey::Kind` sort.
fn kind_rank(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::Dir => 0,
        EntryKind::File => 1,
        EntryKind::Symlink => 2,
        EntryKind::Other => 3,
    }
}

fn compare_by(by: SortBy, a: (&Entry, &Keys), b: (&Entry, &Keys)) -> Ordering {
    let ((entry_a, keys_a), (entry_b, keys_b)) = (a, b);
    let ordering = match by.key {
        SortKey::Name => keys_a.name.compare(&keys_b.name),
        SortKey::Size => entry_a.size.cmp(&entry_b.size),
        SortKey::Modified => entry_a.modified.cmp(&entry_b.modified),
        SortKey::Kind => kind_rank(entry_a.kind)
            .cmp(&kind_rank(entry_b.kind))
            .then_with(|| keys_a.extension.cmp(&keys_b.extension)),
        SortKey::Created => entry_a.created.cmp(&entry_b.created),
        SortKey::Extension => keys_a.extension.cmp(&keys_b.extension),
    };
    match by.order {
        SortOrder::Ascending => ordering,
        SortOrder::Descending => ordering.reverse(),
    }
}

fn compare_with_keys(spec: SortSpec, a: (&Entry, &Keys), b: (&Entry, &Keys)) -> Ordering {
    let (entry_a, keys_a) = a;
    let (entry_b, keys_b) = b;
    if spec.folders_first {
        let (dir_a, dir_b) = (
            entry_a.kind == EntryKind::Dir,
            entry_b.kind == EntryKind::Dir,
        );
        // A folder sorts first whatever the direction, so this is not reversed below.
        if dir_a != dir_b {
            return if dir_a {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
    }
    compare_by(spec.primary, a, b)
        .then_with(|| {
            spec.secondary
                .map_or(Ordering::Equal, |by| compare_by(by, a, b))
        })
        .then_with(|| keys_a.name.compare(&keys_b.name))
        .then_with(|| entry_a.name.cmp(&entry_b.name))
}

/// Compares two entries by `spec`, the rule `sort_entries` sorts by.
///
/// It builds the comparison keys on every call, so use it for a single comparison, such as
/// finding where a changed entry belongs in a sorted list with a binary search. To sort many
/// entries, use `sort_entries`.
pub fn compare_entries(a: &Entry, b: &Entry, spec: &SortSpec) -> Ordering {
    compare_with_keys(*spec, (a, &Keys::new(a)), (b, &Keys::new(b)))
}

/// Sorts `entries` by `spec`. The sort is stable and does not panic whatever the OS natural
/// order does; the result is always a permutation of the input.
pub fn sort_entries(entries: &mut Vec<Entry>, spec: &SortSpec) {
    // The keys may be tied to one thread (`NaturalKey` is not `Send` on macOS), so they live
    // only inside this call.
    let keys: Vec<Keys> = entries.iter().map(Keys::new).collect();
    let order = stable_order_by(entries.len(), |a, b| {
        compare_with_keys(*spec, (&entries[a], &keys[a]), (&entries[b], &keys[b]))
    });
    let mut slots: Vec<Option<Entry>> = entries.drain(..).map(Some).collect();
    entries.extend(order.into_iter().filter_map(|index| slots[index].take()));
}

/// Sorts `items` by the names `name` gives them, as `sort_entries` sorts entries by name
/// ascending with no other key and no folders first: the natural order of the name, then its
/// bytes. For a list of names that need no other field of an [`Entry`]. The sort is stable and
/// does not panic whatever the OS natural order does.
pub fn sort_by_name<T>(items: &mut Vec<T>, name: impl Fn(&T) -> &OsStr) {
    // As in `sort_entries`, the keys live only inside this call.
    let keys: Vec<NaturalKey> = items
        .iter()
        .map(|item| NaturalKey::new(name(item)))
        .collect();
    let order = stable_order_by(items.len(), |a, b| {
        keys[a]
            .compare(&keys[b])
            .then_with(|| name(&items[a]).cmp(name(&items[b])))
    });
    let mut slots: Vec<Option<T>> = items.drain(..).map(Some).collect();
    items.extend(order.into_iter().filter_map(|index| slots[index].take()));
}

/// The indices `0..len` in sorted order, by a bottom-up merge sort: adjacent runs of width 1,
/// 2, 4, ... are merged, and on a tie the left element goes first, so the result is stable.
///
/// `cmp(i, j)` compares element `i` with element `j`. It does not have to be a total order:
/// an inconsistent `cmp` gives an unspecified order but never a panic, and every index
/// appears exactly once.
pub(crate) fn stable_order_by(
    len: usize,
    mut cmp: impl FnMut(usize, usize) -> Ordering,
) -> Vec<usize> {
    let mut from: Vec<usize> = (0..len).collect();
    let mut to = vec![0; len];
    let mut width = 1;
    while width < len {
        let mut start = 0;
        while start < len {
            let middle = start.saturating_add(width).min(len);
            let end = middle.saturating_add(width).min(len);
            merge(
                &from[start..middle],
                &from[middle..end],
                &mut to[start..end],
                &mut cmp,
            );
            start = end;
        }
        std::mem::swap(&mut from, &mut to);
        width = width.saturating_mul(2);
    }
    from
}

/// Merges two runs into `out` (as long as both together); on a tie the left one goes first.
fn merge(
    left: &[usize],
    right: &[usize],
    out: &mut [usize],
    cmp: &mut impl FnMut(usize, usize) -> Ordering,
) {
    let (mut l, mut r) = (0, 0);
    for slot in out {
        let take_left = if l == left.len() {
            false
        } else if r == right.len() {
            true
        } else {
            cmp(left[l], right[r]) != Ordering::Greater
        };
        if take_left {
            *slot = left[l];
            l += 1;
        } else {
            *slot = right[r];
            r += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_order_never_panics_on_inconsistent_cmp() {
        let rps = |a: usize, b: usize| match (a % 3, b % 3) {
            (x, y) if x == y => Ordering::Equal,
            (0, 1) | (1, 2) | (2, 0) => Ordering::Less,
            _ => Ordering::Greater,
        };
        let mut order = stable_order_by(1_000, rps);
        order.sort_unstable();
        assert_eq!(order, (0..1_000).collect::<Vec<_>>());
    }

    #[test]
    fn names_sort_as_entries_sort_by_name() {
        let mut names: Vec<(&str, usize)> =
            vec![("b10", 0), ("Alpha", 1), ("b2", 2), ("beta", 3), ("b2", 4)];
        sort_by_name(&mut names, |(name, _)| OsStr::new(name));
        // Natural order, case-insensitive; equal names keep their order.
        assert_eq!(
            names,
            [("Alpha", 1), ("b2", 2), ("b2", 4), ("b10", 0), ("beta", 3)]
        );
        let mut entries: Vec<Entry> = ["b10", "Alpha", "b2", "beta"]
            .into_iter()
            .map(|name| Entry::new(name, EntryKind::Dir))
            .collect();
        sort_entries(&mut entries, &SortSpec::default());
        let mut alone: Vec<&str> = vec!["b10", "Alpha", "b2", "beta"];
        sort_by_name(&mut alone, |name| OsStr::new(name));
        let sorted: Vec<&OsStr> = entries.iter().map(|entry| entry.name.as_os_str()).collect();
        assert_eq!(sorted, alone.iter().map(OsStr::new).collect::<Vec<_>>());
    }

    #[test]
    fn stable_order_is_stable() {
        let keys = [3, 1, 3, 2, 1, 3];
        assert_eq!(
            stable_order_by(keys.len(), |a, b| keys[a].cmp(&keys[b])),
            [1, 4, 3, 0, 2, 5]
        );
    }
}
