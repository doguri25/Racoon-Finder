//! What the preview panel (VIEW-02) and quick look (VIEW-01) show of an item, and the work a
//! worker thread does to find it out ([`make_preview`]).
//!
//! The UI thread never opens the item: it asks for a [`PreviewRequest`] with a generation
//! ([`Effect::Preview`](crate::state::Effect::Preview)), and the worker
//! ([`Previewer`](crate::controller::Previewer)) waits [`PREVIEW_DELAY`] for the cursor to rest,
//! then reads the item and hands back a [`Previewed`] with that generation, which the state takes
//! only while it is still the one it waits for. A newer request stops an older one: the folder
//! count looks at its [`CancelToken`] between batches, and whatever the older one finds is
//! dropped.
//!
//! - An image (png, jpeg, gif's first frame, webp, bmp, ico, tiff, by its extension) is decoded
//!   with the `image` crate and made at most [`MAX_SIDE`] pixels on its long side. A file over
//!   [`MAX_IMAGE_BYTES`] is not decoded ([`PreviewContent::TooLarge`]: its size shows). Before
//!   anything is decoded, the size the file declares must be within [`MAX_DIMENSION`] a side and
//!   [`MAX_PIXELS`] in all, and the decoder may allocate [`MAX_DECODE_BYTES`] at most, so that a
//!   small file that declares a huge image (a decompression bomb) costs nothing.
//! - Any other file is read for its first [`TEXT_BYTES`]: a byte order mark (UTF-8, UTF-16)
//!   names the encoding, or `chardetng` guesses it (UTF-8, EUC-KR, `Shift_JIS`, `windows-1252` …), and
//!   `encoding_rs` decodes it. Bytes that are not text (a NUL, or many control characters) make
//!   it [`PreviewContent::Plain`]: its icon, kind, size and date.
//! - A folder shows how many items it has, counted on the worker up to [`MAX_ITEMS_COUNTED`].
//! - Every decoder runs inside `catch_unwind`: a file that makes one fail or panic shows "미리 볼
//!   수 없습니다" ([`PreviewContent::Unavailable`]), as one that cannot be read does.

use std::ffi::OsStr;
use std::fmt;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::Duration;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::Encoding;
use image::{DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader, Limits};
use rf_core::{EntryKind, ErrorKind, VPath};
use rf_vfs::{CancelToken, Provider, Vfs, VisibilityFilter};
use slint::{Rgba8Pixel, SharedPixelBuffer};

/// How long the cursor rests on an item before its preview is made: moving on within it asks for
/// nothing.
pub const PREVIEW_DELAY: Duration = Duration::from_millis(100);
/// The most pixels an image preview has on its long side; a larger image is made smaller.
pub const MAX_SIDE: u32 = 1600;
/// The largest image file decoded (50 MB); a larger one shows its size only.
pub const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;
/// The widest and tallest image decoded, as its file declares it.
pub const MAX_DIMENSION: u32 = 32_768;
/// The most pixels an image may declare (a 64-megapixel photo; 8,192 × 8,192).
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;
/// The most memory an image decoder may take.
pub const MAX_DECODE_BYTES: u64 = 512 * 1024 * 1024;
/// How much of a file a text preview reads: its first 256 KB.
pub const TEXT_BYTES: usize = 256 * 1024;
/// The most characters a line of a text preview keeps; the rest of a longer one is cut off.
pub const MAX_LINE_CHARS: usize = 2_000;
/// How many items of a folder are counted at most ("항목 10000개 이상").
pub const MAX_ITEMS_COUNTED: usize = 10_000;
/// How many spaces a tab of a text preview takes.
const TAB_SPACES: &str = "    ";

/// What the state asks a worker to preview: the item at `path`, of kind `kind`, with the
/// generation the answer carries. A folder counts its items as the panes show them (`filter`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewRequest {
    pub generation: u64,
    pub path: VPath,
    pub kind: EntryKind,
    pub filter: VisibilityFilter,
}

