//! The user's well-known folders (home, desktop, documents, ...) as local addresses.

use rf_core::VPath;

/// A well-known folder of the current user, for the sidebar and the folder picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KnownFolder {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Music,
    /// `Videos` on Windows, `Movies` on macOS.
    Videos,
}

impl KnownFolder {
    /// Every folder, in the order the sidebar lists them.
    pub const ALL: [Self; 7] = [
        Self::Home,
        Self::Desktop,
        Self::Documents,
        Self::Downloads,
        Self::Pictures,
        Self::Music,
        Self::Videos,
    ];
}

/// The location of `folder` for the current user, or `None` when the system defines none.
///
/// Windows asks the shell (`SHGetKnownFolderPath`), so folder redirection and `OneDrive` are
/// honored. macOS uses the folders under the home folder (`Videos` is `~/Movies`). On Linux
/// only the XDG user folders that are configured exist.
pub fn known_folder(folder: KnownFolder) -> Option<VPath> {
    let path = match folder {
        KnownFolder::Home => dirs::home_dir(),
        KnownFolder::Desktop => dirs::desktop_dir(),
        KnownFolder::Documents => dirs::document_dir(),
        KnownFolder::Downloads => dirs::download_dir(),
        KnownFolder::Pictures => dirs::picture_dir(),
        KnownFolder::Music => dirs::audio_dir(),
        KnownFolder::Videos => dirs::video_dir(),
    }?;
    VPath::local(path).ok()
}
