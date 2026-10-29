//! The keyboard shortcuts of the main window: one table for each system (PRD KEY-01), from
//! which both the window's keys and the shortcuts its menus and tooltips show (KEY-03) come.
//! `docs/ux/keyboard.md` gives the reason for each key.
//!
//! The rules: an action the system's file manager has keeps its key there (File Explorer on
//! Windows, Finder on macOS); an action only a dual-pane file manager has gets a key neither
//! uses; no Ctrl+Alt key on Windows and Linux, where it is `AltGr` on European layouts and
//! types characters. Linux, a system for development only, takes the Windows table.
//!
//! Slint hands a key press to `FocusScope.key-pressed` as `event.text`: the typed text for a
//! printable key (the shifted letter with Shift, `"N"`), or one character of the private-use
//! or control range for a special key, which `char::from(slint::platform::Key::F2)` gives. On
//! macOS Slint reports Command as `control` and Control as `meta`; with Command held, macOS
//! gives the character Shift makes (`">"` for ⇧⌘.) and leaves Option out (`"v"` for ⌥⌘V).

use rf_core::Os;
use slint::platform::Key;

/// The modifier keys held with a key. On macOS `ctrl` is Command, `alt` Option and `meta`
/// Control; elsewhere `meta` is the Windows or Super key.
#[allow(
    clippy::struct_excessive_bools,
    reason = "the four modifiers of Slint's `KeyboardModifiers`, each held or not"
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Mods {
    /// No modifier.
    pub const NONE: Mods = Mods {
        ctrl: false,
        shift: false,
        alt: false,
        meta: false,
    };
}

/// What a key asks the active pane, or the window, to do.
///
/// The cursor commands come with Shift too: the caller extends the selection from the anchor
/// when `mods.shift` is set (`Selection::move_cursor`). Some commands are in the table before
/// the window can do them (`AppState::key` gives `None` for those, and the key goes on to the
/// system); `docs/ux/keyboard.md` says which task brings each.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// The other pane becomes active.
    SwitchPane,
    /// The next tab of the active pane, after the last one the first.
    NextTab,
    /// The tab before, before the first one the last.
    PrevTab,
    NewTab,
    CloseTab,
    /// The tab the active pane closed last comes back.
    ReopenTab,
    /// The tab that shows goes one place left in its tab bar.
    MoveTabLeft,
    /// The tab that shows goes one place right in its tab bar.
    MoveTabRight,
    /// Open the item under the cursor.
    Open,
    /// The parent folder.
    Up,
    Back,
    Forward,
    /// Edit the path as text.
    EditPath,
    /// The toolbar's filter box (이 폴더에서 거르기) takes the keys.
    FocusFilter,
    /// List the folder again.
    Refresh,
    CursorUp,
    CursorDown,
    CursorHome,
    CursorEnd,
    PageUp,
    PageDown,
    /// Select or unselect the row under the cursor.
    ToggleSelect,
    SelectAll,
    /// Select nothing. A modal dialog takes Esc before the window does and closes instead.
    ClearSelection,
    Rename,
    NewFolder,
    /// To the trash.
    Trash,
    /// For good, after a confirmation.
    DeletePermanently,
    /// Pick where to copy to (PICK).
    CopyTo,
    /// Pick where to move to (PICK).
    MoveTo,
    Undo,
    Redo,
    /// The selected items to the system's clipboard, to copy.
    Copy,
    /// The selected items to the system's clipboard, to move.
    Cut,
    /// The clipboard's items into the folder: copied, or moved when they were cut.
    Paste,
    /// The clipboard's items moved into the folder (macOS, ⌥⌘V).
    PasteMove,
    /// The selected items' paths to the clipboard, as text.
    CopyPath,
    Properties,
    /// Show or hide hidden items.
    ToggleHidden,
    /// Open the 보기 menu.
    ViewMenu,
    /// Open the menu of the selected items, as a right click on them does.
    ContextMenu,
    /// Show the item under the cursor in a popup.
    QuickLook,
    /// Show or hide the preview panel.
    TogglePreview,
    /// Show or hide the folder tree.
    ToggleTree,
    /// The sidebar's folder tree takes the keys ([`tree_key`]); shown first if it is hidden.
    FocusTree,
    /// One pane or two.
    ToggleDualPane,
    /// The two panes side by side, or one above the other.
    ToggleSplit,
    /// The two panes trade their tabs.
    SwapPanes,
    /// The other pane shows the active pane's folder.
    SameFolderInOther,
    /// Going into a folder in one pane goes into the folder of the same name in the other.
    SyncNavigation,
    /// Text typed with no Ctrl, Alt or Meta: go to the first row whose name starts with what was
    /// typed (NAV-14). Joining presses that come within a second is the caller's job, and so
    /// is deciding what a lone space does.
    TypeToSelect(String),
}

impl Command {
    /// Whether the command acts on the rows of the active pane: its items, its cursor or its
    /// selection, or the pane with the keyboard. While the sidebar's tree has the keys, such a
    /// key does nothing (the tree's rows are not the pane's); the window's other commands work
    /// from there as from the panes.
    pub fn acts_on_rows(&self) -> bool {
        matches!(
            self,
            Command::SwitchPane
                | Command::Open
                | Command::CursorUp
                | Command::CursorDown
                | Command::CursorHome
                | Command::CursorEnd
                | Command::PageUp
                | Command::PageDown
                | Command::ToggleSelect
                | Command::SelectAll
                | Command::ClearSelection
                | Command::Rename
                | Command::NewFolder
                | Command::Trash
                | Command::DeletePermanently
                | Command::CopyTo
                | Command::MoveTo
                | Command::Copy
                | Command::Cut
                | Command::Paste
                | Command::PasteMove
                | Command::CopyPath
                | Command::Properties
                | Command::ContextMenu
                | Command::QuickLook
                | Command::TypeToSelect(_)
        )
    }
}

/// What a key does while the sidebar's folder tree has the keys (KEY-02): the moves of a tree
/// view, as in the system's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TreeKey {
    /// ↑: the row above.
    Up,
    /// ↓: the row below.
    Down,
    /// →: opens a closed row, or goes to the first folder of an open one.
    Expand,
    /// ←: closes an open row, or goes to the folder above.
    Collapse,
    /// Enter (and the system's key to open): the active pane shows the row's folder.
    Open,
    /// Esc, Tab, Shift+Tab, and the key that gave the tree the keys: back to the active pane.
    Leave,
}

