//! Compact picture fingerprints: BlurHash placeholders and a perceptual hash
//! for finding near-duplicates.

use std::f32::consts::PI;

use crate::color::{linear_to_srgb, luma, srgb_to_linear};
use crate::error::Result;
use crate::image::Image;
use crate::ops::resize::{fit_within, resize_to};
use crate::pipeline::ResizeFilter;

const BASE83: &[u8; 83] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~";

/// Side of the reduced copy BlurHash is computed from.
const BLURHASH_SIDE: u32 = 64;

/// Side of the reduced copy the perceptual hash is computed from.
const PHASH_SIDE: usize = 32;

/// Side of the low-frequency block kept by the perceptual hash.
const PHASH_BLOCK: usize = 8;

fn push_base83(out: &mut String, value: u32, digits: u32) {
    for digit in (0..digits).rev() {
        let index = (value / 83u32.pow(digit)) % 83;
        out.push(char::from(BASE83[index as usize]));
    }
}

fn signed_sqrt(value: f32) -> f32 {
    value.abs().sqrt().copysign(value)
}

fn srgb_byte(linear: f32) -> u32 {
    (linear_to_srgb(linear.clamp(0.0, 1.0)) * 255.0 + 0.5) as u32
}

/// Encodes a BlurHash with the given number of components (1 to 9 each).
/// Transparent pixels count as white.
pub fn blurhash(image: &Image, components_x: u32, components_y: u32) -> Result<String> {
    let components_x = components_x.clamp(1, 9) as usize;
    let components_y = components_y.clamp(1, 9) as usize;
    let small = fit_within(image, BLURHASH_SIDE, ResizeFilter::Bilinear)?;
    let (width, height) = (small.width() as usize, small.height() as usize);
    let to_linear: Vec<f32> = (0..256)
        .map(|value| srgb_to_linear(value as f32 / 255.0))
        .collect();
    let linear: Vec<[f32; 3]> = small
        .to_rgba8()
        .chunks_exact(4)
        .map(|pixel| {
            let alpha = f32::from(pixel[3]) / 255.0;
            let over_white = |value: u8| {
                let mixed = f32::from(value) * alpha + 255.0 * (1.0 - alpha);
                to_linear[(mixed + 0.5) as usize]
            };
            [
                over_white(pixel[0]),
                over_white(pixel[1]),
                over_white(pixel[2]),
            ]
        })
        .collect();

    let cos_x: Vec<Vec<f32>> = (0..components_x)
        .map(|i| {
            (0..width)
                .map(|x| (PI * i as f32 * x as f32 / width as f32).cos())
                .collect()
        })
        .collect();
    let cos_y: Vec<Vec<f32>> = (0..components_y)
        .map(|j| {
            (0..height)
                .map(|y| (PI * j as f32 * y as f32 / height as f32).cos())
                .collect()
        })
        .collect();

    let mut factors = Vec::with_capacity(components_x * components_y);
    for (j, row_basis) in cos_y.iter().enumerate() {
        for (i, column_basis) in cos_x.iter().enumerate() {
            let mut sum = [0.0f32; 3];
            for y in 0..height {
                for x in 0..width {
                    let basis = column_basis[x] * row_basis[y];
                    let pixel = linear[y * width + x];
                    sum[0] += basis * pixel[0];
                    sum[1] += basis * pixel[1];
                    sum[2] += basis * pixel[2];
                }
            }
            let normalisation = if i == 0 && j == 0 { 1.0 } else { 2.0 };
            let scale = normalisation / (width * height) as f32;
            factors.push([sum[0] * scale, sum[1] * scale, sum[2] * scale]);
        }
    }

    let mut hash = String::with_capacity(6 + 2 * factors.len());
    push_base83(
        &mut hash,
        ((components_x - 1) + (components_y - 1) * 9) as u32,
        1,
    );
    let ac = &factors[1..];
    let maximum = if ac.is_empty() {
        push_base83(&mut hash, 0, 1);
        1.0
    } else {
        let actual = ac
            .iter()
            .flat_map(|factor| factor.iter())
            .fold(0.0f32, |most, value| most.max(value.abs()));
        let quantised = (actual * 166.0 - 0.5).floor().clamp(0.0, 82.0) as u32;
        push_base83(&mut hash, quantised, 1);
        (quantised + 1) as f32 / 166.0
    };
    let dc = factors[0];
    push_base83(
        &mut hash,
        (srgb_byte(dc[0]) << 16) | (srgb_byte(dc[1]) << 8) | srgb_byte(dc[2]),
        4,
    );
    for factor in ac {
        let quantise = |value: f32| {
            (signed_sqrt(value / maximum) * 9.0 + 9.5)
                .floor()
                .clamp(0.0, 18.0) as u32
        };
        let value = quantise(factor[0]) * 19 * 19 + quantise(factor[1]) * 19 + quantise(factor[2]);
        push_base83(&mut hash, value, 2);
    }
    Ok(hash)
}

