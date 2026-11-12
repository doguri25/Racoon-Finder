//! Thumbnails as the operating system makes them (PRD VIEW-09): what File Explorer and Finder
//! show for a picture, a film, a PDF or a document, asked for one file at a time.
//!
//! [`os_thumbnail`] is called on a worker thread (never the window's): it may take a while, and
//! the system's providers are other people's code. The pool that calls it guards it and gives up
//! on a file that takes too long; this module only promises not to wait forever.
//!
//! - **Windows:** the shell item of the path (`SHCreateItemFromParsingName`) and its
//!   `IShellItemImageFactory::GetImage`, asking for a thumbnail only (`SIIGBF_THUMBNAILONLY`,
//!   never the kind's icon) that may be bigger than asked (`SIIGBF_BIGGERSIZEOK`). The calling
//!   thread joins a single-threaded apartment for the call. The bitmap is read as a 32-bit
//!   top-down DIB (`GetDIBits`), turned into RGBA ([`bgra_to_rgba_rows`]), divided by its alpha
//!   (the shell's bitmaps are premultiplied) and deleted. Only the codes that say the file's
//!   content has no thumbnail, and will not have one until the file changes, are
//!   [`ThumbOutcome::None`] (`no_thumbnail` in the Windows module: no handler, a handler that
//!   could not make one, a picture format nothing decodes or a picture that cannot be read).
//!   Every other code, among them a file that is locked, half-written or unreadable for now, is
//!   [`ThumbOutcome::Failed`].
//! - **macOS:** `QLThumbnailGenerator.sharedGenerator` with a request for the `.thumbnail`
//!   representation at `size`×`size` points and scale 1. The generator is not tied to the main
//!   thread (its headers mark nothing of it so): it answers on a queue of its own, and the
//!   calling thread waits for that answer for at most 30 s (`MAC_WAIT`). The `CGImage` is drawn into
//!   an RGBA bitmap and divided by its alpha. Quick Look's "could not make one" errors are
//!   [`ThumbOutcome::None`].
//! - **Elsewhere** (Linux, which is for development): always [`ThumbOutcome::None`].
//!
//! The picture keeps the size the system gives: no bigger than asked on macOS, possibly bigger
//! on Windows. The caller fits it to the cell with [`fit_within`].

use std::fmt;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::time::Duration;

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

/// How long the calling thread waits for Quick Look's answer before it gives up on the file.
#[cfg(target_os = "macos")]
pub(crate) const MAC_WAIT: Duration = Duration::from_secs(30);

/// The longest side of a picture this module reads from the system. A thumbnail is far smaller;
/// a bitmap bigger than this is not copied, and the file is taken to have no thumbnail
/// ([`ThumbOutcome::None`]): the system gives the same for it every time.
#[cfg_attr(
    not(any(windows, target_os = "macos")),
    allow(dead_code, reason = "used by the Windows and macOS providers")
)]
pub(crate) const MAX_SIDE: u32 = 8192;

/// A thumbnail: `width` × `height` pixels of straight (not premultiplied) RGBA, 4 bytes each,
/// row by row from the top.
#[derive(Clone, PartialEq, Eq)]
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl fmt::Debug for Thumb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Thumb")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.rgba.len())
            .finish()
    }
}

/// What the system answered for one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThumbOutcome {
    /// The system's thumbnail.
    Made(Thumb),
    /// The system makes no thumbnail of this file: it keeps its kind's icon (or the app decodes
    /// it itself). Asking again gives the same.
    None,
    /// Something went wrong this time (the file was busy, the provider timed out, …), in the
    /// system's words; asking again later may work.
    Failed(String),
}

/// The system's thumbnail of the file at `path`, no bigger than `size` × `size` on macOS (in
/// pixels) and at least that on Windows when the file's picture is. Call it on a worker thread;
/// it blocks until the system answers. `size` 0 asks for nothing and gets [`ThumbOutcome::None`].
pub fn os_thumbnail(path: &Path, size: u32) -> ThumbOutcome {
    if size == 0 {
        return ThumbOutcome::None;
    }
    imp::thumbnail(path, size)
}

/// Whether this system makes thumbnails at all ([`os_thumbnail`] is always
/// [`ThumbOutcome::None`] where it does not).
pub fn supports_os_thumbnails() -> bool {
    imp::SUPPORTED
}

