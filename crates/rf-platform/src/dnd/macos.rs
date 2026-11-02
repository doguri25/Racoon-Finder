#![allow(
    unsafe_code,
    reason = "a view of the app's own over the window's content, defined with `define_class!`"
)]

//! The window's drop target on macOS: a view of the app's own over the window's content (winit's
//! view), which takes the system's drags of files and lets everything else through to winit.
//!
//! - **Where:** a subview of the content view, the whole of it, which follows its size
//!   (autoresizing). `hitTest:` answers nil, so the mouse, the wheel and the clicks go to winit's
//!   view under it as before. `AppKit` gives a drag to the deepest view under the pointer that is
//!   registered for one of its types (here `public.file-url`), else to the window. winit's own
//!   destination, the window (its delegate, for `NSFilenamesPboardType`), is left as it is: a
//!   drag over the content is this view's, and one over the title bar is still winit's, which
//!   tells its files dropped (`DroppedFile`) as it does on Linux. Should this view take no drag
//!   on some system (if `hitTest:` hid it from drags too), every drag goes to the window and
//!   winit's drop of files takes it, as before this view; and where the view cannot be made, the
//!   window keeps winit's drop alone.
//! - **The items:** the file URLs of the drag's pasteboard (`public.file-url`), read as the
//!   clipboard reads them, byte for byte ([`file_paths`]), as the drag enters and again at the
//!   drop. A drag that names no path the app can read is answered `NSDragOperationNone` all the
//!   way and never reaches the handler; a drag that carries no file URL (text from a browser)
//!   never comes to this view.
//! - **The keys, the point and the effects:** the modifier keys held now
//!   (`NSEvent.modifierFlags`, [`flag_mods`]), never winit's, which it forgets while another app
//!   has the focus; `draggingLocation`, in the view's points from its top left corner
//!   ([`flipped_point`]), which are the window's logical pixels; the source's operation mask
//!   (`draggingSourceOperationMask`, [`mask_effects`]), which `AppKit` has already narrowed by the
//!   keys (⌥ to copy, ⌘ to the generic operation, ⌃ to a link). No drag here is one with the
//!   right button.
//! - **The answers:** what the source hears is never its move ([`answer_for`]), which it may
//!   read as "moved, delete the originals" while the app's own copy still runs: a copy is a copy,
//!   and a move the app's job makes is the generic operation (the destination's own), or a copy
//!   for a source that allows no generic one. A source that allows nothing but its move cannot
//!   drop here: the handler hears that it allows no move ([`mask_effects`]). The pointer shows
//!   that answer at each step, and `AppKit` asks again from time to time while the pointer rests
//!   (`wantsPeriodicDraggingUpdates`), so that a list that scrolls under a still pointer shows
//!   the row under it. The drop is taken only for a copy or a move the app's job does
//!   ([`drop_taken`]): then the source learns of the operation the last step answered, never a
//!   move. The menu of 놓을 때 묻기 and a refusal take nothing, so the source learns of nothing
//!   before the user chooses, and the drag image slides back. The app's job deletes each
//!   original of a move once its copy is done.
//! - **The app's own drag** (a drag of rows that left the window and came back, Task 5) is not
//!   told yet: [`ours_of`] names none.
//! - **No borrow across a call out:** the view's record of the drag (a `RefCell`) is never
//!   borrowed while the handler runs, and the view holds itself alive while it does, so that a
//!   handler that lets the registration go does not let the view go under its own call.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSDragOperation, NSDraggingDestination, NSDraggingInfo, NSEvent,
    NSResponder, NSView,
};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect};

use super::{
    DragMods, DropHandler, DroppedItems, answer_for, drop_taken, flag_mods, flipped_point,
    mask_effects,
};
use crate::clipboard::{file_paths, file_url_type};
use crate::shell::{Owner, WindowPoint};

/// The view over the window's content, there until this is dropped.
pub struct Registration {
    view: Retained<Overlay>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        // A view whose window is gone has no superview any more, which is no matter.
        self.view.unregisterDraggedTypes();
        self.view.removeFromSuperview();
    }
}

pub fn register(owner: Owner, handler: Rc<dyn DropHandler>) -> io::Result<Registration> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| io::Error::other("the window's view is the main thread's"))?;
    let content = owner
        .ns_view()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no window view"))?;
    // SAFETY: `ns_view` is the `NSView` of the app's window's content, as raw-window-handle names
    // it (`owner_of`), alive while the window is; this is the main thread, where the window
    // lives, and the view is retained for as long as it is used here.
    let content = unsafe { Retained::retain(content.as_ptr().cast::<NSView>()) }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no window view"))?;
    let view = Overlay::new(mtm, handler, content.bounds());
    view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    view.registerForDraggedTypes(&NSArray::from_slice(&[file_url_type()]));
    // On top of winit's view and of anything it holds.
    content.addSubview(&view);
    Ok(Registration { view })
}

/// What the view holds.
struct Ivars {
    /// The window's answers to the drags.
    handler: Rc<dyn DropHandler>,
    /// The items of the drag over the view, read as it entered; `None` while none is, and for a
    /// drag that names no path. In an `Rc`, so that a borrow of the record copies a pointer.
    items: RefCell<Option<Rc<DroppedItems>>>,
}

