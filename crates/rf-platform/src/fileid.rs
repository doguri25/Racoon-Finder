//! Stable identity of a file: the same value before and after a rename, and for every hard link.

use std::fs::Metadata;

use rf_core::{Error, FileId, Result, VPath, VolumeId};

/// The volume and file ID of the item at `path`, without following a final link.
///
/// Where the file system keeps stable file IDs (NTFS, `ReFS`, APFS, ext4 and the like), the
/// pair stays the same when the item is renamed or moved within its volume, so a record can
/// find the item again. FAT32 and exFAT keep no IDs: the 64-bit file index Windows reports for
/// them comes from the cluster and the position of the directory entry, and macOS `msdosfs`
/// and `exfat` derive their inode numbers the same way, so there the pair can change when the
/// item is moved or renamed to a longer name. A caller that finds an item again by its identity
/// (OPS-12 undo) must not assume more than the file system keeps.
///
/// A file system that reports no usable ID gives `Error::Unsupported` naming `path`: an ID of 0,
/// which file systems without IDs report (some network shares and FUSE file systems give it
/// for every file), or all ones, Windows' `FILE_INVALID_FILE_ID`. Such an ID would make every
/// item look like the same one. A non-local `path` gives `Error::Unsupported` too.
pub fn file_identity(path: &VPath) -> Result<(VolumeId, FileId)> {
    let Some(local) = path.to_extended_length() else {
        return Err(Error::Unsupported(format!(
            "file identity needs a local path: {path}"
        )));
    };
    imp::identity(path, &local)
}

/// The identity stored in `meta`, metadata read without following a final link, when it holds
/// a usable one.
///
/// On Unix that is `st_dev` and `st_ino`, by the rules of `file_identity`: inode 0 gives
/// `None`. Windows metadata carries no file ID, so there the answer is always `None` and only
/// `file_identity`, which opens the item, can tell.
pub fn metadata_identity(meta: &Metadata) -> Option<(VolumeId, FileId)> {
    imp::metadata_identity(meta)
}

/// `raw`, a file ID as the OS reports it, if it tells files apart: 0 and all ones (in the
/// 64-bit or the 128-bit width) are what file systems without IDs report, so they give `None`.
fn usable_file_id(raw: u128) -> Option<FileId> {
    (raw != 0 && raw != u128::from(u64::MAX) && raw != u128::MAX).then_some(FileId(raw))
}

/// The identity of `path` from its volume ID and raw file ID, or `Error::Unsupported` when the
/// file ID is not usable.
fn identity_from_raw(path: &VPath, volume: u64, raw: u128) -> Result<(VolumeId, FileId)> {
    usable_file_id(raw)
        .map(|file| (VolumeId(volume), file))
        .ok_or_else(|| {
            Error::Unsupported(format!(
                "the file system reports no file identity for {path}"
            ))
        })
}

#[cfg(unix)]
mod imp {
    use std::fs::Metadata;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    use rf_core::{Error, FileId, Result, VPath, VolumeId};

    use super::{identity_from_raw, usable_file_id};

    pub fn identity(path: &VPath, local: &Path) -> Result<(VolumeId, FileId)> {
        let meta = std::fs::symlink_metadata(local)
            .map_err(|e| Error::io("file_identity", path.clone(), e))?;
        identity_from_raw(path, meta.dev(), u128::from(meta.ino()))
    }

    pub fn metadata_identity(meta: &Metadata) -> Option<(VolumeId, FileId)> {
        usable_file_id(u128::from(meta.ino())).map(|file| (VolumeId(meta.dev()), file))
    }
}

#[cfg(windows)]
mod imp {
    #![allow(unsafe_code, reason = "GetFileInformationByHandleEx FFI")]

