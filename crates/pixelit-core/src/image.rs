//! Pixel buffers.
//!
//! Every image is stored as interleaved RGBA with straight alpha, 8 or 16 bits
//! per channel, in the sRGB working space. The `alpha` flag tells whether the
//! alpha channel carries information; encoders drop the channel when it does
//! not. One layout for all images keeps every operation simple and uniform.

use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::error::{Error, Result};

/// Samples per pixel.
pub const CHANNELS: usize = 4;

/// Bits per channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Depth {
    #[default]
    Eight,
    Sixteen,
}

impl Depth {
    pub fn bits(self) -> u32 {
        match self {
            Self::Eight => 8,
            Self::Sixteen => 16,
        }
    }
}

/// A channel value: `u8` or `u16`.
pub trait Sample: Copy + Default + PartialEq + PartialOrd + Send + Sync + 'static {
    /// The largest value, as `u32`.
    const MAX: u32;

    /// Converts with saturation at [`Sample::MAX`].
    fn from_u32(value: u32) -> Self;

    fn to_u32(self) -> u32;

    fn max_value() -> Self {
        Self::from_u32(Self::MAX)
    }

    /// The value in the `0.0..=1.0` range.
    fn to_unit(self) -> f32 {
        self.to_u32() as f32 / Self::MAX as f32
    }

    /// Converts a unit value with rounding and clamping.
    fn from_unit(value: f32) -> Self {
        let scaled = value * Self::MAX as f32 + 0.5;
        if scaled.is_nan() || scaled <= 0.0 {
            Self::from_u32(0)
        } else if scaled >= Self::MAX as f32 {
            Self::max_value()
        } else {
            Self::from_u32(scaled as u32)
        }
    }
}

impl Sample for u8 {
    const MAX: u32 = 255;

    fn from_u32(value: u32) -> Self {
        value.min(255) as u8
    }

    fn to_u32(self) -> u32 {
        u32::from(self)
    }
}

impl Sample for u16 {
    const MAX: u32 = 65535;

    fn from_u32(value: u32) -> Self {
        value.min(65535) as u16
    }

    fn to_u32(self) -> u32 {
        u32::from(self)
    }
}

/// Interleaved RGBA samples.
#[derive(Clone, Debug, PartialEq)]
pub enum Pixels {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) alpha: bool,
    pub(crate) pixels: Pixels,
}

/// Number of samples of a `width` x `height` image, or an error when the size
/// is empty or does not fit into memory addressing.
pub fn sample_count(width: u32, height: u32) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(Error::new("emptyImage"));
    }
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(CHANNELS))
        .ok_or_else(|| {
            Error::new("tooLarge")
                .with("width", width)
                .with("height", height)
        })
}

fn check_len(width: u32, height: u32, channels: usize, len: usize) -> Result<()> {
    let expected = sample_count(width, height)? / CHANNELS * channels;
    if len == expected {
        Ok(())
    } else {
        Err(Error::new("badImage")
            .with("expected", expected)
            .with("actual", len))
    }
}

/// Expands 1, 2 or 3 channel data to RGBA.
fn expand<S: Sample>(data: &[S], channels: usize) -> Vec<S> {
    let max = S::max_value();
    let mut out = Vec::with_capacity(data.len() / channels * CHANNELS);
    match channels {
        1 => {
            for &value in data {
                out.extend_from_slice(&[value, value, value, max]);
            }
        }
        2 => {
            for pair in data.chunks_exact(2) {
                out.extend_from_slice(&[pair[0], pair[0], pair[0], pair[1]]);
            }
        }
        _ => {
            for rgb in data.chunks_exact(3) {
                out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], max]);
            }
        }
    }
    out
}

fn drop_alpha<S: Sample>(data: &[S]) -> Vec<S> {
    let mut out = Vec::with_capacity(data.len() / CHANNELS * 3);
    for pixel in data.chunks_exact(CHANNELS) {
        out.extend_from_slice(&pixel[..3]);
    }
    out
}

fn all_opaque<S: Sample>(data: &[S]) -> bool {
    let max = S::max_value();
    data.chunks_exact(CHANNELS).all(|pixel| pixel[3] == max)
}

