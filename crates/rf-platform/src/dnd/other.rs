//! No drop target of the app's own (Linux, which is for development): winit tells the files
//! dropped on the window. Nor does a drag of rows go out of the window: it stays the window's.

use std::io;
use std::path::PathBuf;
use std::rc::Rc;

use super::{DragMods, DropEffect, DropHandler, Effects};
use crate::shell::Owner;

pub const DRAGS_OUT: bool = false;

/// No key is read here: winit tells the keys, and a menu of the system's own that would make
/// them stale does not run.
pub fn held_mods() -> DragMods {
    DragMods::default()
}

/// No registration is ever made here.
pub enum Registration {}

pub fn register(_owner: Owner, _handler: Rc<dyn DropHandler>) -> io::Result<Registration> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the window takes the files winit tells were dropped on it",
    ))
}

pub fn drag_out(
    _owner: Owner,
    _paths: &[PathBuf],
    _allowed: Effects,
    _ours: u64,
    _done: Box<dyn FnOnce(Option<DropEffect>)>,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "a drag of rows stays in the window",
    ))
}
