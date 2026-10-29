//! Keyboard shortcuts of the main window.
//!
//! Slint hands a key press to `FocusScope.key-pressed` as `event.text`: the typed text for a
//! printable key (the shifted letter with Shift, `"N"`), or one character of the private-use
//! or control range for a special key, which `char::from(slint::platform::Key::F2)` gives. On
//! macOS Slint reports Command as `control`, so the Ctrl shortcuts below are Command ones there.
//! macOS keeps ⌘H (hide the app) for itself, so there hidden items also toggle with ⌘⇧. (period),
//! as in Finder.

use slint::platform::Key;

/// The modifier keys held with a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// What a key asks the active pane, or the window, to do.
///
/// The cursor commands come with Shift too: the caller extends the selection from the anchor
/// when `mods.shift` is set (`Selection::move_cursor`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// Tab (or Shift+Tab): the other pane becomes active.
    SwitchPane,
    /// Enter: open the item under the cursor.
    Open,
    /// Backspace or Alt+Up: the parent folder.
    Up,
    /// Alt+Left.
    Back,
    /// Alt+Right.
    Forward,
    CursorUp,
    CursorDown,
    CursorHome,
    CursorEnd,
    PageUp,
    PageDown,
    /// Ctrl+Space: select or unselect the row under the cursor.
    ToggleSelect,
    /// Ctrl+A.
    SelectAll,
    /// Esc. A modal dialog takes Esc before the window does and closes instead.
    ClearSelection,
    /// F2.
    Rename,
    /// Delete: to the trash.
    Trash,
    /// Shift+Delete: for good, after a confirmation.
    DeletePermanently,
    /// F5: pick where to copy to.
    CopyTo,
    /// F6: pick where to move to.
    MoveTo,
    /// Ctrl+Shift+N or F7.
    NewFolder,
    /// Ctrl+Z.
    Undo,
    /// Ctrl+Y or Ctrl+Shift+Z.
    Redo,
    /// Ctrl+L: edit the path as text.
    EditPath,
    /// Ctrl+T.
    NewTab,
    /// Ctrl+W.
    CloseTab,
    /// Ctrl+H, and on macOS also ⌘⇧. (Command+Shift+Period): show or hide hidden items.
    ToggleHidden,
    /// Text typed with no Ctrl or Alt: go to the first row whose name starts with what was
    /// typed (NAV-14). Joining presses that come within a second is the caller's job, and so
    /// is deciding what a lone space does.
    TypeToSelect(String),
}

/// The command for `key`, the text of a Slint key event, held with `mods`; `None` for a key
/// that is no shortcut (an Alt+letter, a Ctrl+letter not listed, a modifier key on its own).
pub fn command_for(key: &str, mods: Mods) -> Option<Command> {
    command_on(key, mods, cfg!(target_os = "macos"))
}

/// [`command_for`] on macOS (`mac`) or elsewhere.
fn command_on(key: &str, mods: Mods, mac: bool) -> Option<Command> {
    // Shift+Period types `>` on most layouts; what comes with Command held differs.
    if mac && mods.ctrl && mods.shift && !mods.alt && matches!(key, "." | ">") {
        return Some(Command::ToggleHidden);
    }
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if mods.ctrl && !mods.alt && c.is_ascii_alphabetic() {
            return ctrl_letter(c.to_ascii_lowercase(), mods.shift);
        }
        if let Some(command) = special_key(c, mods) {
            return Some(command);
        }
    }
    let typed = !key.is_empty() && key.chars().all(|c| !c.is_control() && !is_private_use(c));
    (typed && !mods.ctrl && !mods.alt).then(|| Command::TypeToSelect(key.to_owned()))
}

/// The command for Ctrl with the lowercase `letter`.
fn ctrl_letter(letter: char, shift: bool) -> Option<Command> {
    let command = match (letter, shift) {
        ('a', false) => Command::SelectAll,
        ('z', false) => Command::Undo,
        ('y', false) | ('z', true) => Command::Redo,
        ('n', true) => Command::NewFolder,
        ('l', false) => Command::EditPath,
        ('t', false) => Command::NewTab,
        ('w', false) => Command::CloseTab,
        ('h', false) => Command::ToggleHidden,
        _ => return None,
    };
    Some(command)
}

