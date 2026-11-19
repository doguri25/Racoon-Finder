#![allow(
    unsafe_code,
    reason = "the shell's IShellItemImageFactory and the GDI calls that read its bitmap"
)]

//! The shell's thumbnails: `IShellItemImageFactory::GetImage` of the path's shell item, read
//! out of its `HBITMAP` as a 32-bit top-down DIB.

use std::ffi::c_void;
use std::path::Path;

use windows::Win32::Foundation::{
    E_FAIL, SIZE, WINCODEC_ERR_BADHEADER, WINCODEC_ERR_BADIMAGE, WINCODEC_ERR_COMPONENTNOTFOUND,
    WINCODEC_ERR_UNKNOWNIMAGEFORMAT, WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::Com::IBindCtx;
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_THUMBNAILONLY,
};
use windows::core::{HRESULT, PCWSTR};

use super::{MAX_SIDE, Thumb, ThumbOutcome, bgra_to_rgba_rows, unpremultiply};
use crate::shell::enter_apartment;
use crate::win_util::wide;

pub(super) const SUPPORTED: bool = true;

/// `WTS_E_FAILEDEXTRACTION` (thumbcache.h): the shell found no thumbnail handler that could
/// make one of the file.
const WTS_E_FAILEDEXTRACTION: HRESULT = HRESULT(0x8004_B200_u32.cast_signed());

/// Whether a code of `IShellItemImageFactory::GetImage` says the file's content has no
/// thumbnail, which stays so until the file changes (the caller's cache key holds the file's
/// size and modification time, so a rewritten file is asked again):
///
/// - `E_FAIL`: what `GetImage` with `SIIGBF_THUMBNAILONLY` gives for a kind without a handler;
/// - `WTS_E_FAILEDEXTRACTION`: no handler could make one;
/// - the Windows Imaging Component found no decoder for the format, a pixel format it cannot
///   convert, an unknown format, a header it cannot read or a picture it cannot read.
///
/// Every other code (the file locked or half-written, a stream that could not be read now, a
/// Win32 error inside WIC, a timeout, no memory, …) is a failure worth another try.
fn no_thumbnail(code: HRESULT) -> bool {
    [
        E_FAIL,
        WTS_E_FAILEDEXTRACTION,
        WINCODEC_ERR_COMPONENTNOTFOUND,
        WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT,
        WINCODEC_ERR_UNKNOWNIMAGEFORMAT,
        WINCODEC_ERR_BADHEADER,
        WINCODEC_ERR_BADIMAGE,
    ]
    .contains(&code)
}

/// The bitmap the shell handed over, deleted when dropped.
struct Bitmap(HBITMAP);

impl Drop for Bitmap {
    fn drop(&mut self) {
        // SAFETY: `GetImage` gave this bitmap to the caller, who owns it; it is selected into
        // no device context, and nothing uses it after this.
        let _ = unsafe { DeleteObject(HGDIOBJ::from(self.0)) };
    }
}

/// A memory device context for `GetDIBits`, deleted when dropped.
struct MemoryDc(HDC);

impl MemoryDc {
    fn new() -> Option<MemoryDc> {
        // SAFETY: no DC is passed, so the new one is compatible with the screen; it is
        // deleted by the drop.
        let dc = unsafe { CreateCompatibleDC(None) };
        (!dc.is_invalid()).then_some(MemoryDc(dc))
    }
}

impl Drop for MemoryDc {
    fn drop(&mut self) {
        // SAFETY: made by `CreateCompatibleDC` and not deleted before.
        let _ = unsafe { DeleteDC(self.0) };
    }
}

/// The text of a failure: the code, then the system's words for it.
fn failed(error: &windows::core::Error) -> ThumbOutcome {
    let hr = u32::from_ne_bytes(error.code().0.to_ne_bytes());
    ThumbOutcome::Failed(format!("{hr:#010X} {}", error.message()))
}

pub(super) fn thumbnail(path: &Path, size: u32) -> ThumbOutcome {
    let _com = match enter_apartment() {
        Ok(scope) => scope,
        Err(error) => return ThumbOutcome::Failed(format!("COM: {error}")),
    };
    let name = wide(path.as_os_str());
    // SAFETY: `name` is NUL-terminated UTF-16 that outlives the call; no bind context.
    let factory: IShellItemImageFactory =
        match unsafe { SHCreateItemFromParsingName(PCWSTR(name.as_ptr()), None::<&IBindCtx>) } {
            Ok(factory) => factory,
            Err(error) => return failed(&error),
        };
    let side = i32::try_from(size.min(MAX_SIDE)).unwrap_or(i32::MAX);
    // SAFETY: a plain call on a live interface; the bitmap it returns is ours to delete, which
    // `Bitmap` does.
    let bitmap = match unsafe {
        factory.GetImage(
            SIZE { cx: side, cy: side },
            SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK,
        )
    } {
        Ok(bitmap) => Bitmap(bitmap),
        Err(error) => {
            if no_thumbnail(error.code()) {
                return ThumbOutcome::None;
            }
            return failed(&error);
        }
    };
    // `_com` ends after the factory and the bitmap, declared after it.
    read(&bitmap)
}

