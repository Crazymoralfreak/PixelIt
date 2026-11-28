//! Pictures the studio has generated, and the request that made each one.
//!
//! The files live in the `generated` folder. This module keeps the request
//! beside them. A file from before the record existed is still listed, without
//! a prompt.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use pixelit_core::{Error, Result};
use pixelit_engine::fsops;
use pixelit_engine::store::{load_or_default, no_migration, save_json};
use pixelit_engine::Paths;
use serde::{Deserialize, Serialize};

use crate::config::{GenerationProvider, Tuning};
use crate::parameters::{self, Parameters};

/// Version of `generated.json`.
const VERSION: u32 = 1;

/// How a picture was made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GeneratedKind {
    #[default]
    Create,
    Edit,
    Outpaint,
    Upscale,
    /// The same size, with detail painted in by the generator.
    Refine,
}

/// One file of a request.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Picture {
    /// File name inside the `generated` folder.
    pub file: String,
    pub seed: u64,
    pub favorite: bool,
    pub width: u32,
    pub height: u32,
}

/// One request and the pictures it produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Record {
    pub id: String,
    /// Milliseconds since 1970.
    pub created: u64,
    /// The prompt as it was typed.
    pub prompt: String,
    pub negative: String,
    pub styles: Vec<String>,
    /// The prompt after the styles were applied.
    pub final_prompt: String,
    pub final_negative: String,
    pub provider: GenerationProvider,
    pub model: String,
    pub width: u32,
    pub height: u32,
    pub steps: u32,
    pub cfg: f32,
    pub strength: f32,
    /// The picture this one was made from, when there was one.
    pub source: String,
    pub kind: GeneratedKind,
    pub pictures: Vec<Picture>,
    /// The sampler, the schedule, the VAE and LoRA files that were applied.
    pub tuning: Tuning,
}

impl Default for Record {
    fn default() -> Self {
        Self {
            id: String::new(),
            created: 0,
            prompt: String::new(),
            negative: String::new(),
            styles: Vec::new(),
            final_prompt: String::new(),
            final_negative: String::new(),
            provider: GenerationProvider::None,
            model: String::new(),
            width: 0,
            height: 0,
            steps: 0,
            cfg: 0.0,
            strength: 0.0,
            source: String::new(),
            kind: GeneratedKind::Create,
            pictures: Vec::new(),
            tuning: Tuning::default(),
        }
    }
}

fn gate() -> &'static Mutex<()> {
    static GATE: Mutex<()> = Mutex::new(());
    &GATE
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn load(paths: &Paths) -> Vec<Record> {
    load_or_default(&paths.generated_index(), VERSION, no_migration)
}

fn save(paths: &Paths, records: &[Record]) -> Result<()> {
    save_json(&paths.generated_index(), VERSION, &records)
}

/// Adds a request at the front of the record.
pub fn record(paths: &Paths, mut entry: Record) -> Result<Record> {
    if entry.id.is_empty() {
        entry.id = format!("{:016x}", fastrand::u64(..));
    }
    if entry.created == 0 {
        entry.created = now_ms();
    }
    entry.pictures.retain(|picture| !picture.file.is_empty());
    let _guard = gate().lock().unwrap_or_else(PoisonError::into_inner);
    let mut records = load(paths);
    records.insert(0, entry.clone());
    save(paths, &records)?;
    Ok(entry)
}

/// The record, newest first. Files that are gone are dropped. Files without
/// a record are listed on their own, and a `parameters` block in the file
/// fills in what it can.
pub fn list(paths: &Paths) -> Vec<Record> {
    let _guard = gate().lock().unwrap_or_else(PoisonError::into_inner);
    let folder = paths.generated();
    let mut records = load(paths);
    let mut known = HashSet::new();
    for entry in &mut records {
        entry
            .pictures
            .retain(|picture| folder.join(&picture.file).is_file());
        for picture in &entry.pictures {
            known.insert(picture.file.clone());
        }
    }
    records.retain(|entry| !entry.pictures.is_empty());
    let mut orphans = orphans_of(&folder, &known);
    let _ = save(paths, &records);
    records.append(&mut orphans);
    records.sort_by(|a, b| b.created.cmp(&a.created).then(b.id.cmp(&a.id)));
    records
}

fn orphans_of(folder: &Path, known: &HashSet<String>) -> Vec<Record> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(folder) else {
        return found;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if known.contains(&name) || !is_picture(&name) {
            continue;
        }
        let path = entry.path();
        let created = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |elapsed| elapsed.as_millis() as u64);
        let mut record = Record {
            id: name.clone(),
            created,
            kind: GeneratedKind::Create,
            pictures: vec![Picture {
                file: name,
                ..Picture::default()
            }],
            ..Record::default()
        };
        if let Some(parameters) = read_parameters(&path) {
            record.prompt.clone_from(&parameters.prompt);
            record.final_prompt = parameters.prompt;
            record.negative.clone_from(&parameters.negative);
            record.final_negative = parameters.negative;
            record.steps = parameters.steps;
            record.cfg = parameters.cfg;
            record.width = parameters.width;
            record.height = parameters.height;
            record.model = parameters.model;
            if let Some(picture) = record.pictures.first_mut() {
                picture.seed = parameters.seed;
                picture.width = parameters.width;
                picture.height = parameters.height;
            }
        }
        found.push(record);
    }
    found
}

