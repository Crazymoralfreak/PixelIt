//! Annotations: arrows, lines, boxes, ellipses, freehand lines, text,
//! highlighting, blurred and pixelated regions.

use crate::error::Result;
use crate::image::{Image, Pixels, Sample, CHANNELS};
use crate::ops::draw::{
    box_distance, edge_coverage, ellipse_distance, paint, paste, segment_distance,
    triangle_distance, Bounds,
};
use crate::ops::filter::gaussian_blur;
use crate::ops::geometry::{crop_pixels, PixelRect};
use crate::pipeline::{Annotate, Mask, Resources, Shape, ShapeKind, Stroke};
use crate::text::render_text;

/// Opacity of the highlighter.
const HIGHLIGHT_OPACITY: f32 = 0.4;

/// Largest number of points of one freehand line that are drawn.
const MAX_POINTS: usize = 20_000;

fn finite(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(-4.0, 5.0)
    } else {
        0.0
    }
}

/// Stroke thickness in pixels for a percentage of the short side.
fn thickness(percent: f32, short_side: f32) -> f32 {
    let percent = if percent.is_finite() {
        percent.clamp(0.0, 50.0)
    } else {
        0.0
    };
    (percent / 100.0 * short_side).max(1.0)
}

/// A coverage buffer for shapes made of many overlapping parts, so that the
/// overlaps are painted once.
struct Coverage {
    bounds: Bounds,
    width: usize,
    values: Vec<f32>,
}

impl Coverage {
    fn new(bounds: Bounds) -> Self {
        let width = bounds.width();
        Self {
            bounds,
            width,
            values: vec![0.0; width * bounds.height()],
        }
    }

    /// Raises the coverage of the pixels around a thick segment.
    fn add_segment(&mut self, ax: f32, ay: f32, bx: f32, by: f32, half: f32) {
        let local = Bounds::around(ax, ay, bx, by, half);
        let left = local.left.max(self.bounds.left);
        let right = local.right.min(self.bounds.right);
        let top = local.top.max(self.bounds.top);
        let bottom = local.bottom.min(self.bounds.bottom);
        for y in top..bottom {
            let row = (y - self.bounds.top) as usize * self.width;
            for x in left..right {
                let distance =
                    segment_distance(x as f32 + 0.5, y as f32 + 0.5, ax, ay, bx, by) - half;
                let value = edge_coverage(distance);
                let slot = &mut self.values[row + (x - self.bounds.left) as usize];
                if value > *slot {
                    *slot = value;
                }
            }
        }
    }

    fn at(&self, x: f32, y: f32) -> f32 {
        let column = x.floor() as i64 - self.bounds.left;
        let row = y.floor() as i64 - self.bounds.top;
        if column < 0 || row < 0 || column >= self.width as i64 {
            return 0.0;
        }
        self.values
            .get(row as usize * self.width + column as usize)
            .copied()
            .unwrap_or(0.0)
    }
}

/// Pixel positions of the points of a freehand line.
fn polyline(points: &[[f32; 2]], width: f32, height: f32) -> Vec<(f32, f32)> {
    points
        .iter()
        .take(MAX_POINTS)
        .map(|point| (finite(point[0]) * width, finite(point[1]) * height))
        .collect()
}

/// Coverage of a thick polyline, clipped to the image.
fn polyline_coverage(
    points: &[(f32, f32)],
    half: f32,
    image_width: u32,
    image_height: u32,
) -> Option<Coverage> {
    let first = points.first()?;
    let mut bounds = Bounds::around(first.0, first.1, first.0, first.1, half);
    for &(x, y) in points {
        let around = Bounds::around(x, y, x, y, half);
        bounds.left = bounds.left.min(around.left);
        bounds.top = bounds.top.min(around.top);
        bounds.right = bounds.right.max(around.right);
        bounds.bottom = bounds.bottom.max(around.bottom);
    }
    let bounds = bounds.clip(image_width, image_height)?;
    let mut coverage = Coverage::new(bounds);
    if points.len() == 1 {
        coverage.add_segment(first.0, first.1, first.0, first.1, half);
    }
    for pair in points.windows(2) {
        coverage.add_segment(pair[0].0, pair[0].1, pair[1].0, pair[1].1, half);
    }
    Some(coverage)
}

