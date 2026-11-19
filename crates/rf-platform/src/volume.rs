//! Free space and file-system facts of the volume that holds a path.

use std::path::PathBuf;

use rf_core::{Error, Result, VPath};

/// Byte counts of one volume.
///
/// Only `available <= free` and `available <= total` are guaranteed. `free <= total` is not:
/// on Windows with per-user quotas, `free` counts the whole volume while `total` counts only
/// the part the current user may use. Callers that want the space left before a copy use
/// `available`, and must not compute `total - free`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskSpace {
    /// Size of the volume; on Windows with per-user quotas, the part of it the current user
    /// may use.
    pub total: u64,
    /// Free bytes on the whole volume.
    pub free: u64,
    /// Free bytes the current user can use. It is smaller than `free` when space is held back
    /// for someone else: root-reserved blocks on Unix (`f_bavail < f_bfree`), or disk quotas
    /// on Windows.
    pub available: u64,
}

/// What the file system of a volume says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct VolumeInfo {
    /// The file-system name: `NTFS`, `FAT32`, `exFAT` or `ReFS` on Windows; `apfs`, `hfs`,
    /// `msdos`, `exfat` or `smbfs` on macOS; `None` on Linux. Size-limit checks (a FAT32 file
    /// cannot exceed 4 GiB) start from this value.
    pub fs_name: Option<String>,
    /// Whether the volume is mounted read-only.
    pub read_only: bool,
}

/// The space on the volume that holds `path`, which may name a file.
///
/// A path that does not exist gives `ErrorKind::NotFound`, and a non-local one gives
/// `Error::Unsupported`.
pub fn disk_space(path: &VPath) -> Result<DiskSpace> {
    let local = existing_local("disk_space", path)?;
    imp::disk_space(path, &local)
}

/// The file-system facts of the volume that holds `path`, which may name a file.
///
/// A path that does not exist gives `ErrorKind::NotFound`, and a non-local one gives
/// `Error::Unsupported`.
pub fn volume_info(path: &VPath) -> Result<VolumeInfo> {
    let local = existing_local("volume_info", path)?;
    imp::volume_info(path, &local)
}

/// What a [`MountedRoot`] is, for the words a window gives it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RootKind {
    /// `/`, the system's own volume, on macOS and Linux.
    System,
    /// A Windows drive, by its letter (`C`).
    Letter(char),
    /// Another mounted volume, by the name of the folder it is mounted on (`/Volumes/USB` is
    /// `USB`).
    Volume(std::ffi::OsString),
}

/// The top folder of a drive or mounted volume.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MountedRoot {
    pub path: VPath,
    pub kind: RootKind,
}

/// The drives and mounted volumes, in this order:
///
/// - Windows: every drive letter whose root can be read.
/// - macOS: `/`, then the folders of `/Volumes` that are other volumes than `/` (which skips the
///   link to the system volume there).
/// - Linux: `/`, then the folders of `/media/*`, `/run/media/*` and `/mnt` that are other volumes
///   than `/`, each volume once, under its folder's name.
///
/// Folders whose names start with a dot (`/Volumes/.timemachine`) are the system's own and are
/// left out. This blocks on a slow or sleeping drive: call it on a worker thread.
pub fn mounted_roots() -> Vec<MountedRoot> {
    roots::mounted_roots()
}

#[cfg(windows)]
mod roots {
    use rf_core::VPath;

    use super::{MountedRoot, RootKind};

    pub(super) fn mounted_roots() -> Vec<MountedRoot> {
        ('A'..='Z')
            .filter_map(|letter| {
                let root = format!("{letter}:\\");
                std::fs::metadata(&root).ok()?;
                Some(MountedRoot {
                    path: VPath::local(&root).ok()?,
                    kind: RootKind::Letter(letter),
                })
            })
            .collect()
    }
}

#[cfg(unix)]
mod roots {
    use std::collections::HashSet;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    use rf_core::VPath;

    use super::{MountedRoot, RootKind};

    /// The folders that may hold other volumes than `/`, as globs of one or two levels.
    #[cfg(target_os = "macos")]
    const MOUNT_FOLDERS: &[(&str, usize)] = &[("/Volumes", 1)];
    #[cfg(not(target_os = "macos"))]
    const MOUNT_FOLDERS: &[(&str, usize)] = &[("/media", 2), ("/run/media", 2), ("/mnt", 1)];

