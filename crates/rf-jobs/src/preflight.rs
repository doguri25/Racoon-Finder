//! What a target file system cannot hold (XFER-12): forbidden characters, reserved Windows
//! names, names and paths that are too long, files that are too big, and names that differ
//! only in case. Every rule is a pure function of a name, a size or a path and the kind of file
//! system, so each can be tested on any host and run before a job writes anything.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;

use rf_core::{Os, VPath};
use rf_vfs::NameFolding;

/// The longest local path on Windows, in UTF-16 units. The system accepts 32,767; the rest is
/// kept free for the temporary names a transfer writes next to its target.
pub const WINDOWS_PATH_LIMIT: usize = 32_000;

/// The longest local path on macOS, in UTF-8 bytes (`PATH_MAX` is 1,024 with the final NUL).
pub const MACOS_PATH_LIMIT: usize = 1_023;

/// The longest local path anywhere else, in UTF-8 bytes (`PATH_MAX` is 4,096 with the final NUL).
pub const OTHER_PATH_LIMIT: usize = 4_095;

/// The longest name of one file or folder, in the unit the file system counts (see
/// [`check_name`]).
pub const NAME_LIMIT: usize = 255;

/// The largest file FAT32 can hold: 4 GiB minus one byte.
pub const FAT32_MAX_FILE: u64 = 4_294_967_295;

/// The kind of file system a target lives on, as far as the checks here care.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FsKind {
    Ntfs,
    ReFs,
    /// FAT12, FAT16 and FAT32. Names and times follow FAT; the 4 GiB file limit is applied to
    /// all of them, which is exact for FAT32 and harmless for the smaller ones.
    Fat32,
    ExFat,
    Apfs,
    /// HFS and HFS+.
    HfsPlus,
    /// A network share reached through SMB.
    Smb,
    /// Anything else, and a volume that did not say what it is. It gets no special rule.
    Other,
}

impl FsKind {
    /// The kind for the file-system name a volume reports (`VolumeInfo::fs_name`, or
    /// `Provider::fs_name`). The match ignores ASCII case and is otherwise exact: `NTFS`,
    /// `ReFS`, `exFAT`, `apfs`, `hfs` and `smbfs` name themselves; `FAT`, `FAT12`, `FAT16`,
    /// `FAT32` and `msdos` are [`FsKind::Fat32`]. Every other name, and `None`, is
    /// [`FsKind::Other`].
    pub fn from_fs_name(name: Option<&str>) -> FsKind {
        const NAMES: [(&str, FsKind); 11] = [
            ("NTFS", FsKind::Ntfs),
            ("ReFS", FsKind::ReFs),
            ("FAT", FsKind::Fat32),
            ("FAT12", FsKind::Fat32),
            ("FAT16", FsKind::Fat32),
            ("FAT32", FsKind::Fat32),
            ("msdos", FsKind::Fat32),
            ("exFAT", FsKind::ExFat),
            ("apfs", FsKind::Apfs),
            ("hfs", FsKind::HfsPlus),
            ("smbfs", FsKind::Smb),
        ];
        name.and_then(|name| {
            NAMES
                .iter()
                .find(|(known, _)| known.eq_ignore_ascii_case(name))
                .map(|&(_, kind)| kind)
        })
        .unwrap_or(FsKind::Other)
    }

    /// Whether names follow the Windows rules: no `< > : " / \ | ? *`, no control characters,
    /// no reserved device names and no trailing dot or space. True for every file system
    /// Windows can create (NTFS, `ReFS`, FAT, exFAT) and for a share, since the server may be
    /// Windows.
    pub const fn windows_names(self) -> bool {
        matches!(
            self,
            FsKind::Ntfs | FsKind::ReFs | FsKind::Fat32 | FsKind::ExFat | FsKind::Smb
        )
    }

    /// Whether two names that differ only in case are the same name. False only for
    /// [`FsKind::Other`], which is assumed to tell them apart.
    pub const fn case_insensitive(self) -> bool {
        !matches!(self, FsKind::Other)
    }

    /// How the file system tells two names of one folder apart: by neither case nor Unicode
    /// form on `Apfs` and `HfsPlus`, by case alone on the others but [`FsKind::Other`], which
    /// tells every spelling apart.
    pub const fn folding(self) -> NameFolding {
        match self {
            FsKind::Apfs | FsKind::HfsPlus => NameFolding::CaseAndForm,
            FsKind::Other => NameFolding::Exact,
            _ => NameFolding::Case,
        }
    }