/// The special keys the shortcuts use.
const SPECIAL_KEYS: [Key; 19] = [
    Key::Tab,
    Key::Backtab,
    Key::Return,
    Key::Backspace,
    Key::Escape,
    Key::Delete,
    Key::Space,
    Key::UpArrow,
    Key::DownArrow,
    Key::LeftArrow,
    Key::RightArrow,
    Key::Home,
    Key::End,
    Key::PageUp,
    Key::PageDown,
    Key::F2,
    Key::F5,
    Key::F6,
    Key::F7,
];

/// The command for the special key whose character is `c`.
fn special_key(c: char, mods: Mods) -> Option<Command> {
    let key = SPECIAL_KEYS.into_iter().find(|&key| char::from(key) == c)?;
    let Mods { ctrl, shift, alt } = mods;
    let plain = !ctrl && !shift && !alt;
    // Shift may be held: it extends a cursor move and makes a delete permanent.
    let shift_allowed = !ctrl && !alt;
    let alt_only = alt && !ctrl && !shift;
    let command = match key {
        Key::Tab | Key::Backtab if shift_allowed => Command::SwitchPane,
        Key::Return if plain => Command::Open,
        Key::Backspace if plain => Command::Up,
        Key::UpArrow if alt_only => Command::Up,
        Key::LeftArrow if alt_only => Command::Back,
        Key::RightArrow if alt_only => Command::Forward,
        Key::UpArrow if shift_allowed => Command::CursorUp,
        Key::DownArrow if shift_allowed => Command::CursorDown,
        Key::Home if shift_allowed => Command::CursorHome,
        Key::End if shift_allowed => Command::CursorEnd,
        Key::PageUp if shift_allowed => Command::PageUp,
        Key::PageDown if shift_allowed => Command::PageDown,
        Key::Space if ctrl && !shift && !alt => Command::ToggleSelect,
        Key::Escape if plain => Command::ClearSelection,
        Key::Delete if shift_allowed => {
            if shift {
                Command::DeletePermanently
            } else {
                Command::Trash
            }
        }
        Key::F2 if plain => Command::Rename,
        Key::F5 if plain => Command::CopyTo,
        Key::F6 if plain => Command::MoveTo,
        Key::F7 if plain => Command::NewFolder,
        _ => return None,
    };
    Some(command)
}

