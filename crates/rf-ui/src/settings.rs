//! The settings kept between runs, in `settings.toml` in the app's config folder
//! (`AppPaths::config_dir`): the theme, whether hidden items show, the window's size, the tabs of
//! both panes and which one has the keyboard, and the destination picker's recent targets.
//!
//! [`load`] reads them once at the start, before the window shows. A file that cannot be read
//! or does not parse is moved aside to `settings.toml.bad` (one is kept: a newer one replaces
//! it) and the app starts with the defaults; nothing here fails the start. [`Settings::save`]
//! writes them atomically: to `settings.toml.tmp` in the same folder, synced to the disk, then
//! renamed over the old file, so a crash leaves the old settings or the new ones, never half a
//! file. [`Saver`] runs those writes on worker threads, one at a time.
//!
//! Keys this version does not know (a newer version wrote them) are kept in the `extra` tables
//! and written back, so running an older version in between does not lose them.
//!
//! Folders are stored as `VPath::to_uri`, which reads back losslessly with `VPath::parse`.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use rf_core::VPath;
use serde::{Deserialize, Serialize};

/// The name of the settings file in the config folder.
pub const FILE_NAME: &str = "settings.toml";
/// Where a settings file that could not be read is moved to.
pub const BAD_FILE_NAME: &str = "settings.toml.bad";
/// The file a save writes before it takes the place of the settings file.
pub const TEMP_FILE_NAME: &str = "settings.toml.tmp";
/// A settings file larger than this is no file this app wrote; it is treated as broken.
const MAX_FILE_SIZE: u64 = 1024 * 1024;

/// The smallest window size, in logical pixels (the main window's `min-width`/`min-height`).
pub const MIN_WINDOW: (u32, u32) = (900, 560);
/// The largest window size taken from the file, in logical pixels: a hand-edited size beyond it
/// would ask the window system for a window it cannot make.
pub const MAX_WINDOW: (u32, u32) = (16_384, 16_384);

/// The colors the window shows (the main window's `theme-choice` turns it into colors).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// Light or dark as the system is set.
    #[default]
    System,
    Light,
    Dark,
}

/// The tabs of one pane, as stored.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneSettings {
    /// The folders of the tabs, in the order of the tab bar, as `VPath::to_uri`.
    pub tabs: Vec<String>,
    /// The position of the tab that shows.
    pub active: usize,
    /// The keys of a newer version.
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// Everything `settings.toml` holds. A key the file lacks takes its default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    pub show_hidden: bool,
    /// The window's last size in logical pixels (width, height), when it was neither maximized
    /// nor full screen; `None` until the first save.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<(u32, u32)>,
    /// The pane with the keyboard: 0 left, 1 right.
    pub active_pane: usize,
    /// The destination picker's recent targets, the last one chosen first, as `VPath::to_uri`.
    pub recent_targets: Vec<String>,
    /// The left pane, then the right one.
    pub panes: [PaneSettings; 2],
    /// The keys of a newer version.
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// What the window showed when the app last ended, as folders: the tabs of each pane and the
/// one that shows, the pane with the keyboard, and whether hidden items show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// The left pane, then the right one.
    pub panes: [SessionPane; 2],
    /// 0 left, 1 right.
    pub active_pane: usize,
    pub show_hidden: bool,
}

/// The tabs of one pane: their folders in order, and the position of the one that shows. A pane
/// without tabs opens its default folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionPane {
    pub tabs: Vec<VPath>,
    pub active: usize,
}

/// A tab of a restored [`Session`] whose folder could not be looked at in time at the start:
/// it shows the home folder meanwhile, and goes to `path` if it turns out to be there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiting {
    /// The pane: 0 left, 1 right.
    pub side: usize,
    /// The tab's position in the pane.
    pub tab: usize,
    /// The folder the tab showed.
    pub path: VPath,
}

