//! The back and forward lists of a tab, and the jumps of the visit history's menu (NAV-04): a
//! right click or a long press on 뒤로 or 앞으로 lists the folders that way, nearest first
//! ([`History::list`]), and choosing one goes that many steps at once ([`History::go_to`]), as a
//! browser's history menu does: the folders passed over stay in the list behind the jump.

use rf_core::VPath;

/// The most folders the back list keeps; the oldest goes first. The forward list fills only as
/// the back list empties, so it stays within this too.
pub const HISTORY_LIMIT: usize = 50;

/// The most folders the visit history's menu lists in one direction.
pub const HISTORY_MENU: usize = 15;

/// A direction of the history: the folders before the current one, or after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Way {
    Back,
    Forward,
}

/// The folders a tab can go back and forward to. The tab's current folder is in neither list:
/// the tab keeps it and hands it in on each move.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    /// Oldest first; the last one is where Back goes.
    back: Vec<VPath>,
    /// Farthest first; the last one is where Forward goes.
    forward: Vec<VPath>,
}

impl History {
    /// Records that the tab left `from` for another folder it opened: `from` is where Back
    /// goes now, and nothing is ahead any more. A move made with [`History::go_back`] or
    /// [`History::go_forward`] is recorded by those and must not be visited too, and neither
    /// must a reload of the same folder.
    pub fn visit(&mut self, from: VPath) {
        self.forward.clear();
        push_capped(&mut self.back, from);
    }

    /// The folder before `current`, which becomes the first one ahead; `None`, and no change,
    /// when there is none.
    pub fn go_back(&mut self, current: VPath) -> Option<VPath> {
        let previous = self.back.pop()?;
        push_capped(&mut self.forward, current);
        Some(previous)
    }

    /// The folder after `current`, which becomes the last one behind; `None`, and no change,
    /// when there is none.
    pub fn go_forward(&mut self, current: VPath) -> Option<VPath> {
        let next = self.forward.pop()?;
        push_capped(&mut self.back, current);
        Some(next)
    }

    /// Whether [`History::go_back`] has a folder to give.
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// Whether [`History::go_forward`] has a folder to give.
    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// The folders `way` goes to, the nearest first: where one step goes, then two steps, and
    /// so on to the end of that list.
    pub fn list(&self, way: Way) -> impl Iterator<Item = &VPath> {
        self.side(way).iter().rev()
    }

    /// The folder `index` places along [`History::list`] of `way` from `current` (0 is where one
    /// step goes): the tab goes there in one move, as that many steps of
    /// [`History::go_back`] or [`History::go_forward`] would take it. The folders passed over,
    /// `current` among them, go to the other list in their order, so that the other way leads
    /// back through them (a browser's menu keeps the pages ahead the same way). `None`, and no
    /// change, when the list is not that long.
    pub fn go_to(&mut self, way: Way, index: usize, current: VPath) -> Option<VPath> {
        let from = self.side(way);
        let at = from.len().checked_sub(index.checked_add(1)?)?;
        let (from, to) = match way {
            Way::Back => (&mut self.back, &mut self.forward),
            Way::Forward => (&mut self.forward, &mut self.back),
        };
        let mut passed = from.split_off(at);
        let target = passed.remove(0);
        push_capped(to, current);
        for folder in passed.into_iter().rev() {
            push_capped(to, folder);
        }
        Some(target)
    }

    /// The list of `way`, the farthest first.
    fn side(&self, way: Way) -> &[VPath] {
        match way {
            Way::Back => &self.back,
            Way::Forward => &self.forward,
        }
    }
}

