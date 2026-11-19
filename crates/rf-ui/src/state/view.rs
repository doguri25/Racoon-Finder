//! How a pane shows its folder (NAV-06, NAV-07, NAV-08, NAV-10), and the commands that change
//! it. A child of [`state`](super), so these methods see the fields of [`AppState`].
//!
//! A [`FolderView`] is the view mode ([`ViewMode`]: 자세히, 목록, icons of four sizes, the
//! thumbnails at a size of their own, or tiles), the columns of 자세히 after the name
//! ([`ShownColumn`]: which, in what order, how wide) and the order of the rows (a `SortSpec`: a
//! key, a second key for ties, folders first or mixed in).
//!
//! The views are remembered per folder ([`ViewStore`], kept in the settings): a change of the
//! view of a tab is remembered for its folder, and a tab that goes to a folder shows the view
//! remembered for it. A folder without a view of its own shows the view of the nearest folder
//! above it that applies its view to its subfolders (하위 폴더에 적용), or else the default one
//! ([`FolderView::default`]). The store keeps at most [`MAX_FOLDER_VIEWS`] folders, and drops
//! the one used longest ago first (a use is a tab showing the folder's view, or a change of it).
//! Folders are known by their `VPath::to_uri`, compared as the system compares names
//! (`rf_vfs::fold_name`): ignoring case on Windows and macOS, and the Unicode form (NFC or NFD)
//! on macOS, exactly elsewhere.
//!
//! The keys move the cursor as the view lays the rows out ([`target`]): 자세히 one row per line,
//! the icons, the thumbnails and the tiles left to right and then down (row-major,
//! [`grid_shape`]), 목록 top to bottom and then right (column-major).

use rf_core::VPath;
use rf_vfs::{ShellPlace, SortBy, SortKey, SortOrder, SortSpec};

use super::{AppState, Effect};
use crate::dialogs::{Confirm, Modal};
use crate::pane::Pane;
use crate::selection::Pick;
use crate::words;

mod store;

pub use store::{Chosen, MAX_FOLDER_URI, MAX_FOLDER_VIEWS, StoredView, UnknownParts, ViewStore};

/// The narrowest and the widest a column of 자세히 can be made, in logical pixels.
pub const MIN_COLUMN_WIDTH: u32 = 40;
pub const MAX_COLUMN_WIDTH: u32 = 600;

/// The smallest and the largest side of a thumbnail in the thumbnail view (썸네일), in logical
/// pixels: its size slider's ends, which Ctrl+wheel and a pinch stop at too.
pub const MIN_THUMB_SIZE: u32 = 64;
pub const MAX_THUMB_SIZE: u32 = 512;
/// The size the thumbnail view takes until one is chosen (a folder that shows it for the first
/// time in a run), and of a settings entry that names none.
pub const DEFAULT_THUMB_SIZE: u32 = 192;
/// The sizes a thumbnail is asked for in ([`thumb_request_side`]): the view's own size rounded
/// up to one of them, so that a drag of the slider asks the pool for no new picture until it
/// passes a step, and the pictures made for a step serve every size below it. The first two are
/// the large and the extra large icons' sides, whose cache the thumbnail view shares.
pub const THUMB_STEPS: [u32; 4] = [96, 160, 256, 512];

/// How a pane lays out its rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ViewMode {
    /// One row per item, with columns (자세히).
    #[default]
    Details,
    /// Names with small icons, top to bottom and then in the next column (목록).
    List,
    /// Icons left to right and then down, with the name beside a small one or under a larger
    /// one (작은 아이콘, 보통 아이콘, 큰 아이콘, 특대 아이콘). From the medium icons up, a file
    /// shows its thumbnail where it has one (VIEW-09, [`ViewMode::thumb_side`]).
    SmallIcons,
    MediumIcons,
    LargeIcons,
    ExtraLargeIcons,
    /// The thumbnails (썸네일, NAV-06) left to right and then down, each in a square of `size`
    /// logical pixels a side ([`MIN_THUMB_SIZE`] to [`MAX_THUMB_SIZE`], any size between: the
    /// status bar's slider, Ctrl+wheel and a pinch change it), the name under it. A folder, and
    /// a file without a thumbnail, show their kind's icon there.
    Thumbnails {
        size: u32,
    },
    /// Tiles (타일): a 48 px icon, or the file's thumbnail, beside three lines (the name, the
    /// kind and the size), left to right and then down as the icons.
    Tiles,
}

impl ViewMode {
    /// Every mode, in the order the 보기 menu lists them: the thumbnails (at their default size)
    /// and the largest icons first, then as File Explorer lists its views, 타일 after 자세히.
    pub const ALL: [ViewMode; 8] = [
        ViewMode::Thumbnails {
            size: DEFAULT_THUMB_SIZE,
        },
        ViewMode::ExtraLargeIcons,
        ViewMode::LargeIcons,
        ViewMode::MediumIcons,
        ViewMode::SmallIcons,
        ViewMode::List,
        ViewMode::Details,
        ViewMode::Tiles,
    ];

    /// The thumbnail view at `size`, made to lie between [`MIN_THUMB_SIZE`] and
    /// [`MAX_THUMB_SIZE`].
    pub fn thumbnails(size: u32) -> ViewMode {
        ViewMode::Thumbnails {
            size: size.clamp(MIN_THUMB_SIZE, MAX_THUMB_SIZE),
        }
    }