/// A 64-bit perceptual hash: pictures that look alike differ in few bits.
pub fn perceptual_hash(image: &Image) -> Result<u64> {
    let side = PHASH_SIDE as u32;
    let small = resize_to(image, side, side, ResizeFilter::Box, false, None)?;
    let gray: Vec<f32> = small
        .to_rgba8()
        .chunks_exact(4)
        .map(|pixel| {
            let alpha = f32::from(pixel[3]) / 255.0;
            let over_white = |value: u8| f32::from(value) * alpha + 255.0 * (1.0 - alpha);
            luma(
                over_white(pixel[0]),
                over_white(pixel[1]),
                over_white(pixel[2]),
            )
        })
        .collect();

    let cosines: Vec<Vec<f32>> = (0..PHASH_BLOCK)
        .map(|frequency| {
            (0..PHASH_SIDE)
                .map(|position| {
                    (PI * (2 * position + 1) as f32 * frequency as f32 / (2 * PHASH_SIDE) as f32)
                        .cos()
                })
                .collect()
        })
        .collect();

    let mut coefficients = [0.0f32; PHASH_BLOCK * PHASH_BLOCK];
    for v in 0..PHASH_BLOCK {
        for u in 0..PHASH_BLOCK {
            let mut sum = 0.0f32;
            for y in 0..PHASH_SIDE {
                for x in 0..PHASH_SIDE {
                    sum += gray[y * PHASH_SIDE + x] * cosines[u][x] * cosines[v][y];
                }
            }
            coefficients[v * PHASH_BLOCK + u] = sum;
        }
    }

    // The first coefficient is the mean brightness; it does not take part in
    // the median so that bright and dark copies hash alike.
    let mut sorted: Vec<f32> = coefficients[1..].to_vec();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2];
    let mut hash = 0u64;
    for (index, coefficient) in coefficients.iter().enumerate().skip(1) {
        if *coefficient > median {
            hash |= 1 << index;
        }
    }
    Ok(hash)
}

/// Number of differing bits between two perceptual hashes.
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::image::Depth;
    use crate::ops::filter::gaussian_blur;

    /// A smooth picture with distinct structure.
    fn picture(shift: u32) -> Image {
        let mut data = Vec::new();
        for y in 0..96u32 {
            for x in 0..128u32 {
                let wave = (((x + shift) as f32 / 9.0).sin() + (y as f32 / 13.0).cos()) * 60.0;
                let r = (128.0 + wave).clamp(0.0, 255.0) as u8;
                let g = ((x * 2) % 256) as u8;
                let b = ((y * 2 + x) % 256) as u8;
                data.extend_from_slice(&[r, g, b, 255]);
            }
        }
        Image::from_rgba8(128, 96, data).unwrap()
    }

    #[test]
    fn base83_encodes_digits() {
        let mut out = String::new();
        push_base83(&mut out, 0, 1);
        push_base83(&mut out, 82, 1);
        push_base83(&mut out, 83, 2);
        assert_eq!(out, "0~10");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn blurhash_has_the_documented_shape() {
        let hash = blurhash(&picture(0), 4, 3).unwrap();
        assert_eq!(hash.len(), 4 + 2 * 4 * 3);
        // 4x3 components encode as (4-1) + (3-1)*9 = 21 = 'L'.
        assert!(hash.starts_with('L'));
        assert!(hash.bytes().all(|byte| BASE83.contains(&byte)));
        assert_eq!(hash, blurhash(&picture(0), 4, 3).unwrap());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn blurhash_of_solid_colour_carries_it_in_dc() {
        let image = Image::new(50, 50, Depth::Eight, Color::rgb(255, 255, 255)).unwrap();
        let hash = blurhash(&image, 1, 1).unwrap();
        // Size flag 0, maximum 0, then 0xFFFFFF in base 83.
        assert_eq!(hash.len(), 6);
        assert!(hash.starts_with("00"));
        let value = hash[2..].bytes().fold(0u32, |sum, byte| {
            sum * 83 + BASE83.iter().position(|&digit| digit == byte).unwrap() as u32
        });
        assert_eq!(value, 0x00FF_FFFF);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn perceptual_hash_tolerates_small_changes() {
        let original = picture(0);
        let base = perceptual_hash(&original).unwrap();

        let mut soft = original.clone();
        gaussian_blur(&mut soft, 1.2);
        assert!(hamming(base, perceptual_hash(&soft).unwrap()) <= 10);

        let small = resize_to(&original, 64, 48, ResizeFilter::Lanczos3, false, None).unwrap();
        assert!(hamming(base, perceptual_hash(&small).unwrap()) <= 10);

        let wide = original.clone().into_depth(Depth::Sixteen);
        assert!(hamming(base, perceptual_hash(&wide).unwrap()) <= 2);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn perceptual_hash_separates_different_pictures() {
        let mut other = picture(0);
        crate::ops::geometry::flip_horizontal(&mut other);
        let distance = hamming(
            perceptual_hash(&picture(0)).unwrap(),
            perceptual_hash(&other).unwrap(),
        );
        assert!(distance >= 12, "{distance}");
    }

    #[test]
    fn hamming_counts_bits() {
        assert_eq!(hamming(0, 0), 0);
        assert_eq!(hamming(0b1011, 0b0001), 2);
        assert_eq!(hamming(u64::MAX, 0), 64);
    }
}
