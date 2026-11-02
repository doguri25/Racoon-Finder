#![allow(
    unsafe_code,
    reason = "OLE drag and drop: the window's IDropTarget, a drag's data object, the drag image"
)]

//! The window's drop target on Windows: an `IDropTarget` of the app's own in the place of
//! winit's, on the window's (STA) thread, where winit has initialized OLE.
//!
//! - **The items:** the paths of `CF_HDROP`, read as the clipboard reads them, when the drag
//!   enters the window and again inside `Drop` (the data object is not used once a call
//!   returns, but for the drop description of the drag over the window, which it holds until
//!   the drag leaves or drops). A drag without `CF_HDROP` (text from a browser) is answered
//!   `DROPEFFECT_NONE` all the way. The tag of the app's own drag, in its registered format
//!   ([`ours_format`](super::ours_format)), names the drag when this process started it
//!   ([`ours_in`](super::ours_in)).
//! - **The keys and the point:** `grfKeyState` ([`key_mods`](super::key_mods); the right
//!   button of the drag is noted as it enters, since its release clears it before `Drop`), and
//!   `POINTL`, a screen point in physical pixels, named in the window's logical pixels
//!   ([`client_point`](super::client_point), from the content's origin, `ClientToScreen`, and
//!   `GetDpiForWindow / 96`).
//! - **What shows:** the shell's drag image follows the drag over the window
//!   (`IDropTargetHelper`), with what the window says of the drop written under it
//!   (`DropDescription`, [`DropHandler::hint`](super::DropHandler::hint)); a description that
//!   cannot be written is left out.
//! - **The drop** answers by [`drop_reply`](super::drop_reply) and writes `Performed
//!   DropEffect` and `Logical Performed DropEffect` on the data object; a write that fails is
//!   left out. A move is never answered as one.

use std::ffi::{OsStr, c_void};
use std::io;
use std::mem::ManuallyDrop;
use std::rc::Rc;

use windows::Win32::Foundation::{HGLOBAL, HWND};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM,
    STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Ole::{DROPEFFECT, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop};
use windows::Win32::UI::Shell::{
    CFSTR_DROPDESCRIPTION, CFSTR_LOGICALPERFORMEDDROPEFFECT, CFSTR_PERFORMEDDROPEFFECT,
    CLSID_DragDropHelper, IDropTargetHelper,
};
use windows::core::{IUnknown, PCWSTR};

use super::{DropHandler, ours_format};
use crate::clipboard::Global;
use crate::shell::Owner;
use crate::win_util::{to_io, wide};

/// The window's drop target, registered until this is dropped.
pub struct Registration {
    hwnd: HWND,
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: a handle is only a number to the call: one of a window that is gone, or that
        // holds no target any more (winit revokes it as the window is destroyed), is refused.
        let _ = unsafe { RevokeDragDrop(self.hwnd) };
    }
}

