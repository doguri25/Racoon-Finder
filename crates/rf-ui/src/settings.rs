//! The settings kept between runs, in `settings.toml` in the app's config folder
//! (`AppPaths::config_dir`): the theme, whether hidden items show, the window's size and layout
//! (`[layout]`), the tabs of both panes (in their order, pinned or not) and which one has the
//! keyboard, and the destination picker's recent targets.
//!
//! [`load`] reads them once at the start, before the window shows; nothing here fails the start.
//! A file that is broken (it does not parse, is not text, or is far too large) is moved aside to
//! `settings.toml.bad` (one is kept: a newer one replaces it) and the app starts with the
//! defaults. A file that is there but cannot be read, or cannot be moved aside, is left alone:
//! the app starts with the defaults and does not save over it ([`Loaded::writable`]).
//!
//! [`Saver`] writes them atomically, on worker threads, one at a time: to `settings.toml.tmp` in
//! the same folder, synced to the disk, then renamed over the old file, so a crash leaves the
//! old settings or the new ones, never half a file.
//!
//! A newer version's settings read as well as they can: keys this version does not know are
//! kept in the `extra` tables and written back, a known key with a value this version does not
//! know (a new theme) takes its default, and a tab whose folder this version cannot read stays
//! in the file. Running an older version in between loses as little as it can. The other way
//! round, a file of an older version (the first demo's, without `[layout]` or `pinned`) opens
//! with the default layout and no tab pinned.
//!
//! Folders are stored as `VPath::to_uri`, which reads back losslessly with `VPath::parse`.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use rf_core::VPath;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::state::Layout;

/// The name of the settings file in the config folder.
pub const FILE_NAME: &str = "settings.toml";
/// Where a broken settings file is moved to.
pub const BAD_FILE_NAME: &str = "settings.toml.bad";
/// The file a save writes before it takes the place of the settings file.
pub const TEMP_FILE_NAME: &str = "settings.toml.tmp";
/// A settings file larger than this is no file this app wrote; it is treated as broken.
const MAX_FILE_SIZE: u64 = 1024 * 1024;

/// How many times a save tries to put its file in place. On Windows an antivirus, an indexer
/// or a sync tool that has the old file open makes the rename fail for a moment.
const RENAME_TRIES: u32 = if cfg!(windows) { 3 } else { 1 };
/// The pause between two tries of the rename.
const RENAME_PAUSE: Duration = Duration::from_millis(50);

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
    #[serde(deserialize_with = "or_default")]
    pub tabs: Vec<String>,
    /// The position of the tab that shows.
    #[serde(deserialize_with = "or_default")]
    pub active: usize,
    /// Whether each tab of `tabs` is pinned, in the same order, as far as the last pinned one:
    /// a tab without a flag is not. Not written when no tab is pinned.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "or_default")]
    pub pinned: Vec<bool>,
    /// The keys of a newer version.
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// `flags`, one for each tab, without the ones after the last `true`: none at all when no tab
/// is pinned, as [`PaneSettings::pinned`] and [`SessionPane::pinned`] keep them.
pub fn pinned_flags(flags: impl IntoIterator<Item = bool>) -> Vec<bool> {
    let mut flags: Vec<bool> = flags.into_iter().collect();
    let kept = flags
        .iter()
        .rposition(|&pinned| pinned)
        .map_or(0, |last| last + 1);
    flags.truncate(kept);
    flags
}

/// The window's layout, as stored (`[layout]`). Each key the file lacks, or whose value this
/// version cannot read, takes its default: two panes side by side, split evenly, the tree shown
/// and the preview panel not. So the names say what differs from that default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each key is a separate choice of the layout, kept as the file has it"
)]
pub struct LayoutSettings {
    /// One pane rather than two.
    #[serde(deserialize_with = "or_default")]
    pub single_pane: bool,
    /// The panes one above the other rather than side by side.
    #[serde(deserialize_with = "or_default")]
    pub stacked: bool,
    /// The first pane's share of the room, from 0.1 to 0.9 (one outside is taken to the
    /// nearer end); `None` for an even split.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "or_default"
    )]
    pub split: Option<f64>,
    /// No sidebar with the folder tree.
    #[serde(deserialize_with = "or_default")]
    pub hide_tree: bool,
    /// The preview panel shows.
    #[serde(deserialize_with = "or_default")]
    pub preview: bool,
    /// The keys of a newer version.
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl LayoutSettings {
    /// The layout these settings say.
    pub fn layout(&self) -> Layout {
        let mut layout = Layout::default();
        layout.dual = !self.single_pane;
        layout.stacked = self.stacked;
        layout.tree = !self.hide_tree;
        layout.preview = self.preview;
        if let Some(split) = self.split {
            layout.set_share(split);
        }
        layout
    }

    /// Takes `layout` in, keeping the keys of a newer version.
    pub fn set(&mut self, layout: Layout) {
        self.single_pane = !layout.dual;
        self.stacked = layout.stacked;
        self.split = Some(f64::from(layout.thousandths()) / 1000.0);
        self.hide_tree = !layout.tree;
        self.preview = layout.preview;
    }
}

