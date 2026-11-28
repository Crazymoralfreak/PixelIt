//! Commands of the interface, mirrored by `src/api/index.ts`. They all run on
//! the worker pool: some wait for a dialog or decode a picture, which the
//! main thread must stay free for.

pub mod agents;
pub mod apps;
pub mod files;
pub mod neural;
pub mod studio;
pub mod web;
pub mod work;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde::Serialize;
use tauri::State;
use tauri_plugin_dialog::DialogExt;

use pixelit_core::{Error, Result};
use pixelit_engine::{thumbs, JobInfo, Settings};

use crate::app::{AppInfo, Core};
use crate::shell::text::text;
use crate::shell::window;
use crate::update::{self, UpdateStatus};

pub type CoreState<'a> = State<'a, Arc<Core>>;

/// Extensions offered by the "open pictures" dialog.
const PICTURES: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "avif", "jxl", "heic", "heif", "tif", "tiff", "bmp", "gif",
    "ico", "svg", "pdf", "psd", "dng", "cr2", "cr3", "nef", "arw", "raf", "orf", "rw2", "pef",
    "srw",
];

/// The release notes shipped with this version, in English and in Russian.
const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");
const CHANGELOG_RU: &str = include_str!("../../../CHANGELOG.ru.md");

pub fn display(path: &Path) -> String {
    path.display().to_string()
}

// Application and settings

#[tauri::command(async)]
pub fn app_info(core: CoreState<'_>) -> AppInfo {
    core.info()
}

#[tauri::command(async)]
pub fn app_quit(core: CoreState<'_>) {
    core.inner().quit();
}

#[tauri::command(async)]
pub fn window_show(core: CoreState<'_>) {
    window::show_main(core.app());
}

/// Files PixelIt was started with; handed out once.
#[tauri::command(async)]
pub fn opened_files(core: CoreState<'_>) -> Vec<String> {
    core.take_opened()
}

#[tauri::command(async)]
pub fn settings_get(core: CoreState<'_>) -> Settings {
    core.engine.settings()
}

#[tauri::command(async)]
pub fn settings_set(core: CoreState<'_>, settings: Settings) -> Result<Settings> {
    core.set_settings(settings)
}

/// The release notes in the language of the interface; every language
/// except Russian reads the English text.
#[tauri::command(async)]
pub fn changelog(core: CoreState<'_>) -> &'static str {
    if core.lang() == "ru" {
        CHANGELOG_RU
    } else {
        CHANGELOG
    }
}

// Updates

#[tauri::command(async)]
pub fn update_status(core: CoreState<'_>) -> UpdateStatus {
    core.updates.status()
}

/// Starts a check in the background; the progress arrives as `update:status` events.
#[tauri::command(async)]
pub fn update_check(core: CoreState<'_>) {
    update::check_now(core.inner());
}

/// The user agreed to the version found: it is downloaded and installed in
/// the background, with the progress in `update:status` events.
#[tauri::command(async)]
pub fn update_install(core: CoreState<'_>) -> Result<()> {
    update::install_now(core.inner())
}

// Jobs

#[tauri::command(async)]
pub fn jobs_list(core: CoreState<'_>) -> Vec<JobInfo> {
    core.engine.jobs.list()
}

#[tauri::command(async)]
pub fn job_cancel(core: CoreState<'_>, id: u64) {
    core.engine.jobs.cancel(id);
}

#[tauri::command(async)]
pub fn job_pause(core: CoreState<'_>, id: u64, paused: bool) {
    core.engine.jobs.set_paused(id, paused);
}

#[tauri::command(async)]
pub fn jobs_clear(core: CoreState<'_>) {
    core.engine.jobs.clear_finished();
}

// Dialogs and Explorer

fn dialog_title(core: &Core, key: &str) -> String {
    text(&core.lang(), key, &[])
}

/// Asks for pictures. Returns nothing when the dialog was closed.
#[tauri::command(async)]
pub fn pick_files(core: CoreState<'_>, multiple: bool) -> Vec<String> {
    let dialog = core
        .app()
        .dialog()
        .file()
        .set_title(dialog_title(&core, "native.dialog.openPictures"))
        .add_filter(dialog_title(&core, "native.dialog.pictures"), PICTURES)
        .add_filter(dialog_title(&core, "native.dialog.allFiles"), &["*"]);
    let chosen = if multiple {
        dialog.blocking_pick_files().unwrap_or_default()
    } else {
        dialog.blocking_pick_file().into_iter().collect()
    };
    chosen
        .into_iter()
        .filter_map(|file| file.into_path().ok())
        .map(|path| display(&path))
        .collect()
}

/// Asks for a file of one kind, such as a recipe or a checkpoint.
#[tauri::command(async)]
pub fn pick_file(core: CoreState<'_>, name: String, extensions: Vec<String>) -> Option<String> {
    let extensions: Vec<&str> = extensions.iter().map(String::as_str).collect();
    let mut dialog = core.app().dialog().file();
    if !extensions.is_empty() {
        dialog = dialog.add_filter(name, &extensions);
    }
    dialog
        .add_filter(dialog_title(&core, "native.dialog.allFiles"), &["*"])
        .blocking_pick_file()
        .and_then(|file| file.into_path().ok())
        .map(|path| display(&path))
}

#[tauri::command(async)]
pub fn pick_folder(core: CoreState<'_>, start: Option<String>) -> Option<String> {
    let mut dialog = core
        .app()
        .dialog()
        .file()
        .set_title(dialog_title(&core, "native.dialog.openFolder"));
    if let Some(start) = start.filter(|start| Path::new(start).is_dir()) {
        dialog = dialog.set_directory(start);
    }
    dialog
        .blocking_pick_folder()
        .and_then(|folder| folder.into_path().ok())
        .map(|path| display(&path))
}

/// Asks where to save a file with the proposed name.
#[tauri::command(async)]
pub fn pick_save(
    core: CoreState<'_>,
    file_name: String,
    name: String,
    extensions: Vec<String>,
) -> Option<String> {
    let extensions: Vec<&str> = extensions.iter().map(String::as_str).collect();
    let mut dialog = core.app().dialog().file().set_file_name(file_name);
    if !extensions.is_empty() {
        dialog = dialog.add_filter(name, &extensions);
    }
    dialog
        .blocking_save_file()
        .and_then(|file| file.into_path().ok())
        .map(|path| display(&path))
}

/// Shows a file selected in the file manager, or opens a folder.
#[tauri::command(async)]
pub fn reveal_path(path: String) -> Result<()> {
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err(Error::new("notFound").with("path", path));
    }
    let mut command = reveal_command(&path, &target);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| Error::from(error).with("path", path))
}

