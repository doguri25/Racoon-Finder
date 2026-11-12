//! The settings kept between runs, in `settings.toml` in the app's config folder
//! (`AppPaths::config_dir`): the theme, whether hidden items show and whether the names show
//! their extensions (`hide_extensions`, NAV-11), the window's size and layout
//! (`[layout]`), the tabs of both panes (in their order, pinned or not) and which one has the
//! keyboard, the destination picker's recent targets, the views remembered per folder
//! (`[[folder_views]]`, NAV-10: at most `state::MAX_FOLDER_VIEWS`, the one used longest ago
//! first), and the choices of 보기 ▸ 설정: what a drop does without a modifier key
//! (`drag_default`, DND-02), whether a dragged move offers its undo (`move_undo_toast`, DND-07),
//! whether a right click shows Windows' menu first (`system_menu_first`, OPS-05) and whether the
//! icon views show thumbnails (`show_thumbnails`, VIEW-09), whether a long press opens the menu
//! (`long_press`, with its delay `long_press_ms`, PICK-02), whether the rows show checkboxes
//! (`checkbox_select`, NAV-15) and what a copy or a move does about an item that fails
//! (`on_error`, XFER-09); and the sidebar's favorites the user
//! keeps (`[[favorites]]`, NAV-05: none until the user first edits the list, then the whole list),
//! the last ten folders visited (`recent_folders`) and whether their group is folded
//! (`recent_folded`).
//!
//! The file carries its version (`version`, [`SETTINGS_VERSION`], INST-08). One without the key
//! is version 0: [`migrate`] brings it to the current version when it is read, and the next save
//! writes the key, so the file is upgraded in place by the save after the run that read it. A
//! file of a newer version that this version can read all the same (every known key falls back
//! to its default, the keys it does not know stay in `extra`) is kept as it is, its version
//! with it, and is not backed up.
//!
//! [`load`] reads them once at the start, before the window shows; nothing here fails the start.
//! A file that cannot be used (it does not parse, is not text, or is far too large; a newer
//! version's included, when even that tolerant read fails) is moved, never copied over, to
//! `settings.toml.bak-<YYYYMMDD-HHMMSS>` in the local time of the move (a name taken gets `-2`,
//! `-3`, … after it), and the app starts with the defaults and tells where the file went
//! ([`Loaded::backup`]). A file that is there but cannot be read, or cannot be moved, is left
//! alone: the app starts with the defaults and does not save over it ([`Loaded::writable`]), so
//! a save never replaces a file that was not backed up first.
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
//! with the default layout and no tab pinned, one without `[[folder_views]]` with no view
//! remembered, one without `hide_extensions` with the extensions shown, and one without the
//! choices of 보기 ▸ 설정 with a drop that moves, the undo offered, the app's menu first, the
//! thumbnails shown and a transfer that carries on past an item that fails. A
//! folder view falls back one key at a time too: a mode or a key of the order this
//! version does not know shows as the default one, and a column it does not know is left out of
//! the view; all of them are kept as written (`UnknownParts`) and written back, until the user
//! changes that part here, and the keys of a newer version stay in its `extra`.
//!
//! Folders are stored as `VPath::to_uri`, which reads back losslessly with `VPath::parse`.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, TryLockError, mpsc};
use std::time::{Duration, Instant};

use rf_core::{Os, VPath};
use rf_vfs::{SortBy, SortOrder, SortSpec};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::state::{
    Column, FolderView, Layout, MAX_COLUMN_WIDTH, MIN_COLUMN_WIDTH, ShownColumn, StoredView,
    UnknownParts, ViewMode, ViewStore,
};

/// The name of the settings file in the config folder.
pub const FILE_NAME: &str = "settings.toml";
/// The start of the name a settings file that cannot be used is moved to; the time of the move
/// follows ([`backup_name`]).
pub const BACKUP_PREFIX: &str = "settings.toml.bak-";
/// The version of the settings file this build writes (INST-08). Version 0 is a file without the
/// key; 1 added the key.
pub const SETTINGS_VERSION: u32 = 1;
/// The file a save writes before it takes the place of the settings file.
pub const TEMP_FILE_NAME: &str = "settings.toml.tmp";
/// A settings file larger than this is no file this app wrote; it is treated as broken. The
/// views of a thousand folders of the longest paths kept (`state::MAX_FOLDER_URI`) stay below
/// it.
const MAX_FILE_SIZE: u64 = 4 * 1024 * 1024;

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

/// What a drop does when no modifier key says (보기 ▸ 설정 ▸ 끌어 놓기: …, DND-02).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DragDefault {
    /// Move the items.
    #[default]
    Move,
    /// Copy them.
    Copy,
    /// As the system's file manager does: move them within a drive, copy them to another.
    System,
    /// Ask with a menu where they are dropped (놓을 때 묻기).
    Ask,
}

/// What a copy or a move does when one of its items fails (보기 ▸ 설정 ▸ 복사·이동 오류는 파일마다
/// 묻기, XFER-09): `on_error = "continue"` or `"ask"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnError {
    /// Carry on with the other items, and list the failed ones at the end, where they can be
    /// tried again.
    #[default]
    Continue,
    /// Stop at the item and ask: try it again, leave it out, or stop the job.
    Ask,
}

impl OnError {
    /// The error policy a transfer runs under.
    pub fn policy(self) -> rf_jobs::ErrorPolicy {
        match self {
            OnError::Continue => rf_jobs::ErrorPolicy::Continue,
            OnError::Ask => rf_jobs::ErrorPolicy::Ask,
        }
    }
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
    /// The folders of the pinned tabs, in the order of the tab bar, as `VPath::to_uri`: each
    /// pins the first tab of `tabs` at that folder that it does not pin yet. Pins are kept by
    /// folder, not by position: a version that knows nothing of them (the first demo) keeps
    /// this key as it is while it writes `tabs` anew, which must not move a pin to another tab.
    /// Not written when no tab is pinned.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "or_default")]
    pub pinned: Vec<String>,
    /// The keys of a newer version.
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// `flags`, one for each tab, without the ones after the last `true`: none at all when no tab
/// is pinned, as [`SessionPane::pinned`] keeps them.
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each key is a separate choice of the user's, kept as the file has it"
)]
pub struct Settings {
    /// The version of the file these settings were read from, brought up to
    /// [`SETTINGS_VERSION`] by [`migrate`] when it was older (0 for a file without the key
    /// before that), and as it was when it is newer. Saved as the greater of this and
    /// [`SETTINGS_VERSION`].
    #[serde(default = "no_version", deserialize_with = "version_or_zero")]
    pub version: u32,
    #[serde(deserialize_with = "or_default")]
    pub theme: ThemeChoice,
    #[serde(deserialize_with = "or_default")]
    pub show_hidden: bool,
    /// The names of files show no extensions (NAV-11); a file without the key shows them.
    #[serde(skip_serializing_if = "is_false", deserialize_with = "or_default")]
    pub hide_extensions: bool,
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
    /// What a drop does when no modifier key says (DND-02).
    #[serde(deserialize_with = "or_default")]
    pub drag_default: DragDefault,
    /// A move made by a drag offers its undo in a toast once it is done (DND-07); on unless the
    /// file says `false`.
    #[serde(deserialize_with = "or_true")]
    pub move_undo_toast: bool,
    /// A right click shows Windows' own menu of the items rather than the app's (OPS-05); only
    /// Windows has one.
    #[serde(deserialize_with = "or_default")]
    pub system_menu_first: bool,
    /// The icon views show the files' thumbnails (VIEW-09); on unless the file says `false`.
    #[serde(deserialize_with = "or_true")]
    pub show_thumbnails: bool,
    /// A left press held on a row (or on the empty part of the list) opens its menu (PICK-02);
    /// on unless the file says `false`.
    #[serde(deserialize_with = "or_true")]
    pub long_press: bool,
    /// How long that press is held, in milliseconds: [`LONG_PRESS_MIN_MS`] to
    /// [`LONG_PRESS_MAX_MS`], [`LONG_PRESS_DEFAULT_MS`] where the file has no whole number. A
    /// number outside the range is brought into it.
    #[serde(deserialize_with = "long_press_ms_or_default")]
    pub long_press_ms: u16,
    /// The rows of 자세히, 목록 and 아이콘 start with a checkbox that selects them (NAV-15).
    #[serde(deserialize_with = "or_default")]
    pub checkbox_select: bool,
    /// What a copy or a move does when one of its items fails (XFER-09): it carries on unless
    /// the file says `"ask"`. A file without the key (0.4.0 and before), or with a value this
    /// version cannot read, carries on.
    #[serde(deserialize_with = "or_default")]
    pub on_error: OnError,
    /// The favorites the user keeps under "즐겨찾기" in the sidebar, in order (NAV-05). None until
    /// the user first edits the list (add, remove, rename, reorder): the sidebar then shows the
    /// system's well-known folders, and the first edit makes that list the user's own and keeps
    /// it here. Read as a list: an entry that is no table, or has no folder, is left out. Not
    /// written when there is none.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "favorites_or_default"
    )]
    pub favorites: Vec<Favorite>,
    /// The user emptied the list of favorites: it stays empty rather than showing the system's
    /// folders again.
    #[serde(skip_serializing_if = "is_false", deserialize_with = "or_default")]
    pub favorites_emptied: bool,
    /// The folders visited last, the latest first, at most ten and none twice, as
    /// `VPath::to_uri`: the sidebar's group "최근 폴더".
    #[serde(deserialize_with = "or_default")]
    pub recent_folders: Vec<String>,
    /// The group "최근 폴더" is folded; a file without the key shows it open.
    #[serde(skip_serializing_if = "is_false", deserialize_with = "or_default")]
    pub recent_folded: bool,
    /// How the window lays out its panes and side panels.
    #[serde(deserialize_with = "or_default")]
    pub layout: LayoutSettings,
    /// The left pane, then the right one. Read as a list: a missing pane is an empty one, and
    /// a pane past the second (a newer version's) is kept as it is.
    #[serde(deserialize_with = "panes_or_default")]
    pub panes: Vec<PaneSettings>,
    /// The views remembered per folder, the one used longest ago first ([`Settings::views`]).
    /// Read as a list: an entry that is no table is left out. Not written when there is none.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "folder_views_or_default"
    )]
    pub folder_views: Vec<FolderViewSettings>,
    /// The keys of a newer version.
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            version: SETTINGS_VERSION,
            theme: ThemeChoice::default(),
            show_hidden: false,
            hide_extensions: false,
            window: None,
            active_pane: 0,
            recent_targets: Vec::new(),
            drag_default: DragDefault::default(),
            move_undo_toast: true,
            system_menu_first: false,
            show_thumbnails: true,
            long_press: true,
            long_press_ms: LONG_PRESS_DEFAULT_MS,
            checkbox_select: false,
            on_error: OnError::default(),
            favorites: Vec::new(),
            favorites_emptied: false,
            recent_folders: Vec::new(),
            recent_folded: false,
            layout: LayoutSettings::default(),
            panes: Vec::new(),
            folder_views: Vec::new(),
            extra: toml::Table::new(),
        }
    }
}

