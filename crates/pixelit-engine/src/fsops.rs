//! Safe file writes and the undo log.
//!
//! A result is written into a temporary file first and renamed into place, so
//! a crash never leaves a half-written picture. A file that gets replaced or
//! removed goes to the Recycle Bin, and every batch records what it created
//! and what it moved there, which makes the batch reversible. On macOS the
//! system Trash cannot be listed, so a replaced original is kept under the
//! data folder until the undo entry ages out.

#[cfg(windows)]
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use pixelit_core::pipeline::Conflict;
use pixelit_core::{Error, Result};
use serde::{Deserialize, Deserializer, Serialize};

use crate::clock;
use crate::paths::Paths;
use crate::store;

/// Version of `undo.json`.
pub const UNDO_VERSION: u32 = 1;

/// Batches kept in the undo log.
const UNDO_LIMIT: usize = 30;

/// Numbered names tried before giving up on a free name.
const RENAME_ATTEMPTS: u32 = 10_000;

/// How a result reached the disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Written {
    /// A new file was created at the path.
    Created(PathBuf),
    /// The file at the path was replaced. `kept` is where the old file is
    /// when it was not sent to the Recycle Bin (the undo folder on macOS).
    Replaced {
        path: PathBuf,
        kept: Option<PathBuf>,
    },
    /// The file at the path exists and was left alone.
    Skipped(PathBuf),
}

impl Written {
    pub fn path(&self) -> &Path {
        match self {
            Self::Created(path) | Self::Skipped(path) | Self::Replaced { path, .. } => path,
        }
    }
}

/// A file a batch removed from its place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashedFile {
    /// Where the file belonged.
    pub path: PathBuf,
    /// Copy kept for undo when the system Trash cannot be read back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<PathBuf>,
}

impl TrashedFile {
    pub fn removed(path: PathBuf) -> Self {
        Self { path, kept: None }
    }

    pub fn kept_at(path: PathBuf, kept: PathBuf) -> Self {
        Self {
            path,
            kept: Some(kept),
        }
    }
}

impl<'de> Deserialize<'de> for TrashedFile {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Form {
            Path(PathBuf),
            Full {
                path: PathBuf,
                #[serde(default)]
                kept: Option<PathBuf>,
            },
        }
        match Form::deserialize(deserializer)? {
            Form::Path(path) => Ok(Self::removed(path)),
            Form::Full { path, kept } => Ok(Self { path, kept }),
        }
    }
}

/// Moves a file or a folder to the Recycle Bin.
pub fn to_trash(path: &Path) -> Result<()> {
    trash::delete(path).map_err(|error| {
        Error::new("trashFailed")
            .with("path", path.display())
            .detail(error)
    })
}

/// Moves `path` into the undo folder and returns the place it was kept.
pub fn keep_for_undo(paths: &Paths, path: &Path) -> Result<PathBuf> {
    let folder = paths.undo_files();
    fs::create_dir_all(&folder)?;
    let name = format!(
        "{:x}-{}",
        clock::unix_now(),
        path.file_name()
            .map_or_else(|| String::from("file"), |name| name.to_string_lossy().into_owned())
    );
    let kept = folder.join(name);
    fs::rename(path, &kept).map_err(|error| Error::from(error).with("path", path.display()))?;
    Ok(kept)
}

/// Removes a file from its place: on macOS into the undo folder when one is
/// given, otherwise into the Recycle Bin.
pub fn remove_for_undo(path: &Path, stash: Option<&Paths>) -> Result<TrashedFile> {
    if cfg!(target_os = "macos") {
        if let Some(paths) = stash {
            let kept = keep_for_undo(paths, path)?;
            return Ok(TrashedFile::kept_at(path.to_path_buf(), kept));
        }
    }
    to_trash(path)?;
    Ok(TrashedFile::removed(path.to_path_buf()))
}

fn numbered(path: &Path, number: u32) -> PathBuf {
    if number <= 1 {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map_or_default(|stem| stem.to_string_lossy().into_owned());
    let name = match path.extension() {
        Some(extension) => format!("{stem} ({number}).{}", extension.to_string_lossy()),
        None => format!("{stem} ({number})"),
    };
    path.with_file_name(name)
}

/// Reserves a free name by creating an empty file; two workers asking for the
/// same name at the same time get different ones.
fn reserve(path: &Path) -> Result<PathBuf> {
    for number in 1..=RENAME_ATTEMPTS {
        let candidate = numbered(path, number);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(Error::from(error).with("path", candidate.display())),
        }
    }
    Err(Error::new("io")
        .with("path", path.display())
        .detail("no free file name"))
}

