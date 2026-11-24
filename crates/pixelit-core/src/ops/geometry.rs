//! Cropping, rotation and mirroring.

use crate::color::Color;
use crate::error::{Error, Result};
use crate::image::{Image, Pixels, Sample, CHANNELS};
use crate::ops::resize::MAX_SIDE;
use crate::pipeline::{Crop, CropMode, Flip, Rotate};
use crate::rows::build_rows;

/// A pixel rectangle inside an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

fn unit(value: f32) -> f64 {
    if value.is_finite() {
        f64::from(value.clamp(0.0, 1.0))
    } else {
        0.0
    }
}

/// The pixel rectangle a [`Crop`] step selects in a `width` x `height` image.
/// The result always lies inside the image and is at least one pixel large.
pub fn crop_rect(width: u32, height: u32, spec: &Crop) -> PixelRect {
    let width = width.max(1);
    let height = height.max(1);
    match spec.mode {
        CropMode::Rect => {
            let x = ((unit(spec.x) * f64::from(width)).round() as u32).min(width - 1);
            let y = ((unit(spec.y) * f64::from(height)).round() as u32).min(height - 1);
            let w = (unit(spec.width) * f64::from(width)).round() as u32;
            let h = (unit(spec.height) * f64::from(height)).round() as u32;
            PixelRect {
                x,
                y,
                width: w.clamp(1, width - x),
                height: h.clamp(1, height - y),
            }
        }
        CropMode::Aspect => {
            let aspect_w = f64::from(spec.aspect_width.max(1));
            let aspect_h = f64::from(spec.aspect_height.max(1));
            let factor = (f64::from(width) / aspect_w).min(f64::from(height) / aspect_h);
            let w = ((aspect_w * factor).round() as u32).clamp(1, width);
            let h = ((aspect_h * factor).round() as u32).clamp(1, height);
            let (anchor_x, anchor_y) = spec.anchor.fractions();
            PixelRect {
                x: (f64::from(width - w) * f64::from(anchor_x)).round() as u32,
                y: (f64::from(height - h) * f64::from(anchor_y)).round() as u32,
                width: w,
                height: h,
            }
        }
    }
}

fn copy_rect<S: Sample>(data: &[S], src_width: usize, rect: PixelRect) -> Vec<S> {
    let row_len = rect.width as usize * CHANNELS;
    let mut out = Vec::with_capacity(row_len * rect.height as usize);
    for row in 0..rect.height as usize {
        let start = ((rect.y as usize + row) * src_width + rect.x as usize) * CHANNELS;
        out.extend_from_slice(&data[start..start + row_len]);
    }
    out
}

/// Cuts a pixel rectangle out of the image.
pub fn crop_pixels(image: &Image, rect: PixelRect) -> Result<Image> {
    let fits = rect.width > 0
        && rect.height > 0
        && u64::from(rect.x) + u64::from(rect.width) <= u64::from(image.width)
        && u64::from(rect.y) + u64::from(rect.height) <= u64::from(image.height);
    if !fits {
        return Err(Error::new("badCrop"));
    }
    let src_width = image.width as usize;
    let pixels = match &image.pixels {
        Pixels::U8(data) => Pixels::U8(copy_rect(data, src_width, rect)),
        Pixels::U16(data) => Pixels::U16(copy_rect(data, src_width, rect)),
    };
    Ok(Image {
        width: rect.width,
        height: rect.height,
        alpha: image.alpha,
        pixels,
    })
}

/// Crops by a [`Crop`] step.
pub fn crop(image: &Image, spec: &Crop) -> Result<Image> {
    let rect = crop_rect(image.width, image.height, spec);
    if rect.x == 0 && rect.y == 0 && rect.width == image.width && rect.height == image.height {
        return Ok(image.clone());
    }
    crop_pixels(image, rect)
}