pub fn register(owner: Owner, handler: Rc<dyn DropHandler>) -> io::Result<Registration> {
    let hwnd = owner
        .raw()
        .map(|hwnd| {
            // A handle is a number that names a window, not memory to read.
            HWND(std::ptr::without_provenance_mut::<c_void>(
                hwnd.get().cast_unsigned(),
            ))
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no window handle"))?;
    let ours = wide(OsStr::new(&ours_format()));
    let formats = Formats {
        ours: registered(PCWSTR(ours.as_ptr())),
        description: registered(CFSTR_DROPDESCRIPTION),
        performed: registered(CFSTR_PERFORMEDDROPEFFECT),
        logical: registered(CFSTR_LOGICALPERFORMEDDROPEFFECT),
    };
    // SAFETY: a class of the shell, made in this thread's apartment (winit initialized OLE on
    // it); without it no drag image follows the drag, which is no failure.
    let helper: Option<IDropTargetHelper> = unsafe {
        CoCreateInstance(
            &CLSID_DragDropHelper,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }
    .ok();
    let target = target::drop_target(hwnd, handler, helper, formats);
    // winit's target goes first. A window without one refuses the call, which is no matter.
    // SAFETY: `hwnd` names the app's window, on this thread.
    let _ = unsafe { RevokeDragDrop(hwnd) };
    // SAFETY: as above, in an apartment OLE is initialized in; the system holds a reference to
    // the target until it is revoked.
    unsafe { RegisterDragDrop(hwnd, &target) }.map_err(|e| to_io(&e))?;
    Ok(Registration { hwnd })
}

/// The number of the registered format `name`; 0, which no format has, when it cannot be
/// registered.
fn registered(name: PCWSTR) -> u16 {
    // SAFETY: `name` is a NUL-terminated string that lives through the call.
    let format = unsafe { RegisterClipboardFormatW(name) };
    // Registered formats are numbered 0xC000 to 0xFFFF.
    u16::try_from(format).unwrap_or(0)
}

/// The registered formats the target reads and writes; 0 for one that could not be registered.
#[derive(Clone, Copy, Debug)]
struct Formats {
    ours: u16,
    description: u16,
    performed: u16,
    logical: u16,
}

/// The drop target, in a module of its own because the `implement` macro writes code that trips
/// two pedantic lints.
#[allow(
    clippy::ref_as_ptr,
    clippy::inline_always,
    reason = "code generated by the `implement` macro"
)]
mod target {
    use std::cell::RefCell;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;
    use std::rc::Rc;

    use windows::Win32::Foundation::{HWND, POINT, POINTL};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::Com::IDataObject;
    use windows::Win32::System::Ole::{CF_HDROP, DROPEFFECT, IDropTarget, IDropTarget_Impl};
    use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Shell::IDropTargetHelper;
    use windows::core::{Ref, implement};

    use super::{Formats, put_block, read_effect, with_block, write_effect};
    use crate::clipboard::{global_bytes, hdrop_units};
    use crate::dnd::{
        DropEffect, DropHandler, DroppedItems, allowed_effects, client_point, description_bytes,
        drop_reply, effect_bits, key_mods, ours_in, right_button,
    };
    use crate::shell::WindowPoint;

    /// A drag of the system over the window, from its `DragEnter` on.
    struct Over {
        /// Its data object, for the drop description while it hovers.
        data: IDataObject,
        /// Its items; `None` for a drag without files, which the window does not take.
        items: Option<Rc<DroppedItems>>,
        /// The drag is one with the right button.
        right_button: bool,
        /// The drop description written last, so that an unchanged one is not written again.
        described: Option<Vec<u8>>,
    }

    #[implement(IDropTarget)]
    pub(super) struct DropTarget {
        hwnd: HWND,
        handler: Rc<dyn DropHandler>,
        helper: Option<IDropTargetHelper>,
        formats: Formats,
        drag: RefCell<Option<Over>>,
    }

    /// The window `hwnd`'s target, which hands the drags to `handler`.
    pub(super) fn drop_target(
        hwnd: HWND,
        handler: Rc<dyn DropHandler>,
        helper: Option<IDropTargetHelper>,
        formats: Formats,
    ) -> IDropTarget {
        DropTarget {
            hwnd,
            handler,
            helper,
            formats,
            drag: RefCell::new(None),
        }
        .into()
    }

    impl DropTarget {
        /// The screen point `point` of the system, in the window's logical pixels.
        fn window_point(&self, point: &POINTL) -> WindowPoint {
            let mut origin = POINT::default();
            // SAFETY: `hwnd` is the app's window, `origin` a live point the call writes.
            let _ = unsafe { ClientToScreen(self.hwnd, &raw mut origin) };
            // SAFETY: only reads the window's DPI; 0 for a window that is gone (scale 1).
            let dpi = unsafe { GetDpiForWindow(self.hwnd) };
            client_point(
                (point.x, point.y),
                (origin.x, origin.y),
                f64::from(dpi) / 96.0,
            )
        }

        /// The drag over the window at `point`, with the key state `state` and the effects
        /// `effect` allows on input: the handler's answer goes to `effect` for the system's
        /// pointer, and under the drag image. The answer's bits.
        fn hover(&self, state: u32, point: &POINTL, effect: *mut DROPEFFECT) -> u32 {
            let allowed = allowed_effects(read_effect(effect));
            let held = self.drag.borrow_mut().as_mut().map(|over| {
                over.right_button |= right_button(state);
                (over.items.clone(), over.data.clone())
            });
            let (answer, data) = match held {
                Some((Some(items), data)) => {
                    // Nothing of the target is borrowed while the handler runs.
                    let at = self.window_point(point);
                    let answer = self.handler.over(at, key_mods(state), allowed, &items);
                    (answer, Some(data))
                }
                Some((None, _)) | None => (None, None),
            };
            let bits = effect_bits(answer);
            write_effect(effect, bits);
            if let Some(data) = data {
                self.describe(&data, answer, &self.handler.hint());
            }
            bits
        }

        /// Writes under the drag image what a drop does (`effect`) and what the window says of it
        /// (`hint`), when it changed.
        fn describe(&self, data: &IDataObject, effect: Option<DropEffect>, hint: &str) {
            let bytes = description_bytes(effect, hint);
            {
                let mut drag = self.drag.borrow_mut();
                let Some(over) = drag.as_mut() else {
                    return;
                };
                if over.described.as_ref() == Some(&bytes) {
                    return;
                }
                over.described = Some(bytes.clone());
            }
            put_block(data, self.formats.description, &bytes);
        }

        /// The items of the data object `data`: its paths of `CF_HDROP` and, for a drag this
        /// process started, its id; `None` for a drag without files.
        fn items(&self, data: &IDataObject) -> Option<DroppedItems> {
            let units = with_block(data, CF_HDROP.0, |global| {
                // SAFETY: the data object's `CF_HDROP` block, which it keeps until the medium is
                // released after this.
                unsafe { hdrop_units(global) }.ok()
            })?;
            if units.is_empty() {
                return None;
            }
            let paths = units
                .iter()
                .map(|units| PathBuf::from(OsString::from_wide(units)))
                .collect();
            let ours = (self.formats.ours != 0)
                .then(|| {
                    with_block(data, self.formats.ours, |global| {
                        // SAFETY: as above, the block of the tag.
                        unsafe { global_bytes(global) }.ok()
                    })
                })
                .flatten()
                .and_then(|tag| ours_in(&tag, std::process::id()));
            Some(DroppedItems { paths, ours })
        }

        /// Shows the drag image at `point`, through the shell's helper, for `call`.
        fn helper(&self, point: POINTL, call: impl FnOnce(&IDropTargetHelper, &POINT)) {
            if let Some(helper) = &self.helper {
                call(
                    helper,
                    &POINT {
                        x: point.x,
                        y: point.y,
                    },
                );
            }
        }
    }

    impl IDropTarget_Impl for DropTarget_Impl {
        fn DragEnter(
            &self,
            pdataobj: Ref<'_, IDataObject>,
            grfkeystate: MODIFIERKEYS_FLAGS,
            pt: &POINTL,
            pdweffect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            let data = pdataobj.cloned();
            let items = data.as_ref().and_then(|data| self.items(data)).map(Rc::new);
            *self.drag.borrow_mut() = data.clone().map(|data| Over {
                data,
                items,
                right_button: right_button(grfkeystate.0),
                described: None,
            });
            let shown = self.hover(grfkeystate.0, pt, pdweffect);
            if let Some(data) = &data {
                self.helper(*pt, |helper, point| {
                    // SAFETY: the window, the drag's data object and a live point.
                    let _ = unsafe { helper.DragEnter(self.hwnd, data, point, DROPEFFECT(shown)) };
                });
            }
            Ok(())
        }

        fn DragOver(
            &self,
            grfkeystate: MODIFIERKEYS_FLAGS,
            pt: &POINTL,
            pdweffect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            let shown = self.hover(grfkeystate.0, pt, pdweffect);
            self.helper(*pt, |helper, point| {
                // SAFETY: a live point.
                let _ = unsafe { helper.DragOver(point, DROPEFFECT(shown)) };
            });
            Ok(())
        }

        fn DragLeave(&self) -> windows::core::Result<()> {
            let over = self.drag.borrow_mut().take();
            self.handler.leave();
            // The source's own description comes back.
            if let Some(over) = over
                && over.described.is_some()
            {
                put_block(
                    &over.data,
                    self.formats.description,
                    &description_bytes(None, ""),
                );
            }
            if let Some(helper) = &self.helper {
                // SAFETY: takes nothing.
                let _ = unsafe { helper.DragLeave() };
            }
            Ok(())
        }

        fn Drop(
            &self,
            pdataobj: Ref<'_, IDataObject>,
            grfkeystate: MODIFIERKEYS_FLAGS,
            pt: &POINTL,
            pdweffect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            let over = self.drag.borrow_mut().take();
            let data = pdataobj.cloned();
            let allowed = allowed_effects(read_effect(pdweffect));
            let right = over.is_some_and(|over| over.right_button) || right_button(grfkeystate.0);
            // Every path is read now: the data object is not used once the drop returns.
            let items = data.as_ref().and_then(|data| self.items(data));
            let done = if let Some(items) = items {
                let at = self.window_point(pt);
                DropHandler::drop(
                    &*self.handler,
                    at,
                    key_mods(grfkeystate.0),
                    allowed,
                    items,
                    right,
                )
            } else {
                self.handler.leave();
                None
            };
            let (reply, performed) = drop_reply(done);
            if let (Some(data), Some(performed)) = (&data, performed) {
                let bytes = performed.to_le_bytes();
                put_block(data, self.formats.performed, &bytes);
                put_block(data, self.formats.logical, &bytes);
            }
            write_effect(pdweffect, reply);
            if let Some(data) = &data {
                self.helper(*pt, |helper, point| {
                    // SAFETY: the drag's data object and a live point.
                    let _ = unsafe { helper.Drop(data, point, DROPEFFECT(reply)) };
                });
            }
            Ok(())
        }
    }
}

/// The effects `effect` holds; none for no pointer.
fn read_effect(effect: *mut DROPEFFECT) -> u32 {
    // SAFETY: the system hands a pointer to a live `DWORD` for the call, or none.
    unsafe { effect.as_ref() }.map_or(0, |effect| effect.0)
}

/// Writes `bits` to `effect`, unless there is no pointer.
fn write_effect(effect: *mut DROPEFFECT, bits: u32) {
    // SAFETY: as in `read_effect`.
    if let Some(effect) = unsafe { effect.as_mut() } {
        *effect = DROPEFFECT(bits);
    }
}

/// The description of a block of global memory in the format `format`.
fn hglobal_format(format: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: format,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0.cast_unsigned(),
    }
}

