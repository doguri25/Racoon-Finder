//! The selected items, the cursor and the anchor of a file list.
//!
//! The selection holds names, so it outlives a change of the list: a row that moves keeps its
//! selection. The cursor and the anchor are row positions in the list the caller shows, and
//! every method that moves them takes that list (`names`), in its order. A position past the
//! end of `names` changes nothing.
//!
//! A cursor the list puts on its first row when it shows ([`Selection::place_cursor`]) is
//! there for the keys to start from, as in the system's file managers: the user has not chosen
//! that row, and a command that changes files acts on it only once the user has moved the
//! cursor or clicked ([`Selection::user_cursor`]). Nor does one act on a row the user
//! unselected (Esc, a Ctrl+click), or one that a command of the selection left unselected
//! ([`Selection::invert`], [`Selection::select_where`]): the screen shows it unselected.
//!
//! A [`NamePattern`] chooses rows by their names (패턴으로 선택): wildcards or a regular
//! expression.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};

use rf_core::Entry;

/// A row the selection keys on by its name.
pub trait Named {
    /// The row's name in its folder.
    fn name(&self) -> &OsStr;
}

impl Named for Entry {
    fn name(&self) -> &OsStr {
        &self.name
    }
}

impl Named for OsString {
    fn name(&self) -> &OsStr {
        self
    }
}

/// What a key that moves the cursor does with the row it goes to ([`Selection::go_to`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The row alone is selected: the key alone.
    Select,
    /// The rows from the anchor to it are selected: with Shift.
    Extend,
    /// Only the cursor moves: with Ctrl (Windows and Linux; Finder has no such keys).
    Focus,
}

/// Which rows of a list are selected, and where the keyboard cursor and the range anchor are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    selected: HashSet<OsString>,
    /// Where a Shift range starts: the row last clicked, Ctrl+clicked or moved to without
    /// Shift.
    anchor: Option<usize>,
    /// The row the keyboard is on, drawn with a focus frame whether or not it is selected.
    cursor: Option<usize>,
    /// Whether the user put the cursor on its row (a key, a click), rather than the list for
    /// the keys to start from, and has not unselected it since.
    cursor_by_user: bool,
}

impl Selection {
    /// The row the keyboard is on, if any.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// The row the keyboard is on when the user put the cursor there (a key, a click): the row
    /// a command acts on with nothing selected. `None` for a cursor the list placed
    /// ([`Selection::place_cursor`]), one left on another row when its own went, or one whose
    /// row the user unselected since ([`Selection::clear`], [`Selection::toggle`]).
    pub fn user_cursor(&self) -> Option<usize> {
        self.cursor.filter(|_| self.cursor_by_user)
    }

    /// The row a Shift range starts from, if any.
    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    /// Whether the row called `name` is selected.
    pub fn is_selected(&self, name: &OsStr) -> bool {
        self.selected.contains(name)
    }

    /// How many rows are selected.
    pub fn len(&self) -> usize {
        self.selected.len()
    }

    /// Whether no row is selected.
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    /// A plain click on row `i`: only that row is selected, and the cursor and the anchor
    /// go to it.
    pub fn click(&mut self, i: usize, names: &[impl Named]) {
        let Some(row) = names.get(i) else { return };
        self.selected.clear();
        self.selected.insert(row.name().to_owned());
        self.put_cursor(i);
    }

    /// A Ctrl+click on row `i`: that row flips between selected and not, the others stay, and
    /// the cursor and the anchor go to it. A row it unselects shows unselected: no command acts
    /// on it ([`Selection::user_cursor`]) until the user chooses it again.
    pub fn toggle(&mut self, i: usize, names: &[impl Named]) {
        let Some(row) = names.get(i) else { return };
        let unselected = self.selected.remove(row.name());
        if !unselected {
            self.selected.insert(row.name().to_owned());
        }
        self.put_cursor(i);
        self.cursor_by_user = !unselected;
    }

    /// A Shift+click on row `i`: the rows from the anchor to `i` are selected and no others,
    /// and the cursor goes to `i`. An anchor past the end of `names` (a list that shrank
    /// without [`Selection::reconcile`]) starts the range at the last row. Without an anchor
    /// the range is `i` alone, which becomes the anchor.
    pub fn extend_to(&mut self, i: usize, names: &[impl Named]) {
        if i >= names.len() {
            return;
        }
        let anchor = self.anchor.unwrap_or(i).min(names.len() - 1);
        let (first, last) = (anchor.min(i), anchor.max(i));
        self.selected = names[first..=last]
            .iter()
            .map(|row| row.name().to_owned())
            .collect();
        self.anchor = Some(anchor);
        self.cursor = Some(i);
        self.cursor_by_user = true;
    }