    /// The largest file the file system can hold, when it has a limit worth checking: 4 GiB
    /// minus one byte on FAT, none on the others.
    pub const fn max_file_size(self) -> Option<u64> {
        match self {
            FsKind::Fat32 => Some(FAT32_MAX_FILE),
            _ => None,
        }
    }

    /// The unit the file system counts a name's length in.
    const fn name_unit(self) -> Unit {
        if self.windows_names() || matches!(self, FsKind::HfsPlus) {
            Unit::Utf16
        } else {
            Unit::Utf8
        }
    }
}

/// How serious an [`IssueKind`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    /// The item can still be written, but it may not behave as expected on the target (a
    /// reserved or trailing-dot name is written literally and then hard to open on Windows).
    /// The user may go on.
    Warning,
    /// The item cannot be written to the target as it is.
    Blocking,
}

/// One thing that is wrong with putting an item on a target.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum IssueKind {
    /// The name holds a character the target file system does not allow.
    InvalidCharacter(char),
    /// The name is a Windows device name (`CON`, `NUL`, `CONIN$`, `COM1`, `COM¹`, ...), with
    /// or without an extension.
    ReservedName,
    /// The name ends with a dot or a space, which Windows file systems cannot keep.
    TrailingDotOrSpace,
    /// The name is longer than the target allows; `len` and `max` count the unit the target
    /// uses (see [`check_name`]).
    NameTooLong { len: usize, max: usize },
    /// The whole path is longer than the OS allows; `len` and `max` count the unit the OS
    /// uses (see [`check_path_len`]).
    PathTooLong { len: usize, max: usize },
    /// The file is larger than the target file system can hold.
    FileTooLarge { size: u64, max: u64 },
    /// Another item in the same folder, `other`, differs from this name only in case.
    CaseCollision { other: OsString },
    /// The target volume has less free space than the job needs.
    NotEnoughSpace { needed: u64, available: u64 },
}

impl IssueKind {
    /// The word the log writes for this issue as the reason of a failure
    /// (`reason=name_too_long`): the variant's name in `snake_case`. It is English whatever
    /// language the UI shows.
    pub const fn name(&self) -> &'static str {
        match self {
            IssueKind::InvalidCharacter(_) => "invalid_character",
            IssueKind::ReservedName => "reserved_name",
            IssueKind::TrailingDotOrSpace => "trailing_dot_or_space",
            IssueKind::NameTooLong { .. } => "name_too_long",
            IssueKind::PathTooLong { .. } => "path_too_long",
            IssueKind::FileTooLarge { .. } => "file_too_large",
            IssueKind::CaseCollision { .. } => "case_collision",
            IssueKind::NotEnoughSpace { .. } => "not_enough_space",
        }
    }

    /// Whether the user may go on despite the issue. Only a reserved name and a trailing dot
    /// or space are warnings; everything else stops the item.
    pub fn severity(&self) -> Severity {
        match self {
            IssueKind::ReservedName | IssueKind::TrailingDotOrSpace => Severity::Warning,
            IssueKind::InvalidCharacter(_)
            | IssueKind::NameTooLong { .. }
            | IssueKind::PathTooLong { .. }
            | IssueKind::FileTooLarge { .. }
            | IssueKind::CaseCollision { .. }
            | IssueKind::NotEnoughSpace { .. } => Severity::Blocking,
        }
    }
}

/// The log text of an issue, in English. The UI builds the text users read from the variant.
impl fmt::Display for IssueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IssueKind::InvalidCharacter(c) => write!(
                f,
                "name contains '{c}', which the target file system does not allow"
            ),
            IssueKind::ReservedName => f.write_str("name is reserved on Windows"),
            IssueKind::TrailingDotOrSpace => f.write_str("name ends with a dot or space"),
            IssueKind::NameTooLong { len, max } => {
                write!(f, "name is {len} units long; the limit is {max}")
            }
            IssueKind::PathTooLong { len, max } => {
                write!(f, "path is {len} units long; the limit is {max}")
            }
            IssueKind::FileTooLarge { size, max } => {
                write!(
                    f,
                    "file is {size} bytes; the target file system allows {max}"
                )
            }
            IssueKind::CaseCollision { other } => {
                write!(
                    f,
                    "name differs only in case from {}",
                    other.to_string_lossy()
                )
            }
            IssueKind::NotEnoughSpace { needed, available } => {
                write!(f, "needs {needed} bytes; {available} available")
            }
        }
    }
}

