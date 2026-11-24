//! Tone and colour correction.

use crate::color::{linear_to_srgb, luma, srgb_to_linear};
use crate::image::{Image, Pixels, Sample, CHANNELS};
use crate::pipeline::Adjust;
use crate::rows::for_each_row;

fn slider(value: f32) -> f32 {
    if value.is_finite() {
        (value / 100.0).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// A monotone cubic curve through control points, sampled on `0.0..=1.0`.
pub struct Curve {
    xs: Vec<f32>,
    ys: Vec<f32>,
    tangents: Vec<f32>,
}

impl Curve {
    /// Builds the curve. Points outside the unit square are clamped, duplicates
    /// of an input value are dropped, and the ends `(0, 0)` and `(1, 1)` are
    /// added when the points do not reach them.
    pub fn new(points: &[[f32; 2]]) -> Self {
        let mut sorted: Vec<[f32; 2]> = points
            .iter()
            .filter(|point| point[0].is_finite() && point[1].is_finite())
            .map(|point| [point[0].clamp(0.0, 1.0), point[1].clamp(0.0, 1.0)])
            .collect();
        sorted.sort_by(|a, b| a[0].total_cmp(&b[0]));
        sorted.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-4);
        if sorted.first().is_none_or(|point| point[0] > 1e-4) {
            sorted.insert(0, [0.0, 0.0]);
        }
        if sorted.last().is_none_or(|point| point[0] < 1.0 - 1e-4) {
            sorted.push([1.0, 1.0]);
        }

        let count = sorted.len();
        let xs: Vec<f32> = sorted.iter().map(|point| point[0]).collect();
        let ys: Vec<f32> = sorted.iter().map(|point| point[1]).collect();
        let slopes: Vec<f32> = (0..count - 1)
            .map(|index| (ys[index + 1] - ys[index]) / (xs[index + 1] - xs[index]))
            .collect();
        // Fritsch-Carlson tangents keep the curve free of overshoot.
        let mut tangents = vec![0.0f32; count];
        tangents[0] = slopes[0];
        tangents[count - 1] = slopes[count - 2];
        for index in 1..count - 1 {
            let before = slopes[index - 1];
            let after = slopes[index];
            tangents[index] = if before * after <= 0.0 {
                0.0
            } else {
                f32::midpoint(before, after)
            };
        }
        for index in 0..count - 1 {
            let slope = slopes[index];
            if slope == 0.0 {
                tangents[index] = 0.0;
                tangents[index + 1] = 0.0;
                continue;
            }
            let a = tangents[index] / slope;
            let b = tangents[index + 1] / slope;
            let length = a * a + b * b;
            if length > 9.0 {
                let factor = 3.0 / length.sqrt();
                tangents[index] = factor * a * slope;
                tangents[index + 1] = factor * b * slope;
            }
        }
        Self { xs, ys, tangents }
    }

    pub fn value(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        let last = self.xs.len() - 1;
        let mut segment = 0;
        while segment + 1 < last && x > self.xs[segment + 1] {
            segment += 1;
        }
        let width = self.xs[segment + 1] - self.xs[segment];
        let t = ((x - self.xs[segment]) / width).clamp(0.0, 1.0);
        let t2 = t * t;
        let t3 = t2 * t;
        let value = (2.0 * t3 - 3.0 * t2 + 1.0) * self.ys[segment]
            + (t3 - 2.0 * t2 + t) * width * self.tangents[segment]
            + (-2.0 * t3 + 3.0 * t2) * self.ys[segment + 1]
            + (t3 - t2) * width * self.tangents[segment + 1];
        value.clamp(0.0, 1.0)
    }
}

/// Per-channel lookup tables for the parts of the correction that act on each
/// channel on its own.
fn tables<S: Sample>(spec: &Adjust) -> [Vec<S>; 3] {
    let exposure = if spec.exposure.is_finite() {
        2.0f32.powf(spec.exposure.clamp(-5.0, 5.0))
    } else {
        1.0
    };
    let brightness = slider(spec.brightness) * 0.5;
    let contrast = slider(spec.contrast);
    let contrast_gain = if contrast >= 0.0 {
        1.0 + contrast * 1.5
    } else {
        1.0 + contrast * 0.75
    };
    let temperature = slider(spec.temperature) * 0.25;
    let tint = slider(spec.tint) * 0.25;
    let gains = [1.0 + temperature, 1.0 - tint, 1.0 - temperature];
    let curve = (!spec.curve.is_empty()).then(|| Curve::new(&spec.curve));

    gains.map(|gain| {
        (0..=S::MAX)
            .map(|index| {
                let mut value = index as f32 / S::MAX as f32;
                if exposure != 1.0 {
                    value = linear_to_srgb((srgb_to_linear(value) * exposure).min(1.0));
                }
                value = value * gain + brightness;
                value = (value - 0.5) * contrast_gain + 0.5;
                value = value.clamp(0.0, 1.0);
                if let Some(curve) = &curve {
                    value = curve.value(value);
                }
                S::from_unit(value)
            })
            .collect()
    })
}

fn adjust_samples<S: Sample>(data: &mut [S], width: usize, spec: &Adjust) {
    let [red, green, blue] = tables::<S>(spec);
    let highlights = slider(spec.highlights) * 0.35;
    let shadows = slider(spec.shadows) * 0.35;
    let saturation = slider(spec.saturation);
    let vibrance = slider(spec.vibrance);
    let per_pixel = highlights != 0.0 || shadows != 0.0 || saturation != 0.0 || vibrance != 0.0;

    for_each_row(data, width * CHANNELS, |_, row| {
        for pixel in row.chunks_exact_mut(CHANNELS) {
            pixel[0] = red[pixel[0].to_u32() as usize];
            pixel[1] = green[pixel[1].to_u32() as usize];
            pixel[2] = blue[pixel[2].to_u32() as usize];
            if !per_pixel {
                continue;
            }
            let mut rgb = [pixel[0].to_unit(), pixel[1].to_unit(), pixel[2].to_unit()];
            let light = luma(rgb[0], rgb[1], rgb[2]);
            if shadows != 0.0 || highlights != 0.0 {
                let dark_weight = (1.0 - light) * (1.0 - light);
                let bright_weight = light * light;
                for value in &mut rgb {
                    let room = |amount: f32, current: f32| {
                        if amount > 0.0 {
                            1.0 - current
                        } else {
                            current
                        }
                    };
                    let lifted = *value + shadows * dark_weight * room(shadows, *value);
                    *value = lifted + highlights * bright_weight * room(highlights, lifted);
                }
            }
            if saturation != 0.0 || vibrance != 0.0 {
                let light = luma(rgb[0], rgb[1], rgb[2]);
                let most = rgb[0].max(rgb[1]).max(rgb[2]);
                let least = rgb[0].min(rgb[1]).min(rgb[2]);
                let gain = (1.0 + saturation) * (1.0 + vibrance * (1.0 - (most - least)));
                for value in &mut rgb {
                    *value = light + (*value - light) * gain;
                }
            }
            pixel[0] = S::from_unit(rgb[0]);
            pixel[1] = S::from_unit(rgb[1]);
            pixel[2] = S::from_unit(rgb[2]);
        }
    });
}

/// Applies an [`Adjust`] step in place. Alpha is left untouched.
pub fn adjust(image: &mut Image, spec: &Adjust) {
    if spec.is_identity() {
        return;
    }
    let width = image.width as usize;
    match &mut image.pixels {
        Pixels::U8(data) => adjust_samples(data, width, spec),
        Pixels::U16(data) => adjust_samples(data, width, spec),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::image::Depth;

    fn gray(value: u8, depth: Depth) -> Image {
        Image::new(4, 4, depth, Color::rgb(value, value, value)).unwrap()
    }

    fn apply(value: u8, spec: &Adjust) -> [u8; 4] {
        let mut image = gray(value, Depth::Eight);
        adjust(&mut image, spec);
        image.pixel8(1, 1)
    }

    #[test]
    fn identity_changes_nothing() {
        for depth in [Depth::Eight, Depth::Sixteen] {
            let mut image = gray(77, depth);
            let before = image.clone();
            adjust(&mut image, &Adjust::default());
            assert_eq!(image, before);
        }
    }

    #[test]
    fn zero_strength_tables_are_identity() {
        // A curve through the diagonal exercises the table path without
        // changing values.
        let spec = Adjust {
            curve: vec![[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]],
            ..Adjust::default()
        };
        for value in [0u8, 1, 17, 128, 254, 255] {
            assert_eq!(apply(value, &spec), [value, value, value, 255]);
        }
    }

    #[test]
    fn exposure_and_brightness_move_values_the_right_way() {
        let brighter = Adjust {
            exposure: 1.0,
            ..Adjust::default()
        };
        assert!(apply(100, &brighter)[0] > 100);
        let darker = Adjust {
            exposure: -1.0,
            ..Adjust::default()
        };
        assert!(apply(100, &darker)[0] < 100);
        let lifted = Adjust {
            brightness: 40.0,
            ..Adjust::default()
        };
        assert_eq!(apply(100, &lifted)[0], 151);
    }

    #[test]
    fn contrast_pivots_around_middle_gray() {
        let spec = Adjust {
            contrast: 100.0,
            ..Adjust::default()
        };
        assert!(apply(200, &spec)[0] > 200);
        assert!(apply(50, &spec)[0] < 50);
        let flat = Adjust {
            contrast: -100.0,
            ..Adjust::default()
        };
        assert!(apply(200, &flat)[0] < 200);
        assert!(apply(50, &flat)[0] > 50);
    }

    #[test]
    fn temperature_shifts_red_against_blue() {
        let warm = Adjust {
            temperature: 100.0,
            ..Adjust::default()
        };
        let [r, g, b, _] = apply(120, &warm);
        assert!(r > 120 && b < 120);
        assert_eq!(g, 120);
    }

    #[test]
    fn saturation_minus_100_gives_gray() {
        let mut image = Image::new(2, 2, Depth::Eight, Color::rgb(200, 50, 10)).unwrap();
        let spec = Adjust {
            saturation: -100.0,
            ..Adjust::default()
        };
        adjust(&mut image, &spec);
        let [r, g, b, a] = image.pixel8(0, 0);
        assert!(r.abs_diff(g) <= 1 && g.abs_diff(b) <= 1);
        assert_eq!(a, 255);
    }

    #[test]
    fn shadows_lift_dark_values_more_than_bright_ones() {
        let spec = Adjust {
            shadows: 100.0,
            ..Adjust::default()
        };
        let dark_gain = i32::from(apply(30, &spec)[0]) - 30;
        let bright_gain = i32::from(apply(220, &spec)[0]) - 220;
        assert!(dark_gain > 20);
        assert!(bright_gain < 3);
    }

    #[test]
    fn curve_is_monotone_and_hits_its_points() {
        let curve = Curve::new(&[[0.25, 0.1], [0.75, 0.9]]);
        assert!((curve.value(0.0)).abs() < 1e-5);
        assert!((curve.value(1.0) - 1.0).abs() < 1e-5);
        assert!((curve.value(0.25) - 0.1).abs() < 1e-4);
        assert!((curve.value(0.75) - 0.9).abs() < 1e-4);
        let mut previous = -1.0f32;
        for step in 0..=1000 {
            let value = curve.value(step as f32 / 1000.0);
            assert!(value >= previous - 1e-5);
            previous = value;
        }
    }

    #[test]
    fn curve_survives_bad_input() {
        let curve = Curve::new(&[[f32::NAN, 0.5], [0.5, 0.5], [0.5, 0.9], [7.0, -3.0]]);
        for step in 0..=10 {
            let value = curve.value(step as f32 / 10.0);
            assert!((0.0..=1.0).contains(&value));
        }
        let empty = Curve::new(&[]);
        assert!((empty.value(0.3) - 0.3).abs() < 1e-5);
    }

    #[test]
    fn sixteen_bit_path_matches_eight_bit_one() {
        let spec = Adjust {
            contrast: 40.0,
            saturation: 30.0,
            exposure: 0.5,
            ..Adjust::default()
        };
        let mut narrow = Image::new(2, 2, Depth::Eight, Color::rgb(180, 90, 40)).unwrap();
        let mut wide = Image::new(2, 2, Depth::Sixteen, Color::rgb(180, 90, 40)).unwrap();
        adjust(&mut narrow, &spec);
        adjust(&mut wide, &spec);
        let a = narrow.pixel8(0, 0);
        let b = wide.pixel8(0, 0);
        for channel in 0..3 {
            assert!(a[channel].abs_diff(b[channel]) <= 2, "{a:?} vs {b:?}");
        }
    }
}