/// Pushes `path` onto `list`, dropping the oldest entry past [`HISTORY_LIMIT`].
fn push_capped(list: &mut Vec<VPath>, path: VPath) {
    if list.len() == HISTORY_LIMIT {
        list.remove(0);
    }
    list.push(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> VPath {
        VPath::local(std::env::temp_dir().join(name)).expect("a local path")
    }

    #[test]
    fn back_and_forward_round_trip() {
        let (a, b, c) = (folder("a"), folder("b"), folder("c"));
        let mut history = History::default();
        assert!(!history.can_back() && !history.can_forward());
        assert_eq!(history.go_back(a.clone()), None);

        // a → b → c
        history.visit(a.clone());
        history.visit(b.clone());
        assert!(history.can_back() && !history.can_forward());

        assert_eq!(history.go_back(c.clone()), Some(b.clone()));
        assert_eq!(history.go_back(b.clone()), Some(a.clone()));
        assert!(!history.can_back() && history.can_forward());
        assert_eq!(history.go_back(a.clone()), None);

        assert_eq!(history.go_forward(a.clone()), Some(b.clone()));
        assert_eq!(history.go_forward(b.clone()), Some(c.clone()));
        assert!(history.can_back() && !history.can_forward());
        assert_eq!(history.go_forward(c.clone()), None);
        assert_eq!(history.go_back(c), Some(b));
    }

    #[test]
    fn visiting_clears_forward() {
        let (a, b, c, d) = (folder("a"), folder("b"), folder("c"), folder("d"));
        let mut history = History::default();
        history.visit(a.clone());
        history.visit(b.clone());
        assert_eq!(history.go_back(c), Some(b.clone()));
        assert!(history.can_forward());

        // At b, the user opens d: c is no longer ahead.
        history.visit(b.clone());
        assert!(!history.can_forward());
        assert_eq!(history.go_forward(d.clone()), None);
        assert_eq!(history.go_back(d), Some(b));
        assert_eq!(history.go_back(folder("b")), Some(a));
    }

    #[test]
    fn the_lists_start_with_the_nearest_folder() {
        let (a, b, c, d) = (folder("a"), folder("b"), folder("c"), folder("d"));
        let mut history = History::default();
        assert_eq!(history.list(Way::Back).count(), 0);
        // a → b → c → d, then back to c.
        history.visit(a.clone());
        history.visit(b.clone());
        history.visit(c.clone());
        assert_eq!(history.go_back(d.clone()), Some(c.clone()));
        let back: Vec<&VPath> = history.list(Way::Back).collect();
        assert_eq!(back, [&b, &a]);
        let forward: Vec<&VPath> = history.list(Way::Forward).collect();
        assert_eq!(forward, [&d]);
    }

    #[test]
    fn a_jump_moves_as_its_steps_would_and_keeps_what_it_passed() {
        let names = ["a", "b", "c", "d", "now"];
        let [a, b, c, d, now] = names.map(folder);
        let mut history = History::default();
        for from in [&a, &b, &c, &d] {
            history.visit(from.clone());
        }
        // At now, two places back is c: the move of two Back steps.
        let mut stepped = history.clone();
        assert_eq!(stepped.go_back(now.clone()), Some(d.clone()));
        assert_eq!(stepped.go_back(d.clone()), Some(c.clone()));
        assert_eq!(history.go_to(Way::Back, 1, now.clone()), Some(c.clone()));
        assert_eq!(history, stepped);
        // Forward leads back through d to now, as a browser keeps the pages ahead.
        let forward: Vec<&VPath> = history.list(Way::Forward).collect();
        assert_eq!(forward, [&d, &now]);
        let back: Vec<&VPath> = history.list(Way::Back).collect();
        assert_eq!(back, [&b, &a]);
        // And the other way: two places forward from c is now.
        assert_eq!(history.go_to(Way::Forward, 1, c.clone()), Some(now.clone()));
        let back: Vec<&VPath> = history.list(Way::Back).collect();
        assert_eq!(back, [&d, &c, &b, &a]);
        assert!(!history.can_forward());
        // The nearest one is a step.
        assert_eq!(history.go_to(Way::Back, 0, now.clone()), Some(d.clone()));
        assert_eq!(history.go_forward(d), Some(now.clone()));
        // A place past the end changes nothing.
        let before = history.clone();
        assert_eq!(history.go_to(Way::Back, 4, now.clone()), None);
        assert_eq!(history.go_to(Way::Forward, 0, now.clone()), None);
        assert_eq!(history.go_to(Way::Back, usize::MAX, now), None);
        assert_eq!(history, before);
    }

    #[test]
    fn a_jump_stays_within_the_limit() {
        let mut history = History::default();
        for i in 0..HISTORY_LIMIT {
            history.visit(folder(&i.to_string()));
        }
        let now = folder("now");
        // All the way back: every folder passed over is ahead, the nearest first, and the
        // forward list is no longer than the back list was.
        let last = HISTORY_LIMIT - 1;
        assert_eq!(history.go_to(Way::Back, last, now), Some(folder("0")));
        assert!(!history.can_back());
        assert_eq!(history.list(Way::Forward).count(), HISTORY_LIMIT);
        assert_eq!(history.list(Way::Forward).next(), Some(&folder("1")));
        assert_eq!(history.list(Way::Forward).last(), Some(&folder("now")));
    }

    #[test]
    fn history_is_capped() {
        let mut history = History::default();
        for i in 0..HISTORY_LIMIT + 10 {
            history.visit(folder(&i.to_string()));
        }
        let mut current = folder("now");
        let mut back = Vec::new();
        while let Some(previous) = history.go_back(current.clone()) {
            back.push(previous.clone());
            current = previous;
        }
        assert_eq!(back.len(), HISTORY_LIMIT);
        // The oldest folders were dropped; the newest is the first one back.
        assert_eq!(back[0], folder(&(HISTORY_LIMIT + 9).to_string()));
        assert_eq!(back[HISTORY_LIMIT - 1], folder("10"));

        // Going all the way back put every folder ahead again.
        let mut forward = 0;
        while let Some(next) = history.go_forward(current.clone()) {
            forward += 1;
            current = next;
        }
        assert_eq!(forward, HISTORY_LIMIT);
        assert_eq!(current, folder("now"));
    }
}
