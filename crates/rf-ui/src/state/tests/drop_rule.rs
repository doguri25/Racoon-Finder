//! The rule of a drop's action: the keys of each system's file manager, the setting, what the
//! drag allows, and whether two folders are on one drive. The rules of all three systems are
//! checked on every system.

use std::path::{Path, PathBuf};

use rf_platform::dnd::{DragMods, DropEffect, Effects};

use super::*;
use crate::settings::DragDefault;
use crate::state::drop_rule::{DropChoice, Refusal, choose, same_volume};

const NO_KEYS: DragMods = DragMods {
    ctrl: false,
    shift: false,
    alt: false,
    logo: false,
};
const CTRL_KEY: DragMods = DragMods {
    ctrl: true,
    ..NO_KEYS
};
const SHIFT_KEY: DragMods = DragMods {
    shift: true,
    ..NO_KEYS
};
const ALT_KEY: DragMods = DragMods {
    alt: true,
    ..NO_KEYS
};
const LOGO_KEY: DragMods = DragMods {
    logo: true,
    ..NO_KEYS
};
const CTRL_SHIFT_KEYS: DragMods = DragMods {
    ctrl: true,
    shift: true,
    ..NO_KEYS
};
const ALT_LOGO_KEYS: DragMods = DragMods {
    alt: true,
    logo: true,
    ..NO_KEYS
};

const COPY: DropChoice = DropChoice::Do(DropEffect::Copy);
const MOVE: DropChoice = DropChoice::Do(DropEffect::Move);
const NO_LINKS: DropChoice = DropChoice::Refuse(Refusal::NoLinks);
const NOT_ALLOWED: DropChoice = DropChoice::Refuse(Refusal::NotAllowed);

/// What a drop does on `os` with `mods` held, the setting a move, everything allowed, within
/// one drive.
fn keyed(os: Os, mods: DragMods) -> DropChoice {
    choose(os, DragDefault::Move, mods, Effects::ALL, || true)
}

#[test]
fn windows_takes_file_explorers_keys() {
    assert_eq!(keyed(Os::Windows, CTRL_KEY), COPY);
    assert_eq!(keyed(Os::Windows, SHIFT_KEY), MOVE);
    assert_eq!(keyed(Os::Windows, ALT_KEY), NO_LINKS);
    assert_eq!(keyed(Os::Windows, CTRL_SHIFT_KEYS), NO_LINKS);
    // The Windows key is the system's.
    assert_eq!(keyed(Os::Windows, LOGO_KEY), MOVE);
    // A key beats the setting.
    let copying = choose(
        Os::Windows,
        DragDefault::Copy,
        SHIFT_KEY,
        Effects::ALL,
        || false,
    );
    assert_eq!(copying, MOVE);
}

#[test]
fn macos_takes_finders_keys() {
    assert_eq!(keyed(Os::MacOs, ALT_KEY), COPY);
    assert_eq!(keyed(Os::MacOs, LOGO_KEY), MOVE);
    assert_eq!(keyed(Os::MacOs, ALT_LOGO_KEYS), NO_LINKS);
    // Control and Shift change nothing there: the setting chooses.
    for mods in [CTRL_KEY, SHIFT_KEY, CTRL_SHIFT_KEYS] {
        assert_eq!(keyed(Os::MacOs, mods), MOVE, "{mods:?}");
        let copying = choose(Os::MacOs, DragDefault::Copy, mods, Effects::ALL, || true);
        assert_eq!(copying, COPY, "{mods:?}");
    }
}

#[test]
fn linux_takes_the_keys_of_its_file_managers() {
    assert_eq!(keyed(Os::Other, CTRL_KEY), COPY);
    assert_eq!(keyed(Os::Other, SHIFT_KEY), MOVE);
    assert_eq!(keyed(Os::Other, CTRL_SHIFT_KEYS), NO_LINKS);
}

#[test]
fn without_a_key_the_setting_chooses() {
    for os in [Os::Windows, Os::MacOs, Os::Other] {
        let plain =
            |default, same_volume| choose(os, default, NO_KEYS, Effects::ALL, || same_volume);
        for same_volume in [true, false] {
            assert_eq!(plain(DragDefault::Move, same_volume), MOVE, "{os:?}");
            assert_eq!(plain(DragDefault::Copy, same_volume), COPY, "{os:?}");
            assert_eq!(
                plain(DragDefault::Ask, same_volume),
                DropChoice::Ask,
                "{os:?}"
            );
        }
        // As the system's file manager does: a move within a drive, a copy to another.
        assert_eq!(plain(DragDefault::System, true), MOVE, "{os:?}");
        assert_eq!(plain(DragDefault::System, false), COPY, "{os:?}");
    }
}