/// The program that reveals a path: Explorer, Finder or the desktop opener.
fn reveal_command(path: &str, target: &Path) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("explorer.exe");
        if target.is_dir() {
            command.arg(explorer_path(path));
        } else {
            // Explorer reads this argument by its own rules, so it is passed as written.
            command.raw_arg(format!("/select,\"{}\"", explorer_path(path)));
        }
        command
    }
    #[cfg(target_os = "macos")]
    {
        let _ = path;
        let mut command = Command::new("open");
        if target.is_file() {
            command.arg("-R");
        }
        command.arg(target);
        command
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = path;
        let mut command = Command::new("xdg-open");
        let folder = if target.is_dir() {
            target
        } else {
            target.parent().unwrap_or(target)
        };
        command.arg(folder);
        command
    }
}

/// The path as Explorer takes it. With a forward slash in it, Explorer does
/// not find the file and opens the user's documents instead.
#[cfg(windows)]
fn explorer_path(path: &str) -> String {
    path.trim().replace('/', "\\")
}

/// Opens the folder with the log files.
#[tauri::command(async)]
pub fn reveal_logs(core: CoreState<'_>) -> Result<()> {
    reveal_path(display(&core.paths().logs()))
}

// Thumbnail cache and fonts

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheInfo {
    pub bytes: u64,
}

#[tauri::command(async)]
pub fn cache_info(core: CoreState<'_>) -> CacheInfo {
    CacheInfo {
        bytes: thumbs::cache_size(core.paths()),
    }
}

#[tauri::command(async)]
pub fn cache_clear(core: CoreState<'_>) -> CacheInfo {
    thumbs::clear_cache(core.paths());
    pixelit_ai::web::clear_previews(core.paths());
    cache_info(core)
}

/// Names of the fonts installed in the system, for text and watermarks.
#[tauri::command(async)]
pub fn fonts_list(core: CoreState<'_>) -> Vec<String> {
    core.engine.resources().families()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn explorer_gets_a_path_with_backslashes_only() {
        assert_eq!(
            explorer_path(r"D:\Programs\ComfyUI\models/upscale_models\4x-UltraSharp.pt"),
            r"D:\Programs\ComfyUI\models\upscale_models\4x-UltraSharp.pt"
        );
        assert_eq!(explorer_path(r"C:\a\b.png"), r"C:\a\b.png");
    }
}
