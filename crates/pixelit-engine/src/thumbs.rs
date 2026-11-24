//! Previews and the thumbnail cache.
//!
//! A thumbnail is keyed by the path, size and change time of its file, so an
//! edited file gets a new thumbnail and the old one ages out of the cache.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use pixelit_codecs::{decode_file, encode, DecodeOptions, EncodeOptions, SourceInfo};
use pixelit_core::ops::resize::fit_within;
use pixelit_core::pipeline::{Effort, ResizeFilter};
use pixelit_core::{CancelToken, Error, Image, ImageFormat, Result};
use walkdir::WalkDir;

use crate::paths::Paths;
use crate::process::Runner;
use crate::store;

/// Thumbnail sides offered; a request is rounded up to one of them.
const SIZES: &[u32] = &[160, 320, 640];

/// Quality of cached thumbnails.
const THUMB_QUALITY: u8 = 78;

/// Quality of previews sent to the interface.
const PREVIEW_QUALITY: u8 = 90;

/// A picture reduced for the screen, with what is known about its file.
pub struct Preview {
    pub image: Image,
    pub info: SourceInfo,
}

/// Decodes a file and reduces it so that its long side is at most `max_side`
/// (`0` keeps the full size). The result has 8 bits per channel.
pub fn load_preview(
    runner: &Runner,
    source: &Path,
    max_side: u32,
    page: u32,
    cancel: &CancelToken,
) -> Result<Preview> {
    let options = DecodeOptions {
        limits: runner.limits,
        pdfium: runner.pdfium.clone(),
        page,
        max_side: (max_side > 0).then_some(max_side),
        animation: false,
        deep: false,
        ..DecodeOptions::default()
    };
    let decoded = decode_file(source, &options, cancel)?;
    let info = decoded.info;
    let image = decoded
        .media
        .into_first()
        .ok_or_else(|| Error::new("emptyImage").with("path", source.display()))?;
    let filter = if max_side > 0 && max_side <= 640 {
        ResizeFilter::CatmullRom
    } else {
        ResizeFilter::Lanczos3
    };
    let image = fit_within(&image, max_side, filter)?;
    Ok(Preview { image, info })
}

/// Encodes a picture for the interface: JPEG for opaque pictures, WebP for
/// transparent ones. Returns the bytes and their MIME type.
pub fn encode_for_screen(image: &Image, quality: u8) -> Result<(Vec<u8>, &'static str)> {
    let format = if image.is_opaque() {
        ImageFormat::Jpeg
    } else {
        ImageFormat::Webp
    };
    let mut options = EncodeOptions::new(format).with_quality(quality);
    options.effort = Effort::Fast;
    options.progressive = false;
    let bytes = encode(image, &options, &CancelToken::new())?;
    Ok((bytes, format.mime()))
}

/// A preview of a file, encoded for the interface.
pub fn preview_bytes(
    runner: &Runner,
    source: &Path,
    max_side: u32,
    page: u32,
    cancel: &CancelToken,
) -> Result<(Vec<u8>, &'static str)> {
    let preview = load_preview(runner, source, max_side, page, cancel)?;
    encode_for_screen(&preview.image, PREVIEW_QUALITY)
}

fn cache_key(source: &Path, side: u32) -> Result<String> {
    let metadata =
        fs::metadata(source).map_err(|error| Error::from(error).with("path", source.display()))?;
    let changed = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    let mut hasher = blake3::Hasher::new();
    hasher.update(source.to_string_lossy().as_bytes());
    hasher.update(&metadata.len().to_le_bytes());
    hasher.update(&changed.to_le_bytes());
    hasher.update(&side.to_le_bytes());
    Ok(hasher.finalize().to_hex()[..32].to_owned())
}

fn cache_file(paths: &Paths, key: &str) -> PathBuf {
    // Two levels of folders keep each of them small.
    paths.thumbs().join(&key[..2]).join(format!("{key}.img"))
}