/// Everything `settings.toml` holds. A key the file lacks, or whose value this version cannot
/// read, takes its default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(deserialize_with = "or_default")]
    pub theme: ThemeChoice,
    #[serde(deserialize_with = "or_default")]
    pub show_hidden: bool,
    /// The window's last size in logical pixels (width, height), when it was neither maximized
    /// nor full screen; `None` until the first save.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "or_default"
    )]
    pub window: Option<(u32, u32)>,
    /// The pane with the keyboard: 0 left, 1 right.
    #[serde(deserialize_with = "or_default")]
    pub active_pane: usize,
    /// The destination picker's recent targets, the last one chosen first, as `VPath::to_uri`.
    #[serde(deserialize_with = "or_default")]
    pub recent_targets: Vec<String>,
    /// How the window lays out its panes and side panels.
    #[serde(deserialize_with = "or_default")]
    pub layout: LayoutSettings,
    /// The left pane, then the right one. Read as a list: a missing pane is an empty one, and
    /// a pane past the second (a newer version's) is kept as it is.
    #[serde(deserialize_with = "panes_or_default")]
    pub panes: Vec<PaneSettings>,
    /// The keys of a newer version.
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// A known key's value as `T`, or `T`'s default when it is no `T` (the wrong type, or a name a
/// newer version added): one odd value does not cost the whole file.
fn or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = toml::Value::deserialize(deserializer)?;
    Ok(value.try_into().unwrap_or_default())
}

/// The panes as a list, each one that cannot be read an empty pane; no list at all is none.
fn panes_or_default<'de, D>(deserializer: D) -> Result<Vec<PaneSettings>, D::Error>
where
    D: Deserializer<'de>,
{
    let toml::Value::Array(panes) = toml::Value::deserialize(deserializer)? else {
        return Ok(Vec::new());
    };
    Ok(panes
        .into_iter()
        .map(|pane| pane.try_into().unwrap_or_default())
        .collect())
}

/// What the window showed when the app last ended, as folders: the tabs of each pane and the
/// one that shows, the pane with the keyboard, whether hidden items show, and the layout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// The left pane, then the right one.
    pub panes: [SessionPane; 2],
    /// 0 left, 1 right.
    pub active_pane: usize,
    pub show_hidden: bool,
    pub layout: Layout,
}

/// The tabs of one pane: their folders in order, the position of the one that shows, and which
/// are pinned. A pane without tabs opens its default folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionPane {
    pub tabs: Vec<VPath>,
    pub active: usize,
    /// Whether each tab is pinned, in the order of `tabs`, as far as the last pinned one
    /// ([`pinned_flags`]): a tab without a flag is not.
    pub pinned: Vec<bool>,
}

/// What a look at a folder of the last run's tabs found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// It is there (or a link to it is).
    Folder,
    /// It is gone: the folders that held it are there, and it is not.
    Gone,
    /// Its drive, share or volume is not there (unplugged, not connected yet): it may come
    /// back.
    Offline,
}