    /// The mode's name in the settings file. The thumbnail view's size is a key of its own
    /// (`thumb_size`), so that 0.4.0, which knows no such mode, keeps it as it is.
    pub fn name(self) -> &'static str {
        match self {
            ViewMode::Details => "details",
            ViewMode::List => "list",
            ViewMode::SmallIcons => "small-icons",
            ViewMode::MediumIcons => "medium-icons",
            ViewMode::LargeIcons => "large-icons",
            ViewMode::ExtraLargeIcons => "extra-large-icons",
            ViewMode::Thumbnails { .. } => "thumbnails",
            ViewMode::Tiles => "tiles",
        }
    }

    /// The mode the settings file calls `name` (the thumbnails at their default size); `None`
    /// for a name this version does not know.
    pub fn from_name(name: &str) -> Option<ViewMode> {
        ViewMode::ALL.into_iter().find(|mode| mode.name() == name)
    }

    /// The number the screen knows the mode by (`PaneView.mode`): 0 자세히, 1 목록, 2 to 5 the
    /// icons from the smallest, 6 the thumbnails (their size is `PaneView.thumb-size`), 7 tiles.
    pub fn number(self) -> i32 {
        match self {
            ViewMode::Details => 0,
            ViewMode::List => 1,
            ViewMode::SmallIcons => 2,
            ViewMode::MediumIcons => 3,
            ViewMode::LargeIcons => 4,
            ViewMode::ExtraLargeIcons => 5,
            ViewMode::Thumbnails { .. } => 6,
            ViewMode::Tiles => 7,
        }
    }

    /// The mode of the screen's number (the thumbnails at their default size); `None` for no
    /// mode.
    pub fn from_number(number: i32) -> Option<ViewMode> {
        ViewMode::ALL
            .into_iter()
            .find(|mode| mode.number() == number)
    }

    /// The thumbnail view's size, in logical pixels; `None` in the other views.
    pub fn thumb_size(self) -> Option<u32> {
        match self {
            ViewMode::Thumbnails { size } => Some(size),
            _ => None,
        }
    }

    /// The next icon size up (Ctrl+wheel up, a pinch out: COMP-04): 자세히, 목록 and 타일 grow
    /// into the small icons; `None` from the largest, and in the thumbnail view, whose size
    /// changes in between the steps ([`AppState::set_thumb_size`]).
    pub fn bigger(self) -> Option<ViewMode> {
        match self {
            ViewMode::Details | ViewMode::List | ViewMode::Tiles => Some(ViewMode::SmallIcons),
            ViewMode::SmallIcons => Some(ViewMode::MediumIcons),
            ViewMode::MediumIcons => Some(ViewMode::LargeIcons),
            ViewMode::LargeIcons => Some(ViewMode::ExtraLargeIcons),
            ViewMode::ExtraLargeIcons | ViewMode::Thumbnails { .. } => None,
        }
    }

    /// The next icon size down; `None` from the small icons, 자세히, 목록 and 타일, which the
    /// steps do not leave the icons for, and in the thumbnail view.
    pub fn smaller(self) -> Option<ViewMode> {
        match self {
            ViewMode::ExtraLargeIcons => Some(ViewMode::LargeIcons),
            ViewMode::LargeIcons => Some(ViewMode::MediumIcons),
            ViewMode::MediumIcons => Some(ViewMode::SmallIcons),
            ViewMode::SmallIcons
            | ViewMode::List
            | ViewMode::Details
            | ViewMode::Tiles
            | ViewMode::Thumbnails { .. } => None,
        }
    }

    /// The side a file's thumbnail is asked for in, in logical pixels, in the views that show
    /// thumbnails: 보통 48, 크게 96, 특대 160, 타일 48, and the thumbnail view's size rounded up
    /// to a step ([`thumb_request_side`]); the picture is drawn down to the view's own size. The
    /// others show each kind's icon.
    pub fn thumb_side(self) -> Option<u32> {
        match self {
            ViewMode::MediumIcons | ViewMode::Tiles => Some(48),
            ViewMode::LargeIcons => Some(96),
            ViewMode::ExtraLargeIcons => Some(160),
            ViewMode::Thumbnails { size } => Some(thumb_request_side(size)),
            ViewMode::SmallIcons | ViewMode::List | ViewMode::Details => None,
        }
    }

    /// The width of the room of one item in the views that lay the items out in rows (the
    /// icons, the thumbnails, the tiles), in logical pixels, as the screen gives it
    /// (`FileGrid`), of which the item takes all but a 4 px gap. `None` for 자세히 and 목록. Its
    /// height is the screen's alone: it follows the theme's row height and size of text
    /// (THEME-07), and nothing here reckons with it.
    pub fn cell_width(self) -> Option<u32> {
        match self {
            ViewMode::SmallIcons => Some(200),
            ViewMode::MediumIcons => Some(104),
            ViewMode::LargeIcons => Some(136),
            ViewMode::ExtraLargeIcons => Some(184),
            // The picture with 12 px on each side.
            ViewMode::Thumbnails { size } => Some(size + 24),
            ViewMode::Tiles => Some(TILE_WIDTH),
            ViewMode::Details | ViewMode::List => None,
        }
    }
}

/// How wide a tile is, in logical pixels: a 48 px icon and three lines of about 24 Hangul
/// syllables.
pub const TILE_WIDTH: u32 = 260;
/// The room the grid leaves at its sides (8 px each) and for its scroll bar (14 px), in logical
/// pixels, as `FileGrid` does.
pub const GRID_PAD: u32 = 8;
pub const GRID_BAR: u32 = 14;

/// The side a thumbnail of the thumbnail view at `size` is asked for in: the first of
/// [`THUMB_STEPS`] that is no smaller (the largest for a size beyond them).
pub fn thumb_request_side(size: u32) -> u32 {
    THUMB_STEPS
        .into_iter()
        .find(|&step| step >= size)
        .unwrap_or(MAX_THUMB_SIZE)
}

/// How the items of a view lie in rows: how many a row holds, and how many rows there are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridShape {
    pub per_row: usize,
    pub rows: usize,
}

/// How `count` items of `mode` lie in a list `width` logical pixels wide, as `FileGrid` lays
/// them out (`per-row`, `line-count`): as many in a row as whole cells fit beside the side pads
/// and the scroll bar's room, at least one. `None` for 자세히 and 목록, which are no rows of
/// items.
pub fn grid_shape(mode: ViewMode, width: u32, count: usize) -> Option<GridShape> {
    let cell = mode.cell_width()?;
    let room = width.saturating_sub(2 * GRID_PAD + GRID_BAR);
    let per_row = usize::try_from(room / cell).unwrap_or(usize::MAX).max(1);
    Some(GridShape {
        per_row,
        rows: count.div_ceil(per_row),
    })
}

/// A column of 자세히. The name always shows, first, and takes the room the others leave; the
/// others show or not, in the order the user put them, as wide as the user made them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    Name,
    Size,
    Modified,
    Created,
    Kind,
    Extension,
    /// The attribute letters (R, H, S, A, ...), [`crate::format::attribute_letters`].
    Attributes,
    /// The last access time, as the listing read it with the rest of each item's metadata (no
    /// look of its own at the files). The file system keeps it as it likes: Linux mounts with
    /// `relatime` or `noatime` update it seldom or never, Windows may have it off.
    Accessed,
    /// The folder the item is in. In a folder's listing that is the folder itself on every row;
    /// it tells the items apart once a view lists items of many folders (search, a flat view).
    Path,
    /// Where an item of the trash was before it was trashed (원래 위치, `Entry::origin`): the
    /// trash's column, empty where the trash does not keep it.
    Original,
    /// When an item of the trash was trashed (삭제한 날짜, `Entry::deleted`): the trash's column.
    Deleted,
}

impl Column {
    /// The columns besides the name of a folder, in the order the header's menu lists them.
    pub const OPTIONAL: [Column; 8] = [
        Column::Size,
        Column::Modified,
        Column::Created,
        Column::Accessed,
        Column::Kind,
        Column::Extension,
        Column::Attributes,
        Column::Path,
    ];

    /// The columns besides the name of the trash (`shell://trash`), in the order its header's
    /// menu lists them: what the trash keeps of an item. The times and the attributes of the item
    /// itself are not kept, and its folder is the trash.
    pub const TRASH: [Column; 5] = [
        Column::Original,
        Column::Deleted,
        Column::Size,
        Column::Kind,
        Column::Extension,
    ];

    /// Every column but the name, the folders' and the trash's.
    const ALL_OPTIONAL: [Column; 10] = [
        Column::Size,
        Column::Modified,
        Column::Created,
        Column::Accessed,
        Column::Kind,
        Column::Extension,
        Column::Attributes,
        Column::Path,
        Column::Original,
        Column::Deleted,
    ];

