//! Filters: blur, sharpening, noise reduction, grain, vignette and simple
//! colour effects.

use crate::color::luma;
use crate::image::{Image, Pixels, Sample, CHANNELS};
use crate::pipeline::{Filter, FilterKind};
use crate::rows::{build_rows, for_each_row};

/// Above this sigma the blur switches from an exact kernel to three box passes,
/// whose cost does not grow with the radius.
const BOX_BLUR_FROM_SIGMA: f32 = 8.0;

/// Largest accepted blur sigma, in pixels.
const MAX_SIGMA: f32 = 500.0;

/// Half of a normalized Gaussian kernel: the centre weight, then one weight
/// per distance from it.
fn gaussian_weights(sigma: f32) -> Vec<f32> {
    let radius = (sigma * 3.0).ceil().max(1.0) as usize;
    let mut weights: Vec<f32> = (0..=radius)
        .map(|distance| (-((distance * distance) as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f32 = weights[0] + 2.0 * weights[1..].iter().sum::<f32>();
    for weight in &mut weights {
        *weight /= total;
    }
    weights
}

fn kernel_rows<S: Sample>(data: &[S], width: usize, height: usize, weights: &[f32]) -> Vec<S> {
    let stride = width * CHANNELS;
    build_rows(height, stride, |row_index, out: &mut [S]| {
        let row = &data[row_index * stride..(row_index + 1) * stride];
        for x in 0..width {
            for channel in 0..CHANNELS {
                let mut sum = weights[0] * row[x * CHANNELS + channel].to_u32() as f32;
                for (distance, weight) in weights.iter().enumerate().skip(1) {
                    let left = x.saturating_sub(distance);
                    let right = (x + distance).min(width - 1);
                    sum += weight
                        * (row[left * CHANNELS + channel].to_u32()
                            + row[right * CHANNELS + channel].to_u32())
                            as f32;
                }
                out[x * CHANNELS + channel] = S::from_u32((sum + 0.5) as u32);
            }
        }
    })
}

fn kernel_columns<S: Sample>(data: &[S], width: usize, height: usize, weights: &[f32]) -> Vec<S> {
    let stride = width * CHANNELS;
    build_rows(height, stride, |row_index, out: &mut [S]| {
        let centre = &data[row_index * stride..(row_index + 1) * stride];
        let mut sums: Vec<f32> = centre
            .iter()
            .map(|sample| weights[0] * sample.to_u32() as f32)
            .collect();
        for (distance, weight) in weights.iter().enumerate().skip(1) {
            let above = row_index.saturating_sub(distance);
            let below = (row_index + distance).min(height - 1);
            let above = &data[above * stride..(above + 1) * stride];
            let below = &data[below * stride..(below + 1) * stride];
            for ((sum, a), b) in sums.iter_mut().zip(above).zip(below) {
                *sum += weight * (a.to_u32() + b.to_u32()) as f32;
            }
        }
        for (target, sum) in out.iter_mut().zip(&sums) {
            *target = S::from_u32((sum + 0.5) as u32);
        }
    })
}

/// Radii of three box passes that approximate a Gaussian of the given sigma.
fn box_radii(sigma: f32) -> [usize; 3] {
    let passes = 3.0f32;
    let ideal = (12.0 * sigma * sigma / passes + 1.0).sqrt();
    let mut lower = ideal.floor() as i64;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let lower = lower.max(1);
    let upper = lower + 2;
    let lower_f = lower as f32;
    let switch =
        (12.0 * sigma * sigma - passes * lower_f * lower_f - 4.0 * passes * lower_f - 3.0 * passes)
            / (-4.0 * lower_f - 4.0);
    let switch = switch.round() as i64;
    let mut radii = [0usize; 3];
    for (index, radius) in radii.iter_mut().enumerate() {
        let size = if (index as i64) < switch {
            lower
        } else {
            upper
        };
        *radius = ((size - 1) / 2) as usize;
    }
    radii
}

fn box_rows<S: Sample>(data: &[S], width: usize, height: usize, radius: usize) -> Vec<S> {
    let stride = width * CHANNELS;
    let window = (2 * radius + 1) as u64;
    build_rows(height, stride, |row_index, out: &mut [S]| {
        let row = &data[row_index * stride..(row_index + 1) * stride];
        let at = |x: i64, channel: usize| -> u64 {
            let x = x.clamp(0, width as i64 - 1) as usize;
            u64::from(row[x * CHANNELS + channel].to_u32())
        };
        for channel in 0..CHANNELS {
            let mut sum: u64 = (-(radius as i64)..=radius as i64)
                .map(|x| at(x, channel))
                .sum();
            for x in 0..width {
                out[x * CHANNELS + channel] = S::from_u32(((sum + window / 2) / window) as u32);
                sum += at(x as i64 + radius as i64 + 1, channel);
                sum -= at(x as i64 - radius as i64, channel);
            }
        }
    })
}

/// The row at `y`, with rows beyond the image repeating the nearest edge.
fn clamped_row<S: Sample>(data: &[S], stride: usize, height: usize, y: i64) -> &[S] {
    let y = y.clamp(0, height as i64 - 1) as usize;
    &data[y * stride..(y + 1) * stride]
}

fn box_columns<S: Sample>(data: &[S], width: usize, height: usize, radius: usize) -> Vec<S> {
    let stride = width * CHANNELS;
    let window = (2 * radius + 1) as u64;
    let mut sums = vec![0u64; stride];
    for y in -(radius as i64)..=radius as i64 {
        for (sum, sample) in sums.iter_mut().zip(clamped_row(data, stride, height, y)) {
            *sum += u64::from(sample.to_u32());
        }
    }
    let mut out = Vec::with_capacity(data.len());
    for y in 0..height {
        out.extend(
            sums.iter()
                .map(|sum| S::from_u32(((sum + window / 2) / window) as u32)),
        );
        let entering = clamped_row(data, stride, height, y as i64 + radius as i64 + 1);
        let leaving = clamped_row(data, stride, height, y as i64 - radius as i64);
        for ((sum, add), remove) in sums.iter_mut().zip(entering).zip(leaving) {
            *sum += u64::from(add.to_u32());
            *sum -= u64::from(remove.to_u32());
        }
    }
    out
}

/// Gaussian blur of all four channels of raw samples.
fn blurred<S: Sample>(data: &[S], width: usize, height: usize, sigma: f32) -> Vec<S> {
    let sigma = sigma.min(MAX_SIGMA);
    if sigma < BOX_BLUR_FROM_SIGMA {
        let weights = gaussian_weights(sigma);
        let rows = kernel_rows(data, width, height, &weights);
        kernel_columns(&rows, width, height, &weights)
    } else {
        let mut current = data.to_vec();
        for radius in box_radii(sigma) {
            let rows = box_rows(&current, width, height, radius);
            current = box_columns(&rows, width, height, radius);
        }
        current
    }
}

fn premultiply<S: Sample>(data: &mut [S]) {
    let max = u64::from(S::MAX);
    for pixel in data.chunks_exact_mut(CHANNELS) {
        let alpha = u64::from(pixel[3].to_u32());
        if alpha != max {
            for sample in &mut pixel[..3] {
                let value = (u64::from(sample.to_u32()) * alpha + max / 2) / max;
                *sample = S::from_u32(value as u32);
            }
        }
    }
}

fn unpremultiply<S: Sample>(data: &mut [S]) {
    let max = u64::from(S::MAX);
    for pixel in data.chunks_exact_mut(CHANNELS) {
        let alpha = u64::from(pixel[3].to_u32());
        if alpha != max && alpha != 0 {
            for sample in &mut pixel[..3] {
                let value = (u64::from(sample.to_u32()) * max + alpha / 2) / alpha;
                *sample = S::from_u32(value as u32);
            }
        }
    }
}

fn blur_samples<S: Sample>(
    data: &mut Vec<S>,
    width: usize,
    height: usize,
    sigma: f32,
    alpha: bool,
) {
    if alpha {
        premultiply(data);
    }
    *data = blurred(data, width, height, sigma);
    if alpha {
        unpremultiply(data);
    }
}

/// Blurs the image with a Gaussian of the given sigma, in pixels.
pub fn gaussian_blur(image: &mut Image, sigma: f32) {
    if !sigma.is_finite() || sigma < 0.05 {
        return;
    }
    let (width, height, alpha) = (image.width as usize, image.height as usize, image.alpha);
    match &mut image.pixels {
        Pixels::U8(data) => blur_samples(data, width, height, sigma, alpha),
        Pixels::U16(data) => blur_samples(data, width, height, sigma, alpha),
    }
}

/// Unsharp mask: adds back the difference between the image and its blur.
fn sharpen_samples<S: Sample>(
    data: &mut [S],
    width: usize,
    height: usize,
    sigma: f32,
    amount: f32,
) {
    let soft = blurred(data, width, height, sigma);
    let stride = width * CHANNELS;
    for_each_row(data, stride, |row_index, row| {
        let soft_row = &soft[row_index * stride..(row_index + 1) * stride];
        for (pixel, soft_pixel) in row
            .chunks_exact_mut(CHANNELS)
            .zip(soft_row.chunks_exact(CHANNELS))
        {
            for channel in 0..3 {
                let value = pixel[channel].to_u32() as f32;
                let detail = value - soft_pixel[channel].to_u32() as f32;
                let sharp = (value + detail * amount + 0.5).max(0.0);
                pixel[channel] = S::from_u32(sharp as u32);
            }
        }
    });
}

/// Replaces a pixel by its blur where the two are close (flat, noisy areas)
/// and keeps it where they differ (edges).
fn denoise_samples<S: Sample>(
    data: &mut [S],
    width: usize,
    height: usize,
    sigma: f32,
    threshold: f32,
) {
    let soft = blurred(data, width, height, sigma);
    let stride = width * CHANNELS;
    let max = S::MAX as f32;
    for_each_row(data, stride, |row_index, row| {
        let soft_row = &soft[row_index * stride..(row_index + 1) * stride];
        for (pixel, soft_pixel) in row
            .chunks_exact_mut(CHANNELS)
            .zip(soft_row.chunks_exact(CHANNELS))
        {
            for channel in 0..3 {
                let value = pixel[channel].to_u32() as f32;
                let difference = soft_pixel[channel].to_u32() as f32 - value;
                let relative = difference / max / threshold;
                let weight = (-relative * relative).exp();
                pixel[channel] = S::from_u32((value + difference * weight + 0.5).max(0.0) as u32);
            }
        }
    });
}

/// Deterministic noise in `-1.0..=1.0` for a pixel position.
fn noise(x: u32, y: u32) -> f32 {
    let mut hash = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2C1B_3C6D);
    hash ^= hash >> 12;
    hash = hash.wrapping_mul(0x297A_2D39);
    hash ^= hash >> 15;
    (hash as f32 / u32::MAX as f32) * 2.0 - 1.0
}

fn grain_samples<S: Sample>(data: &mut [S], width: usize, strength: f32) {
    let max = S::MAX as f32;
    for_each_row(data, width * CHANNELS, |row_index, row| {
        let y = row_index as u32;
        for (column, pixel) in row.chunks_exact_mut(CHANNELS).enumerate() {
            let x = column as u32;
            let grain = f32::midpoint(
                noise(x, y),
                noise(y.wrapping_add(7919), x.wrapping_add(104_729)),
            );
            let shift = grain * strength * max;
            for sample in &mut pixel[..3] {
                let value = sample.to_u32() as f32 + shift + 0.5;
                *sample = S::from_u32(value.max(0.0) as u32);
            }
        }
    });
}

fn vignette_samples<S: Sample>(
    data: &mut [S],
    width: usize,
    height: usize,
    strength: f32,
    inner: f32,
) {
    let centre_x = width as f32 / 2.0;
    let centre_y = height as f32 / 2.0;
    let reach = (centre_x * centre_x + centre_y * centre_y).sqrt().max(1.0);
    for_each_row(data, width * CHANNELS, |row_index, row| {
        let dy = row_index as f32 + 0.5 - centre_y;
        for (column, pixel) in row.chunks_exact_mut(CHANNELS).enumerate() {
            let dx = column as f32 + 0.5 - centre_x;
            let distance = (dx * dx + dy * dy).sqrt() / reach;
            let t = ((distance - inner) / (1.0 - inner)).clamp(0.0, 1.0);
            let falloff = t * t * (3.0 - 2.0 * t);
            if falloff <= 0.0 {
                continue;
            }
            for sample in &mut pixel[..3] {
                let value = sample.to_unit();
                let shaded = if strength >= 0.0 {
                    value * (1.0 - strength * falloff)
                } else {
                    value + (1.0 - value) * (-strength) * falloff
                };
                *sample = S::from_unit(shaded);
            }
        }
    });
}

#[derive(Clone, Copy)]
enum Effect {
    Grayscale,
    Sepia,
    Invert,
}

fn effect_samples<S: Sample>(data: &mut [S], width: usize, effect: Effect, blend: f32) {
    for_each_row(data, width * CHANNELS, |_, row| {
        for pixel in row.chunks_exact_mut(CHANNELS) {
            let [r, g, b] = [pixel[0].to_unit(), pixel[1].to_unit(), pixel[2].to_unit()];
            let changed = match effect {
                Effect::Grayscale => {
                    let light = luma(r, g, b);
                    [light, light, light]
                }
                Effect::Sepia => [
                    0.393 * r + 0.769 * g + 0.189 * b,
                    0.349 * r + 0.686 * g + 0.168 * b,
                    0.272 * r + 0.534 * g + 0.131 * b,
                ],
                Effect::Invert => [1.0 - r, 1.0 - g, 1.0 - b],
            };
            pixel[0] = S::from_unit(r + (changed[0] - r) * blend);
            pixel[1] = S::from_unit(g + (changed[1] - g) * blend);
            pixel[2] = S::from_unit(b + (changed[2] - b) * blend);
        }
    });
}

/// Applies a [`Filter`] step in place. `scale` is the size of the image
/// relative to the full file; pixel radii are multiplied by it.
pub fn filter(image: &mut Image, spec: &Filter, scale: f32) {
    let amount = if spec.amount.is_finite() {
        (spec.amount / 100.0).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let radius = if spec.radius.is_finite() {
        spec.radius.max(0.0)
    } else {
        0.0
    };
    let sigma = (radius * scale).min(MAX_SIGMA);
    let (width, height) = (image.width as usize, image.height as usize);

    match spec.kind {
        FilterKind::Blur => gaussian_blur(image, sigma),
        FilterKind::Sharpen => {
            if amount <= 0.0 {
                return;
            }
            let sigma = sigma.max(0.3);
            match &mut image.pixels {
                Pixels::U8(data) => sharpen_samples(data, width, height, sigma, amount * 2.0),
                Pixels::U16(data) => sharpen_samples(data, width, height, sigma, amount * 2.0),
            }
        }
        FilterKind::Denoise => {
            if amount <= 0.0 {
                return;
            }
            let sigma = sigma.max(0.5);
            let threshold = 0.02 + 0.18 * amount;
            match &mut image.pixels {
                Pixels::U8(data) => denoise_samples(data, width, height, sigma, threshold),
                Pixels::U16(data) => denoise_samples(data, width, height, sigma, threshold),
            }
        }
        FilterKind::Grain => {
            if amount <= 0.0 {
                return;
            }
            match &mut image.pixels {
                Pixels::U8(data) => grain_samples(data, width, amount * 0.25),
                Pixels::U16(data) => grain_samples(data, width, amount * 0.25),
            }
        }
        FilterKind::Vignette => {
            if amount == 0.0 {
                return;
            }
            // The radius of a vignette is the share of the half-diagonal that
            // stays untouched, as a percentage.
            let inner = (radius / 100.0).clamp(0.0, 0.95);
            match &mut image.pixels {
                Pixels::U8(data) => vignette_samples(data, width, height, amount, inner),
                Pixels::U16(data) => vignette_samples(data, width, height, amount, inner),
            }
        }
        FilterKind::Grayscale | FilterKind::Sepia | FilterKind::Invert => {
            if amount <= 0.0 {
                return;
            }
            let effect = match spec.kind {
                FilterKind::Grayscale => Effect::Grayscale,
                FilterKind::Sepia => Effect::Sepia,
                _ => Effect::Invert,
            };
            match &mut image.pixels {
                Pixels::U8(data) => effect_samples(data, width, effect, amount),
                Pixels::U16(data) => effect_samples(data, width, effect, amount),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::image::Depth;

    fn run(image: &mut Image, kind: FilterKind, amount: f32, radius: f32) {
        filter(
            image,
            &Filter {
                kind,
                amount,
                radius,
            },
            1.0,
        );
    }

    /// Black on the left half, white on the right half.
    fn edge(depth: Depth) -> Image {
        let mut data = Vec::new();
        for _ in 0..32 {
            for x in 0..32 {
                let value = if x < 16 { 0 } else { 255 };
                data.extend_from_slice(&[value, value, value, 255]);
            }
        }
        Image::from_rgba8(32, 32, data).unwrap().into_depth(depth)
    }

    #[test]
    fn weights_are_normalized() {
        for sigma in [0.3f32, 1.0, 2.5, 7.9] {
            let weights = gaussian_weights(sigma);
            let total = weights[0] + 2.0 * weights[1..].iter().sum::<f32>();
            assert!((total - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn blur_keeps_flat_areas_and_softens_edges() {
        for depth in [Depth::Eight, Depth::Sixteen] {
            for sigma in [2.0f32, 12.0] {
                let mut flat = Image::new(40, 40, depth, Color::rgb(90, 160, 30)).unwrap();
                gaussian_blur(&mut flat, sigma);
                assert_eq!(flat.pixel8(20, 20), [90, 160, 30, 255]);
                assert_eq!(flat.pixel8(0, 0), [90, 160, 30, 255]);

                let mut image = edge(depth);
                gaussian_blur(&mut image, sigma);
                let middle = image.pixel8(16, 16)[0];
                assert!(middle > 60 && middle < 200, "{middle}");
                assert!(image.pixel8(0, 16)[0] < middle);
                assert!(image.pixel8(31, 16)[0] > middle);
            }
        }
    }

    #[test]
    fn blur_does_not_bleed_colour_from_transparent_pixels() {
        // Opaque red next to fully transparent green.
        let mut data = Vec::new();
        for _ in 0..16 {
            for x in 0..16 {
                if x < 8 {
                    data.extend_from_slice(&[255, 0, 0, 255]);
                } else {
                    data.extend_from_slice(&[0, 255, 0, 0]);
                }
            }
        }
        let mut image = Image::from_rgba8(16, 16, data).unwrap();
        gaussian_blur(&mut image, 2.0);
        let [r, g, _, a] = image.pixel8(8, 8);
        assert!(a > 0 && a < 255);
        assert!(r > 240, "{r}");
        assert!(g < 15, "{g}");
    }

    #[test]
    fn sharpen_increases_edge_contrast() {
        let mut data = Vec::new();
        for _ in 0..32 {
            for x in 0..32 {
                let value = if x < 16 { 100 } else { 150 };
                data.extend_from_slice(&[value, value, value, 255]);
            }
        }
        let mut image = Image::from_rgba8(32, 32, data).unwrap();
        run(&mut image, FilterKind::Sharpen, 100.0, 1.5);
        assert!(image.pixel8(15, 16)[0] < 100);
        assert!(image.pixel8(16, 16)[0] > 150);
        assert_eq!(image.pixel8(2, 16)[0], 100);
        assert_eq!(image.pixel8(29, 16)[0], 150);
    }

    #[test]
    fn denoise_smooths_noise_and_keeps_edges() {
        let mut image = edge(Depth::Eight);
        if let Pixels::U8(data) = image.pixels_mut() {
            for (index, pixel) in data.chunks_exact_mut(4).enumerate() {
                let wobble = if index % 2 == 0 { 6 } else { 0 };
                for sample in &mut pixel[..3] {
                    *sample = if *sample == 0 { wobble } else { 255 - wobble };
                }
            }
        }
        run(&mut image, FilterKind::Denoise, 60.0, 1.5);
        let a = i32::from(image.pixel8(4, 10)[0]);
        let b = i32::from(image.pixel8(5, 10)[0]);
        assert!((a - b).abs() <= 2, "{a} {b}");
        assert!(image.pixel8(12, 10)[0] < 20);
        assert!(image.pixel8(20, 10)[0] > 235);
    }

    #[test]
    fn grain_is_deterministic_and_bounded() {
        let mut first = Image::new(32, 32, Depth::Eight, Color::rgb(128, 128, 128)).unwrap();
        let mut second = first.clone();
        run(&mut first, FilterKind::Grain, 40.0, 0.0);
        run(&mut second, FilterKind::Grain, 40.0, 0.0);
        assert_eq!(first, second);
        let mut changed = 0;
        for y in 0..32 {
            for x in 0..32 {
                let [r, g, b, a] = first.pixel8(x, y);
                assert_eq!((r, a), (g, 255));
                assert_eq!(g, b);
                assert!((i32::from(r) - 128).abs() <= 26);
                if r != 128 {
                    changed += 1;
                }
            }
        }
        assert!(changed > 500);
    }

    #[test]
    fn vignette_darkens_or_lightens_corners_only() {
        let mut dark = Image::new(64, 64, Depth::Eight, Color::rgb(200, 200, 200)).unwrap();
        run(&mut dark, FilterKind::Vignette, 80.0, 40.0);
        assert_eq!(dark.pixel8(32, 32)[0], 200);
        assert!(dark.pixel8(0, 0)[0] < 80);

        let mut light = Image::new(64, 64, Depth::Sixteen, Color::rgb(50, 50, 50)).unwrap();
        run(&mut light, FilterKind::Vignette, -80.0, 40.0);
        assert_eq!(light.pixel8(32, 32)[0], 50);
        assert!(light.pixel8(63, 63)[0] > 180);
    }

    #[test]
    fn colour_effects() {
        let mut image = Image::new(2, 2, Depth::Eight, Color::rgb(200, 100, 0)).unwrap();
        run(&mut image, FilterKind::Invert, 100.0, 0.0);
        assert_eq!(image.pixel8(0, 0), [55, 155, 255, 255]);

        let mut image = Image::new(2, 2, Depth::Eight, Color::rgb(200, 100, 0)).unwrap();
        run(&mut image, FilterKind::Grayscale, 100.0, 0.0);
        let [r, g, b, _] = image.pixel8(0, 0);
        assert_eq!((r, g), (g, b));
        assert_eq!(r, 114);

        let mut image = Image::new(2, 2, Depth::Eight, Color::rgb(200, 100, 0)).unwrap();
        run(&mut image, FilterKind::Grayscale, 0.0, 0.0);
        assert_eq!(image.pixel8(0, 0), [200, 100, 0, 255]);

        let mut image = Image::new(2, 2, Depth::Sixteen, Color::rgb(100, 100, 100)).unwrap();
        run(&mut image, FilterKind::Sepia, 100.0, 0.0);
        let [r, g, b, _] = image.pixel8(0, 0);
        assert!(r > g && g > b);
    }

    #[test]
    fn bad_parameters_are_ignored() {
        let mut image = Image::new(8, 8, Depth::Eight, Color::rgb(10, 20, 30)).unwrap();
        let before = image.clone();
        run(&mut image, FilterKind::Blur, 50.0, f32::NAN);
        run(&mut image, FilterKind::Sharpen, f32::NAN, 1.0);
        run(&mut image, FilterKind::Blur, 50.0, -4.0);
        filter(&mut image, &Filter::default(), f32::NAN);
        assert_eq!(image, before);
    }

    #[test]
    fn box_radii_grow_with_sigma() {
        let small = box_radii(8.0);
        let large = box_radii(40.0);
        assert!(small.iter().all(|&radius| radius >= 1));
        assert!(large[0] > small[2]);
    }
}
