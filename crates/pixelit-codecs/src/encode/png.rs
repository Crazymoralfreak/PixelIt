//! PNG and APNG encoding with the `png` crate, optimised by oxipng.

use oxipng::Options;
use pixelit_core::image::Animation;
use pixelit_core::pipeline::Effort;
use pixelit_core::{CancelToken, Depth, Image, Result};
use png::{BitDepth, ColorType, Compression, Encoder};

use super::{failed, EncodeOptions};

/// oxipng preset of the balanced mode.
const BALANCED_PRESET: u8 = 2;
/// oxipng preset of the maximum mode.
const MAX_PRESET: u8 = 5;
/// Frame delays are written in milliseconds.
const DELAY_DENOMINATOR: u16 = 1000;

/// Samples in the byte order PNG stores them, with or without alpha.
fn samples(image: &Image) -> Vec<u8> {
    match (image.depth(), image.has_alpha()) {
        (Depth::Eight, true) => image.to_rgba8(),
        (Depth::Eight, false) => image.to_rgb8(),
        (Depth::Sixteen, true) => big_endian(&image.to_rgba16()),
        (Depth::Sixteen, false) => big_endian(&image.to_rgb16()),
    }
}

fn big_endian(samples: &[u16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_be_bytes())
        .collect()
}

fn configure(encoder: &mut Encoder<'_, &mut Vec<u8>>, image: &Image, effort: Effort) {
    encoder.set_color(if image.has_alpha() {
        ColorType::Rgba
    } else {
        ColorType::Rgb
    });
    encoder.set_depth(match image.depth() {
        Depth::Eight => BitDepth::Eight,
        Depth::Sixteen => BitDepth::Sixteen,
    });
    encoder.set_compression(match effort {
        Effort::Fast => Compression::Fast,
        Effort::Balanced | Effort::Max => Compression::Balanced,
    });
}

/// Recompresses the file without changing a pixel. The bit depth of 16-bit
/// files stays as requested.
fn optimise(file: Vec<u8>, effort: Effort, depth: Depth) -> Vec<u8> {
    let preset = match effort {
        Effort::Fast => return file,
        Effort::Balanced => BALANCED_PRESET,
        Effort::Max => MAX_PRESET,
    };
    let mut settings = Options::from_preset(preset);
    settings.bit_depth_reduction = depth == Depth::Eight;
    settings.optimize_alpha = effort == Effort::Max;
    oxipng::optimize_from_memory(&file, &settings).unwrap_or(file)
}

pub(super) fn encode(image: &Image, options: &EncodeOptions) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = Encoder::new(&mut out, image.width(), image.height());
    configure(&mut encoder, image, options.effort);
    let mut writer = encoder.write_header().map_err(failed)?;
    writer.write_image_data(&samples(image)).map_err(failed)?;
    writer.finish().map_err(failed)?;
    Ok(optimise(out, options.effort, image.depth()))
}

pub(super) fn encode_animation(
    animation: &Animation,
    options: &EncodeOptions,
    cancel: &CancelToken,
) -> Result<Vec<u8>> {
    let first = &animation.frames[0].image;
    // Every frame is written with alpha when any frame has it.
    let alpha = animation.frames.iter().any(|frame| frame.image.has_alpha());
    let mut out = Vec::new();
    let mut encoder = Encoder::new(&mut out, first.width(), first.height());
    encoder.set_color(if alpha {
        ColorType::Rgba
    } else {
        ColorType::Rgb
    });
    encoder.set_depth(BitDepth::Eight);
    encoder.set_compression(match options.effort {
        Effort::Fast => Compression::Fast,
        Effort::Balanced | Effort::Max => Compression::Balanced,
    });
    encoder
        .set_animated(animation.frames.len() as u32, animation.loop_count)
        .map_err(failed)?;
    let mut writer = encoder.write_header().map_err(failed)?;
    for frame in &animation.frames {
        cancel.check()?;
        let delay = u16::try_from(frame.delay_ms).unwrap_or(u16::MAX);
        writer
            .set_frame_delay(delay, DELAY_DENOMINATOR)
            .map_err(failed)?;
        let data = if alpha {
            frame.image.to_rgba8()
        } else {
            frame.image.to_rgb8()
        };
        writer.write_image_data(&data).map_err(failed)?;
    }
    writer.finish().map_err(failed)?;
    Ok(out)
}