define_class!(
    // SAFETY: `NSView` asks nothing of a subclass that this one breaks: it overrides `hitTest:`
    // with the types `NSView` declares, adds the optional methods of a drag destination, and
    // has no `Drop` of its own (its instance variables go as it is deallocated, on the main
    // thread, where AppKit lets its views go).
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RFDropOverlay"]
    #[ivars = Ivars]
    struct Overlay;

    // SAFETY: `NSObjectProtocol` asks nothing of a subclass of `NSObject`.
    unsafe impl NSObjectProtocol for Overlay {}

    impl Overlay {
        /// No point of the window is the view's: the mouse goes to winit's view under it.
        // SAFETY: `hitTest:` takes a point and returns a view or nil, as `NSView` declares it.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }
    }

    // SAFETY: each method has the selector and the types `NSDraggingDestination` declares; all of
    // them are optional.
    unsafe impl NSDraggingDestination for Overlay {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            let entered = items_of(sender).map(Rc::new);
            // A drag that never left lets its items go once the record is no longer borrowed.
            let stale = self.ivars().items.replace(entered);
            drop(stale);
            self.hover(sender)
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.hover(sender)
        }

        #[unsafe(method(wantsPeriodicDraggingUpdates))]
        fn wants_periodic_dragging_updates(&self) -> bool {
            true
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, _sender: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            let _alive = self.retain();
            let over = self.ivars().items.take();
            drop(over);
            self.ivars().handler.leave();
        }

        #[unsafe(method(prepareForDragOperation:))]
        fn prepare_for_drag_operation(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            // AppKit asks only after a step answered an operation; the drop decides.
            true
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag_operation(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let _alive = self.retain();
            let over = self.ivars().items.take();
            drop(over);
            // Every path is read again now, from the pasteboard of the drop. (One expression: the
            // macro turns the value of the body into a `BOOL`, not that of a `return`.)
            if let Some(items) = items_of(sender) {
                let handler = Rc::clone(&self.ivars().handler);
                let mask = sender.draggingSourceOperationMask().0;
                let done = DropHandler::drop(
                    &*handler,
                    self.point_of(sender),
                    keys(),
                    mask_effects(mask),
                    items,
                    false,
                );
                drop_taken(done)
            } else {
                self.ivars().handler.leave();
                false
            }
        }
    }
);

impl Overlay {
    /// A view at `frame` of the content view, whose drags `handler` answers.
    fn new(mtm: MainThreadMarker, handler: Rc<dyn DropHandler>, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            handler,
            items: RefCell::new(None),
        });
        // SAFETY: `NSView`'s designated initializer, on an object just allocated, once.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// The drag `sender` over the view: the handler's answer, as the operation the pointer shows
    /// and the source hears ([`answer_for`], never a move); none for a drag that names no path,
    /// which the handler never hears of.
    fn hover(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
        let _alive = self.retain();
        // Nothing of the view is borrowed while the handler runs.
        let items = self.ivars().items.borrow().clone();
        let Some(items) = items else {
            return NSDragOperation::None;
        };
        let handler = Rc::clone(&self.ivars().handler);
        let mask = sender.draggingSourceOperationMask().0;
        let answer = handler.over(self.point_of(sender), keys(), mask_effects(mask), &items);
        NSDragOperation(answer_for(answer, mask))
    }

    /// Where the drag `sender` is, in the window's logical pixels from the top left corner of its
    /// content, which the view covers.
    fn point_of(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> WindowPoint {
        // From the window's coordinates to the view's, whose y grows upward.
        let at = self.convertPoint_fromView(sender.draggingLocation(), None);
        flipped_point(self.bounds().size.height, (at.x, at.y))
    }
}

/// The modifier keys held now, as the system tells them.
fn keys() -> DragMods {
    flag_mods(NSEvent::modifierFlags_class().0)
}

/// The items of the drag `sender`: the paths of its file URLs, read byte for byte, and its id
/// when the app started it; `None` when it names no path the app can read (those URLs are left
/// out, as a paste leaves them out).
fn items_of(sender: &ProtocolObject<dyn NSDraggingInfo>) -> Option<DroppedItems> {
    let (paths, _left_out) = autoreleasepool(|_| file_paths(&sender.draggingPasteboard()));
    if paths.is_empty() {
        return None;
    }
    Some(DroppedItems {
        paths,
        ours: ours_of(sender),
    })
}

/// The id of the drag `sender` when this app started it ([`DroppedItems::ours`]): a drag of rows
/// that left the window and came back, whose `draggingSource` is the app's own source object.
/// Task 5 fills this in, as it makes that object; until then every drag is another app's.
fn ours_of(_sender: &ProtocolObject<dyn NSDraggingInfo>) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use objc2::{ClassType, sel};

    use super::Overlay;

    /// Making the view's class, which needs no window, checks in a debug build that `hitTest:`
    /// has the types `NSView` gives it and that each method of a drag destination is one that
    /// `NSDraggingDestination` declares: a mistake would stop the app at the window's first show.
    #[test]
    fn the_view_s_class_answers_what_appkit_asks_of_a_drop_target() {
        let class = Overlay::class();
        for selector in [
            sel!(hitTest:),
            sel!(draggingEntered:),
            sel!(draggingUpdated:),
            sel!(wantsPeriodicDraggingUpdates),
            sel!(draggingExited:),
            sel!(prepareForDragOperation:),
            sel!(performDragOperation:),
        ] {
            assert!(class.responds_to(selector), "{selector}");
        }
        assert_eq!(class.name().to_str(), Ok("RFDropOverlay"));
    }
}