    /// The columns besides the name that a view of `folder` offers: the trash's in the trash
    /// ([`Column::TRASH`]), none in the computer (`shell://computer`, which shows tiles), and a
    /// folder's elsewhere ([`Column::OPTIONAL`]).
    pub fn offered(folder: &VPath) -> &'static [Column] {
        match ShellPlace::of(folder) {
            Some(ShellPlace::Trash) => &Column::TRASH,
            Some(ShellPlace::Computer) => &[],
            _ => &Column::OPTIONAL,
        }
    }

    /// The column's name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            Column::Name => "name",
            Column::Size => "size",
            Column::Modified => "modified",
            Column::Created => "created",
            Column::Kind => "kind",
            Column::Extension => "extension",
            Column::Attributes => "attributes",
            Column::Accessed => "accessed",
            Column::Path => "path",
            Column::Original => "original",
            Column::Deleted => "deleted",
        }
    }

    /// The column the settings file calls `name`; `None` for one this version does not know.
    pub fn from_name(name: &str) -> Option<Column> {
        std::iter::once(Column::Name)
            .chain(Column::ALL_OPTIONAL)
            .find(|column| column.name() == name)
    }

    /// The number the screen knows the column by: 0 name, 1 size, 2 modified, 3 kind (the
    /// numbers of the first demo's column heads), 4 created, 5 extension, 6 attributes,
    /// 7 accessed, 8 path, 9 original place, 10 deleted.
    pub fn number(self) -> i32 {
        match self {
            Column::Name => 0,
            Column::Size => 1,
            Column::Modified => 2,
            Column::Kind => 3,
            Column::Created => 4,
            Column::Extension => 5,
            Column::Attributes => 6,
            Column::Accessed => 7,
            Column::Path => 8,
            Column::Original => 9,
            Column::Deleted => 10,
        }
    }

    /// How many numbers the columns take (0 to 10): the length of `PaneView.shows`.
    pub const COUNT: usize = 11;

    /// The column of the screen's number; `None` for no column.
    pub fn from_number(number: i32) -> Option<Column> {
        std::iter::once(Column::Name)
            .chain(Column::ALL_OPTIONAL)
            .find(|column| column.number() == number)
    }

    /// What its head says.
    pub fn title(self) -> &'static str {
        words::column_title(self)
    }

    /// The key a click on its head sorts by; `None` for the attributes, which sort nothing, and
    /// the path, the same on every row of a folder.
    pub fn sort_key(self) -> Option<SortKey> {
        Some(match self {
            Column::Name => SortKey::Name,
            Column::Size => SortKey::Size,
            Column::Modified => SortKey::Modified,
            Column::Created => SortKey::Created,
            Column::Kind => SortKey::Kind,
            Column::Extension => SortKey::Extension,
            Column::Accessed => SortKey::Accessed,
            Column::Original => SortKey::Origin,
            Column::Deleted => SortKey::Deleted,
            Column::Attributes | Column::Path => return None,
        })
    }

    /// The column whose head sorts by `key`.
    pub fn of_key(key: SortKey) -> Column {
        match key {
            SortKey::Name => Column::Name,
            SortKey::Size => Column::Size,
            SortKey::Modified => Column::Modified,
            SortKey::Created => Column::Created,
            SortKey::Kind => Column::Kind,
            SortKey::Extension => Column::Extension,
            SortKey::Accessed => Column::Accessed,
            SortKey::Origin => Column::Original,
            SortKey::Deleted => Column::Deleted,
        }
    }

    /// How wide it is until the user makes it otherwise, in logical pixels: the date columns
    /// hold the widest `2026-10-28 22:08`.
    pub fn default_width(self) -> u32 {
        match self {
            Column::Name => 120,
            Column::Size => 72,
            Column::Modified | Column::Created | Column::Accessed | Column::Deleted => 126,
            Column::Kind => 104,
            Column::Extension => 64,
            Column::Attributes => 56,
            Column::Path | Column::Original => 220,
        }
    }

    /// Whether its text lines up on the right (the sizes).
    pub fn align_end(self) -> bool {
        self == Column::Size
    }
}

/// A column of 자세히 after the name, and its width in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShownColumn {
    pub column: Column,
    pub width: u32,
}

impl ShownColumn {
    /// `column` at its default width.
    pub fn new(column: Column) -> ShownColumn {
        ShownColumn {
            column,
            width: column.default_width(),
        }
    }
}

/// How a pane shows a folder: the view mode, the columns of 자세히 after the name, and the
/// order of the rows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FolderView {
    pub mode: ViewMode,
    /// Never [`Column::Name`], and each column once.
    pub columns: Vec<ShownColumn>,
    pub sort: SortSpec,
}

impl Default for FolderView {
    /// 자세히 with the size, the modification time and the kind after the name (the first
    /// demo's columns), by name ascending, folders first.
    fn default() -> FolderView {
        FolderView {
            mode: ViewMode::Details,
            columns: [Column::Size, Column::Modified, Column::Kind]
                .into_iter()
                .map(ShownColumn::new)
                .collect(),
            sort: SortSpec::default(),
        }
    }
}

impl FolderView {
    /// The view `folder` shows until one is chosen for it: the trash's
    /// ([`FolderView::trash`]), the computer's ([`FolderView::computer`]), else the default one.
    pub fn default_for(folder: &VPath) -> FolderView {
        match ShellPlace::of(folder) {
            Some(ShellPlace::Trash) => FolderView::trash(),
            Some(ShellPlace::Computer) => FolderView::computer(),
            None => FolderView::default(),
        }
    }

    /// The trash's view until one is chosen for it: 자세히 with the original place, the time the
    /// item was trashed and its size after the name, by name ascending, folders first.
    pub fn trash() -> FolderView {
        FolderView {
            mode: ViewMode::Details,
            columns: [Column::Original, Column::Deleted, Column::Size]
                .into_iter()
                .map(ShownColumn::new)
                .collect(),
            sort: SortSpec::default(),
        }
    }

    /// The computer's view, which is the only one it has: its drives as tiles (each with the bar
    /// of how full it is), in the order of their top folders (the system's first, then by letter
    /// or by the folder they are mounted on).
    pub fn computer() -> FolderView {
        FolderView {
            mode: ViewMode::Tiles,
            columns: Vec::new(),
            sort: SortSpec {
                primary: SortBy {
                    key: SortKey::Origin,
                    order: SortOrder::Ascending,
                },
                secondary: None,
                folders_first: true,
            },
        }
    }

    /// The default view in the order `sort`.
    pub fn sorted(sort: SortSpec) -> FolderView {
        FolderView {
            sort,
            ..FolderView::default()
        }
    }

    /// Whether `column` shows (the name always does).
    pub fn shows(&self, column: Column) -> bool {
        column == Column::Name || self.columns.iter().any(|shown| shown.column == column)
    }

    /// Shows `column` after the others, at its default width, or hides it. The name always
    /// shows.
    pub fn toggle_column(&mut self, column: Column) {
        if column == Column::Name {
            return;
        }
        match self.columns.iter().position(|shown| shown.column == column) {
            Some(at) => {
                self.columns.remove(at);
            }
            None => self.columns.push(ShownColumn::new(column)),
        }
    }

    /// Moves the column at `from` (among the columns after the name) to `slot`, a place between
    /// them as they are now (0 before the first, their number after the last). `false`, and no
    /// change, when that leaves it where it is or a position is past the end.
    pub fn move_column(&mut self, from: usize, slot: usize) -> bool {
        let count = self.columns.len();
        if from >= count || slot > count || slot == from || slot == from + 1 {
            return false;
        }
        let moved = self.columns.remove(from);
        let to = if slot > from { slot - 1 } else { slot };
        self.columns.insert(to, moved);
        true
    }

