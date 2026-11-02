//! Drag and drop with other apps (PRD DND-05): what a drag allows, what a drop does, the
//! modifier keys held, and the items another app drops. The app's own drags and the system's
//! speak of them in these words, so that one rule chooses a drop's action for both.
//!
//! The window takes the system's drags through a drop target ([`register_drop_target`]), which
//! hands each step of a drag to the app's [`DropHandler`] on the window's thread:
//!
//! - **Windows:** an `IDropTarget` of the app's own takes the place of winit's on the window
//!   (`RevokeDragDrop`, then `RegisterDragDrop`; winit has initialized OLE on that thread). It
//!   reads the paths of `CF_HDROP` as the clipboard does, the tag of the app's own drag (the
//!   registered format [`ours_format`]), the keys of `grfKeyState` and the point of `POINTL`,
//!   which it turns into the window's logical pixels ([`client_point`]). The shell's drag image
//!   follows the drag (`IDropTargetHelper`), with what a drop would do written under it
//!   (`DropDescription`, [`DropHandler::hint`]). A drop answers by [`drop_reply`]: a move as
//!   the shell's own optimized move (performed: none, logically: a move), never in words the
//!   source would take as the order to delete the originals itself.
//! - **macOS:** a view of the app's own over the window's content (winit's view) takes the drags
//!   of files and lets every other event through to winit (`hitTest:` answers nil). It reads the
//!   file URLs of the drag's pasteboard as the clipboard reads them, byte for byte, the keys held
//!   (`NSEvent.modifierFlags`, `flag_mods`), the source's operation mask (`mask_effects`) and
//!   `draggingLocation`, which it names from the top ([`flipped_point`]). It answers each step
//!   with an operation the source allows but never the source's move (`answer_for`: a move is
//!   the generic operation, or a copy), and takes a drop only for a copy or a move the app's job
//!   does (`drop_taken`). winit's own drop target, the window, stays: it takes a drag over the
//!   title bar, and every drag should the view take none.
//! - **Elsewhere** (Linux, which is for development): `Unsupported`; winit tells the files
//!   dropped on the window.
//!
//! A drag of the app's rows that leaves the window goes on as the system's drag ([`drag_out`],
//! DND-05 보내기), which other apps take by the system's rules: `SHDoDragDrop` of the shell's data
//! object of the items on Windows, a dragging session of their file URLs on macOS; nothing
//! elsewhere ([`DRAGS_OUT`]). It carries the app's tag, so that the window's own drop target
//! knows it should it come back; and the app deletes nothing once it ends, whatever the taker
//! did.
//!
//! What the system says is turned into this crate's words by pure functions, which the tests
//! check on every system: the answers of a drop, the points, the tag of the app's own drag, the
//! keys, the effects (of a drop and of a drag out) and the words under the drag image.

use std::io;
use std::ops::BitOr;
use std::path::PathBuf;
use std::rc::Rc;

use crate::shell::{Owner, WindowPoint};

#[cfg(target_os = "macos")]
use self::macos as imp;
#[cfg(not(any(windows, target_os = "macos")))]
use self::other as imp;
#[cfg(windows)]
use self::windows as imp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(windows, target_os = "macos")))]
mod other;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub(crate) use self::macos::content_view;

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
    /// How many of its items the system's drag names in a way that gives no path to read safely,
    /// and are left out of `paths`: on macOS a file URL of another computer
    /// (`file://server/share/a`), one with an encoded `/`, or a Finder reference to an item that
    /// is gone, as a paste leaves them out; 0 on Windows, whose `CF_HDROP` gives every path. The
    /// window tells them as it tells the paths it does not take.
    pub skipped: usize,
}

/// What the window does with a drag of the system over it, step by step, on the window's
/// thread ([`register_drop_target`]). The system may call it from a loop of its own that runs
/// inside the app's code (a drag the app started, a menu), so it must not wait for anything the
/// app holds: what it cannot answer now it answers with nothing.
pub trait DropHandler {
    /// The drag carrying `items`, with the keys `mods` held and allowing `allowed`, is over the
    /// point `at` of the window (its first step there too): what a drop there would do, for the
    /// system's pointer; `None` for nothing.
    fn over(
        &self,
        at: WindowPoint,
        mods: DragMods,
        allowed: Effects,
        items: &DroppedItems,
    ) -> Option<DropEffect>;

    /// The drag left the window without a drop.
    fn leave(&self);

    /// The drag was dropped at `at`, with the right button (`right_button`) or not: what was
    /// done, for the system; `None` for nothing (and for a choice that comes later). A move is
    /// done by the app's own job, never by the app the items came from ([`drop_reply`]).
    fn drop(
        &self,
        at: WindowPoint,
        mods: DragMods,
        allowed: Effects,
        items: DroppedItems,
        right_button: bool,
    ) -> Option<DropEffect>;

    /// What the window says of the drop the last [`DropHandler::over`] answered, or why there is
    /// none: the system may show it under its pointer (Windows' drop description). Empty for
    /// nothing to say.
    fn hint(&self) -> DropHint {
        DropHint::default()
    }
}

/// What the window says of a drop, for the words the system shows under its drag image: the
/// `message`, in which `%1` stands for `insert`, a folder's name, which the system shows as it is
/// (a `%1` in a name stays a `%1`), as File Explorer's "이동 → %1" shows the folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct DropHint {
    pub message: String,
    pub insert: String,
}

/// The window's drop target, from [`register_drop_target`]; dropping it takes it away from the
/// window.
pub struct DropRegistration {
    /// Held for its drop.
    _registration: imp::Registration,
}

impl std::fmt::Debug for DropRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DropRegistration").finish_non_exhaustive()
    }
}

/// Makes the window `owner` take the system's drags, which `handler` answers, until the
/// registration is dropped. Call it on the window's thread, for a window that exists; again for
/// a window made anew (a new handle). `InvalidInput` for an owner without the handle this
/// system needs; `Unsupported` on Linux.
pub fn register_drop_target(
    owner: Owner,
    handler: Rc<dyn DropHandler>,
) -> io::Result<DropRegistration> {
    imp::register(owner, handler).map(|registration| DropRegistration {
        _registration: registration,
    })
}