/// Whether `c` is in the Basic Multilingual Plane's private-use area, where Slint puts the
/// arrows, the function keys and the other keys that type nothing.
fn is_private_use(c: char) -> bool {
    ('\u{E000}'..='\u{F8FF}').contains(&c)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods {
        ctrl: false,
        shift: false,
        alt: false,
    };
    const CTRL: Mods = Mods { ctrl: true, ..NONE };
    const SHIFT: Mods = Mods {
        shift: true,
        ..NONE
    };
    const ALT: Mods = Mods { alt: true, ..NONE };
    const CTRL_SHIFT: Mods = Mods {
        ctrl: true,
        shift: true,
        alt: false,
    };

    fn key(k: Key) -> String {
        char::from(k).to_string()
    }

    #[test]
    fn the_shortcut_table() {
        use Command as C;

        let table = [
            (key(Key::Tab), NONE, C::SwitchPane),
            (key(Key::Tab), SHIFT, C::SwitchPane),
            (key(Key::Backtab), SHIFT, C::SwitchPane),
            (key(Key::Return), NONE, C::Open),
            (key(Key::Backspace), NONE, C::Up),
            (key(Key::UpArrow), ALT, C::Up),
            (key(Key::LeftArrow), ALT, C::Back),
            (key(Key::RightArrow), ALT, C::Forward),
            (key(Key::UpArrow), NONE, C::CursorUp),
            (key(Key::DownArrow), NONE, C::CursorDown),
            (key(Key::Home), NONE, C::CursorHome),
            (key(Key::End), NONE, C::CursorEnd),
            (key(Key::PageUp), NONE, C::PageUp),
            (key(Key::PageDown), NONE, C::PageDown),
            (key(Key::Space), CTRL, C::ToggleSelect),
            ("a".to_owned(), CTRL, C::SelectAll),
            (key(Key::Escape), NONE, C::ClearSelection),
            (key(Key::F2), NONE, C::Rename),
            (key(Key::Delete), NONE, C::Trash),
            (key(Key::Delete), SHIFT, C::DeletePermanently),
            (key(Key::F5), NONE, C::CopyTo),
            (key(Key::F6), NONE, C::MoveTo),
            // Slint reports the shifted letter.
            ("N".to_owned(), CTRL_SHIFT, C::NewFolder),
            (key(Key::F7), NONE, C::NewFolder),
            ("z".to_owned(), CTRL, C::Undo),
            ("y".to_owned(), CTRL, C::Redo),
            ("Z".to_owned(), CTRL_SHIFT, C::Redo),
            ("l".to_owned(), CTRL, C::EditPath),
            ("t".to_owned(), CTRL, C::NewTab),
            ("w".to_owned(), CTRL, C::CloseTab),
            ("h".to_owned(), CTRL, C::ToggleHidden),
        ];
        for (text, mods, command) in table {
            assert_eq!(command_for(&text, mods), Some(command), "{text:?} {mods:?}");
        }
    }

    #[test]
    fn command_shift_period_shows_hidden_items_on_macos() {
        for key in [".", ">"] {
            assert_eq!(
                command_on(key, CTRL_SHIFT, true),
                Some(Command::ToggleHidden),
                "{key:?}"
            );
            assert_eq!(command_on(key, CTRL_SHIFT, false), None, "{key:?}");
            assert_eq!(command_on(key, CTRL, true), None, "{key:?}");
            assert_eq!(
                command_on(
                    key,
                    Mods {
                        alt: true,
                        ..CTRL_SHIFT
                    },
                    true
                ),
                None,
                "{key:?}"
            );
        }
        // ⌘H stays a shortcut: macOS takes it first, and other systems use it.
        assert_eq!(command_on("h", CTRL, true), Some(Command::ToggleHidden));
        assert_eq!(command_on("h", CTRL, false), Some(Command::ToggleHidden));
        // Typed alone, a period still selects by name.
        assert_eq!(
            command_on(".", NONE, true),
            Some(Command::TypeToSelect(".".to_owned()))
        );
    }

    #[test]
    fn shift_moves_keep_the_cursor_command() {
        // The caller extends the selection when `mods.shift` is set.
        let moves = [
            (Key::UpArrow, Command::CursorUp),
            (Key::DownArrow, Command::CursorDown),
            (Key::Home, Command::CursorHome),
            (Key::End, Command::CursorEnd),
            (Key::PageUp, Command::PageUp),
            (Key::PageDown, Command::PageDown),
        ];
        for (k, command) in moves {
            assert_eq!(command_for(&key(k), SHIFT), Some(command), "{k:?}");
        }
    }

    #[test]
    fn letters_ignore_caps_lock() {
        assert_eq!(command_for("A", CTRL), Some(Command::SelectAll));
        assert_eq!(command_for("Z", CTRL), Some(Command::Undo));
        assert_eq!(command_for("n", CTRL_SHIFT), Some(Command::NewFolder));
    }

    #[test]
    fn typing_selects_by_name() {
        let typed = |text: &str| Some(Command::TypeToSelect(text.to_owned()));
        assert_eq!(command_for("a", NONE), typed("a"));
        assert_eq!(command_for("A", SHIFT), typed("A"));
        assert_eq!(command_for("가", NONE), typed("가"));
        assert_eq!(command_for("7", NONE), typed("7"));
        assert_eq!(command_for(" ", NONE), typed(" "));
        assert_eq!(command_for("한글", NONE), typed("한글"));
    }

    #[test]
    fn other_keys_do_nothing() {
        let none = [
            ("q".to_owned(), CTRL),
            ("n".to_owned(), CTRL),
            ("a".to_owned(), ALT),
            ("z".to_owned(), Mods { alt: true, ..CTRL }),
            (key(Key::F2), SHIFT),
            (key(Key::F5), CTRL),
            (key(Key::F1), NONE),
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
            assert_eq!(command_for(&text, mods), None, "{text:?} {mods:?}");
        }
    }
}
