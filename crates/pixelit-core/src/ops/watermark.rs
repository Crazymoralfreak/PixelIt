//! Text and picture watermarks.

use crate::color::Color;
use crate::error::Result;
use crate::image::Image;
use crate::ops::draw::composite;
use crate::ops::geometry::rotate_by;
use crate::ops::resize::resize_to;
use crate::pipeline::{ResizeFilter, Resources, Watermark, WatermarkKind};
use crate::text::render_text;

/// Largest number of copies a tiled watermark draws.
const MAX_TILES: usize = 20_000;

fn percent(value: f32, low: f32, high: f32) -> f32 {
    if value.is_finite() {
        value.clamp(low, high) / 100.0
    } else {
        0.0
    }
}

/// Builds the watermark as a separate RGBA layer.
fn layer(image: &Image, spec: &Watermark, resources: &dyn Resources) -> Result<Option<Image>> {
    let short_side = image.width.min(image.height) as f32;
    match spec.kind {
        WatermarkKind::Text => {
            if spec.text.trim().is_empty() {
                return Ok(None);
            }
            let font = resources.font(&spec.font)?;
            let size = (percent(spec.size, 0.1, 100.0) * short_side).max(4.0);
            let Some(mask) = render_text(&font, &spec.text, size) else {
                return Ok(None);
            };
            let Color { r, g, b, a } = spec.color;
            let mut data = Vec::with_capacity(mask.coverage.len() * 4);
            for coverage in &mask.coverage {
                let alpha = (coverage * f32::from(a) + 0.5) as u8;
                data.extend_from_slice(&[r, g, b, alpha]);
            }
            Ok(Some(Image::from_rgba8(mask.width, mask.height, data)?))
        }
        WatermarkKind::Image => {
            if spec.image.trim().is_empty() {
                return Ok(None);
            }
            let picture = resources.image(&spec.image)?;
            let width = (percent(spec.scale, 0.1, 100.0) * image.width as f32)
                .round()
                .max(1.0);
            let factor = width / picture.width as f32;
            let height = (picture.height as f32 * factor).round().max(1.0);
            let resized = resize_to(
                &picture,
                width as u32,
                height as u32,
                ResizeFilter::Lanczos3,
                false,
                None,
            )?;
            Ok(Some(resized))
        }
    }
}