/// Gives a finished file of the same folder its name: `temp` becomes `path`,
/// or `name (2).ext` and so on when the name is taken. Returns the name used.
pub fn place_file(temp: &Path, path: &Path) -> Result<PathBuf> {
    let target = reserve(path)?;
    if let Err(error) = fs::rename(temp, &target) {
        let _ = fs::remove_file(&target);
        return Err(Error::from(error).with("path", target.display()));
    }
    Ok(target)
}

fn write_temp(folder: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let temp = folder.join(format!(".pixelit-{:08x}.tmp", fastrand::u32(..)));
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(Error::from(error).with("path", folder.display()));
    }
    Ok(temp)
}

/// Writes `bytes` to `path` according to the conflict rule.
pub fn write_file(path: &Path, bytes: &[u8], conflict: Conflict) -> Result<Written> {
    write_file_in(path, bytes, conflict, None)
}

/// Like [`write_file`], and on macOS keeps a replaced original under `paths`
/// so the batch can still be undone.
pub fn write_file_in(
    path: &Path,
    bytes: &[u8],
    conflict: Conflict,
    paths: Option<&Paths>,
) -> Result<Written> {
    let folder = path
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
        .ok_or_else(|| Error::new("io").with("path", path.display()))?;
    fs::create_dir_all(folder)
        .map_err(|error| Error::from(error).with("path", folder.display()))?;
    if conflict == Conflict::Skip && path.exists() {
        return Ok(Written::Skipped(path.to_path_buf()));
    }
    let temp = write_temp(folder, bytes)?;
    let placed = (|| -> Result<Written> {
        match conflict {
            Conflict::Rename | Conflict::Skip => {
                let target = if conflict == Conflict::Skip {
                    // The file appeared while the result was being encoded.
                    match OpenOptions::new().write(true).create_new(true).open(path) {
                        Ok(_) => path.to_path_buf(),
                        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                            return Ok(Written::Skipped(path.to_path_buf()));
                        }
                        Err(error) => return Err(Error::from(error).with("path", path.display())),
                    }
                } else {
                    reserve(path)?
                };
                fs::rename(&temp, &target)
                    .map_err(|error| Error::from(error).with("path", target.display()))?;
                Ok(Written::Created(target))
            }
            Conflict::Overwrite => {
                let existed = path.exists();
                let kept = if existed {
                    Some(remove_for_undo(path, paths)?.kept)
                } else {
                    None
                };
                fs::rename(&temp, path)
                    .map_err(|error| Error::from(error).with("path", path.display()))?;
                Ok(if existed {
                    Written::Replaced {
                        path: path.to_path_buf(),
                        kept: kept.flatten(),
                    }
                } else {
                    Written::Created(path.to_path_buf())
                })
            }
        }
    })();
    if placed.is_err() || matches!(placed, Ok(Written::Skipped(_))) {
        let _ = fs::remove_file(&temp);
    }
    placed
}

// ------------------------------------------------------------------ undo

/// What one batch did to the disk.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UndoRecord {
    pub id: String,
    /// Unix time of the start of the batch, in seconds.
    pub time: i64,
    pub title: String,
    /// Files the batch created, with their sizes.
    pub created: Vec<CreatedFile>,
    /// Files the batch removed from their place.
    pub trashed: Vec<TrashedFile>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CreatedFile {
    pub path: PathBuf,
    pub bytes: u64,
}

