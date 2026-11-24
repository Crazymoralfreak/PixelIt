//! Resizing on top of `fast_image_resize` (SIMD), optionally in linear light.

use fast_image_resize::images::{Image as FirImage, ImageRef, TypedImage, TypedImageRef};
use fast_image_resize::pixels::{F32x4, U16x4};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

use crate::color::{linear16_to_srgb8, linear_to_srgb, srgb16_to_linear};
use crate::error::{Error, Result};
use crate::image::{Image, Pixels, Sample};
use crate::pipeline::{Resize, ResizeFilter, ResizeMode};

/// Largest side a resize may produce.
pub const MAX_SIDE: u32 = 65_535;

/// The result of planning a resize: the output size and, for `Fill`, the part
/// of the source that goes into it as `[left, top, width, height]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub width: u32,
    pub height: u32,
    pub crop: Option<[f64; 4]>,
}

fn round_side(value: f64) -> u32 {
    if value.is_nan() {
        return 1;
    }
    value.round().clamp(1.0, f64::from(MAX_SIDE)) as u32
}

/// Computes the output size for a source of `src_width` x `src_height`.
///
/// `scale` is the size of the source relative to the full file; target sizes in
/// pixels are multiplied by it so a reduced preview keeps the proportions.
pub fn plan(src_width: u32, src_height: u32, spec: &Resize, scale: f32) -> Plan {
    let sw = f64::from(src_width.max(1));
    let sh = f64::from(src_height.max(1));
    let scale = if scale.is_finite() && scale > 0.0 {
        f64::from(scale)
    } else {
        1.0
    };
    let want_w = f64::from(spec.width) * scale;
    let want_h = f64::from(spec.height) * scale;
    let limit = |factor: f64| {
        if spec.upscale || !factor.is_finite() {
            factor
        } else {
            factor.min(1.0)
        }
    };
    let uniform = |factor: f64| {
        let factor = limit(factor);
        let factor = if factor.is_finite() && factor > 0.0 {
            factor
        } else {
            1.0
        };
        Plan {
            width: round_side(sw * factor),
            height: round_side(sh * factor),
            crop: None,
        }
    };
    let fit_factor = match (spec.width, spec.height) {
        (0, 0) => 1.0,
        (0, _) => want_h / sh,
        (_, 0) => want_w / sw,
        _ => (want_w / sw).min(want_h / sh),
    };

    match spec.mode {
        ResizeMode::Fit => uniform(fit_factor),
        ResizeMode::Fill => {
            if spec.width == 0 || spec.height == 0 {
                return uniform(fit_factor);
            }
            let cover = (want_w / sw).max(want_h / sh);
            let factor = limit(cover);
            // When enlarging is off the target shrinks, keeping its aspect ratio.
            let shrink = factor / cover;
            let width = round_side(want_w * shrink);
            let height = round_side(want_h * shrink);
            let crop_w = (f64::from(width) / factor).min(sw);
            let crop_h = (f64::from(height) / factor).min(sh);
            Plan {
                width,
                height,
                crop: Some([(sw - crop_w) / 2.0, (sh - crop_h) / 2.0, crop_w, crop_h]),
            }
        }
        ResizeMode::Exact => Plan {
            width: if spec.width == 0 {
                src_width.max(1)
            } else {
                round_side(want_w)
            },
            height: if spec.height == 0 {
                src_height.max(1)
            } else {
                round_side(want_h)
            },
            crop: None,
        },
        ResizeMode::Percent => uniform(f64::from(spec.percent) / 100.0),
        ResizeMode::LongSide => uniform(want_w / sw.max(sh)),
        ResizeMode::ShortSide => uniform(want_w / sw.min(sh)),
        ResizeMode::Width => uniform(want_w / sw),
        ResizeMode::Height => uniform(want_h / sh),
    }
}

