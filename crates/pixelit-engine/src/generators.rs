//! Sets of files made from one picture: icons, social network sizes and
//! responsive images for the web.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use pixelit_codecs::{encode, EncodeOptions};
use pixelit_core::hash::blurhash;
use pixelit_core::ops::draw::composite;
use pixelit_core::ops::resize::{fit_within, resize, resize_to};
use pixelit_core::pipeline::{Conflict, Effort, Resize, ResizeFilter, ResizeMode};
use pixelit_core::{CancelToken, Color, Depth, Error, Image, ImageFormat, Result};
use serde::{Deserialize, Serialize};

use crate::fsops;
use crate::process::Runner;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IconPreset {
    Favicon,
    Pwa,
    Windows,
    Android,
    Ios,
    Macos,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IconRequest {
    pub source: PathBuf,
    /// Folder the set is written into; a new subfolder is made inside it.
    pub folder: PathBuf,
    pub presets: Vec<IconPreset>,
    /// Empty space around the picture as a percentage of the icon side.
    pub padding: f32,
    /// Colour behind the picture; `None` keeps transparency where the
    /// platform allows it.
    pub background: Option<Color>,
    /// Name of the application, used in the web manifest.
    pub app_name: String,
}

impl Default for IconRequest {
    fn default() -> Self {
        Self {
            source: PathBuf::new(),
            folder: PathBuf::new(),
            presets: vec![IconPreset::Favicon, IconPreset::Pwa],
            padding: 0.0,
            background: None,
            app_name: String::new(),
        }
    }
}

/// Files a generator wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Generated {
    pub folder: PathBuf,
    pub files: Vec<PathBuf>,
    /// HTML to paste into a page, when the set has one.
    pub snippet: String,
    /// BlurHash placeholder of the picture, when the set has one.
    pub blurhash: String,
}

/// A folder that does not exist yet: `base`, `base (2)`, ...
fn fresh_folder(parent: &Path, name: &str) -> Result<PathBuf> {
    for number in 1..10_000u32 {
        let candidate = if number == 1 {
            parent.join(name)
        } else {
            parent.join(format!("{name} ({number})"))
        };
        if !candidate.exists() {
            fs::create_dir_all(&candidate)
                .map_err(|error| Error::from(error).with("path", candidate.display()))?;
            return Ok(candidate);
        }
    }
    Err(Error::new("io").with("path", parent.display()))
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| String::from("image"))
}

/// The picture fitted into a square canvas.
fn square(source: &Image, side: u32, padding: f32, background: Option<Color>) -> Result<Image> {
    let padding = f64::from(padding.clamp(0.0, 45.0)) / 100.0;
    let inner = ((f64::from(side) * (1.0 - 2.0 * padding)).round() as u32).max(1);
    let factor = (f64::from(inner) / f64::from(source.width()))
        .min(f64::from(inner) / f64::from(source.height()));
    let width = ((f64::from(source.width()) * factor).round() as u32).clamp(1, side);
    let height = ((f64::from(source.height()) * factor).round() as u32).clamp(1, side);
    let layer = resize_to(source, width, height, ResizeFilter::Lanczos3, true, None)?;
    let mut canvas = Image::new(
        side,
        side,
        Depth::Eight,
        background.unwrap_or(Color::TRANSPARENT),
    )?;
    canvas.set_has_alpha(true);
    composite(
        &mut canvas,
        &layer,
        i64::from((side - width) / 2),
        i64::from((side - height) / 2),
        1.0,
    );
    canvas.normalize_alpha();
    Ok(canvas)
}

fn png(image: &Image) -> Result<Vec<u8>> {
    let mut options = EncodeOptions::new(ImageFormat::Png);
    options.effort = Effort::Balanced;
    encode(image, &options, &CancelToken::new())
}

/// An ICO file holding several PNG-compressed sizes.
fn ico(images: &[Image]) -> Result<Vec<u8>> {
    const HEADER: usize = 6;
    const ENTRY: usize = 16;
    let payloads: Vec<Vec<u8>> = images.iter().map(png).collect::<Result<_>>()?;
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = HEADER + ENTRY * images.len();
    for (image, payload) in images.iter().zip(&payloads) {
        // A side of 256 is stored as 0.
        let side = |value: u32| if value >= 256 { 0 } else { value as u8 };
        out.push(side(image.width()));
        out.push(side(image.height()));
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += payload.len();
    }
    for payload in &payloads {
        out.extend_from_slice(payload);
    }
    Ok(out)
}

struct Writer {
    folder: PathBuf,
    files: Vec<PathBuf>,
}

