//! PDF pages rendered with the PDFium library.
//!
//! PDFium is a separate component: the library is loaded from the path in the
//! decoding options, from the folder of the executable or from the system.

use std::env;
use std::path::{Path, PathBuf};

use pdfium_render::prelude::{
    PdfColor, PdfRenderConfig, Pdfium, PdfiumError, PdfiumInternalError, PdfiumLibraryBindings,
};
use pixelit_core::{Error, Image, Media, Result};

use super::{bad, Loaded};
use crate::info::DecodeOptions;
use crate::metadata::Metadata;

/// PDF user space units per inch.
const POINTS_PER_INCH: f32 = 72.0;

fn library_file(path: &Path) -> PathBuf {
    if path.is_dir() {
        Pdfium::pdfium_platform_library_name_at_path(path)
    } else {
        path.to_path_buf()
    }
}

fn bind(options: &DecodeOptions) -> Result<Box<dyn PdfiumLibraryBindings>> {
    let beside_executable = env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf));
    let candidates = options
        .pdfium
        .iter()
        .cloned()
        .chain(beside_executable)
        .map(|path| library_file(&path));
    for candidate in candidates {
        if candidate.is_file() {
            if let Ok(bindings) = Pdfium::bind_to_library(&candidate) {
                return Ok(bindings);
            }
        }
    }
    Pdfium::bind_to_system_library().map_err(|_| Error::new("pdfiumMissing"))
}

fn document_error(error: PdfiumError) -> Error {
    match error {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            Error::new("pdfPassword")
        }
        other => bad(other),
    }
}

pub(super) fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Loaded> {
    let pdfium = Pdfium::new(bind(options)?);
    let document = pdfium
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(document_error)?;
    let pages = document.pages();
    let count = u32::try_from(pages.len()).unwrap_or(0);
    if options.page >= count {
        return Err(Error::new("badPage")
            .with("page", options.page + 1)
            .with("pages", count));
    }
    let page = pages.get(options.page as _).map_err(bad)?;

    let (points_wide, points_high) = (page.width().value, page.height().value);
    let long = points_wide.max(points_high);
    if !long.is_finite() || long <= 0.0 {
        return Err(Error::new("emptyImage"));
    }
    let scale = options.max_side.map_or_else(
        || options.density.clamp(18.0, 1200.0) / POINTS_PER_INCH,
        |max_side| max_side as f32 / long,
    );
    let width = (points_wide * scale).round().max(1.0) as u32;
    let height = (points_high * scale).round().max(1.0) as u32;
    options.limits.check_size(width, height)?;

    let config = PdfRenderConfig::new()
        .set_target_size(width as _, height as _)
        .set_clear_color(PdfColor::WHITE);
    let bitmap = page.render_with_config(&config).map_err(bad)?;
    let (width, height) = (bitmap.width() as u32, bitmap.height() as u32);
    let mut image = Image::from_rgba8(width, height, bitmap.as_rgba_bytes())?;
    image.set_has_alpha(false);
    Ok(Loaded {
        media: Media::Still(image),
        bits: 8,
        metadata: Metadata::default(),
        orientation: 1,
        pages: count,
    })
}