/// One favorite the user keeps in the sidebar: the name its row shows and its folder. A key the
/// entry lacks, or whose value this version cannot read, is empty; an entry without a folder is
/// left out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Favorite {
    /// The name the row shows.
    #[serde(deserialize_with = "or_default")]
    pub name: String,
    /// The folder, as `VPath::to_uri`.
    #[serde(deserialize_with = "or_default")]
    pub path: String,
}

impl Favorite {
    /// The favorite that shows `name` and opens `folder`.
    pub fn new(name: impl Into<String>, folder: &VPath) -> Favorite {
        Favorite {
            name: name.into(),
            path: folder.to_uri(),
        }
    }

    /// The folder it opens; `None` when the file's text names none this version can read.
    pub fn folder(&self) -> Option<VPath> {
        VPath::parse(&self.path).ok()
    }
}

/// One folder's view, as stored (an entry of `[[folder_views]]`). Each key the entry lacks, or
/// whose value this version cannot read, takes its default, and the defaults are not written:
/// 자세히 with the default columns, by name ascending, folders first, for the folder alone.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each key is a separate choice of the view, kept as the file has it"
)]
pub struct FolderViewSettings {
    /// The folder, as `VPath::to_uri`.
    #[serde(deserialize_with = "or_default")]
    pub folder: String,
    /// The view mode: `list`, `small-icons`, `medium-icons`, `large-icons`, or 자세히
    /// (`details`, or none).
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "or_default"
    )]
    pub mode: String,
    /// The columns of 자세히 after the name, in their order (`size`, `modified`, `created`,
    /// `kind`, `extension`, `attributes`); none for the default ones.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "or_default"
    )]
    pub columns: Option<Vec<String>>,
    /// The widths of `columns`, in logical pixels, in the same order; a column without one (or
    /// with 0) is as wide as by default.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "or_default")]
    pub widths: Vec<u32>,
    /// The column whose key sorts first (as `columns` names them, or `name`); none for the
    /// name.
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "or_default"
    )]
    pub sort: String,
    #[serde(skip_serializing_if = "is_false", deserialize_with = "or_default")]
    pub descending: bool,
    /// The column whose key breaks the ties of the first; none for no second key.
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "or_default"
    )]
    pub then: String,
    #[serde(skip_serializing_if = "is_false", deserialize_with = "or_default")]
    pub then_descending: bool,
    /// The folders mixed in with the files, rather than first.
    #[serde(skip_serializing_if = "is_false", deserialize_with = "or_default")]
    pub mixed: bool,
    /// 하위 폴더에 적용: the folders below show this view too, unless they have one of their own.
    #[serde(skip_serializing_if = "is_false", deserialize_with = "or_default")]
    pub subfolders: bool,
    /// The keys of a newer version.
    #[serde(flatten)]
    pub extra: toml::Table,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's `skip_serializing_if` hands the field by reference"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

impl FolderViewSettings {
    /// What the store keeps of this entry. What this version does not know (a mode, a sort
    /// key, a column of a newer version) shows as its default, and is kept as written
    /// ([`UnknownParts`]).
    pub fn stored(&self) -> StoredView {
        let defaults = FolderView::default();
        let mut unknown = UnknownParts::default();
        let columns = match &self.columns {
            None => defaults.columns,
            Some(names) => {
                let mut columns: Vec<ShownColumn> = Vec::new();
                for (i, name) in names.iter().enumerate() {
                    let width = self.widths.get(i).copied().unwrap_or(0);
                    let Some(column) = Column::from_name(name) else {
                        if !unknown.columns.iter().any(|(known, _)| known == name) {
                            unknown.columns.push((name.clone(), width));
                        }
                        continue;
                    };
                    if column == Column::Name || columns.iter().any(|shown| shown.column == column)
                    {
                        continue;
                    }
                    let width = if width > 0 {
                        width.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
                    } else {
                        column.default_width()
                    };
                    columns.push(ShownColumn { column, width });
                }
                columns
            }
        };
        let key = |name: &str| Column::from_name(name).and_then(Column::sort_key);
        let order = |descending: bool| {
            if descending {
                SortOrder::Descending
            } else {
                SortOrder::Ascending
            }
        };
        let primary = SortBy {
            key: key(&self.sort).unwrap_or(defaults.sort.primary.key),
            order: order(self.descending),
        };
        let secondary = key(&self.then)
            .filter(|then| *then != primary.key)
            .map(|then| SortBy {
                key: then,
                order: order(self.then_descending),
            });
        let mode = ViewMode::from_name(&self.mode);
        if mode.is_none() && !self.mode.is_empty() {
            unknown.mode = Some(self.mode.clone());
        }
        if key(&self.sort).is_none() && !self.sort.is_empty() {
            unknown.sort = Some(self.sort.clone());
        }
        if key(&self.then).is_none() && !self.then.is_empty() {
            unknown.then = Some((self.then.clone(), self.then_descending));
        }
        StoredView {
            folder: self.folder.clone(),
            view: FolderView {
                mode: mode.unwrap_or_default(),
                columns,
                sort: SortSpec {
                    primary,
                    secondary,
                    folders_first: !self.mixed,
                },
            },
            subfolders: self.subfolders,
            unknown,
            extra: self.extra.clone(),
        }
    }