/// The size of a `w` × `h` picture shrunk to fit in a `size` × `size` square, its sides in the
/// same ratio (rounded to the nearest pixel, at least 1). A picture that fits already keeps its
/// size: nothing is ever made bigger. A picture or a square without area gives (0, 0).
pub fn fit_within(w: u32, h: u32, size: u32) -> (u32, u32) {
    if w == 0 || h == 0 || size == 0 {
        return (0, 0);
    }
    if w <= size && h <= size {
        return (w, h);
    }
    // The other side scaled as the longer one is to `size`: short × size / long, rounded.
    let scaled = |short: u32, long: u32| -> u32 {
        let (short, long, size) = (u64::from(short), u64::from(long), u64::from(size));
        let side = (short * size + long / 2) / long;
        u32::try_from(side).unwrap_or(u32::MAX).max(1)
    };
    if w >= h {
        (size, scaled(h, w))
    } else {
        (scaled(w, h), size)
    }
}

/// The `w` × `h` pixels of a 32-bit BGRA bitmap as RGBA rows from the top, without the padding
/// at the end of each row: `stride` bytes from the start of one row to the next, the rows from
/// the bottom when `bottom_up` (as a DIB with a positive height holds them). The last row needs
/// no padding. The colors are copied as they are, premultiplied or not.
///
/// A bitmap whose every alpha byte is 0 has no alpha (a DIB of 32 bits may leave it unused), so
/// its pixels are made opaque; should any pixel have alpha, every alpha is kept.
///
/// Bytes too few for the pixels, or a stride shorter than a row, give an empty `Vec`.
pub fn bgra_to_rgba_rows(bytes: &[u8], w: u32, h: u32, stride: usize, bottom_up: bool) -> Vec<u8> {
    let (Ok(w), Ok(h)) = (usize::try_from(w), usize::try_from(h)) else {
        return Vec::new();
    };
    let Some(row) = w.checked_mul(4) else {
        return Vec::new();
    };
    if w == 0 || h == 0 || stride < row {
        return Vec::new();
    }
    let needed = stride
        .checked_mul(h - 1)
        .and_then(|before_last| before_last.checked_add(row));
    if needed.is_none_or(|needed| bytes.len() < needed) {
        return Vec::new();
    }
    let mut rgba = Vec::with_capacity(row * h);
    let mut any_alpha = false;
    for y in 0..h {
        let from = if bottom_up { h - 1 - y } else { y } * stride;
        for &[blue, green, red, alpha] in bytes[from..from + row].as_chunks::<4>().0 {
            any_alpha |= alpha != 0;
            rgba.extend_from_slice(&[red, green, blue, alpha]);
        }
    }
    if !any_alpha {
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
    }
    rgba
}

/// Turns premultiplied RGBA into straight RGBA in place: each color divided by its pixel's
/// alpha (rounded, and no more than 255 should the data be bad). Opaque and fully transparent
/// pixels are left as they are.
#[cfg_attr(
    not(any(windows, target_os = "macos")),
    allow(
        dead_code,
        reason = "used by the Windows and macOS providers; tested on every OS"
    )
)]
pub(crate) fn unpremultiply(rgba: &mut [u8]) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 || alpha == 255 {
            continue;
        }
        for channel in &mut pixel[..3] {
            let straight = (u32::from(*channel) * 255 + alpha / 2) / alpha;
            *channel = u8::try_from(straight).unwrap_or(u8::MAX);
        }
    }
}

