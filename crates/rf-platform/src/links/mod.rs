//! Making links (OPS-08): the shortcut of each system, the symbolic link, and the junction of
//! Windows, and reading them back to tell what a link points at.
//!
//! - **Shortcut:** on Windows a `.lnk` file (`IShellLinkW` saved through `IPersistFile`); on
//!   macOS a Finder alias, which is a bookmark file (`NSURL` bookmark data written with
//!   `writeBookmarkData:toURL:options:error:` as suitable for a bookmark file). Linux has none.
//! - **Symlink:** `std::os::unix::fs::symlink`, or `symlink_file` and `symlink_dir` on Windows,
//!   which need Developer Mode or an elevated process there ([`symlink_needs_privilege`]).
//! - **Junction:** Windows only, and only to a folder: a folder made with `CreateDirectory`
//!   and given a mount point reparse point ([`mount_point_buffer`]).
//!
//! A link is only ever made where nothing is: [`make_link`] fails with `AlreadyExists` for a
//! path that is taken, whatever it is, and never touches the target. [`remove_link`] removes
//! the link itself and nothing it points to, and only when [`link_points_to`] says what it is.
//! Every function here is for a worker thread, never the window's.
//!
//! The rest of this module is pure, and tested on every system: which kinds a system offers
//! ([`kinds_for`]), the name a link gets ([`link_name`]) and the bytes of a junction
//! ([`mount_point_buffer`], [`mount_point_target`]).

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use rf_core::Os;
use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
use self::macos as imp;
#[cfg(not(any(windows, target_os = "macos")))]
use self::other as imp;
#[cfg(windows)]
use self::windows as imp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(windows, target_os = "macos")))]
mod other;
#[cfg(windows)]
mod windows;

/// The kind of a link. It is part of the job journal's format, so a variant is never renamed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LinkKind {
    /// A `.lnk` file on Windows, a Finder alias file on macOS.
    Shortcut,
    /// A symbolic link.
    Symlink,
    /// A junction (Windows, to a folder only).
    Junction,
}

/// The kinds of link the system `os` makes to a target that is a folder (`target_is_dir`) or a
/// file, in the order a menu lists them: the shortcut (the alias on macOS) first, then the
/// symbolic link, then on Windows the junction, for a folder only. Linux has no shortcut.
pub fn kinds_for(os: Os, target_is_dir: bool) -> Vec<LinkKind> {
    match os {
        Os::Windows if target_is_dir => {
            vec![LinkKind::Shortcut, LinkKind::Symlink, LinkKind::Junction]
        }
        Os::Windows | Os::MacOs => vec![LinkKind::Shortcut, LinkKind::Symlink],
        Os::Other => vec![LinkKind::Symlink],
    }
}

/// The kind of link a drop with the link keys makes on `os` (DND-03): the shortcut on Windows,
/// the alias on macOS, and a symbolic link where there is no shortcut.
pub fn drop_kind(os: Os) -> LinkKind {
    match os {
        Os::Windows | Os::MacOs => LinkKind::Shortcut,
        Os::Other => LinkKind::Symlink,
    }
}

/// The name a link of `kind` to the item called `target_name` gets on `os`, before a number
/// is added for a name that is taken: `이름 - 바로 가기.lnk` for a shortcut on Windows,
/// `이름 별칭` for an alias on macOS, and the target's own name for a symbolic link, a junction
/// and anything else.
pub fn link_name(target_name: &OsStr, kind: LinkKind, os: Os) -> OsString {
    let mut name = target_name.to_owned();
    match (kind, os) {
        (LinkKind::Shortcut, Os::Windows) => name.push(" - 바로 가기.lnk"),
        (LinkKind::Shortcut, Os::MacOs) => name.push(" 별칭"),
        _ => {}
    }
    name
}

