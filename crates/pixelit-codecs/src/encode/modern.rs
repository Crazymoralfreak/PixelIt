//! AVIF (ravif) and JPEG XL (jixel) encoding.

use jixel::{EncodeConfig, Speed};
use pixelit_core::pipeline::Effort;
use pixelit_core::{Depth, Image, Result};
use ravif::{Encoder, Img};
use rgb::FromSlice;

use super::{failed, EncodeOptions};

pub(super) fn encode_avif(image: &Image, options: &EncodeOptions) -> Result<Vec<u8>> {
    let quality = f32::from(options.clamped_quality());
    let speed = match options.effort {
        Effort::Fast => 8,
        Effort::Balanced => 6,
        Effort::Max => 3,
    };
    let mut encoder = Encoder::new()
        .with_quality(quality)
        .with_alpha_quality(quality)
        .with_speed(speed);
    if let Some(exif) = &options.metadata.exif {
        encoder = encoder.with_exif(exif.clone());
    }
    let (width, height) = (image.width() as usize, image.height() as usize);
    let encoded = if image.has_alpha() {
        let data = image.to_rgba8();
        encoder
            .encode_rgba(Img::new(data.as_rgba(), width, height))
            .map_err(failed)?
    } else {
        let data = image.to_rgb8();
        encoder
            .encode_rgb(Img::new(data.as_rgb(), width, height))
            .map_err(failed)?
    };
    Ok(encoded.avif_file)
}

pub(super) fn encode_jxl(image: &Image, options: &EncodeOptions) -> Result<Vec<u8>> {
    let mut config = EncodeConfig::default().with_speed(match options.effort {
        Effort::Fast => Speed::Fastest,
        Effort::Balanced => Speed::Fast,
        Effort::Max => Speed::Slow,
    });
    config = if options.lossless {
        config.with_lossless(true)
    } else {
        config.with_quality(f32::from(options.clamped_quality()))
    };
    let metadata = &options.metadata;
    if let Some(icc) = &metadata.icc {
        config = config.with_icc_profile(icc.clone());
    }
    if let Some(exif) = &metadata.exif {
        config = config.with_exif(exif.clone());
    }
    if let Some(xmp) = &metadata.xmp {
        config = config.with_xmp(xmp.clone());
    }

    let (width, height) = (image.width() as usize, image.height() as usize);
    match (image.depth(), image.has_alpha()) {
        (Depth::Eight, false) => jixel::encode_image(&image.to_rgb8(), width, height, &config),
        (Depth::Eight, true) => {
            jixel::encode_image_with_alpha(&image.to_rgba8(), width, height, &config)
        }
        (Depth::Sixteen, false) => {
            jixel::encode_image_16bit(&image.to_rgb16(), width, height, &config)
        }
        (Depth::Sixteen, true) => {
            jixel::encode_image_with_alpha_16bit(&image.to_rgba16(), width, height, &config)
        }
    }
    .map_err(failed)
}
