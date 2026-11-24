//! Encoding towards a goal: a file size, a visual quality score, or the
//! smallest of several formats.

use pixelit_codecs::{decode_bytes, encode_media, DecodeOptions, EncodeOptions, Metadata};
use pixelit_core::image::{Animation, Frame};
use pixelit_core::ops::geometry::{crop_pixels, PixelRect};
use pixelit_core::ops::resize::resize_to;
use pixelit_core::pipeline::{OutputFormat, OutputSpec, ResizeFilter, Target};
use pixelit_core::{CancelToken, Color, Error, Image, ImageFormat, Media, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use ssimulacra2::{compute_frame_ssimulacra2, ColorPrimaries, Rgb, TransferCharacteristic};

/// Side of the central area the quality score is measured on. Measuring a
/// crop at full resolution keeps compression artefacts at their real scale.
const SCORE_SIDE: u32 = 1024;

/// Smallest side SSIMULACRA2 accepts.
const SCORE_MIN_SIDE: u32 = 8;

/// Quality range searched for a target.
const SEARCH_MIN: u8 = 5;
const SEARCH_MAX: u8 = 98;

/// Lowest quality used before the image is reduced to meet a size target.
const SIZE_FLOOR_QUALITY: u8 = 35;

/// Reductions tried to meet a size target.
const SHRINK_ROUNDS: u32 = 8;

/// An encoded file and how it was made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Encoded {
    #[serde(skip)]
    pub bytes: Vec<u8>,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    /// Quality used, for lossy output.
    pub quality: Option<u8>,
    /// SSIMULACRA2 score against the unencoded picture, when measured.
    pub score: Option<f32>,
    /// The size target could not be met.
    pub target_missed: bool,
}

fn central_crop(image: &Image, side: u32) -> Result<Image> {
    let width = image.width().min(side);
    let height = image.height().min(side);
    crop_pixels(
        image,
        PixelRect {
            x: (image.width() - width) / 2,
            y: (image.height() - height) / 2,
            width,
            height,
        },
    )
}

fn to_score_frame(image: &Image) -> Option<Rgb> {
    let mut flat = image.clone();
    if flat.has_alpha() {
        flat.flatten(Color::WHITE);
    }
    let data: Vec<[f32; 3]> = flat
        .to_rgb8()
        .chunks_exact(3)
        .map(|pixel| {
            [
                f32::from(pixel[0]) / 255.0,
                f32::from(pixel[1]) / 255.0,
                f32::from(pixel[2]) / 255.0,
            ]
        })
        .collect();
    Rgb::new(
        data,
        image.width() as usize,
        image.height() as usize,
        TransferCharacteristic::SRGB,
        ColorPrimaries::BT709,
    )
    .ok()
}

/// SSIMULACRA2 score of `distorted` against `original`: 100 is identical,
/// 90 is visually lossless, 70 is high quality, 50 is medium. `None` when the
/// pictures cannot be compared (different sizes or smaller than 8 pixels).
pub fn score(original: &Image, distorted: &Image) -> Option<f32> {
    if original.width() != distorted.width()
        || original.height() != distorted.height()
        || original.width() < SCORE_MIN_SIDE
        || original.height() < SCORE_MIN_SIDE
    {
        return None;
    }
    let source = to_score_frame(original)?;
    let other = to_score_frame(distorted)?;
    compute_frame_ssimulacra2(source, other)
        .ok()
        .map(|value| value as f32)
}

/// Score of an encoded file against the picture it was made from, measured
/// on the central area. A reduced file is compared with the picture reduced
/// the same way.
pub fn score_encoded(original: &Image, bytes: &[u8], format: ImageFormat) -> Option<f32> {
    let options = DecodeOptions {
        animation: false,
        deep: false,
        ..DecodeOptions::default()
    };
    let decoded = decode_bytes(bytes, Some(format), &options, &CancelToken::new()).ok()?;
    let decoded = decoded.media.into_first()?;
    let reference = if decoded.width() == original.width() && decoded.height() == original.height()
    {
        central_crop(original, SCORE_SIDE).ok()?
    } else {
        let resized = resize_to(
            original,
            decoded.width(),
            decoded.height(),
            ResizeFilter::Lanczos3,
            true,
            None,
        )
        .ok()?;
        central_crop(&resized, SCORE_SIDE).ok()?
    };
    let distorted = central_crop(&decoded, SCORE_SIDE).ok()?;
    score(&reference, &distorted)
}

