//! Sparse files: where a file holds data and where it has holes, so that a copy can keep the
//! holes instead of writing their zeros (XFER-11).

use std::fs::File;
use std::ops::Range;

use rf_core::{Error, Result, VPath};

/// The ranges of the file at `path` that hold data, in ascending order and not overlapping.
/// The gaps between them, and before the first and after the last up to the file's length,
/// are holes: they read as zeros and take no space on the disk.
///
/// `Ok(None)` means there are no holes to keep:
/// - Unix: a file whose allocated blocks (`st_blocks` × 512) cover its length has none, and is
///   not looked at further. Otherwise `lseek` with `SEEK_DATA` and `SEEK_HOLE` walks the file
///   until `ENXIO`. A file system that does not know them (`EINVAL`, `ENOTSUP`) gives `None`.
///   APFS keeps holes; HFS+ does not (it allocates every range a file is extended by and fills
///   it with zeros), so its files never get past the block check.
/// - Windows: a file without the sparse attribute (`Attributes::SPARSE`) has none, and is not
///   opened. For one with it, `FSCTL_QUERY_ALLOCATED_RANGES` is asked until it stops answering
///   `ERROR_MORE_DATA`; a file system that does not know it (`ERROR_INVALID_FUNCTION`) gives
///   `None`.
///
/// One range that covers the whole file is no hole either and gives `None`, as does an answer
/// that does not make sense (a range that goes backwards, a walk that does not advance): a
/// caller then copies every byte, which is always right. A file that is all hole gives an
/// empty list.
///
/// The ranges describe the file as it was when it was looked at: no range reaches past the
/// length it had then. A file that changes afterwards can end before a range does, so a
/// caller that reads the ranges must check that it gets every byte of them. Links are
/// followed, as opening the file follows them. A non-local path gives `Error::Unsupported`;
/// other errors carry the op `"data_ranges"`.
pub fn data_ranges(path: &VPath) -> Result<Option<Vec<Range<u64>>>> {
    let Some(local) = path.to_extended_length() else {
        return Err(Error::Unsupported(format!(
            "data ranges need a local path: {path}"
        )));
    };
    let found = imp::data_ranges(&local).map_err(|e| Error::io("data_ranges", path.clone(), e))?;
    Ok(found.and_then(|(len, ranges)| tidy(len, ranges)))
}

/// Lets the file `file`, open for writing, have holes: on Windows it sets the sparse attribute
/// (`FSCTL_SET_SPARSE`), without which NTFS writes the zeros of every range a write skips.
/// FAT and exFAT refuse it, and the file then cannot have holes.
///
/// Elsewhere it does nothing: a range that is skipped by seeking past it stays a hole wherever
/// the file system keeps holes, and reads as zeros everywhere.
pub fn mark_sparse(file: &File) -> std::io::Result<()> {
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
/// [`data_ranges`] promises: in ascending order, cut at `len`, without empty ones, and joined
/// where they touch or overlap. `None` for a file without holes: an empty one, or one whose
/// ranges cover all of it.
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

#[cfg(unix)]
mod imp {
    use std::fs::File;
    use std::io;
    use std::ops::Range;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    use rustix::fs::SeekFrom;
    use rustix::io::Errno;

    /// The length of the file at `local` and its data ranges as `lseek` finds them, or `None`
    /// when it has no holes or the file system cannot tell.
    pub fn data_ranges(local: &Path) -> io::Result<Option<(u64, Vec<Range<u64>>)>> {
        let meta = std::fs::metadata(local)?;
        let len = meta.len();
        // Linux and macOS count `st_blocks` in units of 512 bytes.
        if meta.blocks().saturating_mul(512) >= len {
            return Ok(None);
        }
        let file = File::open(local)?;
        let mut ranges = Vec::new();
        let mut at = 0;
        while at < len {
            let start = match rustix::fs::seek(&file, SeekFrom::Data(at)) {
                Ok(start) => start,
                // No data from `at` on: the rest of the file is a hole.
                Err(Errno::NXIO) => break,
                Err(e) if unknown(e) => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            let end = match rustix::fs::seek(&file, SeekFrom::Hole(start)) {
                Ok(end) => end,
                // The file got shorter since its data was found: it is copied whole.
                Err(e) if e == Errno::NXIO || unknown(e) => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            // A walk that does not advance cannot be trusted to have found all the data.
            if start < at || end <= start {
                return Ok(None);
            }
            ranges.push(start..end);
            at = end;
        }
        Ok(Some((len, ranges)))
    }

    /// Whether `e` says that the file system cannot seek to data and holes.
    fn unknown(e: Errno) -> bool {
        e == Errno::INVAL || e == Errno::NOTSUP || e == Errno::OPNOTSUPP
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
    use std::path::Path;

    use windows::Win32::Foundation::{ERROR_INVALID_FUNCTION, ERROR_MORE_DATA, HANDLE};
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_SPARSE_FILE;
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{
        FILE_ALLOCATED_RANGE_BUFFER, FSCTL_QUERY_ALLOCATED_RANGES, FSCTL_SET_SPARSE,
    };

    use crate::win_util::to_io;

    /// How many ranges one `FSCTL_QUERY_ALLOCATED_RANGES` call returns at most.
    const BATCH: usize = 512;

    /// The length of the file at `local` and its allocated ranges, or `None` when it is not
    /// sparse or the file system cannot tell.
    pub fn data_ranges(local: &Path) -> io::Result<Option<(u64, Vec<Range<u64>>)>> {
        let meta = std::fs::metadata(local)?;
        if meta.file_attributes() & FILE_ATTRIBUTE_SPARSE_FILE.0 == 0 {
            return Ok(None);
        }
        let len = meta.len();
        let file = File::open(local)?;
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
    use super::tidy;

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
}