/// Whether this system carries a drag of the app's rows on out of the window ([`drag_out`]):
/// Windows and macOS. Elsewhere (Linux) a drag of rows stays the window's, and one let go outside
/// it drops nothing.
pub const DRAGS_OUT: bool = imp::DRAGS_OUT;

/// Carries a drag of the app's rows on out of the window `owner` as the system's drag of the
/// files and folders at `paths` (DND-05 보내기), which another app takes as the system's rules
/// say (File Explorer and Finder by their keys, a browser's upload, a mail's attachment). The
/// drag allows `allowed`, and carries the tag of the app's drag `ours` ([`ours_tag`], or on
/// macOS the app's own source object), so that the window's own drop target knows it as this
/// drag of rows should it come back over the window ([`DroppedItems::ours`]). `done` hears what
/// the app that took it did, once, when the drag ends: `None` for nothing (and for a drop back
/// on the window, whose own job the window made as it took it).
///
/// The app deletes nothing after a drag out, whatever `done` hears (a move included): a move is
/// the taker's to finish; a taker that only copied leaves a copy.
///
/// Call it on the window's thread, with nothing borrowed that the window's drop target needs,
/// and never from inside a call of that drop target:
///
/// - **Windows:** the drag runs inside this call, in the system's own loop (`SHDoDragDrop`,
///   with the shell's drag image), which calls the window's drop target as the drag passes over
///   the window; `done` is called before it returns. The item ids of the paths go (as File
///   Explorer's own drag carries them), with the tag in the format [`ours_format`]; a path that
///   names no item (any more) is left out.
/// - **macOS:** the drag begins (`beginDraggingSessionWithItems:event:source:` on the window's
///   view, a file URL for each path, byte for byte, with its icon) and runs on in the app's event
///   loop; `done` is called as it ends.
///
/// `Unsupported` where [`DRAGS_OUT`] is false; `InvalidInput` for no paths or an owner without
/// the handle this system needs; another error for a drag that could not begin. `done` is never
/// called when this fails.
pub fn drag_out(
    owner: Owner,
    paths: &[PathBuf],
    allowed: Effects,
    ours: u64,
    done: Box<dyn FnOnce(Option<DropEffect>)>,
) -> io::Result<()> {
    if DRAGS_OUT && paths.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "nothing to drag",
        ));
    }
    imp::drag_out(owner, paths, allowed, ours, done)
}

/// `DROPEFFECT_NONE`, `DROPEFFECT_COPY`, `DROPEFFECT_MOVE` and `DROPEFFECT_LINK`.
pub const DROPEFFECT_NONE: u32 = 0;
pub const DROPEFFECT_COPY: u32 = 1;
pub const DROPEFFECT_MOVE: u32 = 2;
pub const DROPEFFECT_LINK: u32 = 4;

/// What a drop target answers the system for a drop ([`drop_reply`]): the effect `Drop` returns,
/// and the effects it writes on the data object as `Performed DropEffect` and as `Logical
/// Performed DropEffect` (`None`: nothing written).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DropReply {
    pub effect: u32,
    pub performed: Option<u32>,
    pub logical: Option<u32>,
}

/// What a drop target answers the system for a drop that did `done` ([`DropReply`]).
///
/// A move is done by the app's own job, which deletes each original once its copy is done, as
/// the shell does after an "optimized move": `Drop` returns `DROPEFFECT_NONE`, `Performed
/// DropEffect` is `DROPEFFECT_NONE` and `Logical Performed DropEffect` is `DROPEFFECT_MOVE`. The
/// source must never read "move" from `Drop`'s effect or from `Performed DropEffect`: either
/// tells it that the target only copied, and it would delete the originals itself, at once,
/// before the app's copy is done (an "unoptimized move"). A copy answers `DROPEFFECT_COPY` in all
/// three. Nothing done (a refusal, the menu of 놓을 때 묻기, whose choice comes later, a link,
/// which the app does not make) answers `DROPEFFECT_NONE` and writes nothing.
pub fn drop_reply(done: Option<DropEffect>) -> DropReply {
    match done {
        Some(DropEffect::Move) => DropReply {
            effect: DROPEFFECT_NONE,
            performed: Some(DROPEFFECT_NONE),
            logical: Some(DROPEFFECT_MOVE),
        },
        Some(DropEffect::Copy) => DropReply {
            effect: DROPEFFECT_COPY,
            performed: Some(DROPEFFECT_COPY),
            logical: Some(DROPEFFECT_COPY),
        },
        Some(DropEffect::Link) | None => DropReply {
            effect: DROPEFFECT_NONE,
            performed: None,
            logical: None,
        },
    }
}

/// The point of the window's content under the screen point `screen` (physical pixels, which
/// may be negative on a monitor left of or above the main one), for content whose top left
/// corner is at `client_origin` on the screen, shown at `scale` physical pixels for each logical
/// one: in the window's logical pixels. A `scale` that is not a positive number counts as 1.
pub fn client_point(screen: (i32, i32), client_origin: (i32, i32), scale: f64) -> WindowPoint {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    // In i64, so that the far ends of the screen do not overflow.
    let along = |at: i32, origin: i32| {
        let physical = i64::from(at) - i64::from(origin);
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            reason = "a point of a screen is a few thousand pixels, far within f32"
        )]
        let logical = (physical as f64 / scale) as f32;
        logical
    };
    WindowPoint {
        x: along(screen.0, client_origin.0),
        y: along(screen.1, client_origin.1),
    }
}

/// The screen point, in physical pixels, of the point `at` of the window's content (its logical
/// pixels), for content whose top left corner is at `client_origin` on the screen, shown at
/// `scale` physical pixels for each logical one: the other way round of [`client_point`], to
/// the nearest pixel, where the system wants a place on the screen (a menu of its own). The far
/// ends of the screen stay within `i32`; a `scale` that is not a positive number counts as 1,
/// and a point that is no number as 0.
pub fn screen_point(at: WindowPoint, client_origin: (i32, i32), scale: f64) -> (i32, i32) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let along = |at: f32, origin: i32| {
        let logical = if at.is_nan() { 0.0 } else { f64::from(at) };
        // In f64, which holds every `i32` exactly; `as` keeps the far ends at `i32`'s.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "clamped to i32's range first; `as` saturates the infinite ends"
        )]
        let physical = (logical * scale + f64::from(origin))
            .round()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
        physical
    };
    (along(at.x, client_origin.0), along(at.y, client_origin.1))
}