/// Makes a link of `kind` called `at` that points to `target`. The target need not exist
/// (a symbolic link and a shortcut then dangle), except for an alias and a junction, which
/// need one. Fails with `AlreadyExists` when anything is at `at`, with `Unsupported` for a kind
/// this system does not make, and with `InvalidInput` for a junction to what is no folder or
/// a target that is not an absolute path. `at` is made in a folder that must exist.
///
/// A symbolic link to a folder on Windows is made as a folder link; every other target gets a
/// file link.
///
/// A symbolic link always points to the absolute path it is given, never to a path relative to
/// its own folder: it keeps working wherever the link is moved to, and stops working if the
/// target (or a folder above it) is moved or renamed, also together with the link. The same holds
/// for a shortcut, an alias and a junction, which keep an absolute path too.
pub fn make_link(kind: LinkKind, target: &Path, at: &Path) -> io::Result<()> {
    if fs::symlink_metadata(at).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    match kind {
        LinkKind::Symlink => make_symlink(target, at),
        LinkKind::Shortcut => imp::make_shortcut(target, at),
        LinkKind::Junction => imp::make_junction(target, at),
    }
}

#[cfg(unix)]
fn make_symlink(target: &Path, at: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, at)
}

#[cfg(windows)]
fn make_symlink(target: &Path, at: &Path) -> io::Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, at)
    } else {
        std::os::windows::fs::symlink_file(target, at)
    }
}

/// What the item at `at` is, if it is a link this module makes, and what it points to: a
/// symbolic link (read with `read_link`), a junction, a `.lnk` file on Windows or an alias file
/// on macOS. `None` for anything else, a file or a folder that is no link, or a `.lnk` or an
/// alias that cannot be read. Fails with `NotFound` for nothing at `at`.
///
/// The target is what the link says, which need not exist and need not be written as the path
/// the link was made with (a shortcut may keep its target's path in another letter case, an
/// alias follows its target to where it was moved): the caller that compares it with a path
/// compares the files too ([`crate::file_identity`]).
pub fn link_points_to(at: &Path) -> io::Result<Option<(LinkKind, PathBuf)>> {
    let meta = fs::symlink_metadata(at)?;
    if meta.file_type().is_symlink() {
        // Windows keeps a junction as a reparse point that `read_link` reads too.
        #[cfg(windows)]
        if let Some(target) = imp::junction_target(at)? {
            return Ok(Some((LinkKind::Junction, target)));
        }
        return Ok(Some((LinkKind::Symlink, fs::read_link(at)?)));
    }
    if meta.is_file()
        && let Some(target) = imp::read_shortcut(at)?
    {
        return Ok(Some((LinkKind::Shortcut, target)));
    }
    Ok(None)
}

/// Removes the link of `kind` at `at`, and only the link: a symbolic link, a junction (its
/// folder is empty of its own, and the target's content is never reached), a `.lnk` or an alias
/// file. Never a folder with something in it, never a recursive delete. The caller has checked
/// what is there ([`link_points_to`]).
pub fn remove_link(at: &Path, kind: LinkKind) -> io::Result<()> {
    match kind {
        // A symbolic link to a folder is a folder entry on Windows, a file entry elsewhere;
        // `remove_dir` on a real folder with content fails, so nothing is lost either way.
        LinkKind::Symlink if cfg!(windows) && is_dir_link(at) => fs::remove_dir(at),
        LinkKind::Junction => fs::remove_dir(at),
        LinkKind::Symlink | LinkKind::Shortcut => fs::remove_file(at),
    }
}

/// Whether the link at `at` is a link to a folder in the way Windows keeps it: an entry that
/// is a folder itself (`FILE_ATTRIBUTE_DIRECTORY`).
#[cfg(windows)]
fn is_dir_link(at: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    fs::symlink_metadata(at)
        .is_ok_and(|meta| meta.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0)
}

#[cfg(not(windows))]
fn is_dir_link(_at: &Path) -> bool {
    false
}

/// Whether a symbolic link cannot be made without more rights: on Windows when Developer Mode
/// is off and the process is not elevated (only the registry and the process token are read);
/// never elsewhere. No job tries a symbolic link then. Reads the system every time it is called.
pub fn symlink_needs_privilege() -> bool {
    imp::symlink_needs_privilege()
}

/// How long [`symlink_needs_privilege_cached`] keeps an answer: long enough that a menu opened
/// again and again reads the system once, short enough that a user who has just turned
/// Developer Mode on finds the entry live after a moment, without starting the app again.
pub const PRIVILEGE_KEPT: Duration = Duration::from_secs(30);

