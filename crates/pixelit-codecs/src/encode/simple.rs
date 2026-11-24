//! TIFF, BMP, GIF and ICO encoding with the `image` crate.

use std::io::Cursor;

use image::codecs::bmp::BmpEncoder;
use image::codecs::gif::{GifEncoder, Repeat};
use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::codecs::tiff::TiffEncoder;
use image::{Delay, ExtendedColorType, Frame, ImageEncoder, RgbaImage};
use pixelit_core::image::Animation;
use pixelit_core::ops::resize::fit_within;
use pixelit_core::pipeline::{Effort, ResizeFilter};
use pixelit_core::{CancelToken, Depth, Error, Image, Result};

use super::{failed, EncodeOptions};

/// Largest width and height of an icon entry.
const ICO_MAX_SIDE: u32 = 256;

pub(super) fn encode_tiff(image: &Image, options: &EncodeOptions) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = TiffEncoder::new(Cursor::new(&mut out));
    if let Some(icc) = &options.metadata.icc {
        // A writer that cannot store the profile still writes the pixels.
        let _ = encoder.set_icc_profile(icc.clone());
    }
    let (width, height) = (image.width(), image.height());
    match (image.depth(), image.has_alpha()) {
        (Depth::Eight, true) => {
            encoder.write_image(&image.to_rgba8(), width, height, ExtendedColorType::Rgba8)
        }
        (Depth::Eight, false) => {
            encoder.write_image(&image.to_rgb8(), width, height, ExtendedColorType::Rgb8)
        }
        (Depth::Sixteen, true) => encoder.write_image(
            bytemuck::cast_slice(&image.to_rgba16()),
            width,
            height,
            ExtendedColorType::Rgba16,
        ),
        (Depth::Sixteen, false) => encoder.write_image(
            bytemuck::cast_slice(&image.to_rgb16()),
            width,
            height,
            ExtendedColorType::Rgb16,
        ),
    }
    .map_err(failed)?;
    Ok(out)
}

pub(super) fn encode_bmp(image: &Image) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    BmpEncoder::new(&mut out)
        .write_image(
            &image.to_rgb8(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgb8,
        )
        .map_err(failed)?;
    Ok(out)
}

fn gif_frame(image: &Image, delay_ms: u32) -> Result<Frame> {
    let buffer = RgbaImage::from_raw(image.width(), image.height(), image.to_rgba8())
        .ok_or_else(|| Error::new("badImage"))?;
    Ok(Frame::from_parts(
        buffer,
        0,
        0,
        Delay::from_numer_denom_ms(delay_ms, 1),
    ))
}

/// Colour quantisation speed of the GIF encoder: `1` is the most careful.
fn gif_speed(effort: Effort) -> i32 {
    match effort {
        Effort::Fast => 30,
        Effort::Balanced => 10,
        Effort::Max => 1,
    }
}

pub(super) fn encode_gif(image: &Image, options: &EncodeOptions) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut encoder = GifEncoder::new_with_speed(&mut out, gif_speed(options.effort));
        encoder.encode_frame(gif_frame(image, 0)?).map_err(failed)?;
    }
    Ok(out)
}

pub(super) fn encode_gif_animation(
    animation: &Animation,
    options: &EncodeOptions,
    cancel: &CancelToken,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut encoder = GifEncoder::new_with_speed(&mut out, gif_speed(options.effort));
        let repeat = match u16::try_from(animation.loop_count) {
            Ok(0) | Err(_) => Repeat::Infinite,
            Ok(count) => Repeat::Finite(count),
        };
        encoder.set_repeat(repeat).map_err(failed)?;
        for frame in &animation.frames {
            cancel.check()?;
            encoder
                .encode_frame(gif_frame(&frame.image, frame.delay_ms)?)
                .map_err(failed)?;
        }
    }
    Ok(out)
}

/// One icon entry stored as PNG. Larger images are scaled down to fit.
pub(super) fn encode_ico(image: &Image) -> Result<Vec<u8>> {
    let fitted = fit_within(image, ICO_MAX_SIDE, ResizeFilter::Lanczos3)?;
    let entry = IcoFrame::as_png(
        &fitted.to_rgba8(),
        fitted.width(),
        fitted.height(),
        ExtendedColorType::Rgba8,
    )
    .map_err(failed)?;
    let mut out = Vec::new();
    IcoEncoder::new(&mut out)
        .encode_images(&[entry])
        .map_err(failed)?;
    Ok(out)
}