/// The point `point` of a macOS view `view_height` points high, whose y grows upward from its
/// bottom, as the window's content names it: y from the top.
pub fn flipped_point(view_height: f64, point: (f64, f64)) -> WindowPoint {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a point of a window is a few thousand points, far within f32"
    )]
    let (x, y) = (point.0 as f32, (view_height - point.1) as f32);
    WindowPoint { x, y }
}

/// The tag a drag of the app's own rows carries through the system (in the format
/// [`ours_format`]): the id of the process that started it, then the drag's id, each in little
/// endian. Another running copy of the app reads the tag too, so the process tells whose drag it
/// is.
pub fn ours_tag(pid: u32, id: u64) -> [u8; 12] {
    let mut tag = [0; 12];
    tag[..4].copy_from_slice(&pid.to_le_bytes());
    tag[4..].copy_from_slice(&id.to_le_bytes());
    tag
}

/// The process and the drag of the tag `bytes` ([`ours_tag`]); `None` for bytes too short to
/// hold one. Bytes after the tag are left alone (the system may round a block up).
pub fn read_ours_tag(bytes: &[u8]) -> Option<(u32, u64)> {
    let pid = <[u8; 4]>::try_from(bytes.get(..4)?).ok()?;
    let id = <[u8; 8]>::try_from(bytes.get(4..12)?).ok()?;
    Some((u32::from_le_bytes(pid), u64::from_le_bytes(id)))
}

/// The drag id of the tag `bytes` when the process `pid` started the drag, for
/// [`DroppedItems::ours`]; `None` for another process's drag and for no tag.
pub fn ours_in(bytes: &[u8], pid: u32) -> Option<u64> {
    read_ours_tag(bytes).and_then(|(started, id)| (started == pid).then_some(id))
}

/// The name of the format a drag of the app's own rows carries its tag in ([`ours_tag`]).
pub fn ours_format() -> String {
    format!("{}.Drag", rf_core::identity::DATA_DIR_NAME)
}

/// `MK_RBUTTON`, `MK_SHIFT`, `MK_CONTROL` and `MK_ALT` of a drop target's key state.
const MK_RBUTTON: u32 = 0x2;
const MK_SHIFT: u32 = 0x4;
const MK_CONTROL: u32 = 0x8;
const MK_ALT: u32 = 0x20;

/// The modifier keys of the key state `state` (`grfKeyState`): Ctrl, Shift and Alt. The state
/// does not tell the Windows key.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn key_mods(state: u32) -> DragMods {
    DragMods {
        ctrl: state & MK_CONTROL != 0,
        shift: state & MK_SHIFT != 0,
        alt: state & MK_ALT != 0,
        logo: false,
    }
}

/// Whether the key state `state` holds the right button: the drag is a drag with it.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn right_button(state: u32) -> bool {
    state & MK_RBUTTON != 0
}

/// The effects the `DROPEFFECT` bits `bits` allow.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn allowed_effects(bits: u32) -> Effects {
    let mut allowed = Effects(0);
    for (bit, effect) in [
        (DROPEFFECT_COPY, Effects::COPY),
        (DROPEFFECT_MOVE, Effects::MOVE),
        (DROPEFFECT_LINK, Effects::LINK),
    ] {
        if bits & bit != 0 {
            allowed = allowed | effect;
        }
    }
    allowed
}

/// The `DROPEFFECT` of `effect` for the system's pointer while the drag hovers.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn effect_bits(effect: Option<DropEffect>) -> u32 {
    match effect {
        Some(DropEffect::Copy) => DROPEFFECT_COPY,
        Some(DropEffect::Move) => DROPEFFECT_MOVE,
        Some(DropEffect::Link) => DROPEFFECT_LINK,
        None => DROPEFFECT_NONE,
    }
}

/// The `DROPEFFECT` bits of the effects `allowed`, which a drag the app starts allows
/// ([`drag_out`]): the other way round of `allowed_effects`.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn effects_bits(allowed: Effects) -> u32 {
    [
        (DropEffect::Copy, DROPEFFECT_COPY),
        (DropEffect::Move, DROPEFFECT_MOVE),
        (DropEffect::Link, DROPEFFECT_LINK),
    ]
    .into_iter()
    .filter(|&(effect, _)| allowed.contains(effect))
    .fold(DROPEFFECT_NONE, |bits, (_, bit)| bits | bit)
}

/// What the app that took a drag the app started did with it, as the system's drag tells as it
/// ends (the effect `SHDoDragDrop` gives, which is what the drop target answered): a move, else a
/// copy, else a link; `None` for nothing (a drag let go where nothing took it, or Esc). Only for
/// the app's ears: it deletes nothing, whatever this says ([`drag_out`]).
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn performed_effect(bits: u32) -> Option<DropEffect> {
    [
        (DROPEFFECT_MOVE, DropEffect::Move),
        (DROPEFFECT_COPY, DropEffect::Copy),
        (DROPEFFECT_LINK, DropEffect::Link),
    ]
    .into_iter()
    .find_map(|(bit, effect)| (bits & bit != 0).then_some(effect))
}

/// `NSDragOperationCopy`, `NSDragOperationLink`, `NSDragOperationGeneric`,
/// `NSDragOperationMove` and `NSDragOperationDelete`.
const NS_DRAG_COPY: usize = 1;
const NS_DRAG_LINK: usize = 1 << 1;
const NS_DRAG_GENERIC: usize = 1 << 2;
const NS_DRAG_MOVE: usize = 1 << 4;
const NS_DRAG_DELETE: usize = 1 << 5;

/// `NSEventModifierFlagShift`, `…Control`, `…Option` and `…Command`.
const NS_SHIFT: usize = 1 << 17;
const NS_CONTROL: usize = 1 << 18;
const NS_OPTION: usize = 1 << 19;
const NS_COMMAND: usize = 1 << 20;

