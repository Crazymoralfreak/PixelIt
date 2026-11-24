//! Reading image files of every supported format.

mod avif;
mod generic;
#[allow(unsafe_code)]
mod heic;
mod jpeg;
mod jxl;
mod pdf;
mod psd;
mod raw;
mod svg;

pub use avif::{
    helper_main as avif_helper_main, set_helper as set_avif_helper,
    HELPER_ARGUMENT as AVIF_HELPER_ARGUMENT,
};

use std::fmt::Display;
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use pixelit_core::image::Animation;
use pixelit_core::ops::geometry::apply_orientation;
use pixelit_core::{CancelToken, Depth, Error, Image, ImageFormat, Media, Result};

use crate::color;
use crate::info::{DecodeOptions, Decoded, SourceInfo};
use crate::metadata::{self, Metadata};

/// Bytes inspected to recognise a format.
const SNIFF_BYTES: usize = 512;

/// The result of one format decoder before the shared finishing steps.
pub(crate) struct Loaded {
    pub media: Media,
    /// Bits per channel of the source.
    pub bits: u8,
    pub metadata: Metadata,
    /// EXIF orientation (1 to 8) that still has to be applied to the pixels.
    pub orientation: u16,
    /// Pages of a document; `0` takes the frame count of the media.
    pub pages: u32,
}

impl Loaded {
    pub(crate) fn still(image: Image, bits: u8) -> Self {
        Self {
            media: Media::Still(image),
            bits,
            metadata: Metadata::default(),
            orientation: 1,
            pages: 0,
        }
    }
}

/// An error for a file the decoder rejected.
pub(crate) fn bad(error: impl Display) -> Error {
    Error::new("badFile").detail(error)
}

/// The format of a file judged by its first bytes and its extension.
pub fn detect(head: &[u8], extension: Option<&str>) -> Option<ImageFormat> {
    let by_extension = extension.and_then(ImageFormat::from_extension);
    let head = &head[..head.len().min(SNIFF_BYTES)];
    // Camera RAW files are TIFF containers: the extension tells them apart.
    if by_extension == Some(ImageFormat::Raw) {
        return by_extension;
    }
    ImageFormat::sniff(head).or(by_extension)
}

fn extension_of(path: &Path) -> Option<&str> {
    path.extension().and_then(|extension| extension.to_str())
}

fn read(path: &Path, options: &DecodeOptions) -> Result<Vec<u8>> {
    let size = fs::metadata(path)?.len();
    options.limits.check_file(size)?;
    Ok(fs::read(path)?)
}

fn dispatch(bytes: &[u8], format: ImageFormat, options: &DecodeOptions) -> Result<Loaded> {
    match format {
        ImageFormat::Jpeg => jpeg::decode(bytes, options),
        ImageFormat::Png
        | ImageFormat::Gif
        | ImageFormat::Bmp
        | ImageFormat::Tiff
        | ImageFormat::Ico
        | ImageFormat::Webp => generic::decode(bytes, format, options),
        ImageFormat::Avif => avif::decode(bytes, options),
        ImageFormat::Jxl => jxl::decode(bytes, options),
        ImageFormat::Heic => heic::decode(bytes, options),
        ImageFormat::Raw => raw::decode(bytes, options),
        ImageFormat::Svg => svg::decode(bytes, options),
        ImageFormat::Psd => psd::decode(bytes, options),
        ImageFormat::Pdf => pdf::decode(bytes, options),
    }
}

fn finish_image(mut image: Image, loaded_icc: Option<&[u8]>, turn: u16, deep: bool) -> Image {
    if turn > 1 {
        image = apply_orientation(image, turn);
    }
    if let Some(icc) = loaded_icc {
        color::to_srgb(&mut image, icc);
    }
    if !deep {
        image = image.into_depth(Depth::Eight);
    }
    image.normalize_alpha();
    image
}