/// [`symlink_needs_privilege`] for a menu, which asks at every opening on the UI thread: the
/// answer of the last [`PRIVILEGE_KEPT`], read from the system when there is none. The first
/// call reads the system, so a worker makes it ahead of the first menu. A job still asks the
/// system itself.
pub fn symlink_needs_privilege_cached() -> bool {
    static LAST: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    kept(
        &LAST,
        Instant::now(),
        PRIVILEGE_KEPT,
        symlink_needs_privilege,
    )
}

/// The answer in `slot` if it is younger than `kept` at `now`; else `read`, which is stored.
fn kept(
    slot: &Mutex<Option<(Instant, bool)>>,
    now: Instant,
    kept: Duration,
    read: impl FnOnce() -> bool,
) -> bool {
    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((at, answer)) = *slot
        && now.saturating_duration_since(at) < kept
    {
        return answer;
    }
    let answer = read();
    *slot = Some((now, answer));
    answer
}

/// `IO_REPARSE_TAG_MOUNT_POINT`.
const MOUNT_POINT_TAG: u32 = 0xA000_0003;

/// The longest path buffer of a reparse point: its data holds at most 16 KiB less its 8-byte
/// header.
const MAX_REPARSE_DATA: usize = 16 * 1024 - 8;

/// The path as UTF-16 units: Windows paths as they are, other systems' as their UTF-8 text.
#[cfg_attr(
    windows,
    allow(
        clippy::unnecessary_wraps,
        reason = "a Windows path is always UTF-16; other systems' paths may not be text"
    )
)]
fn path_units(path: &Path) -> Option<Vec<u16>> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        Some(path.as_os_str().encode_wide().collect())
    }
    #[cfg(not(windows))]
    {
        Some(path.to_str()?.encode_utf16().collect())
    }
}