/// The pixel rectangle between two corners, clipped to the image.
fn region(image: &Image, ax: f32, ay: f32, bx: f32, by: f32) -> Option<PixelRect> {
    let bounds = Bounds {
        left: ax.min(bx).round() as i64,
        top: ay.min(by).round() as i64,
        right: ax.max(bx).round() as i64,
        bottom: ay.max(by).round() as i64,
    }
    .clip(image.width, image.height)?;
    Some(PixelRect {
        x: bounds.left as u32,
        y: bounds.top as u32,
        width: bounds.width() as u32,
        height: bounds.height() as u32,
    })
}

fn pixelate_samples<S: Sample>(data: &mut [S], width: usize, rect: PixelRect, block: usize) {
    let (left, top) = (rect.x as usize, rect.y as usize);
    let (right, bottom) = (left + rect.width as usize, top + rect.height as usize);
    let mut block_top = top;
    while block_top < bottom {
        let block_bottom = (block_top + block).min(bottom);
        let mut block_left = left;
        while block_left < right {
            let block_right = (block_left + block).min(right);
            let mut sums = [0u64; CHANNELS];
            for y in block_top..block_bottom {
                for x in block_left..block_right {
                    let at = (y * width + x) * CHANNELS;
                    for channel in 0..CHANNELS {
                        sums[channel] += u64::from(data[at + channel].to_u32());
                    }
                }
            }
            let count = ((block_bottom - block_top) * (block_right - block_left)) as u64;
            let mean = sums.map(|sum| S::from_u32(((sum + count / 2) / count) as u32));
            for y in block_top..block_bottom {
                for x in block_left..block_right {
                    let at = (y * width + x) * CHANNELS;
                    data[at..at + CHANNELS].copy_from_slice(&mean);
                }
            }
            block_left = block_right;
        }
        block_top = block_bottom;
    }
}

