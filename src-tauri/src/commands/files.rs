//! Commands of the viewer and the editor: folders, file facts, editor
//! sessions, pictures from the clipboard and the screen.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::Serialize;

use pixelit_codecs::{probe_file, SourceInfo};
use pixelit_core::pipeline::{Conflict, Step};
use pixelit_core::{CancelToken, Error, ImageFormat, Result};
use pixelit_engine::clock::LocalTime;
use pixelit_engine::preview::{histogram, Histogram, PreviewSession};
use pixelit_engine::{fsops, scan, thumbs};

use super::neural::confirm_steps;
use super::{display, CoreState};
use crate::app::{lock, Editor};

/// Quality of the pictures the editor shows.
const EDITOR_QUALITY: u8 = 90;
/// Largest picture accepted from the clipboard, in bytes.
const MAX_PASTE: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub bytes: u64,
    /// Time of the last change, in milliseconds since 1970.
    pub modified: u64,
}

impl FileEntry {
    pub fn of(path: &Path) -> Self {
        let metadata = fs::metadata(path).ok();
        let modified = metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_millis() as u64);
        Self {
            path: display(path),
            name: path
                .file_name()
                .map_or_default(|name| name.to_string_lossy().into_owned()),
            bytes: metadata.map_or(0, |metadata| metadata.len()),
            modified,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderListing {
    pub folder: String,
    pub parent: Option<String>,
    pub folders: Vec<FileEntry>,
    pub files: Vec<FileEntry>,
}

fn hidden(name: &str) -> bool {
    name.starts_with('.') || name.starts_with('$')
}

/// Subfolders and pictures of a folder, sorted by name.
#[tauri::command(async)]
pub fn folder_list(folder: String) -> Result<FolderListing> {
    let root = PathBuf::from(&folder);
    let entries = fs::read_dir(&root).map_err(|error| Error::from(error).with("path", &folder))?;
    let mut folders = Vec::new();
    let mut files = Vec::new();
    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if hidden(&name) {
            continue;
        }
        if path.is_dir() {
            folders.push(FileEntry::of(&path));
        } else if scan::is_image(&path) {
            files.push(FileEntry::of(&path));
        }
    }
    let by_name = |a: &FileEntry, b: &FileEntry| a.name.to_lowercase().cmp(&b.name.to_lowercase());
    folders.sort_by(by_name);
    files.sort_by(by_name);
    Ok(FolderListing {
        parent: root.parent().map(display),
        folder,
        folders,
        files,
    })
}

/// Pictures among the given files and folders: what a drop or a dialog adds
/// to a queue.
#[tauri::command(async)]
pub fn files_expand(inputs: Vec<String>, recursive: bool) -> Vec<FileEntry> {
    let inputs: Vec<PathBuf> = inputs.into_iter().map(PathBuf::from).collect();
    scan::collect(&inputs, recursive, None)
        .iter()
        .map(|item| FileEntry::of(&item.source))
        .collect()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub file: FileEntry,
    pub info: SourceInfo,
}

/// Size, format and other facts of a picture, read without decoding it.
#[tauri::command(async)]
pub fn file_info(path: String) -> Result<FileInfo> {
    let path = PathBuf::from(path);
    Ok(FileInfo {
        info: probe_file(&path)?,
        file: FileEntry::of(&path),
    })
}

/// Moves files to the Recycle Bin. Returns how many were moved.
#[tauri::command(async)]
pub fn files_trash(paths: Vec<String>) -> Result<u32> {
    let mut moved = 0;
    for path in paths {
        fsops::to_trash(Path::new(&path))?;
        moved += 1;
    }
    Ok(moved)
}

// Editor sessions

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorInfo {
    pub id: u64,
    pub path: String,
    /// Size of the preview in pixels.
    pub width: u32,
    pub height: u32,
    /// Size of the preview relative to the full file.
    pub scale: f32,
    pub info: SourceInfo,
}

/// Opens a picture in the editor, reduced to `max_side` for the screen.
#[tauri::command(async)]
pub fn editor_open(
    core: CoreState<'_>,
    path: String,
    max_side: u32,
    page: u32,
) -> Result<EditorInfo> {
    let cancel = CancelToken::new();
    let session = PreviewSession::open(
        &core.engine.runner(),
        Path::new(&path),
        max_side.clamp(256, 8192),
        page,
        &cancel,
    )?;
    let base = thumbs::encode_for_screen(session.base(), EDITOR_QUALITY)?;
    let info = EditorInfo {
        id: 0,
        path,
        width: session.base().width(),
        height: session.base().height(),
        scale: session.scale(),
        info: session.info().clone(),
    };
    let id = core.add_editor(Editor {
        session: Mutex::new(session),
        rendered: Mutex::new(base.clone()),
        base,
        version: AtomicU64::new(0),
        cancel: Mutex::new(cancel),
    });
    Ok(EditorInfo { id, ..info })
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderInfo {
    /// Number of this render, to put into the address of the picture.
    pub version: u64,
    pub width: u32,
    pub height: u32,
    pub histogram: Histogram,
}

/// Renders the steps on the preview of a session. A newer call stops the one
/// in progress, which then fails with `cancelled`. `confirmed` tells that the
/// user agreed to send the picture to a cloud provider, when a step needs one.
#[tauri::command(async)]
pub fn editor_render(
    core: CoreState<'_>,
    id: u64,
    steps: Vec<Step>,
    confirmed: bool,
) -> Result<RenderInfo> {
    confirm_steps(&core, &steps, confirmed)?;
    let editor = core.editor(id)?;
    let cancel = {
        let mut current = lock(&editor.cancel);
        current.cancel();
        *current = CancelToken::new();
        current.clone()
    };
    let image = lock(&editor.session).render(&core.engine.runner(), &steps, &cancel)?;
    cancel.check()?;
    let encoded = thumbs::encode_for_screen(&image, EDITOR_QUALITY)?;
    cancel.check()?;
    *lock(&editor.rendered) = encoded;
    Ok(RenderInfo {
        version: editor.version.fetch_add(1, Ordering::SeqCst) + 1,
        width: image.width(),
        height: image.height(),
        histogram: histogram(&image),
    })
}

#[tauri::command(async)]
pub fn editor_close(core: CoreState<'_>, id: u64) {
    core.close_editor(id);
}

// Pictures from outside

/// Saves a picture pasted from the clipboard (base64 of a PNG, JPEG or WebP
/// file) and returns the path of the file.
#[tauri::command(async)]
pub fn paste_image(core: CoreState<'_>, data: String) -> Result<String> {
    if data.len() > MAX_PASTE {
        return Err(Error::new("fileTooLarge"));
    }
    let bytes = STANDARD
        .decode(data.trim())
        .map_err(|_| Error::new("badImage"))?;
    let format = ImageFormat::sniff(&bytes).ok_or_else(|| Error::new("badImage"))?;
    let folder = core.paths().root.join("captures");
    let stamp = LocalTime::now().stamp();
    let path = folder.join(format!("PixelIt {stamp}.{}", format.extension()));
    let written = fsops::write_file(&path, &bytes, Conflict::Rename)?;
    Ok(display(written.path()))
}

/// Takes a picture of the whole desktop and returns the path of the file.
#[tauri::command(async)]
pub fn capture_screen(core: CoreState<'_>) -> Result<String> {
    core.capture_screen()
}
