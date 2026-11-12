//! Sparse files: where a file holds data and where it has holes, so that a copy can keep the
//! holes instead of writing their zeros (XFER-11).
//!
//! Keeping holes is best effort, and an answer is only used where it cannot hide data. A file
//! system that calls a range a hole when the data is only kept elsewhere would turn a copy into
//! zeros without an error, so wherever that is known to happen the answer is "no holes", and
//! the file is copied byte by byte, with the same content.

use std::fs::File;
use std::io;
use std::ops::Range;

use rf_core::{Error, Result, VPath};

/// The ranges of the file at `path` that hold data; [`file_data_ranges`] of the file opened
/// for reading. Links are followed, as opening the file follows them. A non-local path gives
/// `Error::Unsupported`; other errors carry the op `"data_ranges"`.
pub fn data_ranges(path: &VPath) -> Result<Option<Vec<Range<u64>>>> {
    let Some(local) = path.to_extended_length() else {
        return Err(Error::Unsupported(format!(
            "data ranges need a local path: {path}"
        )));
    };
    let fail = |e| Error::io("data_ranges", path.clone(), e);
    let file = File::open(local).map_err(fail)?;
    let found = file_data_ranges(&file).map_err(fail)?;
    Ok(found.map(|found| found.ranges))
}

/// Where an open file holds data, as [`file_data_ranges`] found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataRanges {
    /// The length of the file, taken with the ranges from the same file; no range reaches past
    /// it.
    pub len: u64,
    /// The ranges that hold data, in ascending order and not overlapping.
    pub ranges: Vec<Range<u64>>,
}

/// The length of the open file `file` and its ranges that hold data, in ascending order and not
/// overlapping. The gaps between them, and before the first and after the last up to the
/// length, are holes: they read as zeros and take no space on the disk. Both are taken from
/// `file` (`fstat`, `GetFileInformationByHandle`), so they describe the file that was opened,
/// even when its path names another file by now (an app that saves by replacing the file).
/// The offset of `file` is never moved, so a reader of it can go on where it was.
///
/// `Ok(None)` means there are no holes to keep, or that the answer cannot be trusted:
/// - Linux: a file whose allocated blocks (`st_blocks` × 512) cover its length has none, and
///   nothing more is asked; pseudo-files such as those of `/proc` report no length and stop
///   there too. Otherwise `lseek` with `SEEK_DATA` and `SEEK_HOLE` walks a second open file
///   description of the same file, opened from `/proc/thread-self/fd` or else `/proc/self/fd`,
///   which leaves the offset of `file` alone. One that cannot be opened gives `None`, and so
///   does one that turns out to be another file (by device and inode), as a `/proc` that is
///   not the kernel's can give. A file system that does not know these seeks (`EINVAL`,
///   `ENOTSUP`, `ESPIPE`) gives `None`, and so do answers that do not agree with each other: an
///   `ENXIO` ("no data from here on") for an offset that `SEEK_HOLE` does not confirm as the
///   start of a hole (some file systems answer `ENXIO` for data they keep elsewhere, QEMU issue
///   3124), and data that starts after offset 0 when `SEEK_HOLE` does not find a hole at 0.
/// - Other Unix systems (the BSDs): the same code, but the walk needs the second open file
///   description that only Linux's `/proc` gives, so in effect it is Linux only: elsewhere no
///   place opens and the answer is `None`.
/// - macOS: always. A file compressed by the file system (decmpfs, `UF_COMPRESSED`) has fewer
///   blocks than its length and answers `ENXIO` to `SEEK_DATA` inside it, and APFS answers to
///   `SEEK_HOLE` have lost data before (GNU coreutils bugs 51857 and 61386), so its files are
///   copied whole.
/// - Windows: a file that is not sparse has none. A reparse point (a Cloud Files or `ProjFS`
///   placeholder, a deduplicated file) and a file marked offline or to be recalled on access
///   get their data from a file system filter or from elsewhere, which its allocated ranges do
///   not show, so they give `None` too. For any other sparse file
///   `FSCTL_QUERY_ALLOCATED_RANGES` is asked until it stops answering `ERROR_MORE_DATA`; a file
///   system that does not know it (`ERROR_INVALID_FUNCTION`) gives `None`.
///
/// One range that covers the whole file is no hole either and gives `None`, as does an answer
/// that does not make sense (a range that goes backwards, a walk that does not advance): a
/// caller then copies every byte, which is always right. A file that is all hole gives an
/// empty list.
///
/// The ranges describe the file as it was when it was looked at: no range reaches past the
/// length it had then. A file that changes afterwards can end before a range does, so a
/// caller that reads the ranges must check that it gets every byte of them.
pub fn file_data_ranges(file: &File) -> io::Result<Option<DataRanges>> {
    #[cfg(target_os = "macos")]
    {
        let _ = file;
        Ok(None)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(imp::data_ranges(file)?
            .and_then(|(len, ranges)| tidy(len, ranges).map(|ranges| DataRanges { len, ranges })))
    }
}