fn draw_shape(image: &mut Image, shape: &Shape, resources: &dyn Resources) -> Result<()> {
    let width = image.width as f32;
    let height = image.height as f32;
    let short_side = width.min(height);
    let (ax, ay) = (finite(shape.x1) * width, finite(shape.y1) * height);
    let (bx, by) = (finite(shape.x2) * width, finite(shape.y2) * height);
    let stroke = thickness(shape.width, short_side);
    let half = stroke / 2.0;
    let color = shape.color;

    match shape.kind {
        ShapeKind::Line => {
            let bounds = Bounds::around(ax, ay, bx, by, half);
            paint(image, bounds, color, 1.0, |x, y| {
                edge_coverage(segment_distance(x, y, ax, ay, bx, by) - half)
            });
        }
        ShapeKind::Arrow => {
            let (dx, dy) = (bx - ax, by - ay);
            let length = (dx * dx + dy * dy).sqrt();
            if length < 0.5 {
                return Ok(());
            }
            let (ux, uy) = (dx / length, dy / length);
            let head = (stroke * 5.0).min(length * 0.6).max(2.0);
            let head_half = head * 0.5;
            let (base_x, base_y) = (bx - ux * head, by - uy * head);
            let corners = [
                (bx, by),
                (base_x - uy * head_half, base_y + ux * head_half),
                (base_x + uy * head_half, base_y - ux * head_half),
            ];
            // The shaft ends inside the head so its blunt end stays hidden.
            let (end_x, end_y) = (bx - ux * head * 0.8, by - uy * head * 0.8);
            let bounds = Bounds::around(ax, ay, bx, by, half.max(head_half));
            paint(image, bounds, color, 1.0, |x, y| {
                let shaft = edge_coverage(segment_distance(x, y, ax, ay, end_x, end_y) - half);
                let tip = edge_coverage(triangle_distance(x, y, corners));
                shaft.max(tip)
            });
        }
        ShapeKind::Rect | ShapeKind::Highlight => {
            let (cx, cy) = (f32::midpoint(ax, bx), f32::midpoint(ay, by));
            let (half_w, half_h) = ((bx - ax).abs() / 2.0, (by - ay).abs() / 2.0);
            let filled = shape.fill || shape.kind == ShapeKind::Highlight;
            let opacity = if shape.kind == ShapeKind::Highlight {
                HIGHLIGHT_OPACITY
            } else {
                1.0
            };
            let bounds = Bounds::around(ax, ay, bx, by, half);
            paint(image, bounds, color, opacity, |x, y| {
                let distance = box_distance(x, y, cx, cy, half_w, half_h);
                if filled {
                    edge_coverage(distance)
                } else {
                    edge_coverage(distance.abs() - half)
                }
            });
        }
        ShapeKind::Ellipse => {
            let (cx, cy) = (f32::midpoint(ax, bx), f32::midpoint(ay, by));
            let (radius_x, radius_y) = ((bx - ax).abs() / 2.0, (by - ay).abs() / 2.0);
            let filled = shape.fill;
            let bounds = Bounds::around(ax, ay, bx, by, half);
            paint(image, bounds, color, 1.0, |x, y| {
                let distance = ellipse_distance(x, y, cx, cy, radius_x, radius_y);
                if filled {
                    edge_coverage(distance)
                } else {
                    edge_coverage(distance.abs() - half)
                }
            });
        }
        ShapeKind::Pen => {
            let points = polyline(&shape.points, width, height);
            if let Some(coverage) = polyline_coverage(&points, half, image.width, image.height) {
                paint(image, coverage.bounds, color, 1.0, |x, y| coverage.at(x, y));
            }
        }
        ShapeKind::Text => {
            if shape.text.trim().is_empty() {
                return Ok(());
            }
            let font = resources.font(&shape.font)?;
            let size = thickness(shape.size, short_side).max(4.0);
            if let Some(mask) = render_text(&font, &shape.text, size) {
                let (left, top) = (ax.round() as i64, ay.round() as i64);
                let bounds = Bounds {
                    left,
                    top,
                    right: left + i64::from(mask.width),
                    bottom: top + i64::from(mask.height),
                };
                paint(image, bounds, color, 1.0, |x, y| {
                    mask.at(x.floor() as i64 - left, y.floor() as i64 - top)
                });
            }
        }
        ShapeKind::Blur => {
            if let Some(rect) = region(image, ax, ay, bx, by) {
                let mut patch = crop_pixels(image, rect)?;
                gaussian_blur(&mut patch, (short_side * 0.012).max(3.0));
                paste(image, &patch, i64::from(rect.x), i64::from(rect.y));
            }
        }
        ShapeKind::Pixelate => {
            if let Some(rect) = region(image, ax, ay, bx, by) {
                let block = ((short_side * 0.02) as usize).max(4);
                let image_width = image.width as usize;
                match &mut image.pixels {
                    Pixels::U8(data) => pixelate_samples(data, image_width, rect, block),
                    Pixels::U16(data) => pixelate_samples(data, image_width, rect, block),
                }
            }
        }
    }
    Ok(())
}

/// Applies an [`Annotate`] step in place.
pub fn annotate(image: &mut Image, spec: &Annotate, resources: &dyn Resources) -> Result<()> {
    for shape in &spec.shapes {
        draw_shape(image, shape, resources)?;
    }
    Ok(())
}

