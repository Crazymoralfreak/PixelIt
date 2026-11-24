//! Text rasterization for watermarks and annotations.

use ab_glyph::{point, Font as _, FontArc, FontVec, GlyphId, PxScale, ScaleFont as _};

use crate::error::{Error, Result};

/// A loaded TrueType or OpenType font.
pub type Font = FontArc;

/// Largest text height, in pixels.
const MAX_TEXT_HEIGHT: f32 = 4000.0;

/// Largest side of a rendered text block, in pixels.
const MAX_MASK_SIDE: u32 = 16_384;

/// Largest area of a rendered text block, in pixels.
const MAX_MASK_AREA: u64 = 64_000_000;

/// Parses font data.
pub fn load_font(data: Vec<u8>) -> Result<Font> {
    FontArc::try_from_vec(data).map_err(|_| Error::new("badFont"))
}

/// Loads one face of a font collection (`.ttc`) by its index.
pub fn load_font_face(data: Vec<u8>, index: u32) -> Result<Font> {
    FontVec::try_from_vec_and_index(data, index)
        .map(FontArc::new)
        .map_err(|_| Error::new("badFont"))
}

/// Rendered text: how much of each pixel the glyphs cover, from `0.0` to `1.0`.
#[derive(Clone, Debug, PartialEq)]
pub struct TextMask {
    pub width: u32,
    pub height: u32,
    pub coverage: Vec<f32>,
}

impl TextMask {
    /// Coverage at a pixel; `0.0` outside the mask.
    pub fn at(&self, x: i64, y: i64) -> f32 {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return 0.0;
        }
        self.coverage[y as usize * self.width as usize + x as usize]
    }
}

/// Renders `text` with lines `height` pixels tall. Returns `None` when there is
/// nothing to draw or the block would be unreasonably large.
pub fn render_text(font: &Font, text: &str, height: f32) -> Option<TextMask> {
    if !height.is_finite() || text.trim().is_empty() {
        return None;
    }
    let size = height.clamp(1.0, MAX_TEXT_HEIGHT);
    let scale = PxScale::from(size);
    let scaled = font.as_scaled(scale);
    let ascent = scaled.ascent();
    let descent = scaled.descent();
    let line_gap = scaled.line_gap();
    let line_height = ascent - descent + line_gap;
    // Room for glyphs that reach beyond their advance, such as italics.
    let padding = (size * 0.15).ceil();

    let mut glyphs = Vec::new();
    let mut block_width = 0.0f32;
    let mut line_count = 0usize;
    for line in text.lines() {
        let baseline = padding + ascent + line_count as f32 * line_height;
        let mut pen = padding;
        let mut previous: Option<GlyphId> = None;
        for character in line.chars().filter(|character| !character.is_control()) {
            let id = scaled.glyph_id(character);
            if let Some(previous) = previous {
                pen += scaled.kern(previous, id);
            }
            glyphs.push(id.with_scale_and_position(scale, point(pen, baseline)));
            pen += scaled.h_advance(id);
            previous = Some(id);
        }
        block_width = block_width.max(pen + padding);
        line_count += 1;
    }
    if glyphs.is_empty() {
        return None;
    }

    let block_height = line_count as f32 * line_height - line_gap + 2.0 * padding;
    let width = block_width.ceil().max(1.0);
    let height = block_height.ceil().max(1.0);
    if width > MAX_MASK_SIDE as f32 || height > MAX_MASK_SIDE as f32 {
        return None;
    }
    let (width, height) = (width as u32, height as u32);
    if u64::from(width) * u64::from(height) > MAX_MASK_AREA {
        return None;
    }

    let mut coverage = vec![0.0f32; width as usize * height as usize];
    for glyph in glyphs {
        let Some(outlined) = font.outline_glyph(glyph) else {
            continue;
        };
        let bounds = outlined.px_bounds();
        let left = bounds.min.x as i64;
        let top = bounds.min.y as i64;
        outlined.draw(|x, y, value| {
            let column = left + i64::from(x);
            let row = top + i64::from(y);
            if column >= 0 && row >= 0 && column < i64::from(width) && row < i64::from(height) {
                let slot = &mut coverage[row as usize * width as usize + column as usize];
                *slot = (*slot + value).min(1.0);
            }
        });
    }
    Some(TextMask {
        width,
        height,
        coverage,
    })
}