impl UndoRecord {
    pub fn new(title: &str) -> Self {
        let time = clock::unix_now();
        Self {
            id: format!("{time:x}-{:08x}", fastrand::u32(..)),
            time,
            title: title.to_owned(),
            created: Vec::new(),
            trashed: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.trashed.is_empty()
    }

    /// Notes the outcome of one write.
    pub fn note(&mut self, written: &Written, bytes: u64) {
        match written {
            Written::Created(path) => self.created.push(CreatedFile {
                path: path.clone(),
                bytes,
            }),
            Written::Replaced { path, kept } => {
                self.trashed.push(TrashedFile {
                    path: path.clone(),
                    kept: kept.clone(),
                });
                self.created.push(CreatedFile {
                    path: path.clone(),
                    bytes,
                });
            }
            Written::Skipped(_) => {}
        }
    }
}

/// The result of undoing a batch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoReport {
    /// Created files moved to the Recycle Bin.
    pub removed: u32,
    /// Original files brought back from the Recycle Bin.
    pub restored: u32,
    /// Created files left alone because they changed after the batch.
    pub changed: u32,
    /// Original files that are no longer in the Recycle Bin.
    pub missing: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UndoLog {
    pub records: Vec<UndoRecord>,
}

impl UndoLog {
    pub fn load(paths: &Paths) -> Self {
        store::load_or_default(&paths.undo(), UNDO_VERSION, store::no_migration)
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        store::save_json(&paths.undo(), UNDO_VERSION, self)
    }

    /// Adds a record to the log on disk and drops the oldest ones.
    pub fn push(paths: &Paths, record: UndoRecord) -> Result<()> {
        if record.is_empty() {
            return Ok(());
        }
        let mut log = Self::load(paths);
        log.records.push(record);
        let extra = log.records.len().saturating_sub(UNDO_LIMIT);
        for dropped in log.records.drain(..extra) {
            forget_kept(&dropped);
        }
        log.save(paths)
    }

    /// Reverses a batch and removes its record.
    pub fn undo(paths: &Paths, id: &str) -> Result<UndoReport> {
        let mut log = Self::load(paths);
        let position = log
            .records
            .iter()
            .position(|record| record.id == id)
            .ok_or_else(|| Error::new("undoNotFound").with("id", id))?;
        let record = log.records.remove(position);
        let report = revert(&record)?;
        log.save(paths)?;
        Ok(report)
    }
}

#[cfg(windows)]
fn restore(wanted: &[TrashedFile], since: i64) -> Result<(u32, u32)> {
    let failed = |error: trash::Error| Error::new("trashFailed").detail(error);
    let items = trash::os_limited::list().map_err(failed)?;
    // The first copy of every wanted path deleted since the batch began: the
    // original. A later copy is the result the undo itself has just removed.
    let mut found: HashMap<PathBuf, trash::TrashItem> = HashMap::new();
    for item in items {
        if item.time_deleted < since - 5 {
            continue;
        }
        let original = item.original_path();
        let Some(path) = wanted
            .iter()
            .find(|file| same_path(&file.path, &original))
            .map(|file| file.path.clone())
        else {
            continue;
        };
        let older = found
            .get(&path)
            .is_none_or(|known| item.time_deleted < known.time_deleted);
        if older && !path.exists() {
            found.insert(path, item);
        }
    }
    let restored = found.len() as u32;
    let missing = wanted.len() as u32 - restored;
    if restored > 0 {
        trash::os_limited::restore_all(found.into_values()).map_err(failed)?;
    }
    Ok((restored, missing))
}

#[cfg(not(windows))]
fn restore(wanted: &[TrashedFile], _since: i64) -> Result<(u32, u32)> {
    let mut restored = 0u32;
    let mut missing = 0u32;
    for file in wanted {
        let Some(kept) = &file.kept else {
            missing += 1;
            continue;
        };
        if !kept.is_file() {
            missing += 1;
            continue;
        }
        if file.path.exists() {
            missing += 1;
            continue;
        }
        if let Some(parent) = file.path.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::rename(kept, &file.path) {
            Ok(()) => restored += 1,
            Err(error) => return Err(Error::from(error).with("path", file.path.display())),
        }
    }
    Ok((restored, missing))
}

#[cfg_attr(not(windows), allow(dead_code))]
fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(b.to_string_lossy().as_ref())
}

/// Sends kept originals of a dropped undo entry to the Recycle Bin.
fn forget_kept(record: &UndoRecord) {
    for file in &record.trashed {
        if let Some(kept) = &file.kept {
            let _ = to_trash(kept);
        }
    }
}