    /// Makes `column` `width` logical pixels wide, between [`MIN_COLUMN_WIDTH`] and
    /// [`MAX_COLUMN_WIDTH`]; nothing for a column that does not show, or the name, which takes
    /// the room the others leave.
    pub fn set_width(&mut self, column: Column, width: u32) {
        if let Some(shown) = self.columns.iter_mut().find(|shown| shown.column == column) {
            shown.width = width.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH);
        }
    }

    /// A click on the head that sorts by `key` (with Shift: `then`). A plain click sorts by the
    /// key alone, ascending, or the other way round when it is the key that sorts now (the
    /// second key stays then). With Shift, it is the second key, which breaks the ties of the
    /// first: ascending, or the other way round when it is the second key already; on the
    /// first key's head, Shift does what a plain click does.
    pub fn sort_by(&mut self, key: SortKey, then: bool) {
        let flip = |order: SortOrder| match order {
            SortOrder::Ascending => SortOrder::Descending,
            SortOrder::Descending => SortOrder::Ascending,
        };
        let sort = &mut self.sort;
        if sort.primary.key == key {
            sort.primary.order = flip(sort.primary.order);
        } else if !then {
            sort.primary = SortBy {
                key,
                order: SortOrder::Ascending,
            };
            sort.secondary = None;
        } else {
            sort.secondary = Some(match sort.secondary {
                Some(by) if by.key == key => SortBy {
                    key,
                    order: flip(by.order),
                },
                _ => SortBy {
                    key,
                    order: SortOrder::Ascending,
                },
            });
        }
    }

    /// The second key, ascending (as it is, when it is that key already), or none; the key that
    /// sorts first is no second key.
    pub fn set_then(&mut self, key: Option<SortKey>) {
        let sort = &mut self.sort;
        sort.secondary = match key {
            None => None,
            Some(key) if key == sort.primary.key => None,
            Some(key) => match sort.secondary {
                Some(by) if by.key == key => Some(by),
                _ => Some(SortBy {
                    key,
                    order: SortOrder::Ascending,
                }),
            },
        };
    }
}

/// A key that moves the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyMove {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

/// How the rows lie on screen, for the keys: how many items a line holds (a row of icons, a
/// column of 목록, 1 in 자세히) and how many lines a page key moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paging {
    pub line: usize,
    pub page: usize,
}

impl Paging {
    /// One item a line and `page` lines a page: 자세히, where a page is rows.
    pub fn rows(page: usize) -> Paging {
        Paging { line: 1, page }
    }
}

/// Where `step` takes the cursor at `cursor` (`None`: no cursor yet) among `count` items laid
/// out by `mode` as `paging` says; `None` for a step that does not move in this mode (← and →
/// in 자세히) or an empty list.
///
/// - 자세히: ↑ and ↓ one row, the page keys a page of rows, Home and End the first and the last
///   row, and no row for ← and → (they scroll the columns sideways, [`Effect::ScrollSideways`]).
///   Without a cursor, ↓ lands on the first row, and so does ↑.
/// - Icons (row-major): ← and → the item before and after (from the end of a row to the next
///   one), ↑ and ↓ the item a row up and down (from a last row that is not full, ↓ goes to the
///   last item; at the first and the last row they stay), the page keys a page of rows up and
///   down, as far as the first and the last row.
/// - 목록 (column-major): ↑ and ↓ the item before and after (from the end of a column to the
///   next one), ← and → the item a column left and right, the page keys a page of columns.
///
/// Without a cursor every step but End lands on the first item.
pub fn target(
    mode: ViewMode,
    cursor: Option<usize>,
    count: usize,
    step: KeyMove,
    paging: Paging,
) -> Option<usize> {
    let last = count.checked_sub(1)?;
    let line = paging.line.max(1);
    let page = paging.page.max(1);
    if mode == ViewMode::Details {
        // As `Selection::move_cursor` moves in the first demo.
        let delta = match step {
            KeyMove::Left | KeyMove::Right => return None,
            KeyMove::Up => -1,
            KeyMove::Down => 1,
            KeyMove::Home => isize::MIN,
            KeyMove::End => isize::MAX,
            KeyMove::PageUp => -isize::try_from(page).unwrap_or(isize::MAX),
            KeyMove::PageDown => isize::try_from(page).unwrap_or(isize::MAX),
        };
        let to = match cursor {
            Some(cursor) => cursor.saturating_add_signed(delta),
            None if delta > 0 => delta.unsigned_abs() - 1,
            None => 0,
        };
        return Some(to.min(last));
    }
    let Some(cursor) = cursor.map(|cursor| cursor.min(last)) else {
        return Some(if step == KeyMove::End { last } else { 0 });
    };
    // The steps along the flow (an item) and across it (a line of `line` items): the icons
    // flow along rows, 목록 down columns.
    let list = mode == ViewMode::List;
    let page = isize::try_from(page).unwrap_or(isize::MAX);
    let (along, across): (isize, isize) = match step {
        KeyMove::Home => return Some(0),
        KeyMove::End => return Some(last),
        KeyMove::Up if list => (-1, 0),
        KeyMove::Down if list => (1, 0),
        KeyMove::Left if list => (0, -1),
        KeyMove::Right if list => (0, 1),
        KeyMove::Left => (-1, 0),
        KeyMove::Right => (1, 0),
        KeyMove::Up => (0, -1),
        KeyMove::Down => (0, 1),
        KeyMove::PageUp => (0, -page),
        KeyMove::PageDown => (0, page),
    };
    if along != 0 {
        return Some(cursor.saturating_add_signed(along).min(last));
    }
    // A move across the lines keeps the place in the line, as far as the first and the last
    // line; a last line too short to have that place gives its last item.
    let (at, place) = (cursor / line, cursor % line);
    let to = at.saturating_add_signed(across).min(last / line);
    Some((to * line + place).min(last))
}

impl AppState {
    /// The views remembered per folder (the settings keep them).
    pub fn views(&self) -> &ViewStore {
        &self.views
    }

    /// Takes the views remembered per folder (from the settings, at the start).
    pub fn set_views(&mut self, views: ViewStore) {
        self.views = views;
    }

    /// Gives each tab that has come to another folder the view remembered for it, and hands the
    /// sorts that are too long for the UI thread to a worker. The controller calls this after
    /// every change, with the change's effects: the listing a tab has just asked for (a
    /// [`Effect::Load`] among them) is asked for in the order of the folder's view, so the worker
    /// that lists it sorts it so. A sort a tab needs ([`Tab::take_sort_request`]) becomes an
    /// [`Effect::Sort`].
    ///
    /// [`Tab::take_sort_request`]: crate::pane::Tab::take_sort_request
    pub fn settle_views(&mut self, effects: &mut Vec<Effect>) {
        let views = &mut self.views;
        for tab in self.panes.iter_mut().flat_map(Pane::tabs_mut) {
            if tab.view_for() != Some(tab.location()) {
                let view = views.view_for(tab.location());
                let generation = tab.generation();
                let request = effects.iter_mut().find_map(|effect| match effect {
                    Effect::Load(request) if request.generation == generation => Some(request),
                    _ => None,
                });
                tab.settle_view(view, request);
            }
        }
        for tab in self.panes.iter_mut().flat_map(Pane::tabs_mut) {
            if let Some(request) = tab.take_sort_request() {
                effects.push(Effect::Sort(request));
            }
        }
    }

    /// A worker sorted the rows of a tab ([`Effect::Sort`]): the tab that asked takes them, if
    /// it still waits for them.
    pub fn sorted(&mut self, sorted: crate::pane::Sorted) -> Vec<Effect> {
        if let Some(tab) = self.tab_by_generation(sorted.generation) {
            tab.apply_sort(sorted);
        }
        Vec::new()
    }

