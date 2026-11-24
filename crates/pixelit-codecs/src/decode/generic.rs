//! PNG, GIF, BMP, TIFF, ICO and WebP decoding with the `image` crate.

use std::io::Cursor;

use image::codecs::gif::GifDecoder;
use image::codecs::png::PngDecoder;
use image::codecs::webp::WebPDecoder;
use image::metadata::LoopCount;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageReader};
use pixelit_core::image::{Animation, Frame};
use pixelit_core::{Error, Image, ImageFormat, Media, Result};

use super::{bad, Loaded};
use crate::info::DecodeOptions;
use crate::metadata::{self, Metadata};

/// Delay given to frames that declare none, as browsers do.
const DEFAULT_DELAY_MS: u32 = 100;

fn kind(format: ImageFormat) -> Option<image::ImageFormat> {
    Some(match format {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Gif => image::ImageFormat::Gif,
        ImageFormat::Bmp => image::ImageFormat::Bmp,
        ImageFormat::Tiff => image::ImageFormat::Tiff,
        ImageFormat::Ico => image::ImageFormat::Ico,
        ImageFormat::Webp => image::ImageFormat::WebP,
        _ => return None,
    })
}

fn limits(options: &DecodeOptions) -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(options.limits.max_side);
    limits.max_image_height = Some(options.limits.max_side);
    limits.max_alloc = Some(options.limits.max_pixels.saturating_mul(8));
    limits
}

/// Converts a decoded `image` buffer into the core layout.
pub(super) fn from_dynamic(dynamic: DynamicImage) -> Result<(Image, u8)> {
    let color = dynamic.color();
    let bits = (color.bits_per_pixel() / u16::from(color.channel_count()).max(1)) as u8;
    let (width, height) = (dynamic.width(), dynamic.height());
    let mut image = if bits > 8 {
        Image::from_rgba16(width, height, dynamic.into_rgba16().into_raw())?
    } else {
        Image::from_rgba8(width, height, dynamic.into_rgba8().into_raw())?
    };
    image.set_has_alpha(color.has_alpha());
    Ok((image, bits.min(16)))
}

fn animation<'a>(decoder: impl AnimationDecoder<'a>, options: &DecodeOptions) -> Result<Animation> {
    let loop_count = match decoder.loop_count() {
        LoopCount::Infinite => 0,
        LoopCount::Finite(count) => count.get(),
    };
    let mut frames = Vec::new();
    for frame in decoder.into_frames() {
        let frame = frame.map_err(bad)?;
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay_ms = match numerator.checked_div(denominator) {
            Some(0) | None => DEFAULT_DELAY_MS,
            Some(delay) => delay,
        };
        let buffer = frame.into_buffer();
        let (width, height) = (buffer.width(), buffer.height());
        options
            .limits
            .check_animation(width, height, frames.len() as u64 + 1)?;
        frames.push(Frame {
            image: Image::from_rgba8(width, height, buffer.into_raw())?,
            delay_ms,
        });
    }
    if frames.is_empty() {
        return Err(Error::new("badFile"));
    }
    Ok(Animation { frames, loop_count })
}

/// Decodes every frame when the file is an animation with several frames.
fn animated(bytes: &[u8], format: ImageFormat, options: &DecodeOptions) -> Result<Option<Loaded>> {
    let animation = match format {
        ImageFormat::Gif => {
            let mut decoder = GifDecoder::new(Cursor::new(bytes)).map_err(bad)?;
            decoder.set_limits(limits(options)).map_err(bad)?;
            animation(decoder, options)?
        }
        ImageFormat::Png => {
            let decoder =
                PngDecoder::with_limits(Cursor::new(bytes), limits(options)).map_err(bad)?;
            if !decoder.is_apng().map_err(bad)? {
                return Ok(None);
            }
            animation(decoder.apng().map_err(bad)?, options)?
        }
        ImageFormat::Webp => {
            let mut decoder = WebPDecoder::new(Cursor::new(bytes)).map_err(bad)?;
            if !decoder.has_animation() {
                return Ok(None);
            }
            decoder.set_limits(limits(options)).map_err(bad)?;
            animation(decoder, options)?
        }
        _ => return Ok(None),
    };
    if animation.frames.len() < 2 {
        return Ok(None);
    }
    Ok(Some(Loaded {
        media: Media::Animated(animation),
        bits: 8,
        metadata: Metadata::default(),
        orientation: 1,
        pages: 0,
    }))
}

pub(super) fn decode(bytes: &[u8], format: ImageFormat, options: &DecodeOptions) -> Result<Loaded> {
    let kind = kind(format).ok_or_else(|| Error::new("unsupportedFormat"))?;
    if options.animation {
        if let Some(loaded) = animated(bytes, format, options)? {
            return Ok(loaded);
        }
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), kind);
    reader.limits(limits(options));
    let mut decoder = reader.into_decoder().map_err(bad)?;
    let (width, height) = decoder.dimensions();
    options.limits.check_size(width, height)?;
    let icc = decoder.icc_profile().ok().flatten();
    let exif = decoder.exif_metadata().ok().flatten();
    let xmp = decoder.xmp_metadata().ok().flatten();
    let orientation = decoder
        .orientation()
        .map_or(1, |orientation| u16::from(orientation.to_exif()));
    let dynamic = DynamicImage::from_decoder(decoder).map_err(bad)?;
    let (image, bits) = from_dynamic(dynamic)?;
    Ok(Loaded {
        media: Media::Still(image),
        bits,
        metadata: Metadata {
            exif: exif.and_then(metadata::normalize_exif),
            icc: icc.filter(|profile| !profile.is_empty()),
            xmp,
        },
        orientation,
        pages: 0,
    })
}

pub(super) fn probe(bytes: &[u8], format: ImageFormat) -> Option<(u32, u32)> {
    ImageReader::with_format(Cursor::new(bytes), kind(format)?)
        .into_dimensions()
        .ok()
}
