//! Decoding options and the description of a source file.

use std::path::PathBuf;

use pixelit_core::{ImageFormat, Limits, Media};
use serde::{Deserialize, Serialize};

use crate::metadata::Metadata;

/// Resolution used to rasterise PDF pages when no size hint is given.
pub const DEFAULT_DENSITY: f32 = 150.0;

#[derive(Clone, Debug)]
pub struct DecodeOptions {
    pub limits: Limits,
    /// Longest side the caller needs. Vector formats (SVG, PDF) are rasterised
    /// to this size; raster decoders may ignore the hint.
    pub max_side: Option<u32>,
    /// Zero-based page of a multi-page document (PDF, TIFF).
    pub page: u32,
    /// Resolution of rasterised PDF pages, in dots per inch.
    pub density: f32,
    /// Decode every frame of an animated file.
    pub animation: bool,
    /// Keep more than 8 bits per channel when the source has them.
    pub deep: bool,
    /// Folder or file of the PDFium library used for PDF pages.
    pub pdfium: Option<PathBuf>,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            max_side: None,
            page: 0,
            density: DEFAULT_DENSITY,
            animation: true,
            deep: true,
            pdfium: None,
        }
    }
}

impl DecodeOptions {
    /// Options for previews and thumbnails: first frame, 8 bits.
    pub fn preview(max_side: u32) -> Self {
        Self {
            max_side: Some(max_side),
            animation: false,
            deep: false,
            ..Self::default()
        }
    }
}

/// What is known about a source file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub format: Option<ImageFormat>,
    /// Size in pixels after the orientation is applied.
    pub width: u32,
    pub height: u32,
    /// Bits per channel of the source.
    pub bits: u8,
    pub has_alpha: bool,
    /// Frames of an animation or pages of a document.
    pub frames: u32,
    /// EXIF orientation of the stored pixels (1 to 8).
    pub orientation: u16,
    pub has_icc: bool,
    pub has_exif: bool,
    pub has_xmp: bool,
}

/// A decoded file.
#[derive(Clone, Debug)]
pub struct Decoded {
    pub media: Media,
    pub info: SourceInfo,
    /// Metadata of the source. The EXIF block has its orientation reset
    /// because the pixels are already upright.
    pub metadata: Metadata,
}