impl Writer {
    fn put(&mut self, name: &str, bytes: &[u8]) -> Result<()> {
        let written = fsops::write_file(&self.folder.join(name), bytes, Conflict::Rename)?;
        self.files.push(written.path().to_path_buf());
        Ok(())
    }
}

fn load(runner: &Runner, source: &Path, cancel: &CancelToken) -> Result<Image> {
    let decoded = runner.load(source, 0, cancel)?;
    let image = decoded
        .media
        .into_first()
        .ok_or_else(|| Error::new("emptyImage").with("path", source.display()))?;
    Ok(image.into_depth(Depth::Eight))
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Writes icon sets for the chosen platforms.
pub fn icons(runner: &Runner, request: &IconRequest, cancel: &CancelToken) -> Result<Generated> {
    let source = load(runner, &request.source, cancel)?;
    let folder = fresh_folder(&request.folder, &format!("{}-icons", stem(&request.source)))?;
    let mut writer = Writer {
        folder: folder.clone(),
        files: Vec::new(),
    };
    let padding = request.padding;
    let free = request.background;
    // Platforms that show icons on their own backdrop need opaque files.
    let backdrop = request.background.unwrap_or(Color::WHITE);
    let solid = Some(backdrop);
    let mut snippet = String::new();

    for preset in &request.presets {
        cancel.check()?;
        match preset {
            IconPreset::Favicon => {
                let sizes = [16, 32, 48]
                    .iter()
                    .map(|side| square(&source, *side, padding, free))
                    .collect::<Result<Vec<_>>>()?;
                writer.put("favicon.ico", &ico(&sizes)?)?;
                for image in &sizes {
                    let name = format!("favicon-{0}x{0}.png", image.width());
                    writer.put(&name, &png(image)?)?;
                }
                writer.put(
                    "apple-touch-icon.png",
                    &png(&square(&source, 180, padding, solid)?)?,
                )?;
                snippet.push_str(
                    "<link rel=\"icon\" href=\"/favicon.ico\" sizes=\"48x48\">\n\
                     <link rel=\"icon\" type=\"image/png\" sizes=\"32x32\" href=\"/favicon-32x32.png\">\n\
                     <link rel=\"icon\" type=\"image/png\" sizes=\"16x16\" href=\"/favicon-16x16.png\">\n\
                     <link rel=\"apple-touch-icon\" sizes=\"180x180\" href=\"/apple-touch-icon.png\">\n",
                );
            }
            IconPreset::Pwa => {
                for side in [192, 512] {
                    let name = format!("android-chrome-{side}x{side}.png");
                    writer.put(&name, &png(&square(&source, side, padding, free)?)?)?;
                }
                // A maskable icon keeps its subject inside the central 80 %.
                writer.put(
                    "maskable-512x512.png",
                    &png(&square(&source, 512, padding.max(10.0), solid)?)?,
                )?;
                let name = if request.app_name.trim().is_empty() {
                    stem(&request.source)
                } else {
                    request.app_name.trim().to_owned()
                };
                let manifest = serde_json::json!({
                    "name": name,
                    "short_name": name,
                    "icons": [
                        { "src": "/android-chrome-192x192.png", "sizes": "192x192", "type": "image/png" },
                        { "src": "/android-chrome-512x512.png", "sizes": "512x512", "type": "image/png" },
                        { "src": "/maskable-512x512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable" }
                    ],
                    "theme_color": backdrop.to_hex(),
                    "background_color": backdrop.to_hex(),
                    "display": "standalone"
                });
                writer.put(
                    "site.webmanifest",
                    serde_json::to_string_pretty(&manifest)?.as_bytes(),
                )?;
                snippet.push_str("<link rel=\"manifest\" href=\"/site.webmanifest\">\n");
            }
            IconPreset::Windows => {
                let sizes = [16, 24, 32, 48, 64, 128, 256]
                    .iter()
                    .map(|side| square(&source, *side, padding, free))
                    .collect::<Result<Vec<_>>>()?;
                writer.put("windows/app.ico", &ico(&sizes)?)?;
                for (name, side) in [
                    ("Square44x44Logo.png", 44),
                    ("Square150x150Logo.png", 150),
                    ("Square310x310Logo.png", 310),
                    ("StoreLogo.png", 50),
                ] {
                    writer.put(
                        &format!("windows/{name}"),
                        &png(&square(&source, side, padding, free)?)?,
                    )?;
                }
            }
            IconPreset::Android => {
                for (density, side) in [
                    ("mdpi", 48),
                    ("hdpi", 72),
                    ("xhdpi", 96),
                    ("xxhdpi", 144),
                    ("xxxhdpi", 192),
                ] {
                    writer.put(
                        &format!("android/mipmap-{density}/ic_launcher.png"),
                        &png(&square(&source, side, padding, free)?)?,
                    )?;
                }
                writer.put(
                    "android/playstore-icon.png",
                    &png(&square(&source, 512, padding, solid)?)?,
                )?;
            }
            IconPreset::Ios => {
                for side in [20, 29, 40, 58, 60, 76, 80, 87, 120, 152, 167, 180, 1024] {
                    writer.put(
                        &format!("ios/AppIcon-{side}.png"),
                        &png(&square(&source, side, padding, solid)?)?,
                    )?;
                }
            }
            IconPreset::Macos => {
                for (name, side) in [
                    ("icon_16x16.png", 16),
                    ("icon_16x16@2x.png", 32),
                    ("icon_32x32.png", 32),
                    ("icon_32x32@2x.png", 64),
                    ("icon_128x128.png", 128),
                    ("icon_128x128@2x.png", 256),
                    ("icon_256x256.png", 256),
                    ("icon_256x256@2x.png", 512),
                    ("icon_512x512.png", 512),
                    ("icon_512x512@2x.png", 1024),
                ] {
                    writer.put(
                        &format!("macos/icon.iconset/{name}"),
                        &png(&square(&source, side, padding, free)?)?,
                    )?;
                }
            }
        }
    }
    if !snippet.is_empty() {
        writer.put("snippet.html", snippet.as_bytes())?;
    }
    Ok(Generated {
        folder,
        files: writer.files,
        snippet,
        blurhash: String::new(),
    })
}

/// A social network picture size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SocialSize {
    pub id: &'static str,
    pub width: u32,
    pub height: u32,
}

