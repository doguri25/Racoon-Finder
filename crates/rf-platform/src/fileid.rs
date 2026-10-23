//! Stable identity of a file: the same value before and after a rename, and for every hard link.

use rf_core::{Error, FileId, Result, VPath, VolumeId};

/// The volume and file ID of the item at `path`, without following a final link.
///
/// The pair stays the same when the item is renamed or moved within its volume, so a record can
/// find the item again. A non-local `path` gives `Error::Unsupported`.
pub fn file_identity(path: &VPath) -> Result<(VolumeId, FileId)> {
    let Some(local) = path.to_extended_length() else {
        return Err(Error::Unsupported(format!(
            "file identity needs a local path: {path}"
        )));
    };
    imp::identity(path, &local)
}

#[cfg(unix)]
mod imp {
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    use rf_core::{Error, FileId, Result, VPath, VolumeId};

    pub fn identity(path: &VPath, local: &Path) -> Result<(VolumeId, FileId)> {
        let meta = std::fs::symlink_metadata(local)
            .map_err(|e| Error::io("file_identity", path.clone(), e))?;
        Ok((VolumeId(meta.dev()), FileId(u128::from(meta.ino()))))
    }
}

#[cfg(windows)]
mod imp {
    #![allow(unsafe_code, reason = "GetFileInformationByHandleEx FFI")]

    use std::ffi::c_void;
    use std::fs::{File, OpenOptions};
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
        match id_info(handle) {
            Ok(identity) => Ok(identity),
            // FAT32 and exFAT have no 128-bit IDs; the 64-bit file index still identifies the file.
            Err(_) => {
                file_index(&file).map_err(|e| Error::io("file_identity", path.clone(), to_io(&e)))
            }
        }
    }

    /// The 128-bit ID of NTFS and `ReFS`.
    fn id_info(handle: HANDLE) -> windows::core::Result<(VolumeId, FileId)> {
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
            VolumeId(info.VolumeSerialNumber),
            FileId(u128::from_le_bytes(info.FileId.Identifier)),
        ))
    }

    /// The 32-bit volume serial number and 64-bit file index every Windows file system has.
    fn file_index(file: &File) -> windows::core::Result<(VolumeId, FileId)> {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the handle belongs to `file`, which is open for the duration of the call, and
        // `info` is a writable `BY_HANDLE_FILE_INFORMATION`.
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &raw mut info)? };
        Ok((
            VolumeId(u64::from(info.dwVolumeSerialNumber)),
            FileId((u128::from(info.nFileIndexHigh) << 32) | u128::from(info.nFileIndexLow)),
        ))
    }
}