fn first_size(media: &Media) -> (u32, u32) {
    media
        .first()
        .map_or((0, 0), |image| (image.width(), image.height()))
}

fn plain(
    media: &Media,
    options: &EncodeOptions,
    cancel: &CancelToken,
    lossy: bool,
) -> Result<Encoded> {
    let bytes = encode_media(media, options, cancel)?;
    let (width, height) = first_size(media);
    Ok(Encoded {
        bytes,
        format: options.format,
        width,
        height,
        quality: lossy.then(|| options.quality.clamp(1, 100)),
        score: None,
        target_missed: false,
    })
}

/// Finds the highest quality in `low..=high` whose file fits into `limit`.
fn search_size(
    media: &Media,
    options: &EncodeOptions,
    low: u8,
    high: u8,
    limit: u64,
    cancel: &CancelToken,
) -> Result<Option<Encoded>> {
    let (mut low, mut high) = (low.max(1), high.max(1));
    let mut best: Option<Encoded> = None;
    while low <= high {
        cancel.check()?;
        let middle = low + (high - low) / 2;
        let candidate = plain(media, &options.clone().with_quality(middle), cancel, true)?;
        if candidate.bytes.len() as u64 <= limit {
            best = Some(candidate);
            if middle == u8::MAX {
                break;
            }
            low = middle + 1;
        } else {
            if middle == 0 {
                break;
            }
            high = middle - 1;
        }
    }
    Ok(best)
}