/// An [`IssueKind`] found on one target item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// The path the item would get on the target.
    pub target: VPath,
    pub kind: IssueKind,
}

/// The unit a length is counted in.
#[derive(Clone, Copy, Debug)]
enum Unit {
    Utf16,
    Utf8,
}

/// The length of `text` in `unit`. UTF-8 counts the bytes the string holds, so a Unix name
/// that is not text counts each byte once, as the kernel does. UTF-16 counts the units of the
/// text with every invalid piece as one `U+FFFD`, which is exact for Windows names (a lone
/// surrogate is one unit) and an estimate for bytes that no UTF-16 file system could hold.
/// For valid text this is the length of its `Display` form.
fn len_in(text: &OsStr, unit: Unit) -> usize {
    match unit {
        Unit::Utf16 => text.to_string_lossy().encode_utf16().count(),
        Unit::Utf8 => text.as_encoded_bytes().len(),
    }
}

/// Whether Windows file systems refuse `c` in a name.
fn is_forbidden(c: char) -> bool {
    matches!(
        c,
        '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\u{1}'..='\u{1f}'
    )
}

/// Whether `name` is a Windows device name: the part before the first dot, without trailing
/// spaces, is `CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, `COM1` to `COM9`, `COM¹`, `COM²`,
/// `COM³`, `LPT1` to `LPT9`, `LPT¹`, `LPT²` or `LPT³` in any ASCII case (the superscript
/// digits are U+00B9, U+00B2 and U+00B3, which Windows takes for 1, 2 and 3 there). `COM0`,
/// `COM10`, `COM` with any other digit (a full-width `１`) and names that merely start with a
/// device name are ordinary.
fn is_reserved(name: &str) -> bool {
    let stem = name
        .split_once('.')
        .map_or(name, |(stem, _)| stem)
        .trim_end_matches(' ');
    let device = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
        .iter()
        .any(|device| stem.eq_ignore_ascii_case(device));
    let port = stem
        .get(..3)
        .is_some_and(|head| head.eq_ignore_ascii_case("COM") || head.eq_ignore_ascii_case("LPT"));
    let mut number = stem.get(3..).unwrap_or_default().chars();
    let numbered = port
        && matches!(
            (number.next(), number.next()),
            (Some('1'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None)
        );
    device || numbered
}

/// Everything wrong with `name` as the name of one file or folder on `fs`, in a fixed order:
/// each forbidden character in the order it first appears (once per character), then
/// [`IssueKind::ReservedName`], then [`IssueKind::TrailingDotOrSpace`], then
/// [`IssueKind::NameTooLong`]. An empty result means the name is fine.
///
/// Only a file system with [`FsKind::windows_names`] gets the character, reserved-name and
/// trailing-dot checks: `< > : " / \ | ? *` and U+0001 to U+001F are forbidden, and a reserved
/// name is the part before the first dot, without trailing spaces, equal to `CON`, `PRN`,
/// `AUX`, `NUL`, `CONIN$`, `CONOUT$`, `COM1` to `COM9`, `COM¹` to `COM³`, `LPT1` to `LPT9` or
/// `LPT¹` to `LPT³` in any ASCII case. A rename and a create refuse every name with an issue,
/// a reserved one too.
///
/// The length limit is [`NAME_LIMIT`] (255) on every file system, counted in UTF-16 units on
/// Windows file systems, SMB shares and `HfsPlus` and in UTF-8 bytes on `Apfs` and `Other`. A
/// name with a character outside the Basic Multilingual Plane (an emoji) is two units in the
/// first and four bytes in the second; a Hangul syllable is one unit and three bytes. A name
/// that is not text (possible on Unix) counts its raw bytes in the second and one unit per
/// invalid piece in the first.
///
/// Anything that derives a new name from an old one (a numbered name such as `report (2).txt`
/// can be up to seven units longer than the original) calls this with the candidate and looks
/// for [`IssueKind::NameTooLong`] in the result before it touches the file system.
pub fn check_name(name: &OsStr, fs: FsKind) -> Vec<IssueKind> {
    let mut issues = Vec::new();
    if fs.windows_names() {
        let text = name.to_string_lossy();
        for c in text.chars().filter(|&c| is_forbidden(c)) {
            let issue = IssueKind::InvalidCharacter(c);
            if !issues.contains(&issue) {
                issues.push(issue);
            }
        }
        if is_reserved(&text) {
            issues.push(IssueKind::ReservedName);
        }
        if text.ends_with(['.', ' ']) {
            issues.push(IssueKind::TrailingDotOrSpace);
        }
    }
    let len = len_in(name, fs.name_unit());
    if len > NAME_LIMIT {
        issues.push(IssueKind::NameTooLong {
            len,
            max: NAME_LIMIT,
        });
    }
    issues
}

/// [`IssueKind::PathTooLong`] when the local path `target` is longer than `os` allows, else
/// `None`. A path that is not local has no length to check and gives `None`.
///
/// The length is that of the path's `Display` text (the raw bytes for a Unix path that is not
/// text): UTF-16 units against [`WINDOWS_PATH_LIMIT`] on Windows, UTF-8 bytes against
/// [`MACOS_PATH_LIMIT`] on macOS and against [`OTHER_PATH_LIMIT`] anywhere else.
///
/// The path checked is the final one. The temporary file a transfer writes first has a name of
/// at least 29 units, so a final path that ends in a shorter name and sits right at the limit
/// can still fail when the temporary file is created.
pub fn check_path_len(target: &VPath, os: Os) -> Option<IssueKind> {
    let path = target.as_local()?;
    let (unit, max) = match os {
        Os::Windows => (Unit::Utf16, WINDOWS_PATH_LIMIT),
        Os::MacOs => (Unit::Utf8, MACOS_PATH_LIMIT),
        Os::Other => (Unit::Utf8, OTHER_PATH_LIMIT),
    };
    let len = len_in(path.as_os_str(), unit);
    (len > max).then_some(IssueKind::PathTooLong { len, max })
}

/// [`IssueKind::FileTooLarge`] when a file of `size` bytes exceeds what `fs` can hold, else
/// `None`. The limit is [`FsKind::max_file_size`]: exactly [`FAT32_MAX_FILE`] bytes fit.
pub fn check_size(size: u64, fs: FsKind) -> Option<IssueKind> {
    let max = fs.max_file_size()?;
    (size > max).then_some(IssueKind::FileTooLarge { size, max })
}

/// The key under which two names of one folder on `fs` are the same name: `rf_vfs::fold_name`
/// by the file system's rule ([`FsKind::folding`]), the lossy text composed to NFC first on
/// `Apfs` and `HfsPlus` (which ignore the Unicode form of a name), then lower-cased. It is a
/// simple lower-casing, not a full case folding: `ß` and `SS` stay different names.
///
/// [`CaseIndex`] and the transfer's path comparisons share it. It means something only on a
/// file system with [`FsKind::case_insensitive`]; callers check that first.
pub(crate) fn fold_name(name: &OsStr, fs: FsKind) -> String {
    rf_vfs::fold_name(name, fs.folding())
}

/// Finds names that collide in case on a case-insensitive target as a job plans its writes.
///
/// Feed it every name that will be created, folder by folder. Names are compared by their
/// folded key (see [`FsKind::case_insensitive`]); the folder is compared exactly as given, so
/// two spellings of a folder that collide are caught one level up, as names in their parent.
#[derive(Debug)]
pub struct CaseIndex {
    fs: FsKind,
    /// For each folder, the first name seen under each key.
    dirs: HashMap<VPath, HashMap<String, OsString>>,
}

impl CaseIndex {
    pub fn new(fs: FsKind) -> CaseIndex {
        CaseIndex {
            fs,
            dirs: HashMap::new(),
        }
    }

    /// Records `name` as an item of `dir`. When an item of the same folder that has the same
    /// key but another spelling was recorded first, returns that first spelling (the same
    /// name twice is not a collision). Without [`FsKind::case_insensitive`] nothing is
    /// recorded and the answer is always `None`.
    ///
    /// The key is the lossy text of the name lower-cased, after an NFC normalization on
    /// `Apfs` and `HfsPlus`; a name that is not text is compared by its lossy form.
    pub fn insert(&mut self, dir: &VPath, name: &OsStr) -> Option<OsString> {
        if !self.fs.case_insensitive() {
            return None;
        }
        let first = self
            .dirs
            .entry(dir.clone())
            .or_default()
            .entry(fold_name(name, self.fs))
            .or_insert_with(|| name.to_os_string());
        (first != name).then(|| first.clone())
    }
}
