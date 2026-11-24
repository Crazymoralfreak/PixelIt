//! JPEG decoding with zune-jpeg.

use pixelit_core::{Error, Image, Media, Result};
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

use super::{bad, Loaded};
use crate::info::DecodeOptions;
use crate::metadata::{self, Metadata};

/// Components of CMYK and YCCK files.
const CMYK_COMPONENTS: u8 = 4;

pub(super) fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Loaded> {
    let side = options.limits.max_side as usize;
    let settings = DecoderOptions::default()
        .set_max_width(side)
        .set_max_height(side)
        .jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), settings);
    decoder.decode_headers().map_err(bad)?;
    let (width, height) = decoder.dimensions().ok_or_else(|| Error::new("badFile"))?;
    let (width, height) = (width as u32, height as u32);
    options.limits.check_size(width, height)?;
    let cmyk = decoder
        .info()
        .is_some_and(|info| info.components == CMYK_COMPONENTS);

    let data = decoder.decode().map_err(bad)?;
    let pixels = width as usize * height as usize;
    if pixels == 0 || data.len() % pixels != 0 {
        return Err(Error::new("badFile"));
    }
    let image = Image::from_channels8(width, height, data.len() / pixels, &data)?;

    // The decoder converts CMYK to RGB itself, so a CMYK profile no longer
    // describes the pixels and is left out.
    let icc = if cmyk { None } else { decoder.icc_profile() };
    let metadata = Metadata {
        exif: decoder.exif().cloned().and_then(metadata::normalize_exif),
        icc,
        xmp: decoder.xmp().cloned(),
    };
    let orientation = metadata
        .exif
        .as_deref()
        .and_then(metadata::orientation)
        .unwrap_or(1);
    Ok(Loaded {
        media: Media::Still(image),
        bits: 8,
        metadata,
        orientation,
        pages: 0,
    })
}

/// Width, height and EXIF orientation read from the headers.
pub(super) fn probe(bytes: &[u8]) -> Option<(u32, u32, u16)> {
    let mut decoder = JpegDecoder::new(ZCursor::new(bytes));
    decoder.decode_headers().ok()?;
    let (width, height) = decoder.dimensions()?;
    let stored = decoder
        .exif()
        .cloned()
        .and_then(metadata::normalize_exif)
        .as_deref()
        .and_then(metadata::orientation)
        .unwrap_or(1);
    Some((width as u32, height as u32, stored))
}