    /// An arrow, Home, End or page key: the cursor moves `delta` rows (negative is up) and
    /// stops at the first and the last row, so `isize::MIN` and `isize::MAX` reach them.
    /// Without a cursor, a move down starts above the first row (Down lands on it) and a move
    /// up lands on the first row. Without `extend` the row moved to is selected alone, like a
    /// click; with it (Shift), the range from the anchor to it is, like a Shift+click.
    pub fn move_cursor(&mut self, delta: isize, extend: bool, names: &[impl Named]) {
        let Some(last) = names.len().checked_sub(1) else {
            return;
        };
        let target = match self.cursor {
            Some(cursor) => cursor.saturating_add_signed(delta),
            None if delta > 0 => delta.unsigned_abs() - 1,
            None => 0,
        }
        .min(last);
        if extend {
            self.extend_to(target, names);
        } else {
            self.click(target, names);
        }
    }

    /// A key that moves the cursor to row `i`: the row alone is selected ([`Pick::Select`]),
    /// the range from the anchor to it is ([`Pick::Extend`], Shift), or only the cursor moves
    /// ([`Pick::Focus`], Ctrl) and the selection and the anchor stay. A cursor moved alone onto
    /// a row that is not selected is on a row the screen shows unselected: no command acts on it
    /// ([`Selection::user_cursor`]) until the user selects it (Ctrl+Space, a click).
    pub fn go_to(&mut self, i: usize, pick: Pick, names: &[impl Named]) {
        let Some(row) = names.get(i) else { return };
        match pick {
            Pick::Select => self.click(i, names),
            Pick::Extend => self.extend_to(i, names),
            Pick::Focus => {
                self.cursor_by_user = self.selected.contains(row.name());
                self.cursor = Some(i);
                if self.anchor.is_none() {
                    self.anchor = Some(i);
                }
            }
        }
    }

    /// Puts the cursor and the anchor on row `i` and leaves the selection as it is: a right
    /// click on a row that is selected already, whose menu acts on the whole selection.
    pub fn set_cursor(&mut self, i: usize, names: &[impl Named]) {
        if i < names.len() {
            self.put_cursor(i);
        }
    }

    /// Puts the cursor and the anchor on row `i` for the keys to start from, the selection as
    /// it is: a list that has just shown. The user has not chosen the row
    /// ([`Selection::user_cursor`]).
    pub fn place_cursor(&mut self, i: usize, names: &[impl Named]) {
        if i < names.len() {
            self.anchor = Some(i);
            self.cursor = Some(i);
            self.cursor_by_user = false;
        }
    }

    /// Puts the cursor and the anchor on row `i`, where the user put them.
    fn put_cursor(&mut self, i: usize) {
        self.anchor = Some(i);
        self.cursor = Some(i);
        self.cursor_by_user = true;
    }

    /// Selects every row; the cursor and the anchor stay.
    pub fn select_all(&mut self, names: &[impl Named]) {
        self.selected = names.iter().map(|row| row.name().to_owned()).collect();
    }

    /// Selects nothing (Esc); the cursor and the anchor stay, so the keyboard goes on from
    /// there, but its row shows unselected: no command acts on it ([`Selection::user_cursor`])
    /// until the user chooses it again, as in the system's file managers.
    pub fn clear(&mut self) {
        self.selected.clear();
        self.cursor_by_user = false;
    }

    /// Swaps the selected and the unselected rows; the cursor and the anchor stay. A cursor
    /// whose row it unselects is on a row the screen shows unselected: no command acts on it
    /// ([`Selection::user_cursor`]) until the user chooses it again.
    pub fn invert(&mut self, names: &[impl Named]) {
        self.selected = names
            .iter()
            .filter(|row| !self.selected.contains(row.name()))
            .map(|row| row.name().to_owned())
            .collect();
        self.keep_user_cursor_on_selected(names);
    }

