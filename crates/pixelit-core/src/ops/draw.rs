//! Drawing primitives: anti-aliased painting through coverage functions,
//! layer compositing and signed distance helpers for shapes.

use crate::color::Color;
use crate::image::{Image, Pixels, Sample, CHANNELS};
use crate::rows::for_each_row;

/// A pixel region with exclusive right and bottom edges. May reach outside the
/// image; painting clips it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub left: i64,
    pub top: i64,
    pub right: i64,
    pub bottom: i64,
}

impl Bounds {
    /// The smallest region holding the two points, grown by `margin` pixels.
    pub fn around(ax: f32, ay: f32, bx: f32, by: f32, margin: f32) -> Self {
        let margin = margin.max(0.0) + 1.0;
        Self {
            left: (ax.min(bx) - margin).floor() as i64,
            top: (ay.min(by) - margin).floor() as i64,
            right: (ax.max(bx) + margin).ceil() as i64,
            bottom: (ay.max(by) + margin).ceil() as i64,
        }
    }

    /// The region clipped to a `width` x `height` image, or `None` when
    /// nothing is left.
    pub fn clip(self, width: u32, height: u32) -> Option<Self> {
        let clipped = Self {
            left: self.left.max(0),
            top: self.top.max(0),
            right: self.right.min(i64::from(width)),
            bottom: self.bottom.min(i64::from(height)),
        };
        (clipped.left < clipped.right && clipped.top < clipped.bottom).then_some(clipped)
    }

    pub fn width(self) -> usize {
        (self.right - self.left).max(0) as usize
    }

    pub fn height(self) -> usize {
        (self.bottom - self.top).max(0) as usize
    }
}

/// Blends a straight-alpha unit colour over one pixel.
fn blend<S: Sample>(pixel: &mut [S], color: [f32; 4], coverage: f32) {
    let source_alpha = color[3] * coverage;
    if source_alpha <= 0.0 {
        return;
    }
    let target_alpha = pixel[3].to_unit();
    let rest = target_alpha * (1.0 - source_alpha);
    let alpha = source_alpha + rest;
    if alpha <= 0.0 {
        return;
    }
    for channel in 0..3 {
        let mixed = (color[channel] * source_alpha + pixel[channel].to_unit() * rest) / alpha;
        pixel[channel] = S::from_unit(mixed);
    }
    pixel[3] = S::from_unit(alpha);
}

fn paint_samples<S, F>(data: &mut [S], width: usize, bounds: Bounds, color: [f32; 4], coverage: &F)
where
    S: Sample,
    F: Fn(f32, f32) -> f32 + Sync,
{
    let stride = width * CHANNELS;
    let top = bounds.top as usize;
    let bottom = bounds.bottom as usize;
    let left = bounds.left as usize;
    let right = bounds.right as usize;
    for_each_row(
        &mut data[top * stride..bottom * stride],
        stride,
        |index, row| {
            let y = (top + index) as f32 + 0.5;
            for x in left..right {
                let amount = coverage(x as f32 + 0.5, y);
                if amount > 0.0 {
                    let pixel = &mut row[x * CHANNELS..(x + 1) * CHANNELS];
                    blend(pixel, color, amount.min(1.0));
                }
            }
        },
    );
}

/// Paints `color` inside `bounds`. `coverage` receives the centre of a pixel
/// and returns how much of it the shape covers, from `0.0` to `1.0`.
pub fn paint<F>(image: &mut Image, bounds: Bounds, color: Color, opacity: f32, coverage: F)
where
    F: Fn(f32, f32) -> f32 + Sync,
{
    let Some(bounds) = bounds.clip(image.width, image.height) else {
        return;
    };
    let mut unit = color.to_unit();
    unit[3] *= opacity.clamp(0.0, 1.0);
    if unit[3] <= 0.0 {
        return;
    }
    let width = image.width as usize;
    match &mut image.pixels {
        Pixels::U8(data) => paint_samples(data, width, bounds, unit, &coverage),
        Pixels::U16(data) => paint_samples(data, width, bounds, unit, &coverage),
    }
}