fn is_picture(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("png")
            || extension.eq_ignore_ascii_case("webp")
            || extension.eq_ignore_ascii_case("jpg")
            || extension.eq_ignore_ascii_case("jpeg")
    })
}

fn read_parameters(path: &Path) -> Option<Parameters> {
    let bytes = fs::read(path).ok()?;
    let text = pixelit_codecs::png_text(&bytes, "parameters")?;
    parameters::read(&text)
}

/// Sends files to the trash and drops them from the record.
/// Only files inside the `generated` folder are touched.
pub fn forget(paths: &Paths, files: &[String]) -> Result<u32> {
    let folder = paths.generated();
    let names = names_inside(&folder, files)?;
    let mut removed = 0u32;
    for name in &names {
        fsops::to_trash(&folder.join(name))?;
        removed += 1;
    }
    let _guard = gate().lock().unwrap_or_else(PoisonError::into_inner);
    let mut records = load(paths);
    for entry in &mut records {
        entry
            .pictures
            .retain(|picture| !names.contains(&picture.file));
    }
    records.retain(|entry| !entry.pictures.is_empty());
    save(paths, &records)?;
    Ok(removed)
}

/// Sets or clears the star of one file.
pub fn favorite(paths: &Paths, file: &str, on: bool) -> Result<bool> {
    let name = Path::new(file)
        .file_name()
        .map_or_default(|name| name.to_string_lossy().into_owned());
    let _guard = gate().lock().unwrap_or_else(PoisonError::into_inner);
    let mut records = load(paths);
    let mut found = false;
    for entry in &mut records {
        for picture in &mut entry.pictures {
            if picture.file == name {
                picture.favorite = on;
                found = true;
            }
        }
    }
    if !found {
        return Err(Error::new("notFound").with("path", file));
    }
    save(paths, &records)?;
    Ok(on)
}

/// Copies files from the `generated` folder into another folder.
pub fn export(paths: &Paths, files: &[String], folder: &Path) -> Result<Vec<PathBuf>> {
    if !folder.is_dir() {
        return Err(Error::new("notFound").with("path", folder.display()));
    }
    let home = paths.generated();
    let names = names_inside(&home, files)?;
    let mut written = Vec::new();
    for name in names {
        let source = home.join(&name);
        let mut target = folder.join(&name);
        let mut number = 2u32;
        while target.exists() {
            let stem = Path::new(&name)
                .file_stem()
                .map_or_else(|| name.clone(), |stem| stem.to_string_lossy().into_owned());
            let extension = Path::new(&name)
                .extension()
                .map_or_default(|extension| extension.to_string_lossy().into_owned());
            target = folder.join(format!("{stem}-{number}.{extension}"));
            number += 1;
        }
        fs::copy(&source, &target)?;
        written.push(target);
    }
    Ok(written)
}

fn names_inside(folder: &Path, files: &[String]) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for file in files {
        let path = Path::new(file);
        let name = path
            .file_name()
            .map_or_default(|name| name.to_string_lossy().into_owned());
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            folder.join(&name)
        };
        if !candidate.is_file() || !candidate.starts_with(folder) {
            return Err(Error::new("notFound").with("path", file));
        }
        if !names.contains(&name) {
            names.push(name);
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(name: &str) -> Picture {
        Picture {
            file: name.to_owned(),
            seed: 7,
            width: 8,
            height: 8,
            ..Picture::default()
        }
    }

    #[test]
    fn a_record_lists_its_files_and_drops_missing_ones() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        fs::create_dir_all(paths.generated()).unwrap();
        fs::write(paths.generated().join("one.png"), b"png").unwrap();
        record(
            &paths,
            Record {
                prompt: String::from("a fox"),
                pictures: vec![picture("one.png"), picture("gone.png")],
                ..Record::default()
            },
        )
        .unwrap();
        fs::write(paths.generated().join("old.png"), b"png").unwrap();
        let listed = list(&paths);
        assert_eq!(listed.len(), 2);
        let kept = listed.iter().find(|entry| entry.prompt == "a fox").unwrap();
        assert_eq!(kept.pictures.len(), 1);
        assert_eq!(kept.pictures[0].file, "one.png");
        assert!(listed
            .iter()
            .any(|entry| entry.pictures[0].file == "old.png" && entry.prompt.is_empty()));
    }

    #[test]
    fn forgetting_moves_the_file_out_of_the_record() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        fs::create_dir_all(paths.generated()).unwrap();
        let path = paths.generated().join("one.png");
        fs::write(&path, b"png").unwrap();
        record(
            &paths,
            Record {
                pictures: vec![picture("one.png")],
                ..Record::default()
            },
        )
        .unwrap();
        assert_eq!(forget(&paths, &[path.display().to_string()]).unwrap(), 1);
        assert!(list(&paths).is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn a_star_is_remembered() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        fs::create_dir_all(paths.generated()).unwrap();
        fs::write(paths.generated().join("one.png"), b"png").unwrap();
        record(
            &paths,
            Record {
                pictures: vec![picture("one.png")],
                ..Record::default()
            },
        )
        .unwrap();
        assert!(favorite(&paths, "one.png", true).unwrap());
        assert!(list(&paths)[0].pictures[0].favorite);
        assert!(favorite(&paths, "missing.png", true).is_err());
    }
}
