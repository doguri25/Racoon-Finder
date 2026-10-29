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
//! cursor or clicked ([`Selection::user_cursor`]).

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
    /// the keys to start from.
    cursor_by_user: bool,
}

impl Selection {
    /// The row the keyboard is on, if any.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// The row the keyboard is on when the user put the cursor there (a key, a click): the row
    /// a command acts on with nothing selected. `None` for a cursor the list placed
    /// ([`Selection::place_cursor`]), or one left on another row when its own went.
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
    /// the cursor and the anchor go to it.
    pub fn toggle(&mut self, i: usize, names: &[impl Named]) {
        let Some(row) = names.get(i) else { return };
        if !self.selected.remove(row.name()) {
            self.selected.insert(row.name().to_owned());
        }
        self.put_cursor(i);
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

    /// Selects nothing; the cursor and the anchor stay, so the keyboard goes on from there.
    pub fn clear(&mut self) {
        self.selected.clear();
    }

    /// Swaps the selected and the unselected rows; the cursor and the anchor stay.
    pub fn invert(&mut self, names: &[impl Named]) {
        self.selected = names
            .iter()
            .filter(|row| !self.selected.contains(row.name()))
            .map(|row| row.name().to_owned())
            .collect();
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
        // Taking the selection away leaves it the user's.
        sel.clear();
        assert_eq!(sel.user_cursor(), Some(1));

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
        sel.clear();
        let moved = list(&["x", "a", "b", "c", "d", "e"]);
        sel.reconcile(&names, &moved);
        assert_eq!(sel.user_cursor(), Some(2));
        let without_b = list(&["x", "a", "c", "d", "e"]);
        sel.reconcile(&moved, &without_b);
        assert_eq!((sel.cursor(), sel.user_cursor()), (Some(2), None));
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
