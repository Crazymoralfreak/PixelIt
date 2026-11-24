//! Camera RAW decoding with rawler: demosaic, white balance and sRGB conversion.

use pixelit_core::{Error, Media, Result};
use rawler::decoders::RawDecodeParams;
use rawler::imgop::develop::RawDevelop;
use rawler::rawsource::RawSource;

use super::generic::from_dynamic;
use super::{bad, Loaded};
use crate::info::DecodeOptions;
use crate::metadata::Metadata;

pub(super) fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Loaded> {
    let source = RawSource::new_from_slice(bytes);
    let decoder = rawler::get_decoder(&source).map_err(bad)?;
    let raw = decoder
        .raw_image(&source, &RawDecodeParams::default(), false)
        .map_err(bad)?;
    let width = u32::try_from(raw.width).map_err(|_| Error::new("tooLarge"))?;
    let height = u32::try_from(raw.height).map_err(|_| Error::new("tooLarge"))?;
    options.limits.check_size(width, height)?;
    let stored = raw.orientation.to_u16();

    let developed = RawDevelop::default()
        .develop_intermediate(&raw)
        .map_err(bad)?
        .to_dynamic_image()
        .ok_or_else(|| Error::new("badFile"))?;
    let (image, bits) = from_dynamic(developed)?;
    let image = if (2..=8).contains(&stored) {
        pixelit_core::ops::geometry::apply_orientation(image, stored)
    } else {
        image
    };
    Ok(Loaded {
        media: Media::Still(image),
        bits,
        metadata: Metadata::default(),
        orientation: 1,
        pages: 0,
    })
}
