//! Comparing names the way a file system does (I18N-04): one that ignores case, and one that
//! also ignores the Unicode form of a name (NFC or NFD).
//!
//! This is the one place where names are folded. The jobs' pre-flight check and their path
//! comparisons, the local provider's case-only rename guard and its watch check for one file
//! reported under two names, and the UI's navigation sync, path field completion, remembered
//! folder views and pattern selection all go through it, each with the rule of its own system.

use std::borrow::Cow;
use std::ffi::OsStr;

use rf_core::Os;
use unicode_normalization::{UnicodeNormalization, is_nfc};

/// How a file system tells two names of one folder apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NameFolding {
    /// Every spelling is a name of its own: case and Unicode form count (Linux file systems).
    Exact,
    /// Names that differ only in case are one name (NTFS, `ReFS`, FAT, exFAT, a share).
    Case,
    /// Names that differ only in case or in their Unicode form are one name (APFS, HFS+): the
    /// syllable `한` composed (NFC) and decomposed into its jamo (NFD) is one name there.
    CaseAndForm,
}

impl NameFolding {
    /// The rule of the usual local volumes of `os`: [`NameFolding::Case`] on Windows (NTFS),
    /// [`NameFolding::CaseAndForm`] on macOS (APFS), and [`NameFolding::Exact`] elsewhere.
    pub const fn of(os: Os) -> NameFolding {
        match os {
            Os::Windows => NameFolding::Case,
            Os::MacOs => NameFolding::CaseAndForm,
            Os::Other => NameFolding::Exact,
        }
    }

    /// Whether names that differ only in case are one name.
    pub const fn ignores_case(self) -> bool {
        !matches!(self, NameFolding::Exact)
    }

    /// Whether names that differ only in their Unicode form (NFC or NFD) are one name.
    pub const fn ignores_form(self) -> bool {
        matches!(self, NameFolding::CaseAndForm)
    }

    /// This rule with case not counting either: [`NameFolding::Case`] in place of
    /// [`NameFolding::Exact`], the others as they are. For a comparison that ignores case on
    /// every system, such as a search the user types, and the Unicode form where this rule does.
    #[must_use]
    pub const fn ignoring_case(self) -> NameFolding {
        match self {
            NameFolding::Exact => NameFolding::Case,
            other => other,
        }
    }
}

/// The text of `name` in the one Unicode form `folding` compares it in: composed to NFC where
/// the form does not count ([`NameFolding::ignores_form`]), else as it is. A name that is not
/// Unicode is its lossy text. Case is kept: this is for a comparison whose case rule is its
/// own, as a pattern's is.
pub fn name_text(name: &OsStr, folding: NameFolding) -> Cow<'_, str> {
    let text = name.to_string_lossy();
    if folding.ignores_form() && !is_nfc(&text) {
        Cow::Owned(text.nfc().collect())
    } else {
        text
    }
}

/// `name` folded by `folding`: a key that two names share when a file system of that rule takes
/// them for one. It is [`name_text`] (NFC where the form does not count), lower-cased where
/// case does not count ([`NameFolding::ignores_case`]). A simple lower-casing, not a full case
/// folding: `ß` and `SS` stay different names.
pub fn fold_name(name: &OsStr, folding: NameFolding) -> String {
    let text = name_text(name, folding);
    if folding.ignores_case() {
        text.to_lowercase()
    } else {
        text.into_owned()
    }
}

/// The rule the local provider's own guards compare two spellings by: case everywhere, as a
/// volume that ignores case may be mounted on any system (a file identity decides then), and
/// the Unicode form too on macOS, whose volumes ignore it.
pub(crate) const LOCAL_GUARD: NameFolding = NameFolding::of(Os::CURRENT).ignoring_case();

#[cfg(test)]
mod tests {
    use super::*;

    /// 한글 composed (NFC) and decomposed (NFD).
    const NFC: &str = "한글.txt";
    const NFD: &str = "\u{1112}\u{1161}\u{11ab}\u{1100}\u{1173}\u{11af}.txt";

    fn fold(name: &str, folding: NameFolding) -> String {
        fold_name(OsStr::new(name), folding)
    }

    #[test]
    fn names_that_differ_only_in_case_fold_alike_where_case_does_not_count() {
        for folding in [NameFolding::Case, NameFolding::CaseAndForm] {
            assert_eq!(fold("B.TXT", folding), fold("b.txt", folding));
            assert_eq!(fold("ÜBER", folding), fold("über", folding));
            assert_ne!(fold("a.txt", folding), fold("b.txt", folding));
            // Lower-casing is not full case folding.
            assert_ne!(fold("straße", folding), fold("STRASSE", folding));
        }
        assert_ne!(
            fold("B.TXT", NameFolding::Exact),
            fold("b.txt", NameFolding::Exact)
        );
        assert_eq!(fold("B.TXT", NameFolding::Exact), "B.TXT");
    }

    #[test]
    fn the_two_unicode_forms_of_a_name_fold_alike_only_where_the_form_does_not_count() {
        assert_ne!(NFC, NFD);
        assert_eq!(
            fold(NFC, NameFolding::CaseAndForm),
            fold(NFD, NameFolding::CaseAndForm)
        );
        assert_eq!(fold(NFD, NameFolding::CaseAndForm), NFC);
        for folding in [NameFolding::Exact, NameFolding::Case] {
            assert_ne!(fold(NFC, folding), fold(NFD, folding), "{folding:?}");
        }
        // The text in the one form keeps its case.
        let upper = format!("{NFD}-A");
        assert_eq!(
            name_text(OsStr::new(&upper), NameFolding::CaseAndForm),
            format!("{NFC}-A")
        );
        assert_eq!(
            name_text(OsStr::new(&upper), NameFolding::Case),
            upper.as_str()
        );
    }

    #[test]
    fn each_system_has_the_rule_of_its_usual_volumes() {
        assert_eq!(NameFolding::of(Os::Windows), NameFolding::Case);
        assert_eq!(NameFolding::of(Os::MacOs), NameFolding::CaseAndForm);
        assert_eq!(NameFolding::of(Os::Other), NameFolding::Exact);
        assert_eq!(NameFolding::Exact.ignoring_case(), NameFolding::Case);
        assert_eq!(NameFolding::Case.ignoring_case(), NameFolding::Case);
        assert_eq!(
            NameFolding::CaseAndForm.ignoring_case(),
            NameFolding::CaseAndForm
        );
        // The local guards fold case everywhere, and the form on macOS.
        assert!(LOCAL_GUARD.ignores_case());
        assert_eq!(LOCAL_GUARD.ignores_form(), cfg!(target_os = "macos"));
    }
}
