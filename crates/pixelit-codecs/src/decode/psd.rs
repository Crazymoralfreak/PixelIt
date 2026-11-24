//! Photoshop documents: the flattened composite image.

use pixelit_core::{Image, Result};
use psd::Psd;

use super::{bad, Loaded};
use crate::info::DecodeOptions;

pub(super) fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Loaded> {
    let document = Psd::from_bytes(bytes).map_err(bad)?;
    let (width, height) = (document.width(), document.height());
    options.limits.check_size(width, height)?;
    let mut image = Image::from_rgba8(width, height, document.rgba())?;
    image.normalize_alpha();
    Ok(Loaded::still(image, 8))
}