fn finish(loaded: Loaded, format: ImageFormat, options: &DecodeOptions) -> Decoded {
    let Loaded {
        media,
        bits,
        mut metadata,
        orientation,
        pages,
    } = loaded;
    let turn = if (2..=8).contains(&orientation) {
        orientation
    } else {
        1
    };
    if let Some(exif) = metadata.exif.as_mut() {
        metadata::reset_orientation(exif);
    }
    let icc = metadata.icc.as_deref();
    let media = match media {
        Media::Still(image) => Media::Still(finish_image(image, icc, turn, options.deep)),
        Media::Animated(animation) => Media::Animated(Animation {
            loop_count: animation.loop_count,
            frames: animation
                .frames
                .into_iter()
                .map(|mut frame| {
                    frame.image = finish_image(frame.image, icc, turn, options.deep);
                    frame
                })
                .collect(),
        }),
    };
    let (width, height, has_alpha) = media.first().map_or((0, 0, false), |image| {
        (image.width(), image.height(), image.has_alpha())
    });
    let frames = if pages > 0 {
        pages
    } else {
        media.frame_count() as u32
    };
    let info = SourceInfo {
        format: Some(format),
        width,
        height,
        bits,
        has_alpha,
        frames,
        orientation: turn,
        has_icc: metadata.icc.is_some(),
        has_exif: metadata.exif.is_some(),
        has_xmp: metadata.xmp.is_some(),
    };
    Decoded {
        media,
        info,
        metadata,
    }
}

/// Decodes a file held in memory. `hint` names the format when the caller
/// knows it; otherwise the first bytes decide.
pub fn decode_bytes(
    bytes: &[u8],
    hint: Option<ImageFormat>,
    options: &DecodeOptions,
    cancel: &CancelToken,
) -> Result<Decoded> {
    cancel.check()?;
    options.limits.check_file(bytes.len() as u64)?;
    let format = hint
        .or_else(|| detect(bytes, None))
        .ok_or_else(|| Error::new("unsupportedFormat"))?;
    // Third-party decoders may panic on malformed input; a panic becomes an
    // ordinary error and the application keeps running.
    let loaded = catch_unwind(AssertUnwindSafe(|| dispatch(bytes, format, options)))
        .unwrap_or_else(|_| Err(Error::new("badFile").detail("decoder panicked")))?;
    cancel.check()?;
    Ok(finish(loaded, format, options))
}

/// Decodes a file from disk.
pub fn decode_file(path: &Path, options: &DecodeOptions, cancel: &CancelToken) -> Result<Decoded> {
    cancel.check()?;
    let bytes = read(path, options)?;
    let format = detect(&bytes, extension_of(path))
        .ok_or_else(|| Error::new("unsupportedFormat").with("path", path.display()))?;
    decode_bytes(&bytes, Some(format), options, cancel)
        .map_err(|error| error.with("path", path.display()))
}

/// Finds the largest `ispe` (image spatial extent) box of an ISO BMFF file.
fn bmff_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const HEAD: usize = 1 << 20;
    let head = &bytes[..bytes.len().min(HEAD)];
    let mut best: Option<(u32, u32)> = None;
    let mut at = 0;
    while let Some(found) = head[at..].windows(4).position(|window| window == b"ispe") {
        let start = at + found + 8;
        let field = |offset: usize| -> Option<u32> {
            let raw: [u8; 4] = head.get(offset..offset + 4)?.try_into().ok()?;
            Some(u32::from_be_bytes(raw))
        };
        if let (Some(width), Some(height)) = (field(start), field(start + 4)) {
            let larger = best.is_none_or(|(best_width, best_height)| {
                u64::from(width) * u64::from(height)
                    > u64::from(best_width) * u64::from(best_height)
            });
            if larger {
                best = Some((width, height));
            }
        }
        at += found + 4;
    }
    best
}

fn quick_probe(bytes: &[u8], format: ImageFormat) -> Option<SourceInfo> {
    let base = |width: u32, height: u32| SourceInfo {
        format: Some(format),
        width,
        height,
        bits: 8,
        frames: 1,
        orientation: 1,
        ..SourceInfo::default()
    };
    match format {
        ImageFormat::Jpeg => jpeg::probe(bytes).map(|(width, height, stored)| {
            let (width, height) = if stored >= 5 {
                (height, width)
            } else {
                (width, height)
            };
            SourceInfo {
                orientation: stored,
                ..base(width, height)
            }
        }),
        ImageFormat::Png
        | ImageFormat::Gif
        | ImageFormat::Bmp
        | ImageFormat::Tiff
        | ImageFormat::Ico
        | ImageFormat::Webp => {
            generic::probe(bytes, format).map(|(width, height)| base(width, height))
        }
        ImageFormat::Avif | ImageFormat::Heic => {
            bmff_size(bytes).map(|(width, height)| base(width, height))
        }
        ImageFormat::Jxl => jxl::probe(bytes).map(|(width, height)| base(width, height)),
        ImageFormat::Raw | ImageFormat::Svg | ImageFormat::Psd | ImageFormat::Pdf => None,
    }
}

