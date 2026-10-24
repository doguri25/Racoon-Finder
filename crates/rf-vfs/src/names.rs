//! Comparing names the way a volume that ignores case does.

use std::ffi::OsStr;

/// `name` folded for a comparison that ignores case: the lowercase of its lossy text.
///
/// This is the one place where the crate folds names: the case-only rename guard and the watch
/// check for one file reported under two names both go through it. Unicode normalization (NFC,
/// so that the NFC and NFD spellings APFS treats as one name fold alike) belongs here when
/// I18N-04 adds it.
pub(crate) fn fold_name(name: &OsStr) -> String {
    name.to_string_lossy().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_differ_only_in_case_fold_alike() {
        assert_eq!(
            fold_name(OsStr::new("B.TXT")),
            fold_name(OsStr::new("b.txt"))
        );
        assert_eq!(fold_name(OsStr::new("ÜBER")), fold_name(OsStr::new("über")));
        assert_ne!(
            fold_name(OsStr::new("a.txt")),
            fold_name(OsStr::new("b.txt"))
        );
    }
}