    /// The entry that keeps `stored`, without the defaults, and with what this version did not
    /// know as it was written (the unknown columns after the others).
    pub fn of(stored: &StoredView) -> FolderViewSettings {
        let (view, unknown) = (&stored.view, &stored.unknown);
        let defaults = FolderView::default();
        let mode = match &unknown.mode {
            Some(mode) => mode.clone(),
            None if view.mode == ViewMode::Details => String::new(),
            None => view.mode.name().to_owned(),
        };
        let columns =
            (view.columns != defaults.columns || !unknown.columns.is_empty()).then(|| {
                view.columns
                    .iter()
                    .map(|shown| shown.column.name().to_owned())
                    .chain(unknown.columns.iter().map(|(name, _)| name.clone()))
                    .collect()
            });
        let widths = if columns.is_some() {
            view.columns
                .iter()
                .map(|shown| shown.width)
                .chain(unknown.columns.iter().map(|&(_, width)| width))
                .collect()
        } else {
            Vec::new()
        };
        let name = |by: SortBy| Column::of_key(by.key).name().to_owned();
        let sort = view.sort;
        let (then, then_descending) = match &unknown.then {
            Some((then, descending)) => (then.clone(), *descending),
            None => (
                sort.secondary.map(name).unwrap_or_default(),
                sort.secondary
                    .is_some_and(|by| by.order == SortOrder::Descending),
            ),
        };
        FolderViewSettings {
            folder: stored.folder.clone(),
            mode,
            columns,
            widths,
            sort: match &unknown.sort {
                Some(sort) => sort.clone(),
                None if sort.primary.key == defaults.sort.primary.key => String::new(),
                None => name(sort.primary),
            },
            descending: sort.primary.order == SortOrder::Descending,
            then,
            then_descending,
            mixed: !sort.folders_first,
            subfolders: stored.subfolders,
            extra: stored.extra.clone(),
        }
    }
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

/// A known flag that is on by default: `false` only when the file says so.
fn or_true<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = toml::Value::deserialize(deserializer)?;
    Ok(value.as_bool().unwrap_or(true))
}

/// The version of a file that says none.
fn no_version() -> u32 {
    0
}

/// The file's version: a whole number from 0 up, or 0 for anything else.
fn version_or_zero<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    let value = toml::Value::deserialize(deserializer)?;
    Ok(version_of_value(Some(&value)))
}

fn version_of_value(value: Option<&toml::Value>) -> u32 {
    value
        .and_then(toml::Value::as_integer)
        .and_then(|number| u32::try_from(number).ok())
        .unwrap_or(0)
}

/// Why [`migrate`] could not bring a file to the current version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MigrateError {
    /// The file is of a newer version than this build writes; it is read as it is, not
    /// migrated.
    #[error("settings of version {found} are newer than version {SETTINGS_VERSION}")]
    Newer { found: u32 },
}

/// Brings the settings file `doc`, of version `from`, up to [`SETTINGS_VERSION`], one version at
/// a time. Pure: it reads and writes no file.
///
/// - 0 → 1: the file gets `version = 1`. Nothing else changes.
///
/// A file already at the current version comes back as it is; a newer one is
/// [`MigrateError::Newer`].
pub fn migrate(mut doc: toml::Table, from: u32) -> Result<toml::Table, MigrateError> {
    if from > SETTINGS_VERSION {
        return Err(MigrateError::Newer { found: from });
    }
    let mut at = from;
    while at < SETTINGS_VERSION {
        if at == 0 {
            doc.insert("version".to_owned(), toml::Value::Integer(1));
        }
        at += 1;
    }
    Ok(doc)
}

/// How long a long press is held by default, in milliseconds.
pub const LONG_PRESS_DEFAULT_MS: u16 = 500;
/// The shortest and the longest time a long press can be set to, in milliseconds.
pub const LONG_PRESS_MIN_MS: u16 = 300;
pub const LONG_PRESS_MAX_MS: u16 = 1500;

/// The time of a long press: a whole number brought into [`LONG_PRESS_MIN_MS`] to
/// [`LONG_PRESS_MAX_MS`], or the default when the value is anything else.
fn long_press_ms_or_default<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: Deserializer<'de>,
{
    let value = toml::Value::deserialize(deserializer)?;
    Ok(value.as_integer().map_or(LONG_PRESS_DEFAULT_MS, |ms| {
        u16::try_from(ms.clamp(i64::from(LONG_PRESS_MIN_MS), i64::from(LONG_PRESS_MAX_MS)))
            .unwrap_or(LONG_PRESS_DEFAULT_MS)
    }))
}

/// The folder views as a list, without the entries that are no table; no list at all is none.
fn folder_views_or_default<'de, D>(deserializer: D) -> Result<Vec<FolderViewSettings>, D::Error>
where
    D: Deserializer<'de>,
{
    let toml::Value::Array(views) = toml::Value::deserialize(deserializer)? else {
        return Ok(Vec::new());
    };
    Ok(views
        .into_iter()
        .filter(toml::Value::is_table)
        .filter_map(|view| view.try_into().ok())
        .collect())
}

/// The favorites as a list, without the entries that are no table or name no folder; no list at
/// all is none.
fn favorites_or_default<'de, D>(deserializer: D) -> Result<Vec<Favorite>, D::Error>
where
    D: Deserializer<'de>,
{
    let toml::Value::Array(favorites) = toml::Value::deserialize(deserializer)? else {
        return Ok(Vec::new());
    };
    Ok(favorites
        .into_iter()
        .filter(toml::Value::is_table)
        .filter_map(|favorite| favorite.try_into::<Favorite>().ok())
        .filter(|favorite| !favorite.path.is_empty())
        .collect())
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
    /// The names of files show no extensions.
    pub hide_extensions: bool,
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
    /// could not be read (an I/O error) or could not be moved to its backup: a save would
    /// replace it unread, so the run keeps what it changes to itself.
    pub writable: bool,
    /// Where the settings file that could not be used was moved to ([`BACKUP_PREFIX`]): the
    /// window tells of it once it is open ([`crate::words::settings_backed_up`]).
    pub backup: Option<PathBuf>,
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
            // The folders of the pins not given to a tab yet.
            let mut pins: Vec<VPath> = stored
                .pinned
                .iter()
                .filter_map(|uri| VPath::parse(uri).ok())
                .collect();
            let mut pinned = Vec::new();
            for (i, uri) in stored.tabs.iter().enumerate() {
                if let Ok(path) = VPath::parse(uri) {
                    if i == stored.active {
                        pane.active = pane.tabs.len();
                    }
                    let pin = pins.iter().position(|pin| *pin == path);
                    pinned.push(pin.map(|at| pins.remove(at)).is_some());
                    pane.tabs.push(path);
                }
            }
            pane.pinned = pinned_flags(pinned);
            pane
        };
        Session {
            panes: [pane(0), pane(1)],
            active_pane: self.active_pane.min(1),
            show_hidden: self.show_hidden,
            hide_extensions: self.hide_extensions,
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
            let unread: Vec<String> = stored
                .tabs
                .iter()
                .filter(|uri| VPath::parse(uri).is_err())
                .cloned()
                .collect();
            // A pin of a tab this version cannot read does not read back as a path either.
            let unread_pins = stored
                .pinned
                .iter()
                .filter(|uri| VPath::parse(uri).is_err())
                .cloned();
            let pins = pane
                .tabs
                .iter()
                .zip(&pane.pinned)
                .filter(|&(_, &pinned)| pinned)
                .map(|(path, _)| path.to_uri());
            stored.pinned = pins.chain(unread_pins).collect();
            stored.tabs = pane.tabs.iter().map(VPath::to_uri).chain(unread).collect();
            stored.active = pane.active;
        }
        self.active_pane = session.active_pane;
        self.show_hidden = session.show_hidden;
        self.hide_extensions = session.hide_extensions;
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

    /// The folders visited last that read back as paths, the latest first.
    pub fn recent_visits(&self) -> Vec<VPath> {
        self.recent_folders
            .iter()
            .filter_map(|uri| VPath::parse(uri).ok())
            .collect()
    }

    pub fn set_recent_visits(&mut self, folders: &[VPath]) {
        self.recent_folders = folders.iter().map(VPath::to_uri).collect();
    }

    /// The views remembered per folder, with this system's rule of case. A file with more than
    /// the store keeps keeps the ones used last.
    pub fn views(&self) -> ViewStore {
        ViewStore::new(
            self.folder_views
                .iter()
                .map(FolderViewSettings::stored)
                .collect(),
            Os::CURRENT,
        )
    }

    /// Takes the views of `store` in, the one used longest ago first, with the keys of a newer
    /// version each kept.
    pub fn set_views(&mut self, store: &ViewStore) {
        self.folder_views = store.entries().map(FolderViewSettings::of).collect();
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
    ///
    /// The window asks for it after every change, to see whether there is anything to save, and
    /// most changes leave the folder views (up to a thousand of them) as they were: their text
    /// is made once for each set of them, and put after the rest.
    pub fn to_text(&self) -> io::Result<String> {
        let head = Settings {
            version: self.version.max(SETTINGS_VERSION),
            theme: self.theme,
            show_hidden: self.show_hidden,
            hide_extensions: self.hide_extensions,
            window: self.window,
            active_pane: self.active_pane,
            recent_targets: self.recent_targets.clone(),
            drag_default: self.drag_default,
            move_undo_toast: self.move_undo_toast,
            system_menu_first: self.system_menu_first,
            show_thumbnails: self.show_thumbnails,
            long_press: self.long_press,
            long_press_ms: self.long_press_ms,
            checkbox_select: self.checkbox_select,
            on_error: self.on_error,
            favorites: self.favorites.clone(),
            favorites_emptied: self.favorites_emptied,
            recent_folders: self.recent_folders.clone(),
            recent_folded: self.recent_folded,
            layout: self.layout.clone(),
            panes: self.panes.clone(),
            folder_views: Vec::new(),
            extra: self.extra.clone(),
        };
        let mut text = toml::to_string(&head).map_err(io::Error::other)?;
        if self.folder_views.is_empty() {
            return Ok(text);
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&folder_views_text(&self.folder_views)?);
        Ok(text)
    }

    /// Writes the settings to `config_dir` at once ([`write_text`]). Blocks on the disk.
    pub fn save(&self, config_dir: &Path) -> io::Result<()> {
        write_text(config_dir, &self.to_text()?)
    }
}

