//! Versioned JSON files written atomically.
//!
//! Every data file is `{"version": N, "data": ...}`. A file written by an
//! older version passes through migrations; a file written by a newer version
//! is refused so that nothing is lost by saving it back.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use pixelit_core::{Error, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

/// Upgrades the data of a file from version `from` to version `from + 1`.
pub type Migration = fn(from: u32, data: Value) -> Result<Value>;

/// A migration for formats that have had one version only.
pub fn no_migration(_from: u32, data: Value) -> Result<Value> {
    Ok(data)
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_default(std::ffi::OsStr::to_os_string);
    name.push(format!(".{}.tmp", fastrand::u32(..)));
    path.with_file_name(name)
}

/// Writes a file through a temporary file in the same folder, so a crash
/// leaves either the old content or the new one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    let temp = temp_path(path);
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(Error::from(error).with("path", path.display()));
    }
    Ok(())
}

/// Saves data with its format version.
pub fn save_json<T: Serialize>(path: &Path, version: u32, data: &T) -> Result<()> {
    let document = json!({ "version": version, "data": data });
    let text = serde_json::to_vec_pretty(&document)?;
    write_atomic(path, &text)
}

/// Loads data, migrating older files. A missing file gives the default value.
pub fn load_json<T>(path: &Path, version: u32, migrate: Migration) -> Result<T>
where
    T: DeserializeOwned + Default,
{
    let text = match fs::read(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) => return Err(Error::from(error).with("path", path.display())),
    };
    let bad = |detail: String| {
        Error::new("badFile")
            .with("path", path.display())
            .detail(detail)
    };
    let mut document: Value =
        serde_json::from_slice(&text).map_err(|error| bad(error.to_string()))?;
    let stored = document
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| bad(String::from("no version")))? as u32;
    if stored > version {
        return Err(Error::new("newerFile")
            .with("path", path.display())
            .with("version", stored));
    }
    let mut data = document
        .get_mut("data")
        .map(Value::take)
        .ok_or_else(|| bad(String::from("no data")))?;
    for from in stored..version {
        data = migrate(from, data)?;
    }
    serde_json::from_value(data).map_err(|error| bad(error.to_string()))
}

/// Loads data and falls back to the default when the file is damaged. The
/// damaged file is kept beside the original with a `.damaged` suffix.
pub fn load_or_default<T>(path: &Path, version: u32, migrate: Migration) -> T
where
    T: DeserializeOwned + Default,
{
    match load_json(path, version, migrate) {
        Ok(data) => data,
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "data file replaced by defaults");
            let mut name = path
                .file_name()
                .map_or_default(std::ffi::OsStr::to_os_string);
            name.push(".damaged");
            let _ = fs::rename(path, path.with_file_name(name));
            T::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Sample {
        name: String,
        #[serde(default)]
        count: u32,
    }

    // The signature is the one `load_json` expects of a migration.
    #[allow(clippy::unnecessary_wraps)]
    fn add_count(from: u32, mut data: Value) -> Result<Value> {
        if from == 1 {
            data["count"] = json!(7);
        }
        Ok(data)
    }

    #[test]
    fn round_trip() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("deep").join("sample.json");
        let sample = Sample {
            name: "a".into(),
            count: 3,
        };
        save_json(&path, 1, &sample).unwrap();
        let loaded: Sample = load_json(&path, 1, no_migration).unwrap();
        assert_eq!(loaded, sample);
        // No temporary files stay behind.
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn missing_file_is_default() {
        let folder = tempfile::tempdir().unwrap();
        let loaded: Sample = load_json(&folder.path().join("none.json"), 1, no_migration).unwrap();
        assert_eq!(loaded, Sample::default());
    }

    #[test]
    fn migrates_old_files() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("sample.json");
        fs::write(&path, r#"{"version":1,"data":{"name":"old"}}"#).unwrap();
        let loaded: Sample = load_json(&path, 2, add_count).unwrap();
        assert_eq!(
            loaded,
            Sample {
                name: "old".into(),
                count: 7
            }
        );
    }

    #[test]
    fn refuses_newer_files() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("sample.json");
        fs::write(&path, r#"{"version":9,"data":{"name":"new"}}"#).unwrap();
        let error = load_json::<Sample>(&path, 2, no_migration).unwrap_err();
        assert_eq!(error.code, "newerFile");
        // Defaults are used instead and the newer file is kept aside.
        let loaded: Sample = load_or_default(&path, 2, no_migration);
        assert_eq!(loaded, Sample::default());
        assert!(folder.path().join("sample.json.damaged").exists());
    }

    #[test]
    fn damaged_file_is_set_aside() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("sample.json");
        fs::write(&path, "{ not json").unwrap();
        let loaded: Sample = load_or_default(&path, 1, no_migration);
        assert_eq!(loaded, Sample::default());
        assert!(!path.exists());
        assert!(folder.path().join("sample.json.damaged").exists());
    }
}