/// What `text`, held with `mods`, does in the sidebar's tree on `os`; `None` for a key that is
/// the window's ([`command_for`]).
pub fn tree_key(text: &str, mods: Mods, os: Os) -> Option<TreeKey> {
    let mut chars = text.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let (c, held) = pressed(c, mods);
        let plain = [
            (Key::UpArrow, TreeKey::Up),
            (Key::DownArrow, TreeKey::Down),
            (Key::RightArrow, TreeKey::Expand),
            (Key::LeftArrow, TreeKey::Collapse),
            (Key::Return, TreeKey::Open),
            (Key::Escape, TreeKey::Leave),
            (Key::Tab, TreeKey::Leave),
        ];
        let found = plain
            .into_iter()
            .find(|&(key, _)| char::from(key) == c)
            .map(|(_, tree)| tree);
        match found {
            Some(tree) if held == Mods::NONE => return Some(tree),
            Some(TreeKey::Leave) if held == SHIFT && c == char::from(Key::Tab) => {
                return Some(TreeKey::Leave);
            }
            _ => {}
        }
    }
    match command_for(text, mods, os)? {
        Command::Open => Some(TreeKey::Open),
        Command::FocusTree => Some(TreeKey::Leave),
        _ => None,
    }
}

/// What a key does while the list of a folder's subfolders shows under a `›` of a path row
/// (NAV-03, KEY-02): the moves of a drop-down list, as in the system's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListKey {
    /// ↑: the row above (from no row, the last one).
    Up,
    /// ↓: the row below (from no row, the first one).
    Down,
    /// Enter (and the system's key to open): the active pane goes to the row's folder.
    Choose,
    /// Esc: the list closes.
    Close,
}

/// What `text`, held with `mods`, does in the list of subfolders on `os`; `None` for a key that
/// is the window's ([`command_for`]), which closes the list and does its own work.
pub fn list_key(text: &str, mods: Mods, os: Os) -> Option<ListKey> {
    let mut chars = text.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let (c, held) = pressed(c, mods);
        let plain = [
            (Key::UpArrow, ListKey::Up),
            (Key::DownArrow, ListKey::Down),
            (Key::Return, ListKey::Choose),
            (Key::Escape, ListKey::Close),
        ];
        let found = plain
            .into_iter()
            .find(|&(key, _)| char::from(key) == c)
            .map(|(_, list)| list);
        if let Some(list) = found
            && held == Mods::NONE
        {
            return Some(list);
        }
    }
    match command_for(text, mods, os)? {
        Command::Open => Some(ListKey::Choose),
        _ => None,
    }
}

/// The key of a shortcut: a special key, or a character as typed without Shift (a lowercase
/// letter, a digit or a punctuation mark of the US layout).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Press {
    Key(Key),
    Char(char),
}

/// A shortcut: a key and exactly the modifiers held with it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chord {
    pub press: Press,
    pub mods: Mods,
}

/// The command for `text`, the text of a Slint key event, held with `mods`, on `os`; `None`
/// for a key that is no shortcut there (an Alt+letter, a Ctrl+letter not listed, a modifier
/// key on its own).
pub fn command_for(text: &str, mods: Mods, os: Os) -> Option<Command> {
    let mut chars = text.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let (c, held) = pressed(c, mods);
        if let Some((_, command)) = bindings(os).find(|(chord, _)| chord.matches(c, held)) {
            return Some(command.clone());
        }
    }
    let typed = !text.is_empty() && text.chars().all(|c| !c.is_control() && !is_private_use(c));
    (typed && !mods.ctrl && !mods.alt && !mods.meta).then(|| Command::TypeToSelect(text.to_owned()))
}

/// The shortcuts of `os`, in the table's order: a command's first one is the one its menu
/// item and tooltip show.
pub fn bindings(os: Os) -> impl Iterator<Item = (Chord, &'static Command)> {
    let table: &'static [(Chord, Command)] = match os {
        Os::MacOs => MACOS,
        Os::Windows | Os::Other => WINDOWS,
    };
    table.iter().map(|(chord, command)| (*chord, command))
}

/// How `os` writes the first shortcut of `command` ("Ctrl+Shift+N", "⇧⌘N"); `None` for a
/// command without one there.
pub fn shortcut(command: &Command, os: Os) -> Option<String> {
    bindings(os)
        .find(|(_, bound)| *bound == command)
        .map(|(chord, _)| label(chord, os))
}

/// How `os` writes `chord`: Windows and Linux spell the modifiers out in the order Ctrl, Alt,
/// Shift ("Ctrl+Shift+N"); macOS puts its symbols in the order ⌃⌥⇧⌘ before the key ("⇧⌘N").
pub fn label(chord: Chord, os: Os) -> String {
    let Mods {
        ctrl,
        shift,
        alt,
        meta,
    } = chord.mods;
    let mac = os == Os::MacOs;
    let key = match chord.press {
        Press::Key(key) => key_name(key, mac).to_owned(),
        Press::Char(c) => c.to_uppercase().collect(),
    };
    if mac {
        let symbols = [(meta, "⌃"), (alt, "⌥"), (shift, "⇧"), (ctrl, "⌘")];
        let mut text: String = symbols
            .into_iter()
            .filter_map(|(held, symbol)| held.then_some(symbol))
            .collect();
        text.push_str(&key);
        text
    } else {
        let names = [
            (meta, "Win"),
            (ctrl, "Ctrl"),
            (alt, "Alt"),
            (shift, "Shift"),
        ];
        let mut parts: Vec<&str> = names
            .into_iter()
            .filter_map(|(held, name)| held.then_some(name))
            .collect();
        parts.push(&key);
        parts.join("+")
    }
}

/// The name of a special key on a Mac (`mac`) or elsewhere.
fn key_name(key: Key, mac: bool) -> &'static str {
    let (other, apple) = match key {
        Key::Tab => ("Tab", "⇥"),
        Key::Return => ("Enter", "↩"),
        Key::Backspace => ("Backspace", "⌫"),
        Key::Delete => ("Delete", "⌦"),
        Key::Escape => ("Esc", "⎋"),
        Key::Space => ("Space", "Space"),
        Key::UpArrow => ("↑", "↑"),
        Key::DownArrow => ("↓", "↓"),
        Key::LeftArrow => ("←", "←"),
        Key::RightArrow => ("→", "→"),
        Key::Home => ("Home", "↖"),
        Key::End => ("End", "↘"),
        Key::PageUp => ("PageUp", "⇞"),
        Key::PageDown => ("PageDown", "⇟"),
        Key::Menu => ("Menu", "Menu"),
        Key::F1 => ("F1", "F1"),
        Key::F2 => ("F2", "F2"),
        Key::F3 => ("F3", "F3"),
        Key::F4 => ("F4", "F4"),
        Key::F5 => ("F5", "F5"),
        Key::F6 => ("F6", "F6"),
        Key::F7 => ("F7", "F7"),
        Key::F8 => ("F8", "F8"),
        Key::F9 => ("F9", "F9"),
        Key::F10 => ("F10", "F10"),
        Key::F11 => ("F11", "F11"),
        Key::F12 => ("F12", "F12"),
        _ => ("?", "?"),
    };
    if mac { apple } else { other }
}

