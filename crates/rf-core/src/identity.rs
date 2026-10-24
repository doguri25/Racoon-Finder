//! App identity constants. This is the only file that spells the product name,
//! so a rename (PRD Q1) is a one-file change.

/// Human-readable product name shown in window titles, dialogs and installers.
pub const DISPLAY_NAME: &str = "Racoon Finder";

/// Folder name under the per-user data directory (PRD INST-03).
pub const DATA_DIR_NAME: &str = "RacoonFinder";

/// Reverse-DNS bundle identifier (proposed by the plan; revisit with PRD Q1).
pub const BUNDLE_ID: &str = "io.github.doguri25.RacoonFinder";

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
