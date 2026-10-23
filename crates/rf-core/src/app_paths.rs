//! Where the app keeps its settings, data, caches, logs and temporary files (PRD INST-03,
//! INST-06). The app writes nowhere else.

use std::path::{Path, PathBuf};

use crate::error::{Error, PathError, Result};
use crate::identity::{BUNDLE_ID, DATA_DIR_NAME, PORTABLE_DATA_DIR, PORTABLE_MARKER};
use crate::os::Os;
use crate::vpath::VPath;

const CONFIG: &str = "config";
const STATE: &str = "state";
const CACHE: &str = "cache";
const LOGS: &str = "logs";
const TMP: &str = "tmp";

/// How the app was installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InstallMode {
    /// Per-user folders chosen by the operating system.
    Installed,
    /// Everything lives in a `data` folder next to the executable (PRD INST-06).
    Portable,
}

/// The per-user base folders [`AppPaths::resolve`] builds on.
///
/// Fields a host does not provide (for example `app_data` on macOS) stay `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BaseDirs {
    /// The user's home folder (`HOME`).
    pub home: Option<PathBuf>,
    /// The roaming application data folder (`%APPDATA%`).
    pub app_data: Option<PathBuf>,
    /// The local application data folder (`%LOCALAPPDATA%`).
    pub local_app_data: Option<PathBuf>,
    /// The folder for temporary files in installed mode.
    pub temp: PathBuf,
}

impl BaseDirs {
    /// Reads `HOME`, `APPDATA` and `LOCALAPPDATA` (empty values count as unset) and the
    /// system temp folder.
    pub fn from_env() -> Self {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Self {
            home: var("HOME"),
            app_data: var("APPDATA"),
            local_app_data: var("LOCALAPPDATA"),
            temp: std::env::temp_dir(),
        }
    }
}

/// The folders the app owns. Every file the app writes goes into one of them.
///
/// - Settings TOML: [`config_dir`](Self::config_dir).
/// - Ledger, job journal, metadata DB, index snapshots: [`data_dir`](Self::data_dir).
/// - Thumbnails and other rebuildable data: [`cache_dir`](Self::cache_dir).
/// - Logs: [`log_dir`](Self::log_dir).
/// - Temporary files, named with `identity::TEMP_PREFIX`: [`temp_dir`](Self::temp_dir).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppPaths {
    mode: InstallMode,
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
    log: PathBuf,
    temp: PathBuf,
    preferences: Option<PathBuf>,
    owned: Vec<PathBuf>,
}

impl AppPaths {
    /// Locates the folders for the running executable on this machine.
    pub fn detect() -> Result<Self> {
        let exe =
            std::env::current_exe().map_err(|e| Error::Unsupported(format!("current_exe: {e}")))?;
        Self::resolve(Os::CURRENT, &exe, &BaseDirs::from_env())
    }

    /// Computes the layout for `os` from the executable path and the base folders.
    /// Nothing is created or written; see [`ensure_dirs`](Self::ensure_dirs).
    ///
    /// Except on macOS, a file named `identity::PORTABLE_MARKER` next to `exe` selects
    /// portable mode, which ignores `base` entirely.
    pub fn resolve(os: Os, exe: &Path, base: &BaseDirs) -> Result<Self> {
        require_absolute(exe)?;
        if os != Os::MacOs {
            if let Some(dir) = exe.parent() {
                if dir.join(PORTABLE_MARKER).is_file() {
                    return Ok(Self::portable(&dir.join(PORTABLE_DATA_DIR)));
                }
            }
        }
        require_absolute(&base.temp)?;
        let temp = base.temp.clone();
        match os {
            Os::Windows => {
                let app_data = required(base.app_data.as_deref(), "APPDATA")?;
                let local_app_data = required(base.local_app_data.as_deref(), "LOCALAPPDATA")?;
                Ok(Self::windows(app_data, local_app_data, temp))
            }
            Os::MacOs => Ok(Self::macos(required(base.home.as_deref(), "HOME")?, temp)),
            Os::Other => Ok(Self::other(required(base.home.as_deref(), "HOME")?, temp)),
        }
    }