fn composite_samples<D: Sample, S: Sample>(
    target: &mut [D],
    target_width: usize,
    source: &[S],
    source_width: usize,
    bounds: Bounds,
    left: i64,
    top: i64,
    opacity: f32,
) {
    let stride = target_width * CHANNELS;
    let first_row = bounds.top as usize;
    let last_row = bounds.bottom as usize;
    for_each_row(
        &mut target[first_row * stride..last_row * stride],
        stride,
        |index, row| {
            let source_y = ((first_row + index) as i64 - top) as usize;
            for x in bounds.left as usize..bounds.right as usize {
                let source_x = (x as i64 - left) as usize;
                let at = (source_y * source_width + source_x) * CHANNELS;
                let color = [
                    source[at].to_unit(),
                    source[at + 1].to_unit(),
                    source[at + 2].to_unit(),
                    source[at + 3].to_unit(),
                ];
                blend(&mut row[x * CHANNELS..(x + 1) * CHANNELS], color, opacity);
            }
        },
    );
}

/// Draws `layer` over `image` with its top-left corner at `(left, top)`.
/// The layer may be of another bit depth and may hang over the edges.
pub fn composite(image: &mut Image, layer: &Image, left: i64, top: i64, opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }
    let bounds = Bounds {
        left,
        top,
        right: left + i64::from(layer.width),
        bottom: top + i64::from(layer.height),
    };
    let Some(bounds) = bounds.clip(image.width, image.height) else {
        return;
    };
    let width = image.width as usize;
    let layer_width = layer.width as usize;
    match (&mut image.pixels, &layer.pixels) {
        (Pixels::U8(target), Pixels::U8(source)) => {
            composite_samples(
                target,
                width,
                source,
                layer_width,
                bounds,
                left,
                top,
                opacity,
            );
        }
        (Pixels::U8(target), Pixels::U16(source)) => {
            composite_samples(
                target,
                width,
                source,
                layer_width,
                bounds,
                left,
                top,
                opacity,
            );
        }
        (Pixels::U16(target), Pixels::U8(source)) => {
            composite_samples(
                target,
                width,
                source,
                layer_width,
                bounds,
                left,
                top,
                opacity,
            );
        }
        (Pixels::U16(target), Pixels::U16(source)) => {
            composite_samples(
                target,
                width,
                source,
                layer_width,
                bounds,
                left,
                top,
                opacity,
            );
        }
    }
}

fn paste_samples<S: Sample>(
    target: &mut [S],
    target_width: usize,
    source: &[S],
    source_width: usize,
    bounds: Bounds,
    left: i64,
    top: i64,
) {
    let row_len = bounds.width() * CHANNELS;
    for y in bounds.top..bounds.bottom {
        let source_y = (y - top) as usize;
        let source_x = (bounds.left - left) as usize;
        let from = (source_y * source_width + source_x) * CHANNELS;
        let to = (y as usize * target_width + bounds.left as usize) * CHANNELS;
        target[to..to + row_len].copy_from_slice(&source[from..from + row_len]);
    }
}

/// Replaces the pixels of `image` by those of `patch`, placed with its
/// top-left corner at `(left, top)`.
pub fn paste(image: &mut Image, patch: &Image, left: i64, top: i64) {
    let bounds = Bounds {
        left,
        top,
        right: left + i64::from(patch.width),
        bottom: top + i64::from(patch.height),
    };
    let Some(bounds) = bounds.clip(image.width, image.height) else {
        return;
    };
    let width = image.width as usize;
    let patch_width = patch.width as usize;
    image.alpha |= patch.alpha;
    match (&mut image.pixels, &patch.pixels) {
        (Pixels::U8(target), Pixels::U8(source)) => {
            paste_samples(target, width, source, patch_width, bounds, left, top);
        }
        (Pixels::U16(target), Pixels::U16(source)) => {
            paste_samples(target, width, source, patch_width, bounds, left, top);
        }
        (Pixels::U8(target), Pixels::U16(_)) => {
            let source = patch.to_rgba8();
            paste_samples(target, width, &source, patch_width, bounds, left, top);
        }
        (Pixels::U16(target), Pixels::U8(_)) => {
            let source = patch.to_rgba16();
            paste_samples(target, width, &source, patch_width, bounds, left, top);
        }
    }
}