/// The pixels of `bitmap` as straight RGBA from the top.
fn read(bitmap: &Bitmap) -> ThumbOutcome {
    let mut info = BITMAP::default();
    let info_size = i32::try_from(size_of::<BITMAP>()).unwrap_or(i32::MAX);
    // SAFETY: `info` is a `BITMAP` of `info_size` bytes, which `GetObjectW` fills for a bitmap
    // handle; the handle is live (`Bitmap`).
    let got = unsafe {
        GetObjectW(
            HGDIOBJ::from(bitmap.0),
            info_size,
            Some((&raw mut info).cast::<c_void>()),
        )
    };
    if got == 0 {
        return ThumbOutcome::Failed("the shell's bitmap could not be read".into());
    }
    let Ok(width) = u32::try_from(info.bmWidth) else {
        return ThumbOutcome::Failed("the shell's bitmap has a negative width".into());
    };
    // Negative for a top-down bitmap.
    let height = info.bmHeight.unsigned_abs();
    if width == 0 || height == 0 {
        return ThumbOutcome::None;
    }
    if width > MAX_SIDE || height > MAX_SIDE {
        // The same every time for this file: not a thumbnail to keep asking for.
        return ThumbOutcome::None;
    }
    let (Ok(dib_width), Ok(dib_height)) = (i32::try_from(width), i32::try_from(height)) else {
        return ThumbOutcome::Failed("the shell's bitmap is too big".into());
    };
    let stride = width as usize * 4;
    let mut pixels = vec![0_u8; stride * height as usize];
    let mut format = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: u32::try_from(size_of::<BITMAPINFOHEADER>()).unwrap_or(0),
            biWidth: dib_width,
            // Negative: the rows from the top.
            biHeight: -dib_height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..BITMAPINFOHEADER::default()
        },
        ..BITMAPINFO::default()
    };
    let Some(dc) = MemoryDc::new() else {
        return ThumbOutcome::Failed("no device context for the shell's bitmap".into());
    };
    // SAFETY: the bitmap is live and selected into no DC; `pixels` holds `height` rows of
    // `stride` bytes, which is what a 32-bit DIB of `width` pixels needs (its rows are whole
    // DWORDs already), and `format` describes that DIB with no color table.
    let lines = unsafe {
        GetDIBits(
            dc.0,
            bitmap.0,
            0,
            height,
            Some(pixels.as_mut_ptr().cast::<c_void>()),
            &raw mut format,
            DIB_RGB_COLORS,
        )
    };
    if u32::try_from(lines).ok() != Some(height) {
        return ThumbOutcome::Failed("the shell's bitmap could not be copied".into());
    }
    let mut rgba = bgra_to_rgba_rows(&pixels, width, height, stride, false);
    if rgba.is_empty() {
        return ThumbOutcome::Failed("the shell's bitmap is short".into());
    }
    unpremultiply(&mut rgba);
    ThumbOutcome::Made(Thumb {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::super::os_thumbnail;
    use super::super::tests::sample_png;
    use super::*;
    use crate::thumbs::tests::assert_is_the_sample;
    use windows::Win32::Foundation::{
        E_OUTOFMEMORY, E_UNEXPECTED, WINCODEC_ERR_STREAMREAD, WINCODEC_ERR_WIN32ERROR,
    };
    use windows::Win32::UI::Shell::{
        WTS_E_EXTRACTIONTIMEDOUT, WTS_E_NOSTORAGEPROVIDERTHUMBNAILHANDLER,
    };

    #[test]
    fn the_shells_no_thumbnail_codes_mean_none() {
        // The shell's own name for it, which this module spells out.
        assert_eq!(
            WTS_E_FAILEDEXTRACTION,
            windows::Win32::UI::Shell::WTS_E_FAILEDEXTRACTION
        );
        for code in [
            E_FAIL,
            WTS_E_FAILEDEXTRACTION,
            WINCODEC_ERR_COMPONENTNOTFOUND,
            WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT,
            WINCODEC_ERR_UNKNOWNIMAGEFORMAT,
            WINCODEC_ERR_BADHEADER,
            WINCODEC_ERR_BADIMAGE,
        ] {
            assert!(no_thumbnail(code), "{code:?}");
        }
    }

    #[test]
    fn other_codes_are_failures_to_try_again() {
        for code in [
            HRESULT::from_win32(2),  // file not found
            HRESULT::from_win32(5),  // access denied
            HRESULT::from_win32(32), // a sharing violation: the file is open elsewhere
            WINCODEC_ERR_STREAMREAD,
            WINCODEC_ERR_WIN32ERROR,
            WTS_E_EXTRACTIONTIMEDOUT,
            WTS_E_NOSTORAGEPROVIDERTHUMBNAILHANDLER,
            E_UNEXPECTED,
            E_OUTOFMEMORY,
        ] {
            assert!(!no_thumbnail(code), "{code:?}");
        }
    }

    #[test]
    #[ignore = "needs an interactive session"]
    fn the_shell_makes_a_thumbnail_of_a_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("그림.png");
        std::fs::write(&path, sample_png()).unwrap();
        match os_thumbnail(&path, 64) {
            ThumbOutcome::Made(thumb) => assert_is_the_sample(&thumb),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    #[ignore = "needs an interactive session"]
    fn the_shell_has_no_thumbnail_of_plain_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, "hello").unwrap();
        assert_eq!(os_thumbnail(&path, 64), ThumbOutcome::None);
    }
}
