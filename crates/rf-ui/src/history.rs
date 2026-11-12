//! The back and forward lists of a tab.

use rf_core::VPath;

/// The most folders the back list keeps; the oldest goes first. The forward list fills only as
/// the back list empties, so it stays within this too.
pub const HISTORY_LIMIT: usize = 50;

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