fn mirror_rows<S: Sample>(data: &mut [S], width: usize) {
    for row in data.chunks_exact_mut(width * CHANNELS) {
        let mut left = 0;
        let mut right = width - 1;
        while left < right {
            for channel in 0..CHANNELS {
                row.swap(left * CHANNELS + channel, right * CHANNELS + channel);
            }
            left += 1;
            right -= 1;
        }
    }
}

fn mirror_columns<S: Sample>(data: &mut [S], width: usize, height: usize) {
    let stride = width * CHANNELS;
    let mut top = 0;
    let mut bottom = height - 1;
    while top < bottom {
        let (head, tail) = data.split_at_mut(bottom * stride);
        head[top * stride..(top + 1) * stride].swap_with_slice(&mut tail[..stride]);
        top += 1;
        bottom -= 1;
    }
}

/// Mirrors the image left-right.
pub fn flip_horizontal(image: &mut Image) {
    let width = image.width as usize;
    match &mut image.pixels {
        Pixels::U8(data) => mirror_rows(data, width),
        Pixels::U16(data) => mirror_rows(data, width),
    }
}

/// Mirrors the image top-bottom.
pub fn flip_vertical(image: &mut Image) {
    let width = image.width as usize;
    let height = image.height as usize;
    match &mut image.pixels {
        Pixels::U8(data) => mirror_columns(data, width, height),
        Pixels::U16(data) => mirror_columns(data, width, height),
    }
}

/// Mirrors by a [`Flip`] step.
#[must_use]
pub fn flip(mut image: Image, spec: &Flip) -> Image {
    if spec.horizontal {
        flip_horizontal(&mut image);
    }
    if spec.vertical {
        flip_vertical(&mut image);
    }
    image
}

fn quarter_turns<S: Sample>(data: &[S], width: usize, height: usize, quarters: u8) -> Vec<S> {
    let (out_width, out_height) = if quarters % 2 == 1 {
        (height, width)
    } else {
        (width, height)
    };
    let mut out = Vec::with_capacity(data.len());
    for y in 0..out_height {
        for x in 0..out_width {
            let (source_x, source_y) = match quarters {
                1 => (y, height - 1 - x),
                2 => (width - 1 - x, height - 1 - y),
                _ => (width - 1 - y, x),
            };
            let index = (source_y * width + source_x) * CHANNELS;
            out.extend_from_slice(&data[index..index + CHANNELS]);
        }
    }
    out
}

/// Rotates clockwise by `quarters` right angles without resampling.
#[must_use]
pub fn rotate_quarters(image: Image, quarters: u8) -> Image {
    let quarters = quarters % 4;
    if quarters == 0 {
        return image;
    }
    let width = image.width as usize;
    let height = image.height as usize;
    let pixels = match &image.pixels {
        Pixels::U8(data) => Pixels::U8(quarter_turns(data, width, height, quarters)),
        Pixels::U16(data) => Pixels::U16(quarter_turns(data, width, height, quarters)),
    };
    let (out_width, out_height) = if quarters % 2 == 1 {
        (image.height, image.width)
    } else {
        (image.width, image.height)
    };
    Image {
        width: out_width,
        height: out_height,
        alpha: image.alpha,
        pixels,
    }
}

/// Applies an EXIF orientation value (1 to 8) so the image is upright.
#[must_use]
pub fn apply_orientation(mut image: Image, orientation: u16) -> Image {
    match orientation {
        2 => {
            flip_horizontal(&mut image);
            image
        }
        3 => rotate_quarters(image, 2),
        4 => {
            flip_vertical(&mut image);
            image
        }
        5 => {
            flip_horizontal(&mut image);
            rotate_quarters(image, 3)
        }
        6 => rotate_quarters(image, 1),
        7 => {
            flip_horizontal(&mut image);
            rotate_quarters(image, 1)
        }
        8 => rotate_quarters(image, 3),
        _ => image,
    }
}