fn all_gray<S: Sample>(data: &[S]) -> bool {
    data.chunks_exact(CHANNELS)
        .all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2])
}

fn flatten_onto<S: Sample>(data: &mut [S], background: Color) {
    let max = S::MAX;
    let back = [
        S::from_unit(f32::from(background.r) / 255.0).to_u32(),
        S::from_unit(f32::from(background.g) / 255.0).to_u32(),
        S::from_unit(f32::from(background.b) / 255.0).to_u32(),
    ];
    for pixel in data.chunks_exact_mut(CHANNELS) {
        let alpha = pixel[3].to_u32();
        if alpha != max {
            for channel in 0..3 {
                let value = pixel[channel].to_u32();
                // u64 keeps the 16-bit products in range.
                let mixed = (u64::from(value) * u64::from(alpha)
                    + u64::from(back[channel]) * u64::from(max - alpha)
                    + u64::from(max / 2))
                    / u64::from(max);
                pixel[channel] = S::from_u32(mixed as u32);
            }
            pixel[3] = S::max_value();
        }
    }
}

impl Image {
    /// A solid image.
    pub fn new(width: u32, height: u32, depth: Depth, fill: Color) -> Result<Self> {
        let count = sample_count(width, height)?;
        let pixels = match depth {
            Depth::Eight => {
                let mut data = Vec::with_capacity(count);
                for _ in 0..count / CHANNELS {
                    data.extend_from_slice(&[fill.r, fill.g, fill.b, fill.a]);
                }
                Pixels::U8(data)
            }
            Depth::Sixteen => {
                let wide = |value: u8| u16::from(value) * 257;
                let mut data = Vec::with_capacity(count);
                for _ in 0..count / CHANNELS {
                    data.extend_from_slice(&[
                        wide(fill.r),
                        wide(fill.g),
                        wide(fill.b),
                        wide(fill.a),
                    ]);
                }
                Pixels::U16(data)
            }
        };
        Ok(Self {
            width,
            height,
            alpha: fill.a != 255,
            pixels,
        })
    }

    pub fn from_rgba8(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        check_len(width, height, 4, data.len())?;
        Ok(Self {
            width,
            height,
            alpha: true,
            pixels: Pixels::U8(data),
        })
    }

    pub fn from_rgba16(width: u32, height: u32, data: Vec<u16>) -> Result<Self> {
        check_len(width, height, 4, data.len())?;
        Ok(Self {
            width,
            height,
            alpha: true,
            pixels: Pixels::U16(data),
        })
    }

    /// Builds an image from 8-bit data with 1 (gray), 2 (gray + alpha),
    /// 3 (RGB) or 4 (RGBA) channels.
    pub fn from_channels8(width: u32, height: u32, channels: usize, data: &[u8]) -> Result<Self> {
        if !(1..=4).contains(&channels) {
            return Err(Error::new("badImage").with("channels", channels));
        }
        check_len(width, height, channels, data.len())?;
        let pixels = if channels == 4 {
            data.to_vec()
        } else {
            expand(data, channels)
        };
        Ok(Self {
            width,
            height,
            alpha: channels == 2 || channels == 4,
            pixels: Pixels::U8(pixels),
        })
    }