/// What `read` makes of the data object's block of `format` in global memory; `None` when it
/// holds none.
fn with_block<T>(
    data: &IDataObject,
    format: u16,
    read: impl FnOnce(HGLOBAL) -> Option<T>,
) -> Option<T> {
    let wanted = hglobal_format(format);
    // SAFETY: a live description of the format; the medium handed back is released below.
    let mut medium = unsafe { data.GetData(&raw const wanted) }.ok()?;
    let read = if medium.tymed == TYMED_HGLOBAL.0.cast_unsigned() {
        // SAFETY: the medium is global memory (`tymed`), the data object's until it is
        // released.
        read(unsafe { medium.u.hGlobal })
    } else {
        None
    };
    // SAFETY: the medium `GetData` handed over, released once.
    unsafe { ReleaseStgMedium(&raw mut medium) };
    read
}

/// Writes `bytes` on the data object as its block of `format` in global memory, which the data
/// object then owns; nothing when it cannot (a format that was not registered, a source that
/// takes nothing).
fn put_block(data: &IDataObject, format: u16, bytes: &[u8]) {
    if format == 0 {
        return;
    }
    let Ok(global) = Global::with(bytes) else {
        return;
    };
    let wanted = hglobal_format(format);
    let medium = STGMEDIUM {
        tymed: TYMED_HGLOBAL.0.cast_unsigned(),
        u: STGMEDIUM_0 {
            hGlobal: global.handle(),
        },
        pUnkForRelease: ManuallyDrop::new(None),
    };
    // SAFETY: a live description and medium; with `fRelease` the data object frees the block
    // once the call has worked, and a call that failed leaves it ours, which `global` frees.
    if unsafe { data.SetData(&raw const wanted, &raw const medium, true) }.is_ok() {
        global.given();
    }
}
