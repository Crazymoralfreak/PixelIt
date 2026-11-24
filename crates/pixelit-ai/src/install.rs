//! Installing and removing catalogue entries.
//!
//! An entry is installed into its own folder; a marker file is written last,
//! so an interrupted installation never looks complete.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::json;

use pixelit_core::{CancelToken, Error, Result};
use pixelit_engine::store::write_atomic;
use pixelit_engine::Paths;

use crate::catalog::{self, Download, Entry, Kind, Unpack};
use crate::net::Net;

const MARKER: &str = "installed.json";

/// Largest total size an archive may unpack to: 2 GiB.
const MAX_UNPACKED: u64 = 2 * 1024 * 1024 * 1024;

/// Names stable-diffusion.cpp has used for its command line program.
const GENERATOR_PROGRAMS: [&str; 2] = ["sd-cli.exe", "sd.exe"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub id: &'static str,
    pub kind: Kind,
    pub name: &'static str,
    pub license: &'static str,
    pub source: &'static str,
    pub bytes: u64,
    pub installed: bool,
    pub requires: Vec<&'static str>,
}

fn entry(id: &str) -> Result<&'static Entry> {
    catalog::find(id).ok_or_else(|| Error::new("unknownComponent").with("id", id))
}

/// Folder an entry is installed into.
pub fn folder(paths: &Paths, entry: &Entry) -> PathBuf {
    let root = if entry.kind.is_component() {
        paths.components()
    } else {
        paths.models()
    };
    root.join(entry.id)
}

fn find_program(folder: &Path) -> Option<PathBuf> {
    for name in GENERATOR_PROGRAMS {
        let direct = folder.join(name);
        if direct.is_file() {
            return Some(direct);
        }
    }
    // Some archives keep everything in one inner folder.
    for inner in fs::read_dir(folder).ok()?.flatten() {
        let inner = inner.path();
        if inner.is_dir() {
            for name in GENERATOR_PROGRAMS {
                let nested = inner.join(name);
                if nested.is_file() {
                    return Some(nested);
                }
            }
        }
    }
    None
}

fn main_file(paths: &Paths, entry: &Entry) -> Option<PathBuf> {
    let folder = folder(paths, entry);
    if !folder.join(MARKER).is_file() {
        return None;
    }
    if entry.kind == Kind::Generator {
        return find_program(&folder);
    }
    let file = folder.join(entry.main_file());
    file.is_file().then_some(file)
}

pub fn is_installed(paths: &Paths, id: &str) -> bool {
    catalog::find(id).is_some_and(|entry| main_file(paths, entry).is_some())
}

/// Path of the model, library or program of an installed entry.
pub fn main_path(paths: &Paths, id: &str) -> Result<PathBuf> {
    main_file(paths, entry(id)?).ok_or_else(|| Error::new("componentMissing").with("id", id))
}

/// Every catalogue entry with its state.
pub fn list(paths: &Paths) -> Vec<Status> {
    catalog::ENTRIES
        .iter()
        .map(|entry| Status {
            id: entry.id,
            kind: entry.kind,
            name: entry.name,
            license: entry.license,
            source: entry.source,
            bytes: entry.bytes(),
            installed: main_file(paths, entry).is_some(),
            requires: entry.requires.to_vec(),
        })
        .collect()
}

/// Installed entries of a kind, in catalogue order.
pub fn installed_of(paths: &Paths, kind: Kind) -> Vec<&'static Entry> {
    catalog::of_kind(kind)
        .filter(|entry| main_file(paths, entry).is_some())
        .collect()
}

/// The entry and everything it needs that is still missing, dependencies first.
fn plan(paths: &Paths, id: &str) -> Result<Vec<&'static Entry>> {
    let target = entry(id)?;
    let mut order = Vec::new();
    for required in target.requires {
        let required = entry(required)?;
        if main_file(paths, required).is_none() {
            order.push(required);
        }
    }
    order.push(target);
    Ok(order)
}