/// Lets the file `file`, open for writing, have holes: on Windows it sets the sparse attribute
/// (`FSCTL_SET_SPARSE`), without which NTFS writes the zeros of every range a write skips.
/// FAT and exFAT refuse it, and the file then cannot have holes.
///
/// Elsewhere it does nothing: a range that is skipped by seeking past it stays a hole wherever
/// the file system keeps holes, and reads as zeros everywhere.
pub fn mark_sparse(file: &File) -> io::Result<()> {
    #[cfg(windows)]
    {
        imp::mark_sparse(file)
    }
    #[cfg(not(windows))]
    {
        let _ = file;
        Ok(())
    }
}

/// `ranges`, the data ranges of a file of `len` bytes as the OS listed them, made what
/// [`file_data_ranges`] promises: in ascending order, cut at `len`, without empty ones, and
/// joined where they touch or overlap (a range is never made smaller than what the OS listed,
/// so no data is ever taken for a hole). `None` for a file without holes: an empty one, or one
/// whose ranges cover all of it.
#[cfg_attr(
    target_os = "macos",
    allow(dead_code, reason = "used off macOS; tested on every OS")
)]
fn tidy(len: u64, mut ranges: Vec<Range<u64>>) -> Option<Vec<Range<u64>>> {
    ranges.sort_unstable_by_key(|range| range.start);
    let mut joined: Vec<Range<u64>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        let range = range.start..range.end.min(len);
        if range.is_empty() {
            continue;
        }
        match joined.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => joined.push(range),
        }
    }
    let whole = matches!(joined.as_slice(), [only] if only.start == 0 && only.end == len);
    (len > 0 && !whole).then_some(joined)
}

/// Where one seek of a [`walk`] goes: to the first data, or the first hole, at or after an
/// offset (`SEEK_DATA`, `SEEK_HOLE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    any(windows, target_os = "macos"),
    allow(dead_code, reason = "used on Linux; tested on every OS")
)]
enum To {
    Data(u64),
    Hole(u64),
}

/// What one seek of a [`walk`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    any(windows, target_os = "macos"),
    allow(dead_code, reason = "used on Linux; tested on every OS")
)]
enum Found {
    /// The offset it found.
    At(u64),
    /// Nothing of the kind from the offset on (`ENXIO`).
    Nothing,
    /// The file cannot seek to data and holes (`EINVAL`, `ENOTSUP`, `ESPIPE`).
    Unknown,
}