fn revert(record: &UndoRecord) -> Result<UndoReport> {
    let mut report = UndoReport::default();
    for created in &record.created {
        match fs::metadata(&created.path) {
            Ok(metadata) if metadata.len() == created.bytes => {
                to_trash(&created.path)?;
                report.removed += 1;
            }
            Ok(_) => report.changed += 1,
            Err(_) => {}
        }
    }
    if !record.trashed.is_empty() {
        let (restored, missing) = restore(&record.trashed, record.time)?;
        report.restored = restored;
        report.missing = missing;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_finds_free_names() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("out").join("a.png");
        let first = write_file(&path, b"1", Conflict::Rename).unwrap();
        let second = write_file(&path, b"22", Conflict::Rename).unwrap();
        let third = write_file(&path, b"333", Conflict::Rename).unwrap();
        assert_eq!(first, Written::Created(path.clone()));
        assert_eq!(
            second,
            Written::Created(folder.path().join("out").join("a (2).png"))
        );
        assert_eq!(
            third,
            Written::Created(folder.path().join("out").join("a (3).png"))
        );
        assert_eq!(fs::read(second.path()).unwrap(), b"22");
        // Only the three results remain: no temporary files.
        assert_eq!(fs::read_dir(folder.path().join("out")).unwrap().count(), 3);
    }

    #[test]
    fn skip_leaves_existing_files() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("a.png");
        fs::write(&path, b"old").unwrap();
        let written = write_file(&path, b"new", Conflict::Skip).unwrap();
        assert_eq!(written, Written::Skipped(path.clone()));
        assert_eq!(fs::read(&path).unwrap(), b"old");
        let other = folder.path().join("b.png");
        assert_eq!(
            write_file(&other, b"new", Conflict::Skip).unwrap(),
            Written::Created(other.clone())
        );
        assert_eq!(fs::read(&other).unwrap(), b"new");
    }

    #[test]
    fn overwrite_creates_missing_files() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("a.png");
        let written = write_file(&path, b"new", Conflict::Overwrite).unwrap();
        assert_eq!(written, Written::Created(path.clone()));
        assert_eq!(fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn finished_files_get_free_names() {
        let folder = tempfile::tempdir().unwrap();
        let target = folder.path().join("fox.jpg");
        for (index, name) in ["fox.jpg", "fox (2).jpg", "fox (3).jpg"].iter().enumerate() {
            let temp = folder.path().join(".part");
            fs::write(&temp, [index as u8]).unwrap();
            let placed = place_file(&temp, &target).unwrap();
            assert_eq!(placed, folder.path().join(name));
            assert_eq!(fs::read(&placed).unwrap(), [index as u8]);
            assert!(!temp.exists());
        }
        let missing = place_file(&folder.path().join("absent"), &target).unwrap_err();
        assert_eq!(missing.code, "notFound");
        // The name reserved for the file that never came is free again.
        assert!(!folder.path().join("fox (4).jpg").exists());
    }

    #[test]
    fn numbered_names() {
        assert_eq!(numbered(Path::new("a/b.png"), 1), Path::new("a/b.png"));
        assert_eq!(numbered(Path::new("a/b.png"), 2), Path::new("a/b (2).png"));
        assert_eq!(numbered(Path::new("a/b"), 3), Path::new("a/b (3)"));
    }

    #[test]
    fn undo_record_notes_writes() {
        let mut record = UndoRecord::new("batch");
        assert!(record.is_empty());
        record.note(&Written::Created(PathBuf::from("a")), 3);
        record.note(
            &Written::Replaced {
                path: PathBuf::from("b"),
                kept: None,
            },
            4,
        );
        record.note(&Written::Skipped(PathBuf::from("c")), 5);
        assert_eq!(record.created.len(), 2);
        assert_eq!(record.trashed, [TrashedFile::removed(PathBuf::from("b"))]);
    }

    #[test]
    fn undo_log_is_bounded() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        for index in 0..UNDO_LIMIT + 5 {
            let mut record = UndoRecord::new("batch");
            record.created.push(CreatedFile {
                path: PathBuf::from(format!("{index}.png")),
                bytes: 1,
            });
            UndoLog::push(&paths, record).unwrap();
        }
        let log = UndoLog::load(&paths);
        assert_eq!(log.records.len(), UNDO_LIMIT);
        assert_eq!(log.records[0].created[0].path, PathBuf::from("5.png"));
        assert_eq!(
            UndoLog::undo(&paths, "nope").unwrap_err().code,
            "undoNotFound"
        );
    }
}
