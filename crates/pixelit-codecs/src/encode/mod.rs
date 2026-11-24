//! Writing image files of every output format.

mod jpeg;
mod modern;
mod png;
mod simple;
mod webp;

use std::borrow::Cow;
use std::fmt::Debug;
use std::panic::{catch_unwind, AssertUnwindSafe};

use pixelit_core::image::Animation;
use pixelit_core::pipeline::{Effort, OutputDepth, OutputSpec, Subsampling};
use pixelit_core::{CancelToken, Color, Depth, Error, Image, ImageFormat, Media, Result};

use crate::color;
use crate::metadata::{self, Metadata};

/// Lowest and highest accepted quality.
const QUALITY_RANGE: (u8, u8) = (1, 100);

/// Settings of one encoded file.
#[derive(Clone, Debug, PartialEq)]
pub struct EncodeOptions {
    pub format: ImageFormat,
    /// Quality from `1` to `100` for lossy formats.
    pub quality: u8,
    pub lossless: bool,
    pub effort: Effort,
    pub depth: OutputDepth,
    pub subsampling: Subsampling,
    /// Write progressive JPEG.
    pub progressive: bool,
    /// Colour placed under transparent pixels in formats without alpha.
    pub background: Color,
    /// Blocks written into the file. An ICC profile here makes the encoder
    /// convert the sRGB pixels into that profile.
    pub metadata: Metadata,
}

impl EncodeOptions {
    pub fn new(format: ImageFormat) -> Self {
        let defaults = OutputSpec::default();
        Self {
            format,
            quality: defaults.quality,
            lossless: false,
            effort: defaults.effort,
            depth: defaults.depth,
            subsampling: defaults.subsampling,
            progressive: defaults.progressive,
            background: defaults.background,
            metadata: Metadata::default(),
        }
    }

    /// Settings taken from an output description. `source` is the metadata of
    /// the input file; the metadata policy decides what stays.
    pub fn from_spec(spec: &OutputSpec, format: ImageFormat, source: &Metadata) -> Self {
        Self {
            format,
            quality: spec.quality,
            lossless: spec.lossless && format.supports_lossless(),
            effort: spec.effort,
            depth: spec.depth,
            subsampling: spec.subsampling,
            progressive: spec.progressive,
            background: spec.background,
            metadata: metadata::prepare(source, &spec.metadata, spec.color, format),
        }
    }

    #[must_use]
    pub fn with_quality(mut self, quality: u8) -> Self {
        self.quality = quality;
        self
    }

    pub(crate) fn clamped_quality(&self) -> u8 {
        self.quality.clamp(QUALITY_RANGE.0, QUALITY_RANGE.1)
    }
}

/// An error for an encoder failure.
pub(crate) fn failed(error: impl Debug) -> Error {
    Error::new("encodeFailed").detail(format!("{error:?}"))
}

fn target_depth(image: &Image, options: &EncodeOptions) -> Depth {
    if !options.format.supports_16_bit() {
        return Depth::Eight;
    }
    match options.depth {
        OutputDepth::Auto => image.depth(),
        OutputDepth::Eight => Depth::Eight,
        OutputDepth::Sixteen => Depth::Sixteen,
    }
}

/// The image in the layout the format accepts: flattened when the format has
/// no alpha, at the output bit depth and in the output colour profile.
fn prepare<'a>(image: &'a Image, options: &mut EncodeOptions) -> Cow<'a, Image> {
    let mut prepared = Cow::Borrowed(image);
    if let Some(icc) = options.metadata.icc.clone() {
        if !color::from_srgb(prepared.to_mut(), &icc) {
            options.metadata.icc = None;
        }
    }
    if image.has_alpha() && !options.format.supports_alpha() {
        prepared.to_mut().flatten(options.background);
    }
    let depth = target_depth(image, options);
    if prepared.depth() != depth {
        prepared = Cow::Owned(prepared.into_owned().into_depth(depth));
    }
    prepared
}