    /// The worker could not do the sort number `serial` of the listing `generation` (it
    /// panicked, or no thread could be started for it): the tab that waits for it shows its rows
    /// in the order they are in, and a notice says the folder could not be sorted.
    pub fn sort_failed(&mut self, generation: u64, serial: u64) -> Vec<Effect> {
        let failed = self
            .tab_by_generation(generation)
            .is_some_and(|tab| tab.sort_failed(generation, serial));
        if failed {
            vec![Effect::Notice(words::sort_failed().to_owned())]
        } else {
            Vec::new()
        }
    }

    /// Changes the view of the folder the tab of pane `side` shows with `change`, which says
    /// which parts it chose ([`Chosen`]: they lose the values of a newer version, even where the
    /// choice is the value shown), remembers it for the folder, and shows it in every tab at
    /// that folder; the pane gets the keyboard.
    /// The change goes on top of the view the folder has now, not of what the tab shows: another
    /// tab at the folder may have changed it since (its sort, while this tab's columns change),
    /// which a whole view of this tab's would undo. A folder whose view cannot be kept
    /// ([`ViewStore::keeps`]) changes as its tab shows it.
    ///
    /// The computer (`shell://computer`) keeps its one view: nothing changes there, and the
    /// notice that says so is given back.
    fn change_view(
        &mut self,
        side: usize,
        change: impl FnOnce(&mut FolderView) -> Chosen,
    ) -> Option<Effect> {
        self.activate(side);
        let folder = self.tab(side).location().clone();
        if ShellPlace::of(&folder) == Some(ShellPlace::Computer) {
            return Some(Effect::Notice(words::computer_view_fixed().to_owned()));
        }
        let mut view = if self.views.keeps(&folder) {
            self.views.view_for(&folder)
        } else {
            self.tab(side).view().clone()
        };
        let chosen = change(&mut view);
        self.views.remember_choice(&folder, view.clone(), chosen);
        let views = &self.views;
        for tab in self.panes.iter_mut().flat_map(Pane::tabs_mut) {
            if views.same_folder(tab.location(), &folder) {
                tab.set_view(view.clone());
            }
        }
        None
    }

    /// Shows pane `side` in `mode`, and scrolls to its cursor there. The selection and the
    /// cursor stay as they were: a view lays the same rows out otherwise.
    pub fn set_view_mode(&mut self, side: usize, mode: ViewMode) -> Vec<Effect> {
        if let Some(refused) = self.change_view(side, |view| {
            view.mode = mode;
            Chosen {
                mode: true,
                ..Chosen::default()
            }
        }) {
            return vec![refused];
        }
        let row = self.tab(side).selection().cursor().unwrap_or(0);
        vec![Effect::ScrollTo { side, row }]
    }

    /// 보기 방식 ▸ 썸네일 for pane `side`: the thumbnail view at the size of the folder that
    /// shows it and was used last ([`ViewStore::last_thumb_size`]; [`DEFAULT_THUMB_SIZE`] when
    /// none does); a pane that shows it already keeps its size.
    pub fn show_thumbnail_view(&mut self, side: usize) -> Vec<Effect> {
        let size = self
            .tab(side)
            .view()
            .mode
            .thumb_size()
            .or_else(|| self.views.last_thumb_size())
            .unwrap_or(DEFAULT_THUMB_SIZE);
        self.set_view_mode(side, ViewMode::thumbnails(size))
    }

    /// The size of the thumbnail view of pane `side`, in logical pixels: the slider's value, or
    /// what Ctrl+wheel and a pinch made of it ([`ViewMode::thumbnails`] keeps it between the
    /// ends). Remembered for the folder as the other parts of its view are, and the cursor's
    /// item stays in view. Nothing in another view, or when the size is the one it has.
    pub fn set_thumb_size(&mut self, side: usize, size: u32) -> Vec<Effect> {
        let Some(now) = self.tab(side).view().mode.thumb_size() else {
            return Vec::new();
        };
        let mode = ViewMode::thumbnails(size);
        if mode.thumb_size() == Some(now) {
            return Vec::new();
        }
        if let Some(refused) = self.change_view(side, |view| {
            view.mode = mode;
            Chosen {
                mode: true,
                ..Chosen::default()
            }
        }) {
            return vec![refused];
        }
        match self.tab(side).selection().cursor() {
            Some(row) => vec![Effect::ScrollTo { side, row }],
            None => Vec::new(),
        }
    }

    /// The icons of pane `side` one size bigger or smaller ([`ViewMode::bigger`],
    /// [`ViewMode::smaller`]): Ctrl+wheel (macOS ⌘+wheel) and a trackpad's pinch (COMP-04).
    /// Nothing at the end of the steps, and in the thumbnail view, whose size the controller
    /// changes in between them ([`AppState::set_thumb_size`]).
    pub fn step_icon_size(&mut self, side: usize, bigger: bool) -> Vec<Effect> {
        let mode = self.tab(side).view().mode;
        let next = if bigger {
            mode.bigger()
        } else {
            mode.smaller()
        };
        match next {
            Some(next) => self.set_view_mode(side, next),
            None => Vec::new(),
        }
    }

    /// Whether the icon views show the files' thumbnails (보기 ▸ 설정 ▸ 썸네일 보이기).
    pub fn show_thumbnails(&self) -> bool {
        self.show_thumbnails
    }

    pub fn set_show_thumbnails(&mut self, on: bool) {
        self.show_thumbnails = on;
    }

    /// Shows or hides `column` in pane `side` (the head's menu, the 보기 menu's 열). A column
    /// the folder does not offer ([`Column::offered`]: the trash's elsewhere, a folder's in the
    /// trash) does not show there.
    pub fn toggle_column(&mut self, side: usize, column: Column) -> Vec<Effect> {
        if !Column::offered(self.tab(side).location()).contains(&column) {
            return Vec::new();
        }
        self.change_view(side, |view| {
            view.toggle_column(column);
            Chosen::default()
        })
        .into_iter()
        .collect()
    }

    /// Moves the column at `from` among those after the name to `slot` in pane `side` (a head
    /// dragged along the header).
    pub fn move_column(&mut self, side: usize, from: usize, slot: usize) -> Vec<Effect> {
        self.change_view(side, |view| {
            view.move_column(from, slot);
            Chosen::default()
        })
        .into_iter()
        .collect()
    }

    /// Makes `column` of pane `side` `width` logical pixels wide (its border dragged, or a
    /// double click on it).
    pub fn set_column_width(&mut self, side: usize, column: Column, width: u32) -> Vec<Effect> {
        self.change_view(side, |view| {
            view.set_width(column, width);
            Chosen::default()
        })
        .into_iter()
        .collect()
    }

    /// 모든 열 너비 맞추기 in pane `side`: in 자세히 the screen fits every column to its texts
    /// ([`Effect::FitColumns`]); the other views have no columns.
    pub fn fit_columns(&mut self, side: usize) -> Vec<Effect> {
        self.activate(side);
        if self.tab(side).view().mode == ViewMode::Details {
            vec![Effect::FitColumns(side)]
        } else {
            Vec::new()
        }
    }