/// A preview the worker made for the request with `generation`.
#[derive(Clone, Debug, PartialEq)]
pub struct Previewed {
    pub generation: u64,
    pub content: PreviewContent,
}

/// What the preview of an item shows besides its name, kind, size and date.
#[derive(Clone, Debug, PartialEq)]
pub enum PreviewContent {
    /// The image, made small enough.
    Image(PreviewImage),
    /// The first part of a text file.
    Text(TextPreview),
    /// How many items a folder shows: `items`, and `more` when there are more than that
    /// ([`MAX_ITEMS_COUNTED`]).
    Folder { items: usize, more: bool },
    /// Nothing more than the facts: a file that is no text (or empty), a link, a special file.
    Plain,
    /// An image file too large to decode ([`MAX_IMAGE_BYTES`]): its size shows.
    TooLarge,
    /// It cannot be read, or its content is broken: "미리 볼 수 없습니다".
    Unavailable,
}

/// An image to show: its pixels, at most [`MAX_SIDE`] on the long side, and the size the image
/// has (turned as its file says to show it).
#[derive(Clone)]
pub struct PreviewImage {
    pub pixels: SharedPixelBuffer<Rgba8Pixel>,
    pub width: u32,
    pub height: u32,
}

impl fmt::Debug for PreviewImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreviewImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("shown", &(self.pixels.width(), self.pixels.height()))
            .finish_non_exhaustive()
    }
}

impl PartialEq for PreviewImage {
    fn eq(&self, other: &PreviewImage) -> bool {
        (self.width, self.height) == (other.width, other.height)
            && (self.pixels.width(), self.pixels.height())
                == (other.pixels.width(), other.pixels.height())
            && self.pixels.as_bytes() == other.pixels.as_bytes()
    }
}

/// The first part of a text file, as lines: the encoding it was read in (as `encoding_rs` names
/// it: `UTF-8`, `UTF-16LE`, `EUC-KR`), and whether the file goes on past what was read
/// ([`TEXT_BYTES`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextPreview {
    pub lines: Vec<String>,
    pub encoding: &'static str,
    pub truncated: bool,
}

/// Why an image is not shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRefused {
    /// The file declares a larger image than [`MAX_DIMENSION`] or [`MAX_PIXELS`] allow, or the
    /// decoder would need more than [`MAX_DECODE_BYTES`]: nothing was decoded.
    TooLarge,
    /// It is not an image of a format this preview knows, or it is broken.
    Broken,
}

/// What the first part of a file is as text ([`read_text`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextRead {
    Text(TextPreview),
    /// Not text: a NUL byte, or many control characters.
    Binary,
    /// Bytes that are not valid in the encoding they name or seem to be in.
    Malformed,
}

/// Makes the preview of `request` through `vfs`. Blocks: call it on a worker thread. `None`
/// when `cancel` stopped it (a newer request came).
pub fn make_preview(
    request: &PreviewRequest,
    vfs: &Vfs,
    cancel: &CancelToken,
) -> Option<PreviewContent> {
    // A link or a special file is not opened: a FIFO's reader would wait for its writer.
    if !matches!(request.kind, EntryKind::File | EntryKind::Dir) {
        return Some(PreviewContent::Plain);
    }
    let Ok(provider) = vfs.provider(&request.path) else {
        return Some(PreviewContent::Unavailable);
    };
    if request.kind == EntryKind::Dir {
        return count_items(
            provider.as_ref(),
            &request.path,
            request.filter,
            cancel,
            MAX_ITEMS_COUNTED,
        );
    }
    Some(file_preview(provider.as_ref(), &request.path))
}

