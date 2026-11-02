#![allow(
    unsafe_code,
    reason = "a view of the app's own over the window's content and a drag source, defined with \
              `define_class!`"
)]

//! The window's drop target on macOS: a view of the app's own over the window's content (winit's
//! view), which takes the system's drags of files and lets everything else through to winit. And
//! the drag of rows the app carries out of the window ([`drag_out`]).
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
//!   drop. The URLs that name no path the app can read safely (another computer's, an encoded
//!   `/`, a Finder reference to an item that is gone) are left out and counted
//!   ([`DroppedItems::skipped`]), for the window to tell; a drag of nothing else reaches the
//!   handler all the same, which refuses it and says why. A drag that carries no file URL (text
//!   from a browser) never comes to this view.
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
//!   (as it does for every destination that does not say otherwise; the view's
//!   `wantsPeriodicDraggingUpdates` says YES all the same), so that a list that scrolls under a
//!   still pointer shows the row under it. The drop is taken only for a copy or a move the app's
//!   job does ([`drop_taken`]): then the source learns of the operation the last step answered,
//!   never a move. The menu of 놓을 때 묻기 and a refusal take nothing, so the source learns of nothing
//!   before the user chooses, and the drag image slides back. The app's job deletes each
//!   original of a move once its copy is done.
//! - **The app's own drag** (a drag of rows that left the window and came back) is the one whose
//!   `draggingSource` is the app's own source object ([`DragSource`]): [`ours_of`] names its id.
//! - **A drag out** ([`drag_out`]) is a dragging session that the window's view (winit's) begins
//!   (`beginDraggingSessionWithItems:event:source:`), with a dragging item for each path: its
//!   file URL, written byte for byte as the clipboard writes it, and its icon
//!   (`NSWorkspace iconForFile:`), stacked under the pointer. The event it begins with is the
//!   app's current one when it is a drag of the left button in the window, else one made for
//!   where the mouse is now. Its source ([`DragSource`]) allows what [`source_mask`] says: out of
//!   the app all that Finder's own drags allow, within it a move (with the generic operation, so
//!   that the window's answer keeps the move's pointer) or a copy. As the session ends, it hands
//!   what the taker did to `done` ([`ended_effect`]), and nothing else: nothing is deleted here.
//!   The source is held until then ([`DRAGGING`]).
//! - **No borrow across a call out:** the view's record of the drag (a `RefCell`) is never
//!   borrowed while the handler runs, and the view holds itself alive while it does, so that a
//!   handler that lets the registration go does not let the view go under its own call.

use std::cell::RefCell;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSDragOperation, NSDraggingContext,
    NSDraggingDestination, NSDraggingInfo, NSDraggingItem, NSDraggingSession, NSDraggingSource,
    NSEvent, NSEventType, NSResponder, NSView, NSWindow, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSObject, NSObjectProtocol, NSPoint, NSProcessInfo, NSRect, NSSize, NSString,
};

use super::{
    DragMods, DropEffect, DropHandler, DroppedItems, Effects, answer_for, drop_taken, ended_effect,
    flag_mods, flipped_point, mask_effects, source_mask,
};
use crate::clipboard::{file_item, file_paths, file_url_type};
use crate::shell::{Owner, WindowPoint};

pub const DRAGS_OUT: bool = true;

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

/// The view of the window `owner`'s content (winit's), retained, on the main thread, which it
/// needs; `InvalidInput` for an owner without one. The picker of 공유… shows by it too
/// (`crate::shell`).
pub(crate) fn content_view(owner: Owner) -> io::Result<(MainThreadMarker, Retained<NSView>)> {
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
    Ok((mtm, content))
}