/// The text of `views` as `[[folder_views]]` tables ([`Settings::to_text`]): made once for each
/// set of views on this thread, and kept until another set comes.
fn folder_views_text(views: &[FolderViewSettings]) -> io::Result<String> {
    thread_local! {
        static LAST: RefCell<Option<(Vec<FolderViewSettings>, String)>> = const { RefCell::new(None) };
    }
    #[derive(Serialize)]
    struct Views<'a> {
        folder_views: &'a [FolderViewSettings],
    }
    LAST.with_borrow_mut(|last| {
        if let Some((known, text)) = last.as_ref()
            && known.as_slice() == views
        {
            return Ok(text.clone());
        }
        let text = toml::to_string(&Views {
            folder_views: views,
        })
        .map_err(io::Error::other)?;
        *last = Some((views.to_vec(), text.clone()));
        Ok(text)
    })
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
/// defaults. A file of an older version is migrated ([`migrate`]) in memory; the file itself
/// is upgraded by the next save. A file of a newer version is read as well as it can be and
/// kept as it is (the keys it does not know in `extra`). One that cannot be used (it does not
/// parse, is not text, is too large, or a newer version's that even the tolerant read cannot
/// use) is moved to `settings.toml.bak-<YYYYMMDD-HHMMSS>` ([`BACKUP_PREFIX`], local time, a
/// name taken gets `-2`, `-3`, …) and the settings are the defaults, with [`Loaded::backup`]
/// telling where it went. One that cannot be read (an I/O error), or that cannot be moved,
/// stays where it is, and the settings are the defaults but not [`Loaded::writable`]: a save
/// would replace it unread.
///
/// It reads a small local file and is called once at the start, before the window shows: the
/// UI thread may call it then.
pub fn load(config_dir: &Path) -> Loaded {
    load_with(config_dir, &local_now, &rename_retrying)
}

/// The local time, as the name of a backup shows it.
fn local_now() -> jiff::civil::DateTime {
    jiff::Zoned::now().datetime()
}

/// [`load`] with the clock and the move of a file that cannot be used given: the tests set the
/// time and make the move fail.
fn load_with(
    config_dir: &Path,
    now: &dyn Fn() -> jiff::civil::DateTime,
    rename: &dyn Fn(&Path, &Path) -> io::Result<()>,
) -> Loaded {
    let path = config_dir.join(FILE_NAME);
    let unusable = match read_small(&path) {
        Ok(text) => match parse(&text) {
            Some(settings) => {
                return Loaded {
                    settings,
                    writable: true,
                    backup: None,
                };
            }
            None => true,
        },
        Err(error) => match error.kind() {
            io::ErrorKind::NotFound => {
                return Loaded {
                    settings: Settings::default(),
                    writable: true,
                    backup: None,
                };
            }
            // Not text, or far too large: no settings file this app wrote.
            io::ErrorKind::InvalidData => true,
            _ => false,
        },
    };
    let backup = unusable
        .then(|| {
            let to = backup_path(config_dir, &backup_name(now()), BACKUP_TRIES)?;
            rename(&path, &to).is_ok().then_some(to)
        })
        .flatten();
    Loaded {
        settings: Settings::default(),
        // Only a file that is out of the way may be written over.
        writable: backup.is_some(),
        backup,
    }
}

/// The settings `text` says: its version found, migrated if older, and read leniently. `None`
/// for text that does not parse, or that no version of the reader can use.
fn parse(text: &str) -> Option<Settings> {
    let doc: toml::Table = toml::from_str(text).ok()?;
    let from = version_of_value(doc.get("version"));
    let doc = if from < SETTINGS_VERSION {
        migrate(doc, from).ok()?
    } else {
        doc
    };
    doc.try_into().ok()
}

/// The name a settings file is moved to at `at`: `settings.toml.bak-20261106-143207`.
pub fn backup_name(at: jiff::civil::DateTime) -> String {
    format!("{BACKUP_PREFIX}{}", at.strftime("%Y%m%d-%H%M%S"))
}

/// The first of `name`, `name-2`, `name-3`, … (at most `tries` names) in `config_dir` that
/// nothing is at: a backup is never put over another file. `None` when every name is taken, so
/// the file cannot be backed up.
fn backup_path(config_dir: &Path, name: &str, tries: u32) -> Option<PathBuf> {
    (1..=tries)
        .map(|number| match number {
            1 => config_dir.join(name),
            _ => config_dir.join(format!("{name}-{number}")),
        })
        .find(|path| std::fs::symlink_metadata(path).is_err())
}

/// How many names [`backup_path`] tries before it gives up.
const BACKUP_TRIES: u32 = 1000;

