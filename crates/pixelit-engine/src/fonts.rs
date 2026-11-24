//! Fonts and pictures a pipeline refers to: system fonts by family name, font
//! files by path, watermark pictures by path.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};

use fontdb::{Database, Family, Query};
use pixelit_codecs::{decode_file, DecodeOptions};
use pixelit_core::ops::resize::fit_within;
use pixelit_core::pipeline::{ResizeFilter, Resources};
use pixelit_core::text::{load_font, load_font_face, Font};
use pixelit_core::{CancelToken, Error, Image, Limits, Result};

/// Families tried for the default font, in order.
const DEFAULT_FAMILIES: &[&str] = &["Segoe UI", "Arial", "Noto Sans", "DejaVu Sans"];

/// Pictures kept decoded between steps and files of one batch.
const IMAGE_CACHE: usize = 8;

/// Largest side of a watermark picture kept in memory.
const IMAGE_MAX_SIDE: u32 = 4096;

#[derive(Default)]
pub struct FileResources {
    database: OnceLock<Database>,
    fonts: Mutex<HashMap<String, Font>>,
    images: Mutex<Vec<(String, Image)>>,
}

impl FileResources {
    pub fn new() -> Self {
        Self::default()
    }

    fn database(&self) -> &Database {
        self.database.get_or_init(|| {
            let mut database = Database::new();
            database.load_system_fonts();
            database
        })
    }

    /// Family names of the installed fonts, sorted.
    pub fn families(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .database()
            .faces()
            .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    fn system_font(&self, families: &[Family<'_>]) -> Option<Font> {
        let database = self.database();
        let id = database.query(&Query {
            families,
            ..Query::default()
        })?;
        database
            .with_face_data(id, |data, index| load_font_face(data.to_vec(), index).ok())
            .flatten()
    }

    fn find_font(&self, name: &str) -> Result<Font> {
        if name.is_empty() {
            let mut families: Vec<Family<'_>> = DEFAULT_FAMILIES
                .iter()
                .map(|name| Family::Name(name))
                .collect();
            families.push(Family::SansSerif);
            return self
                .system_font(&families)
                .ok_or_else(|| Error::new("fontNotFound").with("name", "default"));
        }
        let path = Path::new(name);
        if path.is_file() {
            let data = fs::read(path).map_err(|error| Error::from(error).with("path", name))?;
            return load_font(data).map_err(|error| error.with("path", name));
        }
        self.system_font(&[Family::Name(name)])
            .ok_or_else(|| Error::new("fontNotFound").with("name", name))
    }
}

impl Resources for FileResources {
    fn font(&self, name: &str) -> Result<Font> {
        let name = name.trim();
        if let Some(font) = self
            .fonts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
        {
            return Ok(font.clone());
        }
        let font = self.find_font(name)?;
        self.fonts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.to_owned(), font.clone());
        Ok(font)
    }

    fn image(&self, path: &str) -> Result<Image> {
        {
            let images = self.images.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((_, image)) = images.iter().find(|(known, _)| known == path) {
                return Ok(image.clone());
            }
        }
        let options = DecodeOptions {
            limits: Limits::default(),
            max_side: Some(IMAGE_MAX_SIDE),
            animation: false,
            ..DecodeOptions::default()
        };
        let decoded = decode_file(Path::new(path), &options, &CancelToken::new())
            .map_err(|error| error.with("path", path))?;
        let image = decoded
            .media
            .into_first()
            .ok_or_else(|| Error::new("emptyImage").with("path", path))?;
        let image = if image.width().max(image.height()) > IMAGE_MAX_SIDE {
            fit_within(&image, IMAGE_MAX_SIDE, ResizeFilter::Lanczos3)?
        } else {
            image
        };
        let mut images = self.images.lock().unwrap_or_else(PoisonError::into_inner);
        if images.len() >= IMAGE_CACHE {
            images.remove(0);
        }
        images.push((path.to_owned(), image.clone()));
        Ok(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_font_and_picture_are_errors() {
        let resources = FileResources::new();
        let error = resources.font("No Such Font Family 12345").unwrap_err();
        assert_eq!(error.code, "fontNotFound");
        assert!(resources.image("Z:/no/such/picture.png").is_err());
    }
}