    /// Selects the rows of `names` that `wanted` picks: in place of the selection, or
    /// (`add`) with it. The cursor and the anchor stay; a cursor whose row ends up unselected
    /// is nobody's ([`Selection::user_cursor`]), as after [`Selection::invert`].
    pub fn select_where<T: Named>(
        &mut self,
        names: &[T],
        add: bool,
        mut wanted: impl FnMut(usize, &T) -> bool,
    ) {
        if !add {
            self.selected.clear();
        }
        for (i, row) in names.iter().enumerate() {
            if wanted(i, row) {
                self.selected.insert(row.name().to_owned());
            }
        }
        self.keep_user_cursor_on_selected(names);
    }

    /// The cursor stays the user's only on a row that is selected: one the selection left is
    /// on a row the screen shows unselected.
    fn keep_user_cursor_on_selected(&mut self, names: &[impl Named]) {
        let on_selected = self
            .cursor
            .and_then(|cursor| names.get(cursor))
            .is_some_and(|row| self.selected.contains(row.name()));
        self.cursor_by_user &= on_selected;
    }

    /// Carries the selection over a change of the list from `old` to `new` (a refresh, a
    /// rename by another app, a new sort). Selected names that are still in `new` stay
    /// selected. The cursor and the anchor stay on their names where those are still there;
    /// a cursor whose row is gone stays at its position, on the nearest row that is left (a
    /// row the user has not chosen), and an anchor whose row is gone goes to the cursor. An
    /// empty `new` drops both.
    pub fn reconcile(&mut self, old: &[impl Named], new: &[impl Named]) {
        let position_in_new = |index: Option<usize>| {
            let name = old.get(index?)?.name();
            new.iter().position(|row| row.name() == name)
        };
        let cursor_follows = position_in_new(self.cursor);
        let anchor_follows = position_in_new(self.anchor);

        self.selected = new
            .iter()
            .filter(|row| self.selected.contains(row.name()))
            .map(|row| row.name().to_owned())
            .collect();
        let last = new.len().checked_sub(1);
        self.cursor_by_user &= cursor_follows.is_some();
        self.cursor = cursor_follows.or_else(|| Some(self.cursor?.min(last?)));
        self.anchor = anchor_follows.or(self.cursor);
    }

    /// The positions of the selected rows in `names`, in list order.
    pub fn indices(&self, names: &[impl Named]) -> Vec<usize> {
        names
            .iter()
            .enumerate()
            .filter(|(_, row)| self.selected.contains(row.name()))
            .map(|(i, _)| i)
            .collect()
    }
}

/// How the text of a [`NamePattern`] is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PatternSyntax {
    /// Wildcards, as the systems' file dialogs take them: `*` stands for any characters, `?`
    /// for one, and `;` goes between patterns (`*.jpg;*.png`). A pattern matches a whole name;
    /// the spaces around it are not part of it.
    #[default]
    Wildcard,
    /// A regular expression (the syntax of the `regex` crate), found anywhere in a name.
    Regex,
}

/// Why a text is no [`NamePattern`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PatternError {
    /// It is longer than [`MAX_PATTERN_CHARS`].
    TooLong,
    /// It is no regular expression.
    Invalid,
    /// It would take more memory to match than [`PATTERN_SIZE_LIMIT`].
    TooComplex,
}

/// The most characters a pattern has. Names are at most 255 characters on the systems' file
/// systems, so a pattern this long already says more than any name needs.
pub const MAX_PATTERN_CHARS: usize = 1_000;

/// The most memory, in bytes, a pattern compiles to, and its lazy DFA may take: the `regex`
/// crate matches in time linear in the name, and this keeps the factor of the pattern small.
pub const PATTERN_SIZE_LIMIT: usize = 1 << 20;

/// A pattern the names of rows are matched against (NAV-15 패턴 선택): wildcards or a regular
/// expression ([`PatternSyntax`]), ignoring case or not. A name that is not Unicode is matched
/// as the screen shows it, its bytes that are no text replaced.
#[derive(Clone, Debug)]
pub struct NamePattern {
    regex: regex::Regex,
}

impl PartialEq for NamePattern {
    fn eq(&self, other: &NamePattern) -> bool {
        // The flags are written into the text.
        self.regex.as_str() == other.regex.as_str()
    }
}

impl Eq for NamePattern {}

