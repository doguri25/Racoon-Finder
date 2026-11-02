//! What a drop does (DND-02, DND-04): the modifier keys held, the setting of 보기 ▸ 설정 ▸
//! 끌어 놓기 기본 동작 ([`DragDefault`]) and what the drag allows choose it, with the keys of
//! each system's file manager. Pure: the rules of every system are tested on any.
//!
//! | System | Copy | Move | Link |
//! | --- | --- | --- | --- |
//! | Windows (File Explorer) | Ctrl | Shift | Alt, Ctrl+Shift |
//! | macOS (Finder) | ⌥ | ⌘ | ⌥⌘ |
//! | Linux (GNOME Files) | Ctrl | Shift | Ctrl+Shift |
//!
//! Other keys (the Windows key, macOS's Control and Shift) leave the choice to the setting. The
//! app makes no links yet, so a key for one refuses the drop ([`Refusal::NoLinks`]); a key for
//! what the drag does not allow refuses it too ([`Refusal::NotAllowed`]). Without a key the
//! setting chooses: a move, a copy, the system's way (a move within a drive, a copy to
//! another), or the menu of 놓을 때 묻기. A move the drag does not allow is a copy (DND-05), and
//! a copy is never made a move.

use std::path::{Path, PathBuf};

use rf_core::Os;
use rf_platform::dnd::{DragMods, DropEffect, Effects};

use crate::settings::DragDefault;

/// What a drop at a place that takes the items does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DropChoice {
    /// Copy or move them there at once.
    Do(DropEffect),
    /// Ask with a menu where they are dropped (놓을 때 묻기).
    Ask,
    /// Nothing, for this reason.
    Refuse(Refusal),
}

/// Why a drop does nothing at a place that takes the items.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// The keys ask for a link, which the app does not make yet.
    NoLinks,
    /// The drag does not allow what the keys or the setting ask for.
    NotAllowed,
}

/// What a drop does on `os` with the keys `mods` held, the setting `default` and the effects
/// the drag `allowed`, the items going to a folder on the drive they are on or not
/// (`same_volume`, [`same_volume`]).
pub fn choose(
    os: Os,
    default: DragDefault,
    mods: DragMods,
    allowed: Effects,
    same_volume: bool,
) -> DropChoice {
    if let Some(asked) = keyed_effect(os, mods) {
        return match asked {
            DropEffect::Link => DropChoice::Refuse(Refusal::NoLinks),
            _ if allowed.contains(asked) => DropChoice::Do(asked),
            _ => DropChoice::Refuse(Refusal::NotAllowed),
        };
    }
    let wanted = match default {
        DragDefault::Move => DropEffect::Move,
        DragDefault::System if same_volume => DropEffect::Move,
        DragDefault::Copy | DragDefault::System => DropEffect::Copy,
        DragDefault::Ask => {
            let either = allowed.contains(DropEffect::Move) || allowed.contains(DropEffect::Copy);
            return if either {
                DropChoice::Ask
            } else {
                DropChoice::Refuse(Refusal::NotAllowed)
            };
        }
    };
    [DropEffect::Move, DropEffect::Copy]
        .into_iter()
        .skip_while(|&effect| effect != wanted)
        .find(|&effect| allowed.contains(effect))
        .map_or(DropChoice::Refuse(Refusal::NotAllowed), DropChoice::Do)
}

/// What the keys `mods` ask for on `os`; `None` when they leave it to the setting.
fn keyed_effect(os: Os, mods: DragMods) -> Option<DropEffect> {
    let DragMods {
        ctrl,
        shift,
        alt,
        logo,
    } = mods;
    let (copy, moves, link) = match os {
        Os::Windows => (ctrl, shift, alt || (ctrl && shift)),
        Os::MacOs => (alt, logo, alt && logo),
        Os::Other => (ctrl, shift, ctrl && shift),
    };
    if link {
        Some(DropEffect::Link)
    } else if copy {
        Some(DropEffect::Copy)
    } else if moves {
        Some(DropEffect::Move)
    } else {
        None
    }
}

/// Whether `a` and `b` are on one drive: each is on the deepest of `roots` (the drives of the
/// sidebar) that holds it, and that is the same one. A path on none of them is on no drive
/// known, which counts as another drive (a copy is the safe guess).
pub fn same_volume(a: &Path, b: &Path, roots: &[PathBuf]) -> bool {
    let drive = |path: &Path| {
        roots
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
    };
    matches!((drive(a), drive(b)), (Some(a), Some(b)) if a == b)
}