fn shrink(media: &Media, factor: f64) -> Result<Media> {
    let reduce = |image: &Image| -> Result<Image> {
        let width = ((f64::from(image.width()) * factor).round() as u32).max(1);
        let height = ((f64::from(image.height()) * factor).round() as u32).max(1);
        resize_to(image, width, height, ResizeFilter::Lanczos3, true, None)
    };
    Ok(match media {
        Media::Still(image) => Media::Still(reduce(image)?),
        Media::Animated(animation) => Media::Animated(Animation {
            loop_count: animation.loop_count,
            frames: animation
                .frames
                .iter()
                .map(|frame| {
                    Ok(Frame {
                        image: reduce(&frame.image)?,
                        delay_ms: frame.delay_ms,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        }),
    })
}

/// The largest file that fits into `limit` bytes. Quality goes down first;
/// when the lowest reasonable quality is still too large, the picture is
/// reduced step by step.
fn fit_bytes(
    media: &Media,
    options: &EncodeOptions,
    limit: u64,
    lossy: bool,
    cancel: &CancelToken,
) -> Result<Encoded> {
    let first = plain(media, options, cancel, lossy)?;
    if first.bytes.len() as u64 <= limit {
        return Ok(first);
    }
    let top = options.quality.clamp(1, 100);
    if lossy && top > 1 {
        let floor = SIZE_FLOOR_QUALITY.min(top - 1);
        if let Some(found) = search_size(media, options, floor, top - 1, limit, cancel)? {
            return Ok(found);
        }
    }
    let mut smallest = first;
    let mut current = shrink(media, 1.0)?;
    for _ in 0..SHRINK_ROUNDS {
        cancel.check()?;
        let (width, height) = first_size(&current);
        if width <= 1 && height <= 1 {
            break;
        }
        // File size grows roughly with the pixel count.
        let ratio = limit as f64 / smallest.bytes.len().max(1) as f64;
        let factor = (ratio.sqrt() * 0.92).clamp(0.3, 0.9);
        current = shrink(&current, factor)?;
        let found = if lossy {
            let floor = SIZE_FLOOR_QUALITY.min(top);
            search_size(&current, options, floor, top, limit, cancel)?
        } else {
            Some(plain(&current, options, cancel, false)?)
                .filter(|encoded| encoded.bytes.len() as u64 <= limit)
        };
        if let Some(found) = found {
            return Ok(found);
        }
        let floor = SIZE_FLOOR_QUALITY.min(top);
        smallest = plain(
            &current,
            &options.clone().with_quality(floor),
            cancel,
            lossy,
        )?;
        if smallest.bytes.len() as u64 <= limit {
            return Ok(smallest);
        }
    }
    smallest.target_missed = true;
    Ok(smallest)
}

/// The smallest file whose score stays at or above `wanted`. The quality is
/// searched on the central area of the first frame and then used for the
/// whole picture.
fn fit_quality(
    media: &Media,
    options: &EncodeOptions,
    wanted: f32,
    cancel: &CancelToken,
) -> Result<Encoded> {
    let Some(first) = media.first() else {
        return Err(Error::new("emptyImage"));
    };
    let sample = central_crop(first, SCORE_SIDE)?;
    let sample_media = Media::Still(sample.clone());
    let mut probe_options = options.clone();
    probe_options.metadata = Metadata::default();
    let measure = |quality: u8| -> Result<Option<f32>> {
        cancel.check()?;
        let bytes = encode_media(
            &sample_media,
            &probe_options.clone().with_quality(quality),
            cancel,
        )?;
        Ok(score_encoded(&sample, &bytes, options.format))
    };

    let (mut low, mut high) = (SEARCH_MIN, SEARCH_MAX);
    let mut best: Option<(u8, f32)> = None;
    while low <= high {
        let middle = low + (high - low) / 2;
        match measure(middle)? {
            Some(value) if value >= wanted => {
                best = Some((middle, value));
                high = middle - 1;
            }
            Some(_) => low = middle + 1,
            // The picture is too small to measure: keep the requested quality.
            None => return plain(media, options, cancel, true),
        }
    }
    let (quality, value) = match best {
        Some(found) => found,
        None => (SEARCH_MAX, measure(SEARCH_MAX)?.unwrap_or_default()),
    };
    let mut encoded = plain(media, &options.clone().with_quality(quality), cancel, true)?;
    encoded.score = Some(value);
    encoded.target_missed = value < wanted;
    Ok(encoded)
}

/// Encodes into one format, honouring the target of the output description.
pub fn encode_to(
    media: &Media,
    spec: &OutputSpec,
    format: ImageFormat,
    source: &Metadata,
    cancel: &CancelToken,
) -> Result<Encoded> {
    let options = EncodeOptions::from_spec(spec, format, source);
    let lossy = format.is_lossy() && !options.lossless;
    match spec.target {
        Target::None => plain(media, &options, cancel, lossy),
        Target::Bytes { bytes } => fit_bytes(media, &options, bytes.max(1), lossy, cancel),
        Target::Quality { score } if lossy => {
            fit_quality(media, &options, score.clamp(0.0, 100.0), cancel)
        }
        Target::Quality { .. } => plain(media, &options, cancel, false),
    }
}

/// Formats the output may take for this picture, best candidates first.
pub fn candidates(
    spec: &OutputSpec,
    source: Option<ImageFormat>,
    media: &Media,
) -> Vec<ImageFormat> {
    let animated = media.frame_count() > 1;
    let transparent = media.first().is_some_and(|image| !image.is_opaque());
    let fallback = if animated {
        ImageFormat::Webp
    } else if transparent {
        ImageFormat::Png
    } else {
        ImageFormat::Jpeg
    };
    match spec.format {
        OutputFormat::Same => {
            vec![source
                .filter(|format| format.can_encode())
                .unwrap_or(fallback)]
        }
        OutputFormat::Auto => {
            let mut formats: Vec<ImageFormat> = spec
                .auto_formats
                .iter()
                .copied()
                .filter(|format| format.can_encode())
                .filter(|format| !transparent || format.supports_alpha())
                .filter(|format| !animated || format.supports_animation())
                .collect();
            formats.dedup();
            if formats.is_empty() {
                formats.push(fallback);
            }
            formats
        }
        fixed => vec![fixed.fixed().unwrap_or(fallback)],
    }
}

/// Encodes the picture as the output description asks. With several candidate
/// formats every one is encoded and the best is kept: the highest score for a
/// size target, the smallest file otherwise.
pub fn encode_output(
    media: &Media,
    spec: &OutputSpec,
    source_format: Option<ImageFormat>,
    source: &Metadata,
    cancel: &CancelToken,
) -> Result<Encoded> {
    let formats = candidates(spec, source_format, media);
    if let [format] = formats.as_slice() {
        return encode_to(media, spec, *format, source, cancel);
    }
    let results: Vec<Result<Encoded>> = formats
        .par_iter()
        .map(|format| encode_to(media, spec, *format, source, cancel))
        .collect();
    cancel.check()?;
    let mut encoded = Vec::with_capacity(results.len());
    let mut failure = None;
    for result in results {
        match result {
            Ok(value) => encoded.push(value),
            Err(error) => failure = Some(error),
        }
    }
    if encoded.is_empty() {
        return Err(failure.unwrap_or_else(|| Error::new("encodeFailed")));
    }
    let by_size = matches!(spec.target, Target::Bytes { .. });
    if by_size {
        if let Some(original) = media.first() {
            for candidate in &mut encoded {
                if !candidate.target_missed {
                    candidate.score = score_encoded(original, &candidate.bytes, candidate.format);
                }
            }
        }
        encoded.sort_by(|a, b| {
            a.target_missed.cmp(&b.target_missed).then(
                b.score
                    .unwrap_or(f32::MIN)
                    .total_cmp(&a.score.unwrap_or(f32::MIN)),
            )
        });
    } else {
        encoded.sort_by_key(|candidate| (candidate.target_missed, candidate.bytes.len()));
    }
    Ok(encoded.swap_remove(0))
}

/// One row of a format comparison.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub format: ImageFormat,
    pub bytes: u64,
    pub quality: Option<u8>,
    pub score: Option<f32>,
    pub elapsed_ms: u64,
    pub error: Option<Error>,
}

/// Encodes the picture into every given format with the same settings and
/// reports size, score and time of each.
pub fn compare_formats(
    media: &Media,
    spec: &OutputSpec,
    formats: &[ImageFormat],
    source: &Metadata,
    cancel: &CancelToken,
) -> Vec<Comparison> {
    formats
        .par_iter()
        .map(|format| {
            let started = std::time::Instant::now();
            let result = encode_to(media, spec, *format, source, cancel);
            let elapsed_ms = started.elapsed().as_millis() as u64;
            match result {
                Ok(encoded) => {
                    let score = encoded.score.or_else(|| {
                        media
                            .first()
                            .and_then(|image| score_encoded(image, &encoded.bytes, *format))
                    });
                    Comparison {
                        format: *format,
                        bytes: encoded.bytes.len() as u64,
                        quality: encoded.quality,
                        score,
                        elapsed_ms,
                        error: None,
                    }
                }
                Err(error) => Comparison {
                    format: *format,
                    bytes: 0,
                    quality: None,
                    score: None,
                    elapsed_ms,
                    error: Some(error),
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use pixelit_core::pipeline::OutputFormat;
    use pixelit_core::Depth;

    use super::*;

    /// A smooth gradient with some texture, so lossy codecs have work to do.
    fn sample(width: u32, height: u32) -> Image {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let noise = ((x * 31 + y * 17) % 23) as u8;
                data.push((x * 255 / width) as u8);
                data.push((y * 255 / height) as u8);
                data.push(((x + y) % 256) as u8 / 2 + noise);
                data.push(255);
            }
        }
        let mut image = Image::from_rgba8(width, height, data).unwrap();
        image.normalize_alpha();
        image
    }

    #[test]
    fn identical_pictures_score_high() {
        let image = sample(64, 64);
        let value = score(&image, &image).unwrap();
        assert!(value > 99.0, "{value}");
        let tiny = Image::new(4, 4, Depth::Eight, Color::WHITE).unwrap();
        assert_eq!(score(&tiny, &tiny), None);
        assert_eq!(score(&image, &sample(32, 64)), None);
    }

    #[test]
    fn lower_quality_scores_lower() {
        let image = sample(160, 120);
        let media = Media::Still(image.clone());
        let cancel = CancelToken::new();
        let encode = |quality: u8| {
            let options = EncodeOptions::new(ImageFormat::Jpeg).with_quality(quality);
            encode_media(&media, &options, &cancel).unwrap()
        };
        let high = score_encoded(&image, &encode(95), ImageFormat::Jpeg).unwrap();
        let low = score_encoded(&image, &encode(10), ImageFormat::Jpeg).unwrap();
        assert!(high > low, "{high} {low}");
    }

    #[test]
    fn size_target_is_met() {
        let media = Media::Still(sample(320, 240));
        let cancel = CancelToken::new();
        let mut spec = OutputSpec {
            format: OutputFormat::Jpeg,
            quality: 95,
            ..OutputSpec::default()
        };
        let free = encode_output(&media, &spec, None, &Metadata::default(), &cancel).unwrap();
        let limit = free.bytes.len() as u64 / 2;
        spec.target = Target::Bytes { bytes: limit };
        let fitted = encode_output(&media, &spec, None, &Metadata::default(), &cancel).unwrap();
        assert!(fitted.bytes.len() as u64 <= limit);
        assert!(!fitted.target_missed);
        assert!(fitted.quality.unwrap() < 95);
    }

    #[test]
    fn tiny_size_target_reduces_the_picture() {
        let media = Media::Still(sample(320, 240));
        let cancel = CancelToken::new();
        let spec = OutputSpec {
            format: OutputFormat::Jpeg,
            target: Target::Bytes { bytes: 1500 },
            ..OutputSpec::default()
        };
        let fitted = encode_output(&media, &spec, None, &Metadata::default(), &cancel).unwrap();
        if !fitted.target_missed {
            assert!(fitted.bytes.len() <= 1500);
        }
        assert!(fitted.width < 320);
    }

    #[test]
    fn quality_target_picks_a_lower_quality_for_a_lower_score() {
        let media = Media::Still(sample(256, 256));
        let cancel = CancelToken::new();
        let encode = |wanted: f32| {
            let spec = OutputSpec {
                format: OutputFormat::Jpeg,
                target: Target::Quality { score: wanted },
                ..OutputSpec::default()
            };
            encode_output(&media, &spec, None, &Metadata::default(), &cancel).unwrap()
        };
        let fine = encode(85.0);
        let rough = encode(40.0);
        assert!(fine.quality.unwrap() >= rough.quality.unwrap());
        assert!(fine.bytes.len() >= rough.bytes.len());
        assert!(rough.score.is_some());
    }

    #[test]
    fn candidates_follow_the_picture() {
        let opaque = Media::Still(sample(16, 16));
        let mut clear = Image::new(16, 16, Depth::Eight, Color::TRANSPARENT).unwrap();
        clear.normalize_alpha();
        let clear = Media::Still(clear);
        let same = OutputSpec::default();
        assert_eq!(
            candidates(&same, Some(ImageFormat::Webp), &opaque),
            [ImageFormat::Webp]
        );
        assert_eq!(
            candidates(&same, Some(ImageFormat::Heic), &opaque),
            [ImageFormat::Jpeg]
        );
        assert_eq!(
            candidates(&same, Some(ImageFormat::Svg), &clear),
            [ImageFormat::Png]
        );
        let auto = OutputSpec {
            format: OutputFormat::Auto,
            ..OutputSpec::default()
        };
        assert_eq!(
            candidates(&auto, None, &opaque),
            [ImageFormat::Jpeg, ImageFormat::Webp, ImageFormat::Avif]
        );
        assert_eq!(
            candidates(&auto, None, &clear),
            [ImageFormat::Webp, ImageFormat::Avif]
        );
    }

    #[test]
    fn auto_format_keeps_the_smallest_file() {
        let media = Media::Still(sample(128, 96));
        let cancel = CancelToken::new();
        let spec = OutputSpec {
            format: OutputFormat::Auto,
            auto_formats: vec![ImageFormat::Jpeg, ImageFormat::Webp, ImageFormat::Png],
            ..OutputSpec::default()
        };
        let best = encode_output(&media, &spec, None, &Metadata::default(), &cancel).unwrap();
        for format in &spec.auto_formats {
            let other = encode_to(&media, &spec, *format, &Metadata::default(), &cancel).unwrap();
            assert!(best.bytes.len() <= other.bytes.len());
        }
        let rows = compare_formats(
            &media,
            &spec,
            &spec.auto_formats,
            &Metadata::default(),
            &cancel,
        );
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|row| row.error.is_none() && row.bytes > 0));
    }
}