impl Chord {
    /// Whether the key event's character `c`, as [`pressed`] gives it, held with exactly
    /// `mods`, is this shortcut.
    fn matches(&self, c: char, mods: Mods) -> bool {
        let key = match self.press {
            Press::Key(key) => char::from(key),
            Press::Char(key) => key,
        };
        self.mods == mods && key == c
    }
}

/// A key event's one character `c`, held with `mods`, as the tables name it: a letter in
/// lowercase whatever Caps Lock does, a punctuation mark as typed without Shift when Shift is
/// held. Backtab, which some backends send for Shift+Tab, is Tab with Shift.
fn pressed(c: char, mods: Mods) -> (char, Mods) {
    if c == char::from(Key::Backtab) {
        let shifted = Mods {
            shift: true,
            ..mods
        };
        return (char::from(Key::Tab), shifted);
    }
    let c = if mods.shift { unshifted(c) } else { c };
    (c.to_ascii_lowercase(), mods)
}

/// The key a shifted punctuation mark or digit of the US layout is on: `.` for `>`.
fn unshifted(c: char) -> char {
    const SHIFTED: &str = "~!@#$%^&*()_+{}|:\"<>?";
    const PLAIN: &str = "`1234567890-=[]\\;',./";
    SHIFTED
        .chars()
        .position(|shifted| shifted == c)
        .and_then(|i| PLAIN.chars().nth(i))
        .unwrap_or(c)
}

/// Whether `c` is in the Basic Multilingual Plane's private-use area, where Slint puts the
/// arrows, the function keys and the other keys that type nothing.
fn is_private_use(c: char) -> bool {
    ('\u{E000}'..='\u{F8FF}').contains(&c)
}

const NONE: Mods = Mods::NONE;
const SHIFT: Mods = Mods {
    shift: true,
    ..NONE
};
const CTRL: Mods = Mods { ctrl: true, ..NONE };
const CTRL_SHIFT: Mods = Mods {
    ctrl: true,
    shift: true,
    ..NONE
};
const ALT: Mods = Mods { alt: true, ..NONE };
// macOS: Command, Command with Option, and Control.
const CMD: Mods = CTRL;
const CMD_SHIFT: Mods = CTRL_SHIFT;
const CMD_OPT: Mods = Mods {
    ctrl: true,
    alt: true,
    ..NONE
};
const CTL: Mods = Mods { meta: true, ..NONE };
const CTL_CMD: Mods = Mods {
    ctrl: true,
    meta: true,
    ..NONE
};
const CTL_SHIFT: Mods = Mods {
    meta: true,
    shift: true,
    ..NONE
};

const fn k(key: Key, mods: Mods) -> Chord {
    Chord {
        press: Press::Key(key),
        mods,
    }
}

const fn c(c: char, mods: Mods) -> Chord {
    Chord {
        press: Press::Char(c),
        mods,
    }
}

use Command as C;

/// Windows, after File Explorer; and Linux. A command's first key is the one its menu item and
/// tooltip show.
static WINDOWS: &[(Chord, Command)] = &[
    (k(Key::Tab, NONE), C::SwitchPane),
    (k(Key::Tab, SHIFT), C::SwitchPane),
    (k(Key::Tab, CTRL), C::NextTab),
    (k(Key::Tab, CTRL_SHIFT), C::PrevTab),
    (c('t', CTRL), C::NewTab),
    (c('w', CTRL), C::CloseTab),
    (c('t', CTRL_SHIFT), C::ReopenTab),
    // The browsers' keys to move a tab; File Explorer has none.
    (k(Key::PageUp, CTRL_SHIFT), C::MoveTabLeft),
    (k(Key::PageDown, CTRL_SHIFT), C::MoveTabRight),
    (k(Key::Return, NONE), C::Open),
    (k(Key::UpArrow, ALT), C::Up),
    (k(Key::LeftArrow, ALT), C::Back),
    // File Explorer's Back since Windows 7.
    (k(Key::Backspace, NONE), C::Back),
    (k(Key::RightArrow, ALT), C::Forward),
    (c('l', CTRL), C::EditPath),
    (c('d', ALT), C::EditPath),
    (k(Key::F4, NONE), C::EditPath),
    (c('f', CTRL), C::FocusFilter),
    (c('e', CTRL), C::FocusFilter),
    (k(Key::F3, NONE), C::FocusFilter),
    (k(Key::F5, NONE), C::Refresh),
    (c('r', CTRL), C::Refresh),
    (k(Key::UpArrow, NONE), C::CursorUp),
    (k(Key::UpArrow, SHIFT), C::CursorUp),
    (k(Key::DownArrow, NONE), C::CursorDown),
    (k(Key::DownArrow, SHIFT), C::CursorDown),
    (k(Key::Home, NONE), C::CursorHome),
    (k(Key::Home, SHIFT), C::CursorHome),
    (k(Key::End, NONE), C::CursorEnd),
    (k(Key::End, SHIFT), C::CursorEnd),
    (k(Key::PageUp, NONE), C::PageUp),
    (k(Key::PageUp, SHIFT), C::PageUp),
    (k(Key::PageDown, NONE), C::PageDown),
    (k(Key::PageDown, SHIFT), C::PageDown),
    (k(Key::Space, CTRL), C::ToggleSelect),
    (c('a', CTRL), C::SelectAll),
    (k(Key::Escape, NONE), C::ClearSelection),
    (k(Key::F2, NONE), C::Rename),
    (c('n', CTRL_SHIFT), C::NewFolder),
    (k(Key::Delete, NONE), C::Trash),
    (c('d', CTRL), C::Trash),
    (k(Key::Delete, SHIFT), C::DeletePermanently),
    (k(Key::F7, NONE), C::CopyTo),
    (k(Key::F8, NONE), C::MoveTo),
    (c('z', CTRL), C::Undo),
    (c('y', CTRL), C::Redo),
    (c('z', CTRL_SHIFT), C::Redo),
    (c('c', CTRL), C::Copy),
    (c('x', CTRL), C::Cut),
    (c('v', CTRL), C::Paste),
    (c('c', CTRL_SHIFT), C::CopyPath),
    (k(Key::Return, ALT), C::Properties),
    (c('h', CTRL), C::ToggleHidden),
    (k(Key::F10, NONE), C::ViewMenu),
    (k(Key::F10, SHIFT), C::ContextMenu),
    (k(Key::Menu, NONE), C::ContextMenu),
    (c('p', ALT), C::TogglePreview),
    (c('b', CTRL), C::ToggleTree),
    (k(Key::F6, NONE), C::FocusTree),
    (k(Key::F9, NONE), C::SameFolderInOther),
    (k(Key::F9, SHIFT), C::SyncNavigation),
    (k(Key::F12, NONE), C::ToggleDualPane),
    (k(Key::F12, SHIFT), C::ToggleSplit),
    (c('u', CTRL), C::SwapPanes),
];