/// Premultiplied unit RGBA of the pixel at `(x, y)`, or `outside` beyond the
/// image.
fn fetch<S: Sample>(
    data: &[S],
    width: usize,
    height: usize,
    x: i64,
    y: i64,
    outside: [f32; 4],
) -> [f32; 4] {
    if x < 0 || y < 0 || x >= width as i64 || y >= height as i64 {
        return outside;
    }
    let index = (y as usize * width + x as usize) * CHANNELS;
    let alpha = data[index + 3].to_unit();
    [
        data[index].to_unit() * alpha,
        data[index + 1].to_unit() * alpha,
        data[index + 2].to_unit() * alpha,
        alpha,
    ]
}

/// Rotates by an arbitrary angle with bilinear sampling.
fn rotate_free<S: Sample>(
    data: &[S],
    width: usize,
    height: usize,
    out_width: usize,
    out_height: usize,
    radians: f32,
    background: Color,
) -> Vec<S> {
    let (sin, cos) = radians.sin_cos();
    let [back_r, back_g, back_b, back_a] = background.to_unit();
    let outside = [back_r * back_a, back_g * back_a, back_b * back_a, back_a];
    let centre_x = out_width as f32 / 2.0;
    let centre_y = out_height as f32 / 2.0;
    let source_centre_x = width as f32 / 2.0;
    let source_centre_y = height as f32 / 2.0;

    build_rows(
        out_height,
        out_width * CHANNELS,
        |row_index, row: &mut [S]| {
            let dy = row_index as f32 + 0.5 - centre_y;
            for (column, pixel) in row.chunks_exact_mut(CHANNELS).enumerate() {
                let dx = column as f32 + 0.5 - centre_x;
                let source_x = dx * cos + dy * sin + source_centre_x - 0.5;
                let source_y = -dx * sin + dy * cos + source_centre_y - 0.5;
                let base_x = source_x.floor();
                let base_y = source_y.floor();
                let weight_x = source_x - base_x;
                let weight_y = source_y - base_y;
                let x0 = base_x as i64;
                let y0 = base_y as i64;
                let corners = [
                    (
                        fetch(data, width, height, x0, y0, outside),
                        (1.0 - weight_x) * (1.0 - weight_y),
                    ),
                    (
                        fetch(data, width, height, x0 + 1, y0, outside),
                        weight_x * (1.0 - weight_y),
                    ),
                    (
                        fetch(data, width, height, x0, y0 + 1, outside),
                        (1.0 - weight_x) * weight_y,
                    ),
                    (
                        fetch(data, width, height, x0 + 1, y0 + 1, outside),
                        weight_x * weight_y,
                    ),
                ];
                let mut sum = [0.0f32; 4];
                for (value, weight) in corners {
                    for channel in 0..CHANNELS {
                        sum[channel] += value[channel] * weight;
                    }
                }
                let alpha = sum[3];
                if alpha > 0.0 {
                    pixel[0] = S::from_unit(sum[0] / alpha);
                    pixel[1] = S::from_unit(sum[1] / alpha);
                    pixel[2] = S::from_unit(sum[2] / alpha);
                }
                pixel[3] = S::from_unit(alpha);
            }
        },
    )
}