pub fn register(owner: Owner, handler: Rc<dyn DropHandler>) -> io::Result<Registration> {
    let (mtm, content) = content_view(owner)?;
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

        /// YES, as `AppKit` takes it of a destination without this method: said here so that
        /// no change of that default stops the steps of a still pointer.
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

/// The items of the drag `sender`: the paths of its file URLs, read byte for byte, its id when
/// the app started it, and how many of its URLs name no path the app can read safely, which are
/// left out of the paths as a paste leaves them out ([`DroppedItems::skipped`]); `None` when it
/// names no file at all.
fn items_of(sender: &ProtocolObject<dyn NSDraggingInfo>) -> Option<DroppedItems> {
    let (paths, skipped) = autoreleasepool(|_| file_paths(&sender.draggingPasteboard()));
    if paths.is_empty() && skipped == 0 {
        return None;
    }
    Some(DroppedItems {
        paths,
        ours: ours_of(sender),
        skipped,
    })
}

/// The id of the drag `sender` when this app started it ([`DroppedItems::ours`]): a drag of rows
/// that left the window and came back, whose `draggingSource` is the app's own source object
/// ([`DragSource`]). Another app's drag has no source in this process (nil).
fn ours_of(sender: &ProtocolObject<dyn NSDraggingInfo>) -> Option<u64> {
    let source = sender.draggingSource()?;
    source
        .downcast_ref::<DragSource>()
        .map(|source| source.ivars().id)
}

/// The side of a dragging item's icon, in points.
const ICON: f64 = 32.0;

/// How far each item's icon sits from the one before it under the pointer, and how many of them
/// step aside: the rest lie on the last.
const ICON_STEP: f64 = 4.0;
const STEPS: u8 = 4;

thread_local! {
    /// The source of the drag out under way, held until its session ends: `AppKit` asks it what
    /// the drag allows, and tells it the end, for as long as the drag goes on.
    static DRAGGING: RefCell<Option<Retained<DragSource>>> = const { RefCell::new(None) };
}

pub fn drag_out(
    owner: Owner,
    paths: &[PathBuf],
    allowed: Effects,
    ours: u64,
    done: Box<dyn FnOnce(Option<DropEffect>)>,
) -> io::Result<()> {
    let (mtm, view) = content_view(owner)?;
    autoreleasepool(|_| {
        let window = view
            .window()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no window"))?;
        let event = drag_event(mtm, &window)
            .ok_or_else(|| io::Error::other("no event to begin a drag with"))?;
        // Where the pointer is, in the view's coordinates, whose y grows downward or upward:
        // the icons lie around it either way.
        let at = view.convertPoint_fromView(event.locationInWindow(), None);
        let items = dragging_items(paths, at)?;
        let source = DragSource::new(mtm, ours, allowed, done);
        // Held before the session begins, which may end it at once (the button let go). A
        // source of a drag before, which never heard its end, goes now.
        let before = DRAGGING.with(|held| held.replace(Some(source.clone())));
        drop(before);
        let _session = view.beginDraggingSessionWithItems_event_source(
            &NSArray::from_retained_slice(&items),
            &event,
            ProtocolObject::from_ref(&*source),
        );
        Ok(())
    })
}

/// The event a drag out begins with: the app's current one when it is a drag of the left button
/// in `window` (the move that took the pointer out of the window), else a drag of the left
/// button made for where the mouse is now, with the keys held now.
fn drag_event(mtm: MainThreadMarker, window: &NSWindow) -> Option<Retained<NSEvent>> {
    let current = NSApplication::sharedApplication(mtm).currentEvent();
    if let Some(event) = current.filter(|event| {
        event.r#type() == NSEventType::LeftMouseDragged
            && event.windowNumber() == window.windowNumber()
    }) {
        return Some(event);
    }
    NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        NSEventType::LeftMouseDragged,
        window.mouseLocationOutsideOfEventStream(),
        NSEvent::modifierFlags_class(),
        NSProcessInfo::processInfo().systemUptime(),
        window.windowNumber(),
        None,
        0,
        1,
        1.0,
    )
}

/// A dragging item for each of `paths`: its file URL, byte for byte, as the clipboard writes it,
/// and its icon, stacked around `at`, the pointer in the view's coordinates.
fn dragging_items(paths: &[PathBuf], at: NSPoint) -> io::Result<Vec<Retained<NSDraggingItem>>> {
    let workspace = NSWorkspace::sharedWorkspace();
    let mut step = 0;
    paths
        .iter()
        .map(|path| {
            let writer = file_item(path, None)?;
            let item = NSDraggingItem::initWithPasteboardWriter(
                NSDraggingItem::alloc(),
                ProtocolObject::from_ref(&*writer),
            );
            // The icon only shows: a name that is not UTF-8 gets the icon of what its text names.
            let icon = workspace.iconForFile(&NSString::from_str(&path.to_string_lossy()));
            let aside = ICON_STEP * f64::from(step);
            step = (step + 1).min(STEPS);
            let frame = NSRect::new(
                NSPoint::new(at.x - ICON / 2.0 + aside, at.y - ICON / 2.0 + aside),
                NSSize::new(ICON, ICON),
            );
            let contents: &AnyObject = &icon;
            // SAFETY: the contents of a dragging item's frame are an image, which `icon` is.
            unsafe { item.setDraggingFrame_contents(frame, Some(contents)) };
            Ok(item)
        })
        .collect()
}

/// What hears the end of a drag out ([`drag_out`]'s `done`).
type Done = Box<dyn FnOnce(Option<DropEffect>)>;

/// What a [`DragSource`] holds.
struct SourceIvars {
    /// The id of the app's drag it carries out ([`DroppedItems::ours`]).
    id: u64,
    /// What the drag allows ([`source_mask`]).
    allowed: Effects,
    /// Hears what the taker did as the drag ends, once.
    done: RefCell<Option<Done>>,
}

