//! The pipeline model: one description of "what to do with an image" shared by
//! the editor, batch processing, watch folders, the command line, the assistant
//! and the MCP server.
//!
//! All sizes that must survive a change of resolution (crop boxes, annotation
//! coordinates, watermark sizes) are stored as fractions or percentages, so the
//! same pipeline gives the same picture on a small preview and on the full file.

use serde::{Deserialize, Serialize};

use crate::cancel::CancelToken;
use crate::color::Color;
use crate::error::{Error, Result};
use crate::format::ImageFormat;
use crate::image::{Image, Pixels};
use crate::ops;
use crate::text::Font;

/// Version of the serialized pipeline format.
pub const PIPELINE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Pipeline {
    pub version: u32,
    pub steps: Vec<Step>,
    pub output: OutputSpec,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self {
            version: PIPELINE_VERSION,
            steps: Vec::new(),
            output: OutputSpec::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    #[serde(default)]
    pub id: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    pub op: Op,
}

fn enabled_by_default() -> bool {
    true
}

impl Step {
    pub fn new(id: &str, op: Op) -> Self {
        Self {
            id: id.to_owned(),
            enabled: true,
            op,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Op {
    Resize(Resize),
    Crop(Crop),
    Rotate(Rotate),
    Flip(Flip),
    Adjust(Adjust),
    Filter(Filter),
    Watermark(Watermark),
    Annotate(Annotate),
    RemoveBackground(RemoveBackground),
    Upscale(Upscale),
    RestoreFaces(RestoreFaces),
    Inpaint(Inpaint),
    AiEdit(AiEdit),
}

impl Op {
    /// Stable name of the operation, equal to its serialized `type`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Resize(_) => "resize",
            Self::Crop(_) => "crop",
            Self::Rotate(_) => "rotate",
            Self::Flip(_) => "flip",
            Self::Adjust(_) => "adjust",
            Self::Filter(_) => "filter",
            Self::Watermark(_) => "watermark",
            Self::Annotate(_) => "annotate",
            Self::RemoveBackground(_) => "removeBackground",
            Self::Upscale(_) => "upscale",
            Self::RestoreFaces(_) => "restoreFaces",
            Self::Inpaint(_) => "inpaint",
            Self::AiEdit(_) => "aiEdit",
        }
    }

    /// Whether the operation runs a neural model.
    pub fn is_ai(&self) -> bool {
        matches!(
            self,
            Self::RemoveBackground(_)
                | Self::Upscale(_)
                | Self::RestoreFaces(_)
                | Self::Inpaint(_)
                | Self::AiEdit(_)
        )
    }

    /// Whether the operation asks the image generator of the settings, which
    /// may be a service outside this computer.
    pub fn uses_generator(&self) -> bool {
        match self {
            Self::Inpaint(spec) => !spec.prompt.trim().is_empty(),
            Self::AiEdit(spec) => !spec.prompt.trim().is_empty(),
            Self::Resize(_)
            | Self::Crop(_)
            | Self::Rotate(_)
            | Self::Flip(_)
            | Self::Adjust(_)
            | Self::Filter(_)
            | Self::Watermark(_)
            | Self::Annotate(_)
            | Self::RemoveBackground(_)
            | Self::Upscale(_)
            | Self::RestoreFaces(_) => false,
        }
    }
}

/// Nine anchor points of a rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    #[default]
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    /// Horizontal and vertical position as fractions: `0.0` is left or top.
    pub fn fractions(self) -> (f32, f32) {
        match self {
            Self::TopLeft => (0.0, 0.0),
            Self::Top => (0.5, 0.0),
            Self::TopRight => (1.0, 0.0),
            Self::Left => (0.0, 0.5),
            Self::Center => (0.5, 0.5),
            Self::Right => (1.0, 0.5),
            Self::BottomLeft => (0.0, 1.0),
            Self::Bottom => (0.5, 1.0),
            Self::BottomRight => (1.0, 1.0),
        }
    }
}

// ---------------------------------------------------------------- resize

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResizeMode {
    /// Fit inside `width` x `height`, keeping the aspect ratio.
    #[default]
    Fit,
    /// Cover `width` x `height` and crop the overflow around the centre.
    Fill,
    /// Stretch to exactly `width` x `height`.
    Exact,
    /// Scale by `percent`.
    Percent,
    /// Make the long side equal to `width`.
    LongSide,
    /// Make the short side equal to `width`.
    ShortSide,
    /// Make the width equal to `width`.
    Width,
    /// Make the height equal to `height`.
    Height,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResizeFilter {
    Nearest,
    Box,
    Bilinear,
    Hamming,
    CatmullRom,
    Mitchell,
    Gaussian,
    #[default]
    Lanczos3,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Resize {
    pub mode: ResizeMode,
    /// Target width in pixels; `0` leaves the side free in `Fit` mode.
    pub width: u32,
    /// Target height in pixels; `0` leaves the side free in `Fit` mode.
    pub height: u32,
    pub percent: f32,
    pub filter: ResizeFilter,
    /// Resample in linear light. Slower, keeps brightness of fine detail.
    pub gamma_correct: bool,
    /// Allow enlarging images smaller than the target.
    pub upscale: bool,
}

impl Default for Resize {
    fn default() -> Self {
        Self {
            mode: ResizeMode::Fit,
            width: 1920,
            height: 1080,
            percent: 50.0,
            filter: ResizeFilter::Lanczos3,
            gamma_correct: true,
            upscale: false,
        }
    }
}

// ------------------------------------------------------------------ crop

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CropMode {
    /// The box given by `x`, `y`, `width`, `height` as fractions of the image.
    #[default]
    Rect,
    /// The largest box with the `aspectWidth:aspectHeight` ratio, placed by `anchor`.
    Aspect,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Crop {
    pub mode: CropMode,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub aspect_width: u32,
    pub aspect_height: u32,
    pub anchor: Anchor,
}

impl Default for Crop {
    fn default() -> Self {
        Self {
            mode: CropMode::Rect,
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            aspect_width: 1,
            aspect_height: 1,
            anchor: Anchor::Center,
        }
    }
}

// ---------------------------------------------------------- rotate, flip

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Rotate {
    /// Clockwise angle in degrees.
    pub angle: f32,
    /// Grow the canvas so nothing is cut off.
    pub expand: bool,
    /// Colour of the corners uncovered by the rotation.
    pub background: Color,
}

impl Default for Rotate {
    fn default() -> Self {
        Self {
            angle: 90.0,
            expand: true,
            background: Color::TRANSPARENT,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Flip {
    pub horizontal: bool,
    pub vertical: bool,
}

// ---------------------------------------------------------------- adjust

/// Tone and colour correction. Every slider runs from `-100` to `100` with `0`
/// as "no change"; `exposure` is measured in stops from `-5` to `5`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Adjust {
    pub exposure: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub temperature: f32,
    pub tint: f32,
    /// Tone curve control points `[input, output]` in the `0.0..=1.0` range.
    pub curve: Vec<[f32; 2]>,
}

impl Adjust {
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }
}

// ---------------------------------------------------------------- filter

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FilterKind {
    #[default]
    Sharpen,
    Blur,
    Denoise,
    Grain,
    Vignette,
    Grayscale,
    Sepia,
    Invert,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Filter {
    pub kind: FilterKind,
    /// Strength from `0` to `100`; `Vignette` also accepts negative values.
    pub amount: f32,
    /// Radius in pixels of the full-size image, where the filter has one.
    pub radius: f32,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            kind: FilterKind::Sharpen,
            amount: 50.0,
            radius: 1.0,
        }
    }
}

// ------------------------------------------------------------- watermark

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WatermarkKind {
    #[default]
    Text,
    Image,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Watermark {
    pub kind: WatermarkKind,
    pub text: String,
    /// Font family name or path to a font file; empty selects the default font.
    pub font: String,
    /// Text height as a percentage of the short side of the image.
    pub size: f32,
    pub color: Color,
    /// Path to the watermark picture.
    pub image: String,
    /// Width of the watermark picture as a percentage of the image width.
    pub scale: f32,
    pub anchor: Anchor,
    /// Distance from the edges as a percentage of the short side.
    pub margin: f32,
    /// Opacity from `0` to `100`.
    pub opacity: f32,
    /// Clockwise rotation in degrees.
    pub rotation: f32,
    /// Repeat the watermark over the whole image.
    pub tile: bool,
    /// Gap between repeats as a percentage of the watermark size.
    pub tile_gap: f32,
}

impl Default for Watermark {
    fn default() -> Self {
        Self {
            kind: WatermarkKind::Text,
            text: String::from("PixelIt"),
            font: String::new(),
            size: 5.0,
            color: Color::WHITE,
            image: String::new(),
            scale: 20.0,
            anchor: Anchor::BottomRight,
            margin: 3.0,
            opacity: 60.0,
            rotation: 0.0,
            tile: false,
            tile_gap: 60.0,
        }
    }
}

// -------------------------------------------------------------- annotate

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShapeKind {
    #[default]
    Arrow,
    Line,
    Rect,
    Ellipse,
    Pen,
    Text,
    Highlight,
    Blur,
    Pixelate,
}

/// One annotation. `x1, y1` and `x2, y2` are opposite corners (or the ends of a
/// line) as fractions of the image size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Shape {
    pub kind: ShapeKind,
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub color: Color,
    /// Stroke thickness as a percentage of the short side of the image.
    pub width: f32,
    /// Fill the inside of a rectangle or an ellipse.
    pub fill: bool,
    pub text: String,
    pub font: String,
    /// Text height as a percentage of the short side of the image.
    pub size: f32,
    /// Points of a freehand line as fractions of the image size.
    pub points: Vec<[f32; 2]>,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            kind: ShapeKind::Arrow,
            x1: 0.25,
            y1: 0.25,
            x2: 0.75,
            y2: 0.75,
            color: Color::rgb(229, 57, 53),
            width: 0.6,
            fill: false,
            text: String::new(),
            font: String::new(),
            size: 4.0,
            points: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Annotate {
    pub shapes: Vec<Shape>,
}

// -------------------------------------------------------------- AI steps

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoveBackground {
    /// Model id from the component catalogue; empty selects the installed default.
    pub model: String,
    /// Solid colour placed behind the subject; `None` keeps transparency.
    pub background: Option<Color>,
    /// Areas painted by hand that stay as they are, whatever the model decides.
    pub keep: Vec<Stroke>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Upscale {
    pub model: String,
    /// Enlargement factor: 2 or 4.
    pub factor: u32,
}

impl Default for Upscale {
    fn default() -> Self {
        Self {
            model: String::new(),
            factor: 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RestoreFaces {
    pub model: String,
    /// Blend of the restored face with the original, from `0` to `100`.
    pub strength: f32,
}

impl Default for RestoreFaces {
    fn default() -> Self {
        Self {
            model: String::new(),
            strength: 80.0,
        }
    }
}

/// A brush stroke of a mask. Points are fractions of the image size, `width`
/// is a percentage of the short side.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Stroke {
    pub points: Vec<[f32; 2]>,
    pub width: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Inpaint {
    pub model: String,
    /// Area to repaint.
    pub strokes: Vec<Stroke>,
    /// Text prompt for generative providers; empty removes the object.
    pub prompt: String,
}

/// A change described in words and drawn by the image generator of the
/// settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiEdit {
    /// What to change; an empty prompt leaves the picture as it is.
    pub prompt: String,
    /// Area to change; without strokes the whole picture is changed.
    pub strokes: Vec<Stroke>,
    /// How far the result may move from the picture, from `0` to `100`.
    pub strength: f32,
    /// Number of the attempt: another number asks for another result.
    pub variant: u32,
}

impl Default for AiEdit {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            strokes: Vec::new(),
            strength: 75.0,
            variant: 0,
        }
    }
}

// ---------------------------------------------------------------- output

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputFormat {
    /// Keep the format of the source file when it can be written.
    #[default]
    Same,
    /// Pick the smallest of `autoFormats` at the requested quality.
    Auto,
    Jpeg,
    Png,
    Webp,
    Avif,
    Jxl,
    Tiff,
    Bmp,
    Gif,
    Ico,
}

impl OutputFormat {
    /// The concrete format, when the choice does not depend on the source.
    pub fn fixed(self) -> Option<ImageFormat> {
        match self {
            Self::Same | Self::Auto => None,
            Self::Jpeg => Some(ImageFormat::Jpeg),
            Self::Png => Some(ImageFormat::Png),
            Self::Webp => Some(ImageFormat::Webp),
            Self::Avif => Some(ImageFormat::Avif),
            Self::Jxl => Some(ImageFormat::Jxl),
            Self::Tiff => Some(ImageFormat::Tiff),
            Self::Bmp => Some(ImageFormat::Bmp),
            Self::Gif => Some(ImageFormat::Gif),
            Self::Ico => Some(ImageFormat::Ico),
        }
    }
}

/// Trade-off between encoding time and file size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Effort {
    Fast,
    #[default]
    Balanced,
    Max,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputDepth {
    /// 16 bits when both the image and the format have them, 8 bits otherwise.
    #[default]
    Auto,
    Eight,
    Sixteen,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Subsampling {
    /// 4:2:0 below quality 90, 4:4:4 from 90 up.
    #[default]
    Auto,
    Full,
    Half,
}

/// What the encoder aims for besides `quality`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Target {
    #[default]
    None,
    /// The largest file that fits into `bytes`.
    #[serde(rename_all = "camelCase")]
    Bytes { bytes: u64 },
    /// The smallest file whose SSIMULACRA2 score stays at or above `score`.
    #[serde(rename_all = "camelCase")]
    Quality { score: f32 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MetadataMode {
    /// Keep everything the output format can carry.
    #[default]
    Keep,
    /// Remove all metadata.
    Strip,
    /// Keep only the groups switched on below.
    Custom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MetadataPolicy {
    pub mode: MetadataMode,
    pub keep_exif: bool,
    pub keep_gps: bool,
    pub keep_xmp: bool,
    pub keep_icc: bool,
    /// Written into the copyright field when not empty.
    pub copyright: String,
    /// Written into the author field when not empty.
    pub author: String,
}

impl Default for MetadataPolicy {
    fn default() -> Self {
        Self {
            mode: MetadataMode::Keep,
            keep_exif: true,
            keep_gps: false,
            keep_xmp: false,
            keep_icc: true,
            copyright: String::new(),
            author: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColorPolicy {
    /// Write sRGB pixels.
    #[default]
    Srgb,
    /// Convert back to the colour profile of the source file and embed it.
    Original,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Destination {
    /// Next to the source file.
    #[default]
    SameFolder,
    /// In a subfolder of the source folder.
    #[serde(rename_all = "camelCase")]
    Subfolder { name: String },
    /// In one fixed folder.
    #[serde(rename_all = "camelCase")]
    Folder { path: String },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Conflict {
    /// Add a number to the new file name.
    #[default]
    Rename,
    Overwrite,
    Skip,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OutputSpec {
    pub format: OutputFormat,
    /// Candidates of the `Auto` format.
    pub auto_formats: Vec<ImageFormat>,
    /// Quality from `1` to `100` for lossy formats.
    pub quality: u8,
    pub lossless: bool,
    pub effort: Effort,
    pub depth: OutputDepth,
    pub subsampling: Subsampling,
    /// Write progressive JPEG.
    pub progressive: bool,
    pub target: Target,
    pub metadata: MetadataPolicy,
    pub color: ColorPolicy,
    /// Colour placed under transparent pixels in formats without alpha.
    pub background: Color,
    /// File name template, see [`crate::naming`].
    pub naming: String,
    pub destination: Destination,
    pub conflict: Conflict,
    /// Move the source file to the Recycle Bin after a successful write.
    pub replace_original: bool,
    /// Repeat the folder tree of the source under the destination folder.
    pub keep_structure: bool,
}

impl Default for OutputSpec {
    fn default() -> Self {
        Self {
            format: OutputFormat::Same,
            auto_formats: vec![ImageFormat::Jpeg, ImageFormat::Webp, ImageFormat::Avif],
            quality: 82,
            lossless: false,
            effort: Effort::Balanced,
            depth: OutputDepth::Auto,
            subsampling: Subsampling::Auto,
            progressive: true,
            target: Target::None,
            metadata: MetadataPolicy::default(),
            color: ColorPolicy::Srgb,
            background: Color::WHITE,
            naming: String::from("{name}"),
            destination: Destination::Subfolder {
                name: String::from("PixelIt"),
            },
            conflict: Conflict::Rename,
            replace_original: false,
            keep_structure: true,
        }
    }
}

// --------------------------------------------------------------- running

/// A single-channel 8-bit mask: `255` marks the selected area.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Fonts and pictures a pipeline refers to by name. The engine implements this
/// with system fonts and files; tests use an in-memory implementation.
pub trait Resources: Sync {
    /// The font with the given family name or path; an empty name is the
    /// default font.
    fn font(&self, name: &str) -> Result<Font>;

    /// The picture at the given path.
    fn image(&self, path: &str) -> Result<Image>;
}

/// Resources of a pipeline that draws no text and no pictures.
pub struct NoResources;

impl Resources for NoResources {
    fn font(&self, name: &str) -> Result<Font> {
        Err(Error::new("fontNotFound").with("name", name))
    }

    fn image(&self, path: &str) -> Result<Image> {
        Err(Error::new("notFound").with("path", path))
    }
}

/// Neural models behind the AI steps. Implemented by `pixelit-ai`.
pub trait AiBackend: Sync {
    /// Returns the image with the background made transparent.
    fn remove_background(&self, image: &Image, model: &str, cancel: &CancelToken) -> Result<Image>;

    fn upscale(
        &self,
        image: &Image,
        model: &str,
        factor: u32,
        cancel: &CancelToken,
    ) -> Result<Image>;

    /// `strength` runs from `0.0` to `1.0`.
    fn restore_faces(
        &self,
        image: &Image,
        model: &str,
        strength: f32,
        cancel: &CancelToken,
    ) -> Result<Image>;

    fn inpaint(
        &self,
        image: &Image,
        mask: &Mask,
        model: &str,
        prompt: &str,
        cancel: &CancelToken,
    ) -> Result<Image>;

    /// Changes the picture following the prompt of `spec`: inside `mask`, or
    /// everywhere without one. `preview` tells that the picture is the
    /// reduced copy of the editor, whose result the real run should repeat.
    fn edit(
        &self,
        image: &Image,
        mask: Option<&Mask>,
        spec: &AiEdit,
        preview: bool,
        cancel: &CancelToken,
    ) -> Result<Image>;
}

/// Everything a step needs besides the image.
#[derive(Clone, Copy)]
pub struct ApplyContext<'a> {
    pub resources: &'a dyn Resources,
    pub ai: Option<&'a dyn AiBackend>,
    pub cancel: &'a CancelToken,
    /// Size of the processed image relative to the full file: `1.0` for the
    /// real run, below `1.0` for a preview on a reduced copy. Pixel-based
    /// parameters are multiplied by it.
    pub scale: f32,
    /// Whether this is the preview of the editor rather than the real run.
    pub preview: bool,
}

impl<'a> ApplyContext<'a> {
    pub fn new(resources: &'a dyn Resources, cancel: &'a CancelToken) -> Self {
        Self {
            resources,
            ai: None,
            cancel,
            scale: 1.0,
            preview: false,
        }
    }
}

fn copy_masked<T: Copy>(target: &mut [T], source: &[T], mask: &[u8]) {
    for ((new, old), cover) in target
        .chunks_exact_mut(4)
        .zip(source.chunks_exact(4))
        .zip(mask)
    {
        if *cover > 127 {
            new.copy_from_slice(old);
        }
    }
}

/// Puts the pixels of `original` back inside the mask.
fn keep_original(result: &mut Image, original: &Image, mask: &Mask) {
    if result.width() != original.width() || result.height() != original.height() {
        return;
    }
    match (result.pixels_mut(), original.pixels()) {
        (Pixels::U8(new), Pixels::U8(old)) => copy_masked(new, old, &mask.data),
        (Pixels::U16(new), Pixels::U16(old)) => copy_masked(new, old, &mask.data),
        (Pixels::U8(_), Pixels::U16(_)) | (Pixels::U16(_), Pixels::U8(_)) => {}
    }
}

/// Applies one operation.
pub fn apply_op(image: Image, op: &Op, context: &ApplyContext<'_>) -> Result<Image> {
    context.cancel.check()?;
    match op {
        Op::Resize(spec) => ops::resize::resize(&image, spec, context.scale),
        Op::Crop(spec) => ops::geometry::crop(&image, spec),
        Op::Rotate(spec) => ops::geometry::rotate(image, spec),
        Op::Flip(spec) => Ok(ops::geometry::flip(image, spec)),
        Op::Adjust(spec) => {
            let mut image = image;
            ops::adjust::adjust(&mut image, spec);
            Ok(image)
        }
        Op::Filter(spec) => {
            let mut image = image;
            ops::filter::filter(&mut image, spec, context.scale);
            Ok(image)
        }
        Op::Watermark(spec) => {
            let mut image = image;
            ops::watermark::watermark(&mut image, spec, context.resources)?;
            Ok(image)
        }
        Op::Annotate(spec) => {
            let mut image = image;
            ops::annotate::annotate(&mut image, spec, context.resources)?;
            Ok(image)
        }
        Op::RemoveBackground(spec) => {
            let ai = context.ai.ok_or_else(|| Error::new("aiUnavailable"))?;
            let mut result = ai.remove_background(&image, &spec.model, context.cancel)?;
            if !spec.keep.is_empty() {
                let mask = ops::annotate::stroke_mask(image.width(), image.height(), &spec.keep);
                keep_original(&mut result, &image, &mask);
            }
            if let Some(background) = spec.background {
                result.flatten(background);
            }
            Ok(result)
        }
        Op::Upscale(spec) => {
            let ai = context.ai.ok_or_else(|| Error::new("aiUnavailable"))?;
            ai.upscale(&image, &spec.model, spec.factor.clamp(2, 4), context.cancel)
        }
        Op::RestoreFaces(spec) => {
            let ai = context.ai.ok_or_else(|| Error::new("aiUnavailable"))?;
            let strength = (spec.strength / 100.0).clamp(0.0, 1.0);
            ai.restore_faces(&image, &spec.model, strength, context.cancel)
        }
        Op::Inpaint(spec) => {
            let ai = context.ai.ok_or_else(|| Error::new("aiUnavailable"))?;
            let mask = ops::annotate::stroke_mask(image.width(), image.height(), &spec.strokes);
            ai.inpaint(&image, &mask, &spec.model, &spec.prompt, context.cancel)
        }
        Op::AiEdit(spec) => {
            if spec.prompt.trim().is_empty() {
                return Ok(image);
            }
            let ai = context.ai.ok_or_else(|| Error::new("aiUnavailable"))?;
            let mask = (!spec.strokes.is_empty())
                .then(|| ops::annotate::stroke_mask(image.width(), image.height(), &spec.strokes));
            ai.edit(&image, mask.as_ref(), spec, context.preview, context.cancel)
        }
    }
}

/// Applies the enabled steps in order.
pub fn apply_steps(mut image: Image, steps: &[Step], context: &ApplyContext<'_>) -> Result<Image> {
    for step in steps.iter().filter(|step| step.enabled) {
        image = apply_op(image, &step.op, context).map_err(|error| {
            if error.is_cancelled() {
                error
            } else {
                error.with("step", step.op.kind())
            }
        })?;
    }
    Ok(image)
}

impl Pipeline {
    pub fn apply(&self, image: Image, context: &ApplyContext<'_>) -> Result<Image> {
        apply_steps(image, &self.steps, context)
    }

    /// Whether an enabled step runs a neural model.
    pub fn uses_ai(&self) -> bool {
        self.steps
            .iter()
            .any(|step| step.enabled && step.op.is_ai())
    }

    /// Reads a pipeline, refusing files written by a newer version.
    pub fn from_json(text: &str) -> Result<Self> {
        let pipeline: Self = serde_json::from_str(text)?;
        if pipeline.version > PIPELINE_VERSION {
            return Err(Error::new("newerFile").with("version", pipeline.version));
        }
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pipeline_round_trips() {
        let pipeline = Pipeline::default();
        let json = serde_json::to_string(&pipeline).unwrap();
        assert_eq!(Pipeline::from_json(&json).unwrap(), pipeline);
    }

    #[test]
    fn steps_use_tagged_camel_case() {
        let step = Step::new("a", Op::RemoveBackground(RemoveBackground::default()));
        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(json["op"]["type"], "removeBackground");
        assert_eq!(json["enabled"], true);
        assert_eq!(step.op.kind(), "removeBackground");
    }

    #[test]
    fn every_op_kind_matches_its_serialized_tag() {
        let ops = [
            Op::Resize(Resize::default()),
            Op::Crop(Crop::default()),
            Op::Rotate(Rotate::default()),
            Op::Flip(Flip::default()),
            Op::Adjust(Adjust::default()),
            Op::Filter(Filter::default()),
            Op::Watermark(Watermark::default()),
            Op::Annotate(Annotate::default()),
            Op::RemoveBackground(RemoveBackground::default()),
            Op::Upscale(Upscale::default()),
            Op::RestoreFaces(RestoreFaces::default()),
            Op::Inpaint(Inpaint::default()),
            Op::AiEdit(AiEdit::default()),
        ];
        for op in ops {
            let json = serde_json::to_value(&op).unwrap();
            assert_eq!(json["type"], op.kind());
            let back: Op = serde_json::from_value(json).unwrap();
            assert_eq!(back, op);
        }
    }

    #[test]
    fn partial_json_takes_defaults() {
        let pipeline = Pipeline::from_json(
            r#"{"steps":[{"op":{"type":"resize","width":800}}],"output":{"format":"webp"}}"#,
        )
        .unwrap();
        assert_eq!(pipeline.version, PIPELINE_VERSION);
        assert!(pipeline.steps[0].enabled);
        match &pipeline.steps[0].op {
            Op::Resize(resize) => {
                assert_eq!(resize.width, 800);
                assert_eq!(resize.height, 1080);
            }
            other => panic!("unexpected op {other:?}"),
        }
        assert_eq!(pipeline.output.format, OutputFormat::Webp);
        assert_eq!(pipeline.output.quality, 82);
    }

    #[test]
    fn refuses_newer_versions() {
        let error = Pipeline::from_json(r#"{"version":999}"#).unwrap_err();
        assert_eq!(error.code, "newerFile");
    }

    #[test]
    fn tagged_enums_round_trip() {
        for target in [
            Target::None,
            Target::Bytes { bytes: 200_000 },
            Target::Quality { score: 80.0 },
        ] {
            let json = serde_json::to_string(&target).unwrap();
            assert_eq!(serde_json::from_str::<Target>(&json).unwrap(), target);
        }
        let destination = Destination::Folder {
            path: String::from("C:\\out"),
        };
        let json = serde_json::to_value(&destination).unwrap();
        assert_eq!(json["kind"], "folder");
        assert_eq!(
            serde_json::from_value::<Destination>(json).unwrap(),
            destination
        );
    }

    #[test]
    fn disabled_steps_and_missing_ai() {
        let cancel = CancelToken::new();
        let context = ApplyContext::new(&NoResources, &cancel);
        let image = Image::new(4, 4, crate::image::Depth::Eight, Color::WHITE).unwrap();

        let mut step = Step::new("ai", Op::Upscale(Upscale::default()));
        let error = apply_steps(image.clone(), std::slice::from_ref(&step), &context).unwrap_err();
        assert_eq!(error.code, "aiUnavailable");
        assert_eq!(error.params["step"], "upscale");

        step.enabled = false;
        let result = apply_steps(image.clone(), &[step], &context).unwrap();
        assert_eq!(result, image);
    }

    #[test]
    fn kept_areas_take_the_original_pixels() {
        let original = Image::new(40, 20, crate::image::Depth::Eight, Color::WHITE).unwrap();
        let mut result = Image::from_rgba8(40, 20, vec![0; 40 * 20 * 4]).unwrap();
        let mask = ops::annotate::stroke_mask(
            40,
            20,
            &[Stroke {
                points: vec![[0.2, 0.5], [0.4, 0.5]],
                width: 20.0,
            }],
        );
        keep_original(&mut result, &original, &mask);
        assert_eq!(result.pixel8(12, 10), [255, 255, 255, 255]);
        assert_eq!(result.pixel8(35, 10), [0, 0, 0, 0]);

        let other = Image::new(8, 8, crate::image::Depth::Eight, Color::WHITE).unwrap();
        let before = result.clone();
        keep_original(&mut result, &other, &mask);
        assert_eq!(result, before);
    }

    #[test]
    fn an_edit_without_a_prompt_changes_nothing() {
        let cancel = CancelToken::new();
        let context = ApplyContext::new(&NoResources, &cancel);
        let image = Image::new(4, 4, crate::image::Depth::Eight, Color::WHITE).unwrap();
        let mut edit = AiEdit::default();
        let untouched = apply_op(image.clone(), &Op::AiEdit(edit.clone()), &context).unwrap();
        assert_eq!(untouched, image);
        assert!(!Op::AiEdit(edit.clone()).uses_generator());

        edit.prompt = String::from("make it night");
        let op = Op::AiEdit(edit);
        assert!(op.uses_generator());
        assert!(op.is_ai());
        assert_eq!(
            apply_op(image, &op, &context).unwrap_err().code,
            "aiUnavailable"
        );
        assert!(!Op::Inpaint(Inpaint::default()).uses_generator());
    }

    #[test]
    fn cancelled_run_stops() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let context = ApplyContext::new(&NoResources, &cancel);
        let image = Image::new(4, 4, crate::image::Depth::Eight, Color::WHITE).unwrap();
        let steps = [Step::new("flip", Op::Flip(Flip::default()))];
        assert!(apply_steps(image, &steps, &context)
            .unwrap_err()
            .is_cancelled());
    }
}