/// A tab of a restored [`Session`] that shows home in place of its folder `path`: one not
/// looked at in time (a slow or sleeping drive), which goes to its folder once that is found,
/// or one on a drive that is not there. Until the user moves the tab, the settings keep `path`.
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

    /// The session to open at the start, with what `look` found at each tab's folder: a folder
    /// that is gone shows `home` instead. A folder on a drive that is not there, or one `look`
    /// cannot tell about yet (`None`: a slow or sleeping drive), shows `home` too, so the window
    /// need not wait for it, and its tab comes back as a [`Waiting`].
    pub fn resolve(
        mut self,
        home: &VPath,
        mut look: impl FnMut(&VPath) -> Option<Found>,
    ) -> (Session, Vec<Waiting>) {
        let mut waiting = Vec::new();
        for (side, pane) in self.panes.iter_mut().enumerate() {
            for (tab, path) in pane.tabs.iter_mut().enumerate() {
                match look(path) {
                    Some(Found::Folder) => continue,
                    Some(Found::Gone) => {}
                    Some(Found::Offline) | None => waiting.push(Waiting {
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

    /// The session with `folder`, the folder the app was asked to open, showing in the left
    /// pane, which gets the keyboard: in the left tab that shows it already, in the one that
    /// stands in for it (it shows the folder now, as asked, and no longer waits in `waiting`),
    /// or else in a new tab after the others. The other tabs of `waiting` show home in place of
    /// their folder, so they are not taken for it. `found` is what a look at `folder` found, if
    /// it was looked at in time: a folder that is gone adds no tab to a pane that has some
    /// (asked for at every start, it would add one each time), and leaves the session as it is.
    /// `true` when the session shows the folder.
    #[must_use]
    pub fn with_start(
        mut self,
        folder: VPath,
        found: Option<Found>,
        waiting: &mut Vec<Waiting>,
    ) -> (Session, bool) {
        let left = &mut self.panes[0];
        let standing_in = |tab: usize| waiting.iter().any(|w| w.side == 0 && w.tab == tab);
        let shown = left
            .tabs
            .iter()
            .enumerate()
            .position(|(tab, path)| *path == folder && !standing_in(tab));
        let stand_in = || {
            waiting
                .iter()
                .position(|w| w.side == 0 && w.path == folder && w.tab < left.tabs.len())
        };
        left.active = match (shown, stand_in()) {
            (Some(tab), _) => tab,
            (None, Some(index)) => {
                let tab = waiting.remove(index).tab;
                left.tabs[tab] = folder;
                tab
            }
            (None, None) if found == Some(Found::Gone) && !left.tabs.is_empty() => {
                return (self, false);
            }
            (None, None) => {
                left.tabs.push(folder);
                left.tabs.len() - 1
            }
        };
        self.active_pane = 0;
        (self, true)
    }
}

/// What [`load`] found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loaded {
    pub settings: Settings,
    /// Whether the settings may be written back. `false` when a settings file is there that
    /// could not be read (an I/O error) or could not be moved aside: a save would replace it
    /// unread, so the run keeps what it changes to itself.
    pub writable: bool,
}

impl Settings {
    /// The tabs and which are pinned, the active pane, the hidden items and the layout. A tab
    /// whose folder does not read back as a path is left out (it stays in the file, with its
    /// pin, [`Settings::set_session`]); the tab that shows stays the same one when it is kept,
    /// and is the first otherwise.
    pub fn session(&self) -> Session {
        let pane = |side: usize| {
            let mut pane = SessionPane::default();
            let Some(stored) = self.panes.get(side) else {
                return pane;
            };
            let mut pinned = Vec::new();
            for (i, uri) in stored.tabs.iter().enumerate() {
                if let Ok(path) = VPath::parse(uri) {
                    if i == stored.active {
                        pane.active = pane.tabs.len();
                    }
                    pane.tabs.push(path);
                    pinned.push(stored.pinned.get(i).copied().unwrap_or(false));
                }
            }
            pane.pinned = pinned_flags(pinned);
            pane
        };
        Session {
            panes: [pane(0), pane(1)],
            active_pane: self.active_pane.min(1),
            show_hidden: self.show_hidden,
            layout: self.layout.layout(),
        }
    }

    /// Takes `session` in, keeping the keys and the panes of a newer version, and the tabs whose
    /// folder this version cannot read, with their pins, after the others.
    pub fn set_session(&mut self, session: &Session) {
        if self.panes.len() < 2 {
            self.panes.resize_with(2, PaneSettings::default);
        }
        for (stored, pane) in self.panes.iter_mut().zip(&session.panes) {
            let (unread, unread_pins): (Vec<String>, Vec<bool>) = stored
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, uri)| VPath::parse(uri).is_err())
                .map(|(i, uri)| (uri.clone(), stored.pinned.get(i).copied().unwrap_or(false)))
                .unzip();
            let pins = (0..pane.tabs.len()).map(|i| pane.pinned.get(i).copied().unwrap_or(false));
            stored.pinned = pinned_flags(pins.chain(unread_pins));
            stored.tabs = pane.tabs.iter().map(VPath::to_uri).chain(unread).collect();
            stored.active = pane.active;
        }
        self.active_pane = session.active_pane;
        self.show_hidden = session.show_hidden;
        self.layout.set(session.layout);
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

    /// The text of `settings.toml` for these settings. Two settings are the same to save when
    /// their texts are: a newer version's `nan` is never equal to itself as a value.
    pub fn to_text(&self) -> io::Result<String> {
        toml::to_string(self).map_err(io::Error::other)
    }

    /// Writes the settings to `config_dir` at once ([`write_text`]). Blocks on the disk.
    pub fn save(&self, config_dir: &Path) -> io::Result<()> {
        write_text(config_dir, &self.to_text()?)
    }
}

/// Writes `text` as `settings.toml` in `config_dir` (made if missing), atomically: to
/// `settings.toml.tmp` there, synced to the disk with `rf_platform::sync_file`, then renamed
/// over the old file (tried again a few times on Windows), and the folder synced. The
/// temporary file is removed when a step fails.
///
/// Blocks on the disk: call it on a worker thread ([`Saver`]).
pub fn write_text(config_dir: &Path, text: &str) -> io::Result<()> {
    std::fs::create_dir_all(config_dir)?;
    let temp = config_dir.join(TEMP_FILE_NAME);
    let written = write_synced(&temp, text.as_bytes())
        .and_then(|()| rename_retrying(&temp, &config_dir.join(FILE_NAME)));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
        return written;
    }
    // Makes the rename durable. A folder that cannot be synced leaves, after a power loss, the
    // old settings at worst, never half a file: nothing more to do then.
    let _ = rf_platform::sync_dir(config_dir);
    Ok(())
}

/// Writes `bytes` to a new file at `path` and makes it durable.
fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    rf_platform::sync_file(&file)
}

/// Renames `from` over `to`, [`RENAME_TRIES`] times at most.
fn rename_retrying(from: &Path, to: &Path) -> io::Result<()> {
    let mut tries = RENAME_TRIES;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => {
                tries -= 1;
                if tries == 0 {
                    return Err(error);
                }
                std::thread::sleep(RENAME_PAUSE);
            }
        }
    }
}