/// macOS, after Finder. A command's first key is the one its menu item and tooltip show.
static MACOS: &[(Chord, Command)] = &[
    (k(Key::Tab, NONE), C::SwitchPane),
    (k(Key::Tab, SHIFT), C::SwitchPane),
    (k(Key::Tab, CTL), C::NextTab),
    (c(']', CMD_SHIFT), C::NextTab),
    (k(Key::Tab, CTL_SHIFT), C::PrevTab),
    (c('[', CMD_SHIFT), C::PrevTab),
    (c('t', CMD), C::NewTab),
    (c('w', CMD), C::CloseTab),
    (c('t', CMD_SHIFT), C::ReopenTab),
    // Finder has no keys to move a tab. The browsers' ⌃⇧⇞ and ⌃⇧⇟ are fn⌃⇧↑ and fn⌃⇧↓ on a
    // Mac keyboard, which macOS 15 takes to arrange windows; ⇧⌘← and ⇧⌘→ are free, on every
    // layout, and a text field that has the keys keeps them for selecting.
    (k(Key::LeftArrow, CMD_SHIFT), C::MoveTabLeft),
    (k(Key::RightArrow, CMD_SHIFT), C::MoveTabRight),
    (k(Key::DownArrow, CMD), C::Open),
    (c('o', CMD), C::Open),
    (k(Key::UpArrow, CMD), C::Up),
    (c('[', CMD), C::Back),
    (c(']', CMD), C::Forward),
    // The browsers' Back and Forward too: arrows, which every keyboard layout has where they
    // are. A text field that has the keys keeps them for its own line start and end.
    (k(Key::LeftArrow, CMD), C::Back),
    (k(Key::RightArrow, CMD), C::Forward),
    (c('g', CMD_SHIFT), C::EditPath),
    (c('f', CMD), C::FocusFilter),
    (c('r', CMD), C::Refresh),
    (k(Key::UpArrow, NONE), C::CursorUp),
    (k(Key::UpArrow, SHIFT), C::CursorUp),
    (k(Key::DownArrow, NONE), C::CursorDown),
    (k(Key::DownArrow, SHIFT), C::CursorDown),
    (k(Key::Home, NONE), C::CursorHome),
    (k(Key::Home, SHIFT), C::CursorHome),
    (k(Key::End, NONE), C::CursorEnd),
    (k(Key::End, SHIFT), C::CursorEnd),
    (k(Key::PageUp, NONE), C::PageUp),
    (k(Key::PageUp, SHIFT), C::PageUp),
    (k(Key::PageDown, NONE), C::PageDown),
    (k(Key::PageDown, SHIFT), C::PageDown),
    (c('a', CMD), C::SelectAll),
    (k(Key::Escape, NONE), C::ClearSelection),
    (c('a', CMD_OPT), C::ClearSelection),
    (k(Key::Return, NONE), C::Rename),
    (c('n', CMD_SHIFT), C::NewFolder),
    // The Mac's delete key is Backspace; fn+delete is Delete.
    (k(Key::Backspace, CMD), C::Trash),
    (k(Key::Delete, CMD), C::Trash),
    (k(Key::Backspace, CMD_OPT), C::DeletePermanently),
    (k(Key::Delete, CMD_OPT), C::DeletePermanently),
    (k(Key::F7, NONE), C::CopyTo),
    (k(Key::F8, NONE), C::MoveTo),
    (c('z', CMD), C::Undo),
    (c('z', CMD_SHIFT), C::Redo),
    (c('c', CMD), C::Copy),
    (c('v', CMD), C::Paste),
    (c('v', CMD_OPT), C::PasteMove),
    (c('c', CMD_OPT), C::CopyPath),
    (c('i', CMD), C::Properties),
    (c('.', CMD_SHIFT), C::ToggleHidden),
    (c('j', CMD), C::ViewMenu),
    (c('y', CMD), C::QuickLook),
    (c('p', CMD_SHIFT), C::TogglePreview),
    // AppKit's Show/Hide Sidebar, then Finder's own.
    (c('s', CTL_CMD), C::ToggleTree),
    (c('s', CMD_OPT), C::ToggleTree),
    (k(Key::F6, NONE), C::FocusTree),
    (k(Key::F9, NONE), C::SameFolderInOther),
    (k(Key::F9, SHIFT), C::SyncNavigation),
    (k(Key::F12, NONE), C::ToggleDualPane),
    (k(Key::F12, SHIFT), C::ToggleSplit),
    (c('u', CMD), C::SwapPanes),
];

#[cfg(test)]
mod tests {
    use super::*;
    use Command as C;

    const NONE: Mods = Mods::NONE;
    const CTRL: Mods = Mods { ctrl: true, ..NONE };
    const SHIFT: Mods = Mods {
        shift: true,
        ..NONE
    };
    const ALT: Mods = Mods { alt: true, ..NONE };
    const META: Mods = Mods { meta: true, ..NONE };
    const CTRL_SHIFT: Mods = Mods {
        ctrl: true,
        shift: true,
        ..NONE
    };
    const CTRL_ALT: Mods = Mods {
        ctrl: true,
        alt: true,
        ..NONE
    };
    // On macOS Slint reports Command as `ctrl`, Option as `alt` and Control as `meta`.
    const CMD: Mods = CTRL;
    const CMD_SHIFT: Mods = CTRL_SHIFT;
    const CMD_OPT: Mods = CTRL_ALT;
    const CTL: Mods = META;
    const CTL_SHIFT: Mods = Mods {
        meta: true,
        shift: true,
        ..NONE
    };
    const CTL_CMD: Mods = Mods { meta: true, ..CMD };

    fn key(k: Key) -> String {
        char::from(k).to_string()
    }

    fn text(t: &str) -> String {
        t.to_owned()
    }

    /// Checks that each key of `cases`, held with its modifiers, gives its command on `os`.
    fn check(os: Os, cases: &[(String, Mods, Command)]) {
        for (text, mods, command) in cases {
            assert_eq!(
                command_for(text, *mods, os).as_ref(),
                Some(command),
                "{os:?} {text:?} {mods:?}"
            );
        }
    }

    /// The cursor keys, alone and with Shift (the caller extends the selection then).
    fn cursor_moves() -> Vec<(String, Mods, Command)> {
        let moves = [
            (Key::UpArrow, C::CursorUp),
            (Key::DownArrow, C::CursorDown),
            (Key::Home, C::CursorHome),
            (Key::End, C::CursorEnd),
            (Key::PageUp, C::PageUp),
            (Key::PageDown, C::PageDown),
        ];
        moves
            .into_iter()
            .flat_map(|(k, command)| [(key(k), NONE, command.clone()), (key(k), SHIFT, command)])
            .collect()
    }