    /// Builds an image from 16-bit data with 1, 2, 3 or 4 channels.
    pub fn from_channels16(width: u32, height: u32, channels: usize, data: &[u16]) -> Result<Self> {
        if !(1..=4).contains(&channels) {
            return Err(Error::new("badImage").with("channels", channels));
        }
        check_len(width, height, channels, data.len())?;
        let pixels = if channels == 4 {
            data.to_vec()
        } else {
            expand(data, channels)
        };
        Ok(Self {
            width,
            height,
            alpha: channels == 2 || channels == 4,
            pixels: Pixels::U16(pixels),
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn depth(&self) -> Depth {
        match self.pixels {
            Pixels::U8(_) => Depth::Eight,
            Pixels::U16(_) => Depth::Sixteen,
        }
    }

    /// Whether the alpha channel carries information.
    pub fn has_alpha(&self) -> bool {
        self.alpha
    }

    pub fn set_has_alpha(&mut self, alpha: bool) {
        self.alpha = alpha;
    }

    pub fn pixels(&self) -> &Pixels {
        &self.pixels
    }

    pub fn pixels_mut(&mut self) -> &mut Pixels {
        &mut self.pixels
    }

    pub fn into_pixels(self) -> Pixels {
        self.pixels
    }

    /// Memory taken by the samples, in bytes.
    pub fn byte_size(&self) -> usize {
        match &self.pixels {
            Pixels::U8(data) => data.len(),
            Pixels::U16(data) => data.len() * 2,
        }
    }

    /// Whether every pixel is fully opaque.
    pub fn is_opaque(&self) -> bool {
        match &self.pixels {
            Pixels::U8(data) => all_opaque(data),
            Pixels::U16(data) => all_opaque(data),
        }
    }

    /// Whether every pixel has equal red, green and blue.
    pub fn is_gray(&self) -> bool {
        match &self.pixels {
            Pixels::U8(data) => all_gray(data),
            Pixels::U16(data) => all_gray(data),
        }
    }

    /// Clears the alpha flag when no pixel is transparent.
    pub fn normalize_alpha(&mut self) {
        if self.alpha && self.is_opaque() {
            self.alpha = false;
        }
    }

    /// Composites the image onto a solid background and makes it opaque.
    pub fn flatten(&mut self, background: Color) {
        if self.alpha {
            match &mut self.pixels {
                Pixels::U8(data) => flatten_onto(data, background),
                Pixels::U16(data) => flatten_onto(data, background),
            }
            self.alpha = false;
        }
    }

    /// The image at another bit depth.
    #[must_use]
    pub fn into_depth(self, depth: Depth) -> Self {
        let pixels = match (self.pixels, depth) {
            (Pixels::U8(data), Depth::Sixteen) => {
                Pixels::U16(data.iter().map(|&value| u16::from(value) * 257).collect())
            }
            (Pixels::U16(data), Depth::Eight) => Pixels::U8(
                data.iter()
                    .map(|&value| ((u32::from(value) + 128) / 257) as u8)
                    .collect(),
            ),
            (pixels, _) => pixels,
        };
        Self {
            width: self.width,
            height: self.height,
            alpha: self.alpha,
            pixels,
        }
    }

    /// RGBA, 8 bits per channel.
    pub fn to_rgba8(&self) -> Vec<u8> {
        match &self.pixels {
            Pixels::U8(data) => data.clone(),
            Pixels::U16(data) => data
                .iter()
                .map(|&value| ((u32::from(value) + 128) / 257) as u8)
                .collect(),
        }
    }

    /// RGB, 8 bits per channel. Alpha is dropped without compositing.
    pub fn to_rgb8(&self) -> Vec<u8> {
        match &self.pixels {
            Pixels::U8(data) => drop_alpha(data),
            Pixels::U16(_) => drop_alpha(&self.to_rgba8()),
        }
    }

    /// RGBA, 16 bits per channel.
    pub fn to_rgba16(&self) -> Vec<u16> {
        match &self.pixels {
            Pixels::U8(data) => data.iter().map(|&value| u16::from(value) * 257).collect(),
            Pixels::U16(data) => data.clone(),
        }
    }

    /// RGB, 16 bits per channel. Alpha is dropped without compositing.
    pub fn to_rgb16(&self) -> Vec<u16> {
        match &self.pixels {
            Pixels::U8(_) => drop_alpha(&self.to_rgba16()),
            Pixels::U16(data) => drop_alpha(data),
        }
    }

    /// The pixel at `(x, y)` scaled to 8 bits. Panics outside the image.
    pub fn pixel8(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(x < self.width && y < self.height, "pixel outside the image");
        let index = (y as usize * self.width as usize + x as usize) * CHANNELS;
        match &self.pixels {
            Pixels::U8(data) => [
                data[index],
                data[index + 1],
                data[index + 2],
                data[index + 3],
            ],
            Pixels::U16(data) => {
                let narrow = |value: u16| ((u32::from(value) + 128) / 257) as u8;
                [
                    narrow(data[index]),
                    narrow(data[index + 1]),
                    narrow(data[index + 2]),
                    narrow(data[index + 3]),
                ]
            }
        }
    }
}

/// One frame of an animation with its display time.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub image: Image,
    pub delay_ms: u32,
}

/// A decoded animation. Every frame is a full canvas of the same size.
#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    pub frames: Vec<Frame>,
    /// Number of repetitions; `0` plays forever.
    pub loop_count: u32,
}

impl Animation {
    pub fn duration_ms(&self) -> u64 {
        self.frames
            .iter()
            .map(|frame| u64::from(frame.delay_ms))
            .sum()
    }
}

/// A decoded file: a still image or an animation.
#[derive(Clone, Debug, PartialEq)]
pub enum Media {
    Still(Image),
    Animated(Animation),
}

impl Media {
    /// The still image or the first frame of the animation.
    pub fn first(&self) -> Option<&Image> {
        match self {
            Self::Still(image) => Some(image),
            Self::Animated(animation) => animation.frames.first().map(|frame| &frame.image),
        }
    }