/// Whether a `QLThumbnailErrorDomain` code says Quick Look makes no thumbnail of the file (it
/// could not generate one, or has none in its cache or in the cloud) rather than a failure
/// worth another try (an invalid or cancelled request).
#[cfg_attr(
    not(target_os = "macos"),
    allow(dead_code, reason = "used by the macOS provider; tested on every OS")
)]
pub(crate) fn no_quicklook_thumbnail(code: isize) -> bool {
    /// `QLThumbnailErrorGenerationFailed`, `…NoCachedThumbnail` and `…NoCloudThumbnail`.
    const NONE: [isize; 3] = [0, 2, 3];
    NONE.contains(&code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CRC-32 of PNG chunks (and of zip, gzip, …).
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFF_u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// A 32 × 32 RGBA PNG of a color gradient, its data stored without compression, for the
    /// tests that hand a real file to the system's provider.
    #[cfg_attr(
        not(any(windows, target_os = "macos")),
        allow(dead_code, reason = "the providers' tests use it")
    )]
    pub(super) fn sample_png() -> Vec<u8> {
        const SIDE: u32 = 32;
        let mut raw = Vec::new();
        for y in 0..SIDE {
            raw.push(0); // filter: none
            for x in 0..SIDE {
                let (x, y) = (u8::try_from(x * 8).unwrap(), u8::try_from(y * 8).unwrap());
                raw.extend_from_slice(&[x, y, 128, 255]);
            }
        }
        // zlib: a header, stored blocks of at most 65535 bytes, then the Adler-32 of the data.
        let mut zlib = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
        for (i, block) in blocks.iter().enumerate() {
            zlib.push(u8::from(i + 1 == blocks.len()));
            let len = u16::try_from(block.len()).unwrap();
            zlib.extend_from_slice(&len.to_le_bytes());
            zlib.extend_from_slice(&(!len).to_le_bytes());
            zlib.extend_from_slice(block);
        }
        let (mut a, mut b) = (1_u32, 0_u32);
        for &byte in &raw {
            a = (a + u32::from(byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = |kind: &[u8; 4], data: &[u8]| {
            png.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
            let start = png.len();
            png.extend_from_slice(kind);
            png.extend_from_slice(data);
            let crc = crc32(&png[start..]);
            png.extend_from_slice(&crc.to_be_bytes());
        };
        let mut header = Vec::new();
        header.extend_from_slice(&SIDE.to_be_bytes());
        header.extend_from_slice(&SIDE.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bits, RGBA, deflate, no filter, no interlace
        chunk(b"IHDR", &header);
        chunk(b"IDAT", &zlib);
        chunk(b"IEND", &[]);
        png
    }

    /// Checks a system's thumbnail of [`sample_png`]: whole pixels, every one opaque, and the
    /// top-left one near the sample's (0, 0, 128) (scaling and color management move it a bit).
    #[cfg_attr(
        not(any(windows, target_os = "macos")),
        allow(dead_code, reason = "the providers' tests use it")
    )]
    pub(super) fn assert_is_the_sample(thumb: &Thumb) {
        assert!(thumb.width > 0 && thumb.height > 0, "{thumb:?}");
        assert_eq!(
            thumb.rgba.len(),
            thumb.width as usize * thumb.height as usize * 4
        );
        assert!(
            thumb
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 255),
            "the sample is opaque"
        );
        let near = |got: u8, want: u8| got.abs_diff(want) <= 24;
        let top_left = &thumb.rgba[..4];
        assert!(
            near(top_left[0], 0) && near(top_left[1], 0) && near(top_left[2], 128),
            "top-left pixel {top_left:?}"
        );
    }

    #[test]
    fn the_sample_pngs_checksums_are_the_standard_ones() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let png = sample_png();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(png.ends_with(&[0xAE, 0x42, 0x60, 0x82])); // the CRC of an empty IEND
    }

    #[test]
    fn a_square_picture_fits_the_square() {
        assert_eq!(fit_within(1000, 1000, 256), (256, 256));
    }

    #[test]
    fn a_wide_picture_keeps_its_ratio() {
        assert_eq!(fit_within(1600, 900, 256), (256, 144));
        // Rounded to the nearest pixel: 256 × 100 / 300 = 85.33.
        assert_eq!(fit_within(300, 100, 256), (256, 85));
    }

    #[test]
    fn a_tall_picture_keeps_its_ratio() {
        assert_eq!(fit_within(900, 1600, 256), (144, 256));
        assert_eq!(fit_within(100, 300, 256), (85, 256));
    }

    #[test]
    fn a_small_picture_is_never_scaled_up() {
        assert_eq!(fit_within(40, 30, 256), (40, 30));
        assert_eq!(fit_within(256, 10, 256), (256, 10));
    }

    #[test]
    fn a_thin_strip_keeps_at_least_one_pixel() {
        assert_eq!(fit_within(10_000, 1, 256), (256, 1));
        assert_eq!(fit_within(1, 10_000, 256), (1, 256));
    }

    #[test]
    fn nothing_fits_in_nothing() {
        assert_eq!(fit_within(0, 10, 256), (0, 0));
        assert_eq!(fit_within(10, 0, 256), (0, 0));
        assert_eq!(fit_within(10, 10, 0), (0, 0));
    }

    #[test]
    fn huge_sides_do_not_overflow() {
        assert_eq!(fit_within(u32::MAX, u32::MAX / 2, 1000), (1000, 500));
    }

    /// One BGRA pixel.
    fn bgra(b: u8, g: u8, r: u8, a: u8) -> [u8; 4] {
        [b, g, r, a]
    }

    #[test]
    fn bgra_becomes_rgba() {
        let bytes: Vec<u8> = [bgra(1, 2, 3, 4), bgra(5, 6, 7, 8)].concat();
        assert_eq!(
            bgra_to_rgba_rows(&bytes, 2, 1, 8, false),
            vec![3, 2, 1, 4, 7, 6, 5, 8]
        );
    }

    #[test]
    fn the_padding_at_the_end_of_each_row_is_dropped() {
        // Two rows of one pixel each, every row padded to 8 bytes.
        let bytes: Vec<u8> = [
            bgra(1, 2, 3, 9).as_slice(),
            &[0xEE; 4],
            bgra(4, 5, 6, 9).as_slice(),
            &[0xEE; 4],
        ]
        .concat();
        assert_eq!(
            bgra_to_rgba_rows(&bytes, 1, 2, 8, false),
            vec![3, 2, 1, 9, 6, 5, 4, 9]
        );
    }

    #[test]
    fn the_last_row_needs_no_padding() {
        let bytes: Vec<u8> = [
            bgra(1, 2, 3, 9).as_slice(),
            &[0xEE; 4],
            bgra(4, 5, 6, 9).as_slice(),
        ]
        .concat();
        assert_eq!(
            bgra_to_rgba_rows(&bytes, 1, 2, 8, false),
            vec![3, 2, 1, 9, 6, 5, 4, 9]
        );
    }

    #[test]
    fn bottom_up_rows_are_turned_over() {
        // A bottom-up bitmap holds its bottom row first.
        let bytes: Vec<u8> = [bgra(1, 1, 1, 9), bgra(2, 2, 2, 9), bgra(3, 3, 3, 9)].concat();
        assert_eq!(
            bgra_to_rgba_rows(&bytes, 1, 3, 4, true),
            vec![3, 3, 3, 9, 2, 2, 2, 9, 1, 1, 1, 9]
        );
    }

    #[test]
    fn a_bitmap_without_alpha_is_opaque() {
        let bytes: Vec<u8> = [bgra(1, 2, 3, 0), bgra(4, 5, 6, 0)].concat();
        assert_eq!(
            bgra_to_rgba_rows(&bytes, 2, 1, 8, false),
            vec![3, 2, 1, 255, 6, 5, 4, 255]
        );
    }

    #[test]
    fn one_alpha_byte_keeps_every_transparent_pixel() {
        let bytes: Vec<u8> = [bgra(1, 2, 3, 0), bgra(4, 5, 6, 1)].concat();
        assert_eq!(
            bgra_to_rgba_rows(&bytes, 2, 1, 8, false),
            vec![3, 2, 1, 0, 6, 5, 4, 1]
        );
    }

    #[test]
    fn the_padding_does_not_count_as_alpha() {
        // Every pixel's alpha is 0; the padding's bytes are not.
        let bytes: Vec<u8> = [bgra(1, 2, 3, 0).as_slice(), &[0xFF; 4]].concat();
        assert_eq!(
            bgra_to_rgba_rows(&bytes, 1, 1, 8, false),
            vec![3, 2, 1, 255]
        );
    }

    #[test]
    fn too_few_bytes_give_nothing() {
        assert_eq!(bgra_to_rgba_rows(&[0; 7], 2, 1, 8, false), Vec::<u8>::new());
        assert_eq!(
            bgra_to_rgba_rows(&[0; 16], 1, 3, 8, false),
            Vec::<u8>::new()
        );
        // A stride shorter than a row.
        assert_eq!(
            bgra_to_rgba_rows(&[0; 16], 2, 2, 4, false),
            Vec::<u8>::new()
        );
        assert_eq!(bgra_to_rgba_rows(&[], 0, 0, 0, false), Vec::<u8>::new());
    }

    #[test]
    fn premultiplied_colors_are_divided_by_their_alpha() {
        let mut rgba = vec![
            64, 32, 0, 128, // half covered
            10, 20, 30, 255, // opaque: unchanged
            0, 0, 0, 0, // fully transparent: stays black
            200, 200, 200, 100, // over the alpha (bad data): clamped
        ];
        unpremultiply(&mut rgba);
        assert_eq!(
            rgba,
            vec![
                128, 64, 0, 128, 10, 20, 30, 255, 0, 0, 0, 0, 255, 255, 255, 100
            ]
        );
    }

    #[test]
    fn quicklooks_could_not_make_one_means_none() {
        assert!(no_quicklook_thumbnail(0)); // generation failed
        assert!(no_quicklook_thumbnail(2)); // no cached thumbnail
        assert!(no_quicklook_thumbnail(3)); // no cloud thumbnail
        assert!(!no_quicklook_thumbnail(1)); // saving to a URL failed
        assert!(!no_quicklook_thumbnail(4)); // request invalid
        assert!(!no_quicklook_thumbnail(5)); // request cancelled
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn linux_asks_no_one() {
        assert!(!supports_os_thumbnails());
        assert_eq!(os_thumbnail(Path::new("/a.png"), 128), ThumbOutcome::None);
    }

    #[test]
    fn a_thumbnail_of_no_size_is_none() {
        assert_eq!(os_thumbnail(Path::new("a.png"), 0), ThumbOutcome::None);
    }
}
