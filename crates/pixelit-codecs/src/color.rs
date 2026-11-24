//! Colour management: conversion between an ICC profile and the sRGB working space.

use moxcms::{CmsError, ColorProfile, DataColorSpace, Layout, TransformOptions};
use pixelit_core::image::Pixels;
use pixelit_core::Image;
use rayon::prelude::*;

/// Samples converted per task: 16 384 RGBA pixels.
const CHUNK: usize = 4 * 16_384;

/// Levels of the probe grid that detects profiles equal to sRGB.
const PROBE_LEVELS: [u8; 5] = [0, 64, 128, 192, 255];

fn rgb_profile(icc: &[u8]) -> Option<ColorProfile> {
    let profile = ColorProfile::new_from_slice(icc).ok()?;
    (profile.color_space == DataColorSpace::Rgb).then_some(profile)
}

fn run<S>(
    data: &mut [S],
    apply: impl Fn(&[S], &mut [S]) -> Result<(), CmsError> + Sync,
) -> Result<(), CmsError>
where
    S: Copy + Send + Sync,
{
    data.par_chunks_mut(CHUNK).try_for_each(|chunk| {
        let source = chunk.to_vec();
        apply(&source, chunk)
    })
}

/// Whether converting between the profiles leaves 8-bit colours unchanged.
fn is_identity(source: &ColorProfile, target: &ColorProfile) -> bool {
    let Ok(transform) = source.create_transform_8bit(
        Layout::Rgba,
        target,
        Layout::Rgba,
        TransformOptions::default(),
    ) else {
        return false;
    };
    let mut probe = Vec::with_capacity(PROBE_LEVELS.len().pow(3) * 4);
    for red in PROBE_LEVELS {
        for green in PROBE_LEVELS {
            for blue in PROBE_LEVELS {
                probe.extend_from_slice(&[red, green, blue, 255]);
            }
        }
    }
    let mut out = vec![0_u8; probe.len()];
    transform.transform(&probe, &mut out).is_ok()
        && probe
            .iter()
            .zip(&out)
            .all(|(before, after)| before.abs_diff(*after) <= 1)
}

fn convert(
    image: &mut Image,
    source: &ColorProfile,
    target: &ColorProfile,
) -> Result<(), CmsError> {
    match image.pixels_mut() {
        Pixels::U8(data) => {
            let transform = source.create_transform_8bit(
                Layout::Rgba,
                target,
                Layout::Rgba,
                TransformOptions::default(),
            )?;
            run(data, |from, to| transform.transform(from, to))
        }
        Pixels::U16(data) => {
            let transform = source.create_transform_16bit(
                Layout::Rgba,
                target,
                Layout::Rgba,
                TransformOptions::default(),
            )?;
            run(data, |from, to| transform.transform(from, to))
        }
    }
}

/// Whether the profile describes RGB data that differs from sRGB.
pub fn needs_conversion(icc: &[u8]) -> bool {
    rgb_profile(icc).is_some_and(|profile| !is_identity(&profile, &ColorProfile::new_srgb()))
}

/// Converts pixels from the given profile to sRGB. Returns `true` when the
/// pixels changed. Profiles of other colour models and profiles equal to sRGB
/// leave the image as it is.
pub fn to_srgb(image: &mut Image, icc: &[u8]) -> bool {
    let Some(profile) = rgb_profile(icc) else {
        return false;
    };
    let srgb = ColorProfile::new_srgb();
    if is_identity(&profile, &srgb) {
        return false;
    }
    convert(image, &profile, &srgb).is_ok()
}

/// Converts sRGB pixels into the given profile. Returns `true` when the pixels
/// are now in that profile and the profile has to be embedded.
pub fn from_srgb(image: &mut Image, icc: &[u8]) -> bool {
    let Some(profile) = rgb_profile(icc) else {
        return false;
    };
    let srgb = ColorProfile::new_srgb();
    if is_identity(&srgb, &profile) {
        return false;
    }
    convert(image, &srgb, &profile).is_ok()
}

/// The bytes of a standard sRGB profile.
pub fn srgb_profile() -> Option<Vec<u8>> {
    ColorProfile::new_srgb().encode().ok()
}

#[cfg(test)]
mod tests {
    use moxcms::ColorProfile;
    use pixelit_core::{Color, Depth, Image};

    use super::*;

    fn p3() -> Vec<u8> {
        ColorProfile::new_display_p3().encode().unwrap()
    }

    #[test]
    fn srgb_profile_is_identity() {
        let icc = srgb_profile().unwrap();
        assert!(!needs_conversion(&icc));
        let mut image = Image::new(4, 4, Depth::Eight, Color::rgb(200, 30, 90)).unwrap();
        assert!(!to_srgb(&mut image, &icc));
        assert_eq!(image.pixel8(0, 0), [200, 30, 90, 255]);
    }

    #[test]
    fn display_p3_changes_saturated_colours() {
        let icc = p3();
        assert!(needs_conversion(&icc));
        let mut image = Image::new(4, 4, Depth::Eight, Color::rgb(200, 60, 60)).unwrap();
        assert!(to_srgb(&mut image, &icc));
        let [red, green, blue, alpha] = image.pixel8(1, 1);
        // P3 red is more saturated than sRGB red with the same numbers.
        assert!(red > 200, "red {red}");
        assert!(green < 60, "green {green}");
        assert!(blue < 60, "blue {blue}");
        assert_eq!(alpha, 255);
    }

    #[test]
    fn round_trip_is_close() {
        let icc = p3();
        let mut image = Image::new(8, 8, Depth::Sixteen, Color::rgb(120, 140, 90)).unwrap();
        assert!(from_srgb(&mut image, &icc));
        assert!(to_srgb(&mut image, &icc));
        let [red, green, blue, _] = image.pixel8(3, 3);
        assert!(red.abs_diff(120) <= 1 && green.abs_diff(140) <= 1 && blue.abs_diff(90) <= 1);
    }

    #[test]
    fn garbage_is_ignored() {
        let mut image = Image::new(2, 2, Depth::Eight, Color::rgb(1, 2, 3)).unwrap();
        assert!(!to_srgb(&mut image, b"not a profile"));
        assert!(!needs_conversion(&[]));
        assert_eq!(image.pixel8(0, 0), [1, 2, 3, 255]);
    }
}