/// Sizes social networks ask for.
pub const SOCIAL_SIZES: &[SocialSize] = &[
    SocialSize {
        id: "og",
        width: 1200,
        height: 630,
    },
    SocialSize {
        id: "x",
        width: 1200,
        height: 675,
    },
    SocialSize {
        id: "instagram-square",
        width: 1080,
        height: 1080,
    },
    SocialSize {
        id: "instagram-portrait",
        width: 1080,
        height: 1350,
    },
    SocialSize {
        id: "story",
        width: 1080,
        height: 1920,
    },
    SocialSize {
        id: "youtube",
        width: 1280,
        height: 720,
    },
    SocialSize {
        id: "linkedin",
        width: 1200,
        height: 627,
    },
    SocialSize {
        id: "pinterest",
        width: 1000,
        height: 1500,
    },
    SocialSize {
        id: "facebook-cover",
        width: 1640,
        height: 624,
    },
];

/// Writes the picture cropped to the chosen social network sizes. An empty
/// list of ids writes every size.
pub fn social(
    runner: &Runner,
    source: &Path,
    folder: &Path,
    ids: &[String],
    quality: u8,
    cancel: &CancelToken,
) -> Result<Generated> {
    let image = load(runner, source, cancel)?;
    let name = stem(source);
    let folder = fresh_folder(folder, &format!("{name}-social"))?;
    let mut writer = Writer {
        folder: folder.clone(),
        files: Vec::new(),
    };
    for size in SOCIAL_SIZES
        .iter()
        .filter(|size| ids.is_empty() || ids.iter().any(|id| id == size.id))
    {
        cancel.check()?;
        let cropped = resize(
            &image,
            &Resize {
                mode: ResizeMode::Fill,
                width: size.width,
                height: size.height,
                upscale: true,
                ..Resize::default()
            },
            1.0,
        )?;
        let options = EncodeOptions::new(ImageFormat::Jpeg).with_quality(quality.clamp(1, 100));
        writer.put(
            &format!("{name}-{}-{}x{}.jpg", size.id, size.width, size.height),
            &encode(&cropped, &options, cancel)?,
        )?;
    }
    Ok(Generated {
        folder,
        files: writer.files,
        snippet: String::new(),
        blurhash: String::new(),
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WebKitRequest {
    pub source: PathBuf,
    pub folder: PathBuf,
    /// Widths of the variants in pixels; wider than the source are left out.
    pub widths: Vec<u32>,
    /// Formats of the variants, the most modern first. The last one is the
    /// fallback of the `<img>` tag.
    pub formats: Vec<ImageFormat>,
    pub quality: u8,
    pub alt: String,
    /// Value of the `sizes` attribute.
    pub sizes: String,
    /// Path prefix of the files in the HTML, such as `/images/`.
    pub url_prefix: String,
}

impl Default for WebKitRequest {
    fn default() -> Self {
        Self {
            source: PathBuf::new(),
            folder: PathBuf::new(),
            widths: vec![480, 768, 1080, 1440, 1920],
            formats: vec![ImageFormat::Avif, ImageFormat::Webp, ImageFormat::Jpeg],
            quality: 78,
            alt: String::new(),
            sizes: String::from("100vw"),
            url_prefix: String::new(),
        }
    }
}

/// The average colour of a picture, for a placeholder behind it.
pub fn average_color(image: &Image) -> Result<Color> {
    let small = fit_within(image, 32, ResizeFilter::Box)?.into_depth(Depth::Eight);
    let data = small.to_rgb8();
    let pixels = (data.len() / 3).max(1) as u64;
    let mut sums = [0u64; 3];
    for pixel in data.chunks_exact(3) {
        for (sum, value) in sums.iter_mut().zip(pixel) {
            *sum += u64::from(*value);
        }
    }
    Ok(Color::rgb(
        (sums[0] / pixels) as u8,
        (sums[1] / pixels) as u8,
        (sums[2] / pixels) as u8,
    ))
}

/// Writes responsive variants of a picture and the `<picture>` markup that
/// uses them, with a BlurHash and an average colour as placeholders.
pub fn web_kit(
    runner: &Runner,
    request: &WebKitRequest,
    cancel: &CancelToken,
) -> Result<Generated> {
    let image = load(runner, &request.source, cancel)?;
    let name = stem(&request.source);
    let folder = fresh_folder(&request.folder, &format!("{name}-web"))?;
    let mut writer = Writer {
        folder: folder.clone(),
        files: Vec::new(),
    };
    let mut widths: Vec<u32> = request
        .widths
        .iter()
        .copied()
        .filter(|width| *width > 0 && *width < image.width())
        .collect();
    widths.push(image.width());
    widths.sort_unstable();
    widths.dedup();
    let mut formats: Vec<ImageFormat> = request
        .formats
        .iter()
        .copied()
        .filter(|format| format.can_encode())
        .collect();
    formats.dedup();
    if formats.is_empty() {
        formats.push(ImageFormat::Jpeg);
    }

    let variants: Vec<Image> = widths
        .iter()
        .map(|width| {
            let height = (u64::from(image.height()) * u64::from(*width) / u64::from(image.width()))
                .max(1) as u32;
            resize_to(&image, *width, height, ResizeFilter::Lanczos3, true, None)
        })
        .collect::<Result<_>>()?;

    let prefix = &request.url_prefix;
    let mut sources = String::new();
    let mut fallback_srcset = String::new();
    let mut fallback_src = String::new();
    for (position, format) in formats.iter().enumerate() {
        let mut srcset = Vec::new();
        for variant in &variants {
            cancel.check()?;
            let options = EncodeOptions::new(*format).with_quality(request.quality.clamp(1, 100));
            let file = format!("{name}-{}.{}", variant.width(), format.extension());
            writer.put(&file, &encode(variant, &options, cancel)?)?;
            srcset.push(format!("{prefix}{file} {}w", variant.width()));
            fallback_src = format!("{prefix}{file}");
        }
        let srcset = srcset.join(", ");
        if position + 1 == formats.len() {
            fallback_srcset = srcset;
        } else {
            let _ = writeln!(
                sources,
                "  <source type=\"{}\" srcset=\"{srcset}\" sizes=\"{}\">",
                format.mime(),
                html_escape(&request.sizes)
            );
        }
    }
    let hash = blurhash(&image, 4, 3)?;
    let color = average_color(&image)?;
    let snippet = format!(
        "<picture>\n{sources}  <img src=\"{fallback_src}\" srcset=\"{fallback_srcset}\" sizes=\"{}\" \
         width=\"{}\" height=\"{}\" alt=\"{}\" loading=\"lazy\" decoding=\"async\" \
         style=\"background-color: {}\" data-blurhash=\"{}\">\n</picture>\n",
        html_escape(&request.sizes),
        image.width(),
        image.height(),
        html_escape(&request.alt),
        color.to_hex(),
        html_escape(&hash),
    );
    writer.put("snippet.html", snippet.as_bytes())?;
    Ok(Generated {
        folder,
        files: writer.files,
        snippet,
        blurhash: hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(folder: &Path, width: u32, height: u32) -> PathBuf {
        let image = Image::new(width, height, Depth::Eight, Color::rgb(30, 144, 255)).unwrap();
        let path = folder.join("logo.png");
        fs::write(&path, png(&image).unwrap()).unwrap();
        path
    }

    #[test]
    fn ico_holds_every_size() {
        let images: Vec<Image> = [16, 32, 256]
            .iter()
            .map(|side| Image::new(*side, *side, Depth::Eight, Color::WHITE).unwrap())
            .collect();
        let bytes = ico(&images).unwrap();
        assert_eq!(&bytes[..6], &[0, 0, 1, 0, 3, 0]);
        assert_eq!(bytes[6], 16);
        assert_eq!(bytes[6 + 32], 0);
        assert_eq!(ImageFormat::sniff(&bytes), Some(ImageFormat::Ico));
        let first_offset = u32::from_le_bytes(bytes[18..22].try_into().unwrap()) as usize;
        assert_eq!(first_offset, 6 + 16 * 3);
        assert!(bytes[first_offset..].starts_with(b"\x89PNG"));
    }

    #[test]
    fn square_keeps_proportions() {
        let wide = Image::new(200, 100, Depth::Eight, Color::rgb(255, 0, 0)).unwrap();
        let icon = square(&wide, 64, 0.0, None).unwrap();
        assert_eq!((icon.width(), icon.height()), (64, 64));
        assert_eq!(icon.pixel8(32, 32), [255, 0, 0, 255]);
        assert_eq!(icon.pixel8(32, 2)[3], 0);
        let solid = square(&wide, 64, 10.0, Some(Color::WHITE)).unwrap();
        assert_eq!(solid.pixel8(1, 1), [255, 255, 255, 255]);
        assert!(solid.is_opaque());
    }

    #[test]
    fn icon_sets_are_written() {
        let folder = tempfile::tempdir().unwrap();
        let request = IconRequest {
            source: source(folder.path(), 300, 300),
            folder: folder.path().join("out"),
            presets: vec![IconPreset::Favicon, IconPreset::Pwa, IconPreset::Android],
            app_name: "Demo".into(),
            ..IconRequest::default()
        };
        let made = icons(&Runner::default(), &request, &CancelToken::new()).unwrap();
        assert_eq!(made.folder, folder.path().join("out").join("logo-icons"));
        for name in [
            "favicon.ico",
            "favicon-32x32.png",
            "apple-touch-icon.png",
            "site.webmanifest",
            "android/mipmap-xxxhdpi/ic_launcher.png",
            "snippet.html",
        ] {
            assert!(made.folder.join(name).is_file(), "{name}");
        }
        assert!(made.snippet.contains("apple-touch-icon"));
        let manifest = fs::read_to_string(made.folder.join("site.webmanifest")).unwrap();
        assert!(manifest.contains("\"Demo\""));

        // A second run goes into its own folder.
        let again = icons(&Runner::default(), &request, &CancelToken::new()).unwrap();
        assert_eq!(
            again.folder,
            folder.path().join("out").join("logo-icons (2)")
        );
    }

    #[test]
    fn web_kit_writes_variants_and_markup() {
        let folder = tempfile::tempdir().unwrap();
        let request = WebKitRequest {
            source: source(folder.path(), 1000, 500),
            folder: folder.path().to_path_buf(),
            widths: vec![400, 800, 4000],
            formats: vec![ImageFormat::Webp, ImageFormat::Jpeg],
            alt: "A \"logo\"".into(),
            ..WebKitRequest::default()
        };
        let made = web_kit(&Runner::default(), &request, &CancelToken::new()).unwrap();
        for name in ["logo-400.webp", "logo-800.jpg", "logo-1000.jpg"] {
            assert!(made.folder.join(name).is_file(), "{name}");
        }
        assert!(!made.folder.join("logo-4000.jpg").exists());
        assert!(made.snippet.contains("type=\"image/webp\""));
        assert!(made.snippet.contains("logo-400.jpg 400w"));
        assert!(made.snippet.contains("alt=\"A &quot;logo&quot;\""));
        assert!(made.snippet.contains("width=\"1000\" height=\"500\""));
        assert!(!made.blurhash.is_empty());
    }

    #[test]
    fn social_sizes_are_exact() {
        let folder = tempfile::tempdir().unwrap();
        let path = source(folder.path(), 640, 480);
        let made = social(
            &Runner::default(),
            &path,
            folder.path(),
            &[String::from("og"), String::from("story")],
            85,
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(made.files.len(), 2);
        assert!(made.folder.join("logo-og-1200x630.jpg").is_file());
        assert_eq!(
            average_color(&Image::new(8, 8, Depth::Eight, Color::rgb(10, 20, 30)).unwrap())
                .unwrap(),
            Color::rgb(10, 20, 30)
        );
    }
}