impl Session {
    /// Every folder of the tabs, each once, in the order of the panes and their tabs.
    pub fn folders(&self) -> Vec<VPath> {
        let mut folders: Vec<VPath> = Vec::new();
        for path in self.panes.iter().flat_map(|pane| &pane.tabs) {
            if !folders.contains(path) {
                folders.push(path.clone());
            }
        }
        folders
    }

    /// The session to open at the start: a tab whose folder `exists` says is gone shows `home`
    /// instead. A tab whose folder `exists` cannot tell about yet (`None`: a slow or sleeping
    /// drive) shows `home` too, so the window need not wait for it, and comes back as a
    /// [`Waiting`], to go to its folder once that is found.
    pub fn resolve(
        mut self,
        home: &VPath,
        mut exists: impl FnMut(&VPath) -> Option<bool>,
    ) -> (Session, Vec<Waiting>) {
        let mut waiting = Vec::new();
        for (side, pane) in self.panes.iter_mut().enumerate() {
            for (tab, path) in pane.tabs.iter_mut().enumerate() {
                match exists(path) {
                    Some(true) => continue,
                    Some(false) => {}
                    None => waiting.push(Waiting {
                        side,
                        tab,
                        path: path.clone(),
                    }),
                }
                home.clone_into(path);
            }
        }
        (self, waiting)
    }
}

impl Settings {
    /// The tabs, the active pane and the hidden items as folders. A tab whose folder does not
    /// read back as a path is left out; the tab that shows stays the same one when it is kept,
    /// and is the first otherwise.
    pub fn session(&self) -> Session {
        let pane = |stored: &PaneSettings| {
            let mut pane = SessionPane::default();
            for (i, uri) in stored.tabs.iter().enumerate() {
                if let Ok(path) = VPath::parse(uri) {
                    if i == stored.active {
                        pane.active = pane.tabs.len();
                    }
                    pane.tabs.push(path);
                }
            }
            pane
        };
        Session {
            panes: [pane(&self.panes[0]), pane(&self.panes[1])],
            active_pane: self.active_pane.min(1),
            show_hidden: self.show_hidden,
        }
    }

    /// Takes `session` in, keeping the keys of a newer version.
    pub fn set_session(&mut self, session: &Session) {
        for (stored, pane) in self.panes.iter_mut().zip(&session.panes) {
            stored.tabs = pane.tabs.iter().map(VPath::to_uri).collect();
            stored.active = pane.active;
        }
        self.active_pane = session.active_pane;
        self.show_hidden = session.show_hidden;
    }

    /// The recent targets that read back as paths, in their order.
    pub fn recent(&self) -> Vec<VPath> {
        self.recent_targets
            .iter()
            .filter_map(|uri| VPath::parse(uri).ok())
            .collect()
    }

    pub fn set_recent(&mut self, targets: &[VPath]) {
        self.recent_targets = targets.iter().map(VPath::to_uri).collect();
    }

    /// The size the window opens at, in logical pixels: the stored one, kept between
    /// [`MIN_WINDOW`] and [`MAX_WINDOW`]; `None` without one.
    pub fn window_size(&self) -> Option<(u32, u32)> {
        self.window.map(|(width, height)| {
            (
                width.clamp(MIN_WINDOW.0, MAX_WINDOW.0),
                height.clamp(MIN_WINDOW.1, MAX_WINDOW.1),
            )
        })
    }

    /// Writes the settings to `settings.toml` in `config_dir` (made if missing), atomically:
    /// to `settings.toml.tmp` there, synced to the disk with `rf_platform::sync_file`, then
    /// renamed over the old file. The temporary file is removed when a step fails.
    ///
    /// Blocks on the disk: call it on a worker thread ([`Saver`]).
    pub fn save(&self, config_dir: &Path) -> io::Result<()> {
        let text = toml::to_string(self).map_err(io::Error::other)?;
        std::fs::create_dir_all(config_dir)?;
        let temp = config_dir.join(TEMP_FILE_NAME);
        let written = write_synced(&temp, text.as_bytes())
            .and_then(|()| std::fs::rename(&temp, config_dir.join(FILE_NAME)));
        if written.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        written
    }
}