/// Download size of an installation, dependencies included.
pub fn install_size(paths: &Paths, id: &str) -> Result<u64> {
    Ok(plan(paths, id)?.iter().map(|entry| entry.bytes()).sum())
}

fn archive_name(download: &Download) -> &str {
    download.url.rsplit('/').next().unwrap_or("download.bin")
}

fn copy_entry(reader: &mut impl io::Read, target: &Path, budget: &mut u64) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create(target)?;
    let written = io::copy(&mut io::Read::take(&mut *reader, *budget + 1), &mut file)?;
    if written > *budget {
        drop(file);
        let _ = fs::remove_file(target);
        return Err(Error::new("badArchive").detail("unpacked size is too large"));
    }
    *budget -= written;
    Ok(())
}

fn bad_archive(error: zip::result::ZipError) -> Error {
    Error::new("badArchive").detail(error)
}

fn unpack(archive: &Path, download: &Download, folder: &Path, cancel: &CancelToken) -> Result<()> {
    let mut zip = zip::ZipArchive::new(File::open(archive)?).map_err(bad_archive)?;
    let mut budget = MAX_UNPACKED;
    match download.unpack {
        Unpack::File(_) => {}
        Unpack::Entries(entries) => {
            for (inside, outside) in entries {
                cancel.check()?;
                let mut file = zip.by_name(inside).map_err(bad_archive)?;
                copy_entry(&mut file, &folder.join(outside), &mut budget)?;
            }
        }
        Unpack::All => {
            for index in 0..zip.len() {
                cancel.check()?;
                let mut file = zip.by_index(index).map_err(bad_archive)?;
                // Names that would leave the folder are skipped.
                let Some(name) = file.enclosed_name() else {
                    continue;
                };
                let target = folder.join(name);
                if file.is_dir() {
                    fs::create_dir_all(&target)?;
                } else {
                    copy_entry(&mut file, &target, &mut budget)?;
                }
            }
        }
    }
    Ok(())
}

fn install_entry(
    net: &Net,
    paths: &Paths,
    entry: &Entry,
    progress: &mut dyn FnMut(u64),
    cancel: &CancelToken,
) -> Result<()> {
    let folder = folder(paths, entry);
    fs::create_dir_all(&folder)?;
    let _ = fs::remove_file(folder.join(MARKER));
    let mut before = 0u64;
    for download in entry.downloads {
        cancel.check()?;
        let mut report = |have: u64| progress(before + have);
        match download.unpack {
            Unpack::File(name) => {
                net.download(
                    download.url,
                    &folder.join(name),
                    download.bytes,
                    &download.checksum,
                    &mut report,
                    cancel,
                )?;
            }
            Unpack::Entries(_) | Unpack::All => {
                let archive = paths.temp().join("downloads").join(archive_name(download));
                net.download(
                    download.url,
                    &archive,
                    download.bytes,
                    &download.checksum,
                    &mut report,
                    cancel,
                )?;
                let unpacked = unpack(&archive, download, &folder, cancel);
                let _ = fs::remove_file(&archive);
                unpacked?;
            }
        }
        before += download.bytes;
    }
    let marker = json!({ "id": entry.id, "name": entry.name });
    write_atomic(&folder.join(MARKER), marker.to_string().as_bytes())?;
    if main_file(paths, entry).is_none() {
        return Err(Error::new("badArchive").detail("the main file is missing"));
    }
    Ok(())
}