    #[test]
    fn windows_and_linux_follow_explorer() {
        let mut cases = vec![
            (key(Key::Tab), NONE, C::SwitchPane),
            (key(Key::Tab), SHIFT, C::SwitchPane),
            (key(Key::Backtab), SHIFT, C::SwitchPane),
            (key(Key::Backtab), NONE, C::SwitchPane),
            (key(Key::Tab), CTRL, C::NextTab),
            (key(Key::Tab), CTRL_SHIFT, C::PrevTab),
            (key(Key::Backtab), CTRL, C::PrevTab),
            (text("t"), CTRL, C::NewTab),
            (text("w"), CTRL, C::CloseTab),
            (text("T"), CTRL_SHIFT, C::ReopenTab),
            // The browsers' keys to move a tab.
            (key(Key::PageUp), CTRL_SHIFT, C::MoveTabLeft),
            (key(Key::PageDown), CTRL_SHIFT, C::MoveTabRight),
            (key(Key::Return), NONE, C::Open),
            (key(Key::UpArrow), ALT, C::Up),
            (key(Key::Backspace), NONE, C::Back),
            (key(Key::LeftArrow), ALT, C::Back),
            (key(Key::RightArrow), ALT, C::Forward),
            (text("l"), CTRL, C::EditPath),
            (text("d"), ALT, C::EditPath),
            (key(Key::F4), NONE, C::EditPath),
            (text("f"), CTRL, C::FocusFilter),
            (text("e"), CTRL, C::FocusFilter),
            (key(Key::F3), NONE, C::FocusFilter),
            (key(Key::F5), NONE, C::Refresh),
            (text("r"), CTRL, C::Refresh),
            (key(Key::Space), CTRL, C::ToggleSelect),
            (text("a"), CTRL, C::SelectAll),
            (key(Key::Escape), NONE, C::ClearSelection),
            (key(Key::F2), NONE, C::Rename),
            // Slint reports the shifted letter.
            (text("N"), CTRL_SHIFT, C::NewFolder),
            (key(Key::Delete), NONE, C::Trash),
            (text("d"), CTRL, C::Trash),
            (key(Key::Delete), SHIFT, C::DeletePermanently),
            (key(Key::F7), NONE, C::CopyTo),
            (key(Key::F8), NONE, C::MoveTo),
            (text("z"), CTRL, C::Undo),
            (text("y"), CTRL, C::Redo),
            (text("Z"), CTRL_SHIFT, C::Redo),
            (text("c"), CTRL, C::Copy),
            (text("x"), CTRL, C::Cut),
            (text("v"), CTRL, C::Paste),
            (text("C"), CTRL_SHIFT, C::CopyPath),
            (key(Key::Return), ALT, C::Properties),
            (text("h"), CTRL, C::ToggleHidden),
            (key(Key::F10), NONE, C::ViewMenu),
            (key(Key::F10), SHIFT, C::ContextMenu),
            (key(Key::Menu), NONE, C::ContextMenu),
            (text("p"), ALT, C::TogglePreview),
            (text("b"), CTRL, C::ToggleTree),
            (key(Key::F9), NONE, C::SameFolderInOther),
            (key(Key::F9), SHIFT, C::SyncNavigation),
            (key(Key::F12), NONE, C::ToggleDualPane),
            (key(Key::F12), SHIFT, C::ToggleSplit),
            (text("u"), CTRL, C::SwapPanes),
            (key(Key::F6), NONE, C::FocusTree),
        ];
        cases.extend(cursor_moves());
        check(Os::Windows, &cases);
        check(Os::Other, &cases);
    }

    #[test]
    fn macos_follows_finder() {
        let mut cases = vec![
            (key(Key::Tab), NONE, C::SwitchPane),
            (key(Key::Tab), SHIFT, C::SwitchPane),
            (key(Key::Tab), CTL, C::NextTab),
            (key(Key::Tab), CTL_SHIFT, C::PrevTab),
            // ⇧⌘] and ⇧⌘[: macOS reports the shifted character with Command held.
            (text("}"), CMD_SHIFT, C::NextTab),
            (text("]"), CMD_SHIFT, C::NextTab),
            (text("{"), CMD_SHIFT, C::PrevTab),
            (text("t"), CMD, C::NewTab),
            (text("w"), CMD, C::CloseTab),
            (text("T"), CMD_SHIFT, C::ReopenTab),
            (key(Key::LeftArrow), CMD_SHIFT, C::MoveTabLeft),
            (key(Key::RightArrow), CMD_SHIFT, C::MoveTabRight),
            (key(Key::DownArrow), CMD, C::Open),
            (text("o"), CMD, C::Open),
            (key(Key::Return), NONE, C::Rename),
            (key(Key::UpArrow), CMD, C::Up),
            (text("["), CMD, C::Back),
            (text("]"), CMD, C::Forward),
            // The browsers' arrows, which no keyboard layout moves.
            (key(Key::LeftArrow), CMD, C::Back),
            (key(Key::RightArrow), CMD, C::Forward),
            (text("G"), CMD_SHIFT, C::EditPath),
            (text("f"), CMD, C::FocusFilter),
            (text("r"), CMD, C::Refresh),
            (text("a"), CMD, C::SelectAll),
            (key(Key::Escape), NONE, C::ClearSelection),
            (text("a"), CMD_OPT, C::ClearSelection),
            (text("N"), CMD_SHIFT, C::NewFolder),
            // The Mac's delete key is Backspace, its forward delete (fn+delete) Delete.
            (key(Key::Backspace), CMD, C::Trash),
            (key(Key::Delete), CMD, C::Trash),
            (key(Key::Backspace), CMD_OPT, C::DeletePermanently),
            (key(Key::Delete), CMD_OPT, C::DeletePermanently),
            (key(Key::F7), NONE, C::CopyTo),
            (key(Key::F8), NONE, C::MoveTo),
            (text("z"), CMD, C::Undo),
            (text("Z"), CMD_SHIFT, C::Redo),
            (text("c"), CMD, C::Copy),
            (text("v"), CMD, C::Paste),
            (text("v"), CMD_OPT, C::PasteMove),
            (text("c"), CMD_OPT, C::CopyPath),
            (text("i"), CMD, C::Properties),
            (text("."), CMD_SHIFT, C::ToggleHidden),
            (text(">"), CMD_SHIFT, C::ToggleHidden),
            (text("j"), CMD, C::ViewMenu),
            (text("y"), CMD, C::QuickLook),
            (text("P"), CMD_SHIFT, C::TogglePreview),
            (text("s"), CTL_CMD, C::ToggleTree),
            (text("s"), CMD_OPT, C::ToggleTree),
            (key(Key::F9), NONE, C::SameFolderInOther),
            (key(Key::F9), SHIFT, C::SyncNavigation),
            (key(Key::F12), NONE, C::ToggleDualPane),
            (key(Key::F12), SHIFT, C::ToggleSplit),
            (text("u"), CMD, C::SwapPanes),
            (key(Key::F6), NONE, C::FocusTree),
        ];
        cases.extend(cursor_moves());
        check(Os::MacOs, &cases);
    }

