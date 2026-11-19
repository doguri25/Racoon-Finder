//! App identity constants. This is the only file that spells the product name,
//! so a rename (PRD Q1) is a one-file change.

/// Human-readable product name shown in window titles, dialogs and installers.
pub const DISPLAY_NAME: &str = "Racoon Finder";

/// Folder name under the per-user data directory (PRD INST-03).
pub const DATA_DIR_NAME: &str = "RacoonFinder";

/// Reverse-DNS bundle identifier (proposed by the plan; revisit with PRD Q1).
pub const BUNDLE_ID: &str = "io.github.doguri25.RacoonFinder";

/// The Windows `AppUserModelID` of the app (PRD INST-01): the Start menu shortcut the installer
/// makes carries it, and an installed app's notifications go out under it, so that they show
/// the app's name and icon. The same value as [`BUNDLE_ID`], under the name Windows gives it.
pub const AUMID: &str = "io.github.doguri25.RacoonFinder";

/// Custom URL scheme registered by the app (PRD MACX-02).
pub const URL_SCHEME: &str = "racoonfinder";

/// Prefix of every temporary file or folder the app creates.
pub const TEMP_PREFIX: &str = "RacoonFinder-";

/// Name of the marker file that switches the app into portable mode (PRD INST-06).
pub const PORTABLE_MARKER: &str = "portable";

/// Folder next to the executable that holds all data in portable mode (PRD INST-06).
pub const PORTABLE_DATA_DIR: &str = "data";

/// Crate version, shared by every workspace member.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_id_is_the_bundle_id() {
        assert_eq!(AUMID, BUNDLE_ID);
    }
}
