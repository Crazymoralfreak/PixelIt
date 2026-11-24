//! SVG rasterisation with resvg.

use std::sync::{Arc, OnceLock};

use pixelit_core::{Error, Image, Result};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{fontdb, Options, Tree};

use super::{bad, Loaded};
use crate::info::DecodeOptions;

/// System fonts are scanned once: the scan takes a noticeable time.
fn fonts() -> Arc<fontdb::Database> {
    static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut database = fontdb::Database::new();
            database.load_system_fonts();
            Arc::new(database)
        })
        .clone()
}

pub(super) fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Loaded> {
    let settings = Options {
        fontdb: fonts(),
        ..Options::default()
    };
    let tree = Tree::from_data(bytes, &settings).map_err(bad)?;
    let size = tree.size();
    let (natural_width, natural_height) = (size.width(), size.height());
    let long = natural_width.max(natural_height);
    if !long.is_finite() || long <= 0.0 {
        return Err(Error::new("emptyImage"));
    }
    let scale = options
        .max_side
        .map_or(1.0, |max_side| max_side as f32 / long);
    let width = (natural_width * scale).round().max(1.0) as u32;
    let height = (natural_height * scale).round().max(1.0) as u32;
    options.limits.check_size(width, height)?;

    let mut pixmap = Pixmap::new(width, height).ok_or_else(|| Error::new("tooLarge"))?;
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut image = Image::from_rgba8(width, height, pixmap.take_demultiplied())?;
    image.normalize_alpha();
    Ok(Loaded::still(image, 8))
}
