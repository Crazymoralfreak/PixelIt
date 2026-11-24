//! WebP encoding: libwebp, or the lossless encoder of the `image` crate when
//! libwebp is not compiled in.

#[cfg(not(feature = "webp-lossy"))]
use image::codecs::webp::WebPEncoder;
#[cfg(not(feature = "webp-lossy"))]
use image::{ExtendedColorType, ImageEncoder};
use pixelit_core::image::Animation;
#[cfg(feature = "webp-lossy")]
use pixelit_core::pipeline::Effort;
use pixelit_core::{CancelToken, Error, Image, Result};
#[cfg(feature = "webp-lossy")]
use webp::{AnimEncoder, AnimFrame, Encoder, WebPConfig};

use super::{failed, EncodeOptions};

/// Largest width and height of a WebP file.
const MAX_SIDE: u32 = 16_383;

fn check_size(width: u32, height: u32) -> Result<()> {
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(Error::new("tooLargeForFormat")
            .with("format", "WebP")
            .with("max", MAX_SIDE));
    }
    Ok(())
}

#[cfg(feature = "webp-lossy")]
fn config(options: &EncodeOptions) -> Result<WebPConfig> {
    let mut config = WebPConfig::new().map_err(|()| Error::new("encodeFailed"))?;
    let (method, lossless_level) = match options.effort {
        Effort::Fast => (2, 30.0),
        Effort::Balanced => (4, 70.0),
        Effort::Max => (6, 100.0),
    };
    config.method = method;
    if options.lossless {
        config.lossless = 1;
        // In the lossless mode the quality field sets the compression effort.
        config.quality = lossless_level;
        // Keep the colour of fully transparent pixels.
        config.exact = 1;
    } else {
        config.lossless = 0;
        config.quality = f32::from(options.clamped_quality());
    }
    Ok(config)
}

#[cfg(feature = "webp-lossy")]
pub(super) fn encode(image: &Image, options: &EncodeOptions) -> Result<Vec<u8>> {
    let (width, height) = (image.width(), image.height());
    check_size(width, height)?;
    let config = config(options)?;
    let data = if image.has_alpha() {
        image.to_rgba8()
    } else {
        image.to_rgb8()
    };
    let encoder = if image.has_alpha() {
        Encoder::from_rgba(&data, width, height)
    } else {
        Encoder::from_rgb(&data, width, height)
    };
    let memory = encoder.encode_advanced(&config).map_err(failed)?;
    Ok(memory.to_vec())
}

#[cfg(not(feature = "webp-lossy"))]
pub(super) fn encode(image: &Image, _options: &EncodeOptions) -> Result<Vec<u8>> {
    let (width, height) = (image.width(), image.height());
    check_size(width, height)?;
    let mut out = Vec::new();
    let encoder = WebPEncoder::new_lossless(&mut out);
    if image.has_alpha() {
        encoder
            .write_image(&image.to_rgba8(), width, height, ExtendedColorType::Rgba8)
            .map_err(failed)?;
    } else {
        encoder
            .write_image(&image.to_rgb8(), width, height, ExtendedColorType::Rgb8)
            .map_err(failed)?;
    }
    Ok(out)
}

/// Animated WebP. The last frame is shown for the average frame time: the
/// libwebp binding closes the animation without a final timestamp.
#[cfg(feature = "webp-lossy")]
pub(super) fn encode_animation(
    animation: &Animation,
    options: &EncodeOptions,
    cancel: &CancelToken,
) -> Result<Vec<u8>> {
    let first = &animation.frames[0].image;
    let (width, height) = (first.width(), first.height());
    check_size(width, height)?;
    let config = config(options)?;
    let buffers: Vec<Vec<u8>> = animation
        .frames
        .iter()
        .map(|frame| frame.image.to_rgba8())
        .collect();
    cancel.check()?;
    let mut encoder = AnimEncoder::new(width, height, &config);
    encoder.set_loop_count(i32::try_from(animation.loop_count).unwrap_or(0));
    let mut timestamp = 0_i32;
    for (frame, buffer) in animation.frames.iter().zip(&buffers) {
        encoder.add_frame(AnimFrame::from_rgba(buffer, width, height, timestamp));
        let delay = i32::try_from(frame.delay_ms).unwrap_or(i32::MAX);
        timestamp = timestamp.saturating_add(delay);
    }
    let memory = encoder.try_encode().map_err(failed)?;
    Ok(memory.to_vec())
}

#[cfg(not(feature = "webp-lossy"))]
pub(super) fn encode_animation(
    _animation: &Animation,
    _options: &EncodeOptions,
    _cancel: &CancelToken,
) -> Result<Vec<u8>> {
    Err(Error::new("cannotEncode").with("format", "animated WebP"))
}