fn algorithm(filter: ResizeFilter) -> ResizeAlg {
    match filter {
        ResizeFilter::Nearest => ResizeAlg::Nearest,
        ResizeFilter::Box => ResizeAlg::Convolution(FilterType::Box),
        ResizeFilter::Bilinear => ResizeAlg::Convolution(FilterType::Bilinear),
        ResizeFilter::Hamming => ResizeAlg::Convolution(FilterType::Hamming),
        ResizeFilter::CatmullRom => ResizeAlg::Convolution(FilterType::CatmullRom),
        ResizeFilter::Mitchell => ResizeAlg::Convolution(FilterType::Mitchell),
        ResizeFilter::Gaussian => ResizeAlg::Convolution(FilterType::Gaussian),
        ResizeFilter::Lanczos3 => ResizeAlg::Convolution(FilterType::Lanczos3),
    }
}

fn failed(error: impl std::fmt::Debug) -> Error {
    Error::new("resizeFailed").detail(format!("{error:?}"))
}

/// Resizes in linear light. Both depths are worked on in floating point:
/// linear values need more than 8 bits, and a filter with negative lobes has
/// to be free to go below black between its two passes. `encode` turns a
/// linear value back into a stored sample.
fn resize_linear<S: Sample>(
    data: &[S],
    (source_width, source_height): (u32, u32),
    (width, height): (u32, u32),
    resizer: &mut Resizer,
    options: &ResizeOptions,
    encode: impl Fn(f32) -> S,
) -> Result<Vec<S>> {
    let to_linear = srgb16_to_linear();
    // Position of a sample in the table of 16-bit values.
    let step = (u32::from(u16::MAX) / S::MAX) as usize;
    let source_pixels: Vec<F32x4> = data
        .chunks_exact(4)
        .map(|pixel| {
            F32x4::new([
                to_linear[pixel[0].to_u32() as usize * step],
                to_linear[pixel[1].to_u32() as usize * step],
                to_linear[pixel[2].to_u32() as usize * step],
                pixel[3].to_unit(),
            ])
        })
        .collect();
    let source = TypedImageRef::new(source_width, source_height, &source_pixels).map_err(failed)?;
    let mut target = TypedImage::<F32x4>::new(width, height);
    resizer
        .resize_typed(&source, &mut target, options)
        .map_err(failed)?;
    let mut out = Vec::with_capacity(target.pixels().len() * 4);
    for pixel in target.pixels() {
        let [r, g, b, a] = pixel.0;
        out.extend_from_slice(&[encode(r), encode(g), encode(b), S::from_unit(a)]);
    }
    Ok(out)
}

/// Resizes by a [`Resize`] step.
pub fn resize(image: &Image, spec: &Resize, scale: f32) -> Result<Image> {
    let plan = plan(image.width, image.height, spec, scale);
    resize_to(
        image,
        plan.width,
        plan.height,
        spec.filter,
        spec.gamma_correct,
        plan.crop,
    )
}

/// Resizes to an exact size. `crop` selects the part of the source to use.
pub fn resize_to(
    image: &Image,
    width: u32,
    height: u32,
    filter: ResizeFilter,
    gamma_correct: bool,
    crop: Option<[f64; 4]>,
) -> Result<Image> {
    if width == 0 || height == 0 {
        return Err(Error::new("emptyImage"));
    }
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(Error::new("tooLarge")
            .with("width", width)
            .with("height", height));
    }
    if crop.is_none() && width == image.width && height == image.height {
        return Ok(image.clone());
    }

    let mut options = ResizeOptions::new()
        .resize_alg(algorithm(filter))
        .use_alpha(image.alpha);
    if let Some([left, top, crop_width, crop_height]) = crop {
        options = options.crop(left, top, crop_width, crop_height);
    }
    let linear = gamma_correct && filter != ResizeFilter::Nearest;
    let mut resizer = Resizer::new();

    let pixels = match (&image.pixels, linear) {
        (Pixels::U8(data), false) => {
            let source =
                ImageRef::new(image.width, image.height, data, PixelType::U8x4).map_err(failed)?;
            let mut target = FirImage::new(width, height, PixelType::U8x4);
            resizer
                .resize(&source, &mut target, &options)
                .map_err(failed)?;
            Pixels::U8(target.into_vec())
        }
        (Pixels::U8(data), true) => {
            let to_srgb = linear16_to_srgb8();
            Pixels::U8(resize_linear(
                data,
                (image.width, image.height),
                (width, height),
                &mut resizer,
                &options,
                |linear| to_srgb[usize::from(u16::from_unit(linear))],
            )?)
        }
        (Pixels::U16(data), false) => {
            let source_pixels: Vec<U16x4> = data
                .chunks_exact(4)
                .map(|pixel| U16x4::new([pixel[0], pixel[1], pixel[2], pixel[3]]))
                .collect();
            let source =
                TypedImageRef::new(image.width, image.height, &source_pixels).map_err(failed)?;
            let mut target = TypedImage::<U16x4>::new(width, height);
            resizer
                .resize_typed(&source, &mut target, &options)
                .map_err(failed)?;
            let mut out = Vec::with_capacity(target.pixels().len() * 4);
            for pixel in target.pixels() {
                out.extend_from_slice(&pixel.0);
            }
            Pixels::U16(out)
        }
        (Pixels::U16(data), true) => Pixels::U16(resize_linear(
            data,
            (image.width, image.height),
            (width, height),
            &mut resizer,
            &options,
            |linear| u16::from_unit(linear_to_srgb(linear.clamp(0.0, 1.0))),
        )?),
    };

    let expected = width as usize * height as usize * 4;
    let pixels = match pixels {
        Pixels::U8(mut data) => {
            data.truncate(expected);
            Pixels::U8(data)
        }
        Pixels::U16(mut data) => {
            data.truncate(expected);
            Pixels::U16(data)
        }
    };
    Ok(Image {
        width,
        height,
        alpha: image.alpha,
        pixels,
    })
}