define_class!(
    // SAFETY: `NSObject` asks nothing of a subclass that this one breaks: it adds the methods of
    // a drag source, and has no `Drop` of its own (its instance variables go as it is
    // deallocated, on the main thread, where AppKit lets its drag sources go).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RFDragSource"]
    #[ivars = SourceIvars]
    struct DragSource;

    // SAFETY: `NSObjectProtocol` asks nothing of a subclass of `NSObject`.
    unsafe impl NSObjectProtocol for DragSource {}

    // SAFETY: each method has the selector and the types `NSDraggingSource` declares.
    unsafe impl NSDraggingSource for DragSource {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operation_mask(
            &self,
            _session: &NSDraggingSession,
            context: NSDraggingContext,
        ) -> NSDragOperation {
            let within = context == NSDraggingContext::WithinApplication;
            NSDragOperation(source_mask(self.ivars().allowed, within))
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn ended(&self, _session: &NSDraggingSession, _at: NSPoint, operation: NSDragOperation) {
            // Held while it runs: letting the session's hold go may let the source go.
            let _alive = self.retain();
            let done = self.ivars().done.take();
            let held = DRAGGING.with(|held| {
                let mine = held
                    .borrow()
                    .as_ref()
                    .is_some_and(|held| std::ptr::eq(&raw const **held, self));
                if mine { held.take() } else { None }
            });
            drop(held);
            if let Some(done) = done {
                done(ended_effect(operation.0));
            }
        }
    }
);

impl DragSource {
    /// The source of the drag `id` out of the window, which allows `allowed` and tells `done` its
    /// end.
    fn new(mtm: MainThreadMarker, id: u64, allowed: Effects, done: Done) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SourceIvars {
            id,
            allowed,
            done: RefCell::new(Some(done)),
        });
        // SAFETY: `NSObject`'s designated initializer, on an object just allocated, once.
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use objc2::encode::Encode;
    use objc2::runtime::{AnyClass, Bool, Sel};
    use objc2::{ClassType, sel};
    use objc2_app_kit::{NSDragOperation, NSDraggingContext};
    use objc2_foundation::NSPoint;

    use super::{DragSource, Overlay};

    /// Whether the method `selector` of `class` returns `returns` and takes `takes` after `self`
    /// and `_cmd`, by the types it was registered with.
    fn has_types(class: &AnyClass, selector: Sel, returns: &str, takes: &[&str]) -> bool {
        let Some(method) = class.instance_method(selector) else {
            return false;
        };
        let given = |encoding: &std::ffi::CStr| encoding.to_str().unwrap_or("").to_owned();
        let arguments: Vec<String> = (2..method.arguments_count())
            .filter_map(|i| method.argument_type(i).map(|encoding| given(&encoding)))
            .collect();
        given(&method.return_type()) == returns && arguments == takes
    }

    /// Making the source's class, which needs no drag, checks in a debug build that each method
    /// of a drag source is one `NSDraggingSource` declares; the types of each (which that check
    /// does not look at) are checked here, against what `AppKit` passes: a mistake would stop the
    /// app at the first drag out.
    #[test]
    fn the_drag_source_s_class_answers_what_appkit_asks_of_a_drag_source() {
        let class = DragSource::class();
        assert_eq!(class.name().to_str(), Ok("RFDragSource"));
        let operation = NSDragOperation::ENCODING.to_string();
        assert!(has_types(
            class,
            sel!(draggingSession:sourceOperationMaskForDraggingContext:),
            &operation,
            &["@", &NSDraggingContext::ENCODING.to_string()],
        ));
        assert!(has_types(
            class,
            sel!(draggingSession:endedAtPoint:operation:),
            "v",
            &["@", &NSPoint::ENCODING.to_string(), &operation],
        ));
    }

    /// Making the view's class, which needs no window, checks in a debug build that `hitTest:`
    /// has the types `NSView` gives it and that each method of a drag destination is one that
    /// `NSDraggingDestination` declares: a mistake would stop the app at the window's first show.
    /// The types of the drag methods, which that check does not look at, are checked here
    /// against what `AppKit` passes and takes back.
    #[test]
    fn the_view_s_class_answers_what_appkit_asks_of_a_drop_target() {
        let class = Overlay::class();
        assert_eq!(class.name().to_str(), Ok("RFDropOverlay"));
        let operation = NSDragOperation::ENCODING.to_string();
        let yes_or_no = Bool::ENCODING.to_string();
        let point = NSPoint::ENCODING.to_string();
        for (selector, returns, takes) in [
            (sel!(hitTest:), "@", point.as_str()),
            (sel!(draggingEntered:), operation.as_str(), "@"),
            (sel!(draggingUpdated:), operation.as_str(), "@"),
            (sel!(draggingExited:), "v", "@"),
            (sel!(prepareForDragOperation:), yes_or_no.as_str(), "@"),
            (sel!(performDragOperation:), yes_or_no.as_str(), "@"),
        ] {
            assert!(has_types(class, selector, returns, &[takes]), "{selector}");
        }
        assert!(has_types(
            class,
            sel!(wantsPeriodicDraggingUpdates),
            &yes_or_no,
            &[]
        ));
    }
}