/// The data ranges of a file of `len` bytes, found by seeking with `seek` from data to hole and
/// on until no data is left, or `None` when the answers cannot be trusted to show all the data.
///
/// "No data from here on" (`Found::Nothing` for `To::Data`) is believed only if the same offset
/// is the start of a hole (`To::Hole` finds it itself): a file system that keeps data elsewhere
/// can say there is none in a file full of it (QEMU issue 3124). Data that starts after offset
/// 0 is believed the same way: only if a hole starts at 0. Every later offset the walk asks
/// from is the start of a hole that `To::Hole` found itself.
#[cfg_attr(
    any(windows, target_os = "macos"),
    allow(dead_code, reason = "used on Linux; tested on every OS")
)]
fn walk(
    len: u64,
    mut seek: impl FnMut(To) -> io::Result<Found>,
) -> io::Result<Option<Vec<Range<u64>>>> {
    let mut ranges = Vec::new();
    let mut at = 0;
    while at < len {
        let start = match seek(To::Data(at))? {
            Found::At(start) => start,
            Found::Nothing => {
                let in_a_hole = seek(To::Hole(at))? == Found::At(at);
                return Ok(in_a_hole.then_some(ranges));
            }
            Found::Unknown => return Ok(None),
        };
        if at == 0 && start > 0 && seek(To::Hole(0))? != Found::At(0) {
            return Ok(None);
        }
        // `Nothing` here means the file got shorter since its data was found.
        let Found::At(end) = seek(To::Hole(start))? else {
            return Ok(None);
        };
        // A walk that does not advance cannot be trusted to have found all the data.
        if start < at || end <= start {
            return Ok(None);
        }
        ranges.push(start..end);
        at = end;
    }
    Ok(Some(ranges))
}

/// The Windows `FILE_ATTRIBUTE_SPARSE_FILE` bit.
const SPARSE_FILE: u32 = 0x200;

/// The Windows attribute bits of a file whose content need not lie in its allocated ranges:
/// `REPARSE_POINT` (Cloud Files and `ProjFS` placeholders and deduplicated files, whose data a
/// file system filter supplies), `OFFLINE`, `RECALL_ON_OPEN` and `RECALL_ON_DATA_ACCESS`.
const SERVED_ELSEWHERE: u32 = 0x400 | 0x1000 | 0x4_0000 | 0x40_0000;

/// Whether the allocated ranges of a Windows file with the attribute bits `attributes` show
/// where its data is: only for a sparse file whose content is not served from elsewhere.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn allocation_shows_data(attributes: u32) -> bool {
    attributes & SPARSE_FILE != 0 && attributes & SERVED_ELSEWHERE == 0
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::fs::{File, Metadata};
    use std::io;
    use std::ops::Range;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    use rustix::fs::{Mode, OFlags, SeekFrom};
    use rustix::io::Errno;

    use super::{Found, To, walk};

    /// The length of `file` and its data ranges as `lseek` finds them, or `None` when it has
    /// no holes or the answers cannot be trusted. The offset of `file` is never moved.
    pub fn data_ranges(file: &File) -> io::Result<Option<(u64, Vec<Range<u64>>)>> {
        let meta = file.metadata()?;
        let len = meta.len();
        // Linux counts `st_blocks` in units of 512 bytes.
        if meta.blocks().saturating_mul(512) >= len {
            return Ok(None);
        }
        // Seeking moves the offset of the open file description, which `file` shares with
        // every duplicate of it. A new description of the same file has an offset of its own.
        let fd = file.as_raw_fd();
        let places = ["thread-self", "self"].map(|of| PathBuf::from(format!("/proc/{of}/fd/{fd}")));
        let Some(walked) = reopened(&meta, places) else {
            return Ok(None);
        };
        let ranges = walk(len, |to| {
            let target = match to {
                To::Data(offset) => SeekFrom::Data(offset),
                To::Hole(offset) => SeekFrom::Hole(offset),
            };
            match rustix::fs::seek(&walked, target) {
                Ok(offset) => Ok(Found::At(offset)),
                Err(Errno::NXIO) => Ok(Found::Nothing),
                Err(e)
                    if [Errno::INVAL, Errno::NOTSUP, Errno::OPNOTSUPP, Errno::SPIPE]
                        .contains(&e) =>
                {
                    Ok(Found::Unknown)
                }
                Err(e) => Err(e.into()),
            }
        })?;
        Ok(ranges.map(|ranges| (len, ranges)))
    }

    /// The file that `meta` describes (by its `fstat`), opened again from the first of `places`
    /// that opens: a new open file description of it, with an offset of its own. The kernel
    /// names an open file of this thread at `/proc/thread-self/fd/N` (since Linux 3.17) and of
    /// this process at `/proc/self/fd/N`; the thread's own table comes first, since a thread can
    /// have a table of its own.
    ///
    /// `None` when no place opens, and when the first one that opens is another file, by device
    /// and inode: a `/proc` that is not the kernel's (in a chroot, say) or a table that is not
    /// this thread's can name one, and its holes are not the file's. The open never blocks
    /// (`O_NONBLOCK`), should a place lead to a FIFO.
    pub(super) fn reopened(
        meta: &Metadata,
        places: impl IntoIterator<Item = impl AsRef<Path>>,
    ) -> Option<File> {
        let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;
        let opened = places
            .into_iter()
            .find_map(|place| rustix::fs::open(place.as_ref(), flags, Mode::empty()).ok())?;
        let opened = File::from(opened);
        let same = opened
            .metadata()
            .is_ok_and(|now| (now.dev(), now.ino()) == (meta.dev(), meta.ino()));
        same.then_some(opened)
    }
}