    pub fn into_first(self) -> Option<Image> {
        match self {
            Self::Still(image) => Some(image),
            Self::Animated(animation) => {
                animation.frames.into_iter().next().map(|frame| frame.image)
            }
        }
    }

    pub fn frame_count(&self) -> usize {
        match self {
            Self::Still(_) => 1,
            Self::Animated(animation) => animation.frames.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_mismatched_buffers() {
        assert_eq!(
            Image::from_rgba8(0, 4, vec![]).unwrap_err().code,
            "emptyImage"
        );
        assert_eq!(
            Image::from_rgba8(2, 2, vec![0; 15]).unwrap_err().code,
            "badImage"
        );
        assert_eq!(
            Image::from_channels8(1, 1, 5, &[0; 5]).unwrap_err().code,
            "badImage"
        );
    }

    #[test]
    fn expands_gray_and_rgb() {
        let gray = Image::from_channels8(2, 1, 1, &[10, 20]).unwrap();
        assert_eq!(gray.to_rgba8(), vec![10, 10, 10, 255, 20, 20, 20, 255]);
        assert!(!gray.has_alpha());
        assert!(gray.is_gray());

        let gray_alpha = Image::from_channels8(1, 1, 2, &[10, 99]).unwrap();
        assert_eq!(gray_alpha.to_rgba8(), vec![10, 10, 10, 99]);
        assert!(gray_alpha.has_alpha());

        let rgb = Image::from_channels16(1, 1, 3, &[1, 2, 3]).unwrap();
        assert_eq!(rgb.to_rgba16(), vec![1, 2, 3, 65535]);
        assert!(!rgb.is_gray());
    }

    #[test]
    fn depth_conversion_round_trips_8_bit_values() {
        let data: Vec<u8> = (0..=255)
            .flat_map(|value| [value, value, value, 255])
            .collect();
        let image = Image::from_rgba8(256, 1, data.clone()).unwrap();
        let wide = image.clone().into_depth(Depth::Sixteen);
        assert_eq!(wide.depth(), Depth::Sixteen);
        assert_eq!(wide.into_depth(Depth::Eight).to_rgba8(), data);
    }

    #[test]
    fn flatten_mixes_with_background() {
        let mut image = Image::from_rgba8(2, 1, vec![255, 0, 0, 0, 0, 0, 0, 128]).unwrap();
        image.flatten(Color::WHITE);
        assert!(!image.has_alpha());
        assert_eq!(image.pixel8(0, 0), [255, 255, 255, 255]);
        assert_eq!(image.pixel8(1, 0), [127, 127, 127, 255]);
    }

    #[test]
    fn normalize_alpha_drops_unused_channel() {
        let mut image = Image::from_rgba8(1, 1, vec![1, 2, 3, 255]).unwrap();
        assert!(image.has_alpha());
        image.normalize_alpha();
        assert!(!image.has_alpha());
    }

    #[test]
    fn sample_from_unit_clamps() {
        assert_eq!(u8::from_unit(-1.0), 0);
        assert_eq!(u8::from_unit(2.0), 255);
        assert_eq!(u8::from_unit(f32::NAN), 0);
        assert_eq!(u16::from_unit(1.0), 65535);
        assert_eq!(u8::from_unit(0.5), 128);
    }
}
