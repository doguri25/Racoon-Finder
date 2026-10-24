//! Maps OS file flags onto the platform-neutral [`Attributes`].

use std::ffi::OsStr;
use std::fs::Metadata;

use rf_core::Attributes;

/// Windows `FILE_ATTRIBUTE_*` bits and the attribute each one sets. `RECALL_ON_OPEN` and
/// `RECALL_ON_DATA_ACCESS` (cloud placeholders) count as `OFFLINE` like the classic
/// `FILE_ATTRIBUTE_OFFLINE`; every other bit (directory, normal, temporary, ...) is ignored.
const WINDOWS_BITS: [(u32, Attributes); 11] = [
    (0x1, Attributes::READONLY),
    (0x2, Attributes::HIDDEN),
    (0x4, Attributes::SYSTEM),
    (0x20, Attributes::ARCHIVE),
    (0x800, Attributes::COMPRESSED),
    (0x4000, Attributes::ENCRYPTED),
    (0x200, Attributes::SPARSE),
    (0x1000, Attributes::OFFLINE),
    (0x4_0000, Attributes::OFFLINE),
    (0x40_0000, Attributes::OFFLINE),
    (0x400, Attributes::REPARSE_POINT),
];

/// macOS `st_flags` bits (`UF_IMMUTABLE`, `UF_COMPRESSED`, `UF_HIDDEN`, `SF_DATALESS`) and the
/// attribute each one sets.
const BSD_FLAGS: [(u32, Attributes); 4] = [
    (0x2, Attributes::READONLY),
    (0x20, Attributes::COMPRESSED),
    (0x8000, Attributes::HIDDEN),
    (0x4000_0000, Attributes::OFFLINE),
];

fn map_bits(bits: u32, table: &[(u32, Attributes)]) -> Attributes {
    table
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .fold(Attributes::empty(), |acc, (_, attribute)| acc | *attribute)
}

/// Maps Windows `dwFileAttributes` bits to [`Attributes`]; bits without an equivalent are ignored.
pub fn attributes_from_windows(bits: u32) -> Attributes {
    map_bits(bits, &WINDOWS_BITS)
}

/// Maps macOS `st_flags` (BSD file flags) to [`Attributes`]; other flags are ignored.
pub fn attributes_from_bsd_flags(flags: u32) -> Attributes {
    map_bits(flags, &BSD_FLAGS)
}

/// Whether `name` starts with a dot, which hides it on Unix systems.
pub fn is_dot_file(name: &OsStr) -> bool {
    // The encoded bytes keep ASCII as is on every OS, so a leading `.` is the first byte.
    name.as_encoded_bytes().first() == Some(&b'.')
}

/// The attributes of the entry `name` whose metadata is `meta` (taken without following links).
///
/// Windows reports hidden files through an attribute and a leading dot means nothing there.
/// Elsewhere a leading dot hides the entry, on top of the macOS `UF_HIDDEN` flag.
pub fn entry_attributes(name: &OsStr, meta: &Metadata) -> Attributes {
    let mut attributes = native_attributes(meta);
    if cfg!(not(windows)) && is_dot_file(name) {
        attributes |= Attributes::HIDDEN;
    }
    attributes
}

#[cfg(windows)]
fn native_attributes(meta: &Metadata) -> Attributes {
    use std::os::windows::fs::MetadataExt;

    attributes_from_windows(meta.file_attributes())
}

#[cfg(target_os = "macos")]
fn native_attributes(meta: &Metadata) -> Attributes {
    use std::os::macos::fs::MetadataExt;

    let mut attributes = attributes_from_bsd_flags(meta.st_flags());
    if meta.permissions().readonly() {
        attributes |= Attributes::READONLY;
    }
    attributes
}

#[cfg(not(any(windows, target_os = "macos")))]
fn native_attributes(meta: &Metadata) -> Attributes {
    if meta.permissions().readonly() {
        Attributes::READONLY
    } else {
        Attributes::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_bits_map_to_attributes() {
        assert_eq!(
            attributes_from_windows(0x2 | 0x4),
            Attributes::HIDDEN | Attributes::SYSTEM
        );
        let cases = [
            (0x1, Attributes::READONLY),
            (0x20, Attributes::ARCHIVE),
            (0x800, Attributes::COMPRESSED),
            (0x4000, Attributes::ENCRYPTED),
            (0x200, Attributes::SPARSE),
            (0x1000, Attributes::OFFLINE),
            (0x4_0000, Attributes::OFFLINE),
            (0x40_0000, Attributes::OFFLINE),
            (0x400, Attributes::REPARSE_POINT),
        ];
        for (bits, want) in cases {
            assert_eq!(attributes_from_windows(bits), want, "{bits:#x}");
        }
        assert_eq!(attributes_from_windows(0x10 | 0x80), Attributes::empty());
    }

    #[test]
    fn bsd_flags_map_to_attributes() {
        assert_eq!(attributes_from_bsd_flags(0x8000), Attributes::HIDDEN);
        assert_eq!(attributes_from_bsd_flags(0x2), Attributes::READONLY);
        assert_eq!(attributes_from_bsd_flags(0x20), Attributes::COMPRESSED);
        assert_eq!(attributes_from_bsd_flags(0x4000_0000), Attributes::OFFLINE);
        assert_eq!(attributes_from_bsd_flags(0), Attributes::empty());
    }

    #[test]
    fn dot_files() {
        assert!(is_dot_file(OsStr::new(".bashrc")) && is_dot_file(OsStr::new("..x")));
        assert!(!is_dot_file(OsStr::new("a.txt")) && !is_dot_file(OsStr::new("")));
    }
}