/// The modifier keys of the flags `flags` (`NSEvent.modifierFlags`): ⌃ is `ctrl`, ⇧ `shift`, ⌥
/// `alt` and ⌘ `logo`. Caps Lock, fn and the bits of the left or right key leave them alone.
#[cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, reason = "used by macOS code; tested on every OS")
)]
fn flag_mods(flags: usize) -> DragMods {
    DragMods {
        ctrl: flags & NS_CONTROL != 0,
        shift: flags & NS_SHIFT != 0,
        alt: flags & NS_OPTION != 0,
        logo: flags & NS_COMMAND != 0,
    }
}

/// The effects the source's operation mask `mask` (`draggingSourceOperationMask`) allows: copy
/// for its copy; a move for its move or its generic operation (⌘ narrows Finder's mask to the
/// generic one), but only where the app can answer the move without telling the source of a move
/// ([`answer_for`]), so a source that allows nothing but its move allows the app none; a link for
/// its link.
#[cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, reason = "used by macOS code; tested on every OS")
)]
fn mask_effects(mask: usize) -> Effects {
    let mut allowed = Effects(0);
    if mask & NS_DRAG_COPY != 0 {
        allowed = allowed | Effects::COPY;
    }
    if mask & (NS_DRAG_MOVE | NS_DRAG_GENERIC) != 0 && answer_for(Some(DropEffect::Move), mask) != 0
    {
        allowed = allowed | Effects::MOVE;
    }
    if mask & NS_DRAG_LINK != 0 {
        allowed = allowed | Effects::LINK;
    }
    allowed
}

/// What the source of a drag hears of the app's choice `chosen`, for a source whose mask is
/// `mask` (an `NSDragOperation`): what the pointer shows while the drag hovers, and what the
/// source learns of a drop the app takes ([`drop_taken`]). Never its move, which a source may
/// read as "moved, delete the originals" while the app's own copy still runs: a copy is a copy;
/// a move the app's job makes is the generic operation (the destination's own), or, for a source
/// that allows no generic one, a copy, or else nothing (the drop is refused); nothing is nothing.
/// A link, which the app does not make, is nothing too.
#[cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, reason = "used by macOS code; tested on every OS")
)]
fn answer_for(chosen: Option<DropEffect>, mask: usize) -> usize {
    match chosen {
        Some(DropEffect::Copy) => NS_DRAG_COPY,
        Some(DropEffect::Move) if mask & NS_DRAG_GENERIC != 0 => NS_DRAG_GENERIC,
        Some(DropEffect::Move) if mask & NS_DRAG_COPY != 0 => NS_DRAG_COPY,
        Some(DropEffect::Move | DropEffect::Link) | None => 0,
    }
}

/// What `performDragOperation:` answers for a drop that did `done`: `YES`, the drop is taken,
/// only for a copy or a move the app's job does; then the source learns the operation the last
/// hover answered ([`answer_for`], never a move). `NO` for nothing done (a refusal, a link, which
/// the app does not make, and the menu of 놓을 때 묻기, whose choice comes later): the source
/// learns of no operation, and the drag image slides back.
#[cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, reason = "used by macOS code; tested on every OS")
)]
fn drop_taken(done: Option<DropEffect>) -> bool {
    matches!(done, Some(DropEffect::Copy | DropEffect::Move))
}

/// What the app's own drag source allows of the effects `allowed`, as `AppKit` asks it for the
/// place the drag is over (`draggingSession:sourceOperationMaskForDraggingContext:`): out of the
/// app (`within` false), as Finder's own drags do, a copy, a move, its generic operation (the
/// taker's own, which Finder takes for a move) and a link, for the taker to choose from by its
/// keys; within the app (back over the window), a move or a copy alone, which the window's drop
/// makes ([`mask_effects`]), with the generic operation for the move: the window answers a move
/// with it ([`answer_for`]), and the pointer keeps the move's arrow rather than a copy's "+". The
/// app is the source there and deletes nothing whatever it hears.
#[cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, reason = "used by macOS code; tested on every OS")
)]
fn source_mask(allowed: Effects, within: bool) -> usize {
    let mut mask = 0;
    if allowed.contains(DropEffect::Copy) {
        mask |= NS_DRAG_COPY;
    }
    if allowed.contains(DropEffect::Move) {
        mask |= NS_DRAG_MOVE | NS_DRAG_GENERIC;
    }
    if allowed.contains(DropEffect::Link) && !within {
        mask |= NS_DRAG_LINK;
    }
    mask
}

/// What the app that took a drag the app started did with it, as its source hears as it ends
/// (`draggingSession:endedAtPoint:operation:`): a move (the source's move, the generic operation,
/// which the taker made its own, or the Trash's delete), else a copy, else a link; `None` for
/// nothing. Only for the app's ears: it deletes nothing, whatever this says ([`drag_out`]).
#[cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, reason = "used by macOS code; tested on every OS")
)]
fn ended_effect(operation: usize) -> Option<DropEffect> {
    [
        (
            NS_DRAG_MOVE | NS_DRAG_GENERIC | NS_DRAG_DELETE,
            DropEffect::Move,
        ),
        (NS_DRAG_COPY, DropEffect::Copy),
        (NS_DRAG_LINK, DropEffect::Link),
    ]
    .into_iter()
    .find_map(|(bits, effect)| (operation & bits != 0).then_some(effect))
}

/// `DROPIMAGE_INVALID`, `DROPIMAGE_NONE`, `DROPIMAGE_COPY`, `DROPIMAGE_MOVE`.
const DROPIMAGE_INVALID: i32 = -1;
const DROPIMAGE_NONE: i32 = 0;
const DROPIMAGE_COPY: i32 = 1;
const DROPIMAGE_MOVE: i32 = 2;

/// The length of `DROPDESCRIPTION`'s strings, in UTF-16 units with their NUL (`MAX_PATH`).
const DESCRIPTION_UNITS: usize = 260;