/// Coverage of a pixel whose centre lies `distance` pixels outside (positive)
/// or inside (negative) the edge of a shape.
pub fn edge_coverage(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}

/// Distance from a point to the segment `a`-`b`.
pub fn segment_distance(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (abx, aby) = (bx - ax, by - ay);
    let (apx, apy) = (px - ax, py - ay);
    let length_squared = abx * abx + aby * aby;
    let t = if length_squared > 0.0 {
        ((apx * abx + apy * aby) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (dx, dy) = (apx - abx * t, apy - aby * t);
    (dx * dx + dy * dy).sqrt()
}

/// Signed distance to an axis-aligned box given by its centre and half sizes.
pub fn box_distance(px: f32, py: f32, cx: f32, cy: f32, half_w: f32, half_h: f32) -> f32 {
    let dx = (px - cx).abs() - half_w;
    let dy = (py - cy).abs() - half_h;
    let outside = (dx.max(0.0) * dx.max(0.0) + dy.max(0.0) * dy.max(0.0)).sqrt();
    outside + dx.max(dy).min(0.0)
}

/// Approximate signed distance to an axis-aligned ellipse.
pub fn ellipse_distance(px: f32, py: f32, cx: f32, cy: f32, radius_x: f32, radius_y: f32) -> f32 {
    let radius_x = radius_x.max(0.5);
    let radius_y = radius_y.max(0.5);
    let (x, y) = (px - cx, py - cy);
    let (kx, ky) = (x / radius_x, y / radius_y);
    let length = (kx * kx + ky * ky).sqrt();
    if length < 1e-6 {
        return -radius_x.min(radius_y);
    }
    let (gx, gy) = (kx / radius_x / length, ky / radius_y / length);
    let gradient = (gx * gx + gy * gy).sqrt();
    (length - 1.0) / gradient
}

/// Signed distance to a triangle: negative inside.
pub fn triangle_distance(px: f32, py: f32, corners: [(f32, f32); 3]) -> f32 {
    let [(ax, ay), (bx, by), (cx, cy)] = corners;
    let winding = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
    let sign = if winding >= 0.0 { 1.0 } else { -1.0 };
    let mut farthest = f32::NEG_INFINITY;
    for ((x1, y1), (x2, y2)) in [
        ((ax, ay), (bx, by)),
        ((bx, by), (cx, cy)),
        ((cx, cy), (ax, ay)),
    ] {
        let (ex, ey) = (x2 - x1, y2 - y1);
        let length = (ex * ex + ey * ey).sqrt();
        if length <= 0.0 {
            continue;
        }
        // Distance to the edge line, positive on the outer side.
        let distance = sign * ((px - x1) * ey - (py - y1) * ex) / length;
        farthest = farthest.max(distance);
    }
    if farthest == f32::NEG_INFINITY {
        f32::INFINITY
    } else {
        farthest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Depth;

    #[test]
    fn bounds_clip_to_the_image() {
        let bounds = Bounds::around(-5.0, 2.0, 3.0, 50.0, 1.0);
        let clipped = bounds.clip(10, 10).unwrap();
        assert_eq!(
            clipped,
            Bounds {
                left: 0,
                top: 0,
                right: 5,
                bottom: 10
            }
        );
        assert_eq!((clipped.width(), clipped.height()), (5, 10));
        assert!(Bounds::around(20.0, 20.0, 30.0, 30.0, 0.0)
            .clip(10, 10)
            .is_none());
    }

    #[test]
    fn paint_blends_by_coverage_and_opacity() {
        let mut image = Image::new(4, 4, Depth::Eight, Color::BLACK).unwrap();
        let whole = Bounds {
            left: 0,
            top: 0,
            right: 4,
            bottom: 4,
        };
        paint(&mut image, whole, Color::WHITE, 0.5, |x, _| {
            if x < 2.0 {
                1.0
            } else {
                0.0
            }
        });
        assert_eq!(image.pixel8(0, 0), [128, 128, 128, 255]);
        assert_eq!(image.pixel8(3, 3), [0, 0, 0, 255]);
        // Regions outside the image are ignored.
        paint(
            &mut image,
            Bounds {
                left: -10,
                top: -10,
                right: 100,
                bottom: 100,
            },
            Color::rgb(255, 0, 0),
            1.0,
            |_, _| 1.0,
        );
        assert_eq!(image.pixel8(3, 3), [255, 0, 0, 255]);
    }

    #[test]
    fn painting_on_transparency_keeps_the_colour() {
        let mut image = Image::new(2, 2, Depth::Sixteen, Color::TRANSPARENT).unwrap();
        let whole = Bounds {
            left: 0,
            top: 0,
            right: 2,
            bottom: 2,
        };
        paint(&mut image, whole, Color::rgb(10, 20, 30), 0.5, |_, _| 1.0);
        assert_eq!(image.pixel8(0, 0), [10, 20, 30, 128]);
    }

    #[test]
    fn composite_handles_offsets_and_depths() {
        let layer = Image::new(2, 2, Depth::Eight, Color::rgb(255, 0, 0)).unwrap();
        for depth in [Depth::Eight, Depth::Sixteen] {
            let mut image = Image::new(4, 4, depth, Color::rgb(0, 0, 255)).unwrap();
            composite(&mut image, &layer, 3, 3, 1.0);
            composite(&mut image, &layer, -1, -1, 0.5);
            composite(&mut image, &layer, 10, 10, 1.0);
            assert_eq!(image.pixel8(3, 3), [255, 0, 0, 255]);
            assert_eq!(image.pixel8(0, 0), [128, 0, 128, 255]);
            assert_eq!(image.pixel8(1, 1), [0, 0, 255, 255]);
        }
    }

    #[test]
    fn paste_replaces_pixels_including_alpha() {
        let mut image = Image::new(3, 3, Depth::Eight, Color::WHITE).unwrap();
        let patch = Image::new(2, 2, Depth::Sixteen, Color::TRANSPARENT).unwrap();
        paste(&mut image, &patch, 2, 2);
        assert_eq!(image.pixel8(2, 2), [0, 0, 0, 0]);
        assert_eq!(image.pixel8(1, 1), [255, 255, 255, 255]);
    }

    #[test]
    fn distances() {
        assert!((segment_distance(0.0, 5.0, -10.0, 0.0, 10.0, 0.0) - 5.0).abs() < 1e-5);
        assert!((segment_distance(13.0, 4.0, -10.0, 0.0, 10.0, 0.0) - 5.0).abs() < 1e-5);
        assert!((segment_distance(3.0, 4.0, 0.0, 0.0, 0.0, 0.0) - 5.0).abs() < 1e-5);

        assert!((box_distance(0.0, 0.0, 0.0, 0.0, 4.0, 2.0) + 2.0).abs() < 1e-5);
        assert!((box_distance(7.0, 6.0, 0.0, 0.0, 4.0, 2.0) - 5.0).abs() < 1e-5);

        assert!(ellipse_distance(0.0, 0.0, 0.0, 0.0, 10.0, 5.0) < 0.0);
        assert!(ellipse_distance(10.0, 0.0, 0.0, 0.0, 10.0, 5.0).abs() < 1e-4);
        assert!((ellipse_distance(0.0, 8.0, 0.0, 0.0, 10.0, 5.0) - 3.0).abs() < 1e-3);

        for corners in [
            [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)],
            [(0.0, 0.0), (0.0, 10.0), (10.0, 0.0)],
        ] {
            assert!(triangle_distance(2.0, 2.0, corners) < 0.0);
            assert!(triangle_distance(-3.0, 5.0, corners) > 2.9);
            assert!(triangle_distance(9.0, 9.0, corners) > 0.0);
        }
        assert_eq!(edge_coverage(0.0), 0.5);
        assert_eq!(edge_coverage(-2.0), 1.0);
        assert_eq!(edge_coverage(2.0), 0.0);
    }
}