#[cfg(windows)]
mod imp {
    #![allow(unsafe_code, reason = "FSCTL sparse file FFI")]

    use std::ffi::c_void;
    use std::fs::File;
    use std::io;
    use std::mem::size_of;
    use std::ops::Range;
    use std::os::windows::fs::MetadataExt;
    use std::os::windows::io::AsRawHandle;

    use windows::Win32::Foundation::{ERROR_INVALID_FUNCTION, ERROR_MORE_DATA, HANDLE};
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{
        FILE_ALLOCATED_RANGE_BUFFER, FSCTL_QUERY_ALLOCATED_RANGES, FSCTL_SET_SPARSE,
    };

    use super::allocation_shows_data;
    use crate::win_util::to_io;

    /// How many ranges one `FSCTL_QUERY_ALLOCATED_RANGES` call returns at most.
    const BATCH: usize = 512;

    /// The length of `file` and its allocated ranges, or `None` when they do not show where
    /// its data is or the file system cannot tell.
    pub fn data_ranges(file: &File) -> io::Result<Option<(u64, Vec<Range<u64>>)>> {
        // `GetFileInformationByHandle` of the open file: its attributes and its length.
        let meta = file.metadata()?;
        if !allocation_shows_data(meta.file_attributes()) {
            return Ok(None);
        }
        let len = meta.len();
        let handle = HANDLE(file.as_raw_handle());
        let range_size = size_of::<FILE_ALLOCATED_RANGE_BUFFER>();
        let query_size = u32::try_from(range_size).expect("a range is 16 bytes");
        let mut found = vec![FILE_ALLOCATED_RANGE_BUFFER::default(); BATCH];
        let found_size = u32::try_from(BATCH * range_size).expect("the batch is 8 KiB");
        let mut ranges = Vec::new();
        let mut at = 0;
        while at < len {
            let (Ok(offset), Ok(length)) = (i64::try_from(at), i64::try_from(len - at)) else {
                return Ok(None);
            };
            let query = FILE_ALLOCATED_RANGE_BUFFER {
                FileOffset: offset,
                Length: length,
            };
            let mut returned = 0u32;
            // SAFETY: `handle` belongs to `file`, which stays open for the call. `query` is a
            // readable `FILE_ALLOCATED_RANGE_BUFFER` of `query_size` bytes, `found` a writable
            // array of `BATCH` of them, `found_size` bytes in all, and `returned` a writable
            // count, which a call without an `OVERLAPPED` needs.
            let result = unsafe {
                DeviceIoControl(
                    handle,
                    FSCTL_QUERY_ALLOCATED_RANGES,
                    Some(std::ptr::from_ref(&query).cast::<c_void>()),
                    query_size,
                    Some(found.as_mut_ptr().cast::<c_void>()),
                    found_size,
                    Some(&raw mut returned),
                    None,
                )
            };
            let more = match result {
                Ok(()) => false,
                // The ranges that fit are there; the rest come from the next call.
                Err(e) if e.code() == ERROR_MORE_DATA.to_hresult() => true,
                Err(e) if e.code() == ERROR_INVALID_FUNCTION.to_hresult() => return Ok(None),
                Err(e) => return Err(to_io(&e)),
            };
            let count = usize::try_from(returned).map_or(0, |bytes| bytes / range_size);
            for range in found.iter().take(count.min(BATCH)) {
                let (Ok(start), Ok(length)) =
                    (u64::try_from(range.FileOffset), u64::try_from(range.Length))
                else {
                    return Ok(None);
                };
                ranges.push(start..start.saturating_add(length));
            }
            if !more {
                break;
            }
            // The next call starts after the last range returned, which must be past `at`.
            match ranges.last() {
                Some(last) if last.end > at => at = last.end,
                _ => return Ok(None),
            }
        }
        Ok(Some((len, ranges)))
    }