/// The `DropDescription` a drop target writes for a drag over a place where a drop does
/// `effect` and the window says `hint`: a `DROPDESCRIPTION` (packed: its image type, then the
/// message and the insert, each 260 UTF-16 units ended by a NUL). The image is the effect's (a
/// refusal's "none" when the window says why), or `DROPIMAGE_INVALID`, which gives the source's
/// own back, where the window says nothing. The message and the insert are the hint's, each cut
/// at a whole character to fit.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn description_bytes(effect: Option<DropEffect>, hint: &DropHint) -> Vec<u8> {
    let image = match effect {
        _ if hint.message.is_empty() => DROPIMAGE_INVALID,
        Some(DropEffect::Copy) => DROPIMAGE_COPY,
        Some(DropEffect::Move) => DROPIMAGE_MOVE,
        Some(DropEffect::Link) | None => DROPIMAGE_NONE,
    };
    let mut bytes = Vec::with_capacity(4 + DESCRIPTION_UNITS * 4);
    bytes.extend_from_slice(&image.to_le_bytes());
    for text in [&hint.message, &hint.insert] {
        for unit in description_units(text) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
    }
    bytes
}

/// `text` as one of `DROPDESCRIPTION`'s strings: its UTF-16 units, cut at a whole character to
/// leave room for the NUL, then NULs to the end.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by Windows code; tested on every OS")
)]
fn description_units(text: &str) -> [u16; DESCRIPTION_UNITS] {
    let mut units = [0; DESCRIPTION_UNITS];
    let mut length = 0;
    for character in text.chars() {
        let mut encoded = [0; 2];
        let encoded = character.encode_utf16(&mut encoded);
        if length + encoded.len() >= DESCRIPTION_UNITS {
            break;
        }
        units[length..length + encoded.len()].copy_from_slice(encoded);
        length += encoded.len();
    }
    units
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

    #[test]
    fn a_drop_never_answers_a_move_that_the_source_would_finish_by_deleting() {
        // Review Focus: File Explorer deletes the originals at once when `Drop` or `Performed
        // DropEffect` says "move" (an unoptimized move); the app's job deletes each one only
        // once its copy is done, as the shell's own optimized move does.
        assert_eq!(
            drop_reply(Some(DropEffect::Move)),
            DropReply {
                effect: DROPEFFECT_NONE,
                performed: Some(DROPEFFECT_NONE),
                logical: Some(DROPEFFECT_MOVE),
            }
        );
        assert_eq!(
            drop_reply(Some(DropEffect::Copy)),
            DropReply {
                effect: DROPEFFECT_COPY,
                performed: Some(DROPEFFECT_COPY),
                logical: Some(DROPEFFECT_COPY),
            }
        );
        // The menu of 놓을 때 묻기 and a refusal: nothing done yet, nothing written.
        let nothing = DropReply {
            effect: DROPEFFECT_NONE,
            performed: None,
            logical: None,
        };
        assert_eq!(drop_reply(None), nothing);
        assert_eq!(drop_reply(Some(DropEffect::Link)), nothing);
        for done in [None, Some(DropEffect::Copy), Some(DropEffect::Move)] {
            let reply = drop_reply(done);
            assert_ne!(reply.effect, DROPEFFECT_MOVE, "{done:?}");
            assert_ne!(reply.performed, Some(DROPEFFECT_MOVE), "{done:?}");
        }
    }

    #[test]
    fn a_point_of_the_screen_is_named_in_logical_pixels_of_the_window_s_content() {
        let at = |screen, origin, scale| {
            let point = client_point(screen, origin, scale);
            (point.x, point.y)
        };
        assert_eq!(at((300, 200), (100, 50), 1.0), (200.0, 150.0));
        // 125% and 150%: the same place in the window, wherever its content starts.
        assert_eq!(at((500, 375), (250, 125), 1.25), (200.0, 200.0));
        assert_eq!(at((750, 450), (150, 150), 1.5), (400.0, 200.0));
        // A monitor left of the main one, and one above it: negative screen coordinates.
        assert_eq!(at((-1000, 300), (-1600, 150), 1.5), (400.0, 100.0));
        assert_eq!(at((-500, -200), (-1700, -600), 1.25), (960.0, 320.0));
        assert_eq!(at((-10, -10), (-10, -10), 1.0), (0.0, 0.0));
        // Left of and above the content: negative window points, which name no place.
        assert_eq!(at((90, 40), (100, 50), 1.0), (-10.0, -10.0));
        // A scale the system did not tell counts as 1.
        assert_eq!(at((300, 200), (100, 50), 0.0), (200.0, 150.0));
        assert_eq!(at((300, 200), (100, 50), f64::NAN), (200.0, 150.0));
        // The far ends of the screen do not overflow.
        assert_eq!(
            at((i32::MAX, i32::MIN), (i32::MIN, i32::MAX), 1.0),
            (4_294_967_295.0, -4_294_967_295.0)
        );
    }

    #[test]
    fn a_point_of_the_window_s_content_is_named_in_physical_pixels_of_the_screen() {
        let at = |x, y| WindowPoint { x, y };
        assert_eq!(screen_point(at(200.0, 150.0), (100, 50), 1.0), (300, 200));
        // 125% and 150%: the content's origin is where the system says, already in pixels.
        assert_eq!(screen_point(at(200.0, 200.0), (250, 125), 1.25), (500, 375));
        assert_eq!(screen_point(at(400.0, 200.0), (150, 150), 1.5), (750, 450));
        // A monitor left of the main one, and one above it: negative screen coordinates.
        assert_eq!(
            screen_point(at(400.0, 100.0), (-1600, 150), 1.5),
            (-1000, 300)
        );
        assert_eq!(
            screen_point(at(960.0, 320.0), (-1700, -600), 1.25),
            (-500, -200)
        );
        assert_eq!(screen_point(at(0.0, 0.0), (-10, -10), 1.0), (-10, -10));
        // A fraction of a logical pixel goes to the nearest physical one.
        assert_eq!(screen_point(at(10.3, 10.7), (0, 0), 1.0), (10, 11));
        assert_eq!(screen_point(at(10.2, 10.6), (0, 0), 1.25), (13, 13));
        // The other way round of `client_point`, at every scale and on every monitor.
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            for origin in [(0, 0), (-1920, 0), (-2560, -1440), (1920, 200)] {
                for point in [
                    at(0.0, 0.0),
                    at(12.0, 34.0),
                    at(799.2, 599.6),
                    at(-8.0, -8.0),
                ] {
                    let screen = screen_point(point, origin, scale);
                    let back = client_point(screen, origin, scale);
                    let pixel = 1.0 / scale;
                    assert!(
                        f64::from((back.x - point.x).abs()) <= pixel / 2.0 + 1e-4
                            && f64::from((back.y - point.y).abs()) <= pixel / 2.0 + 1e-4,
                        "{point:?} {origin:?} {scale}: {screen:?} {back:?}"
                    );
                }
            }
        }
        // A scale the system did not tell counts as 1, and no number as 0.
        assert_eq!(screen_point(at(200.0, 150.0), (100, 50), 0.0), (300, 200));
        assert_eq!(
            screen_point(at(200.0, 150.0), (100, 50), f64::NAN),
            (300, 200)
        );
        assert_eq!(
            screen_point(at(f32::NAN, 150.0), (100, 50), 1.0),
            (100, 200)
        );
        // The far ends of the screen stay within its numbers.
        assert_eq!(
            screen_point(at(f32::MAX, -f32::MAX), (i32::MAX, i32::MIN), 1.0),
            (i32::MAX, i32::MIN)
        );
        assert_eq!(
            screen_point(at(f32::INFINITY, f32::NEG_INFINITY), (0, 0), 1.5),
            (i32::MAX, i32::MIN)
        );
    }

    #[test]
    fn a_point_of_a_macos_view_is_named_from_the_top() {
        let flipped = flipped_point(600.0, (120.5, 600.0));
        assert_eq!((flipped.x, flipped.y), (120.5, 0.0));
        let flipped = flipped_point(600.0, (0.0, 0.0));
        assert_eq!((flipped.x, flipped.y), (0.0, 600.0));
        let flipped = flipped_point(812.5, (40.0, 212.25));
        assert_eq!((flipped.x, flipped.y), (40.0, 600.25));
    }

    #[test]
    fn the_tag_of_the_app_s_own_drag_names_its_process_and_its_drag() {
        let tag = ours_tag(4242, 0x0102_0304_0506_0708);
        assert_eq!(read_ours_tag(&tag), Some((4242, 0x0102_0304_0506_0708)));
        assert_eq!(
            tag,
            [
                0x92, 0x10, 0, 0, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01
            ]
        );
        // The system may hand back a block longer than the tag.
        let mut rounded = tag.to_vec();
        rounded.extend([0xAA; 4]);
        assert_eq!(read_ours_tag(&rounded), Some((4242, 0x0102_0304_0506_0708)));
        assert_eq!(read_ours_tag(&tag[..11]), None);
        assert_eq!(read_ours_tag(&[]), None);
        // Only this process's drag is its own: another running copy's is another app's.
        assert_eq!(ours_in(&tag, 4242), Some(0x0102_0304_0506_0708));
        assert_eq!(ours_in(&tag, 4243), None);
        assert_eq!(ours_in(&tag[..4], 4242), None);
        assert_eq!(
            ours_in(&ours_tag(u32::MAX, u64::MAX), u32::MAX),
            Some(u64::MAX)
        );
    }

    #[test]
    fn the_key_state_of_a_drag_names_its_keys_and_its_button() {
        assert_eq!(key_mods(0), DragMods::default());
        // The left button alone, as during any drag with it.
        assert_eq!(key_mods(0x1), DragMods::default());
        assert!(!right_button(0x1));
        assert_eq!(
            key_mods(MK_CONTROL),
            DragMods {
                ctrl: true,
                ..DragMods::default()
            }
        );
        assert_eq!(
            key_mods(MK_SHIFT | MK_ALT | MK_RBUTTON),
            DragMods {
                shift: true,
                alt: true,
                ..DragMods::default()
            }
        );
        assert!(right_button(MK_SHIFT | MK_ALT | MK_RBUTTON));
        assert!(!right_button(MK_CONTROL | MK_SHIFT | MK_ALT));
        assert_eq!(
            key_mods(MK_CONTROL | MK_SHIFT | MK_ALT),
            DragMods {
                ctrl: true,
                shift: true,
                alt: true,
                logo: false,
            }
        );
    }

    #[test]
    fn the_effects_of_a_drag_and_of_its_pointer_are_the_system_s_bits() {
        assert_eq!(allowed_effects(DROPEFFECT_COPY), Effects::COPY);
        assert_eq!(
            allowed_effects(DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK),
            Effects::ALL
        );
        // File Explorer's scroll bit, and bits of nothing the app knows, allow nothing more.
        assert_eq!(
            allowed_effects(DROPEFFECT_MOVE | 0x8000_0000),
            Effects::MOVE
        );
        assert!(!allowed_effects(DROPEFFECT_NONE).contains(DropEffect::Copy));
        assert!(!allowed_effects(DROPEFFECT_NONE).contains(DropEffect::Move));
        assert_eq!(effect_bits(Some(DropEffect::Copy)), DROPEFFECT_COPY);
        assert_eq!(effect_bits(Some(DropEffect::Move)), DROPEFFECT_MOVE);
        assert_eq!(effect_bits(Some(DropEffect::Link)), DROPEFFECT_LINK);
        assert_eq!(effect_bits(None), DROPEFFECT_NONE);
    }

    #[test]
    fn the_flags_of_a_macos_drag_name_its_keys() {
        assert_eq!(flag_mods(0), DragMods::default());
        // ⌥ copies, ⌘ moves, ⌥⌘ links in Finder; ⌃ and ⇧ are told as they are.
        assert_eq!(
            flag_mods(1 << 19),
            DragMods {
                alt: true,
                ..DragMods::default()
            }
        );
        assert_eq!(
            flag_mods(1 << 20),
            DragMods {
                logo: true,
                ..DragMods::default()
            }
        );
        assert_eq!(
            flag_mods((1 << 19) | (1 << 20)),
            DragMods {
                alt: true,
                logo: true,
                ..DragMods::default()
            }
        );
        assert_eq!(
            flag_mods((1 << 17) | (1 << 18)),
            DragMods {
                shift: true,
                ctrl: true,
                ..DragMods::default()
            }
        );
        assert_eq!(
            flag_mods(NS_SHIFT | NS_CONTROL | NS_OPTION | NS_COMMAND),
            DragMods {
                ctrl: true,
                shift: true,
                alt: true,
                logo: true,
            }
        );
        // Caps Lock, the numeric pad, help, fn, and the bits of the left or right key (the low
        // 16) are no modifier keys of a drag.
        let others = (1 << 16) | (1 << 21) | (1 << 22) | (1 << 23) | 0x20 | 0x0100;
        assert_eq!(flag_mods(others), DragMods::default());
        assert_eq!(
            flag_mods(others | (1 << 20)),
            DragMods {
                logo: true,
                ..DragMods::default()
            }
        );
    }

    #[test]
    fn the_operations_a_macos_drag_allows_are_the_app_s_effects() {
        assert_eq!(mask_effects(1), Effects::COPY);
        // ⌘ narrows Finder's mask to the generic operation: a move.
        assert_eq!(mask_effects(4), Effects::MOVE);
        assert_eq!(mask_effects(4 | 16), Effects::MOVE);
        // A move the app answers as a copy, the source allowing no generic operation.
        assert_eq!(mask_effects(1 | 16), Effects::COPY | Effects::MOVE);
        // A source that allows nothing but its move allows the app no move: the app never
        // answers one.
        assert_eq!(mask_effects(16), Effects(0));
        assert_eq!(mask_effects(2 | 16), Effects::LINK);
        assert_eq!(mask_effects(2), Effects::LINK);
        assert_eq!(mask_effects(1 | 2 | 4 | 16), Effects::ALL);
        // `NSDragOperationEvery`.
        assert_eq!(mask_effects(usize::MAX), Effects::ALL);
        // Private and delete allow nothing the app does.
        assert_eq!(mask_effects(8 | 32), Effects(0));
        assert_eq!(mask_effects(0), Effects(0));
    }

    #[test]
    fn a_macos_drag_s_source_never_hears_of_a_move() {
        let finder = 1 | 2 | 4 | 16;
        assert_eq!(answer_for(Some(DropEffect::Copy), finder), 1);
        assert_eq!(answer_for(Some(DropEffect::Copy), 1), 1);
        // A move is the destination's own operation, never the source's move.
        assert_eq!(answer_for(Some(DropEffect::Move), finder), 4);
        assert_eq!(answer_for(Some(DropEffect::Move), usize::MAX), 4);
        // ⌘ held: Finder allows only its generic operation.
        assert_eq!(answer_for(Some(DropEffect::Move), 4), 4);
        assert_eq!(answer_for(Some(DropEffect::Move), 4 | 16), 4);
        // No generic operation: a copy, as far as the source knows; the app's job still moves.
        assert_eq!(answer_for(Some(DropEffect::Move), 1 | 16), 1);
        // A source that allows only its move cannot drop into the app.
        assert_eq!(answer_for(Some(DropEffect::Move), 16), 0);
        assert_eq!(answer_for(Some(DropEffect::Move), 2 | 16), 0);
        // Nothing, and a link, which the app does not make: `NSDragOperationNone`.
        assert_eq!(answer_for(None, finder), 0);
        assert_eq!(answer_for(None, 0), 0);
        assert_eq!(answer_for(Some(DropEffect::Link), finder), 0);
        let masks = [
            0,
            1,
            2,
            4,
            8,
            16,
            32,
            1 | 16,
            2 | 16,
            4 | 16,
            finder,
            usize::MAX,
        ];
        for mask in masks {
            for chosen in [
                None,
                Some(DropEffect::Copy),
                Some(DropEffect::Move),
                Some(DropEffect::Link),
            ] {
                // Never a move, nor more than one operation.
                let answer = answer_for(chosen, mask);
                assert!(
                    [0, 1, 4].contains(&answer),
                    "{chosen:?} {mask:#x}: {answer}"
                );
            }
            // The app is told a move is allowed only where it can answer one.
            if mask_effects(mask).contains(DropEffect::Move) {
                assert_ne!(answer_for(Some(DropEffect::Move), mask), 0, "{mask:#x}");
            }
        }
    }

    #[test]
    fn rows_are_dragged_out_where_the_system_takes_them() {
        assert_eq!(DRAGS_OUT, cfg!(any(windows, target_os = "macos")));
    }

    #[test]
    fn a_drag_out_allows_the_system_s_bits_of_its_effects() {
        assert_eq!(
            effects_bits(Effects::ALL),
            DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK
        );
        assert_eq!(effects_bits(Effects::COPY), DROPEFFECT_COPY);
        assert_eq!(effects_bits(Effects::MOVE), DROPEFFECT_MOVE);
        assert_eq!(effects_bits(Effects::LINK), DROPEFFECT_LINK);
        assert_eq!(effects_bits(Effects(0)), DROPEFFECT_NONE);
        for allowed in [
            Effects(0),
            Effects::COPY,
            Effects::MOVE,
            Effects::LINK,
            Effects::COPY | Effects::MOVE,
            Effects::MOVE | Effects::LINK,
            Effects::ALL,
        ] {
            // What a drop target reads of the drag is what the drag allows.
            assert_eq!(
                allowed_effects(effects_bits(allowed)),
                allowed,
                "{allowed:?}"
            );
        }
    }

    #[test]
    fn what_another_app_did_with_a_drag_out_on_windows_is_its_effect() {
        assert_eq!(performed_effect(DROPEFFECT_NONE), None);
        assert_eq!(performed_effect(DROPEFFECT_COPY), Some(DropEffect::Copy));
        assert_eq!(performed_effect(DROPEFFECT_MOVE), Some(DropEffect::Move));
        assert_eq!(performed_effect(DROPEFFECT_LINK), Some(DropEffect::Link));
        // File Explorer's scroll bit, and bits of nothing the app knows, tell nothing more.
        assert_eq!(
            performed_effect(DROPEFFECT_COPY | 0x8000_0000),
            Some(DropEffect::Copy)
        );
        assert_eq!(performed_effect(0x8000_0000), None);
        // More than one, which no target answers: the move, then the copy.
        assert_eq!(
            performed_effect(DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK),
            Some(DropEffect::Move)
        );
        assert_eq!(
            performed_effect(DROPEFFECT_COPY | DROPEFFECT_LINK),
            Some(DropEffect::Copy)
        );
    }

    #[test]
    fn the_app_s_drag_source_allows_all_out_of_the_app_and_a_move_or_a_copy_in_it() {
        let finder = NS_DRAG_COPY | NS_DRAG_LINK | NS_DRAG_GENERIC | NS_DRAG_MOVE;
        // Out of the app, as Finder's own drags: the taker chooses, by its keys.
        assert_eq!(source_mask(Effects::ALL, false), finder);
        // Within it (back over the window): a move or a copy, which the window's drop makes,
        // with the generic operation for its move, so that the window answers a move as Finder's
        // own drags hear it, with the move cursor (never the "+" of a copy).
        let within = source_mask(Effects::ALL, true);
        assert_eq!(within, NS_DRAG_COPY | NS_DRAG_GENERIC | NS_DRAG_MOVE);
        assert_eq!(mask_effects(within), Effects::COPY | Effects::MOVE);
        assert_eq!(answer_for(Some(DropEffect::Move), within), NS_DRAG_GENERIC);
        assert_eq!(answer_for(Some(DropEffect::Copy), within), NS_DRAG_COPY);
        // Only what the drag allows.
        assert_eq!(source_mask(Effects::COPY, false), NS_DRAG_COPY);
        assert_eq!(source_mask(Effects::COPY, true), NS_DRAG_COPY);
        assert_eq!(
            source_mask(Effects::MOVE, false),
            NS_DRAG_GENERIC | NS_DRAG_MOVE
        );
        assert_eq!(
            source_mask(Effects::MOVE, true),
            NS_DRAG_GENERIC | NS_DRAG_MOVE
        );
        assert_eq!(source_mask(Effects::LINK, false), NS_DRAG_LINK);
        assert_eq!(source_mask(Effects::LINK, true), 0);
        assert_eq!(source_mask(Effects(0), false), 0);
    }

    #[test]
    fn what_another_app_did_with_a_drag_out_on_macos_is_its_operation() {
        assert_eq!(ended_effect(0), None);
        assert_eq!(ended_effect(NS_DRAG_COPY), Some(DropEffect::Copy));
        assert_eq!(ended_effect(NS_DRAG_LINK), Some(DropEffect::Link));
        // The source's move, the taker's own (generic) one, and the Trash's delete all moved
        // the items away.
        assert_eq!(ended_effect(NS_DRAG_MOVE), Some(DropEffect::Move));
        assert_eq!(ended_effect(NS_DRAG_GENERIC), Some(DropEffect::Move));
        assert_eq!(ended_effect(NS_DRAG_DELETE), Some(DropEffect::Move));
        // `NSDragOperationPrivate` tells nothing the app knows.
        assert_eq!(ended_effect(1 << 3), None);
        // More than one, which no taker answers: the move, then the copy.
        assert_eq!(ended_effect(usize::MAX), Some(DropEffect::Move));
        assert_eq!(
            ended_effect(NS_DRAG_COPY | NS_DRAG_LINK),
            Some(DropEffect::Copy)
        );
    }

    #[test]
    fn a_macos_drop_is_taken_only_for_what_the_app_did() {
        assert!(drop_taken(Some(DropEffect::Copy)));
        assert!(drop_taken(Some(DropEffect::Move)));
        // The menu of 놓을 때 묻기 and a refusal: the source must not learn of a move yet.
        assert!(!drop_taken(None));
        assert!(!drop_taken(Some(DropEffect::Link)));
    }

    /// The image type, the message and the insert of a `DROPDESCRIPTION`'s bytes.
    fn description(bytes: &[u8]) -> (i32, String, String) {
        assert_eq!(bytes.len(), 4 + DESCRIPTION_UNITS * 4);
        let image = i32::from_le_bytes(bytes[..4].try_into().expect("four bytes"));
        let units: Vec<u16> = bytes[4..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        let text = |units: &[u16]| {
            let end = units.iter().position(|&unit| unit == 0).expect("a NUL");
            String::from_utf16(&units[..end]).expect("UTF-16")
        };
        let (message, insert) = units.split_at(DESCRIPTION_UNITS);
        (image, text(message), text(insert))
    }

    fn hint(message: &str, insert: &str) -> DropHint {
        DropHint {
            message: message.to_owned(),
            insert: insert.to_owned(),
        }
    }

    #[test]
    fn the_words_under_the_drag_image_say_what_the_window_says() {
        // The folder goes in the insert, for `%1`, as File Explorer's own words do.
        let moved = description(&description_bytes(
            Some(DropEffect::Move),
            &hint("이동 → %1", "백업"),
        ));
        assert_eq!(
            moved,
            (DROPIMAGE_MOVE, "이동 → %1".to_owned(), "백업".to_owned())
        );
        let copied = description(&description_bytes(
            Some(DropEffect::Copy),
            &hint("복사 → %1", "문서"),
        ));
        assert_eq!(
            copied,
            (DROPIMAGE_COPY, "복사 → %1".to_owned(), "문서".to_owned())
        );
        // A folder whose name holds `%1` keeps it: the insert is never read for `%1`.
        let odd = description(&description_bytes(
            Some(DropEffect::Move),
            &hint("이동 → %1", "100%1 완료"),
        ));
        assert_eq!(
            (odd.1.as_str(), odd.2.as_str()),
            ("이동 → %1", "100%1 완료")
        );
        // A refusal says why, with the "none" image.
        let refused = description(&description_bytes(
            None,
            &hint(
                "여기에 놓을 수 없습니다: 항목이 이미 이 폴더에 있습니다",
                "",
            ),
        ));
        assert_eq!(
            refused,
            (
                DROPIMAGE_NONE,
                "여기에 놓을 수 없습니다: 항목이 이미 이 폴더에 있습니다".to_owned(),
                String::new()
            )
        );
        // Nothing to say gives the source's own description back.
        let silent = description(&description_bytes(None, &DropHint::default()));
        assert_eq!(silent, (DROPIMAGE_INVALID, String::new(), String::new()));
        // A long folder name is cut to fit, never inside a character of two units.
        let long = "😀".repeat(200);
        let (_, _, insert) = description(&description_bytes(
            Some(DropEffect::Move),
            &hint("이동 → %1", &long),
        ));
        assert!(long.starts_with(&insert));
        assert!(insert.encode_utf16().count() < DESCRIPTION_UNITS);
        assert!(insert.encode_utf16().count() >= DESCRIPTION_UNITS - 2);
    }
}