    /// A click on a head of pane `side` that sorts by `key` (`then` with Shift: the second
    /// key), as [`FolderView::sort_by`] says.
    pub fn sort_by(&mut self, side: usize, key: SortKey, then: bool) -> Vec<Effect> {
        self.change_view(side, |view| {
            // The first key's head flips the first key; a plain click on another one sorts by
            // that key alone; with Shift, it is the second key.
            let first = view.sort.primary.key == key;
            view.sort_by(key, then);
            Chosen {
                sort: first || !then,
                then: !first,
                ..Chosen::default()
            }
        })
        .into_iter()
        .collect()
    }

    /// The 보기 menu's 2차 정렬 for pane `side`: `key`, or none.
    pub fn sort_then(&mut self, side: usize, key: Option<SortKey>) -> Vec<Effect> {
        self.change_view(side, |view| {
            view.set_then(key);
            Chosen {
                then: true,
                ..Chosen::default()
            }
        })
        .into_iter()
        .collect()
    }

    /// Folders first, or mixed in with the files (NAV-08), in pane `side`.
    pub fn toggle_folders_first(&mut self, side: usize) -> Vec<Effect> {
        self.change_view(side, |view| {
            view.sort.folders_first = !view.sort.folders_first;
            Chosen::default()
        })
        .into_iter()
        .collect()
    }

    /// 하위 폴더에 적용: the view of pane `side` is the view of its folder and of every folder
    /// below it ([`ViewStore::apply_to_subfolders`]); a notice says so. The tabs that show a
    /// folder below it now take it when they go to a folder next.
    ///
    /// When folders below have views of their own, which it would forget, a confirmation asks
    /// first and says how many ([`Confirm::apply_view_below`]).
    ///
    /// [`Confirm::apply_view_below`]: crate::dialogs::Confirm::apply_view_below
    pub fn apply_view_to_subfolders(&mut self, side: usize) -> Vec<Effect> {
        self.activate(side);
        let tab = self.panes[side].active_tab();
        let (folder, view) = (tab.location().clone(), tab.view().clone());
        let below = self.views.count_below(&folder);
        if below > 0 && self.views.keeps(&folder) {
            let name = folder_name(&folder);
            self.open(Modal::Confirm(Confirm::apply_view_below(
                folder, view, &name, below,
            )));
            return Vec::new();
        }
        self.apply_view_below(&folder, view)
    }

    /// 하위 폴더에 적용, without asking (asked, or nothing to ask about): `view` for `folder` and
    /// the folders below it.
    pub(super) fn apply_view_below(&mut self, folder: &VPath, view: FolderView) -> Vec<Effect> {
        if !self.views.apply_to_subfolders(folder, view) {
            return vec![Effect::Notice(words::view_not_kept().to_owned())];
        }
        vec![Effect::Notice(words::view_applied_below(&folder_name(
            folder,
        )))]
    }

    /// 기본값으로 되돌리기: the folder of pane `side` forgets its own view
    /// ([`ViewStore::reset`]) and shows what it takes from above, or the default view.
    pub fn reset_view(&mut self, side: usize) -> Vec<Effect> {
        self.activate(side);
        let folder = self.tab(side).location().clone();
        self.views.reset(&folder);
        let tab = self.tab_mut(side);
        tab.unsettle_view();
        let mut effects = Vec::new();
        self.settle_views(&mut effects);
        effects.push(Effect::Notice(words::view_reset(&folder_name(&folder))));
        effects
    }

    /// A key that moves the cursor of pane `side` by `step`, the view laying its rows out as
    /// `paging` says: Shift selects the range from the anchor to where it goes, Ctrl moves the
    /// cursor alone, and with neither the row it goes to is selected alone.
    pub(super) fn step_cursor(
        &mut self,
        side: usize,
        step: KeyMove,
        pick: Pick,
        paging: Paging,
    ) -> Vec<Effect> {
        let tab = self.tab_mut(side);
        let mode = tab.view().mode;
        // ← and → in 자세히 scroll it sideways, as far as its columns reach.
        if mode == ViewMode::Details && matches!(step, KeyMove::Left | KeyMove::Right) {
            let right = step == KeyMove::Right;
            return vec![Effect::ScrollSideways { side, right }];
        }
        let (selection, rows) = tab.selection_mut();
        let Some(to) = target(mode, selection.cursor(), rows.len(), step, paging) else {
            return Vec::new();
        };
        selection.go_to(to, pick, rows);
        vec![Effect::ScrollTo { side, row: to }]
    }
}