    pub(super) fn mounted_roots() -> Vec<MountedRoot> {
        let mut roots = Vec::new();
        let mut seen = HashSet::new();
        let root = Path::new("/");
        if let Ok(meta) = std::fs::metadata(root) {
            seen.insert(meta.dev());
        }
        if let Ok(path) = VPath::local(root) {
            roots.push(MountedRoot {
                path,
                kind: RootKind::System,
            });
        }
        for &(base, depth) in MOUNT_FOLDERS {
            for folder in folders_at(Path::new(base), depth) {
                // A folder on the device of a volume already listed is no mount of its own; the
                // device also tells a volume reached under two paths.
                let Ok(meta) = std::fs::metadata(&folder) else {
                    continue;
                };
                if !meta.is_dir() || !seen.insert(meta.dev()) {
                    continue;
                }
                let name = folder
                    .file_name()
                    .map(ToOwned::to_owned)
                    .unwrap_or_default();
                if let Ok(path) = VPath::local(&folder) {
                    roots.push(MountedRoot {
                        path,
                        kind: RootKind::Volume(name),
                    });
                }
            }
        }
        roots
    }

    /// The folders `depth` levels below `base`, sorted by path, without those whose names start
    /// with a dot.
    pub(super) fn folders_at(base: &Path, depth: usize) -> Vec<PathBuf> {
        let mut level = vec![base.to_path_buf()];
        for _ in 0..depth {
            level = level
                .iter()
                .filter_map(|folder| std::fs::read_dir(folder).ok())
                .flatten()
                .filter_map(Result::ok)
                .filter(|child| !child.file_name().as_encoded_bytes().starts_with(b"."))
                .map(|child| child.path())
                .collect();
        }
        level.sort();
        level
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn mount_folders_skip_dot_folders() {
            let t = tempfile::tempdir().expect("a temporary folder");
            for name in ["user/USB", "user/.hidden", ".timemachine/x", "Data"] {
                std::fs::create_dir_all(t.path().join(name)).expect("a folder");
            }
            assert_eq!(
                folders_at(t.path(), 1),
                [t.path().join("Data"), t.path().join("user")]
            );
            assert_eq!(folders_at(t.path(), 2), [t.path().join("user/USB")]);
        }

        #[test]
        fn the_system_volume_comes_first_and_each_volume_once() {
            let roots = mounted_roots();
            assert_eq!(
                roots[0],
                MountedRoot {
                    path: VPath::local("/").unwrap(),
                    kind: RootKind::System
                }
            );
            for (i, root) in roots.iter().enumerate() {
                assert!(
                    roots[..i].iter().all(|other| other.path != root.path),
                    "{roots:?}"
                );
            }
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod roots {
    use super::MountedRoot;

    pub(super) fn mounted_roots() -> Vec<MountedRoot> {
        Vec::new()
    }
}

/// The form of `path` to hand to the OS, after checking that the item exists.
///
/// The check matters on Windows, where `GetVolumePathNameW` returns a root even for a path
/// that does not exist. It does not follow a final link, so a dangling link is still found.
fn existing_local(op: &'static str, path: &VPath) -> Result<PathBuf> {
    let Some(local) = path.to_extended_length() else {
        return Err(Error::Unsupported(format!(
            "{op} needs a local path: {path}"
        )));
    };
    std::fs::symlink_metadata(&local).map_err(|e| Error::io(op, path.clone(), e))?;
    Ok(local)
}

#[cfg(unix)]
mod imp {
    use std::path::Path;

    use rf_core::{Error, Result, VPath};
    use rustix::fs::{StatVfsMountFlags, statvfs};

    use super::{DiskSpace, VolumeInfo};

    pub fn disk_space(path: &VPath, local: &Path) -> Result<DiskSpace> {
        let stat = statvfs(local).map_err(|e| Error::io("disk_space", path.clone(), e.into()))?;
        Ok(DiskSpace {
            total: stat.f_blocks.saturating_mul(stat.f_frsize),
            free: stat.f_bfree.saturating_mul(stat.f_frsize),
            available: stat.f_bavail.saturating_mul(stat.f_frsize),
        })
    }

    pub fn volume_info(path: &VPath, local: &Path) -> Result<VolumeInfo> {
        let fail = |e: rustix::io::Errno| Error::io("volume_info", path.clone(), e.into());
        let stat = statvfs(local).map_err(fail)?;
        #[cfg(target_os = "macos")]
        let fs_name = Some(type_name(local).map_err(fail)?);
        #[cfg(not(target_os = "macos"))]
        let fs_name = None;
        Ok(VolumeInfo {
            fs_name,
            read_only: stat.f_flag.contains(StatVfsMountFlags::RDONLY),
        })
    }

    /// The file-system type that `statfs` reports, such as `apfs`.
    #[cfg(target_os = "macos")]
    fn type_name(local: &Path) -> rustix::io::Result<String> {
        let stat = rustix::fs::statfs(local)?;
        let bytes: Vec<u8> = stat
            .f_fstypename
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| u8::from_ne_bytes(c.to_ne_bytes()))
            .collect();
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[cfg(windows)]
mod imp {
    #![allow(unsafe_code, reason = "volume query FFI")]

    use std::path::Path;

    use rf_core::winpath::{MAX_EXTENDED_PATH, strip_extended_length};
    use rf_core::{Error, Result, VPath};
    use windows::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetVolumeInformationW, GetVolumePathNameW,
    };
    use windows::core::PCWSTR;

    use super::{DiskSpace, VolumeInfo};
    use crate::win_util::{to_io, wide};

    /// `FILE_READ_ONLY_VOLUME` from `GetVolumeInformationW`; kept here so the feature module
    /// that defines it (`Win32_System_SystemServices`) stays out of the build.
    const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;

    /// Length of the buffer for the file-system name (`MAX_PATH + 1`).
    const FS_NAME_LEN: usize = 261;

    /// The root of the volume that holds `local`, such as `C:\` or `\\srv\share\`, without the
    /// `\\?\` prefix and ending in `0`: the form that the volume APIs take.
    pub(crate) fn volume_root(local: &Path) -> std::io::Result<Vec<u16>> {
        let path = wide(local.as_os_str());
        let mut buf = vec![0_u16; MAX_EXTENDED_PATH];
        // SAFETY: `path` is NUL-terminated and outlives the call, and `buf` is a writable
        // buffer whose length the binding passes along.
        unsafe { GetVolumePathNameW(PCWSTR(path.as_ptr()), &mut buf) }.map_err(|e| to_io(&e))?;
        let len = buf.iter().position(|&unit| unit == 0).unwrap_or(buf.len());
        let mut root = strip_extended_length(&buf[..len]);
        root.push(0);
        Ok(root)
    }

    pub fn disk_space(path: &VPath, local: &Path) -> Result<DiskSpace> {
        let fail = |e: std::io::Error| Error::io("disk_space", path.clone(), e);
        let root = volume_root(local).map_err(fail)?;
        let (mut available, mut total, mut free) = (0_u64, 0_u64, 0_u64);
        // SAFETY: `root` is NUL-terminated and outlives the call, and the three pointers
        // address live, writable `u64` values.
        unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(root.as_ptr()),
                Some(&raw mut available),
                Some(&raw mut total),
                Some(&raw mut free),
            )
        }
        .map_err(|e| fail(to_io(&e)))?;
        Ok(DiskSpace {
            total,
            free,
            available,
        })
    }

    pub fn volume_info(path: &VPath, local: &Path) -> Result<VolumeInfo> {
        let fail = |e: std::io::Error| Error::io("volume_info", path.clone(), e);
        let root = volume_root(local).map_err(fail)?;
        let mut flags = 0_u32;
        let mut name = [0_u16; FS_NAME_LEN];
        // SAFETY: `root` is NUL-terminated and outlives the call, `flags` is a live, writable
        // `u32`, and `name` is a writable buffer whose length the binding passes along.
        unsafe {
            GetVolumeInformationW(
                PCWSTR(root.as_ptr()),
                None,
                None,
                None,
                Some(&raw mut flags),
                Some(&mut name[..]),
            )
        }
        .map_err(|e| fail(to_io(&e)))?;
        let len = name
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(name.len());
        Ok(VolumeInfo {
            fs_name: Some(String::from_utf16_lossy(&name[..len])),
            read_only: flags & FILE_READ_ONLY_VOLUME != 0,
        })
    }
}

#[cfg(windows)]
pub(crate) use imp::volume_root;
