//! The window's drop target for the system's drags (DND-05 받기): registered on the window's
//! handle each time the window shows (its first show, and its return from the tray), and again
//! when the handle is another one, since a window shown again may be a new window. A handle
//! keeps its first registration, or its first failure, which is told once and leaves the window
//! without drops from other apps; the app goes on.

use std::cell::RefCell;
use std::fmt;
use std::io;

use rf_platform::dnd::{DragMods, DropEffect, DropHandler, DroppedItems, Effects};
use rf_platform::shell::{Owner, WindowPoint};

use super::{Controller, with};

/// The system's drags over the window, which the platform's drop target hands over, for the
/// controller to answer ([`Controller::outside_over`], [`Controller::outside_drop`],
/// [`Controller::outside_leave`]). It holds nothing of the window: each step finds the
/// controller of the thread, and answers nothing once there is none.
#[derive(Debug, Default)]
pub struct WindowDrops;

impl DropHandler for WindowDrops {
    fn over(
        &self,
        at: WindowPoint,
        mods: DragMods,
        allowed: Effects,
        items: &DroppedItems,
    ) -> Option<DropEffect> {
        with(|c| c.outside_over(at, items, mods, allowed)).flatten()
    }

    fn leave(&self) {
        with(Controller::outside_leave);
    }

    fn drop(
        &self,
        at: WindowPoint,
        mods: DragMods,
        allowed: Effects,
        items: DroppedItems,
        right_button: bool,
    ) -> Option<DropEffect> {
        with(|c| c.outside_drop(at, items, mods, allowed, right_button)).flatten()
    }

    fn hint(&self) -> String {
        with(Controller::outside_hint).unwrap_or_default()
    }
}

/// The registration of the window's drop target, made by `register` (the platform's
/// [`register_drop_target`](rf_platform::dnd::register_drop_target) in the app, a stand-in in
/// the tests) and held while the window keeps its handle.
pub struct DropTarget<R> {
    register: Box<dyn Fn(Owner) -> io::Result<R>>,
    /// The handle registered last, with its registration (`None` where it failed).
    held: RefCell<Option<(Owner, Option<R>)>>,
}

impl<R> fmt::Debug for DropTarget<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DropTarget").finish_non_exhaustive()
    }
}

impl<R> DropTarget<R> {
    pub fn new(register: impl Fn(Owner) -> io::Result<R> + 'static) -> DropTarget<R> {
        DropTarget {
            register: Box::new(register),
            held: RefCell::new(None),
        }
    }

    /// The window shows with the handle `owner` ([`Owner::NONE`]: none yet, which keeps what is
    /// held): a handle other than the one registered last is registered, once the old
    /// registration has gone. A failure is told (but for a system without such a drop target,
    /// where winit tells the files dropped), and the handle is not asked about again.
    pub fn follow(&self, owner: Owner) {
        if owner == Owner::NONE
            || self
                .held
                .borrow()
                .as_ref()
                .is_some_and(|(held, _)| *held == owner)
        {
            return;
        }
        // The old window's target goes before the new one's comes.
        drop(self.held.take());
        let registration = match (self.register)(owner) {
            Ok(registration) => Some(registration),
            Err(error) => {
                if error.kind() != io::ErrorKind::Unsupported {
                    log_refused(&error);
                }
                None
            }
        };
        *self.held.borrow_mut() = Some((owner, registration));
    }
}

/// Tells why the window takes no drops from other apps, where a developer or a report of a user
/// sees it.
#[expect(
    clippy::print_stderr,
    reason = "the app has no log of its own yet; Slint's own messages go to the same place"
)]
fn log_refused(error: &io::Error) {
    eprintln!("the window takes no drops from other apps: {error}");
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::num::NonZeroIsize;
    use std::rc::Rc;

    use super::*;

    /// A stand-in registration, which counts the ones that are held.
    struct Held(Rc<Cell<usize>>);

    impl Drop for Held {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }

    fn window(handle: isize) -> Owner {
        Owner::hwnd(NonZeroIsize::new(handle).expect("not zero"))
    }

    #[test]
    fn a_new_window_handle_registers_the_drop_target_again() {
        let asked = Rc::new(RefCell::new(Vec::new()));
        let held = Rc::new(Cell::new(0));
        let target = DropTarget::new({
            let (asked, held) = (Rc::clone(&asked), Rc::clone(&held));
            move |owner: Owner| {
                asked.borrow_mut().push(owner);
                held.set(held.get() + 1);
                Ok(Held(Rc::clone(&held)))
            }
        });
        // No window yet: nothing to register on.
        target.follow(Owner::NONE);
        assert!(asked.borrow().is_empty());
        // The first show registers; the window shown again with the same handle does not.
        target.follow(window(0x10));
        target.follow(window(0x10));
        assert_eq!(*asked.borrow(), [window(0x10)]);
        assert_eq!(held.get(), 1);
        // A window made anew: the old registration goes, and the new handle gets its own.
        target.follow(window(0x20));
        assert_eq!(*asked.borrow(), [window(0x10), window(0x20)]);
        assert_eq!(held.get(), 1);
        target.follow(window(0x20));
        assert_eq!(asked.borrow().len(), 2);
        // A show before the new window exists keeps what is held.
        target.follow(Owner::NONE);
        assert_eq!(held.get(), 1);
        drop(target);
        assert_eq!(held.get(), 0);
    }

    #[test]
    fn a_handle_the_system_refuses_is_asked_about_once_and_the_app_goes_on() {
        let asked = Rc::new(Cell::new(0));
        let target = DropTarget::new({
            let asked = Rc::clone(&asked);
            move |owner: Owner| -> io::Result<()> {
                asked.set(asked.get() + 1);
                if owner == window(0x10) {
                    Err(io::Error::other("refused"))
                } else {
                    Ok(())
                }
            }
        });
        target.follow(window(0x10));
        target.follow(window(0x10));
        assert_eq!(asked.get(), 1);
        // Another handle is asked about again.
        target.follow(window(0x20));
        assert_eq!(asked.get(), 2);
    }
}
