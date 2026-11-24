//! Image file formats known to PixelIt and what each of them can carry.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageFormat {
    Jpeg,
    Png,
    Webp,
    Avif,
    Jxl,
    Tiff,
    Bmp,
    Gif,
    Ico,
    Heic,
    Raw,
    Svg,
    Psd,
    Pdf,
}

/// Extensions of camera RAW files.
const RAW_EXTENSIONS: &[&str] = &[
    "3fr", "ari", "arw", "bay", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "iiq", "k25",
    "kdc", "mef", "mos", "mrw", "nef", "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2",
    "srf", "srw", "x3f",
];

impl ImageFormat {
    /// Formats PixelIt writes.
    pub const OUTPUT: &'static [Self] = &[
        Self::Jpeg,
        Self::Png,
        Self::Webp,
        Self::Avif,
        Self::Jxl,
        Self::Tiff,
        Self::Bmp,
        Self::Gif,
        Self::Ico,
    ];

    /// Stable lower-case name, equal to the serialized value.
    pub fn name(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Avif => "avif",
            Self::Jxl => "jxl",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
            Self::Gif => "gif",
            Self::Ico => "ico",
            Self::Heic => "heic",
            Self::Raw => "raw",
            Self::Svg => "svg",
            Self::Psd => "psd",
            Self::Pdf => "pdf",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        [
            Self::Jpeg,
            Self::Png,
            Self::Webp,
            Self::Avif,
            Self::Jxl,
            Self::Tiff,
            Self::Bmp,
            Self::Gif,
            Self::Ico,
            Self::Heic,
            Self::Raw,
            Self::Svg,
            Self::Psd,
            Self::Pdf,
        ]
        .into_iter()
        .find(|format| format.name() == name)
        .or_else(|| Self::from_extension(&name))
    }

    /// The format a file extension stands for; the extension has no dot.
    pub fn from_extension(extension: &str) -> Option<Self> {
        let extension = extension.to_ascii_lowercase();
        let format = match extension.as_str() {
            "jpg" | "jpeg" | "jpe" | "jfif" => Self::Jpeg,
            "png" | "apng" => Self::Png,
            "webp" => Self::Webp,
            "avif" => Self::Avif,
            "jxl" => Self::Jxl,
            "tif" | "tiff" => Self::Tiff,
            "bmp" | "dib" => Self::Bmp,
            "gif" => Self::Gif,
            "ico" | "cur" => Self::Ico,
            "heic" | "heif" | "hif" => Self::Heic,
            "svg" | "svgz" => Self::Svg,
            "psd" | "psb" => Self::Psd,
            "pdf" => Self::Pdf,
            other if RAW_EXTENSIONS.contains(&other) => Self::Raw,
            _ => return None,
        };
        Some(format)
    }

    /// Guesses the format from the first bytes of a file.
    pub fn sniff(head: &[u8]) -> Option<Self> {
        if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(Self::Jpeg);
        }
        if head.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some(Self::Png);
        }
        if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
            return Some(Self::Webp);
        }
        if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
            return Some(Self::Gif);
        }
        if head.starts_with(b"BM") {
            return Some(Self::Bmp);
        }
        if head.starts_with(&[0xFF, 0x0A])
            || head.starts_with(&[
                0, 0, 0, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A,
            ])
        {
            return Some(Self::Jxl);
        }
        if head.starts_with(b"8BPS") {
            return Some(Self::Psd);
        }
        if head.starts_with(b"%PDF") {
            return Some(Self::Pdf);
        }
        if head.starts_with(&[0, 0, 1, 0]) || head.starts_with(&[0, 0, 2, 0]) {
            return Some(Self::Ico);
        }
        if head.len() >= 12 && &head[4..8] == b"ftyp" {
            return match &head[8..12] {
                b"avif" | b"avis" => Some(Self::Avif),
                b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"mif1" | b"msf1" => {
                    Some(Self::Heic)
                }
                b"crx " => Some(Self::Raw),
                _ => None,
            };
        }
        if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
            // Many RAW formats are TIFF containers; the extension decides.
            return Some(Self::Tiff);
        }
        let text = String::from_utf8_lossy(&head[..head.len().min(512)]);
        let text = text.trim_start_matches('\u{feff}').trim_start();
        if text.starts_with("<svg") || (text.starts_with("<?xml") && text.contains("<svg")) {
            return Some(Self::Svg);
        }
        None
    }

    /// Preferred file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Tiff => "tif",
            other => other.name(),
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
            Self::Jxl => "image/jxl",
            Self::Tiff => "image/tiff",
            Self::Bmp => "image/bmp",
            Self::Gif => "image/gif",
            Self::Ico => "image/x-icon",
            Self::Heic => "image/heic",
            Self::Raw => "image/x-raw",
            Self::Svg => "image/svg+xml",
            Self::Psd => "image/vnd.adobe.photoshop",
            Self::Pdf => "application/pdf",
        }
    }

    pub fn can_encode(self) -> bool {
        Self::OUTPUT.contains(&self)
    }

    /// Whether the written file can hold transparency.
    pub fn supports_alpha(self) -> bool {
        matches!(
            self,
            Self::Png | Self::Webp | Self::Avif | Self::Jxl | Self::Tiff | Self::Gif | Self::Ico
        )
    }

    /// Whether PixelIt writes animations in this format.
    pub fn supports_animation(self) -> bool {
        matches!(self, Self::Gif | Self::Png | Self::Webp)
    }

    /// Whether PixelIt writes 16 bits per channel in this format.
    pub fn supports_16_bit(self) -> bool {
        matches!(self, Self::Png | Self::Tiff | Self::Jxl)
    }

    /// Whether the format has a quality setting.
    pub fn is_lossy(self) -> bool {
        matches!(self, Self::Jpeg | Self::Webp | Self::Avif | Self::Jxl)
    }

    /// Whether the format can also be written without loss.
    pub fn supports_lossless(self) -> bool {
        !matches!(self, Self::Jpeg | Self::Avif)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_to_formats() {
        assert_eq!(ImageFormat::from_extension("JPG"), Some(ImageFormat::Jpeg));
        assert_eq!(ImageFormat::from_extension("heif"), Some(ImageFormat::Heic));
        assert_eq!(ImageFormat::from_extension("cr3"), Some(ImageFormat::Raw));
        assert_eq!(ImageFormat::from_extension("txt"), None);
    }

    #[test]
    fn names_round_trip_and_match_serde() {
        for &format in ImageFormat::OUTPUT {
            assert_eq!(ImageFormat::from_name(format.name()), Some(format));
            let json = serde_json::to_string(&format).unwrap();
            assert_eq!(json, format!("\"{}\"", format.name()));
            assert_eq!(
                ImageFormat::from_extension(format.extension()),
                Some(format)
            );
        }
        assert_eq!(ImageFormat::from_name("jpg"), Some(ImageFormat::Jpeg));
    }

    #[test]
    fn sniffs_signatures() {
        assert_eq!(
            ImageFormat::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(
            ImageFormat::sniff(b"\x89PNG\r\n\x1a\n...."),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            ImageFormat::sniff(b"RIFF\0\0\0\0WEBPVP8 "),
            Some(ImageFormat::Webp)
        );
        assert_eq!(ImageFormat::sniff(b"GIF89a"), Some(ImageFormat::Gif));
        assert_eq!(
            ImageFormat::sniff(b"\0\0\0\x20ftypavif"),
            Some(ImageFormat::Avif)
        );
        assert_eq!(
            ImageFormat::sniff(b"\0\0\0\x20ftypheic"),
            Some(ImageFormat::Heic)
        );
        assert_eq!(
            ImageFormat::sniff(b"  <svg xmlns='x'>"),
            Some(ImageFormat::Svg)
        );
        assert_eq!(ImageFormat::sniff(b"%PDF-1.7"), Some(ImageFormat::Pdf));
        assert_eq!(ImageFormat::sniff(b"hello"), None);
        assert_eq!(ImageFormat::sniff(b""), None);
    }

    #[test]
    fn only_output_formats_encode() {
        assert!(ImageFormat::Jxl.can_encode());
        assert!(!ImageFormat::Heic.can_encode());
        assert!(!ImageFormat::Jpeg.supports_alpha());
        assert!(ImageFormat::Webp.supports_animation());
    }
}