/// The preview of the file at `path`: an image by its extension, else its first part as text.
fn file_preview(provider: &dyn Provider, path: &VPath) -> PreviewContent {
    let Ok(mut stream) = provider.open_read(path) else {
        return PreviewContent::Unavailable;
    };
    let Ok(length) = stream.seek(SeekFrom::End(0)) else {
        return PreviewContent::Unavailable;
    };
    if stream.rewind().is_err() {
        return PreviewContent::Unavailable;
    }
    if let Some(format) = path.file_name().and_then(image_format) {
        if length > MAX_IMAGE_BYTES {
            return PreviewContent::TooLarge;
        }
        return match guarded(|| decode_image(BufReader::new(stream), format)) {
            Some(Ok(image)) => PreviewContent::Image(image),
            Some(Err(_)) | None => PreviewContent::Unavailable,
        };
    }
    let wanted = usize::try_from(length).map_or(TEXT_BYTES, |length| length.min(TEXT_BYTES));
    let mut head = Vec::with_capacity(wanted);
    let limit = u64::try_from(TEXT_BYTES).unwrap_or(u64::MAX);
    if stream.take(limit).read_to_end(&mut head).is_err() {
        return PreviewContent::Unavailable;
    }
    if head.is_empty() {
        return PreviewContent::Plain;
    }
    let whole = u64::try_from(head.len()).is_ok_and(|read| read >= length);
    match guarded(|| read_text(&head, whole)) {
        Some(TextRead::Text(text)) => PreviewContent::Text(text),
        Some(TextRead::Binary) => PreviewContent::Plain,
        Some(TextRead::Malformed) | None => PreviewContent::Unavailable,
    }
}

/// How many items of the folder `dir` `filter` shows, up to `cap`: `(items, more)`, with
/// `more` when there are more than `cap`. Stops between batches once `cancel` is cancelled
/// (`None`); a folder that cannot be listed is [`PreviewContent::Unavailable`].
pub fn count_items(
    provider: &dyn Provider,
    dir: &VPath,
    filter: VisibilityFilter,
    cancel: &CancelToken,
    cap: usize,
) -> Option<PreviewContent> {
    if cancel.is_cancelled() {
        return None;
    }
    // The listing stops at its next batch once the count is past the cap, or once `cancel` is.
    let stop = CancelToken::new();
    let mut items = 0usize;
    let listed = provider.list(dir, &stop, &mut |batch| {
        items += batch
            .iter()
            .filter(|entry| filter.is_visible(entry))
            .count();
        if items > cap || cancel.is_cancelled() {
            stop.cancel();
        }
    });
    if cancel.is_cancelled() {
        return None;
    }
    let more = items > cap;
    match listed {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::Cancelled && more => {}
        Err(_) => return Some(PreviewContent::Unavailable),
    }
    Some(PreviewContent::Folder {
        items: items.min(cap),
        more,
    })
}

/// The image format a file called `name` is in, by its extension, when the preview shows it.
pub fn image_format(name: &OsStr) -> Option<ImageFormat> {
    // A dot file (`.png`) has no extension.
    let extension = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "gif" => Some(ImageFormat::Gif),
        "webp" => Some(ImageFormat::WebP),
        "bmp" => Some(ImageFormat::Bmp),
        "ico" => Some(ImageFormat::Ico),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        _ => None,
    }
}

/// Whether `format` is one the preview decodes (the features of the `image` dependency).
fn shown_format(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::Gif
            | ImageFormat::WebP
            | ImageFormat::Bmp
            | ImageFormat::Ico
            | ImageFormat::Tiff
    )
}

/// Decodes the image `reader` holds (in `format`, or the format its first bytes say), within the
/// limits of the module, and makes it at most [`MAX_SIDE`] on its long side.
pub fn decode_image(
    reader: impl BufRead + Seek,
    format: ImageFormat,
) -> Result<PreviewImage, ImageRefused> {
    let mut reader = ImageReader::with_format(reader, format)
        .with_guessed_format()
        .map_err(|_| ImageRefused::Broken)?;
    if !reader.format().is_some_and(shown_format) {
        return Err(ImageRefused::Broken);
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|error| refusal(&error))?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err(ImageRefused::Broken);
    }
    // The decoded pixels are the decoder's largest allocation: refused before it is made.
    if u64::from(width) * u64::from(height) > MAX_PIXELS || decoder.total_bytes() > MAX_DECODE_BYTES
    {
        return Err(ImageRefused::TooLarge);
    }
    let orientation = decoder.orientation().map_err(|error| refusal(&error))?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|error| refusal(&error))?;
    image.apply_orientation(orientation);
    let (width, height) = (image.width(), image.height());
    if width > MAX_SIDE || height > MAX_SIDE {
        image = image.thumbnail(MAX_SIDE, MAX_SIDE);
    }
    let rgba = image.into_rgba8();
    let pixels = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    );
    Ok(PreviewImage {
        pixels,
        width,
        height,
    })
}