fn units_of(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// The `REPARSE_DATA_BUFFER` of a junction to `target`, as `FSCTL_SET_REPARSE_POINT` takes it:
/// the tag `IO_REPARSE_TAG_MOUNT_POINT`, the length of the data, and the mount point buffer
/// (offsets and lengths of the substitute name `\??\C:\target` and the print name `C:\target`,
/// then both names, each ended by a NUL unit). Everything is little-endian.
///
/// `target` is the absolute path of a folder on a drive: `C:\dir`, or the same with the
/// extended prefix `\\?\C:\dir`. A relative path, a network path (a junction cannot point
/// there) and a path too long for a reparse point are `InvalidInput`. A closing separator is
/// left out, except for a drive's root.
pub fn mount_point_buffer(target: &Path) -> io::Result<Vec<u8>> {
    let invalid = |why: &'static str| io::Error::new(io::ErrorKind::InvalidInput, why);
    let units = path_units(target).ok_or_else(|| invalid("the path is not valid text"))?;
    let text = String::from_utf16(&units).map_err(|_| invalid("the path is not valid text"))?;
    let plain = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let drive = plain.as_bytes();
    let on_a_drive =
        drive.len() >= 3 && drive[0].is_ascii_alphabetic() && drive[1] == b':' && drive[2] == b'\\';
    if !on_a_drive || plain.starts_with(r"UNC\") {
        return Err(invalid(
            "a junction needs the absolute path of a folder on a drive",
        ));
    }
    let print = if plain.len() > 3 {
        plain.trim_end_matches('\\')
    } else {
        plain
    };
    let substitute = units_of(&format!(r"\??\{print}"));
    let print = units_of(print);
    let names = (substitute.len() + 1 + print.len() + 1) * 2;
    if names > MAX_REPARSE_DATA - 8 {
        return Err(invalid("the path is too long for a junction"));
    }
    let to_u16 = |n: usize| u16::try_from(n).expect("the length was checked against 16 KiB");
    let mut bytes = Vec::with_capacity(16 + names);
    bytes.extend_from_slice(&MOUNT_POINT_TAG.to_le_bytes());
    bytes.extend_from_slice(&to_u16(8 + names).to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&to_u16(substitute.len() * 2).to_le_bytes());
    bytes.extend_from_slice(&to_u16((substitute.len() + 1) * 2).to_le_bytes());
    bytes.extend_from_slice(&to_u16(print.len() * 2).to_le_bytes());
    for unit in substitute.iter().chain([&0]).chain(&print).chain([&0]) {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(bytes)
}

/// The target a `REPARSE_DATA_BUFFER` of a junction names (the other way round of
/// [`mount_point_buffer`], for what `FSCTL_GET_REPARSE_POINT` gives), as UTF-16 units without
/// the `\??\` prefix: the print name when there is one, else the substitute name. `None` for a
/// buffer of another tag (a symbolic link, a cloud placeholder) or one that is cut short or
/// whose offsets point out of it.
pub fn mount_point_target(buffer: &[u8]) -> Option<Vec<u16>> {
    let word = |at: usize| -> Option<usize> {
        let pair = buffer.get(at..at + 2)?;
        Some(usize::from(u16::from_le_bytes([pair[0], pair[1]])))
    };
    let tag = u32::from_le_bytes(buffer.get(0..4)?.try_into().ok()?);
    if tag != MOUNT_POINT_TAG {
        return None;
    }
    let (sub_at, sub_len, print_at, print_len) = (word(8)?, word(10)?, word(12)?, word(14)?);
    let names = buffer.get(16..)?;
    let read = |at: usize, len: usize| -> Option<Vec<u16>> {
        let bytes = names.get(at..at.checked_add(len)?)?;
        Some(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect(),
        )
    };
    let print = read(print_at, print_len).filter(|print| !print.is_empty());
    print.or_else(|| {
        let substitute = read(sub_at, sub_len)?;
        let prefix = units_of(r"\??\");
        Some(substitute.strip_prefix(prefix.as_slice())?.to_vec())
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn an_answer_is_kept_for_a_while_and_read_again_after() {
        let slot = Mutex::new(None);
        let kept_for = Duration::from_secs(30);
        let start = Instant::now();
        let reads = Cell::new(0);
        let reads_ref = &reads;
        let read = |answer: bool| {
            move || {
                reads_ref.set(reads_ref.get() + 1);
                answer
            }
        };
        // The first call reads the system.
        assert!(kept(&slot, start, kept_for, read(true)));
        assert_eq!(reads.get(), 1);
        // A menu opened again soon after gets the same answer without a read, even if the
        // system would say otherwise now.
        for seconds in [0, 1, 29] {
            let now = start + Duration::from_secs(seconds);
            assert!(kept(&slot, now, kept_for, read(false)), "{seconds} s");
        }
        assert_eq!(reads.get(), 1);
        // After the time, the system is asked again, and its new answer is kept.
        let later = start + Duration::from_secs(31);
        assert!(!kept(&slot, later, kept_for, read(false)));
        assert_eq!(reads.get(), 2);
        assert!(!kept(
            &slot,
            later + Duration::from_secs(5),
            kept_for,
            read(true)
        ));
        assert_eq!(reads.get(), 2);
        // The public one agrees with the system on a system that never needs a privilege.
        if !cfg!(windows) {
            assert!(!symlink_needs_privilege_cached());
        }
    }

    #[test]
    fn each_system_offers_its_own_kinds() {
        use LinkKind::{Junction, Shortcut, Symlink};
        assert_eq!(kinds_for(Os::Windows, true), [Shortcut, Symlink, Junction]);
        // A junction points to folders only.
        assert_eq!(kinds_for(Os::Windows, false), [Shortcut, Symlink]);
        for is_dir in [true, false] {
            assert_eq!(kinds_for(Os::MacOs, is_dir), [Shortcut, Symlink]);
            assert_eq!(kinds_for(Os::Other, is_dir), [Symlink]);
        }
    }

    #[test]
    fn a_drop_with_the_link_keys_makes_the_shortcut_of_the_system() {
        assert_eq!(drop_kind(Os::Windows), LinkKind::Shortcut);
        assert_eq!(drop_kind(Os::MacOs), LinkKind::Shortcut);
        assert_eq!(drop_kind(Os::Other), LinkKind::Symlink);
        // What it makes is a kind the system offers.
        for os in [Os::Windows, Os::MacOs, Os::Other] {
            assert!(kinds_for(os, false).contains(&drop_kind(os)), "{os:?}");
        }
    }

    #[test]
    fn a_link_is_named_as_its_system_names_it() {
        let name = |target: &str, kind, os| link_name(OsStr::new(target), kind, os);
        assert_eq!(
            name("보고서.docx", LinkKind::Shortcut, Os::Windows),
            "보고서.docx - 바로 가기.lnk"
        );
        assert_eq!(name("사진", LinkKind::Shortcut, Os::MacOs), "사진 별칭");
        for os in [Os::Windows, Os::MacOs, Os::Other] {
            assert_eq!(name("a.txt", LinkKind::Symlink, os), "a.txt", "{os:?}");
            assert_eq!(name("a", LinkKind::Junction, os), "a", "{os:?}");
        }
        // A system with no shortcut keeps the name.
        assert_eq!(name("a", LinkKind::Shortcut, Os::Other), "a");
    }

    /// The buffer of `C:\t` written out by hand.
    #[test]
    fn a_junctions_buffer_is_laid_out_as_windows_reads_it() {
        let mut want: Vec<u8> = Vec::new();
        // The tag, the length of the data after the header (8 + the two names with their
        // NULs: (8 + 1 + 4 + 1) * 2 = 28), and the reserved word.
        want.extend_from_slice(&[0x03, 0x00, 0x00, 0xA0, 36, 0, 0, 0]);
        // The substitute name at 0 for 16 bytes, the print name at 18 for 8 bytes.
        want.extend_from_slice(&[0, 0, 16, 0, 18, 0, 8, 0]);
        for unit in r"\??\C:\t".encode_utf16().chain([0]) {
            want.extend_from_slice(&unit.to_le_bytes());
        }
        for unit in r"C:\t".encode_utf16().chain([0]) {
            want.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(mount_point_buffer(Path::new(r"C:\t")).unwrap(), want);
    }

    #[test]
    fn a_junctions_buffer_reads_back_to_its_target() {
        for (target, back) in [
            (r"C:\Users\나\문서", r"C:\Users\나\문서"),
            (r"D:\", r"D:\"),
            (r"\\?\C:\a b\c", r"C:\a b\c"),
            // A closing separator is not part of the name.
            (r"C:\dir\", r"C:\dir"),
        ] {
            let buffer = mount_point_buffer(Path::new(target)).unwrap();
            let want: Vec<u16> = back.encode_utf16().collect();
            assert_eq!(mount_point_target(&buffer), Some(want), "{target}");
            // The length in the header is the data after the 8-byte header.
            let length = usize::from(u16::from_le_bytes([buffer[4], buffer[5]]));
            assert_eq!(buffer.len(), 8 + length, "{target}");
        }
    }

    #[test]
    fn a_junction_needs_an_absolute_path_on_a_drive() {
        for target in [
            "",
            "t",
            r"..\t",
            r"\t",
            r"C:t",
            r"\\server\share\t",
            r"\\?\UNC\server\share\t",
        ] {
            let error = mount_point_buffer(Path::new(target)).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{target:?}");
        }
        let long = format!(r"C:\{}", "a".repeat(20_000));
        let error = mount_point_buffer(Path::new(&long)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_buffer_that_is_no_junction_names_no_target() {
        let mut buffer = mount_point_buffer(Path::new(r"C:\t")).unwrap();
        // The symbolic link tag.
        let mut symlink = buffer.clone();
        symlink[..4].copy_from_slice(&0xA000_000Cu32.to_le_bytes());
        assert_eq!(mount_point_target(&symlink), None);
        // Cut short, and with an offset out of the buffer.
        assert_eq!(mount_point_target(&buffer[..10]), None);
        assert_eq!(mount_point_target(&[]), None);
        buffer[12..14].copy_from_slice(&200u16.to_le_bytes());
        buffer[10..12].copy_from_slice(&200u16.to_le_bytes());
        assert_eq!(mount_point_target(&buffer), None);
    }

    #[cfg(unix)]
    mod unix {
        use super::*;

        #[test]
        fn a_symlink_is_made_read_and_removed_and_its_target_stays() {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("target.txt");
            fs::write(&target, "kept").unwrap();
            let at = dir.path().join("link");
            make_link(LinkKind::Symlink, &target, &at).unwrap();
            assert_eq!(
                link_points_to(&at).unwrap(),
                Some((LinkKind::Symlink, target.clone()))
            );
            // Nothing is made over what is there, a link or not.
            for taken in [&at, &target] {
                let error = make_link(LinkKind::Symlink, &target, taken).unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            }
            remove_link(&at, LinkKind::Symlink).unwrap();
            assert_eq!(fs::read_to_string(&target).unwrap(), "kept");
            assert_eq!(
                link_points_to(&at).unwrap_err().kind(),
                io::ErrorKind::NotFound
            );
        }

        #[test]
        fn a_symlink_to_nothing_is_made() {
            let dir = tempfile::tempdir().unwrap();
            let at = dir.path().join("link");
            make_link(LinkKind::Symlink, &dir.path().join("missing"), &at).unwrap();
            let (kind, _) = link_points_to(&at).unwrap().unwrap();
            assert_eq!(kind, LinkKind::Symlink);
        }

        #[test]
        fn a_file_and_a_folder_are_no_links() {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("f");
            fs::write(&file, "x").unwrap();
            assert_eq!(link_points_to(&file).unwrap(), None);
            assert_eq!(link_points_to(dir.path()).unwrap(), None);
        }

        #[test]
        fn a_symlink_to_a_folder_is_removed_without_the_folder() {
            let dir = tempfile::tempdir().unwrap();
            let folder = dir.path().join("folder");
            fs::create_dir(&folder).unwrap();
            fs::write(folder.join("inner"), "x").unwrap();
            let at = dir.path().join("link");
            make_link(LinkKind::Symlink, &folder, &at).unwrap();
            remove_link(&at, LinkKind::Symlink).unwrap();
            assert!(folder.join("inner").exists());
        }

        #[test]
        fn kinds_this_system_does_not_make_are_unsupported() {
            let dir = tempfile::tempdir().unwrap();
            let at = dir.path().join("link");
            let error = make_link(LinkKind::Junction, dir.path(), &at).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Unsupported);
            assert!(!symlink_needs_privilege());
            if !cfg!(target_os = "macos") {
                let error = make_link(LinkKind::Shortcut, dir.path(), &at).unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::Unsupported);
            }
        }
    }

    /// Needs Explorer's shell and a desktop session (manual QA in `docs/ux/keyboard.md`).
    #[cfg(windows)]
    #[test]
    #[ignore = "needs an interactive session"]
    fn a_shortcut_and_a_junction_are_made_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("folder");
        fs::create_dir(&folder).unwrap();
        let shortcut = dir.path().join("folder - 바로 가기.lnk");
        make_link(LinkKind::Shortcut, &folder, &shortcut).unwrap();
        let (kind, target) = link_points_to(&shortcut).unwrap().unwrap();
        assert_eq!(kind, LinkKind::Shortcut);
        assert_eq!(
            fs::canonicalize(target).unwrap(),
            fs::canonicalize(&folder).unwrap()
        );
        let junction = dir.path().join("junction");
        make_link(LinkKind::Junction, &folder, &junction).unwrap();
        let (kind, _) = link_points_to(&junction).unwrap().unwrap();
        assert_eq!(kind, LinkKind::Junction);
        remove_link(&junction, LinkKind::Junction).unwrap();
        remove_link(&shortcut, LinkKind::Shortcut).unwrap();
        assert!(folder.exists());
    }

    /// Needs Finder's bookmark services and a desktop session.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs an interactive session"]
    fn an_alias_is_made_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        fs::write(&target, "x").unwrap();
        let alias = dir.path().join("target.txt 별칭");
        make_link(LinkKind::Shortcut, &target, &alias).unwrap();
        let (kind, found) = link_points_to(&alias).unwrap().unwrap();
        assert_eq!(kind, LinkKind::Shortcut);
        assert_eq!(
            fs::canonicalize(found).unwrap(),
            fs::canonicalize(&target).unwrap()
        );
        remove_link(&alias, LinkKind::Shortcut).unwrap();
        assert!(target.exists());
    }
}