#[test]
fn the_drives_are_looked_at_only_for_the_systems_way_without_a_key() {
    let looks = std::cell::Cell::new(0);
    let look = || {
        looks.set(looks.get() + 1);
        true
    };
    // A setting other than the system's way, or a key, chooses without them.
    for default in [DragDefault::Move, DragDefault::Copy, DragDefault::Ask] {
        choose(Os::Windows, default, NO_KEYS, Effects::ALL, look);
    }
    let copying = choose(
        Os::Windows,
        DragDefault::System,
        CTRL_KEY,
        Effects::ALL,
        look,
    );
    assert_eq!(copying, COPY);
    assert_eq!(looks.get(), 0);
    let moving = choose(
        Os::Windows,
        DragDefault::System,
        NO_KEYS,
        Effects::ALL,
        look,
    );
    assert_eq!(moving, MOVE);
    assert_eq!(looks.get(), 1);
}

#[test]
fn a_key_that_asks_for_what_the_drag_does_not_allow_refuses() {
    let copy_only = choose(
        Os::Windows,
        DragDefault::Move,
        SHIFT_KEY,
        Effects::COPY,
        || true,
    );
    assert_eq!(copy_only, NOT_ALLOWED);
    let move_only = choose(Os::MacOs, DragDefault::Move, ALT_KEY, Effects::MOVE, || {
        true
    });
    assert_eq!(move_only, NOT_ALLOWED);
    let move_only = choose(
        Os::Other,
        DragDefault::Move,
        CTRL_KEY,
        Effects::MOVE,
        || true,
    );
    assert_eq!(move_only, NOT_ALLOWED);
    // A key for a link refuses for that reason, whatever the drag allows.
    let linked = choose(
        Os::Windows,
        DragDefault::Move,
        ALT_KEY,
        Effects::LINK,
        || true,
    );
    assert_eq!(linked, NO_LINKS);
}

#[test]
fn a_setting_the_drag_does_not_allow_falls_back_from_a_move_to_a_copy() {
    let plain = |default, allowed, same_volume| {
        choose(Os::Windows, default, NO_KEYS, allowed, || same_volume)
    };
    // DND-05: where the drag allows no move, a move is a copy.
    assert_eq!(plain(DragDefault::Move, Effects::COPY, true), COPY);
    assert_eq!(plain(DragDefault::System, Effects::COPY, true), COPY);
    // A copy is never made a move.
    assert_eq!(plain(DragDefault::Copy, Effects::MOVE, true), NOT_ALLOWED);
    assert_eq!(
        plain(DragDefault::System, Effects::MOVE, false),
        NOT_ALLOWED
    );
    assert_eq!(plain(DragDefault::Move, Effects::MOVE, false), MOVE);
    // Neither allowed: nothing.
    for default in [
        DragDefault::Move,
        DragDefault::Copy,
        DragDefault::System,
        DragDefault::Ask,
    ] {
        assert_eq!(
            plain(default, Effects::LINK, true),
            NOT_ALLOWED,
            "{default:?}"
        );
    }
    // 놓을 때 묻기 asks while a copy or a move is allowed.
    for allowed in [Effects::COPY, Effects::MOVE, Effects::ALL] {
        assert_eq!(plain(DragDefault::Ask, allowed, true), DropChoice::Ask);
    }
}

/// The root of drive `letter` as Windows writes it (`C:\`). Elsewhere it is a name like any
/// other, and `drive('C').join("a")` is that name with `a` below it (`C:\/a`): either way the
/// path is on that root, part by part.
fn drive(letter: char) -> PathBuf {
    PathBuf::from(format!("{letter}:\\"))
}

#[test]
fn two_paths_are_on_one_drive_when_the_deepest_root_of_each_is_the_same() {
    let drives = [drive('C'), drive('D')];
    let (c, d) = (drive('C'), drive('D'));
    assert!(same_volume(&c.join("a"), &c.join("b"), &drives));
    assert!(same_volume(&c, &c.join("b").join("c"), &drives));
    assert!(!same_volume(&c.join("a"), &d.join("b"), &drives));

    // A volume mounted in a folder of another is a drive of its own.
    let roots = [PathBuf::from("/"), PathBuf::from("/Volumes/USB")];
    assert!(!same_volume(
        Path::new("/Users/x"),
        Path::new("/Volumes/USB/y"),
        &roots
    ));
    assert!(same_volume(
        Path::new("/Volumes/USB/y"),
        Path::new("/Volumes/USB/z/w"),
        &roots
    ));
    assert!(same_volume(
        Path::new("/Users/x"),
        Path::new("/Volumes/other"),
        &roots
    ));
    // A root is no root of a folder whose name only starts with its own.
    assert!(!same_volume(
        Path::new("/Volumes/USB2/a"),
        Path::new("/Volumes/USB/a"),
        &roots
    ));
}

#[test]
fn a_path_on_no_drive_known_is_on_another_drive() {
    let drives = [drive('C')];
    let c = drive('C');
    assert!(!same_volume(&c.join("a"), Path::new("/Users/x"), &drives));
    assert!(!same_volume(Path::new("/Users/x"), &c.join("a"), &drives));
    // Before the drives are known, nothing is on one.
    assert!(!same_volume(&c.join("a"), &c.join("b"), &[]));
}