/// Why `error` refused an image: a limit it met, or anything else.
fn refusal(error: &ImageError) -> ImageRefused {
    match error {
        ImageError::Limits(_) => ImageRefused::TooLarge,
        _ => ImageRefused::Broken,
    }
}

/// Reads `bytes`, the first part of a file (`whole` when it is all of it), as text.
pub fn read_text(bytes: &[u8], whole: bool) -> TextRead {
    let (encoding, body) = if let Some((encoding, mark)) = Encoding::for_bom(bytes) {
        (encoding, &bytes[mark..])
    } else {
        // Text without a mark has no NUL; UTF-16 without one is not told (it is rare).
        if bytes.contains(&0) {
            return TextRead::Binary;
        }
        let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
        detector.feed(bytes, whole);
        (detector.guess(None, Utf8Detection::Allow), bytes)
    };
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let room = decoder
        .max_utf8_buffer_length(body.len())
        .unwrap_or(body.len().saturating_mul(3));
    let mut text = String::with_capacity(room);
    // A part cut inside a character leaves that character in the decoder: it is no error.
    let (_, _, malformed) = decoder.decode_to_string(body, &mut text, whole);
    if malformed {
        return TextRead::Malformed;
    }
    if looks_binary(&text) {
        return TextRead::Binary;
    }
    let mut lines: Vec<String> = text.split('\n').map(shown_line).collect();
    // The line break that ends the text starts no line.
    if text.ends_with('\n') {
        lines.pop();
    }
    TextRead::Text(TextPreview {
        lines,
        encoding: encoding.name(),
        truncated: !whole,
    })
}

/// Whether `text` holds more control characters than text does: more than one in a hundred,
/// besides tabs, line breaks, form feeds and escapes.
fn looks_binary(text: &str) -> bool {
    let mut all = 0usize;
    let mut controls = 0usize;
    for c in text.chars() {
        all += 1;
        if c.is_control() && !matches!(c, '\t' | '\n' | '\r' | '\u{c}' | '\u{1b}') {
            controls += 1;
        }
    }
    controls * 100 > all
}

/// A line of a text preview as it shows: without the `\r` of a Windows line break, its tabs as
/// spaces, and cut off with `…` past [`MAX_LINE_CHARS`].
fn shown_line(line: &str) -> String {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let mut shown = String::with_capacity(line.len());
    for (i, c) in line.chars().enumerate() {
        if i == MAX_LINE_CHARS {
            shown.push('…');
            break;
        }
        if c == '\t' {
            shown.push_str(TAB_SPACES);
        } else {
            shown.push(c);
        }
    }
    shown
}

