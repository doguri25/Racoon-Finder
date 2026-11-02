//! The macOS drop target, which is not there yet: winit's own drop of files takes the files
//! dropped on the window meanwhile.

use std::io;
use std::rc::Rc;

use super::DropHandler;
use crate::shell::Owner;

/// No registration is made yet.
pub enum Registration {}

pub fn register(_owner: Owner, _handler: Rc<dyn DropHandler>) -> io::Result<Registration> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no drop target of the app's own on macOS yet",
    ))
}
