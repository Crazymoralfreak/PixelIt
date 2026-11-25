//! Application settings shared by the window, the command line and the MCP
//! server.

use std::path::{Path, PathBuf};

use pixelit_core::pipeline::OutputSpec;
use pixelit_core::Result;
use serde::{Deserialize, Serialize};

use crate::paths::Paths;
use crate::store;

/// Version of `settings.json`.
pub const SETTINGS_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    /// Follow the theme of the operating system.
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

/// What agents (the assistant, MCP clients, the MCP server) may do with files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct AgentAccess {
    /// Folders agents may read and write. Empty means agents have no file access.
    pub folders: Vec<String>,
    /// Let agents replace existing files; replaced files go to the Recycle Bin.
    pub allow_overwrite: bool,
    /// Let agents move files to the Recycle Bin.
    pub allow_delete: bool,
    /// Let agents start downloads of models and components.
    pub allow_downloads: bool,
    /// Let agents send images to cloud providers.
    pub allow_cloud: bool,
}

impl AgentAccess {
    /// Whether the path lies inside one of the allowed folders. The path is
    /// compared after resolving `..` and links, so a path cannot step outside.
    pub fn allows(&self, path: &Path) -> bool {
        let Some(resolved) = resolve(path) else {
            return false;
        };
        self.folders.iter().any(|folder| {
            resolve(Path::new(folder)).is_some_and(|folder| resolved.starts_with(folder))
        })
    }
}

/// The absolute form of a path. A file that does not exist yet is resolved
/// through its nearest existing parent.
fn resolve(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        if let Ok(resolved) = current.canonicalize() {
            let mut resolved = resolved;
            for part in missing.iter().rev() {
                resolved.push(part);
            }
            return Some(resolved);
        }
        let name = current.file_name()?;
        if name == ".." || name == "." {
            return None;
        }
        missing.push(name.to_os_string());
        current = current.parent()?;
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Interface language code; empty follows the operating system.
    pub language: String,
    pub theme: Theme,
    /// Worker threads of batch processing; `0` uses every processor core.
    pub threads: u32,
    /// Memory batch processing may hold in decoded pixels, in megabytes.
    pub memory_limit_mb: u32,
    /// Output settings offered for a new batch.
    pub output: OutputSpec,
    /// Ask before replacing existing files.
    pub confirm_overwrite: bool,
    /// Ask before moving files to the Recycle Bin.
    pub confirm_delete: bool,
    /// Largest size of the thumbnail cache, in megabytes.
    pub thumb_cache_mb: u32,
    pub close_to_tray: bool,
    pub start_minimized: bool,
    pub autostart: bool,
    pub context_menu: bool,
    pub check_updates: bool,
    pub update_channel: UpdateChannel,
    /// Version whose "What's new" page the user has seen.
    pub seen_version: String,
    /// Newer version the user chose to skip; it is not offered again.
    pub skipped_version: String,
    /// Global shortcut of the screen capture; empty switches it off.
    pub capture_hotkey: String,
    /// Folder of the PDFium library; empty looks next to the application.
    pub pdfium_folder: String,
    pub agents: AgentAccess,
    /// Folders opened recently, newest first.
    pub recent_folders: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: String::new(),
            theme: Theme::System,
            threads: 0,
            memory_limit_mb: 4096,
            output: OutputSpec::default(),
            confirm_overwrite: true,
            confirm_delete: true,
            thumb_cache_mb: 512,
            close_to_tray: false,
            start_minimized: false,
            autostart: false,
            context_menu: false,
            check_updates: true,
            update_channel: UpdateChannel::Stable,
            seen_version: String::new(),
            skipped_version: String::new(),
            capture_hotkey: String::new(),
            pdfium_folder: String::new(),
            agents: AgentAccess::default(),
            recent_folders: Vec::new(),
        }
    }
}

/// Most recent folders remembered.
const RECENT_LIMIT: usize = 12;

impl Settings {
    pub fn load(paths: &Paths) -> Self {
        let mut settings: Self =
            store::load_or_default(&paths.settings(), SETTINGS_VERSION, store::no_migration);
        settings.normalize();
        settings
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        store::save_json(&paths.settings(), SETTINGS_VERSION, self)
    }

    /// Brings values typed by hand into their valid ranges.
    pub fn normalize(&mut self) {
        self.threads = self.threads.min(256);
        self.memory_limit_mb = self.memory_limit_mb.clamp(256, 262_144);
        self.thumb_cache_mb = self.thumb_cache_mb.clamp(16, 65_536);
        self.output.quality = self.output.quality.clamp(1, 100);
        self.recent_folders.truncate(RECENT_LIMIT);
    }

    /// Number of worker threads to use.
    pub fn worker_threads(&self) -> usize {
        if self.threads > 0 {
            return self.threads as usize;
        }
        std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get)
    }

    /// Folder of the PDFium library, when one is configured.
    pub fn pdfium(&self) -> Option<PathBuf> {
        let folder = self.pdfium_folder.trim();
        (!folder.is_empty()).then(|| PathBuf::from(folder))
    }

    pub fn remember_folder(&mut self, folder: &str) {
        self.recent_folders.retain(|known| known != folder);
        self.recent_folders.insert(0, folder.to_owned());
        self.recent_folders.truncate(RECENT_LIMIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        assert_eq!(Settings::load(&paths), Settings::default());
        let settings = Settings {
            language: "ru".into(),
            threads: 3,
            ..Settings::default()
        };
        settings.save(&paths).unwrap();
        assert_eq!(Settings::load(&paths), settings);
        assert_eq!(settings.worker_threads(), 3);
    }

    #[test]
    fn normalize_clamps() {
        let mut settings = Settings {
            threads: 100_000,
            memory_limit_mb: 1,
            ..Settings::default()
        };
        settings.output.quality = 0;
        settings.normalize();
        assert_eq!(settings.threads, 256);
        assert_eq!(settings.memory_limit_mb, 256);
        assert_eq!(settings.output.quality, 1);
    }

    #[test]
    fn agents_stay_inside_allowed_folders() {
        let folder = tempfile::tempdir().unwrap();
        let allowed = folder.path().join("allowed");
        std::fs::create_dir_all(&allowed).unwrap();
        let access = AgentAccess {
            folders: vec![allowed.display().to_string()],
            ..AgentAccess::default()
        };
        assert!(access.allows(&allowed.join("new.png")));
        assert!(access.allows(&allowed.join("deep").join("new.png")));
        assert!(!access.allows(&folder.path().join("other.png")));
        assert!(!access.allows(&allowed.join("..").join("other.png")));
        assert!(!access.allows(Path::new("relative.png")));
        assert!(!AgentAccess::default().allows(&allowed.join("new.png")));
    }

    #[test]
    fn recent_folders_are_unique() {
        let mut settings = Settings::default();
        settings.remember_folder("a");
        settings.remember_folder("b");
        settings.remember_folder("a");
        assert_eq!(settings.recent_folders, ["a", "b"]);
    }
}