/// Rotates clockwise by `degrees`. Right angles are exact; other angles are
/// resampled, with the uncovered corners filled by `background`.
pub fn rotate_by(image: Image, degrees: f32, expand: bool, background: Color) -> Result<Image> {
    if !degrees.is_finite() {
        return Ok(image);
    }
    let degrees = degrees.rem_euclid(360.0);
    let nearest_quarter = (degrees / 90.0).round();
    if (degrees - nearest_quarter * 90.0).abs() < 0.01 {
        let quarters = (nearest_quarter as u8) % 4;
        if quarters.is_multiple_of(2) || expand || image.width == image.height {
            return Ok(rotate_quarters(image, quarters));
        }
    }

    let radians = degrees.to_radians();
    let width = image.width as usize;
    let height = image.height as usize;
    let (out_width, out_height) = if expand {
        let (sin, cos) = radians.sin_cos();
        let w = (image.width as f32 * cos.abs() + image.height as f32 * sin.abs()).ceil();
        let h = (image.width as f32 * sin.abs() + image.height as f32 * cos.abs()).ceil();
        (w.max(1.0) as u32, h.max(1.0) as u32)
    } else {
        (image.width, image.height)
    };
    if out_width > MAX_SIDE || out_height > MAX_SIDE {
        return Err(Error::new("tooLarge")
            .with("width", out_width)
            .with("height", out_height));
    }
    let (out_w, out_h) = (out_width as usize, out_height as usize);
    let pixels = match &image.pixels {
        Pixels::U8(data) => Pixels::U8(rotate_free(
            data, width, height, out_w, out_h, radians, background,
        )),
        Pixels::U16(data) => Pixels::U16(rotate_free(
            data, width, height, out_w, out_h, radians, background,
        )),
    };
    Ok(Image {
        width: out_width,
        height: out_height,
        alpha: image.alpha || background.a != 255,
        pixels,
    })
}