/// The cached thumbnail of a file, made on first use. Returns the bytes and
/// their MIME type.
pub fn thumbnail(
    paths: &Paths,
    runner: &Runner,
    source: &Path,
    side: u32,
    cancel: &CancelToken,
) -> Result<(Vec<u8>, &'static str)> {
    let side = SIZES
        .iter()
        .copied()
        .find(|size| *size >= side)
        .unwrap_or(SIZES[SIZES.len() - 1]);
    let key = cache_key(source, side)?;
    let file = cache_file(paths, &key);
    if let Ok(bytes) = fs::read(&file) {
        if let Some(format) = ImageFormat::sniff(&bytes) {
            return Ok((bytes, format.mime()));
        }
    }
    let preview = load_preview(runner, source, side, 0, cancel)?;
    let (bytes, mime) = encode_for_screen(&preview.image, THUMB_QUALITY)?;
    if let Err(error) = store::write_atomic(&file, &bytes) {
        tracing::debug!(%error, "thumbnail not cached");
    }
    Ok((bytes, mime))
}

/// Size of the thumbnail cache in bytes.
pub fn cache_size(paths: &Paths) -> u64 {
    WalkDir::new(paths.thumbs())
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
        .map(|metadata| metadata.len())
        .sum()
}

/// Removes the least recently changed thumbnails until the cache fits into
/// `limit` bytes. Returns the bytes freed.
pub fn trim_cache(paths: &Paths, limit: u64) -> u64 {
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = WalkDir::new(paths.thumbs())
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            Some((
                entry.into_path(),
                metadata.len(),
                metadata.modified().unwrap_or(UNIX_EPOCH),
            ))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    if total <= limit {
        return 0;
    }
    files.sort_by_key(|(_, _, changed)| *changed);
    let mut freed = 0;
    for (path, size, _) in files {
        if total <= limit {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total -= size;
            freed += size;
        }
    }
    freed
}

/// Empties the thumbnail cache.
pub fn clear_cache(paths: &Paths) -> u64 {
    trim_cache(paths, 0)
}

#[cfg(test)]
mod tests {
    use pixelit_core::{Color, Depth};

    use super::*;

    fn write_png(path: &Path, width: u32, height: u32, color: Color) {
        let image = Image::new(width, height, Depth::Eight, color).unwrap();
        let bytes = encode(
            &image,
            &EncodeOptions::new(ImageFormat::Png),
            &CancelToken::new(),
        )
        .unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn preview_is_reduced() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("a.png");
        write_png(&source, 800, 400, Color::rgb(10, 20, 30));
        let preview =
            load_preview(&Runner::default(), &source, 200, 0, &CancelToken::new()).unwrap();
        assert_eq!((preview.image.width(), preview.image.height()), (200, 100));
        assert_eq!((preview.info.width, preview.info.height), (800, 400));
        let (bytes, mime) =
            preview_bytes(&Runner::default(), &source, 0, 0, &CancelToken::new()).unwrap();
        assert_eq!(mime, "image/jpeg");
        assert_eq!(ImageFormat::sniff(&bytes), Some(ImageFormat::Jpeg));
    }

    #[test]
    fn thumbnails_are_cached_and_trimmed() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path().join("data"));
        let source = folder.path().join("a.png");
        write_png(&source, 400, 400, Color::TRANSPARENT);
        let runner = Runner::default();
        let cancel = CancelToken::new();
        let (first, mime) = thumbnail(&paths, &runner, &source, 100, &cancel).unwrap();
        assert_eq!(mime, "image/webp");
        assert!(cache_size(&paths) > 0);
        let (second, _) = thumbnail(&paths, &runner, &source, 150, &cancel).unwrap();
        assert_eq!(first, second);
        assert!(thumbnail(
            &paths,
            &runner,
            &folder.path().join("none.png"),
            100,
            &cancel
        )
        .is_err());
        assert!(clear_cache(&paths) > 0);
        assert_eq!(cache_size(&paths), 0);
    }
}