/// Applies a [`Watermark`] step in place.
pub fn watermark(image: &mut Image, spec: &Watermark, resources: &dyn Resources) -> Result<()> {
    let opacity = percent(spec.opacity, 0.0, 100.0);
    if opacity <= 0.0 {
        return Ok(());
    }
    let Some(mut mark) = layer(image, spec, resources)? else {
        return Ok(());
    };
    if spec.rotation.is_finite() && spec.rotation.rem_euclid(360.0) != 0.0 {
        mark = rotate_by(mark, spec.rotation, true, Color::TRANSPARENT)?;
    }

    let width = image.width as f32;
    let height = image.height as f32;
    let mark_width = mark.width as f32;
    let mark_height = mark.height as f32;

    if spec.tile {
        let gap = 1.0 + percent(spec.tile_gap, 0.0, 1000.0);
        let step_x = (mark_width * gap).max(8.0);
        let step_y = (mark_height * gap).max(8.0);
        let mut drawn = 0usize;
        let mut row = 0usize;
        let mut y = -mark_height / 2.0;
        while y < height && drawn < MAX_TILES {
            // Every other row is shifted by half a step.
            let mut x = if row.is_multiple_of(2) {
                -mark_width / 2.0
            } else {
                -mark_width / 2.0 - step_x / 2.0
            };
            while x < width && drawn < MAX_TILES {
                composite(image, &mark, x.round() as i64, y.round() as i64, opacity);
                drawn += 1;
                x += step_x;
            }
            y += step_y;
            row += 1;
        }
    } else {
        let margin = percent(spec.margin, 0.0, 50.0) * width.min(height);
        let (anchor_x, anchor_y) = spec.anchor.fractions();
        let x = margin + (width - 2.0 * margin - mark_width) * anchor_x;
        let y = margin + (height - 2.0 * margin - mark_height) * anchor_y;
        composite(image, &mark, x.round() as i64, y.round() as i64, opacity);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::image::Depth;
    use crate::pipeline::Anchor;
    use crate::text::Font;

    /// Serves one solid picture for any path and no fonts.
    struct Pictures(Image);

    impl Resources for Pictures {
        fn font(&self, name: &str) -> Result<Font> {
            Err(Error::new("fontNotFound").with("name", name))
        }

        fn image(&self, _path: &str) -> Result<Image> {
            Ok(self.0.clone())
        }
    }

    fn red() -> Pictures {
        Pictures(Image::new(40, 20, Depth::Eight, Color::rgb(255, 0, 0)).unwrap())
    }

    fn picture_mark() -> Watermark {
        Watermark {
            kind: WatermarkKind::Image,
            image: String::from("logo.png"),
            scale: 20.0,
            margin: 0.0,
            opacity: 100.0,
            anchor: Anchor::BottomRight,
            ..Watermark::default()
        }
    }

    #[test]
    fn picture_is_scaled_and_anchored() {
        let mut image = Image::new(200, 100, Depth::Eight, Color::WHITE).unwrap();
        watermark(&mut image, &picture_mark(), &red()).unwrap();
        // 20% of 200 px is 40 px wide and 20 px tall, in the bottom-right corner.
        assert_eq!(image.pixel8(199, 99), [255, 0, 0, 255]);
        assert_eq!(image.pixel8(161, 81), [255, 0, 0, 255]);
        assert_eq!(image.pixel8(158, 78), [255, 255, 255, 255]);
        assert_eq!(image.pixel8(0, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn margin_and_anchor_move_the_mark() {
        let mut image = Image::new(200, 100, Depth::Sixteen, Color::WHITE).unwrap();
        let spec = Watermark {
            anchor: Anchor::TopLeft,
            margin: 10.0,
            ..picture_mark()
        };
        watermark(&mut image, &spec, &red()).unwrap();
        assert_eq!(image.pixel8(5, 5), [255, 255, 255, 255]);
        assert_eq!(image.pixel8(12, 12), [255, 0, 0, 255]);
        assert_eq!(image.pixel8(55, 12), [255, 255, 255, 255]);
    }

    #[test]
    fn opacity_blends() {
        let mut image = Image::new(200, 100, Depth::Eight, Color::WHITE).unwrap();
        let spec = Watermark {
            opacity: 50.0,
            ..picture_mark()
        };
        watermark(&mut image, &spec, &red()).unwrap();
        let [r, g, b, _] = image.pixel8(190, 95);
        assert_eq!(r, 255);
        assert!((i32::from(g) - 128).abs() <= 1 && g == b);

        let mut untouched = Image::new(20, 20, Depth::Eight, Color::WHITE).unwrap();
        let before = untouched.clone();
        let spec = Watermark {
            opacity: 0.0,
            ..picture_mark()
        };
        watermark(&mut untouched, &spec, &red()).unwrap();
        assert_eq!(untouched, before);
    }

    #[test]
    fn tiling_covers_the_whole_image() {
        let mut image = Image::new(300, 300, Depth::Eight, Color::WHITE).unwrap();
        let spec = Watermark {
            tile: true,
            tile_gap: 100.0,
            scale: 10.0,
            ..picture_mark()
        };
        watermark(&mut image, &spec, &red()).unwrap();
        let mut marked = [false; 9];
        for y in 0..300u32 {
            for x in 0..300u32 {
                let [r, g, b, _] = image.pixel8(x, y);
                if r > 250 && g < 5 && b < 5 {
                    marked[(y / 100 * 3 + x / 100) as usize] = true;
                }
            }
        }
        assert!(marked.iter().all(|&cell| cell));
        assert_eq!(image.pixel8(299, 149), [255, 255, 255, 255]);
    }

    #[test]
    fn rotation_grows_the_layer() {
        let mut image = Image::new(200, 200, Depth::Eight, Color::WHITE).unwrap();
        let spec = Watermark {
            rotation: 90.0,
            anchor: Anchor::TopLeft,
            ..picture_mark()
        };
        watermark(&mut image, &spec, &red()).unwrap();
        // The 40x20 mark stands upright: 20 wide, 40 tall.
        assert_eq!(image.pixel8(10, 35), [255, 0, 0, 255]);
        assert_eq!(image.pixel8(30, 10), [255, 255, 255, 255]);
    }

    #[test]
    fn empty_marks_are_skipped() {
        let mut image = Image::new(50, 50, Depth::Eight, Color::WHITE).unwrap();
        let before = image.clone();
        let text = Watermark {
            text: String::from("   "),
            ..Watermark::default()
        };
        watermark(&mut image, &text, &red()).unwrap();
        let picture = Watermark {
            image: String::new(),
            ..picture_mark()
        };
        watermark(&mut image, &picture, &red()).unwrap();
        assert_eq!(image, before);

        let needs_font = Watermark::default();
        let error = watermark(&mut image, &needs_font, &red()).unwrap_err();
        assert_eq!(error.code, "fontNotFound");
    }
}