/// Rotates by a [`Rotate`] step.
pub fn rotate(image: Image, spec: &Rotate) -> Result<Image> {
    rotate_by(image, spec.angle, spec.expand, spec.background)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Depth;
    use crate::pipeline::Anchor;

    /// A 3x2 image whose pixel values encode their position.
    fn sample() -> Image {
        let mut data = Vec::new();
        for y in 0..2u8 {
            for x in 0..3u8 {
                data.extend_from_slice(&[x * 10, y * 10, 0, 255]);
            }
        }
        Image::from_rgba8(3, 2, data).unwrap()
    }

    #[test]
    fn crop_rect_stays_inside() {
        let spec = Crop {
            x: 0.5,
            y: 0.5,
            width: 2.0,
            height: f32::NAN,
            ..Crop::default()
        };
        let rect = crop_rect(100, 80, &spec);
        assert_eq!(
            rect,
            PixelRect {
                x: 50,
                y: 40,
                width: 50,
                height: 1
            }
        );

        let far = Crop {
            x: 1.0,
            y: 1.0,
            ..Crop::default()
        };
        let rect = crop_rect(10, 10, &far);
        assert_eq!(
            rect,
            PixelRect {
                x: 9,
                y: 9,
                width: 1,
                height: 1
            }
        );
    }

    #[test]
    fn aspect_crop_uses_anchor() {
        let mut spec = Crop {
            mode: CropMode::Aspect,
            aspect_width: 1,
            aspect_height: 1,
            ..Crop::default()
        };
        assert_eq!(
            crop_rect(300, 100, &spec),
            PixelRect {
                x: 100,
                y: 0,
                width: 100,
                height: 100
            }
        );
        spec.anchor = Anchor::Right;
        assert_eq!(crop_rect(300, 100, &spec).x, 200);
        spec.aspect_width = 16;
        spec.aspect_height = 9;
        let rect = crop_rect(1000, 1000, &spec);
        assert_eq!((rect.width, rect.height), (1000, 563));
    }

    #[test]
    fn crop_copies_the_right_pixels() {
        let image = sample();
        let cut = crop_pixels(
            &image,
            PixelRect {
                x: 1,
                y: 1,
                width: 2,
                height: 1,
            },
        )
        .unwrap();
        assert_eq!(cut.to_rgba8(), vec![10, 10, 0, 255, 20, 10, 0, 255]);
        assert!(crop_pixels(
            &image,
            PixelRect {
                x: 2,
                y: 0,
                width: 2,
                height: 1
            }
        )
        .is_err());
    }

    #[test]
    fn flips_are_involutions() {
        let image = sample();
        let mut flipped = image.clone();
        flip_horizontal(&mut flipped);
        assert_eq!(flipped.pixel8(0, 0), [20, 0, 0, 255]);
        flip_horizontal(&mut flipped);
        assert_eq!(flipped, image);
        flip_vertical(&mut flipped);
        assert_eq!(flipped.pixel8(0, 0), [0, 10, 0, 255]);
        flip_vertical(&mut flipped);
        assert_eq!(flipped, image);
    }

    #[test]
    fn quarter_rotations_move_corners_clockwise() {
        let image = sample();
        let once = rotate_quarters(image.clone(), 1);
        assert_eq!((once.width(), once.height()), (2, 3));
        // The top-left pixel ends in the top-right corner.
        assert_eq!(once.pixel8(1, 0), [0, 0, 0, 255]);
        // The bottom-left pixel ends in the top-left corner.
        assert_eq!(once.pixel8(0, 0), [0, 10, 0, 255]);

        let twice = rotate_quarters(image.clone(), 2);
        assert_eq!(twice.pixel8(2, 1), [0, 0, 0, 255]);

        let thrice = rotate_quarters(image.clone(), 3);
        assert_eq!(thrice.pixel8(0, 2), [0, 0, 0, 255]);

        let full = rotate_quarters(rotate_quarters(image.clone(), 1), 3);
        assert_eq!(full, image);
    }

    #[test]
    fn orientation_values_match_quarter_turns() {
        let image = sample();
        assert_eq!(apply_orientation(image.clone(), 1), image);
        assert_eq!(
            apply_orientation(image.clone(), 6),
            rotate_quarters(image.clone(), 1)
        );
        assert_eq!(
            apply_orientation(image.clone(), 3),
            rotate_quarters(image.clone(), 2)
        );
        assert_eq!(
            apply_orientation(image.clone(), 8),
            rotate_quarters(image.clone(), 3)
        );
        // Transpose: the pixel at (x, y) moves to (y, x).
        let transposed = apply_orientation(image.clone(), 5);
        assert_eq!((transposed.width(), transposed.height()), (2, 3));
        assert_eq!(transposed.pixel8(1, 2), image.pixel8(2, 1));
        assert_eq!(transposed.pixel8(0, 1), image.pixel8(1, 0));
    }

    #[test]
    fn right_angles_take_the_exact_path() {
        let image = sample();
        let rotated = rotate_by(image.clone(), 90.0, true, Color::TRANSPARENT).unwrap();
        assert_eq!(rotated, rotate_quarters(image.clone(), 1));
        let rotated = rotate_by(image.clone(), -90.0, true, Color::TRANSPARENT).unwrap();
        assert_eq!(rotated, rotate_quarters(image.clone(), 3));
        let same = rotate_by(image.clone(), 360.0, true, Color::TRANSPARENT).unwrap();
        assert_eq!(same, image);
        let same = rotate_by(image.clone(), f32::NAN, true, Color::TRANSPARENT).unwrap();
        assert_eq!(same, image);
    }

    #[test]
    fn free_rotation_expands_and_fills_corners() {
        let image = Image::new(100, 100, Depth::Eight, Color::rgb(10, 200, 30)).unwrap();
        let rotated = rotate_by(image, 45.0, true, Color::TRANSPARENT).unwrap();
        assert_eq!((rotated.width(), rotated.height()), (142, 142));
        assert!(rotated.has_alpha());
        assert_eq!(rotated.pixel8(0, 0)[3], 0);
        assert_eq!(rotated.pixel8(71, 71), [10, 200, 30, 255]);

        let image = Image::new(100, 60, Depth::Sixteen, Color::BLACK).unwrap();
        let kept = rotate_by(image, 30.0, false, Color::WHITE).unwrap();
        assert_eq!((kept.width(), kept.height()), (100, 60));
        assert!(!kept.has_alpha());
        assert_eq!(kept.pixel8(0, 0), [255, 255, 255, 255]);
        assert_eq!(kept.pixel8(50, 30), [0, 0, 0, 255]);
    }
}