/// The theme of `settings.toml` in `config_dir`, for a window shown while another instance
/// runs (it says so): the file is only read. Without a file, or with one [`load`] would not
/// take, it is the default theme. A small local file; the UI thread may call it.
pub fn peek_theme(config_dir: &Path) -> ThemeChoice {
    read_small(&config_dir.join(FILE_NAME))
        .ok()
        .and_then(|text| parse(&text))
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

    /// Writes nothing from now on, however many saves are asked for: waits up to `wait` for a
    /// write under way to end (it has, when this returns `true`), and the saves after it are
    /// skipped. For the removal of the app, which must not find its folder written again. Blocks.
    pub fn stop_within(&self, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        loop {
            match self.written.try_lock() {
                Ok(mut written) => {
                    *written = u64::MAX;
                    return true;
                }
                Err(TryLockError::Poisoned(poisoned)) => {
                    *poisoned.into_inner() = u64::MAX;
                    return true;
                }
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(TryLockError::WouldBlock) => return false,
            }
        }
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
    use crate::state::{Column, FolderView, Layout, ViewMode, ViewStore};

    fn folder(dir: &Path, name: &str) -> VPath {
        VPath::local(dir.join(name)).expect("a local path")
    }

    /// Settings with something in every field, keys of a newer version among them.
    fn full(dir: &Path) -> Settings {
        let mut settings = Settings {
            theme: ThemeChoice::Dark,
            show_hidden: true,
            window: Some((1500, 900)),
            drag_default: DragDefault::Ask,
            move_undo_toast: false,
            system_menu_first: true,
            long_press: false,
            long_press_ms: 800,
            checkbox_select: true,
            on_error: OnError::Ask,
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
            hide_extensions: true,
            layout: odd_layout(),
        });
        settings.set_recent(&[folder(dir, "r1"), folder(dir, "r2")]);
        settings.favorites = vec![
            Favorite::new("작업", &folder(dir, "work")),
            Favorite::new("사진 \"원본\"", &folder(dir, "설계 문서")),
        ];
        settings.set_recent_visits(&[folder(dir, "v1"), folder(dir, "v2")]);
        settings.recent_folded = true;
        settings.favorites_emptied = true;
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
        settings.set_views(&odd_views(dir));
        settings
    }

    /// Views of three folders, with every value of a view off its default in one of them.
    fn odd_views(dir: &Path) -> ViewStore {
        let mut store = ViewStore::default();
        let mut icons = FolderView {
            mode: ViewMode::LargeIcons,
            ..FolderView::default()
        };
        store.remember(&folder(dir, "a"), icons.clone());
        icons.mode = ViewMode::List;
        store.apply_to_subfolders(&folder(dir, "사진"), icons);
        let mut odd = FolderView::default();
        odd.toggle_column(Column::Kind);
        odd.toggle_column(Column::Attributes);
        odd.toggle_column(Column::Created);
        odd.move_column(2, 0);
        odd.set_width(Column::Size, 88);
        odd.sort_by(rf_vfs::SortKey::Modified, false);
        odd.sort_by(rf_vfs::SortKey::Modified, false);
        odd.sort_by(rf_vfs::SortKey::Extension, true);
        odd.sort.folders_first = false;
        store.remember(&folder(dir, "c"), odd);
        store
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
            backup: None,
        }
    }

    /// The names in `dir` that start like a backup, sorted.
    fn backups(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("the folder")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .into_string()
                    .expect("text")
            })
            .filter(|name| name.starts_with(BACKUP_PREFIX))
            .collect();
        names.sort();
        names
    }

    /// 2026-11-06 14:32:07, the time every test of the backups runs at.
    fn at() -> jiff::civil::DateTime {
        jiff::civil::date(2026, 11, 6).at(14, 32, 7, 0)
    }

    const STAMPED: &str = "settings.toml.bak-20261106-143207";

    fn load_at(dir: &Path) -> Loaded {
        load_with(dir, &at, &rename_retrying)
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
        assert!(loaded.settings.session().hide_extensions);
        let text = std::fs::read_to_string(dir.path().join(FILE_NAME)).expect("read");
        assert!(text.contains("hide_extensions = true"), "{text}");
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
        assert_eq!(backups(dir.path()), Vec::<String>::new());
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
        assert_eq!(backups(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn the_extensions_are_written_only_when_hidden() {
        let shown = Settings::default().to_text().expect("text");
        assert!(!shown.contains("hide_extensions"), "{shown}");
        let hidden = Settings {
            hide_extensions: true,
            ..Settings::default()
        };
        let text = hidden.to_text().expect("text");
        assert!(text.contains("hide_extensions = true"), "{text}");
        // A value this version cannot read shows them.
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join(FILE_NAME), "hide_extensions = \"yes\"\n").expect("written");
        assert!(!load(dir.path()).settings.hide_extensions);
    }

    #[test]
    fn the_drag_settings_round_trip_and_take_their_defaults_when_missing() {
        // A drop moves, a dragged move offers its undo, a right click shows the app's menu.
        let defaults = Settings::default();
        assert_eq!(defaults.drag_default, DragDefault::Move);
        assert!(defaults.move_undo_toast);
        assert!(!defaults.system_menu_first);

        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        let drag = |settings: &Settings| {
            (
                settings.drag_default,
                settings.move_undo_toast,
                settings.system_menu_first,
            )
        };
        for (default, name) in [
            (DragDefault::Move, "move"),
            (DragDefault::Copy, "copy"),
            (DragDefault::System, "system"),
            (DragDefault::Ask, "ask"),
        ] {
            let settings = Settings {
                drag_default: default,
                move_undo_toast: false,
                system_menu_first: true,
                ..Settings::default()
            };
            settings.save(dir.path()).expect("saved");
            let text = std::fs::read_to_string(&file).expect("read");
            assert!(
                text.contains(&format!("drag_default = \"{name}\"")),
                "{text}"
            );
            assert_eq!(load(dir.path()), writable(settings));
        }
        // A file of the demos before, without the keys, takes the defaults.
        std::fs::write(&file, "theme = \"dark\"\n").expect("written");
        assert_eq!(
            drag(&load(dir.path()).settings),
            (DragDefault::Move, true, false)
        );
        // So do values this version cannot read: one odd value does not cost the others.
        std::fs::write(
            &file,
            "drag_default = \"fling\"\nmove_undo_toast = \"yes\"\nsystem_menu_first = 1\n\
             theme = \"dark\"\n",
        )
        .expect("written");
        let loaded = load(dir.path()).settings;
        assert_eq!(drag(&loaded), (DragDefault::Move, true, false));
        assert_eq!(loaded.theme, ThemeChoice::Dark);
    }

    #[test]
    fn the_error_choice_round_trips_and_an_older_file_carries_on() {
        // The default is what the demos before did: carry on, and list the failures at the end.
        let defaults = Settings::default();
        assert_eq!(defaults.on_error, OnError::Continue);
        assert_eq!(OnError::Continue.policy(), rf_jobs::ErrorPolicy::Continue);
        assert_eq!(OnError::Ask.policy(), rf_jobs::ErrorPolicy::Ask);

        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        for (choice, name) in [(OnError::Continue, "continue"), (OnError::Ask, "ask")] {
            let settings = Settings {
                on_error: choice,
                ..Settings::default()
            };
            settings.save(dir.path()).expect("saved");
            let text = std::fs::read_to_string(&file).expect("read");
            assert!(text.contains(&format!("on_error = \"{name}\"")), "{text}");
            // The key adds no version: the file is still the version it was.
            assert!(
                text.contains(&format!("version = {SETTINGS_VERSION}")),
                "{text}"
            );
            assert_eq!(load(dir.path()), writable(settings));
        }
        assert_eq!(SETTINGS_VERSION, 1);
        // A 0.4.0 file, which has its version and no such key, loads as it is and carries on.
        std::fs::write(
            &file,
            "version = 1\ntheme = \"dark\"\ndrag_default = \"copy\"\nmove_undo_toast = false\n\
             \n[layout]\nhide_tree = true\n",
        )
        .expect("written");
        let loaded = load(dir.path());
        assert!(loaded.backup.is_none() && loaded.writable);
        assert_eq!(loaded.settings.on_error, OnError::Continue);
        assert_eq!(loaded.settings.version, 1);
        assert_eq!(loaded.settings.theme, ThemeChoice::Dark);
        assert_eq!(loaded.settings.drag_default, DragDefault::Copy);
        assert!(!loaded.settings.move_undo_toast);
        assert!(loaded.settings.layout.hide_tree);
        // A value this version cannot read carries on too, and costs no other key.
        std::fs::write(&file, "on_error = \"panic\"\ntheme = \"dark\"\n").expect("written");
        let loaded = load(dir.path()).settings;
        assert_eq!(loaded.on_error, OnError::Continue);
        assert_eq!(loaded.theme, ThemeChoice::Dark);
    }

    #[test]
    fn favorites_and_recent_folders_are_kept_and_older_files_lack_them() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        let defaults = Settings::default();
        assert!(defaults.favorites.is_empty() && defaults.recent_folders.is_empty());
        assert!(!defaults.recent_folded);
        // Nothing of them is written until the user has something to keep.
        let text = defaults.to_text().expect("text");
        assert!(
            !text.contains("favorites") && !text.contains("recent_folded"),
            "{text}"
        );

        let work = folder(dir.path(), "work");
        let settings = Settings {
            favorites: vec![
                Favorite::new("작업", &work),
                Favorite::new("사진 \"원본\"", &folder(dir.path(), "설계 문서")),
            ],
            recent_folders: vec![work.to_uri()],
            recent_folded: true,
            ..Settings::default()
        };
        settings.save(dir.path()).expect("saved");
        let text = std::fs::read_to_string(&file).expect("read");
        assert!(text.contains("[[favorites]]"), "{text}");
        assert!(text.contains("recent_folded = true"), "{text}");
        let loaded = load(dir.path());
        assert_eq!(loaded, writable(settings));
        assert_eq!(loaded.settings.favorites[0].folder(), Some(work.clone()));
        assert_eq!(loaded.settings.recent_visits(), [work]);

        // A file of the demos before has neither: the sidebar shows the known folders, no
        // recent folder, and the group open.
        std::fs::write(&file, "theme = \"dark\"\n").expect("written");
        let older = load(dir.path()).settings;
        assert!(older.favorites.is_empty() && older.recent_folders.is_empty());
        assert!(!older.recent_folded);
        // Values this version cannot read cost only themselves: an entry that is no table or
        // names no folder is left out, the rest stays.
        let ok = toml_string(&folder(dir.path(), "ok").to_uri());
        let text = format!(
            "recent_folders = 5\nrecent_folded = \"yes\"\n\
             favorites = [3, {{ name = \"비었음\" }}, {{ name = 7, path = {ok} }}, \
             {{ name = \"좋음\", path = {ok} }}]\n"
        );
        std::fs::write(&file, text).expect("written");
        let loaded = load(dir.path()).settings;
        assert!(loaded.recent_folders.is_empty() && !loaded.recent_folded);
        assert_eq!(
            loaded.favorites,
            [
                Favorite {
                    name: String::new(),
                    path: folder(dir.path(), "ok").to_uri()
                },
                Favorite::new("좋음", &folder(dir.path(), "ok")),
            ]
        );
        // A key of another kind altogether is none.
        std::fs::write(&file, "favorites = \"x\"\n").expect("written");
        assert_eq!(load(dir.path()).settings.favorites, []);
        // A list the user emptied is kept as emptied, not as none.
        let emptied = Settings {
            favorites_emptied: true,
            ..Settings::default()
        };
        emptied.save(dir.path()).expect("saved");
        assert!(load(dir.path()).settings.favorites_emptied);
        assert!(!Settings::default().favorites_emptied);
    }

    #[test]
    fn thumbnails_show_unless_the_file_says_otherwise() {
        assert!(Settings::default().show_thumbnails);
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        let settings = Settings {
            show_thumbnails: false,
            ..Settings::default()
        };
        settings.save(dir.path()).expect("saved");
        let text = std::fs::read_to_string(&file).expect("read");
        assert!(text.contains("show_thumbnails = false"), "{text}");
        assert_eq!(load(dir.path()), writable(settings));
        // The demos before had no such key: thumbnails show.
        std::fs::write(&file, "theme = \"dark\"\n").expect("written");
        assert!(load(dir.path()).settings.show_thumbnails);
        // Nor does a value this version cannot read cost them.
        std::fs::write(&file, "show_thumbnails = \"no\"\n").expect("written");
        assert!(load(dir.path()).settings.show_thumbnails);
    }

    #[test]
    fn long_presses_and_checkboxes_round_trip_and_old_files_lack_them() {
        let defaults = Settings::default();
        assert!(defaults.long_press);
        assert_eq!(defaults.long_press_ms, 500);
        assert!(!defaults.checkbox_select);
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        let settings = Settings {
            long_press: false,
            long_press_ms: 1200,
            checkbox_select: true,
            ..Settings::default()
        };
        settings.save(dir.path()).expect("saved");
        let text = std::fs::read_to_string(&file).expect("read");
        assert!(text.contains("long_press = false"), "{text}");
        assert!(text.contains("long_press_ms = 1200"), "{text}");
        assert!(text.contains("checkbox_select = true"), "{text}");
        assert_eq!(load(dir.path()), writable(settings));
        // The demos before had none of the keys: the defaults.
        std::fs::write(&file, "theme = \"dark\"\n").expect("written");
        let old = load(dir.path()).settings;
        assert!(old.long_press);
        assert_eq!(old.long_press_ms, 500);
        assert!(!old.checkbox_select);
    }

    #[test]
    fn a_long_press_time_is_brought_into_its_range_and_odd_values_take_the_default() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        for (text, want) in [
            ("long_press_ms = 100\n", 300),
            ("long_press_ms = 99999\n", 1500),
            ("long_press_ms = -5\n", 300),
            ("long_press_ms = 800\n", 800),
            ("long_press_ms = \"slow\"\n", 500),
            ("long_press_ms = 400.5\n", 500),
            ("long_press = \"yes\"\ncheckbox_select = 1\n", 500),
        ] {
            std::fs::write(&file, text).expect("written");
            let loaded = load(dir.path()).settings;
            assert_eq!(loaded.long_press_ms, want, "{text}");
            assert!(loaded.long_press, "{text}");
            assert!(!loaded.checkbox_select, "{text}");
        }
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
    fn folder_views_round_trip_without_their_defaults() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut settings = Settings::default();
        settings.set_views(&odd_views(dir.path()));
        let text = settings.to_text().expect("text");
        settings.save(dir.path()).expect("saved");

        let views = load(dir.path()).settings.views();
        assert_eq!(views, odd_views(dir.path()));
        // The one used last comes last.
        let order: Vec<&str> = views.entries().map(|entry| entry.folder.as_str()).collect();
        let uri = |name: &str| folder(dir.path(), name).to_uri();
        assert_eq!(order, [uri("a"), uri("사진"), uri("c")]);
        // A view keeps only what differs from the default view: a's has its mode alone.
        let a = &settings.folder_views[0];
        assert_eq!(a.mode, "large-icons");
        assert_eq!((a.columns.as_ref(), a.sort.as_str()), (None, ""));
        assert!(
            !text.contains("folders_first") && !text.contains("then_descending"),
            "{text}"
        );
        let c = &settings.folder_views[2];
        assert_eq!(
            c.columns.as_deref(),
            Some(
                [
                    "attributes".to_owned(),
                    "size".to_owned(),
                    "modified".to_owned(),
                    "created".to_owned()
                ]
                .as_slice()
            )
        );
        assert_eq!(c.widths, [56, 88, 126, 126]);
        assert_eq!(
            (c.sort.as_str(), c.descending, c.then.as_str(), c.mixed),
            ("modified", true, "extension", true)
        );
        // No views, no table: the file reads as the first demo's did.
        settings.set_views(&ViewStore::default());
        assert!(!settings.to_text().expect("text").contains("folder_views"));
    }

    #[test]
    fn folder_views_of_a_newer_version_fall_back_one_key_at_a_time() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let a = toml_string(&folder(dir.path(), "a").to_uri());
        let b = toml_string(&folder(dir.path(), "b").to_uri());
        let text = format!(
            "theme = \"dark\"\n\n[[folder_views]]\nfolder = {a}\nmode = \"thumbnails\"\n\
             columns = [\"size\", \"tags\", \"kind\", \"name\", \"size\"]\n\
             widths = [5000, 90, 0]\nsort = \"rating\"\ndescending = true\nthen = \"kind\"\n\
             zoom = 2\n\n[[folder_views]]\nfolder = {b}\nmode = 7\nsubfolders = \"yes\"\n\
             mixed = true\n"
        );
        std::fs::write(dir.path().join(FILE_NAME), &text).expect("written");

        let settings = load(dir.path()).settings;
        assert_eq!(settings.theme, ThemeChoice::Dark);
        let views = settings.views();
        let first = views.own(&folder(dir.path(), "a")).expect("a's view");
        // A mode it does not know: 자세히. A column it does not know is left out, and so is the
        // name and a column twice; a width past the most is the most, and 0 the default.
        assert_eq!(first.view.mode, ViewMode::Details);
        let columns: Vec<(Column, u32)> = first
            .view
            .columns
            .iter()
            .map(|shown| (shown.column, shown.width))
            .collect();
        assert_eq!(
            columns,
            [(Column::Size, MAX_COLUMN_WIDTH), (Column::Kind, 104)]
        );
        // A key it does not know: the name, in the direction given; the second key stays.
        assert_eq!(first.view.sort.primary.key, rf_vfs::SortKey::Name);
        assert_eq!(first.view.sort.primary.order, SortOrder::Descending);
        assert_eq!(
            first.view.sort.secondary.map(|by| by.key),
            Some(rf_vfs::SortKey::Kind)
        );
        assert_eq!(
            first.extra.get("zoom"),
            Some(&toml::Value::Integer(2)),
            "a key of a newer version stays"
        );
        let second = views.own(&folder(dir.path(), "b")).expect("b's view");
        assert_eq!(second.view.mode, ViewMode::Details);
        assert!(!second.subfolders);
        assert!(!second.view.sort.folders_first);
        // Written back, the newer key stays, and so do the values this version did not know.
        let mut again = settings.clone();
        again.set_views(&views);
        let written = again.to_text().expect("text");
        assert!(written.contains("zoom = 2"));
        let entry = &again.folder_views[0];
        assert_eq!(entry.mode, "thumbnails");
        assert_eq!(
            entry.columns.as_deref(),
            Some(&["size".to_owned(), "kind".to_owned(), "tags".to_owned()][..]),
            "the unknown column after the known ones"
        );
        assert_eq!(entry.widths, [MAX_COLUMN_WIDTH, 104, 90]);
        assert_eq!(entry.sort, "rating");
        assert!(entry.descending);
        assert_eq!(entry.then, "kind");
        // Read back, the same view and the same unknown values.
        let read = FolderViewSettings::stored(entry);
        assert_eq!(read.view, first.view);
        assert_eq!(read.unknown, first.unknown);

        // A table that is no list, and entries that are no tables, leave no views.
        for broken in [
            "folder_views = 5\n",
            "folder_views = [1, \"x\"]\n",
            "[folder_views]\nfolder = \"x\"\n",
        ] {
            std::fs::write(dir.path().join(FILE_NAME), broken).expect("written");
            let loaded = load(dir.path());
            assert!(loaded.writable, "{broken}");
            assert!(loaded.settings.views().is_empty(), "{broken}");
        }
    }

    #[test]
    fn a_file_with_more_views_than_kept_keeps_the_ones_used_last() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let many = crate::state::MAX_FOLDER_VIEWS + 5;
        let settings = Settings {
            folder_views: (0..many)
                .map(|i| FolderViewSettings {
                    folder: folder(dir.path(), &format!("f{i}")).to_uri(),
                    mode: "list".to_owned(),
                    ..FolderViewSettings::default()
                })
                .collect(),
            ..Settings::default()
        };
        settings.save(dir.path()).expect("saved");
        let views = load(dir.path()).settings.views();
        assert_eq!(views.len(), crate::state::MAX_FOLDER_VIEWS);
        assert!(views.own(&folder(dir.path(), "f4")).is_none());
        assert!(views.own(&folder(dir.path(), "f5")).is_some());
        assert!(
            views
                .own(&folder(dir.path(), &format!("f{}", many - 1)))
                .is_some()
        );
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
        // The extensions show, as they did then.
        assert!(!session.hide_extensions);
        assert_eq!(session.layout, Layout::default());
        assert!(settings.views().is_empty());

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
    fn pinned_tabs_are_kept_by_their_folders() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let (a, b) = (folder(dir.path(), "a"), folder(dir.path(), "b"));
        let (qa, qb) = (toml_string(&a.to_uri()), toml_string(&b.to_uri()));
        let session = |tabs: &[&VPath], pinned: Vec<bool>| Session {
            panes: [
                SessionPane {
                    tabs: tabs.iter().map(|&path| path.clone()).collect(),
                    active: 0,
                    pinned,
                },
                SessionPane::default(),
            ],
            ..Session::default()
        };
        // The folder of each pinned tab.
        let mut settings = Settings::default();
        settings.set_session(&session(&[&a, &b], vec![false, true]));
        let text = settings.to_text().expect("text");
        assert!(text.contains(&format!("pinned = [{qb}]")), "{text}");
        settings.save(dir.path()).expect("saved");
        assert_eq!(
            load(dir.path()).settings.session(),
            session(&[&a, &b], vec![false, true])
        );
        // Nothing pinned, nothing written: the file reads as the first demo's did.
        settings.set_session(&session(&[&a, &b], Vec::new()));
        assert!(!settings.to_text().expect("text").contains("pinned"));

        // The first demo, run in between, kept the pins as they were and wrote the tabs in
        // another order: the pins stay on their folders' tabs.
        let read = |text: String| {
            std::fs::write(dir.path().join(FILE_NAME), text).expect("written");
            load(dir.path()).settings.session().panes[0].clone()
        };
        let pane = read(format!("[[panes]]\ntabs = [{qb}, {qa}]\npinned = [{qb}]\n"));
        assert_eq!(pane.tabs, [b.clone(), a.clone()]);
        assert_eq!(pane.pinned, [true]);
        let pane = read(format!("[[panes]]\ntabs = [{qa}, {qb}]\npinned = [{qb}]\n"));
        assert_eq!(pane.pinned, [false, true]);
        // One pin for each entry, on the first tab at its folder not pinned yet; a pin whose
        // folder no tab shows any more pins nothing.
        let qc = toml_string(&folder(dir.path(), "c").to_uri());
        let pane = read(format!(
            "[[panes]]\ntabs = [{qa}, {qb}, {qa}]\npinned = [{qa}, {qc}]\n"
        ));
        assert_eq!(pane.pinned, [true]);
        let pane = read(format!(
            "[[panes]]\ntabs = [{qa}, {qb}, {qa}]\npinned = [{qa}, {qa}]\n"
        ));
        assert_eq!(pane.pinned, [true, false, true]);
        // Pins of a newer version that are no list of folders pin nothing; the tabs stay.
        let pane = read(format!(
            "[[panes]]\ntabs = [{qa}, {qb}]\npinned = [true, false]\nactive = 1\n"
        ));
        assert_eq!(pane.tabs, [a.clone(), b.clone()]);
        assert_eq!(pane.active, 1);
        assert_eq!(pane.pinned, Vec::<bool>::new());

        // A tab this version cannot read keeps its pin, after the others.
        let mut stored = Settings {
            panes: vec![PaneSettings {
                tabs: vec!["cloud+new://x".to_owned(), a.to_uri(), b.to_uri()],
                pinned: vec!["cloud+new://x".to_owned(), b.to_uri()],
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
        assert_eq!(
            stored.panes[0].pinned,
            [b.to_uri(), "cloud+new://x".to_owned()]
        );
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
        assert_eq!(backups(dir.path()), Vec::<String>::new());
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
    fn a_file_without_a_version_is_version_zero_and_upgraded_by_the_next_save() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        let old = "theme = \"dark\"\nshow_hidden = true\n";
        std::fs::write(&file, old).expect("written");

        let loaded = load(dir.path());

        // 0 → 1 in memory: read as it says, no backup, the file as it was.
        assert!(loaded.writable);
        assert_eq!(loaded.backup, None);
        assert_eq!(loaded.settings.version, SETTINGS_VERSION);
        assert_eq!(loaded.settings.theme, ThemeChoice::Dark);
        assert!(loaded.settings.show_hidden);
        assert_eq!(std::fs::read_to_string(&file).expect("kept"), old);
        // The next save writes the version, first, and nothing else is lost.
        loaded.settings.save(dir.path()).expect("saved");
        let text = std::fs::read_to_string(&file).expect("saved");
        assert!(text.starts_with("version = 1\n"), "{text}");
        assert_eq!(load(dir.path()), writable(loaded.settings));
        assert_eq!(backups(dir.path()), Vec::<String>::new());
        // A fresh start writes it too.
        assert!(
            Settings::default()
                .to_text()
                .expect("text")
                .starts_with("version = 1\n")
        );
    }

    #[test]
    fn migrating_from_zero_adds_the_version_and_changes_nothing_else() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut zero: toml::Table =
            toml::from_str(&full(dir.path()).to_text().expect("text")).expect("a table");
        zero.remove("version").expect("written with a version");
        let mut one = zero.clone();
        one.insert("version".to_owned(), toml::Value::Integer(1));

        assert_eq!(migrate(zero.clone(), 0), Ok(one.clone()));
        // A file at the current version stays as it is; a newer one is not migrated.
        assert_eq!(migrate(one.clone(), 1), Ok(one.clone()));
        assert_eq!(migrate(one, 2), Err(MigrateError::Newer { found: 2 }));
        // A version that says nothing readable counts as 0.
        for odd in ["version = \"one\"\n", "version = -3\n", "version = 1.5\n"] {
            let doc: toml::Table = toml::from_str(odd).expect("a table");
            let from = version_of_value(doc.get("version"));
            assert_eq!(from, 0, "{odd}");
            assert_eq!(parse(odd).expect("read").version, 1, "{odd}");
        }
    }

    #[test]
    fn a_file_that_cannot_be_used_is_moved_to_a_backup_and_the_app_starts_with_the_defaults() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);

        for (n, broken) in [
            &b"theme = \"dark\"\npanes = [[[[\n"[..],
            b"theme = \"dark\"\ntheme = \"light\"\n",
            b"\xFF\xFE not text",
        ]
        .into_iter()
        .enumerate()
        {
            std::fs::write(&file, broken).expect("written");
            let loaded = load_at(dir.path());
            let name = if n == 0 {
                STAMPED.to_owned()
            } else {
                format!("{STAMPED}-{}", n + 1)
            };
            let backup = dir.path().join(&name);
            assert_eq!(
                loaded,
                Loaded {
                    settings: Settings::default(),
                    writable: true,
                    backup: Some(backup.clone()),
                }
            );
            // Moved, not copied: the file is gone, the backup holds its bytes.
            assert!(!file.exists(), "{broken:?} stays in place");
            assert_eq!(std::fs::read(&backup).expect("backed up"), broken);
            // The notice, once the window is open, names the backup.
            assert_eq!(
                crate::words::settings_backed_up(&backup),
                format!(
                    "설정 파일을 읽을 수 없어 백업하고 기본값으로 시작했습니다: {}",
                    backup.display()
                )
            );
        }
        // Every file that was moved is still there.
        assert_eq!(
            backups(dir.path()),
            [STAMPED, &format!("{STAMPED}-2"), &format!("{STAMPED}-3")]
        );

        // Settings saved after that load as usual, and nothing is backed up again.
        full(dir.path()).save(dir.path()).expect("saved");
        assert_eq!(load_at(dir.path()), writable(full(dir.path())));
        assert_eq!(backups(dir.path()).len(), 3);
    }

    #[test]
    fn a_backup_never_takes_the_place_of_a_file_that_is_there() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        // The name of this second, and the next two, are taken.
        for taken in [STAMPED.to_owned(), format!("{STAMPED}-2")] {
            std::fs::write(dir.path().join(taken), "an older backup").expect("written");
        }
        std::fs::write(&file, "theme = [[[").expect("written");

        let loaded = load_at(dir.path());

        assert_eq!(loaded.backup, Some(dir.path().join(format!("{STAMPED}-3"))));
        assert!(loaded.writable);
        for taken in [STAMPED.to_owned(), format!("{STAMPED}-2")] {
            assert_eq!(
                std::fs::read_to_string(dir.path().join(taken)).expect("kept"),
                "an older backup"
            );
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join(format!("{STAMPED}-3"))).expect("moved"),
            "theme = [[["
        );
        assert_eq!(backup_name(at()), STAMPED);
    }

    #[test]
    fn a_file_with_every_backup_name_taken_stays_and_is_not_written_over() {
        let dir = tempfile::tempdir().expect("a temp dir");
        for taken in [STAMPED.to_owned(), format!("{STAMPED}-2")] {
            std::fs::write(dir.path().join(taken), "older").expect("written");
        }
        assert_eq!(
            backup_path(dir.path(), STAMPED, 3),
            Some(dir.path().join(format!("{STAMPED}-3")))
        );
        assert_eq!(backup_path(dir.path(), STAMPED, 2), None);
        assert_eq!(backup_path(dir.path(), STAMPED, 0), None);
    }

    #[test]
    fn a_file_of_a_newer_version_that_can_be_read_is_kept_as_it_is() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        let newer = "version = 7\ntheme = \"neon\"\nshow_hidden = true\nzoom = 3\n\
                     [sparkle]\nlevel = 9\n";
        std::fs::write(&file, newer).expect("written");

        let loaded = load_at(dir.path());

        // Read as far as it goes, not backed up, not moved.
        assert!(loaded.writable);
        assert_eq!(loaded.backup, None);
        assert!(file.is_file());
        assert_eq!(backups(dir.path()), Vec::<String>::new());
        let settings = loaded.settings;
        assert_eq!(settings.version, 7);
        assert_eq!(
            settings.theme,
            ThemeChoice::System,
            "a theme it does not know"
        );
        assert!(settings.show_hidden);
        // Its keys and its version are written back.
        let text = settings.to_text().expect("text");
        assert!(text.starts_with("version = 7\n"), "{text}");
        assert!(text.contains("zoom = 3"), "{text}");
        assert!(text.contains("[sparkle]"), "{text}");
        assert_eq!(peek_theme(dir.path()), ThemeChoice::System);
    }

    #[test]
    fn a_file_of_a_newer_version_that_cannot_be_read_is_backed_up() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        let newer = "version = 7\nwhen = tomorrow at noon\n";
        std::fs::write(&file, newer).expect("written");

        let loaded = load_at(dir.path());

        let backup = dir.path().join(STAMPED);
        assert_eq!(
            loaded,
            Loaded {
                settings: Settings::default(),
                writable: true,
                backup: Some(backup.clone()),
            }
        );
        assert!(!file.exists());
        assert_eq!(std::fs::read_to_string(backup).expect("moved"), newer);
    }

    #[test]
    fn a_huge_file_is_moved_to_a_backup() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let huge = format!(
            "# {}\n",
            "x".repeat(usize::try_from(MAX_FILE_SIZE).unwrap())
        );
        std::fs::write(dir.path().join(FILE_NAME), huge).expect("written");
        assert_eq!(load(dir.path()).settings, Settings::default());
        assert_eq!(backups(dir.path()).len(), 1);
        assert!(!dir.path().join(FILE_NAME).exists());
    }

    #[test]
    fn a_file_that_cannot_be_read_or_backed_up_is_never_replaced() {
        // A folder in the place of the file: it cannot be read, and stays.
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        std::fs::create_dir(&file).expect("made");
        std::fs::write(file.join("keep"), "keep").expect("written");
        let loaded = load(dir.path());
        assert_eq!(loaded.settings, Settings::default());
        assert!(!loaded.writable);
        assert_eq!(loaded.backup, None);
        assert!(file.join("keep").is_file());
        assert_eq!(backups(dir.path()), Vec::<String>::new());

        // A broken file that cannot be moved: it stays, nothing is told, and the run that
        // starts with the defaults never saves over it.
        let dir = tempfile::tempdir().expect("a temp dir");
        let file = dir.path().join(FILE_NAME);
        std::fs::write(&file, "theme = [[[").expect("written");
        let loaded = load_with(dir.path(), &at, &|_, _| {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        });
        assert_eq!(loaded.settings, Settings::default());
        assert!(!loaded.writable);
        assert_eq!(loaded.backup, None);
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
        assert_eq!(backups(dir.path()), Vec::<String>::new());
        assert!(!dir.path().join(TEMP_FILE_NAME).exists());
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
            hide_extensions: false,
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
    fn a_stopped_saver_writes_nothing_more() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let saver = Saver::new(dir.path().to_owned(), true);
        let first = Settings {
            theme: ThemeChoice::Dark,
            ..Settings::default()
        };
        assert!(matches!(
            saver.save_within(first.to_text().expect("text"), Duration::from_secs(10)),
            Some(Ok(()))
        ));

        assert!(saver.stop_within(Duration::from_secs(10)));

        // A save asked for after that reports success and writes nothing, and a folder that was
        // deleted is not made again.
        let later = Settings {
            theme: ThemeChoice::Light,
            ..Settings::default()
        };
        assert!(matches!(
            saver.save_within(later.to_text().expect("text"), Duration::from_secs(10)),
            Some(Ok(()))
        ));
        assert_eq!(load(dir.path()), writable(first));
        std::fs::remove_dir_all(dir.path()).expect("deleted");
        assert!(matches!(
            saver.save_within(later.to_text().expect("text"), Duration::from_secs(10)),
            Some(Ok(()))
        ));
        assert!(!dir.path().exists());
        // The tempdir's own drop finds nothing to delete; that is fine.
    }

    #[test]
    fn a_saver_stops_once_a_write_under_way_ends_and_gives_up_on_a_hung_one() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let saver = Saver::new(dir.path().to_owned(), true);
        let hung = saver.written.lock().expect("not poisoned");

        let started = Instant::now();
        assert!(!saver.stop_within(Duration::from_millis(100)));
        assert!(started.elapsed() < Duration::from_secs(5));

        drop(hung);
        assert!(saver.stop_within(Duration::from_secs(10)));
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