/// Downloads and installs an entry with everything it needs.
/// `progress` receives the downloaded and the total number of bytes.
pub fn install(
    net: &Net,
    paths: &Paths,
    id: &str,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> Result<()> {
    let order = plan(paths, id)?;
    let total: u64 = order.iter().map(|entry| entry.bytes()).sum();
    let mut before = 0u64;
    for entry in order {
        if main_file(paths, entry).is_none() {
            let mut report = |have: u64| progress(before + have, total);
            install_entry(net, paths, entry, &mut report, cancel)?;
        }
        before += entry.bytes();
        progress(before, total);
    }
    Ok(())
}

/// Deletes an installed entry.
pub fn remove(paths: &Paths, id: &str) -> Result<()> {
    let folder = folder(paths, entry(id)?);
    match fs::remove_dir_all(&folder) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;
    use crate::net::Checksum;

    fn paths() -> (tempfile::TempDir, Paths) {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        (folder, paths)
    }

    #[test]
    fn nothing_is_installed_at_first() {
        let (_folder, paths) = paths();
        assert!(list(&paths).iter().all(|status| !status.installed));
        assert!(!is_installed(&paths, catalog::RUNTIME));
        assert_eq!(
            main_path(&paths, "lama").unwrap_err().code,
            "componentMissing"
        );
        assert_eq!(
            main_path(&paths, "nope").unwrap_err().code,
            "unknownComponent"
        );
    }

    #[test]
    fn marker_and_file_make_an_entry_installed() {
        let (_folder, paths) = paths();
        let entry = catalog::find("u2netp").unwrap();
        let folder = folder(&paths, entry);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(entry.main_file()), b"model").unwrap();
        assert!(!is_installed(&paths, "u2netp"));
        fs::write(folder.join(MARKER), b"{}").unwrap();
        assert!(is_installed(&paths, "u2netp"));
        assert_eq!(installed_of(&paths, Kind::Background), vec![entry]);
        remove(&paths, "u2netp").unwrap();
        assert!(!is_installed(&paths, "u2netp"));
        remove(&paths, "u2netp").unwrap();
    }

    #[test]
    fn plans_put_dependencies_first() {
        let (_folder, paths) = paths();
        let order: Vec<&str> = plan(&paths, "gfpgan-1.4")
            .unwrap()
            .iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(order, [catalog::RUNTIME, catalog::DETECTOR, "gfpgan-1.4"]);
        assert!(install_size(&paths, "gfpgan-1.4").unwrap() > 340_000_000);
    }

    #[test]
    fn archives_are_unpacked_safely() {
        let folder = tempfile::tempdir().unwrap();
        let archive = folder.path().join("test.zip");
        {
            let mut writer = zip::ZipWriter::new(File::create(&archive).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("bin/tool.exe", options).unwrap();
            writer.write_all(b"program").unwrap();
            writer.start_file("../escape.txt", options).unwrap();
            writer.write_all(b"no").unwrap();
            writer.finish().unwrap();
        }
        let cancel = CancelToken::new();
        let all = Download {
            url: "https://example.invalid/test.zip",
            bytes: 1,
            checksum: Checksum::Sha256(""),
            unpack: Unpack::All,
        };
        let out = folder.path().join("out");
        fs::create_dir_all(&out).unwrap();
        unpack(&archive, &all, &out, &cancel).unwrap();
        assert_eq!(fs::read(out.join("bin/tool.exe")).unwrap(), b"program");
        assert!(!folder.path().join("escape.txt").exists());

        let named = Download {
            unpack: Unpack::Entries(&[("bin/tool.exe", "tool.exe")]),
            ..all
        };
        let picked = folder.path().join("picked");
        unpack(&archive, &named, &picked, &cancel).unwrap();
        assert_eq!(fs::read(picked.join("tool.exe")).unwrap(), b"program");

        let missing = Download {
            unpack: Unpack::Entries(&[("absent.dll", "absent.dll")]),
            ..all
        };
        assert_eq!(
            unpack(&archive, &missing, &picked, &cancel)
                .unwrap_err()
                .code,
            "badArchive"
        );
    }

    #[test]
    fn generator_program_is_found_in_an_inner_folder() {
        let folder = tempfile::tempdir().unwrap();
        assert!(find_program(folder.path()).is_none());
        let inner = folder.path().join("build");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("sd.exe"), b"x").unwrap();
        assert_eq!(find_program(folder.path()).unwrap(), inner.join("sd.exe"));
    }
}