fn dispatch(image: &Image, options: &EncodeOptions) -> Result<Vec<u8>> {
    match options.format {
        ImageFormat::Jpeg => jpeg::encode(image, options),
        ImageFormat::Png => png::encode(image, options),
        ImageFormat::Webp => webp::encode(image, options),
        ImageFormat::Avif => modern::encode_avif(image, options),
        ImageFormat::Jxl => modern::encode_jxl(image, options),
        ImageFormat::Tiff => simple::encode_tiff(image, options),
        ImageFormat::Bmp => simple::encode_bmp(image),
        ImageFormat::Gif => simple::encode_gif(image, options),
        ImageFormat::Ico => simple::encode_ico(image),
        ImageFormat::Heic
        | ImageFormat::Raw
        | ImageFormat::Svg
        | ImageFormat::Psd
        | ImageFormat::Pdf => Err(Error::new("cannotEncode").with("format", options.format.name())),
    }
}

/// Runs an encoder and turns a panic of a third-party library into an error.
fn guarded(work: impl FnOnce() -> Result<Vec<u8>>) -> Result<Vec<u8>> {
    catch_unwind(AssertUnwindSafe(work))
        .unwrap_or_else(|_| Err(Error::new("encodeFailed").detail("encoder panicked")))
}

/// Encodes a still image. The pixels are sRGB.
pub fn encode(image: &Image, options: &EncodeOptions, cancel: &CancelToken) -> Result<Vec<u8>> {
    cancel.check()?;
    let mut options = options.clone();
    let prepared = prepare(image, &mut options);
    cancel.check()?;
    let bytes = guarded(|| dispatch(&prepared, &options))?;
    cancel.check()?;
    Ok(metadata::embed(bytes, options.format, &options.metadata))
}

/// Encodes an animation as GIF, APNG or animated WebP.
pub fn encode_animation(
    animation: &Animation,
    options: &EncodeOptions,
    cancel: &CancelToken,
) -> Result<Vec<u8>> {
    cancel.check()?;
    let Some(first) = animation.frames.first() else {
        return Err(Error::new("emptyImage"));
    };
    let (width, height) = (first.image.width(), first.image.height());
    if animation
        .frames
        .iter()
        .any(|frame| frame.image.width() != width || frame.image.height() != height)
    {
        return Err(Error::new("badImage").detail("frames differ in size"));
    }
    let bytes = guarded(|| match options.format {
        ImageFormat::Gif => simple::encode_gif_animation(animation, options, cancel),
        ImageFormat::Png => png::encode_animation(animation, options, cancel),
        ImageFormat::Webp => webp::encode_animation(animation, options, cancel),
        _ => Err(Error::new("cannotEncode").with("format", options.format.name())),
    })?;
    cancel.check()?;
    Ok(bytes)
}

/// Encodes a still image or an animation. An animation written into a format
/// without animation support keeps its first frame.
pub fn encode_media(
    media: &Media,
    options: &EncodeOptions,
    cancel: &CancelToken,
) -> Result<Vec<u8>> {
    match media {
        Media::Animated(animation)
            if animation.frames.len() > 1 && options.format.supports_animation() =>
        {
            encode_animation(animation, options, cancel)
        }
        _ => {
            let image = media.first().ok_or_else(|| Error::new("emptyImage"))?;
            encode(image, options, cancel)
        }
    }
}

#[cfg(test)]
mod tests {
    use pixelit_core::image::Frame;
    use pixelit_core::pipeline::{ColorPolicy, MetadataPolicy};

    use super::*;
    use crate::decode::decode_bytes;
    use crate::info::DecodeOptions;