impl NamePattern {
    /// The pattern `text` writes in `syntax`, ignoring case when `ignore_case`; `Ok(None)` for
    /// one with nothing to match (an empty text, or wildcards of nothing but `;` and spaces).
    pub fn new(
        text: &str,
        syntax: PatternSyntax,
        ignore_case: bool,
    ) -> Result<Option<NamePattern>, PatternError> {
        if text.chars().count() > MAX_PATTERN_CHARS {
            return Err(PatternError::TooLong);
        }
        let body = match syntax {
            PatternSyntax::Wildcard => {
                let parts: Vec<String> = text
                    .split(';')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(wildcard_regex)
                    .collect();
                if parts.is_empty() {
                    return Ok(None);
                }
                format!("^(?:{})$", parts.join("|"))
            }
            PatternSyntax::Regex if text.is_empty() => return Ok(None),
            PatternSyntax::Regex => text.to_owned(),
        };
        // `s`: a `.` and a `*` stand for a line break too, which a name may have on Unix.
        let flags = if ignore_case { "(?is)" } else { "(?s)" };
        let regex = regex::RegexBuilder::new(&format!("{flags}{body}"))
            .size_limit(PATTERN_SIZE_LIMIT)
            .dfa_size_limit(PATTERN_SIZE_LIMIT)
            .build()
            .map_err(|error| match error {
                regex::Error::CompiledTooBig(_) => PatternError::TooComplex,
                _ => PatternError::Invalid,
            })?;
        Ok(Some(NamePattern { regex }))
    }

    /// Whether `name` matches.
    pub fn matches(&self, name: &OsStr) -> bool {
        match name.to_str() {
            Some(text) => self.regex.is_match(text),
            None => self.regex.is_match(&name.to_string_lossy()),
        }
    }
}