    use std::ffi::c_void;
    use std::fs::{File, Metadata, OpenOptions};
    use std::mem::size_of;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    use rf_core::{Error, FileId, Result, VPath, VolumeId};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
    };

    use super::identity_from_raw;
    use crate::win_util::to_io;

    pub fn identity(path: &VPath, local: &Path) -> Result<(VolumeId, FileId)> {
        // Attribute-only access with full sharing opens files that are in use; backup semantics
        // let a folder open; the reparse flag keeps a final link from being followed.
        let file = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES.0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(local)
            .map_err(|e| Error::io("file_identity", path.clone(), e))?;
        let handle = HANDLE(file.as_raw_handle());
        let (volume, raw) = match id_info(handle) {
            Ok(raw) => raw,
            // FAT32 and exFAT have no 128-bit IDs; the 64-bit file index stands in for one.
            Err(_) => file_index(&file)
                .map_err(|e| Error::io("file_identity", path.clone(), to_io(&e)))?,
        };
        // An all-zero ID, from either call, identifies nothing.
        identity_from_raw(path, volume, raw)
    }

    /// Windows metadata has no file ID; only an open handle can tell.
    pub fn metadata_identity(_meta: &Metadata) -> Option<(VolumeId, FileId)> {
        None
    }

    /// The volume serial number and 128-bit file ID of NTFS and `ReFS`, as reported.
    fn id_info(handle: HANDLE) -> windows::core::Result<(u64, u128)> {
        let mut info = FILE_ID_INFO::default();
        let size = u32::try_from(size_of::<FILE_ID_INFO>()).expect("FILE_ID_INFO is 24 bytes");
        // SAFETY: `handle` is open for the duration of the call, and `info` is a writable
        // `FILE_ID_INFO` of exactly `size` bytes, which is what `FileIdInfo` fills in.
        unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                std::ptr::from_mut(&mut info).cast::<c_void>(),
                size,
            )?;
        }
        Ok((
            info.VolumeSerialNumber,
            u128::from_le_bytes(info.FileId.Identifier),
        ))
    }

    /// The 32-bit volume serial number and 64-bit file index every Windows file system has, as
    /// reported.
    fn file_index(file: &File) -> windows::core::Result<(u64, u128)> {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the handle belongs to `file`, which is open for the duration of the call, and
        // `info` is a writable `BY_HANDLE_FILE_INFORMATION`.
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &raw mut info)? };
        Ok((
            u64::from(info.dwVolumeSerialNumber),
            (u128::from(info.nFileIndexHigh) << 32) | u128::from(info.nFileIndexLow),
        ))
    }
}

#[cfg(test)]
mod tests {
    use rf_core::{ErrorKind, FileId, VPath, VolumeId};

    use super::{identity_from_raw, usable_file_id};

    #[test]
    fn zero_and_all_ones_ids_identify_nothing() {
        // Inode 0, a zero 64-bit file index, an all-zero 128-bit `FileIdInfo` ID, and Windows'
        // `FILE_INVALID_FILE_ID` in its 64-bit and 128-bit widths.
        for raw in [0, u128::from(u64::MAX), u128::MAX] {
            assert_eq!(usable_file_id(raw), None, "{raw:#x}");
        }
    }

    #[test]
    fn other_ids_are_kept_as_they_are() {
        for raw in [
            1,
            2,
            0x1_0000_0000,
            u128::from(u64::MAX) - 1,
            1 << 64,
            u128::MAX - 1,
        ] {
            assert_eq!(usable_file_id(raw), Some(FileId(raw)), "{raw:#x}");
        }
    }

    #[test]
    fn a_raw_identity_without_a_usable_id_is_unsupported() {
        let path = VPath::local(std::env::temp_dir().join("no-id")).unwrap();
        let e = identity_from_raw(&path, 7, 0).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Unsupported);
        let message = e.to_string();
        assert!(message.contains(&path.to_string()), "{message}");
        assert!(message.contains("reports no file identity"), "{message}");
        assert_eq!(
            identity_from_raw(&path, 7, 42).unwrap(),
            (VolumeId(7), FileId(42))
        );
    }
}