    #[test]
    fn each_system_keeps_its_own_meaning_of_the_others_keys() {
        let mac_none = [
            // Finder has none of these.
            (key(Key::F2), NONE),
            (key(Key::F5), NONE),
            (key(Key::Backspace), NONE),
            (key(Key::Delete), NONE),
            (key(Key::Delete), SHIFT),
            (key(Key::LeftArrow), ALT),
            (key(Key::Return), ALT),
            // macOS takes these first: hide the app, Spotlight, the app switcher.
            (text("h"), CMD),
            (key(Key::Space), CMD),
            (key(Key::Tab), CMD),
            // Finder edits the path with ⇧⌘G, not a browser's ⌘L; ⌘D duplicates, ⌘E ejects.
            (text("l"), CMD),
            (text("d"), CMD),
            (text("e"), CMD),
            // ⇧⌘C goes to the computer, ⇧⌘O to the documents.
            (text("C"), CMD_SHIFT),
            (text("O"), CMD_SHIFT),
        ];
        for (text, mods) in mac_none {
            assert_eq!(
                command_for(&text, mods, Os::MacOs),
                None,
                "{text:?} {mods:?}"
            );
        }
        // ⌘Y is Quick Look in Finder, not Redo.
        assert_eq!(command_for("y", CMD, Os::MacOs), Some(C::QuickLook));
        let windows_none = [
            (key(Key::DownArrow), CTRL),
            (key(Key::UpArrow), CTRL),
            // A word to the left or right in text, and nothing in the list.
            (key(Key::LeftArrow), CTRL),
            (key(Key::RightArrow), CTRL),
            (text("["), CTRL),
            (text("o"), CTRL),
            (text("i"), CTRL),
            (text("."), CTRL_SHIFT),
            (text("G"), CTRL_SHIFT),
        ];
        for (text, mods) in windows_none {
            assert_eq!(
                command_for(&text, mods, Os::Windows),
                None,
                "{text:?} {mods:?}"
            );
        }
    }

    #[test]
    fn the_windows_key_belongs_to_the_system() {
        for os in [Os::Windows, Os::Other] {
            for (text, mods) in [
                (text("e"), META),
                (text("a"), META),
                (key(Key::Tab), META),
                (key(Key::UpArrow), META),
                (key(Key::Return), META),
            ] {
                assert_eq!(command_for(&text, mods, os), None, "{os:?} {text:?}");
            }
        }
    }

    /// The commands only a dual-pane file manager has: they get keys no file manager of the
    /// system uses (PRD KEY-01 (2)).
    const DUAL_PANE: [Command; 7] = [
        C::CopyTo,
        C::MoveTo,
        C::SameFolderInOther,
        C::SyncNavigation,
        C::ToggleDualPane,
        C::ToggleSplit,
        C::SwapPanes,
    ];

    fn chord(press: Press, mods: Mods) -> Chord {
        Chord { press, mods }
    }

    fn letter(c: char, mods: Mods) -> Chord {
        chord(Press::Char(c), mods)
    }

    fn named(k: Key, mods: Mods) -> Chord {
        chord(Press::Key(k), mods)
    }

    /// The keys File Explorer gives a meaning, and those Windows keeps for itself.
    fn explorer_keys() -> Vec<Chord> {
        let mut keys = vec![
            named(Key::F1, NONE),
            named(Key::F2, NONE),
            named(Key::F3, NONE),
            named(Key::F4, NONE),
            named(Key::F5, NONE),
            named(Key::F6, NONE),
            named(Key::F6, SHIFT),
            named(Key::F10, NONE),
            named(Key::F10, SHIFT),
            named(Key::F11, NONE),
            named(Key::F4, ALT),
            named(Key::F5, CTRL),
            named(Key::Tab, CTRL),
            named(Key::Tab, CTRL_SHIFT),
            named(Key::Return, NONE),
            named(Key::Return, ALT),
            named(Key::Backspace, NONE),
            named(Key::Delete, NONE),
            named(Key::Delete, SHIFT),
            named(Key::LeftArrow, ALT),
            named(Key::RightArrow, ALT),
            named(Key::UpArrow, ALT),
            named(Key::Space, CTRL),
            letter('d', ALT),
            letter('p', ALT),
            letter(
                'p',
                Mods {
                    alt: true,
                    shift: true,
                    ..NONE
                },
            ),
            letter('n', CTRL_SHIFT),
            letter('e', CTRL_SHIFT),
            letter('c', CTRL_SHIFT),
        ];
        keys.extend("acdeflnrtvwxyz".chars().map(|c| letter(c, CTRL)));
        // The views (large icons to content).
        keys.extend("12345678".chars().map(|c| letter(c, CTRL_SHIFT)));
        keys
    }

    /// The keys Finder gives a meaning, and those macOS keeps for itself.
    fn finder_keys() -> Vec<Chord> {
        let mut keys = vec![
            named(Key::Return, NONE),
            named(Key::Space, NONE),
            named(Key::Space, CMD),
            named(Key::Space, Mods { alt: true, ..CMD }),
            named(Key::Tab, CMD),
            named(Key::Tab, CTL),
            named(Key::Tab, CTL_SHIFT),
            named(Key::F11, NONE),
            named(Key::UpArrow, CMD),
            named(Key::DownArrow, CMD),
            named(Key::UpArrow, CTL),
            named(Key::DownArrow, CTL),
            named(Key::LeftArrow, CTL),
            named(Key::RightArrow, CTL),
            named(Key::Backspace, CMD),
            named(Key::Backspace, CMD_SHIFT),
            named(Key::Backspace, CMD_OPT),
            letter('[', CMD),
            letter(']', CMD),
            letter('[', CMD_SHIFT),
            letter(']', CMD_SHIFT),
            letter('.', CMD_SHIFT),
            letter('\\', CMD_SHIFT),
            letter('/', CMD),
            letter(',', CMD),
            letter('`', CMD),
        ];
        // ⌘ with a letter: new window, duplicate, eject, find, info, view options, connect,
        // show original, open, hide, minimize, quit, Quick Look and the editing keys.
        keys.extend("acdefhijkmnoqrtvwxyz".chars().map(|c| letter(c, CMD)));
        // ⌃⌘: make alias, full screen, lock screen, new folder with the selection, add to the
        // sidebar, and AppKit's show or hide the sidebar.
        keys.extend("afnqst".chars().map(|c| letter(c, CTL_CMD)));
        // ⇧⌘: the folders of the Go menu, new folder, preview, tab bar, log out, screenshots.
        keys.extend("acdfghikmnopqrtuz345".chars().map(|c| letter(c, CMD_SHIFT)));
        // ⌥⌘: hide the others, the Dock, force quit, the inspector, downloads, path bar,
        // sidebar, toolbar, close all, smart folder, slideshow, copy as path, move here.
        keys.extend("cdhilmnpstvwy".chars().map(|c| letter(c, CMD_OPT)));
        keys.extend("1234".chars().map(|c| letter(c, CMD)));
        keys.push(named(Key::Escape, CMD_OPT));
        keys
    }

