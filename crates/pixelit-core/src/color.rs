//! Colour values and the sRGB transfer curve.
//!
//! The working colour space of PixelIt is sRGB. Codecs convert every decoded
//! image into it once, so all operations here assume sRGB-encoded samples.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::error::Error;

/// An sRGB colour with straight alpha, written as `#rrggbb` or `#rrggbbaa`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const WHITE: Self = Self::rgb(255, 255, 255);
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.trim().strip_prefix('#')?;
        if !hex.is_ascii() {
            return None;
        }
        let byte = |index: usize| u8::from_str_radix(hex.get(index..index + 2)?, 16).ok();
        let nibble = |index: usize| {
            let value = u8::from_str_radix(hex.get(index..=index)?, 16).ok()?;
            Some(value * 17)
        };
        match hex.len() {
            3 => Some(Self::rgb(nibble(0)?, nibble(1)?, nibble(2)?)),
            4 => Some(Self::rgba(nibble(0)?, nibble(1)?, nibble(2)?, nibble(3)?)),
            6 => Some(Self::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Self::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    /// The colour as unit floats `[r, g, b, a]`.
    pub fn to_unit(self) -> [f32; 4] {
        [
            f32::from(self.r) / 255.0,
            f32::from(self.g) / 255.0,
            f32::from(self.b) / 255.0,
            f32::from(self.a) / 255.0,
        ]
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::WHITE
    }
}

impl TryFrom<String> for Color {
    type Error = Error;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text).ok_or_else(|| Error::new("badColor").with("value", text))
    }
}

impl From<Color> for String {
    fn from(color: Color) -> Self {
        color.to_hex()
    }
}

/// sRGB-encoded unit value to linear light.
pub fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light to an sRGB-encoded unit value.
pub fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// Rec. 709 luma of sRGB-encoded components.
pub fn luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// 8-bit sRGB to 16-bit linear light.
pub fn srgb8_to_linear16() -> &'static [u16; 256] {
    static TABLE: OnceLock<[u16; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u16; 256];
        for (index, slot) in table.iter_mut().enumerate() {
            let linear = srgb_to_linear(index as f32 / 255.0);
            *slot = (linear * 65535.0 + 0.5) as u16;
        }
        table
    })
}

/// 16-bit linear light to 8-bit sRGB.
pub fn linear16_to_srgb8() -> &'static [u8] {
    static TABLE: OnceLock<Vec<u8>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=65535u32)
            .map(|index| {
                let encoded = linear_to_srgb(index as f32 / 65535.0);
                (encoded * 255.0 + 0.5).clamp(0.0, 255.0) as u8
            })
            .collect()
    })
}

/// 16-bit sRGB to linear light as a unit float.
pub fn srgb16_to_linear() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=65535u32)
            .map(|index| srgb_to_linear(index as f32 / 65535.0))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_hex_forms() {
        assert_eq!(Color::parse("#fff"), Some(Color::WHITE));
        assert_eq!(Color::parse("#0008"), Some(Color::rgba(0, 0, 0, 136)));
        assert_eq!(Color::parse("#102030"), Some(Color::rgb(16, 32, 48)));
        assert_eq!(
            Color::parse(" #10203040 "),
            Some(Color::rgba(16, 32, 48, 64))
        );
        assert_eq!(Color::parse("102030"), None);
        assert_eq!(Color::parse("#12345"), None);
        assert_eq!(Color::parse("#ggg"), None);
        assert_eq!(Color::parse("#яяя"), None);
    }

    #[test]
    fn hex_round_trip() {
        for color in [Color::rgb(1, 2, 3), Color::rgba(250, 128, 0, 7)] {
            assert_eq!(Color::parse(&color.to_hex()), Some(color));
        }
    }

    #[test]
    fn serde_uses_hex_strings() {
        let json = serde_json::to_string(&Color::rgb(255, 0, 16)).unwrap();
        assert_eq!(json, r##""#ff0010""##);
        assert!(serde_json::from_str::<Color>(r#""red""#).is_err());
    }

    #[test]
    fn transfer_curve_round_trip() {
        for step in 0..=100 {
            let value = step as f32 / 100.0;
            let back = linear_to_srgb(srgb_to_linear(value));
            assert!((back - value).abs() < 1e-4, "{value} -> {back}");
        }
    }

    #[test]
    fn tables_round_trip_every_8_bit_value() {
        let forward = srgb8_to_linear16();
        let backward = linear16_to_srgb8();
        for value in 0..=255usize {
            assert_eq!(usize::from(backward[usize::from(forward[value])]), value);
        }
    }
}