/// The regular expression of one wildcard pattern: `*` any characters (a run of them one), `?`
/// one, anything else itself.
fn wildcard_regex(pattern: &str) -> String {
    let mut regex = String::with_capacity(pattern.len() * 2);
    let mut last = None;
    for c in pattern.chars() {
        match c {
            '*' if last == Some('*') => {}
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            _ => regex.push_str(&regex::escape(c.encode_utf8(&mut [0; 4]))),
        }
        last = Some(c);
    }
    regex
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(names: &[&str]) -> Vec<OsString> {
        names.iter().map(OsString::from).collect()
    }

    const ABCDE: [&str; 5] = ["a", "b", "c", "d", "e"];

    #[test]
    fn click_selects_one_row() {
        let names = list(&ABCDE);
        let mut sel = Selection::default();
        assert!(sel.is_empty());
        assert_eq!(sel.cursor(), None);

        sel.click(1, &names);
        sel.click(3, &names);
        assert_eq!(sel.indices(&names), [3]);
        assert_eq!((sel.cursor(), sel.anchor()), (Some(3), Some(3)));
        assert!(sel.is_selected(OsStr::new("d")) && !sel.is_selected(OsStr::new("b")));
        assert_eq!(sel.len(), 1);

        // A row past the end changes nothing.
        sel.click(9, &names);
        assert_eq!(sel.indices(&names), [3]);
        assert_eq!(sel.cursor(), Some(3));
    }

    #[test]
    fn shift_click_selects_range_from_anchor() {
        let names = list(&ABCDE);
        let mut sel = Selection::default();
        sel.click(1, &names);
        sel.extend_to(3, &names);
        assert_eq!(sel.indices(&names), [1, 2, 3]);
        assert_eq!((sel.cursor(), sel.anchor()), (Some(3), Some(1)));

        // The range runs from the same anchor the other way and replaces the last one.
        sel.extend_to(0, &names);
        assert_eq!(sel.indices(&names), [0, 1]);
        assert_eq!((sel.cursor(), sel.anchor()), (Some(0), Some(1)));

        // A Ctrl+click moves the anchor.
        sel.toggle(3, &names);
        sel.extend_to(4, &names);
        assert_eq!(sel.indices(&names), [3, 4]);

        // With no anchor yet, the range is the row itself.
        let mut fresh = Selection::default();
        fresh.extend_to(2, &names);
        assert_eq!(fresh.indices(&names), [2]);
        assert_eq!(fresh.anchor(), Some(2));
    }

    #[test]
    fn an_anchor_past_a_shorter_list_starts_at_its_last_row() {
        let mut sel = Selection::default();
        sel.click(4, &list(&ABCDE));
        // The list shrank without a reconcile: the anchor is at its last row now.
        let shorter = list(&["a", "b", "c"]);
        sel.extend_to(0, &shorter);
        assert_eq!(sel.indices(&shorter), [0, 1, 2]);
        assert_eq!((sel.cursor(), sel.anchor()), (Some(0), Some(2)));
    }

    #[test]
    fn ctrl_click_toggles() {
        let names = list(&ABCDE);
        let mut sel = Selection::default();
        sel.click(0, &names);
        sel.toggle(2, &names);
        assert_eq!(sel.indices(&names), [0, 2]);
        assert_eq!((sel.cursor(), sel.anchor()), (Some(2), Some(2)));
        sel.toggle(0, &names);
        assert_eq!(sel.indices(&names), [2]);
        sel.toggle(2, &names);
        assert!(sel.is_empty());
        assert_eq!(sel.cursor(), Some(2));
        sel.toggle(9, &names);
        assert!(sel.is_empty());
    }

    #[test]
    fn cursor_moves_select_and_extend() {
        let names = list(&ABCDE);
        let mut sel = Selection::default();
        // With no cursor, Down starts at the first row and End goes to the last.
        sel.move_cursor(1, false, &names);
        assert_eq!((sel.cursor(), sel.indices(&names)), (Some(0), vec![0]));
        let mut from_none = Selection::default();
        from_none.move_cursor(isize::MAX, false, &names);
        assert_eq!(from_none.cursor(), Some(4));
        let mut up_from_none = Selection::default();
        up_from_none.move_cursor(-1, false, &names);
        assert_eq!(up_from_none.cursor(), Some(0));

        sel.move_cursor(1, false, &names);
        assert_eq!((sel.cursor(), sel.indices(&names)), (Some(1), vec![1]));
        sel.move_cursor(1, true, &names);
        assert_eq!((sel.cursor(), sel.indices(&names)), (Some(2), vec![1, 2]));
        assert_eq!(sel.anchor(), Some(1));
        sel.move_cursor(isize::MAX, true, &names);
        assert_eq!(
            (sel.cursor(), sel.indices(&names)),
            (Some(4), vec![1, 2, 3, 4])
        );
        sel.move_cursor(isize::MIN, false, &names);
        assert_eq!((sel.cursor(), sel.indices(&names)), (Some(0), vec![0]));
        // The cursor stops at the ends.
        sel.move_cursor(-1, false, &names);
        assert_eq!(sel.cursor(), Some(0));
        sel.move_cursor(20, false, &names);
        assert_eq!(sel.cursor(), Some(4));

        // An empty list has nowhere to go.
        let mut empty = Selection::default();
        empty.move_cursor(1, true, &list(&[]));
        assert_eq!(empty, Selection::default());
    }

    #[test]
    fn reconcile_keeps_names_and_moves_cursor() {
        let old = list(&ABCDE);
        let mut sel = Selection::default();
        sel.click(1, &old);
        sel.toggle(3, &old);
        sel.toggle(2, &old);
        sel.toggle(2, &old); // b and d selected, cursor and anchor on c
        assert_eq!(sel.indices(&old), [1, 3]);
        assert_eq!(sel.cursor(), Some(2));

        // Another app removed c and added x on top.
        let new = list(&["x", "a", "b", "d", "e"]);
        sel.reconcile(&old, &new);
        assert_eq!(sel.indices(&new), [2, 3]);
        // c is gone: the cursor stays at its place, on b now.
        assert_eq!(sel.cursor(), Some(2));
        assert_eq!(sel.anchor(), Some(2));

        // A row that moved keeps the cursor on it.
        let mut sel = Selection::default();
        let old = list(&["a", "b", "c"]);
        sel.click(2, &old);
        let new = list(&["c", "a", "b"]);
        sel.reconcile(&old, &new);
        assert_eq!(
            (sel.cursor(), sel.anchor(), sel.indices(&new)),
            (Some(0), Some(0), vec![0])
        );

        // A shorter list pulls the cursor to its last row; an empty one drops it.
        let mut sel = Selection::default();
        let old = list(&["a", "b", "c", "d"]);
        sel.click(1, &old);
        sel.extend_to(3, &old);
        let new = list(&["a"]);
        sel.reconcile(&old, &new);
        assert_eq!((sel.cursor(), sel.anchor()), (Some(0), Some(0)));
        assert!(sel.is_empty());
        sel.select_all(&new);
        sel.reconcile(&new, &list(&[]));
        assert_eq!(sel, Selection::default());
    }

    #[test]
    fn a_cursor_placed_for_the_keys_is_not_the_users_until_they_move_it() {
        let names = list(&ABCDE);
        // A list just shown: the cursor is on its first row for the keys to start from, and
        // the user has chosen nothing.
        let mut sel = Selection::default();
        sel.place_cursor(0, &names);
        assert_eq!((sel.cursor(), sel.user_cursor()), (Some(0), None));
        assert!(sel.is_empty());
        // A row past the end changes nothing.
        let mut past = Selection::default();
        past.place_cursor(9, &names);
        assert_eq!(past.cursor(), None);

        // An arrow key moves it from there, and from then on it is where the user put it.
        sel.move_cursor(1, false, &names);
        assert_eq!((sel.cursor(), sel.user_cursor()), (Some(1), Some(1)));
        // Taking the selection away (Esc) leaves the cursor there, on a row the screen shows
        // unselected: nothing acts on it.
        sel.clear();
        assert_eq!((sel.cursor(), sel.user_cursor()), (Some(1), None));
        // So does a Ctrl+click that unselects the row, until one selects it again.
        sel.toggle(2, &names);
        assert_eq!(sel.user_cursor(), Some(2));
        sel.toggle(2, &names);
        assert_eq!((sel.cursor(), sel.user_cursor()), (Some(2), None));
        sel.toggle(2, &names);
        assert_eq!(sel.user_cursor(), Some(2));

        // Clicks, Ctrl+clicks, Shift+clicks and right clicks put it there too.
        for put in [
            Selection::click,
            Selection::toggle,
            Selection::extend_to,
            Selection::set_cursor,
        ] {
            let mut sel = Selection::default();
            sel.place_cursor(0, &names);
            put(&mut sel, 2, &names);
            assert_eq!(sel.user_cursor(), Some(2));
        }

        // A list that changes keeps it on the user's row; once that row is gone, the cursor
        // that stays at its place is on a row nobody chose.
        let mut sel = Selection::default();
        sel.click(1, &names);
        let moved = list(&["x", "a", "b", "c", "d", "e"]);
        sel.reconcile(&names, &moved);
        assert_eq!(sel.user_cursor(), Some(2));
        let without_b = list(&["x", "a", "c", "d", "e"]);
        sel.reconcile(&moved, &without_b);
        assert_eq!((sel.cursor(), sel.user_cursor()), (Some(2), None));
    }

    #[test]
    fn a_key_selects_extends_or_moves_the_cursor_alone() {
        let names = list(&ABCDE);
        let mut sel = Selection::default();
        sel.go_to(1, Pick::Select, &names);
        assert_eq!((sel.cursor(), sel.indices(&names)), (Some(1), vec![1]));
        // Ctrl: the cursor moves, the selection and the anchor stay; the row it is on shows
        // unselected, and nothing acts on it.
        sel.go_to(3, Pick::Focus, &names);
        assert_eq!((sel.cursor(), sel.anchor()), (Some(3), Some(1)));
        assert_eq!(sel.indices(&names), [1]);
        assert_eq!(sel.user_cursor(), None);
        // On a selected row it is the user's again.
        sel.go_to(1, Pick::Focus, &names);
        assert_eq!(sel.user_cursor(), Some(1));
        // Shift: the range from the anchor.
        sel.go_to(3, Pick::Focus, &names);
        sel.go_to(4, Pick::Extend, &names);
        assert_eq!(sel.indices(&names), [1, 2, 3, 4]);
        assert_eq!(sel.user_cursor(), Some(4));
        // A cursor placed by the list stays nobody's when Ctrl moves it to a row not selected.
        let mut placed = Selection::default();
        placed.place_cursor(0, &names);
        placed.go_to(2, Pick::Focus, &names);
        assert_eq!((placed.cursor(), placed.user_cursor()), (Some(2), None));
        assert!(placed.is_empty());
        // Past the end, nothing.
        placed.go_to(9, Pick::Select, &names);
        assert_eq!(placed.cursor(), Some(2));
    }

    #[test]
    fn invert_and_all() {
        let names = list(&ABCDE);
        let mut sel = Selection::default();
        sel.select_all(&names);
        assert_eq!(sel.indices(&names), [0, 1, 2, 3, 4]);
        assert_eq!(sel.len(), 5);
        assert_eq!(sel.cursor(), None);
        sel.invert(&names);
        assert!(sel.is_empty());

        sel.click(1, &names);
        sel.toggle(3, &names);
        sel.invert(&names);
        assert_eq!(sel.indices(&names), [0, 2, 4]);
        // Neither moves the cursor.
        assert_eq!(sel.cursor(), Some(3));

        sel.clear();
        assert!(sel.is_empty());
        assert_eq!(sel.cursor(), Some(3));
    }

    #[test]
    fn an_invert_that_unselects_the_cursor_s_row_leaves_it_nobody_s() {
        let names = list(&ABCDE);
        // The user clicked b: the cursor is theirs, on a selected row.
        let mut sel = Selection::default();
        sel.click(1, &names);
        assert_eq!(sel.user_cursor(), Some(1));
        // Inverted, b shows unselected: no command acts on it, the selection is a, c, d, e.
        sel.invert(&names);
        assert_eq!(sel.indices(&names), [0, 2, 3, 4]);
        assert_eq!((sel.cursor(), sel.user_cursor()), (Some(1), None));
        // Inverted back, b is selected again, but a row the invert left nobody's stays so
        // until the user chooses it.
        sel.invert(&names);
        assert_eq!(sel.indices(&names), [1]);
        assert_eq!(sel.user_cursor(), None);
        // An invert that leaves the cursor's row selected keeps it the user's.
        let mut kept = Selection::default();
        kept.click(1, &names);
        kept.toggle(3, &names);
        kept.toggle(1, &names);
        kept.toggle(2, &names);
        assert_eq!(
            (kept.indices(&names), kept.user_cursor()),
            (vec![2, 3], Some(2))
        );
        kept.go_to(0, Pick::Focus, &names);
        kept.invert(&names);
        assert_eq!(kept.indices(&names), [0, 1, 4]);
        assert_eq!(kept.cursor(), Some(0));
        // The cursor moved alone (Ctrl) onto a row that was not selected is nobody's, and
        // selecting it by an invert does not make it the user's choice either.
        assert_eq!(kept.user_cursor(), None);
    }

    #[test]
    fn rows_chosen_by_a_test_replace_the_selection_or_join_it() {
        let names = list(&["a.jpg", "b.png", "c.jpg", "d.txt"]);
        let jpg = |_: usize, row: &OsString| row.to_string_lossy().ends_with(".jpg");
        let mut sel = Selection::default();
        sel.click(1, &names);
        // In place of the selection: b goes, and the cursor on it is nobody's.
        sel.select_where(&names, false, jpg);
        assert_eq!(sel.indices(&names), [0, 2]);
        assert_eq!((sel.cursor(), sel.user_cursor()), (Some(1), None));
        // With it: d joins.
        sel.select_where(&names, true, |i, _| i == 3);
        assert_eq!(sel.indices(&names), [0, 2, 3]);
        // A cursor on a row that stays selected stays the user's.
        let mut kept = Selection::default();
        kept.click(2, &names);
        kept.select_where(&names, false, jpg);
        assert_eq!(
            (kept.indices(&names), kept.user_cursor()),
            (vec![0, 2], Some(2))
        );
        // Nothing chosen, in place: nothing selected.
        kept.select_where(&names, false, |_, _| false);
        assert!(kept.is_empty());
        assert_eq!(kept.user_cursor(), None);
    }

    fn pattern(text: &str, syntax: PatternSyntax, ignore_case: bool) -> NamePattern {
        NamePattern::new(text, syntax, ignore_case)
            .expect("a pattern")
            .expect("something to match")
    }

    fn matching(pattern: &NamePattern, names: &[&str]) -> Vec<String> {
        names
            .iter()
            .filter(|name| pattern.matches(OsStr::new(name)))
            .map(|&name| name.to_owned())
            .collect()
    }

    const PHOTOS: [&str; 7] = [
        "a.jpg",
        "B.JPG",
        "c.png",
        "d.jpg.txt",
        "사진 1.jpeg",
        "x.jp",
        "jpg",
    ];

    #[test]
    fn wildcards_match_whole_names_ignoring_case_by_choice() {
        let both = pattern("*.jpg;*.png", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&both, &PHOTOS), ["a.jpg", "B.JPG", "c.png"]);
        // Spaces around a pattern and empty patterns between `;` are nothing.
        let spaced = pattern(" *.jpg ; ;*.png ", PatternSyntax::Wildcard, true);
        assert_eq!(spaced, both);
        // Case counts when asked to.
        let exact = pattern("*.jpg", PatternSyntax::Wildcard, false);
        assert_eq!(matching(&exact, &PHOTOS), ["a.jpg"]);
        // `?` is one character; anything else is itself, regex characters too.
        let one = pattern("?.jp?", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&one, &PHOTOS), ["a.jpg", "B.JPG"]);
        let words = pattern("사진 ?.jpeg", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&words, &PHOTOS), ["사진 1.jpeg"]);
        let literal = pattern("a+(1).[x]", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&literal, &["a+(1).[x]", "aa(1).x"]), ["a+(1).[x]"]);
        // A pattern without wildcards is a whole name.
        let name = pattern("jpg", PatternSyntax::Wildcard, true);
        assert_eq!(matching(&name, &PHOTOS), ["jpg"]);
        // A run of stars is one.
        assert_eq!(
            pattern("***.png", PatternSyntax::Wildcard, true),
            pattern("*.png", PatternSyntax::Wildcard, true)
        );
        // A name with a line break, which Unix allows.
        let star = pattern("a*", PatternSyntax::Wildcard, true);
        assert!(star.matches(OsStr::new("a\nb")));
    }

    #[test]
    fn a_regular_expression_is_found_anywhere_in_a_name() {
        let jpg = pattern(r"\.jpe?g$", PatternSyntax::Regex, true);
        assert_eq!(matching(&jpg, &PHOTOS), ["a.jpg", "B.JPG", "사진 1.jpeg"]);
        let exact = pattern(r"\.jpe?g$", PatternSyntax::Regex, false);
        assert_eq!(matching(&exact, &PHOTOS), ["a.jpg", "사진 1.jpeg"]);
        let part = pattern("jp", PatternSyntax::Regex, true);
        assert_eq!(matching(&part, &PHOTOS).len(), 6);
        // `;` is the regex's own.
        let semicolon = pattern("a;b", PatternSyntax::Regex, true);
        assert_eq!(matching(&semicolon, &["a;b", "a", "b"]), ["a;b"]);
    }

    #[test]
    fn a_text_that_is_no_pattern_says_why() {
        for syntax in [PatternSyntax::Wildcard, PatternSyntax::Regex] {
            assert_eq!(NamePattern::new("", syntax, true), Ok(None));
            let long = "a".repeat(MAX_PATTERN_CHARS + 1);
            assert_eq!(
                NamePattern::new(&long, syntax, true),
                Err(PatternError::TooLong)
            );
            let longest = "가".repeat(MAX_PATTERN_CHARS);
            assert!(matches!(
                NamePattern::new(&longest, syntax, true),
                Ok(Some(_))
            ));
        }
        assert_eq!(
            NamePattern::new(" ; ", PatternSyntax::Wildcard, true),
            Ok(None)
        );
        for bad in ["(", "[a-", "*.jpg", "a{2,1}", r"\p{Nope}"] {
            assert_eq!(
                NamePattern::new(bad, PatternSyntax::Regex, true),
                Err(PatternError::Invalid),
                "{bad}"
            );
        }
        // Short, but it would compile to more than the limit.
        assert_eq!(
            NamePattern::new(r"\w{200}\w{200}\w{200}", PatternSyntax::Regex, true),
            Err(PatternError::TooComplex)
        );
        // The same text in wildcards is a name.
        assert!(matches!(
            NamePattern::new("(", PatternSyntax::Wildcard, true),
            Ok(Some(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_is_no_text_is_matched_as_it_shows() {
        use std::os::unix::ffi::OsStrExt;
        let name = OsStr::from_bytes(b"photo\xff.jpg");
        assert!(pattern("photo?.jpg", PatternSyntax::Wildcard, true).matches(name));
    }

    #[test]
    fn entries_are_keyed_by_name() {
        use rf_core::EntryKind;

        let entries = [
            Entry::new("문서", EntryKind::Dir),
            Entry::new("a.txt", EntryKind::File),
        ];
        let mut sel = Selection::default();
        sel.click(1, &entries);
        assert!(sel.is_selected(OsStr::new("a.txt")));
        assert_eq!(sel.indices(&entries), [1]);
    }
}