    /// The keys macOS takes before an app sees them, as Slint would hand them in: Spotlight,
    /// the app and input source switchers, Mission Control and the spaces (⌃ with the arrows),
    /// and the window tiling of macOS 15 (Window ▸ Move & Resize): fn⌃ and fn⌃⇧ with the
    /// arrows, which a Mac keyboard sends as the keys home, end, page up and page down (another
    /// keyboard's fn with an arrow is an arrow), and fn⌃F, fn⌃C, fn⌃R.
    fn macos_reserved_keys() -> Vec<Chord> {
        let mut keys = vec![
            named(Key::Space, CMD),
            named(Key::Space, Mods { alt: true, ..CMD }),
            named(Key::Space, CTL),
            named(Key::Tab, CMD),
            named(Key::Tab, CMD_SHIFT),
            letter('h', CMD),
            letter('h', CMD_OPT),
            letter('m', CMD),
        ];
        let arrows = [
            Key::LeftArrow,
            Key::RightArrow,
            Key::UpArrow,
            Key::DownArrow,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown,
        ];
        for mods in [CTL, CTL_SHIFT] {
            keys.extend(arrows.iter().map(|&arrow| named(arrow, mods)));
        }
        keys.extend("fcr".chars().map(|c| letter(c, CTL)));
        keys
    }

    #[test]
    fn macos_keeps_its_own_keys() {
        let taken = macos_reserved_keys();
        for (chord, command) in bindings(Os::MacOs) {
            assert!(
                !taken.contains(&chord),
                "{command:?}: {} is the system's",
                label(chord, Os::MacOs)
            );
        }
        // The browsers' keys to move a tab are among them on a Mac keyboard.
        assert!(taken.contains(&named(Key::PageUp, CTL_SHIFT)));
    }

    #[test]
    fn dual_pane_keys_keep_clear_of_the_file_managers() {
        for (os, taken) in [
            (Os::Windows, explorer_keys()),
            (Os::Other, explorer_keys()),
            (Os::MacOs, finder_keys()),
        ] {
            for command in &DUAL_PANE {
                let chords: Vec<Chord> = bindings(os)
                    .filter(|(_, bound)| *bound == command)
                    .map(|(chord, _)| chord)
                    .collect();
                assert!(!chords.is_empty(), "{os:?} {command:?} has no key");
                for chord in chords {
                    assert!(
                        !taken.contains(&chord),
                        "{os:?} {command:?}: {} is the system's",
                        label(chord, os)
                    );
                }
            }
        }
    }

    #[test]
    fn windows_and_linux_use_no_ctrl_alt_keys() {
        // Ctrl+Alt is AltGr on European layouts: it types characters there.
        for os in [Os::Windows, Os::Other] {
            for (chord, command) in bindings(os) {
                assert!(
                    !(chord.mods.ctrl && chord.mods.alt),
                    "{os:?} {command:?}: {}",
                    label(chord, os)
                );
            }
        }
    }

    #[test]
    fn each_key_gives_one_command() {
        for os in [Os::Windows, Os::MacOs, Os::Other] {
            let all: Vec<(Chord, &Command)> = bindings(os).collect();
            for (i, (chord, command)) in all.iter().enumerate() {
                for (other, other_command) in &all[i + 1..] {
                    assert_ne!(
                        chord,
                        other,
                        "{os:?}: {} is both {command:?} and {other_command:?}",
                        label(*chord, os)
                    );
                }
            }
        }
    }

    #[test]
    fn linux_uses_the_windows_table() {
        let windows: Vec<(Chord, &Command)> = bindings(Os::Windows).collect();
        let linux: Vec<(Chord, &Command)> = bindings(Os::Other).collect();
        assert_eq!(windows, linux);
    }

    #[test]
    fn labels_as_each_system_writes_them() {
        let cases = [
            (C::NewFolder, Some("Ctrl+Shift+N"), Some("⇧⌘N")),
            (C::Back, Some("Alt+←"), Some("⌘[")),
            (C::Up, Some("Alt+↑"), Some("⌘↑")),
            (C::ToggleTree, Some("Ctrl+B"), Some("⌃⌘S")),
            (C::Open, Some("Enter"), Some("⌘↓")),
            (C::Rename, Some("F2"), Some("↩")),
            (C::Trash, Some("Delete"), Some("⌘⌫")),
            (C::DeletePermanently, Some("Shift+Delete"), Some("⌥⌘⌫")),
            (C::CopyTo, Some("F7"), Some("F7")),
            (C::MoveTo, Some("F8"), Some("F8")),
            (C::SyncNavigation, Some("Shift+F9"), Some("⇧F9")),
            (C::ToggleHidden, Some("Ctrl+H"), Some("⇧⌘.")),
            (C::NextTab, Some("Ctrl+Tab"), Some("⌃⇥")),
            (C::ClearSelection, Some("Esc"), Some("⎋")),
            (C::Properties, Some("Alt+Enter"), Some("⌘I")),
            (C::ToggleSelect, Some("Ctrl+Space"), None),
            (C::PasteMove, None, Some("⌥⌘V")),
            (C::Undo, Some("Ctrl+Z"), Some("⌘Z")),
            (C::ViewMenu, Some("F10"), Some("⌘J")),
            (C::FocusFilter, Some("Ctrl+F"), Some("⌘F")),
            (C::EditPath, Some("Ctrl+L"), Some("⇧⌘G")),
            (C::SwapPanes, Some("Ctrl+U"), Some("⌘U")),
            (C::FocusTree, Some("F6"), Some("F6")),
            // ⌘[ and ⌘] come first; ⌘← and ⌘→ are the other keys.
            (C::Forward, Some("Alt+→"), Some("⌘]")),
            (C::ReopenTab, Some("Ctrl+Shift+T"), Some("⇧⌘T")),
            (C::MoveTabLeft, Some("Ctrl+Shift+PageUp"), Some("⇧⌘←")),
            (C::MoveTabRight, Some("Ctrl+Shift+PageDown"), Some("⇧⌘→")),
            // No key at all.
            (C::TypeToSelect("a".to_owned()), None, None),
        ];
        for (command, windows, mac) in cases {
            assert_eq!(
                shortcut(&command, Os::Windows).as_deref(),
                windows,
                "{command:?}"
            );
            assert_eq!(
                shortcut(&command, Os::Other).as_deref(),
                windows,
                "{command:?}"
            );
            assert_eq!(shortcut(&command, Os::MacOs).as_deref(), mac, "{command:?}");
        }
    }