/// The name a notice gives `folder`: its last part, or the whole path at a root.
fn folder_name(folder: &VPath) -> String {
    folder.file_name().map_or_else(
        || folder.to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRID: Paging = Paging { line: 4, page: 2 };

    #[test]
    fn icons_move_along_rows_and_down_columns() {
        let at = |cursor, step| target(ViewMode::MediumIcons, Some(cursor), 10, step, GRID);
        // Rows of four: 0 1 2 3 / 4 5 6 7 / 8 9.
        assert_eq!(at(1, KeyMove::Right), Some(2));
        assert_eq!(
            at(3, KeyMove::Right),
            Some(4),
            "from a row's end to the next row"
        );
        assert_eq!(at(9, KeyMove::Right), Some(9));
        assert_eq!(at(4, KeyMove::Left), Some(3));
        assert_eq!(at(0, KeyMove::Left), Some(0));
        assert_eq!(at(1, KeyMove::Down), Some(5));
        assert_eq!(at(5, KeyMove::Down), Some(9));
        assert_eq!(
            at(6, KeyMove::Down),
            Some(9),
            "the last row is short: its last item"
        );
        assert_eq!(at(9, KeyMove::Down), Some(9));
        assert_eq!(at(6, KeyMove::Up), Some(2));
        assert_eq!(at(2, KeyMove::Up), Some(2), "the first row stays");
        // A page is two rows here, as far as the first and the last row.
        assert_eq!(at(1, KeyMove::PageDown), Some(9));
        assert_eq!(at(9, KeyMove::PageUp), Some(1));
        assert_eq!(at(5, KeyMove::PageUp), Some(1));
        assert_eq!(at(5, KeyMove::Home), Some(0));
        assert_eq!(at(5, KeyMove::End), Some(9));
        // Without a cursor every step starts at the first item, End at the last.
        let fresh = |step| target(ViewMode::LargeIcons, None, 10, step, GRID);
        assert_eq!(fresh(KeyMove::Down), Some(0));
        assert_eq!(fresh(KeyMove::Right), Some(0));
        assert_eq!(fresh(KeyMove::End), Some(9));
        assert_eq!(
            target(ViewMode::SmallIcons, Some(0), 0, KeyMove::Down, GRID),
            None
        );
    }

    #[test]
    fn the_list_moves_down_columns_and_across_them() {
        // Columns of four: 0 4 8 / 1 5 9 / 2 6 / 3 7.
        let at = |cursor, step| target(ViewMode::List, Some(cursor), 10, step, GRID);
        assert_eq!(at(1, KeyMove::Down), Some(2));
        assert_eq!(
            at(3, KeyMove::Down),
            Some(4),
            "from a column's end to the next one"
        );
        assert_eq!(at(4, KeyMove::Up), Some(3));
        assert_eq!(at(1, KeyMove::Right), Some(5));
        assert_eq!(
            at(6, KeyMove::Right),
            Some(9),
            "the last column is short: its last item"
        );
        assert_eq!(at(9, KeyMove::Right), Some(9));
        assert_eq!(at(5, KeyMove::Left), Some(1));
        assert_eq!(at(1, KeyMove::Left), Some(1), "the first column stays");
        assert_eq!(at(1, KeyMove::PageDown), Some(9));
        assert_eq!(at(9, KeyMove::PageUp), Some(1));
    }

    #[test]
    fn details_move_by_rows_and_not_sideways() {
        let rows = Paging::rows(3);
        let at = |cursor, step| target(ViewMode::Details, cursor, 10, step, rows);
        assert_eq!(at(Some(4), KeyMove::Down), Some(5));
        assert_eq!(at(Some(4), KeyMove::Up), Some(3));
        assert_eq!(at(Some(4), KeyMove::Left), None);
        assert_eq!(at(Some(4), KeyMove::Right), None);
        assert_eq!(at(Some(4), KeyMove::PageDown), Some(7));
        assert_eq!(at(Some(8), KeyMove::PageDown), Some(9));
        assert_eq!(at(Some(1), KeyMove::PageUp), Some(0));
        assert_eq!(at(None, KeyMove::Down), Some(0));
        assert_eq!(at(None, KeyMove::Up), Some(0));
        assert_eq!(at(None, KeyMove::PageDown), Some(2));
        assert_eq!(at(None, KeyMove::End), Some(9));
    }

    #[test]
    fn columns_show_move_and_take_their_widths() {
        let mut view = FolderView::default();
        let shown = |view: &FolderView| -> Vec<Column> {
            view.columns.iter().map(|shown| shown.column).collect()
        };
        assert_eq!(shown(&view), [Column::Size, Column::Modified, Column::Kind]);
        assert!(view.shows(Column::Name));
        view.toggle_column(Column::Created);
        view.toggle_column(Column::Modified);
        view.toggle_column(Column::Name);
        assert_eq!(shown(&view), [Column::Size, Column::Kind, Column::Created]);
        assert!(!view.shows(Column::Modified));
        // The first one goes after the last; a move that leaves it in place is none.
        assert!(view.move_column(0, 3));
        assert_eq!(shown(&view), [Column::Kind, Column::Created, Column::Size]);
        assert!(!view.move_column(1, 1) && !view.move_column(1, 2));
        assert!(!view.move_column(5, 0));
        assert!(view.move_column(2, 0));
        assert_eq!(shown(&view), [Column::Size, Column::Kind, Column::Created]);
        view.set_width(Column::Kind, 5);
        view.set_width(Column::Created, 10_000);
        view.set_width(Column::Size, 90);
        let widths: Vec<u32> = view.columns.iter().map(|shown| shown.width).collect();
        assert_eq!(widths, [90, MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH]);
        // A column shown again starts at its default width.
        view.toggle_column(Column::Kind);
        view.toggle_column(Column::Kind);
        assert_eq!(view.columns[2], ShownColumn::new(Column::Kind));
    }

    #[test]
    fn heads_sort_by_one_key_then_another() {
        let mut view = FolderView::default();
        let by = |key, order| SortBy { key, order };
        view.sort_by(SortKey::Size, false);
        assert_eq!(view.sort.primary, by(SortKey::Size, SortOrder::Ascending));
        // Shift: the second key, ascending, then the other way round.
        view.sort_by(SortKey::Name, true);
        assert_eq!(
            view.sort.secondary,
            Some(by(SortKey::Name, SortOrder::Ascending))
        );
        view.sort_by(SortKey::Name, true);
        assert_eq!(
            view.sort.secondary,
            Some(by(SortKey::Name, SortOrder::Descending))
        );
        // The first key's head flips the first key, with Shift too, and keeps the second.
        view.sort_by(SortKey::Size, true);
        assert_eq!(view.sort.primary, by(SortKey::Size, SortOrder::Descending));
        assert!(view.sort.secondary.is_some());
        // A plain click on another head sorts by it alone.
        view.sort_by(SortKey::Kind, false);
        assert_eq!(view.sort.primary, by(SortKey::Kind, SortOrder::Ascending));
        assert_eq!(view.sort.secondary, None);
        // The menu's second key: not the first key.
        view.set_then(Some(SortKey::Kind));
        assert_eq!(view.sort.secondary, None);
        view.set_then(Some(SortKey::Created));
        assert_eq!(
            view.sort.secondary,
            Some(by(SortKey::Created, SortOrder::Ascending))
        );
        view.set_then(None);
        assert_eq!(view.sort.secondary, None);
        assert!(view.sort.folders_first);
    }

    #[test]
    fn the_icon_sizes_step_up_and_down() {
        use ViewMode::{Details, ExtraLargeIcons, LargeIcons, List, MediumIcons, SmallIcons};
        assert_eq!(ExtraLargeIcons.bigger(), None, "not into the thumbnails");
        assert_eq!(SmallIcons.bigger(), Some(MediumIcons));
        assert_eq!(MediumIcons.bigger(), Some(LargeIcons));
        assert_eq!(LargeIcons.bigger(), Some(ExtraLargeIcons));
        assert_eq!(ExtraLargeIcons.bigger(), None, "the largest stays");
        assert_eq!(ExtraLargeIcons.smaller(), Some(LargeIcons));
        assert_eq!(MediumIcons.smaller(), Some(SmallIcons));
        assert_eq!(SmallIcons.smaller(), None, "the smallest icons stay");
        // 자세히 and 목록 grow into the small icons, and shrink no further.
        assert_eq!(Details.bigger(), Some(SmallIcons));
        assert_eq!(List.bigger(), Some(SmallIcons));
        assert_eq!(Details.smaller(), None);
        assert_eq!(List.smaller(), None);
    }

    #[test]
    fn thumbnails_show_in_the_medium_icons_and_larger() {
        assert_eq!(ViewMode::MediumIcons.thumb_side(), Some(48));
        assert_eq!(ViewMode::LargeIcons.thumb_side(), Some(96));
        assert_eq!(ViewMode::ExtraLargeIcons.thumb_side(), Some(160));
        assert_eq!(ViewMode::Tiles.thumb_side(), Some(48));
        for mode in [ViewMode::SmallIcons, ViewMode::List, ViewMode::Details] {
            assert_eq!(mode.thumb_side(), None, "{mode:?}");
        }
    }

    #[test]
    fn a_thumbnail_is_asked_for_at_the_step_at_or_above_its_size() {
        let asked = |size| ViewMode::thumbnails(size).thumb_side();
        // From the smallest up to the first step, then each step's sizes.
        for (size, step) in [
            (MIN_THUMB_SIZE, 96),
            (95, 96),
            (96, 96),
            (97, 160),
            (160, 160),
            (161, 256),
            (DEFAULT_THUMB_SIZE, 256),
            (256, 256),
            (257, 512),
            (MAX_THUMB_SIZE, 512),
        ] {
            assert_eq!(asked(size), Some(step), "{size}");
            assert_eq!(thumb_request_side(size), step, "{size}");
        }
        // A drag across one step asks for no other size: every size of a step is one picture.
        let sizes: std::collections::BTreeSet<u32> = (161..=256)
            .filter_map(|size| ViewMode::thumbnails(size).thumb_side())
            .collect();
        assert_eq!(sizes.into_iter().collect::<Vec<_>>(), [256]);
        // Past the steps (which the slider cannot reach), the largest.
        assert_eq!(thumb_request_side(4000), MAX_THUMB_SIZE);
        assert_eq!(THUMB_STEPS.last(), Some(&MAX_THUMB_SIZE));
    }

    #[test]
    fn the_thumbnail_size_stays_between_the_slider_s_ends() {
        assert_eq!(ViewMode::thumbnails(0).thumb_size(), Some(MIN_THUMB_SIZE));
        assert_eq!(ViewMode::thumbnails(63).thumb_size(), Some(MIN_THUMB_SIZE));
        assert_eq!(ViewMode::thumbnails(64).thumb_size(), Some(64));
        assert_eq!(ViewMode::thumbnails(300).thumb_size(), Some(300));
        assert_eq!(ViewMode::thumbnails(512).thumb_size(), Some(512));
        assert_eq!(
            ViewMode::thumbnails(u32::MAX).thumb_size(),
            Some(MAX_THUMB_SIZE)
        );
        assert_eq!(ViewMode::Tiles.thumb_size(), None);
        // The size is no step of the icon sizes: Ctrl+wheel changes it in between them.
        assert_eq!(ViewMode::thumbnails(200).bigger(), None);
        assert_eq!(ViewMode::thumbnails(200).smaller(), None);
        // 타일 grows into the small icons, as 자세히 and 목록 do.
        assert_eq!(ViewMode::Tiles.bigger(), Some(ViewMode::SmallIcons));
        assert_eq!(ViewMode::Tiles.smaller(), None);
    }

    #[test]
    fn tiles_and_thumbnails_lie_in_rows_as_wide_as_the_list_holds() {
        let shape = |mode, width, count| grid_shape(mode, width, count);
        // A tile is 260 wide; the list keeps 8 px at each side and 14 for its scroll bar.
        for (width, per_row) in [
            (0, 1),
            (200, 1),
            (289, 1),
            (290, 1),
            (549, 1),
            (550, 2),
            (809, 2),
            (810, 3),
            (1900, 7),
        ] {
            assert_eq!(
                shape(ViewMode::Tiles, width, 10).map(|shape| shape.per_row),
                Some(per_row),
                "{width}"
            );
        }
        assert_eq!(
            shape(ViewMode::Tiles, 810, 10),
            Some(GridShape {
                per_row: 3,
                rows: 4
            })
        );
        assert_eq!(
            shape(ViewMode::Tiles, 810, 9),
            Some(GridShape {
                per_row: 3,
                rows: 3
            })
        );
        assert_eq!(
            shape(ViewMode::Tiles, 400, 0),
            Some(GridShape {
                per_row: 1,
                rows: 0
            })
        );
        // The thumbnails' cell grows with their size: 88 at the smallest, 536 at the largest.
        let thumbs =
            |size, width| shape(ViewMode::thumbnails(size), width, 100).map(|shape| shape.per_row);
        assert_eq!(thumbs(64, 30 + 88 * 4), Some(4));
        assert_eq!(thumbs(64, 30 + 88 * 4 - 1), Some(3));
        assert_eq!(thumbs(512, 800), Some(1));
        assert_eq!(thumbs(512, 30 + 536 * 2), Some(2));
        assert_eq!(thumbs(200, 1000), Some(4));
        // The icons as the screen lays them out (`FileGrid.tile-w`).
        assert_eq!(
            shape(ViewMode::MediumIcons, 30 + 104 * 5, 7),
            Some(GridShape {
                per_row: 5,
                rows: 2
            })
        );
        assert_eq!(shape(ViewMode::Details, 800, 5), None);
        assert_eq!(shape(ViewMode::List, 800, 5), None);
    }

    #[test]
    fn tiles_and_thumbnails_move_as_the_icons_do() {
        for mode in [ViewMode::Tiles, ViewMode::thumbnails(300)] {
            let at = |cursor, step| target(mode, Some(cursor), 10, step, GRID);
            assert_eq!(at(1, KeyMove::Right), Some(2), "{mode:?}");
            assert_eq!(at(3, KeyMove::Right), Some(4), "{mode:?}");
            assert_eq!(at(1, KeyMove::Down), Some(5), "{mode:?}");
            assert_eq!(at(6, KeyMove::Down), Some(9), "{mode:?}");
            assert_eq!(at(6, KeyMove::Up), Some(2), "{mode:?}");
            assert_eq!(at(1, KeyMove::PageDown), Some(9), "{mode:?}");
        }
    }

    #[test]
    fn the_extra_large_icons_have_their_own_name_and_number() {
        assert_eq!(ViewMode::ExtraLargeIcons.name(), "extra-large-icons");
        assert_eq!(ViewMode::ExtraLargeIcons.number(), 5);
        // The numbers the screen knew before stay.
        let numbers: Vec<i32> = [
            ViewMode::Details,
            ViewMode::List,
            ViewMode::SmallIcons,
            ViewMode::MediumIcons,
            ViewMode::LargeIcons,
        ]
        .into_iter()
        .map(ViewMode::number)
        .collect();
        assert_eq!(numbers, [0, 1, 2, 3, 4]);
        assert_eq!(ViewMode::ALL[1], ViewMode::ExtraLargeIcons);
    }

    #[test]
    fn names_and_numbers_round_trip() {
        for mode in ViewMode::ALL {
            assert_eq!(ViewMode::from_name(mode.name()), Some(mode));
            assert_eq!(ViewMode::from_number(mode.number()), Some(mode));
        }
        let numbers: Vec<i32> = ViewMode::ALL.into_iter().map(ViewMode::number).collect();
        assert_eq!(numbers, [6, 5, 4, 3, 2, 1, 0, 7]);
        // The thumbnails of any size have one name and one number; the size goes apart.
        assert_eq!(ViewMode::thumbnails(300).name(), "thumbnails");
        assert_eq!(ViewMode::thumbnails(300).number(), 6);
        assert_eq!(
            ViewMode::from_name("thumbnails"),
            Some(ViewMode::thumbnails(DEFAULT_THUMB_SIZE))
        );
        assert_eq!(ViewMode::from_name("tiles"), Some(ViewMode::Tiles));
        assert_eq!(ViewMode::from_name("gallery"), None);
        assert_eq!(ViewMode::from_number(8), None);
        for column in std::iter::once(Column::Name).chain(Column::OPTIONAL) {
            assert_eq!(Column::from_name(column.name()), Some(column));
            assert_eq!(Column::from_number(column.number()), Some(column));
            assert!(usize::try_from(column.number()).is_ok_and(|n| n < Column::COUNT));
            if let Some(key) = column.sort_key() {
                assert_eq!(Column::of_key(key), column);
            }
        }
        assert_eq!(Column::from_number(11), None);
        // The trash's columns have their own numbers and keys, and are no folder's.
        for column in Column::TRASH {
            assert_eq!(Column::from_name(column.name()), Some(column));
            assert_eq!(Column::from_number(column.number()), Some(column));
        }
        assert_eq!(Column::Original.sort_key(), Some(SortKey::Origin));
        assert_eq!(Column::of_key(SortKey::Deleted), Column::Deleted);
        assert_eq!(Column::from_name("original"), Some(Column::Original));
        assert!(!Column::OPTIONAL.contains(&Column::Deleted));
        assert_eq!(Column::Attributes.sort_key(), None);
        assert_eq!(Column::Path.sort_key(), None);
        assert_eq!(Column::Accessed.sort_key(), Some(SortKey::Accessed));
        assert_eq!(Column::from_name("accessed"), Some(Column::Accessed));
        assert_eq!(Column::from_name("path"), Some(Column::Path));
    }
}