/// Reads `settings.toml` in `config_dir`. Without one (the first run) the settings are the
/// defaults. A broken one (it does not parse, is not text, or is too large) is moved aside to
/// `settings.toml.bad`, replacing an older one there, and the settings are the defaults: the
/// app starts either way, and the broken file stays for a look. One that cannot be read (an
/// I/O error), or that cannot be moved aside, stays where it is, and the settings are the
/// defaults but not [`Loaded::writable`]: a save would replace it unread.
///
/// It reads a small local file and is called once at the start, before the window shows: the
/// UI thread may call it then.
pub fn load(config_dir: &Path) -> Loaded {
    let path = config_dir.join(FILE_NAME);
    let broken = match read_small(&path) {
        Ok(text) => match toml::from_str(&text) {
            Ok(settings) => {
                return Loaded {
                    settings,
                    writable: true,
                };
            }
            Err(_) => true,
        },
        Err(error) => match error.kind() {
            io::ErrorKind::NotFound => {
                return Loaded {
                    settings: Settings::default(),
                    writable: true,
                };
            }
            // Not text, or far too large: no settings file this app wrote.
            io::ErrorKind::InvalidData => true,
            _ => false,
        },
    };
    let writable = broken && std::fs::rename(&path, config_dir.join(BAD_FILE_NAME)).is_ok();
    Loaded {
        settings: Settings::default(),
        writable,
    }
}