/// Rasterizes brush strokes into a mask of the given size.
pub fn stroke_mask(width: u32, height: u32, strokes: &[Stroke]) -> Mask {
    let mut data = vec![0u8; width as usize * height as usize];
    let short_side = width.min(height) as f32;
    for stroke in strokes {
        let half = thickness(stroke.width, short_side) / 2.0;
        let points = polyline(&stroke.points, width as f32, height as f32);
        let Some(coverage) = polyline_coverage(&points, half, width, height) else {
            continue;
        };
        let bounds = coverage.bounds;
        for (index, value) in coverage.values.iter().enumerate() {
            if *value >= 0.5 {
                let x = bounds.left as usize + index % coverage.width;
                let y = bounds.top as usize + index / coverage.width;
                data[y * width as usize + x] = 255;
            }
        }
    }
    Mask {
        width,
        height,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::image::Depth;
    use crate::pipeline::NoResources;

    fn canvas() -> Image {
        Image::new(100, 100, Depth::Eight, Color::WHITE).unwrap()
    }

    fn draw(image: &mut Image, shape: Shape) {
        let spec = Annotate {
            shapes: vec![shape],
        };
        annotate(image, &spec, &NoResources).unwrap();
    }

    fn shape(kind: ShapeKind) -> Shape {
        Shape {
            kind,
            x1: 0.2,
            y1: 0.2,
            x2: 0.8,
            y2: 0.8,
            color: Color::BLACK,
            width: 4.0,
            ..Shape::default()
        }
    }

    #[test]
    fn line_covers_its_path_only() {
        let mut image = canvas();
        draw(&mut image, shape(ShapeKind::Line));
        assert_eq!(image.pixel8(50, 50), [0, 0, 0, 255]);
        assert_eq!(image.pixel8(20, 80), [255, 255, 255, 255]);
        assert_eq!(image.pixel8(95, 95), [255, 255, 255, 255]);
    }

    #[test]
    fn arrow_has_a_wider_head() {
        let mut image = canvas();
        let mut arrow = shape(ShapeKind::Arrow);
        arrow.y1 = 0.5;
        arrow.y2 = 0.5;
        draw(&mut image, arrow);
        // Shaft: 4 px thick around y = 50.
        assert_eq!(image.pixel8(30, 50), [0, 0, 0, 255]);
        assert_eq!(image.pixel8(30, 56), [255, 255, 255, 255]);
        // Head: 20 px long and wide, its base is at x = 60.
        assert_eq!(image.pixel8(63, 56), [0, 0, 0, 255]);
        assert_eq!(image.pixel8(85, 50), [255, 255, 255, 255]);
    }

    #[test]
    fn rect_outline_and_fill() {
        let mut image = canvas();
        draw(&mut image, shape(ShapeKind::Rect));
        assert_eq!(image.pixel8(20, 50), [0, 0, 0, 255]);
        assert_eq!(image.pixel8(50, 50), [255, 255, 255, 255]);

        let mut filled = shape(ShapeKind::Rect);
        filled.fill = true;
        let mut image = canvas();
        draw(&mut image, filled);
        assert_eq!(image.pixel8(50, 50), [0, 0, 0, 255]);
        assert_eq!(image.pixel8(10, 10), [255, 255, 255, 255]);
    }

    #[test]
    fn ellipse_outline_and_fill() {
        let mut image = canvas();
        draw(&mut image, shape(ShapeKind::Ellipse));
        assert_eq!(image.pixel8(50, 20), [0, 0, 0, 255]);
        assert_eq!(image.pixel8(50, 50), [255, 255, 255, 255]);
        assert_eq!(image.pixel8(22, 22), [255, 255, 255, 255]);

        let mut filled = shape(ShapeKind::Ellipse);
        filled.fill = true;
        let mut image = canvas();
        draw(&mut image, filled);
        assert_eq!(image.pixel8(50, 50), [0, 0, 0, 255]);
    }

    #[test]
    fn highlight_tints_without_hiding() {
        let mut image = canvas();
        let mut marker = shape(ShapeKind::Highlight);
        marker.color = Color::rgb(255, 255, 0);
        draw(&mut image, marker);
        let [r, g, b, a] = image.pixel8(50, 50);
        assert_eq!((r, g, a), (255, 255, 255));
        assert!(b > 100 && b < 200);
    }

    #[test]
    fn pen_paints_overlaps_once() {
        let mut image = canvas();
        let mut pen = shape(ShapeKind::Pen);
        pen.color = Color::rgba(0, 0, 0, 128);
        pen.points = vec![[0.2, 0.5], [0.8, 0.5], [0.2, 0.5]];
        draw(&mut image, pen);
        // Two passes over the same pixels give the opacity of one pass.
        let value = image.pixel8(50, 50)[0];
        assert!((i32::from(value) - 127).abs() <= 1, "{value}");

        let mut image = canvas();
        let mut dot = shape(ShapeKind::Pen);
        dot.points = vec![[0.5, 0.5]];
        draw(&mut image, dot);
        assert_eq!(image.pixel8(50, 50), [0, 0, 0, 255]);
        assert_eq!(image.pixel8(60, 50), [255, 255, 255, 255]);
    }

    #[test]
    fn blur_and_pixelate_change_only_their_region() {
        let mut data = Vec::new();
        for y in 0..100u32 {
            for x in 0..100u32 {
                let value = if (x / 2 + y / 2) % 2 == 0 { 255 } else { 0 };
                data.extend_from_slice(&[value, value, value, 255]);
            }
        }
        let source = Image::from_rgba8(100, 100, data).unwrap();
        for kind in [ShapeKind::Blur, ShapeKind::Pixelate] {
            let mut image = source.clone();
            draw(&mut image, shape(kind));
            let inside = image.pixel8(50, 50)[0];
            assert!(inside > 90 && inside < 165, "{kind:?} {inside}");
            assert_eq!(image.pixel8(5, 5), source.pixel8(5, 5));
            assert_eq!(image.pixel8(90, 90), source.pixel8(90, 90));
        }
    }

    #[test]
    fn text_needs_a_font_and_empty_text_is_skipped() {
        let mut image = canvas();
        let mut label = shape(ShapeKind::Text);
        let spec = Annotate {
            shapes: vec![label.clone()],
        };
        assert!(annotate(&mut image, &spec, &NoResources).is_ok());
        label.text = String::from("Hello");
        let spec = Annotate {
            shapes: vec![label],
        };
        let error = annotate(&mut image, &spec, &NoResources).unwrap_err();
        assert_eq!(error.code, "fontNotFound");
    }

    #[test]
    fn shapes_outside_the_image_and_bad_numbers_are_harmless() {
        let mut image = canvas();
        let before = image.clone();
        for kind in [
            ShapeKind::Line,
            ShapeKind::Arrow,
            ShapeKind::Rect,
            ShapeKind::Ellipse,
            ShapeKind::Blur,
            ShapeKind::Pixelate,
            ShapeKind::Highlight,
        ] {
            let mut far = shape(kind);
            far.x1 = 3.0;
            far.y1 = 3.0;
            far.x2 = 4.0;
            far.y2 = 4.0;
            draw(&mut image, far);
            let mut broken = shape(kind);
            broken.x1 = f32::NAN;
            broken.y1 = f32::INFINITY;
            broken.x2 = f32::NAN;
            broken.y2 = f32::NEG_INFINITY;
            broken.width = f32::NAN;
            draw(&mut image, broken);
        }
        assert_eq!(image.pixel8(50, 50), before.pixel8(50, 50));
    }

    #[test]
    fn stroke_mask_marks_the_brush_path() {
        let strokes = [Stroke {
            points: vec![[0.25, 0.5], [0.75, 0.5]],
            width: 10.0,
        }];
        let mask = stroke_mask(200, 100, &strokes);
        assert_eq!(mask.data.len(), 200 * 100);
        assert_eq!(mask.data[50 * 200 + 100], 255);
        assert_eq!(mask.data[50 * 200 + 20], 0);
        assert_eq!(mask.data[10 * 200 + 100], 0);
        let empty = stroke_mask(10, 10, &[]);
        assert!(empty.data.iter().all(|&value| value == 0));
    }
}