/// Runs `work`, and gives `None` if it panics: a decoder's bug or a file it chokes on never takes
/// the worker down.
pub fn guarded<T>(work: impl FnOnce() -> T) -> Option<T> {
    catch_unwind(AssertUnwindSafe(work)).ok()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::path::PathBuf;

    use image::{ImageBuffer, Rgba, RgbaImage};
    use rf_core::Attributes;

    use super::*;

    /// A picture of `width` × `height` pixels in `format`, made here rather than kept as a file.
    fn picture(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let image: RgbaImage = ImageBuffer::from_fn(width, height, |x, y| {
            Rgba([
                u8::try_from(x % 256).unwrap_or(0),
                u8::try_from(y % 256).unwrap_or(0),
                128,
                255,
            ])
        });
        let mut bytes = Cursor::new(Vec::new());
        let image = if format == ImageFormat::Jpeg || format == ImageFormat::Bmp {
            DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(image).into_rgb8())
        } else {
            DynamicImage::ImageRgba8(image)
        };
        image
            .write_to(&mut bytes, format)
            .expect("the picture encodes");
        bytes.into_inner()
    }

    /// The header of a 32-bit BMP that declares `width` × `height` pixels and holds none of them.
    fn declared_bmp(width: i32, height: i32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"BM");
        bytes.extend_from_slice(&54u32.to_le_bytes()); // file size (the header alone)
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&54u32.to_le_bytes()); // where the pixels start
        bytes.extend_from_slice(&40u32.to_le_bytes()); // BITMAPINFOHEADER
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // planes
        bytes.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        bytes.extend_from_slice(&[0; 24]); // no compression, sizes and colors left to the reader
        bytes
    }

    fn decode(bytes: Vec<u8>, format: ImageFormat) -> Result<PreviewImage, ImageRefused> {
        decode_image(Cursor::new(bytes), format)
    }

    #[test]
    fn each_format_of_the_preview_decodes() {
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Gif,
            ImageFormat::WebP,
            ImageFormat::Bmp,
            ImageFormat::Ico,
            ImageFormat::Tiff,
        ] {
            let image = decode(picture(40, 24, format), format)
                .unwrap_or_else(|why| panic!("{format:?}: {why:?}"));
            assert_eq!((image.width, image.height), (40, 24), "{format:?}");
            assert_eq!(
                (image.pixels.width(), image.pixels.height()),
                (40, 24),
                "{format:?}"
            );
        }
    }

    #[test]
    fn a_large_image_is_made_at_most_1600_on_its_long_side() {
        let image = decode(picture(3200, 800, ImageFormat::Png), ImageFormat::Png)
            .expect("the picture decodes");
        assert_eq!((image.width, image.height), (3200, 800));
        assert_eq!((image.pixels.width(), image.pixels.height()), (1600, 400));
        let tall = decode(picture(300, 2000, ImageFormat::Png), ImageFormat::Png)
            .expect("the picture decodes");
        assert_eq!((tall.pixels.width(), tall.pixels.height()), (240, 1600));
    }

    #[test]
    fn the_format_inside_counts_more_than_the_name() {
        // A PNG named as a JPEG.
        let image = decode(picture(8, 8, ImageFormat::Png), ImageFormat::Jpeg)
            .expect("the content says PNG");
        assert_eq!((image.width, image.height), (8, 8));
    }

    #[test]
    fn a_broken_png_is_refused_without_a_panic() {
        let whole = picture(64, 64, ImageFormat::Png);
        // Cut off in its pixel data.
        let cut = whole[..whole.len() / 2].to_vec();
        assert_eq!(decode(cut, ImageFormat::Png), Err(ImageRefused::Broken));
        // The signature, then garbage.
        let mut garbage = whole[..8].to_vec();
        garbage.extend(std::iter::repeat_n(0xA5, 200));
        assert_eq!(decode(garbage, ImageFormat::Png), Err(ImageRefused::Broken));
        // Pixel data that was changed after the header.
        let mut changed = whole.clone();
        let middle = changed.len() / 2;
        for byte in &mut changed[middle..middle + 64] {
            *byte ^= 0x5A;
        }
        assert_eq!(decode(changed, ImageFormat::Png), Err(ImageRefused::Broken));
        // No bytes at all.
        assert_eq!(
            decode(Vec::new(), ImageFormat::Png),
            Err(ImageRefused::Broken)
        );
    }

    #[test]
    fn an_image_that_declares_too_much_is_refused_before_it_is_decoded() {
        // 20,000 × 20,000 is within a side but 400 megapixels in all; 40,000 is past a side (a
        // negative height is a BMP drawn from the top).
        for (width, height) in [(20_000, 20_000), (40_000, 10), (10, -40_000)] {
            assert_eq!(
                decode(declared_bmp(width, height), ImageFormat::Bmp),
                Err(ImageRefused::TooLarge),
                "{width} × {height}"
            );
        }
        // Past 65,535 a side the BMP decoder refuses it itself.
        assert!(decode(declared_bmp(100_000, 10), ImageFormat::Bmp).is_err());
        // A small one with nothing in it is only broken.
        assert_eq!(
            decode(declared_bmp(10, 10), ImageFormat::Bmp),
            Err(ImageRefused::Broken)
        );
    }

    #[test]
    fn the_extension_names_the_image_formats_of_the_preview() {
        for (name, format) in [
            ("a.png", Some(ImageFormat::Png)),
            ("a.PNG", Some(ImageFormat::Png)),
            ("a.jpg", Some(ImageFormat::Jpeg)),
            ("a.jpeg", Some(ImageFormat::Jpeg)),
            ("a.gif", Some(ImageFormat::Gif)),
            ("a.webp", Some(ImageFormat::WebP)),
            ("a.bmp", Some(ImageFormat::Bmp)),
            ("a.ico", Some(ImageFormat::Ico)),
            ("a.tif", Some(ImageFormat::Tiff)),
            ("a.tiff", Some(ImageFormat::Tiff)),
            ("a.heic", None),
            ("a.svg", None),
            ("a.txt", None),
            ("png", None),
            (".png", None),
        ] {
            assert_eq!(image_format(OsStr::new(name)), format, "{name}");
        }
    }

    /// Korean prose, long enough for the encoding to be told.
    const KOREAN: &str = "다람쥐 헌 쳇바퀴에 타고파. 키스의 고유조건은 입술끼리 만나야 하고 특별한 기술은 \
                          필요치 않다. 오늘 회의에서는 설계안의 두 번째 판을 검토했습니다.\n\
                          둘째 줄입니다.\r\n셋째 줄\t탭 뒤\n";

    fn text(read: TextRead) -> TextPreview {
        match read {
            TextRead::Text(text) => text,
            other => panic!("not text: {other:?}"),
        }
    }

    #[test]
    fn euc_kr_text_is_told_and_read_in_korean() {
        let (bytes, _, unmappable) = encoding_rs::EUC_KR.encode(KOREAN);
        assert!(!unmappable);
        let read = text(read_text(&bytes, true));
        assert_eq!(read.encoding, "EUC-KR");
        assert!(!read.truncated);
        assert_eq!(
            read.lines,
            [
                "다람쥐 헌 쳇바퀴에 타고파. 키스의 고유조건은 입술끼리 만나야 하고 특별한 기술은 \
                 필요치 않다. 오늘 회의에서는 설계안의 두 번째 판을 검토했습니다.",
                "둘째 줄입니다.",
                "셋째 줄    탭 뒤",
            ]
        );
    }

    #[test]
    fn utf8_and_utf16_with_a_mark_are_read() {
        let utf8 = text(read_text(KOREAN.as_bytes(), true));
        assert_eq!(utf8.encoding, "UTF-8");
        assert_eq!(utf8.lines[1], "둘째 줄입니다.");
        // UTF-8 with its byte order mark, which does not show.
        let mut marked = vec![0xEF, 0xBB, 0xBF];
        marked.extend_from_slice("첫 줄\n".as_bytes());
        assert_eq!(text(read_text(&marked, true)).lines, ["첫 줄"]);
        for (mark, encoding, big) in [
            ([0xFF, 0xFE], "UTF-16LE", false),
            ([0xFE, 0xFF], "UTF-16BE", true),
        ] {
            let mut bytes = mark.to_vec();
            for unit in "가나다\nabc".encode_utf16() {
                let pair = if big {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                };
                bytes.extend_from_slice(&pair);
            }
            let read = text(read_text(&bytes, true));
            assert_eq!(read.encoding, encoding);
            assert_eq!(read.lines, ["가나다", "abc"]);
        }
    }

    #[test]
    fn a_part_cut_inside_a_character_reads_up_to_it() {
        let bytes = "가나다라".as_bytes();
        // Cut in the middle of 라 (three bytes in UTF-8): the read stops before it.
        let read = text(read_text(&bytes[..10], false));
        assert_eq!(read.lines, ["가나다"]);
        // EUC-KR cut in the middle of a syllable.
        let (euc, _, _) = encoding_rs::EUC_KR.encode(KOREAN);
        let read = text(read_text(&euc[..euc.len() - 1], false));
        assert_eq!(read.encoding, "EUC-KR");
    }

    #[test]
    fn bytes_that_are_no_text_are_told() {
        assert_eq!(
            read_text(b"PK\x03\x04\x00\x00binary", true),
            TextRead::Binary
        );
        let controls: Vec<u8> = (0..200u8).map(|i| i % 8).collect();
        assert_eq!(read_text(&controls, true), TextRead::Binary);
        // A mark that names UTF-8 before bytes that are not.
        let mut broken = vec![0xEF, 0xBB, 0xBF];
        broken.extend_from_slice(&[b'a', 0xFF, 0xFE, 0xC3, b'b']);
        assert_eq!(read_text(&broken, true), TextRead::Malformed);
        // UTF-16 with half a unit at its end.
        assert_eq!(
            read_text(&[0xFF, 0xFE, b'a', 0, b'b'], true),
            TextRead::Malformed
        );
    }

    #[test]
    fn long_lines_are_cut_and_the_last_line_break_ends_the_text() {
        let long = "가".repeat(MAX_LINE_CHARS + 50);
        let read = text(read_text(format!("{long}\nend\n").as_bytes(), true));
        assert_eq!(read.lines.len(), 2);
        assert_eq!(read.lines[0].chars().count(), MAX_LINE_CHARS + 1);
        assert!(read.lines[0].ends_with('…'));
        assert_eq!(read.lines[1], "end");
        // An empty line in the middle stays.
        assert_eq!(text(read_text(b"a\n\nb", true)).lines, ["a", "", "b"]);
    }

    #[test]
    fn a_panic_in_the_work_is_kept_from_the_worker() {
        assert_eq!(guarded(|| 7), Some(7));
        let panicked: Option<u8> = guarded(|| panic!("a decoder's bug"));
        assert_eq!(panicked, None);
    }

    /// A folder for the files of a test, under the system's temporary folder (long on CI).
    fn scratch() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temporary folder")
    }

    fn vpath(path: &Path) -> VPath {
        VPath::local(path).expect("a local path")
    }

    fn request(path: &Path, kind: EntryKind) -> PreviewRequest {
        PreviewRequest {
            generation: 7,
            path: vpath(path),
            kind,
            filter: VisibilityFilter::default(),
        }
    }

    fn preview_of(path: &Path, kind: EntryKind) -> PreviewContent {
        make_preview(
            &request(path, kind),
            &Vfs::with_local(),
            &CancelToken::new(),
        )
        .expect("not stopped")
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("the file is written");
        path
    }

    #[test]
    fn files_are_previewed_by_what_they_hold() {
        let dir = scratch();
        let png = write(dir.path(), "사진.png", &picture(30, 20, ImageFormat::Png));
        match preview_of(&png, EntryKind::File) {
            PreviewContent::Image(image) => assert_eq!((image.width, image.height), (30, 20)),
            other => panic!("not an image: {other:?}"),
        }
        let (euc, _, _) = encoding_rs::EUC_KR.encode(KOREAN);
        let notes = write(dir.path(), "메모.txt", &euc);
        match preview_of(&notes, EntryKind::File) {
            PreviewContent::Text(text) => {
                assert_eq!(text.encoding, "EUC-KR");
                assert_eq!(text.lines[1], "둘째 줄입니다.");
            }
            other => panic!("not text: {other:?}"),
        }
        // A text file without an extension is text too.
        let readme = write(dir.path(), "README", b"hello\n");
        assert!(matches!(
            preview_of(&readme, EntryKind::File),
            PreviewContent::Text(_)
        ));
        let zip = write(dir.path(), "a.zip", b"PK\x03\x04\x00\x00\x00");
        assert_eq!(preview_of(&zip, EntryKind::File), PreviewContent::Plain);
        let empty = write(dir.path(), "empty.txt", b"");
        assert_eq!(preview_of(&empty, EntryKind::File), PreviewContent::Plain);
        let broken = write(dir.path(), "깨진.png", b"\x89PNG\r\n\x1a\nnot really");
        assert_eq!(
            preview_of(&broken, EntryKind::File),
            PreviewContent::Unavailable
        );
        let gone = dir.path().join("없음.txt");
        assert_eq!(
            preview_of(&gone, EntryKind::File),
            PreviewContent::Unavailable
        );
        // A link or a special file shows its facts only, and is not opened.
        assert_eq!(preview_of(&png, EntryKind::Symlink), PreviewContent::Plain);
        assert_eq!(preview_of(&png, EntryKind::Other), PreviewContent::Plain);
    }

    #[test]
    fn only_the_first_part_of_a_long_text_is_read() {
        let dir = scratch();
        let line = "0123456789abcdef\n".repeat(TEXT_BYTES / 17 + 100);
        let long = write(dir.path(), "long.log", line.as_bytes());
        match preview_of(&long, EntryKind::File) {
            PreviewContent::Text(text) => {
                assert!(text.truncated);
                let read: usize = text.lines.iter().map(|line| line.len() + 1).sum();
                assert!(read <= TEXT_BYTES + 1, "{read}");
                assert!(read > TEXT_BYTES - 20, "{read}");
            }
            other => panic!("not text: {other:?}"),
        }
    }

    #[test]
    fn an_image_file_past_50_mb_shows_its_size_only() {
        let dir = scratch();
        let path = dir.path().join("huge.png");
        let file = std::fs::File::create(&path).expect("the file is made");
        file.set_len(MAX_IMAGE_BYTES + 1).expect("the file grows");
        drop(file);
        assert_eq!(preview_of(&path, EntryKind::File), PreviewContent::TooLarge);
    }

    #[test]
    fn a_folder_counts_the_items_the_panes_show_up_to_a_cap() {
        let dir = scratch();
        for name in ["a", "b", "c"] {
            write(dir.path(), name, b"x");
        }
        std::fs::create_dir(dir.path().join("sub")).expect("a folder");
        write(dir.path(), ".hidden", b"x");
        let provider = Vfs::with_local()
            .provider(&vpath(dir.path()))
            .expect("a provider");
        let count = |filter, cap| {
            count_items(
                provider.as_ref(),
                &vpath(dir.path()),
                filter,
                &CancelToken::new(),
                cap,
            )
        };
        let shown = VisibilityFilter::default();
        let all = VisibilityFilter {
            show_hidden: true,
            ..shown
        };
        // `.hidden` is hidden on every system the tests run on: a dot file on Unix; Windows
        // needs the attribute.
        if cfg!(windows) {
            rf_platform::set_attributes(&vpath(&dir.path().join(".hidden")), Attributes::HIDDEN)
                .expect("the file is hidden");
        }
        assert_eq!(
            count(shown, 100),
            Some(PreviewContent::Folder {
                items: 4,
                more: false
            })
        );
        assert_eq!(
            count(all, 100),
            Some(PreviewContent::Folder {
                items: 5,
                more: false
            })
        );
        assert_eq!(
            count(all, 3),
            Some(PreviewContent::Folder {
                items: 3,
                more: true
            })
        );
        // Stopped, it gives nothing.
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(
            count_items(provider.as_ref(), &vpath(dir.path()), all, &cancel, 100),
            None
        );
        // A folder that is not there cannot be previewed.
        assert_eq!(
            count_items(
                provider.as_ref(),
                &vpath(&dir.path().join("없음")),
                all,
                &CancelToken::new(),
                100
            ),
            Some(PreviewContent::Unavailable)
        );
        // Through the request.
        assert_eq!(
            preview_of(dir.path(), EntryKind::Dir),
            PreviewContent::Folder {
                items: 4,
                more: false
            }
        );
    }
}