    fn portable(root: &Path) -> Self {
        Self {
            mode: InstallMode::Portable,
            config: root.join(CONFIG),
            data: root.join(STATE),
            cache: root.join(CACHE),
            log: root.join(LOGS),
            temp: root.join(TMP),
            preferences: None,
            owned: vec![root.to_path_buf()],
        }
    }

    /// Settings roam with the user profile. The data folder holds per-machine state
    /// (ledger, file IDs, index), so it stays in the local profile.
    fn windows(app_data: &Path, local_app_data: &Path, temp: PathBuf) -> Self {
        let roaming = app_data.join(DATA_DIR_NAME);
        let local = local_app_data.join(DATA_DIR_NAME);
        Self {
            mode: InstallMode::Installed,
            config: roaming.clone(),
            data: local.join(STATE),
            cache: local.join(CACHE),
            log: local.join(LOGS),
            temp,
            preferences: None,
            owned: vec![roaming, local],
        }
    }

    fn macos(home: &Path, temp: PathBuf) -> Self {
        let library = home.join("Library");
        let support = library.join("Application Support").join(BUNDLE_ID);
        let caches = library.join("Caches").join(BUNDLE_ID);
        let logs = library.join("Logs").join(BUNDLE_ID);
        let plist = library
            .join("Preferences")
            .join(format!("{BUNDLE_ID}.plist"));
        Self {
            mode: InstallMode::Installed,
            config: support.clone(),
            data: support.join(STATE),
            cache: caches.clone(),
            log: logs.clone(),
            temp,
            preferences: Some(plist.clone()),
            owned: vec![support, caches, logs, plist],
        }
    }

    /// Development layout for targets the product does not ship on.
    fn other(home: &Path, temp: PathBuf) -> Self {
        let root = home.join(".local").join("share").join(DATA_DIR_NAME);
        Self {
            mode: InstallMode::Installed,
            config: root.join(CONFIG),
            data: root.join(STATE),
            cache: root.join(CACHE),
            log: root.join(LOGS),
            temp,
            preferences: None,
            owned: vec![root],
        }
    }

    pub fn mode(&self) -> InstallMode {
        self.mode
    }

    /// Settings files.
    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    /// Ledger, job journal, metadata DB and index snapshots.
    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    /// Thumbnails and other data that can be rebuilt.
    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }

    pub fn log_dir(&self) -> &Path {
        &self.log
    }

    /// Temporary files and folders, named with `identity::TEMP_PREFIX`.
    pub fn temp_dir(&self) -> &Path {
        &self.temp
    }

    /// The macOS preferences property list, if this layout has one.
    pub fn preferences_file(&self) -> Option<&Path> {
        self.preferences.as_deref()
    }

    /// Everything the app owns outside the shared temp folder: the paths an
    /// uninstaller or a "reset all data" command removes (PRD INST-03).
    pub fn owned_paths(&self) -> &[PathBuf] {
        &self.owned
    }

    /// Creates the config, data, cache, log and temp folders (and their parents)
    /// if they are missing. Creates nothing else.
    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            self.config_dir(),
            self.data_dir(),
            self.cache_dir(),
            self.log_dir(),
            self.temp_dir(),
        ] {
            if let Err(e) = std::fs::create_dir_all(dir) {
                return Err(Error::io("create_dir_all", VPath::local(dir)?, e));
            }
        }
        Ok(())
    }
}

fn require_absolute(path: &Path) -> Result<(), PathError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(PathError::NotAbsolute(path.to_string_lossy().into_owned()))
    }
}

/// A base folder the chosen layout needs: it must be set and absolute.
fn required<'a>(dir: Option<&'a Path>, var: &'static str) -> Result<&'a Path> {
    let dir = dir.ok_or(Error::MissingEnv(var))?;
    require_absolute(dir)?;
    Ok(dir)
}