/// Reduces the image so that its long side is at most `max_side`. Used for
/// previews and thumbnails; returns a copy when the image is small enough.
pub fn fit_within(image: &Image, max_side: u32, filter: ResizeFilter) -> Result<Image> {
    let long = image.width.max(image.height);
    if long <= max_side || max_side == 0 {
        return Ok(image.clone());
    }
    let factor = f64::from(max_side) / f64::from(long);
    resize_to(
        image,
        round_side(f64::from(image.width) * factor),
        round_side(f64::from(image.height) * factor),
        filter,
        false,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::image::Depth;

    fn spec(mode: ResizeMode, width: u32, height: u32) -> Resize {
        Resize {
            mode,
            width,
            height,
            ..Resize::default()
        }
    }

    #[test]
    fn fit_keeps_aspect_and_never_enlarges_by_default() {
        let plan_a = plan(4000, 3000, &spec(ResizeMode::Fit, 1920, 1080), 1.0);
        assert_eq!((plan_a.width, plan_a.height), (1440, 1080));
        let plan_b = plan(800, 600, &spec(ResizeMode::Fit, 1920, 1080), 1.0);
        assert_eq!((plan_b.width, plan_b.height), (800, 600));
        let plan_c = plan(4000, 3000, &spec(ResizeMode::Fit, 0, 300), 1.0);
        assert_eq!((plan_c.width, plan_c.height), (400, 300));
        let plan_d = plan(4000, 3000, &spec(ResizeMode::Fit, 0, 0), 1.0);
        assert_eq!((plan_d.width, plan_d.height), (4000, 3000));
    }

    #[test]
    fn upscale_flag_allows_enlarging() {
        let mut resize = spec(ResizeMode::Fit, 1600, 1600);
        resize.upscale = true;
        let planned = plan(800, 600, &resize, 1.0);
        assert_eq!((planned.width, planned.height), (1600, 1200));
    }

    #[test]
    fn fill_crops_the_centre() {
        let planned = plan(4000, 3000, &spec(ResizeMode::Fill, 1000, 1000), 1.0);
        assert_eq!((planned.width, planned.height), (1000, 1000));
        let [left, top, width, height] = planned.crop.unwrap();
        assert!((left - 500.0).abs() < 1e-6);
        assert!(top.abs() < 1e-6);
        assert!((width - 3000.0).abs() < 1e-6);
        assert!((height - 3000.0).abs() < 1e-6);
    }

    #[test]
    fn fill_without_upscale_keeps_target_aspect() {
        let planned = plan(400, 300, &spec(ResizeMode::Fill, 1000, 500), 1.0);
        assert_eq!((planned.width, planned.height), (400, 200));
        let [_, _, width, height] = planned.crop.unwrap();
        assert!((width - 400.0).abs() < 1e-6);
        assert!((height - 200.0).abs() < 1e-6);
    }

    #[test]
    fn other_modes() {
        let source = (3000, 2000);
        let size = |mode, width, height| {
            let planned = plan(source.0, source.1, &spec(mode, width, height), 1.0);
            (planned.width, planned.height)
        };
        assert_eq!(size(ResizeMode::Exact, 100, 50), (100, 50));
        assert_eq!(size(ResizeMode::LongSide, 1500, 0), (1500, 1000));
        assert_eq!(size(ResizeMode::ShortSide, 1000, 0), (1500, 1000));
        assert_eq!(size(ResizeMode::Width, 600, 0), (600, 400));
        assert_eq!(size(ResizeMode::Height, 0, 500), (750, 500));
        let mut percent = spec(ResizeMode::Percent, 0, 0);
        percent.percent = 10.0;
        let planned = plan(source.0, source.1, &percent, 1.0);
        assert_eq!((planned.width, planned.height), (300, 200));
    }

    #[test]
    fn preview_scale_keeps_proportions() {
        // A quarter-size preview of a 4000x3000 file resized to 2000 wide.
        let planned = plan(1000, 750, &spec(ResizeMode::Width, 2000, 0), 0.25);
        assert_eq!((planned.width, planned.height), (500, 375));
    }

    #[test]
    fn degenerate_values_stay_in_range() {
        let mut percent = spec(ResizeMode::Percent, 0, 0);
        percent.percent = f32::NAN;
        let planned = plan(10, 10, &percent, 1.0);
        assert_eq!((planned.width, planned.height), (10, 10));
        percent.percent = 0.0;
        let planned = plan(10, 10, &percent, f32::NAN);
        assert_eq!((planned.width, planned.height), (10, 10));
        let mut huge = spec(ResizeMode::Exact, u32::MAX, u32::MAX);
        huge.upscale = true;
        let planned = plan(10, 10, &huge, 1.0);
        assert_eq!((planned.width, planned.height), (MAX_SIDE, MAX_SIDE));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn resizes_every_depth_and_mode() {
        for depth in [Depth::Eight, Depth::Sixteen] {
            for gamma in [false, true] {
                let image = Image::new(64, 48, depth, Color::rgb(200, 100, 50)).unwrap();
                let small = resize_to(&image, 16, 12, ResizeFilter::Lanczos3, gamma, None).unwrap();
                assert_eq!((small.width(), small.height()), (16, 12));
                assert_eq!(small.depth(), depth);
                let [r, g, b, a] = small.pixel8(8, 6);
                assert!((i32::from(r) - 200).abs() <= 1, "{r}");
                assert!((i32::from(g) - 100).abs() <= 1, "{g}");
                assert!((i32::from(b) - 50).abs() <= 1, "{b}");
                assert_eq!(a, 255);
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn gamma_correct_resize_keeps_checkerboard_brightness() {
        let mut data = Vec::new();
        for y in 0..64u32 {
            for x in 0..64u32 {
                let value = if (x + y) % 2 == 0 { 255 } else { 0 };
                data.extend_from_slice(&[value, value, value, 255]);
            }
        }
        let image = Image::from_rgba8(64, 64, data).unwrap();
        let plain = resize_to(&image, 8, 8, ResizeFilter::Box, false, None).unwrap();
        let linear = resize_to(&image, 8, 8, ResizeFilter::Box, true, None).unwrap();
        // Half of the light is sRGB 188, half of the code values is 128.
        assert!((i32::from(plain.pixel8(4, 4)[0]) - 128).abs() <= 2);
        assert!((i32::from(linear.pixel8(4, 4)[0]) - 188).abs() <= 2);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn fit_within_limits_the_long_side() {
        let image = Image::new(300, 100, Depth::Eight, Color::WHITE).unwrap();
        let small = fit_within(&image, 150, ResizeFilter::Bilinear).unwrap();
        assert_eq!((small.width(), small.height()), (150, 50));
        let same = fit_within(&image, 1000, ResizeFilter::Bilinear).unwrap();
        assert_eq!((same.width(), same.height()), (300, 100));
    }
}