    pub fn mark_sparse(file: &File) -> io::Result<()> {
        let mut returned = 0u32;
        // SAFETY: the handle belongs to `file`, which stays open for the call. Without an input
        // buffer `FSCTL_SET_SPARSE` sets the attribute (as `SetSparse = TRUE` would), it has no
        // output, and `returned` is a writable count, which a call without an `OVERLAPPED`
        // needs.
        unsafe {
            DeviceIoControl(
                HANDLE(file.as_raw_handle()),
                FSCTL_SET_SPARSE,
                None,
                0,
                None,
                0,
                Some(&raw mut returned),
                None,
            )
        }
        .map_err(|e| to_io(&e))
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::ops::Range;

    use super::{Found, SERVED_ELSEWHERE, SPARSE_FILE, To, allocation_shows_data, tidy, walk};

    #[test]
    fn ranges_are_sorted_cut_and_joined() {
        assert_eq!(
            tidy(100, vec![60..70, 0..10, 10..20, 15..18, 30..30, 90..150]),
            Some(vec![0..20, 60..70, 90..100])
        );
        assert_eq!(tidy(100, vec![]), Some(vec![]));
    }

    #[test]
    fn a_file_without_holes_has_no_ranges() {
        assert_eq!(tidy(100, vec![0..50, 50..100]), None);
        assert_eq!(tidy(100, std::iter::once(0..200).collect()), None);
        assert_eq!(tidy(0, vec![]), None);
    }

    /// `SEEK_DATA` and `SEEK_HOLE` on a file of `len` bytes with data in `data`, as a file
    /// system that answers truly does.
    fn honest(data: &[Range<u64>], len: u64) -> impl FnMut(To) -> io::Result<Found> + '_ {
        move |to| {
            Ok(match to {
                To::Data(at) => data
                    .iter()
                    .find(|r| r.end > at)
                    .map_or(Found::Nothing, |r| Found::At(r.start.max(at))),
                To::Hole(at) if at >= len => Found::Nothing,
                To::Hole(at) => {
                    Found::At(data.iter().find(|r| r.contains(&at)).map_or(at, |r| r.end))
                }
            })
        }
    }

    #[test]
    fn a_walk_finds_the_data_and_the_holes_after_it() {
        let data = [0..10, 50..60];
        assert_eq!(walk(100, honest(&data, 100)).unwrap(), Some(data.to_vec()));
        // All hole: the first `SEEK_DATA` finds nothing, and offset 0 starts a hole.
        assert_eq!(walk(100, honest(&[], 100)).unwrap(), Some(vec![]));
    }

    #[test]
    fn no_data_is_believed_only_where_a_hole_starts() {
        // Data the file system keeps elsewhere (decmpfs): "no data" at 0, where `SEEK_HOLE`
        // finds the first hole only at the end of the file.
        let mut lying = |to| match to {
            To::Data(_) => Ok(Found::Nothing),
            To::Hole(_) => Ok(Found::At(100)),
        };
        assert_eq!(walk(100, &mut lying).unwrap(), None);
        // The same after a first range, at an offset that `SEEK_HOLE` itself does not take for
        // the start of a hole.
        let mut lying = |to| match to {
            To::Data(0) => Ok(Found::At(0)),
            To::Hole(0) => Ok(Found::At(10)),
            To::Data(_) => Ok(Found::Nothing),
            To::Hole(at) => Ok(Found::At(at + 1)),
        };
        assert_eq!(walk(100, &mut lying).unwrap(), None);
    }