/// Writes `bytes` to a new file at `path` and makes it durable.
fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    rf_platform::sync_file(&file)
}

/// Reads `settings.toml` in `config_dir`. Without one (the first run) the settings are the
/// defaults. One that cannot be read, is too large or does not parse is moved aside to
/// `settings.toml.bad`, replacing an older one there, and the settings are the defaults: the
/// app starts either way, and the broken file stays for a look.
///
/// It reads a small local file and is called once at the start, before the window shows: the
/// UI thread may call it then.
pub fn load(config_dir: &Path) -> Settings {
    let path = config_dir.join(FILE_NAME);
    match read_small(&path) {
        Ok(text) => {
            if let Ok(settings) = toml::from_str(&text) {
                return settings;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Settings::default(),
        Err(_) => {}
    }
    let _ = std::fs::rename(&path, config_dir.join(BAD_FILE_NAME));
    Settings::default()
}

/// The text of the file at `path`; `InvalidData` for one larger than [`MAX_FILE_SIZE`] or not
/// UTF-8.
fn read_small(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    File::open(path)?
        .take(MAX_FILE_SIZE + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_FILE_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "settings too large",
        ));
    }
    Ok(text)
}

/// Writes the settings to the config folder, one write at a time, on worker threads or on the
/// calling one. Every write has a number in the order it was asked for, and a write whose
/// number is below that of one already written is skipped: the settings asked for last are the
/// ones that stay, even when an earlier write waited longer. Clones share the order.
#[derive(Clone, Debug)]
pub struct Saver {
    config_dir: Arc<PathBuf>,
    /// The number of the last write done; held while a write runs.
    written: Arc<Mutex<u64>>,
    next: Arc<AtomicU64>,
}

impl Saver {
    pub fn new(config_dir: PathBuf) -> Saver {
        Saver {
            config_dir: Arc::new(config_dir),
            written: Arc::new(Mutex::new(0)),
            next: Arc::new(AtomicU64::new(1)),
        }
    }

    /// The folder the settings go to.
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// Writes `settings` on a new worker thread and returns at once; `done` gets the result
    /// there, `Ok` too when a later write made this one needless. `Err` when the thread could
    /// not start: nothing was written then.
    pub fn save_in_background(
        &self,
        settings: Settings,
        done: impl FnOnce(io::Result<()>) + Send + 'static,
    ) -> io::Result<()> {
        let number = self.next.fetch_add(1, Ordering::SeqCst);
        let saver = self.clone();
        std::thread::Builder::new()
            .name("rf-ui-settings".to_owned())
            .spawn(move || done(saver.write(number, &settings)))
            .map(drop)
    }

    /// Writes `settings` on the calling thread, after any write in progress. Blocks on the disk:
    /// call it on a worker thread.
    pub fn save(&self, settings: &Settings) -> io::Result<()> {
        let number = self.next.fetch_add(1, Ordering::SeqCst);
        self.write(number, settings)
    }