    #[test]
    fn the_tree_takes_the_arrows_and_gives_the_keys_back() {
        let t = |os: Os, text: &str, mods: Mods| tree_key(text, mods, os);
        for os in [Os::Windows, Os::MacOs, Os::Other] {
            for (k, wanted) in [
                (Key::UpArrow, TreeKey::Up),
                (Key::DownArrow, TreeKey::Down),
                (Key::RightArrow, TreeKey::Expand),
                (Key::LeftArrow, TreeKey::Collapse),
                (Key::Return, TreeKey::Open),
                (Key::Escape, TreeKey::Leave),
                (Key::Tab, TreeKey::Leave),
                // F6, which gave the tree the keys, gives them back.
                (Key::F6, TreeKey::Leave),
            ] {
                assert_eq!(t(os, &key(k), NONE), Some(wanted), "{os:?} {k:?}");
            }
            assert_eq!(t(os, &key(Key::Tab), SHIFT), Some(TreeKey::Leave));
            assert_eq!(t(os, &key(Key::Backtab), SHIFT), Some(TreeKey::Leave));
            // Other keys are the window's.
            for (text, mods) in [
                (text("b"), CTRL),
                (text("a"), NONE),
                (key(Key::UpArrow), SHIFT),
                (key(Key::Delete), NONE),
                (key(Key::F12), NONE),
            ] {
                assert_eq!(t(os, &text, mods), None, "{os:?} {text:?} {mods:?}");
            }
        }
        // The system's own key to open opens too.
        assert_eq!(t(Os::MacOs, &key(Key::DownArrow), CMD), Some(TreeKey::Open));
        assert_eq!(t(Os::MacOs, "o", CMD), Some(TreeKey::Open));
    }

    #[test]
    fn the_list_of_subfolders_takes_the_arrows_enter_and_esc() {
        for os in [Os::Windows, Os::MacOs, Os::Other] {
            for (k, wanted) in [
                (Key::UpArrow, ListKey::Up),
                (Key::DownArrow, ListKey::Down),
                (Key::Return, ListKey::Choose),
                (Key::Escape, ListKey::Close),
            ] {
                assert_eq!(list_key(&key(k), NONE, os), Some(wanted), "{os:?} {k:?}");
            }
            // Other keys are the window's: they close the list and do their own work.
            for (text, mods) in [
                (key(Key::Tab), NONE),
                (key(Key::UpArrow), SHIFT),
                (key(Key::UpArrow), ALT),
                (key(Key::LeftArrow), NONE),
                (key(Key::Home), NONE),
                (text("a"), NONE),
                (text("t"), CTRL),
                (key(Key::F2), NONE),
            ] {
                assert_eq!(list_key(&text, mods, os), None, "{os:?} {text:?} {mods:?}");
            }
        }
        // The system's own key to open chooses too.
        assert_eq!(
            list_key(&key(Key::DownArrow), CMD, Os::MacOs),
            Some(ListKey::Choose)
        );
        assert_eq!(list_key("o", CMD, Os::MacOs), Some(ListKey::Choose));
    }

    #[test]
    fn the_commands_of_the_rows_are_known_apart_from_the_window_s() {
        for command in [
            C::Open,
            C::CursorDown,
            C::SelectAll,
            C::ClearSelection,
            C::Rename,
            C::NewFolder,
            C::Trash,
            C::DeletePermanently,
            C::CopyTo,
            C::MoveTo,
            C::Copy,
            C::Paste,
            C::Properties,
            C::QuickLook,
            C::SwitchPane,
            C::TypeToSelect("a".to_owned()),
        ] {
            assert!(command.acts_on_rows(), "{command:?}");
        }
        for command in [
            C::ToggleTree,
            C::ToggleDualPane,
            C::SwapPanes,
            C::SyncNavigation,
            C::ViewMenu,
            C::FocusFilter,
            C::EditPath,
            C::NewTab,
            C::ReopenTab,
            C::MoveTabLeft,
            C::MoveTabRight,
            C::Back,
            C::Up,
            C::Undo,
            C::ToggleHidden,
        ] {
            assert!(!command.acts_on_rows(), "{command:?}");
        }
    }

    #[test]
    fn letters_ignore_caps_lock() {
        assert_eq!(command_for("A", CTRL, Os::Windows), Some(C::SelectAll));
        assert_eq!(command_for("Z", CTRL, Os::Windows), Some(C::Undo));
        assert_eq!(
            command_for("n", CTRL_SHIFT, Os::Windows),
            Some(C::NewFolder)
        );
        assert_eq!(command_for("A", CMD, Os::MacOs), Some(C::SelectAll));
    }

    #[test]
    fn typing_selects_by_name() {
        let typed = |text: &str| Some(C::TypeToSelect(text.to_owned()));
        for os in [Os::Windows, Os::MacOs, Os::Other] {
            assert_eq!(command_for("a", NONE, os), typed("a"));
            assert_eq!(command_for("A", SHIFT, os), typed("A"));
            assert_eq!(command_for("가", NONE, os), typed("가"));
            assert_eq!(command_for("7", NONE, os), typed("7"));
            assert_eq!(command_for(".", NONE, os), typed("."));
            assert_eq!(command_for(" ", NONE, os), typed(" "));
            assert_eq!(command_for("한글", NONE, os), typed("한글"));
            // Held with a modifier other than Shift, a letter types nothing.
            for mods in [CTRL, ALT, META] {
                assert_eq!(command_for("q", mods, os), None, "{os:?} {mods:?}");
            }
        }
    }

    #[test]
    fn other_keys_do_nothing() {
        let none = [
            (text("q"), CTRL),
            (text("n"), CTRL),
            (text("a"), ALT),
            (text("z"), CTRL_ALT),
            (key(Key::F2), SHIFT),
            (key(Key::F5), CTRL),
            (key(Key::F1), NONE),
            // Explorer goes full screen with F11.
            (key(Key::F6), SHIFT),
            (key(Key::F11), NONE),
            (key(Key::Return), CTRL),
            (key(Key::Backspace), CTRL),
            (key(Key::DownArrow), ALT),
            (key(Key::LeftArrow), NONE),
            (key(Key::UpArrow), CTRL),
            (key(Key::Delete), CTRL),
            (key(Key::Escape), SHIFT),
            (key(Key::Space), CTRL_SHIFT),
            (key(Key::Control), CTRL),
            (key(Key::Shift), SHIFT),
            (key(Key::Insert), NONE),
            (String::new(), NONE),
        ];
        for (text, mods) in none {
            assert_eq!(
                command_for(&text, mods, Os::Windows),
                None,
                "{text:?} {mods:?}"
            );
        }
    }
}
