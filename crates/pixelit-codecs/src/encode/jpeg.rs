//! JPEG encoding: MozJPEG, or the `image` crate when MozJPEG is not compiled in.

#[cfg(not(feature = "mozjpeg"))]
use image::codecs::jpeg::JpegEncoder;
#[cfg(not(feature = "mozjpeg"))]
use image::{ExtendedColorType, ImageEncoder};
#[cfg(feature = "mozjpeg")]
use mozjpeg::{ColorSpace, Compress};
#[cfg(feature = "mozjpeg")]
use pixelit_core::pipeline::{Effort, Subsampling};
use pixelit_core::{Image, Result};

use super::{failed, EncodeOptions};

/// Quality from which chroma keeps its full resolution in the automatic mode.
#[cfg(feature = "mozjpeg")]
const FULL_CHROMA_QUALITY: u8 = 90;

/// Whether chroma is stored at full resolution (4:4:4).
#[cfg(feature = "mozjpeg")]
fn full_chroma(options: &EncodeOptions) -> bool {
    match options.subsampling {
        Subsampling::Auto => options.clamped_quality() >= FULL_CHROMA_QUALITY,
        Subsampling::Full => true,
        Subsampling::Half => false,
    }
}

/// One byte per pixel for images without colour.
fn gray_samples(image: &Image) -> Vec<u8> {
    image
        .to_rgb8()
        .chunks_exact(3)
        .map(|pixel| pixel[0])
        .collect()
}

#[cfg(feature = "mozjpeg")]
pub(super) fn encode(image: &Image, options: &EncodeOptions) -> Result<Vec<u8>> {
    let gray = image.is_gray();
    let mut compress = Compress::new(if gray {
        ColorSpace::JCS_GRAYSCALE
    } else {
        ColorSpace::JCS_RGB
    });
    compress.set_size(image.width() as usize, image.height() as usize);
    match (options.effort, options.progressive) {
        (Effort::Fast, progressive) => {
            compress.set_fastest_defaults();
            compress.set_optimize_coding(true);
            if progressive {
                compress.set_progressive_mode();
            }
        }
        // Dropping the optimised scan script leaves a baseline file that
        // still uses trellis quantisation.
        (_, false) => compress.set_optimize_scans(false),
        (Effort::Balanced, true) => {
            compress.set_optimize_scans(false);
            compress.set_progressive_mode();
        }
        // The default profile searches for the best progressive scan script.
        (Effort::Max, true) => {}
    }
    compress.set_quality(f32::from(options.clamped_quality()));
    if !gray {
        let chroma = if full_chroma(options) { (1, 1) } else { (2, 2) };
        compress.set_chroma_sampling_pixel_sizes(chroma, chroma);
    }

    let samples = if gray {
        gray_samples(image)
    } else {
        image.to_rgb8()
    };
    let mut started = compress.start_compress(Vec::new()).map_err(failed)?;
    started.write_scanlines(&samples).map_err(failed)?;
    started.finish().map_err(failed)
}

#[cfg(not(feature = "mozjpeg"))]
pub(super) fn encode(image: &Image, options: &EncodeOptions) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut out, options.clamped_quality());
    let (width, height) = (image.width(), image.height());
    if image.is_gray() {
        encoder
            .write_image(&gray_samples(image), width, height, ExtendedColorType::L8)
            .map_err(failed)?;
    } else {
        encoder
            .write_image(&image.to_rgb8(), width, height, ExtendedColorType::Rgb8)
            .map_err(failed)?;
    }
    Ok(out)
}
