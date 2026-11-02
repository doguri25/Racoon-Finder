//! No drop target of the app's own (Linux, which is for development): winit tells the files
//! dropped on the window.

use std::io;
use std::rc::Rc;

use super::DropHandler;
use crate::shell::Owner;

/// No registration is ever made here.
pub enum Registration {}

pub fn register(_owner: Owner, _handler: Rc<dyn DropHandler>) -> io::Result<Registration> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the window takes the files winit tells were dropped on it",
    ))
}