    /// A smooth gradient with a transparent corner when `alpha` is set.
    fn sample(width: u32, height: u32, alpha: bool, depth: Depth) -> Image {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let red = (x * 255 / width.max(1)) as u8;
                let green = (y * 255 / height.max(1)) as u8;
                let blue = ((x + y) * 255 / (width + height).max(1)) as u8;
                let opacity = if alpha && x < width / 4 && y < height / 4 {
                    0
                } else {
                    255
                };
                data.extend_from_slice(&[red, green, blue, opacity]);
            }
        }
        let mut image = Image::from_rgba8(width, height, data).unwrap();
        image.set_has_alpha(alpha);
        image.into_depth(depth)
    }

    fn round_trip(image: &Image, options: &EncodeOptions) -> Image {
        let cancel = CancelToken::new();
        let bytes = encode(image, options, &cancel).unwrap();
        assert_eq!(
            ImageFormat::sniff(&bytes[..bytes.len().min(64)]),
            Some(options.format),
            "signature of {:?}",
            options.format
        );
        let decoded = decode_bytes(&bytes, None, &DecodeOptions::default(), &cancel).unwrap();
        assert_eq!(decoded.info.format, Some(options.format));
        decoded.media.into_first().unwrap()
    }

    /// Largest difference of a colour channel between two images of equal size.
    fn max_difference(left: &Image, right: &Image) -> u8 {
        assert_eq!(
            (left.width(), left.height()),
            (right.width(), right.height())
        );
        left.to_rgba8()
            .chunks_exact(4)
            .zip(right.to_rgba8().chunks_exact(4))
            .filter(|(a, b)| a[3] != 0 || b[3] != 0)
            .flat_map(|(a, b)| (0..3).map(move |index| a[index].abs_diff(b[index])))
            .max()
            .unwrap_or(0)
    }

    fn mean_difference(left: &Image, right: &Image) -> f64 {
        let (a, b) = (left.to_rgb8(), right.to_rgb8());
        let total: u64 = a
            .iter()
            .zip(&b)
            .map(|(x, y)| u64::from(x.abs_diff(*y)))
            .sum();
        total as f64 / a.len() as f64
    }

    #[test]
    fn lossless_formats_keep_pixels() {
        let image = sample(48, 32, true, Depth::Eight);
        for format in [
            ImageFormat::Png,
            ImageFormat::Webp,
            ImageFormat::Tiff,
            ImageFormat::Jxl,
        ] {
            let mut options = EncodeOptions::new(format);
            options.lossless = true;
            let decoded = round_trip(&image, &options);
            assert!(decoded.has_alpha(), "{format:?} keeps alpha");
            assert_eq!(max_difference(&image, &decoded), 0, "{format:?}");
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn lossy_formats_stay_close() {
        let image = sample(64, 64, false, Depth::Eight);
        for format in [
            ImageFormat::Jpeg,
            ImageFormat::Webp,
            ImageFormat::Avif,
            ImageFormat::Jxl,
        ] {
            let options = EncodeOptions::new(format).with_quality(90);
            let decoded = round_trip(&image, &options);
            assert!(!decoded.has_alpha(), "{format:?}");
            let difference = mean_difference(&image, &decoded);
            assert!(difference < 4.0, "{format:?} mean difference {difference}");
        }
    }

    #[test]
    fn quality_changes_file_size() {
        let image = sample(96, 96, false, Depth::Eight);
        let cancel = CancelToken::new();
        for format in [ImageFormat::Jpeg, ImageFormat::Webp] {
            let small = encode(
                &image,
                &EncodeOptions::new(format).with_quality(30),
                &cancel,
            )
            .unwrap();
            let large = encode(
                &image,
                &EncodeOptions::new(format).with_quality(95),
                &cancel,
            )
            .unwrap();
            assert!(small.len() < large.len(), "{format:?}");
        }
    }

    #[test]
    fn formats_without_alpha_flatten_onto_background() {
        let image = sample(32, 32, true, Depth::Eight);
        let mut options = EncodeOptions::new(ImageFormat::Bmp);
        options.background = Color::rgb(255, 0, 0);
        let decoded = round_trip(&image, &options);
        assert!(!decoded.has_alpha());
        assert_eq!(decoded.pixel8(0, 0), [255, 0, 0, 255]);
        assert_eq!(decoded.pixel8(31, 31), image.pixel8(31, 31));
    }

    #[test]
    fn sixteen_bits_survive_png_tiff_and_jxl() {
        let image = sample(24, 24, false, Depth::Sixteen);
        for format in [ImageFormat::Png, ImageFormat::Tiff, ImageFormat::Jxl] {
            let mut options = EncodeOptions::new(format);
            options.lossless = true;
            let decoded = round_trip(&image, &options);
            assert_eq!(decoded.depth(), Depth::Sixteen, "{format:?}");
            assert_eq!(decoded.to_rgba16(), image.to_rgba16(), "{format:?}");
        }
    }

    #[test]
    fn eight_bit_formats_narrow_deep_images() {
        let image = sample(24, 24, false, Depth::Sixteen);
        let decoded = round_trip(&image, &EncodeOptions::new(ImageFormat::Bmp));
        assert_eq!(decoded.depth(), Depth::Eight);
        assert_eq!(max_difference(&image, &decoded), 0);
    }

    #[test]
    fn gif_and_ico_round_trip() {
        let image = sample(40, 40, false, Depth::Eight);
        let gif = round_trip(&image, &EncodeOptions::new(ImageFormat::Gif));
        assert_eq!((gif.width(), gif.height()), (40, 40));
        // GIF keeps at most 256 colours.
        assert!(mean_difference(&image, &gif) < 12.0);

        let large = sample(400, 200, false, Depth::Eight);
        let icon = round_trip(&large, &EncodeOptions::new(ImageFormat::Ico));
        assert_eq!((icon.width(), icon.height()), (256, 128));
    }

    #[test]
    fn metadata_is_embedded_and_read_back() {
        let image = sample(32, 32, false, Depth::Eight);
        let cancel = CancelToken::new();
        let mut exif = b"II*\0".to_vec();
        exif.extend_from_slice(&8_u32.to_le_bytes());
        exif.extend_from_slice(&1_u16.to_le_bytes());
        exif.extend_from_slice(&0x0112_u16.to_le_bytes());
        exif.extend_from_slice(&3_u16.to_le_bytes());
        exif.extend_from_slice(&1_u32.to_le_bytes());
        exif.extend_from_slice(&1_u32.to_le_bytes());
        exif.extend_from_slice(&0_u32.to_le_bytes());
        let source = Metadata {
            exif: Some(exif.clone()),
            icc: None,
            xmp: Some(b"<x:xmpmeta/>".to_vec()),
        };
        for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Webp] {
            let spec = OutputSpec::default();
            let options = EncodeOptions::from_spec(&spec, format, &source);
            let bytes = encode(&image, &options, &cancel).unwrap();
            let read = metadata::extract(&bytes, format);
            assert_eq!(read.exif.as_ref(), Some(&exif), "{format:?} EXIF");
            if metadata::supports_xmp(format) {
                assert_eq!(read.xmp, source.xmp, "{format:?} XMP");
            }
            let decoded = decode_bytes(&bytes, None, &DecodeOptions::default(), &cancel).unwrap();
            assert!(decoded.info.has_exif, "{format:?}");
        }
    }

    #[test]
    fn original_colour_policy_embeds_profile() {
        let image = sample(32, 32, false, Depth::Eight);
        let cancel = CancelToken::new();
        let p3 = moxcms::ColorProfile::new_display_p3().encode().unwrap();
        let source = Metadata {
            exif: None,
            icc: Some(p3),
            xmp: None,
        };
        let spec = OutputSpec {
            color: ColorPolicy::Original,
            lossless: true,
            metadata: MetadataPolicy::default(),
            ..OutputSpec::default()
        };
        let options = EncodeOptions::from_spec(&spec, ImageFormat::Png, &source);
        assert!(options.metadata.icc.is_some());
        let bytes = encode(&image, &options, &cancel).unwrap();
        let decoded = decode_bytes(&bytes, None, &DecodeOptions::default(), &cancel).unwrap();
        assert!(decoded.info.has_icc);
        // Converting to the profile and back returns the same colours. With 8
        // bits per channel the wider gamut costs a few levels in the darkest
        // channel of a saturated colour, so single pixels may differ more
        // than the picture as a whole.
        let back = decoded.media.into_first().unwrap();
        let (mean, largest) = (
            mean_difference(&image, &back),
            max_difference(&image, &back),
        );
        assert!(mean < 0.5 && largest <= 8, "mean {mean}, largest {largest}");

        let plain = EncodeOptions::from_spec(&OutputSpec::default(), ImageFormat::Png, &source);
        assert!(plain.metadata.icc.is_none());
    }

    #[test]
    fn animations_round_trip() {
        let frames: Vec<Frame> = (0..3)
            .map(|index| Frame {
                image: Image::new(
                    20,
                    12,
                    Depth::Eight,
                    Color::rgb(index * 100, 40, 200 - index * 90),
                )
                .unwrap(),
                delay_ms: 120,
            })
            .collect();
        let animation = Animation {
            frames,
            loop_count: 0,
        };
        let cancel = CancelToken::new();
        let mut formats = vec![ImageFormat::Gif, ImageFormat::Png];
        if cfg!(feature = "webp-lossy") {
            formats.push(ImageFormat::Webp);
        }
        for format in formats {
            let mut options = EncodeOptions::new(format);
            options.lossless = true;
            let bytes =
                encode_media(&Media::Animated(animation.clone()), &options, &cancel).unwrap();
            let decoded = decode_bytes(&bytes, None, &DecodeOptions::default(), &cancel).unwrap();
            let Media::Animated(read) = decoded.media else {
                panic!("{format:?} lost its frames");
            };
            assert_eq!(read.frames.len(), 3, "{format:?}");
            assert_eq!(read.frames[0].delay_ms, 120, "{format:?}");
            let colour = read.frames[1].image.pixel8(5, 5);
            assert!(colour[0].abs_diff(100) <= 8, "{format:?} {colour:?}");
        }
    }

    #[test]
    fn animation_in_still_format_keeps_first_frame() {
        let animation = Animation {
            frames: vec![
                Frame {
                    image: Image::new(8, 8, Depth::Eight, Color::rgb(10, 200, 30)).unwrap(),
                    delay_ms: 50,
                },
                Frame {
                    image: Image::new(8, 8, Depth::Eight, Color::rgb(200, 10, 30)).unwrap(),
                    delay_ms: 50,
                },
            ],
            loop_count: 0,
        };
        let cancel = CancelToken::new();
        let mut options = EncodeOptions::new(ImageFormat::Bmp);
        options.lossless = true;
        let bytes = encode_media(&Media::Animated(animation), &options, &cancel).unwrap();
        let decoded = decode_bytes(&bytes, None, &DecodeOptions::default(), &cancel).unwrap();
        assert_eq!(
            decoded.media.into_first().unwrap().pixel8(2, 2),
            [10, 200, 30, 255]
        );
    }

    #[test]
    fn input_only_formats_are_rejected() {
        let image = sample(8, 8, false, Depth::Eight);
        let error = encode(
            &image,
            &EncodeOptions::new(ImageFormat::Psd),
            &CancelToken::new(),
        )
        .unwrap_err();
        assert_eq!(error.code, "cannotEncode");
    }

    #[test]
    fn cancelled_encode_stops() {
        let image = sample(8, 8, false, Depth::Eight);
        let cancel = CancelToken::new();
        cancel.cancel();
        let error = encode(&image, &EncodeOptions::new(ImageFormat::Png), &cancel).unwrap_err();
        assert!(error.is_cancelled());
    }
}
