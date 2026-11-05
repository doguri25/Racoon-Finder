#![allow(
    unsafe_code,
    reason = "Quick Look's thumbnail generator and the Core Graphics bitmap its picture is drawn in"
)]

//! Quick Look's thumbnails: `QLThumbnailGenerator.sharedGenerator`, asked for the `.thumbnail`
//! representation of one file and waited for on the calling thread.
//!
//! The generator does not need the main thread: none of `QLThumbnailGenerator`,
//! `QLThumbnailGenerationRequest` or `QLThumbnailRepresentation` is marked for the main actor in
//! the headers (so objc2 gives them no `MainThreadMarker`), and the generator calls its handler
//! on a queue of its own. The handler turns the picture into RGBA there and sends it over a
//! channel; the calling thread waits for it at most [`MAC_WAIT`].

use std::ffi::c_void;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::autoreleasepool;
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_foundation::NSError;
use objc2_quick_look_thumbnailing::{
    QLThumbnailErrorDomain, QLThumbnailGenerationRequest,
    QLThumbnailGenerationRequestRepresentationTypes, QLThumbnailGenerator,
    QLThumbnailRepresentation,
};

use super::{MAC_WAIT, MAX_SIDE, Thumb, ThumbOutcome, no_quicklook_thumbnail, unpremultiply};
use crate::clipboard::file_url;

pub(super) const SUPPORTED: bool = true;

pub(super) fn thumbnail(path: &Path, size: u32) -> ThumbOutcome {
    let (sender, answer) = mpsc::channel::<ThumbOutcome>();
    let asked = autoreleasepool(|_| {
        let url = match file_url(path) {
            Ok(url) => url,
            Err(error) => return Err(ThumbOutcome::Failed(error.to_string())),
        };
        let side = CGFloat::from(size.min(MAX_SIDE));
        // SAFETY: an initializer of a freshly allocated request, with a live file URL, a
        // positive size and a scale of 1.
        let request = unsafe {
            QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
                QLThumbnailGenerationRequest::alloc(),
                &url,
                CGSize::new(side, side),
                1.0,
                QLThumbnailGenerationRequestRepresentationTypes::Thumbnail,
            )
        };
        // Called once, on a queue of Quick Look's: it reads the picture there and only sends on
        // the channel, whose sender may be used from any thread. A send after the caller gave
        // up goes nowhere. A panic stays here rather than unwind into Quick Look's queue.
        let done = RcBlock::new(
            move |representation: *mut QLThumbnailRepresentation, error: *mut NSError| {
                // SAFETY: Quick Look passes either nil or a live object for each, valid for the
                // handler's call.
                let (representation, error) = unsafe { (representation.as_ref(), error.as_ref()) };
                let read = panic::catch_unwind(AssertUnwindSafe(|| {
                    autoreleasepool(|_| outcome(representation, error))
                }));
                let _ = sender.send(read.unwrap_or_else(|_| {
                    ThumbOutcome::Failed("reading Quick Look's picture panicked".into())
                }));
            },
        );
        // SAFETY: a class method that takes no arguments and returns the shared generator,
        // retained; the class may be used from any thread (it is not main-thread-only, see the
        // module's notes).
        let generator = unsafe { QLThumbnailGenerator::sharedGenerator() };
        // SAFETY: a live request and a handler of the type the method takes; the generator
        // keeps (copies) the block until it has called it.
        unsafe {
            generator.generateBestRepresentationForRequest_completionHandler(&request, &done);
        }
        Ok((generator, request))
    });
    let (generator, request) = match asked {
        Ok(asked) => asked,
        Err(outcome) => return outcome,
    };
    match answer.recv_timeout(MAC_WAIT) {
        Ok(outcome) => outcome,
        Err(RecvTimeoutError::Disconnected) => {
            ThumbOutcome::Failed("Quick Look let the request go unanswered".into())
        }
        Err(RecvTimeoutError::Timeout) => {
            // SAFETY: the request this generator was given.
            unsafe { generator.cancelRequest(&request) };
            ThumbOutcome::Failed("timed out".into())
        }
    }
}

/// What Quick Look's answer means.
fn outcome(
    representation: Option<&QLThumbnailRepresentation>,
    error: Option<&NSError>,
) -> ThumbOutcome {
    if let Some(representation) = representation {
        // SAFETY: a plain getter of a live representation.
        let image = unsafe { representation.CGImage() };
        return rgba_of(&image);
    }
    let Some(error) = error else {
        return ThumbOutcome::None;
    };
    // SAFETY: an immutable `NSString` constant of the thumbnailing framework, set once the
    // framework is loaded, which it is (its generator just answered).
    let quicklook = unsafe { QLThumbnailErrorDomain };
    if quicklook.is_some_and(|domain| *error.domain() == *domain)
        && no_quicklook_thumbnail(error.code())
    {
        return ThumbOutcome::None;
    }
    ThumbOutcome::Failed(format!(
        "{} {}: {}",
        error.domain(),
        error.code(),
        error.localizedDescription()
    ))
}

/// The pixels of `image`, drawn into an sRGB bitmap of 8-bit RGBA (premultiplied, as Core
/// Graphics draws) and divided by their alpha.
fn rgba_of(image: &CGImage) -> ThumbOutcome {
    let (width, height) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    if width == 0 || height == 0 {
        return ThumbOutcome::None;
    }
    let (Ok(w), Ok(h)) = (u32::try_from(width), u32::try_from(height)) else {
        return ThumbOutcome::None;
    };
    if w > MAX_SIDE || h > MAX_SIDE {
        // The same every time for this file: not a thumbnail to keep asking for.
        return ThumbOutcome::None;
    }
    let stride = width * 4;
    let mut rgba = vec![0_u8; stride * height];
    // SAFETY: an immutable `CFString` constant of Core Graphics.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }));
    // R, G, B, A in this order in memory: alpha last, premultiplied, 32 bits big-endian.
    let info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
    {
        // SAFETY: `rgba` holds `height` rows of `stride` bytes, 4 bytes of 8 bits for each of
        // `width` pixels, and outlives the context, which this block drops.
        let context = unsafe {
            CGBitmapContextCreate(
                rgba.as_mut_ptr().cast::<c_void>(),
                width,
                height,
                8,
                stride,
                space.as_deref(),
                info,
            )
        };
        let Some(context) = context else {
            return ThumbOutcome::Failed("no bitmap for Quick Look's picture".into());
        };
        let whole = CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(CGFloat::from(w), CGFloat::from(h)),
        );
        // The bitmap's first row is the picture's top.
        CGContext::draw_image(Some(&context), whole, Some(image));
    }
    unpremultiply(&mut rgba);
    ThumbOutcome::Made(Thumb {
        width: w,
        height: h,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::super::os_thumbnail;
    use super::super::tests::sample_png;
    use super::*;
    use crate::thumbs::tests::assert_is_the_sample;

    #[test]
    #[ignore = "needs an interactive session"]
    fn quicklook_makes_a_thumbnail_of_a_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("그림.png");
        std::fs::write(&path, sample_png()).unwrap();
        match os_thumbnail(&path, 64) {
            ThumbOutcome::Made(thumb) => {
                assert!(thumb.width <= 64 && thumb.height <= 64, "{thumb:?}");
                assert_is_the_sample(&thumb);
            }
            other => panic!("{other:?}"),
        }
    }
}