    #[test]
    fn data_after_offset_0_is_believed_only_after_a_hole_at_0() {
        let data: Vec<Range<u64>> = std::iter::once(20..30).collect();
        assert_eq!(walk(100, honest(&data, 100)).unwrap(), Some(data.clone()));
        // `SEEK_DATA` skips to 20, but `SEEK_HOLE` finds data at 0 that ends at 5.
        let mut lying = {
            let mut truth = honest(&data, 100);
            move |to| match to {
                To::Hole(0) => Ok(Found::At(5)),
                other => truth(other),
            }
        };
        assert_eq!(walk(100, &mut lying).unwrap(), None);
    }

    #[test]
    fn answers_that_cannot_be_trusted_give_no_ranges() {
        assert_eq!(walk(100, |_| Ok(Found::Unknown)).unwrap(), None);
        // A hole that does not come after its data.
        assert_eq!(walk(100, |_| Ok(Found::At(5))).unwrap(), None);
        // A file that got shorter between the two seeks.
        let mut shrunk = |to| match to {
            To::Data(at) => Ok(Found::At(at)),
            To::Hole(_) => Ok(Found::Nothing),
        };
        assert_eq!(walk(100, &mut shrunk).unwrap(), None);
        let failing = |_| Err(io::Error::other("seek failed"));
        assert!(walk(100, failing).is_err());
    }

    #[test]
    fn only_plain_sparse_files_show_their_data_in_their_allocation() {
        assert!(allocation_shows_data(SPARSE_FILE));
        // Read-only, hidden, archive: still a plain sparse file.
        assert!(allocation_shows_data(SPARSE_FILE | 0x1 | 0x2 | 0x20));
        assert!(!allocation_shows_data(0));
        for served_elsewhere in [0x400, 0x1000, 0x4_0000, 0x40_0000] {
            assert_eq!(SERVED_ELSEWHERE & served_elsewhere, served_elsewhere);
            assert!(!allocation_shows_data(SPARSE_FILE | served_elsewhere));
        }
    }

    /// The walk runs on a new open of the file it was asked about, never on another file: a
    /// place that opens another one (a `/proc` that is not the kernel's) gives nothing, a place
    /// that does not open is passed over, and the kernel's own places name the open file.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_reopen_that_finds_another_file_is_not_walked() {
        use std::os::fd::AsRawFd;

        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let file = std::fs::File::open(&a).unwrap();
        let meta = file.metadata().unwrap();
        assert!(super::imp::reopened(&meta, [b.clone()]).is_none());
        // The first place that opens decides: another file there is not passed over.
        assert!(super::imp::reopened(&meta, [b, a.clone()]).is_none());
        assert!(super::imp::reopened(&meta, [dir.path().join("missing"), a]).is_some());
        let fd = file.as_raw_fd();
        for place in ["thread-self", "self"] {
            let path = std::path::PathBuf::from(format!("/proc/{place}/fd/{fd}"));
            assert!(super::imp::reopened(&meta, [path]).is_some(), "{place}");
        }
    }

    /// The bits are the ones Windows itself defines.
    #[cfg(windows)]
    #[test]
    fn the_attribute_bits_are_the_windows_ones() {
        use windows::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
            FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_ATTRIBUTE_SPARSE_FILE,
        };

        assert_eq!(SPARSE_FILE, FILE_ATTRIBUTE_SPARSE_FILE.0);
        let served_elsewhere = FILE_ATTRIBUTE_REPARSE_POINT
            | FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_RECALL_ON_OPEN
            | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS;
        assert_eq!(SERVED_ELSEWHERE, served_elsewhere.0);
    }
}
