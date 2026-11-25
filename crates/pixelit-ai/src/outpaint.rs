//! Extending a picture past its edges.
//!
//! The new area is filled with the pixels of the nearest edge, and the mask
//! covers that area plus a thin strip of the original so the generator can
//! blend the seam. The caller puts the generated picture back with the mask,
//! which leaves the rest of the original untouched.

use pixelit_core::pipeline::Mask;
use pixelit_core::{Error, Image, Result};
use serde::{Deserialize, Serialize};

/// How far to grow a picture, in pixels, on each side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Expand {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

impl Expand {
    pub const fn is_empty(self) -> bool {
        self.left == 0 && self.right == 0 && self.top == 0 && self.bottom == 0
    }
}

/// Margins that bring a picture to a ratio by growing the short axis, split
/// evenly between the two sides. The sides stay on a grid of 8.
pub fn margins_for_ratio(width: u32, height: u32, ratio_w: u32, ratio_h: u32) -> Expand {
    let ratio_w = u64::from(ratio_w.max(1));
    let ratio_h = u64::from(ratio_h.max(1));
    let width = u64::from(width.max(1));
    let height = u64::from(height.max(1));
    // Compare width/height with ratio_w/ratio_h without division.
    if width * ratio_h >= height * ratio_w {
        let target = snap8(width * ratio_h / ratio_w);
        let extra = target.saturating_sub(height);
        Expand {
            top: (extra / 2) as u32,
            bottom: (extra - extra / 2) as u32,
            ..Expand::default()
        }
    } else {
        let target = snap8(height * ratio_w / ratio_h);
        let extra = target.saturating_sub(width);
        Expand {
            left: (extra / 2) as u32,
            right: (extra - extra / 2) as u32,
            ..Expand::default()
        }
    }
}

fn snap8(value: u64) -> u64 {
    value.div_ceil(8) * 8
}

/// The enlarged picture and the mask of what the generator may change.
pub fn prepare(image: &Image, expand: Expand) -> Result<(Image, Mask)> {
    if expand.is_empty() {
        return Err(Error::new("badFile"));
    }
    let source = image.to_rgba8();
    let sw = image.width() as usize;
    let sh = image.height() as usize;
    if sw == 0 || sh == 0 {
        return Err(Error::new("emptyImage"));
    }
    let left = expand.left as usize;
    let right = expand.right as usize;
    let top = expand.top as usize;
    let bottom = expand.bottom as usize;
    let dw = sw + left + right;
    let dh = sh + top + bottom;
    if dw > 8192 || dh > 8192 {
        return Err(Error::new("tooLarge").with("width", dw).with("height", dh));
    }
    let mut canvas = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        for x in 0..dw {
            let from_x = x.saturating_sub(left).min(sw - 1);
            let from_y = y.saturating_sub(top).min(sh - 1);
            let from = (from_y * sw + from_x) * 4;
            let into = (y * dw + x) * 4;
            canvas[into..into + 4].copy_from_slice(&source[from..from + 4]);
        }
    }
    let overlap = overlap_of(sw, sh);
    let mut data = vec![0u8; dw * dh];
    for y in 0..dh {
        for x in 0..dw {
            let in_picture = x >= left && x < left + sw && y >= top && y < top + sh;
            let seam = in_picture
                && ((left > 0 && x < left + overlap)
                    || (right > 0 && x + overlap >= left + sw)
                    || (top > 0 && y < top + overlap)
                    || (bottom > 0 && y + overlap >= top + sh));
            if !in_picture || seam {
                data[y * dw + x] = 255;
            }
        }
    }
    let picture = Image::from_rgba8(dw as u32, dh as u32, canvas)?;
    Ok((
        picture,
        Mask {
            width: dw as u32,
            height: dh as u32,
            data,
        },
    ))
}

fn overlap_of(width: usize, height: usize) -> usize {
    let short = width.min(height);
    (short / 12).clamp(8, 64).min(short / 2)
}

#[cfg(test)]
mod tests {
    use pixelit_core::{Color, Depth};

    use super::*;

    #[test]
    fn the_original_stays_and_the_margin_is_masked() {
        let image = Image::new(32, 16, Depth::Eight, Color::rgb(10, 20, 30)).unwrap();
        let (canvas, mask) = prepare(
            &image,
            Expand {
                left: 8,
                ..Expand::default()
            },
        )
        .unwrap();
        assert_eq!((canvas.width(), canvas.height()), (40, 16));
        assert_eq!(canvas.pixel8(8, 0), [10, 20, 30, 255]);
        assert_eq!(canvas.pixel8(0, 0), [10, 20, 30, 255]);
        assert_eq!(mask.data[0], 255);
        // Far from the new edge the original is not offered to the generator.
        let inside = 8 + 20;
        assert_eq!(mask.data[inside], 0);
        assert!(prepare(&image, Expand::default()).is_err());
    }

    #[test]
    fn a_ratio_grows_only_the_short_axis() {
        let wide = margins_for_ratio(160, 90, 1, 1);
        assert!(wide.top > 0 || wide.bottom > 0);
        assert_eq!((wide.left, wide.right), (0, 0));
        assert_eq!((90 + wide.top + wide.bottom) % 8, 0);

        let tall = margins_for_ratio(90, 160, 16, 9);
        assert!(tall.left > 0 || tall.right > 0);
        assert_eq!((tall.top, tall.bottom), (0, 0));
    }
}
