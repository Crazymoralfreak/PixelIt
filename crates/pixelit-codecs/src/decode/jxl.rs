//! JPEG XL decoding with jxl-oxide.

use jxl_oxide::{AuxBoxData, JxlImage};
use pixelit_core::image::{Animation, Frame};
use pixelit_core::{Error, Image, Media, Result};

use super::{bad, Loaded};
use crate::info::DecodeOptions;
use crate::metadata::{self, Metadata};

/// Delay given to animation frames that declare none.
const DEFAULT_DELAY_MS: u32 = 100;

fn render(source: &JxlImage, index: usize, deep: bool) -> Result<Image> {
    let frame = source.render_frame(index).map_err(bad)?;
    let mut stream = frame.stream();
    let (width, height) = (stream.width(), stream.height());
    let channels = stream.channels() as usize;
    if !(1..=4).contains(&channels) {
        return Err(Error::new("unsupportedFormat").detail("colour model"));
    }
    let samples = width as usize * height as usize * channels;
    if deep {
        let mut buffer = vec![0_u16; samples];
        stream.write_to_buffer(&mut buffer);
        Image::from_channels16(width, height, channels, &buffer)
    } else {
        let mut buffer = vec![0_u8; samples];
        stream.write_to_buffer(&mut buffer);
        Image::from_channels8(width, height, channels, &buffer)
    }
}

pub(super) fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Loaded> {
    let source = JxlImage::builder().read(bytes).map_err(bad)?;
    options.limits.check_size(source.width(), source.height())?;
    if source.pixel_format().has_black() {
        return Err(Error::new("unsupportedFormat").detail("CMYK"));
    }
    let header = &source.image_header().metadata;
    let bits = header.bit_depth.bits_per_sample().min(16) as u8;
    let deep = bits > 8;
    // Ticks per second of an animation as a fraction.
    let ticks = header
        .animation
        .as_ref()
        .map(|animation| (animation.tps_numerator, animation.tps_denominator));
    let loop_count = header
        .animation
        .as_ref()
        .map_or(0, |animation| animation.num_loops);

    let count = source.num_loaded_keyframes();
    if count == 0 {
        return Err(Error::new("badFile"));
    }
    let media = match ticks {
        Some((numerator, denominator)) if options.animation && count > 1 => {
            let mut frames = Vec::with_capacity(count);
            for index in 0..count {
                options.limits.check_animation(
                    source.width(),
                    source.height(),
                    index as u64 + 1,
                )?;
                let duration = source
                    .frame_header(index)
                    .map_or(0, |header| u64::from(header.duration));
                let delay = (duration * 1000 * u64::from(denominator))
                    .checked_div(u64::from(numerator))
                    .unwrap_or(0);
                frames.push(Frame {
                    image: render(&source, index, deep)?,
                    delay_ms: match u32::try_from(delay) {
                        Ok(0) | Err(_) => DEFAULT_DELAY_MS,
                        Ok(delay) => delay,
                    },
                });
            }
            Media::Animated(Animation { frames, loop_count })
        }
        _ => Media::Still(render(&source, 0, deep)?),
    };

    let exif = match source.aux_boxes().first_exif() {
        Ok(AuxBoxData::Data(exif)) => metadata::normalize_exif(exif.payload().to_vec()),
        _ => None,
    };
    let xmp = match source.aux_boxes().first_xml() {
        AuxBoxData::Data(xml) => Some(xml.to_vec()),
        _ => None,
    };
    Ok(Loaded {
        media,
        bits,
        metadata: Metadata {
            exif,
            // The profile of the rendered pixels; equal to sRGB for most files.
            icc: Some(source.rendered_icc()).filter(|profile| !profile.is_empty()),
            xmp,
        },
        // The decoder applies the orientation stored in the codestream.
        orientation: 1,
        pages: 0,
    })
}

pub(super) fn probe(bytes: &[u8]) -> Option<(u32, u32)> {
    let source = JxlImage::builder().read(bytes).ok()?;
    Some((source.width(), source.height()))
}
