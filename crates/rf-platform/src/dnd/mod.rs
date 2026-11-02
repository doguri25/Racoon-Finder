//! Drag and drop with other apps (PRD DND-05): what a drag allows, what a drop does, the
//! modifier keys held, and the items another app drops. The app's own drags and the system's
//! speak of them in these words, so that one rule chooses a drop's action for both.
//!
//! These types are the same on every system; the system's drags themselves come in later.

use std::ops::BitOr;
use std::path::PathBuf;

/// What a drop does with the items.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DropEffect {
    Copy,
    Move,
    /// A link to them (a shortcut on Windows, an alias on macOS), which the app does not make
    /// yet.
    Link,
}

/// The [`DropEffect`]s a drag allows, as its source says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Effects(u8);

impl Effects {
    pub const COPY: Effects = Effects(1);
    pub const MOVE: Effects = Effects(1 << 1);
    pub const LINK: Effects = Effects(1 << 2);
    pub const ALL: Effects = Effects(Effects::COPY.0 | Effects::MOVE.0 | Effects::LINK.0);

    /// Whether `effect` is among these.
    pub const fn contains(self, effect: DropEffect) -> bool {
        let one = match effect {
            DropEffect::Copy => Effects::COPY,
            DropEffect::Move => Effects::MOVE,
            DropEffect::Link => Effects::LINK,
        };
        self.0 & one.0 != 0
    }
}

impl BitOr for Effects {
    type Output = Effects;

    fn bitor(self, other: Effects) -> Effects {
        Effects(self.0 | other.0)
    }
}

/// The modifier keys held during a drag. `logo` is ⌘ on macOS and the Windows key on Windows;
/// `ctrl` is Control on every system (⌃ on macOS).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each key is held or not, apart from the others"
)]
pub struct DragMods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
}

/// The items a drag from another app carries.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct DroppedItems {
    pub paths: Vec<PathBuf>,
    /// The id of the drag when this app dragged them out itself; `None` for another app's.
    pub ours: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_of_effects_holds_what_it_is_made_of() {
        let both = Effects::COPY | Effects::MOVE;
        assert!(both.contains(DropEffect::Copy));
        assert!(both.contains(DropEffect::Move));
        assert!(!both.contains(DropEffect::Link));
        assert!(Effects::LINK.contains(DropEffect::Link));
        assert!(!Effects::LINK.contains(DropEffect::Copy));
        assert!(!Effects::COPY.contains(DropEffect::Move));
        for effect in [DropEffect::Copy, DropEffect::Move, DropEffect::Link] {
            assert!(Effects::ALL.contains(effect), "{effect:?}");
        }
        assert_eq!(both | Effects::LINK, Effects::ALL);
        assert_eq!(Effects::COPY | Effects::COPY, Effects::COPY);
    }
}