    fn write(&self, number: u64, settings: &Settings) -> io::Result<()> {
        let mut written = self.written.lock().unwrap_or_else(PoisonError::into_inner);
        if *written > number {
            return Ok(());
        }
        *written = number;
        settings.save(&self.config_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(dir: &Path, name: &str) -> VPath {
        VPath::local(dir.join(name)).expect("a local path")
    }

    /// Settings with something in every field, keys of a newer version among them.
    fn full(dir: &Path) -> Settings {
        let mut settings = Settings {
            theme: ThemeChoice::Dark,
            show_hidden: true,
            window: Some((1500, 900)),
            ..Settings::default()
        };
        settings.set_session(&Session {
            panes: [
                SessionPane {
                    tabs: vec![folder(dir, "a"), folder(dir, "설계 문서")],
                    active: 1,
                },
                SessionPane {
                    tabs: vec![folder(dir, "c")],
                    active: 0,
                },
            ],
            active_pane: 1,
            show_hidden: true,
        });
        settings.set_recent(&[folder(dir, "r1"), folder(dir, "r2")]);
        settings
            .extra
            .insert("future_value".to_owned(), toml::Value::Integer(7));
        let mut table = toml::Table::new();
        table.insert(
            "density".to_owned(),
            toml::Value::String("compact".to_owned()),
        );
        settings
            .extra
            .insert("future_table".to_owned(), toml::Value::Table(table));
        settings.panes[1]
            .extra
            .insert("split".to_owned(), toml::Value::Float(0.5));
        settings
    }

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let settings = full(dir.path());

        settings.save(dir.path()).expect("saved");
        assert!(dir.path().join(FILE_NAME).is_file());
        assert!(!dir.path().join(TEMP_FILE_NAME).exists());
        let loaded = load(dir.path());

        assert_eq!(loaded, settings);
        assert_eq!(loaded.session(), settings.session());
        assert_eq!(
            loaded.recent(),
            [folder(dir.path(), "r1"), folder(dir.path(), "r2")]
        );
        // A second save replaces the first.
        let mut changed = loaded;
        changed.theme = ThemeChoice::Light;
        changed.save(dir.path()).expect("saved again");
        assert_eq!(load(dir.path()).theme, ThemeChoice::Light);
    }

    #[test]
    fn missing_file_gives_the_defaults() {
        let dir = tempfile::tempdir().expect("a temp dir");
        assert_eq!(load(dir.path()), Settings::default());
        assert!(!dir.path().join(BAD_FILE_NAME).exists());
        assert_eq!(Settings::default().theme, ThemeChoice::System);
        assert_eq!(Settings::default().window_size(), None);
    }

    #[test]
    fn keys_a_file_lacks_take_their_defaults() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join(FILE_NAME), "theme = \"light\"\n").expect("written");
        let loaded = load(dir.path());
        assert_eq!(loaded.theme, ThemeChoice::Light);
        assert!(!loaded.show_hidden);
        assert_eq!(loaded.session(), Session::default());
    }

    #[test]
    fn corrupt_file_is_moved_aside() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        let bad = dir.path().join(BAD_FILE_NAME);
        std::fs::write(&bad, "an older broken file").expect("written");

        for broken in [
            &b"theme = \"dark\"\npanes = [[[[\n"[..],
            b"theme = \"purple\"\n",
            b"show_hidden = 3\n",
            b"\xFF\xFE not text",
        ] {
            std::fs::write(&file, broken).expect("written");
            assert_eq!(load(dir.path()), Settings::default());
            assert!(!file.exists(), "{broken:?} stays in place");
            // The newest broken file replaces the older one.
            assert_eq!(std::fs::read(&bad).expect("set aside"), broken);
        }

        // Settings saved after that load as usual.
        full(dir.path()).save(dir.path()).expect("saved");
        assert_eq!(load(dir.path()), full(dir.path()));
    }

    #[test]
    fn a_huge_file_is_moved_aside() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let huge = format!(
            "# {}\n",
            "x".repeat(usize::try_from(MAX_FILE_SIZE).unwrap())
        );
        std::fs::write(dir.path().join(FILE_NAME), huge).expect("written");
        assert_eq!(load(dir.path()), Settings::default());
        assert!(dir.path().join(BAD_FILE_NAME).is_file());
    }

    #[test]
    fn unparsable_tabs_are_left_out() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let a = folder(dir.path(), "a");
        let b = folder(dir.path(), "b");
        let mut settings = Settings::default();
        settings.panes[0].tabs = vec![a.to_uri(), "nonsense".to_owned(), b.to_uri()];
        settings.panes[0].active = 2;
        settings.panes[1].tabs = vec!["nonsense".to_owned(), a.to_uri()];
        settings.panes[1].active = 0;
        settings.active_pane = 5;

        let session = settings.session();

        assert_eq!(session.panes[0].tabs, [a.clone(), b.clone()]);
        assert_eq!(session.panes[0].active, 1, "the same tab still shows");
        assert_eq!(session.panes[1].tabs, [a]);
        assert_eq!(session.panes[1].active, 0, "the first one shows");
        assert_eq!(session.active_pane, 1);
    }

    #[test]
    fn missing_folders_fall_back_to_home() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let home = folder(dir.path(), "home");
        let a = folder(dir.path(), "a");
        let b = folder(dir.path(), "b");
        let gone = folder(dir.path(), "gone");
        let slow = folder(dir.path(), "slow");
        for made in [&home, &a, &b, &slow] {
            std::fs::create_dir(made.as_local().expect("local")).expect("made");
        }
        let mut stored = Settings::default();
        stored.set_session(&Session {
            panes: [
                SessionPane {
                    tabs: vec![a.clone(), gone.clone(), b.clone()],
                    active: 1,
                },
                SessionPane {
                    tabs: vec![slow.clone(), gone.clone()],
                    active: 0,
                },
            ],
            active_pane: 1,
            show_hidden: false,
        });
        stored.save(dir.path()).expect("saved");
        let session = load(dir.path()).session();
        assert_eq!(
            session.folders(),
            [a.clone(), gone, b.clone(), slow.clone()]
        );

        // The slow folder is not looked at in time.
        let (restored, waiting) = session.resolve(&home, |path| {
            (*path != slow).then(|| path.as_local().is_some_and(Path::is_dir))
        });

        assert_eq!(restored.panes[0].tabs, [a, home.clone(), b]);
        assert_eq!(restored.panes[0].active, 1);
        assert_eq!(restored.panes[1].tabs, [home.clone(), home]);
        assert_eq!(restored.active_pane, 1);
        assert_eq!(
            waiting,
            [Waiting {
                side: 1,
                tab: 0,
                path: slow
            }]
        );
    }

    #[test]
    fn the_window_size_is_kept_in_bounds() {
        let sized = |window| Settings {
            window: Some(window),
            ..Settings::default()
        };
        assert_eq!(sized((1500, 900)).window_size(), Some((1500, 900)));
        assert_eq!(sized((300, 200)).window_size(), Some(MIN_WINDOW));
        assert_eq!(sized((1000, 100)).window_size(), Some((1000, 560)));
        assert_eq!(sized((u32::MAX, u32::MAX)).window_size(), Some(MAX_WINDOW));
    }

    #[test]
    fn the_saver_keeps_the_last_settings_asked_for() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let saver = Saver::new(dir.path().to_owned());
        let (sent, received) = std::sync::mpsc::channel();
        for theme in [ThemeChoice::Light, ThemeChoice::Dark, ThemeChoice::System] {
            let sent = sent.clone();
            let settings = Settings {
                theme,
                ..Settings::default()
            };
            saver
                .save_in_background(settings, move |result| {
                    let _ = sent.send(result.is_ok());
                })
                .expect("the thread starts");
        }
        let last = Settings {
            theme: ThemeChoice::Dark,
            show_hidden: true,
            ..Settings::default()
        };
        saver.save(&last).expect("saved");
        for _ in 0..3 {
            assert!(received.recv().expect("each write reports"));
        }
        assert_eq!(load(dir.path()), last);
    }

    #[test]
    fn a_save_that_fails_leaves_no_temp_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        // A folder in the place of the file: the rename cannot replace it.
        std::fs::create_dir(dir.path().join(FILE_NAME)).expect("made");
        std::fs::write(dir.path().join(FILE_NAME).join("x"), "x").expect("written");
        assert!(full(dir.path()).save(dir.path()).is_err());
        assert!(!dir.path().join(TEMP_FILE_NAME).exists());
    }
}