/// The theme of `settings.toml` in `config_dir`, for a window shown while another instance
/// runs (it says so): the file is only read. Without a file, or with one [`load`] would not
/// take, it is the default theme. A small local file; the UI thread may call it.
pub fn peek_theme(config_dir: &Path) -> ThemeChoice {
    read_small(&config_dir.join(FILE_NAME))
        .ok()
        .and_then(|text| toml::from_str::<Settings>(&text).ok())
        .map(|settings| settings.theme)
        .unwrap_or_default()
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

/// Writes the settings to the config folder on worker threads, one write at a time. Every
/// write has a number in the order it was asked for, and a write whose number is below that of
/// one already written is skipped: the settings asked for last are the ones that stay, even
/// when an earlier write waited longer. Clones share the order.
///
/// A saver that may not write ([`Loaded::writable`]) writes nothing and reports success.
#[derive(Clone, Debug)]
pub struct Saver {
    config_dir: Arc<PathBuf>,
    writable: bool,
    /// The number of the last write done; held while a write runs.
    written: Arc<Mutex<u64>>,
    next: Arc<AtomicU64>,
}

impl Saver {
    pub fn new(config_dir: PathBuf, writable: bool) -> Saver {
        Saver {
            config_dir: Arc::new(config_dir),
            writable,
            written: Arc::new(Mutex::new(0)),
            next: Arc::new(AtomicU64::new(1)),
        }
    }

    /// The folder the settings go to.
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// Whether this saver writes at all.
    pub fn writable(&self) -> bool {
        self.writable
    }

    /// Writes `text` ([`Settings::to_text`]) on a new worker thread and returns at once; `done`
    /// gets the result there, `Ok` too when a later write made this one needless. `Err` when
    /// the thread could not start: nothing was written then.
    pub fn save_in_background(
        &self,
        text: String,
        done: impl FnOnce(io::Result<()>) + Send + 'static,
    ) -> io::Result<()> {
        let number = self.next.fetch_add(1, Ordering::SeqCst);
        let saver = self.clone();
        std::thread::Builder::new()
            .name("rf-ui-settings".to_owned())
            .spawn(move || done(saver.write(number, &text)))
            .map(drop)
    }

    /// Writes `text` on a worker thread, after any write in progress, and waits up to `wait`
    /// for it: its result, or `None` when it has not ended by then (a hung network home; the
    /// write goes on, and the file is the old one or the new one either way). For the app's
    /// end, which must not hang on a disk. Blocks: call it on a worker thread.
    pub fn save_within(&self, text: String, wait: Duration) -> Option<io::Result<()>> {
        let (sent, result) = mpsc::channel();
        let started = self.save_in_background(text, move |done| {
            let _ = sent.send(done);
        });
        if let Err(error) = started {
            return Some(Err(error));
        }
        result.recv_timeout(wait).ok()
    }

    fn write(&self, number: u64, text: &str) -> io::Result<()> {
        if !self.writable {
            return Ok(());
        }
        let mut written = self.written.lock().unwrap_or_else(PoisonError::into_inner);
        if *written > number {
            return Ok(());
        }
        *written = number;
        write_text(&self.config_dir, text)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::state::Layout;

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
                    pinned: vec![true],
                },
                SessionPane {
                    tabs: vec![folder(dir, "c")],
                    active: 0,
                    pinned: Vec::new(),
                },
            ],
            active_pane: 1,
            show_hidden: true,
            layout: odd_layout(),
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

    /// A layout with every value off its default.
    fn odd_layout() -> Layout {
        let mut layout = Layout::default();
        layout.dual = false;
        layout.stacked = true;
        layout.tree = false;
        layout.preview = true;
        layout.set_share(0.3);
        layout
    }

    fn writable(settings: Settings) -> Loaded {
        Loaded {
            settings,
            writable: true,
        }
    }

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let settings = full(dir.path());

        settings.save(dir.path()).expect("saved");
        assert!(dir.path().join(FILE_NAME).is_file());
        assert!(!dir.path().join(TEMP_FILE_NAME).exists());
        let loaded = load(dir.path());

        assert_eq!(loaded, writable(settings.clone()));
        assert_eq!(loaded.settings.session(), settings.session());
        assert_eq!(loaded.settings.session().layout, odd_layout());
        assert_eq!(
            loaded.settings.recent(),
            [folder(dir.path(), "r1"), folder(dir.path(), "r2")]
        );
        // A second save replaces the first.
        let mut changed = loaded.settings;
        changed.theme = ThemeChoice::Light;
        changed.save(dir.path()).expect("saved again");
        assert_eq!(load(dir.path()).settings.theme, ThemeChoice::Light);
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_name_that_is_not_text_round_trips() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().expect("a temp dir");
        let odd = VPath::local(dir.path().join(OsStr::from_bytes(b"caf\xE9 \xFF")))
            .expect("a local path");
        let mut settings = Settings::default();
        settings.set_session(&Session {
            panes: [
                SessionPane {
                    tabs: vec![odd.clone()],
                    active: 0,
                    pinned: Vec::new(),
                },
                SessionPane::default(),
            ],
            ..Session::default()
        });
        settings.save(dir.path()).expect("saved");

        assert_eq!(load(dir.path()).settings.session().panes[0].tabs, [odd]);
    }

    #[test]
    fn missing_file_gives_the_defaults() {
        let dir = tempfile::tempdir().expect("a temp dir");
        assert_eq!(load(dir.path()), writable(Settings::default()));
        assert!(!dir.path().join(BAD_FILE_NAME).exists());
        assert_eq!(Settings::default().theme, ThemeChoice::System);
        assert_eq!(Settings::default().window_size(), None);
    }

    #[test]
    fn the_theme_is_peeked_at_without_touching_the_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        assert_eq!(peek_theme(dir.path()), ThemeChoice::System);
        assert!(!file.exists());
        std::fs::write(&file, "theme = \"dark\"\nshow_hidden = true\n").expect("written");
        assert_eq!(peek_theme(dir.path()), ThemeChoice::Dark);
        // A broken file stays where it is, for the instance that runs to look at.
        let broken = "theme = \"dark\"\npanes = [[[[\n";
        std::fs::write(&file, broken).expect("written");
        assert_eq!(peek_theme(dir.path()), ThemeChoice::System);
        assert_eq!(std::fs::read_to_string(&file).expect("read"), broken);
        assert!(!dir.path().join(BAD_FILE_NAME).exists());
    }

    #[test]
    fn keys_a_file_lacks_take_their_defaults() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join(FILE_NAME), "theme = \"light\"\n").expect("written");
        let loaded = load(dir.path());
        assert!(loaded.writable);
        assert_eq!(loaded.settings.theme, ThemeChoice::Light);
        assert!(!loaded.settings.show_hidden);
        assert_eq!(loaded.settings.session(), Session::default());
    }

    #[test]
    fn a_file_of_the_first_demo_opens_with_the_default_layout() {
        // As the first demo wrote it: no layout.
        let dir = tempfile::tempdir().expect("a temp dir");
        let a = folder(dir.path(), "a");
        let c = folder(dir.path(), "c");
        let (qa, qc) = (toml_string(&a.to_uri()), toml_string(&c.to_uri()));
        let text = format!(
            "theme = \"dark\"\nshow_hidden = true\nwindow = [1500, 900]\nactive_pane = 1\n\
             recent_targets = [{qc}]\n\n[[panes]]\ntabs = [{qa}, {qc}]\nactive = 1\n\n\
             [[panes]]\ntabs = [{qc}]\nactive = 0\n"
        );
        std::fs::write(dir.path().join(FILE_NAME), &text).expect("written");

        let loaded = load(dir.path());

        assert!(loaded.writable);
        let settings = loaded.settings;
        assert_eq!(settings.theme, ThemeChoice::Dark);
        assert_eq!(settings.window_size(), Some((1500, 900)));
        assert_eq!(settings.recent(), std::slice::from_ref(&c));
        assert!(settings.extra.is_empty());
        let session = settings.session();
        // The tabs in their order, none of them pinned.
        assert_eq!(session.panes[0].tabs, [a.clone(), c.clone()]);
        assert_eq!(session.panes[0].active, 1);
        assert_eq!(session.panes[0].pinned, Vec::<bool>::new());
        assert_eq!(session.panes[1].pinned, Vec::<bool>::new());
        assert_eq!(session.active_pane, 1);
        assert!(session.show_hidden);
        assert_eq!(session.layout, Layout::default());

        // Saved again with another layout, it keeps the rest as it was.
        let mut changed = session.clone();
        changed.layout = odd_layout();
        let mut saved = settings.clone();
        saved.set_session(&changed);
        saved.save(dir.path()).expect("saved");
        let again = load(dir.path()).settings;
        assert_eq!(again.session(), changed);
        assert_eq!(again.theme, ThemeChoice::Dark);
        assert_eq!(again.recent(), [c]);
    }

    #[test]
    fn layout_values_of_a_newer_version_fall_back_one_key_at_a_time() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        std::fs::write(
            &file,
            "[layout]\nsingle_pane = \"maybe\"\nstacked = true\nsplit = 7.5\nhide_tree = 1\n\
             preview = true\ngrid = \"2x2\"\n",
        )
        .expect("written");

        let settings = load(dir.path()).settings;

        let layout = settings.session().layout;
        assert!(layout.dual, "a value it cannot read keeps two panes");
        assert!(layout.stacked);
        assert!((layout.share() - 0.9).abs() < 1e-6, "{}", layout.share());
        assert!(layout.tree);
        assert!(layout.preview);
        // The key of a newer version stays, and is written back.
        let text = settings.to_text().expect("text");
        assert!(text.contains("grid = \"2x2\""), "{text}");

        // A share that is no number is an even one; a layout that is no table, the default.
        std::fs::write(&file, "[layout]\nsplit = nan\nstacked = true\n").expect("written");
        let layout = load(dir.path()).settings.session().layout;
        assert!(layout.stacked);
        assert!((layout.share() - 0.5).abs() < 1e-6);
        std::fs::write(&file, "layout = 5\ntheme = \"light\"\n").expect("written");
        let settings = load(dir.path()).settings;
        assert_eq!(settings.theme, ThemeChoice::Light);
        assert_eq!(settings.session().layout, Layout::default());
    }

    #[test]
    fn pinned_tabs_keep_their_flags_beside_their_folders() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let (a, b) = (folder(dir.path(), "a"), folder(dir.path(), "b"));
        let session = |pinned: Vec<bool>| Session {
            panes: [
                SessionPane {
                    tabs: vec![a.clone(), b.clone()],
                    active: 0,
                    pinned,
                },
                SessionPane::default(),
            ],
            ..Session::default()
        };
        // A flag for each tab, as far as the last pinned one.
        let mut settings = Settings::default();
        settings.set_session(&session(vec![true]));
        let text = settings.to_text().expect("text");
        assert!(text.contains("pinned = [true]"), "{text}");
        settings.save(dir.path()).expect("saved");
        assert_eq!(load(dir.path()).settings.session(), session(vec![true]));
        // Nothing pinned, nothing written: the file reads as the first demo's did.
        settings.set_session(&session(Vec::new()));
        assert!(!settings.to_text().expect("text").contains("pinned"));

        // A tab this version cannot read keeps its flag, after the others.
        let mut stored = Settings {
            panes: vec![PaneSettings {
                tabs: vec!["cloud+new://x".to_owned(), a.to_uri(), b.to_uri()],
                pinned: vec![true, false, true],
                ..PaneSettings::default()
            }],
            ..Settings::default()
        };
        let read = stored.session();
        assert_eq!(read.panes[0].tabs, [a.clone(), b.clone()]);
        assert_eq!(read.panes[0].pinned, [false, true]);
        stored.set_session(&read);
        assert_eq!(
            stored.panes[0].tabs,
            [a.to_uri(), b.to_uri(), "cloud+new://x".to_owned()]
        );
        assert_eq!(stored.panes[0].pinned, [false, true, true]);

        // Flags of a newer version that are no list of flags pin nothing; the tabs stay.
        let (qa, qb) = (toml_string(&a.to_uri()), toml_string(&b.to_uri()));
        std::fs::write(
            dir.path().join(FILE_NAME),
            format!("[[panes]]\ntabs = [{qa}, {qb}]\npinned = \"all\"\nactive = 1\n"),
        )
        .expect("written");
        let read = load(dir.path()).settings.session();
        assert_eq!(read.panes[0].tabs, [a, b]);
        assert_eq!(read.panes[0].active, 1);
        assert_eq!(read.panes[0].pinned, Vec::<bool>::new());
    }

    /// `text` as a TOML string, quoted and escaped: a Windows path's `\` would otherwise
    /// start an escape, which makes the file one that does not parse.
    fn toml_string(text: &str) -> String {
        toml::Value::String(text.to_owned()).to_string()
    }

    #[test]
    fn windows_folders_round_trip_on_every_system() {
        // A file saved on Windows holds a `\` in every folder; it reads back as it was written.
        let dir = tempfile::tempdir().expect("a temp dir");
        let folders = [r"local:C:\Users\me\설계 문서", r"local:\\srv\share\"];
        let settings = Settings {
            recent_targets: folders.map(str::to_owned).to_vec(),
            ..Settings::default()
        };
        settings.save(dir.path()).expect("saved");
        assert_eq!(load(dir.path()), writable(settings));
        // So does a file the tests write by hand with `toml_string`.
        let text = format!("recent_targets = [{}]\n", toml_string(folders[0]));
        std::fs::write(dir.path().join(FILE_NAME), text).expect("written");
        assert_eq!(load(dir.path()).settings.recent_targets, [folders[0]]);
    }

    #[test]
    fn values_of_a_newer_version_fall_back_one_key_at_a_time() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let a = folder(dir.path(), "a").to_uri();
        let quoted = toml_string(&a);
        let text = format!(
            "theme = \"high-contrast\"\nshow_hidden = 3\nwindow = \"big\"\nactive_pane = 1\n\
             recent_targets = [{quoted}]\npanes = [{{ tabs = [{quoted}], active = \"x\" }}, 5, \
             {{ tabs = [{quoted}], layout = \"grid\" }}]\n"
        );
        std::fs::write(dir.path().join(FILE_NAME), &text).expect("written");

        let loaded = load(dir.path());

        assert!(loaded.writable);
        assert!(!dir.path().join(BAD_FILE_NAME).exists());
        let settings = loaded.settings;
        assert_eq!(settings.theme, ThemeChoice::System);
        assert!(!settings.show_hidden);
        assert_eq!(settings.window, None);
        assert_eq!(settings.active_pane, 1);
        assert_eq!(settings.recent_targets, std::slice::from_ref(&a));
        assert_eq!(settings.panes.len(), 3, "a third pane is kept");
        assert_eq!(settings.panes[0].tabs, std::slice::from_ref(&a));
        assert_eq!(settings.panes[0].active, 0);
        assert_eq!(settings.panes[1], PaneSettings::default());
        assert_eq!(
            settings.panes[2].extra.get("layout"),
            Some(&toml::Value::String("grid".to_owned()))
        );
        // A file with one pane, or none, opens the other with its default.
        std::fs::write(
            dir.path().join(FILE_NAME),
            format!("[[panes]]\ntabs = [{quoted}]\n"),
        )
        .expect("written");
        let session = load(dir.path()).settings.session();
        assert_eq!(session.panes[0].tabs.len(), 1);
        assert_eq!(session.panes[1], SessionPane::default());
    }

    #[test]
    fn tabs_this_version_cannot_read_stay_in_the_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let a = folder(dir.path(), "a");
        let b = folder(dir.path(), "b");
        let mut settings = Settings {
            panes: vec![
                PaneSettings {
                    tabs: vec![a.to_uri(), "cloud+new://x/y".to_owned(), b.to_uri()],
                    active: 2,
                    ..PaneSettings::default()
                },
                PaneSettings {
                    tabs: vec!["nonsense".to_owned(), a.to_uri()],
                    active: 0,
                    ..PaneSettings::default()
                },
            ],
            active_pane: 5,
            ..Settings::default()
        };

        let session = settings.session();

        assert_eq!(session.panes[0].tabs, [a.clone(), b.clone()]);
        assert_eq!(session.panes[0].active, 1, "the same tab still shows");
        assert_eq!(session.panes[1].tabs, std::slice::from_ref(&a));
        assert_eq!(session.panes[1].active, 0, "the first one shows");
        assert_eq!(session.active_pane, 1);

        // Saving the session back keeps them, after the tabs that open; twice too.
        settings.set_session(&session);
        settings.set_session(&session);
        assert_eq!(
            settings.panes[0].tabs,
            [a.to_uri(), b.to_uri(), "cloud+new://x/y".to_owned()]
        );
        assert_eq!(settings.panes[0].active, 1);
        assert_eq!(settings.panes[1].tabs, [a.to_uri(), "nonsense".to_owned()]);
    }

    #[test]
    fn corrupt_file_is_moved_aside() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        let bad = dir.path().join(BAD_FILE_NAME);
        std::fs::write(&bad, "an older broken file").expect("written");

        for broken in [
            &b"theme = \"dark\"\npanes = [[[[\n"[..],
            b"theme = \"dark\"\ntheme = \"light\"\n",
            b"\xFF\xFE not text",
        ] {
            std::fs::write(&file, broken).expect("written");
            assert_eq!(load(dir.path()), writable(Settings::default()));
            assert!(!file.exists(), "{broken:?} stays in place");
            // The newest broken file replaces the older one.
            assert_eq!(std::fs::read(&bad).expect("set aside"), broken);
        }

        // Settings saved after that load as usual.
        full(dir.path()).save(dir.path()).expect("saved");
        assert_eq!(load(dir.path()), writable(full(dir.path())));
    }

    #[test]
    fn a_huge_file_is_moved_aside() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let huge = format!(
            "# {}\n",
            "x".repeat(usize::try_from(MAX_FILE_SIZE).unwrap())
        );
        std::fs::write(dir.path().join(FILE_NAME), huge).expect("written");
        assert_eq!(load(dir.path()), writable(Settings::default()));
        assert!(dir.path().join(BAD_FILE_NAME).is_file());
    }

    #[test]
    fn a_file_that_cannot_be_read_or_set_aside_is_never_replaced() {
        // A folder in the place of the file: it cannot be read, and stays.
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        std::fs::create_dir(&file).expect("made");
        std::fs::write(file.join("keep"), "keep").expect("written");
        let loaded = load(dir.path());
        assert_eq!(loaded.settings, Settings::default());
        assert!(!loaded.writable);
        assert!(file.join("keep").is_file());
        assert!(!dir.path().join(BAD_FILE_NAME).exists());

        // A broken file whose place aside is taken by a folder that is not empty: it stays,
        // and the run that starts with the defaults never saves over it.
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        let bad = dir.path().join(BAD_FILE_NAME);
        std::fs::create_dir(&bad).expect("made");
        std::fs::write(bad.join("x"), "x").expect("written");
        std::fs::write(&file, "theme = [[[").expect("written");
        let loaded = load(dir.path());
        assert!(!loaded.writable);
        let saver = Saver::new(dir.path().to_owned(), loaded.writable);
        let text = full(dir.path()).to_text().expect("text");
        assert!(matches!(
            saver.save_within(text, Duration::from_secs(10)),
            Some(Ok(()))
        ));
        assert_eq!(
            std::fs::read_to_string(&file).expect("still there"),
            "theme = [[["
        );
    }

    #[test]
    fn a_nan_of_a_newer_version_reads_the_same_each_time() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join(FILE_NAME), "theme = \"dark\"\nzoom = nan\n")
            .expect("written");
        let settings = load(dir.path()).settings;
        assert_ne!(settings, settings.clone(), "a nan is not equal to itself");
        let text = settings.to_text().expect("text");
        assert_eq!(settings.clone().to_text().expect("text"), text);
        assert!(text.contains("zoom = nan"));
    }

    #[test]
    fn missing_folders_fall_back_to_home() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let home = folder(dir.path(), "home");
        let a = folder(dir.path(), "a");
        let b = folder(dir.path(), "b");
        let gone = folder(dir.path(), "gone");
        let slow = folder(dir.path(), "slow");
        let unplugged = folder(dir.path(), "usb");
        for made in [&home, &a, &b, &slow] {
            std::fs::create_dir(made.as_local().expect("local")).expect("made");
        }
        let mut stored = Settings::default();
        stored.set_session(&Session {
            panes: [
                SessionPane {
                    tabs: vec![a.clone(), gone.clone(), b.clone()],
                    active: 1,
                    pinned: Vec::new(),
                },
                SessionPane {
                    tabs: vec![slow.clone(), gone.clone(), unplugged.clone()],
                    active: 0,
                    pinned: Vec::new(),
                },
            ],
            active_pane: 1,
            show_hidden: false,
            layout: Layout::default(),
        });
        stored.save(dir.path()).expect("saved");
        let session = load(dir.path()).settings.session();
        assert_eq!(
            session.folders(),
            [a.clone(), gone, b.clone(), slow.clone(), unplugged.clone()]
        );

        // The slow folder is not looked at in time; the unplugged one's drive is not there.
        let (restored, waiting) = session.resolve(&home, |path| {
            if *path == slow {
                None
            } else if *path == unplugged {
                Some(Found::Offline)
            } else if path.as_local().is_some_and(Path::is_dir) {
                Some(Found::Folder)
            } else {
                Some(Found::Gone)
            }
        });

        assert_eq!(restored.panes[0].tabs, [a, home.clone(), b]);
        assert_eq!(restored.panes[0].active, 1);
        assert_eq!(restored.panes[1].tabs, [home.clone(), home.clone(), home]);
        assert_eq!(restored.active_pane, 1);
        assert_eq!(
            waiting,
            [
                Waiting {
                    side: 1,
                    tab: 0,
                    path: slow
                },
                Waiting {
                    side: 1,
                    tab: 2,
                    path: unplugged
                }
            ]
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
        let saver = Saver::new(dir.path().to_owned(), true);
        let (sent, received) = mpsc::channel();
        for theme in [ThemeChoice::Light, ThemeChoice::Dark, ThemeChoice::System] {
            let sent = sent.clone();
            let settings = Settings {
                theme,
                ..Settings::default()
            };
            saver
                .save_in_background(settings.to_text().expect("text"), move |result| {
                    let _ = sent.send(result.is_ok());
                })
                .expect("the thread starts");
        }
        let last = Settings {
            theme: ThemeChoice::Dark,
            show_hidden: true,
            ..Settings::default()
        };
        let text = last.to_text().expect("text");
        assert!(matches!(
            saver.save_within(text, Duration::from_secs(10)),
            Some(Ok(()))
        ));
        for _ in 0..3 {
            assert!(received.recv().expect("each write reports"));
        }
        assert_eq!(load(dir.path()), writable(last));
    }

    #[test]
    fn the_last_save_does_not_wait_for_a_hung_write() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let saver = Saver::new(dir.path().to_owned(), true);
        let text = full(dir.path()).to_text().expect("text");
        // A write that hangs (a network home that stopped answering) holds the saver.
        let hung = saver.written.lock().expect("not poisoned");

        let started = Instant::now();
        let result = saver.save_within(text, Duration::from_millis(200));

        assert!(result.is_none(), "it does not wait for the write");
        assert!(started.elapsed() < Duration::from_secs(5));
        // The write goes on once the hung one ends.
        drop(hung);
        let deadline = Instant::now() + Duration::from_secs(10);
        while load(dir.path()) != writable(full(dir.path())) {
            assert!(Instant::now() < deadline, "the write ends");
            std::thread::sleep(Duration::from_millis(20));
        }
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