/// Reads the size and format of a file held in memory. Formats without a
/// cheap header are decoded.
pub fn probe_bytes(bytes: &[u8], hint: Option<ImageFormat>) -> Result<SourceInfo> {
    let format = hint
        .or_else(|| detect(bytes, None))
        .ok_or_else(|| Error::new("unsupportedFormat"))?;
    let quick = catch_unwind(AssertUnwindSafe(|| quick_probe(bytes, format))).unwrap_or(None);
    if let Some(info) = quick {
        return Ok(info);
    }
    let options = DecodeOptions {
        animation: false,
        ..DecodeOptions::default()
    };
    decode_bytes(bytes, Some(format), &options, &CancelToken::new()).map(|decoded| decoded.info)
}

/// Reads the size and format of a file on disk.
pub fn probe_file(path: &Path) -> Result<SourceInfo> {
    let bytes = read(path, &DecodeOptions::default())?;
    let format = detect(&bytes, extension_of(path))
        .ok_or_else(|| Error::new("unsupportedFormat").with("path", path.display()))?;
    probe_bytes(&bytes, Some(format)).map_err(|error| error.with("path", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_wins_for_raw_files() {
        let tiff_head = [b'I', b'I', 42, 0, 8, 0, 0, 0];
        assert_eq!(detect(&tiff_head, Some("nef")), Some(ImageFormat::Raw));
        assert_eq!(detect(&tiff_head, Some("tif")), Some(ImageFormat::Tiff));
        assert_eq!(detect(&tiff_head, None), Some(ImageFormat::Tiff));
    }

    #[test]
    fn content_wins_over_wrong_extension() {
        let png_head = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(detect(&png_head, Some("jpg")), Some(ImageFormat::Png));
        assert_eq!(detect(b"????", Some("bmp")), Some(ImageFormat::Bmp));
        assert_eq!(detect(b"????", None), None);
    }

    #[test]
    fn finds_bmff_size() {
        let mut data = vec![0_u8; 32];
        data.extend_from_slice(b"ispe");
        data.extend_from_slice(&[0, 0, 0, 0]);
        data.extend_from_slice(&640_u32.to_be_bytes());
        data.extend_from_slice(&480_u32.to_be_bytes());
        data.extend_from_slice(b"ispe");
        data.extend_from_slice(&[0, 0, 0, 0]);
        data.extend_from_slice(&4000_u32.to_be_bytes());
        data.extend_from_slice(&3000_u32.to_be_bytes());
        assert_eq!(bmff_size(&data), Some((4000, 3000)));
        assert_eq!(bmff_size(b"ispe"), None);
        assert_eq!(bmff_size(&[]), None);
    }

    #[test]
    fn garbage_is_an_error() {
        let options = DecodeOptions::default();
        let cancel = CancelToken::new();
        let error = decode_bytes(b"definitely not an image", None, &options, &cancel)
            .err()
            .unwrap();
        assert_eq!(error.code, "unsupportedFormat");
        for format in [
            ImageFormat::Jpeg,
            ImageFormat::Png,
            ImageFormat::Gif,
            ImageFormat::Webp,
            ImageFormat::Tiff,
            ImageFormat::Bmp,
            ImageFormat::Ico,
            ImageFormat::Avif,
            ImageFormat::Jxl,
            ImageFormat::Psd,
            ImageFormat::Svg,
            ImageFormat::Raw,
        ] {
            let result = decode_bytes(b"definitely not an image", Some(format), &options, &cancel);
            assert!(result.is_err(), "{format:?}");
        }
    }

    #[test]
    fn cancelled_before_start() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let error = decode_bytes(b"x", None, &DecodeOptions::default(), &cancel)
            .err()
            .unwrap();
        assert!(error.is_cancelled());
    }
}
