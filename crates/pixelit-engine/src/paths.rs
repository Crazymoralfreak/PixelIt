//! Where PixelIt keeps its data.
//!
//! An installed copy stores data under `%APPDATA%\PixelIt`. A portable copy
//! (a `portable` marker file or a `data` folder next to the executable) keeps
//! everything in `data` beside the executable.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use pixelit_core::Result;

const APP_FOLDER: &str = "PixelIt";
const PORTABLE_MARKER: &str = "portable";
const PORTABLE_DATA: &str = "data";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub root: PathBuf,
    pub portable: bool,
}

impl Paths {
    /// Finds the data folder of this copy of the application.
    pub fn detect() -> Self {
        let beside = env::current_exe()
            .ok()
            .and_then(|executable| executable.parent().map(Path::to_path_buf));
        if let Some(folder) = beside {
            if folder.join(PORTABLE_MARKER).is_file() || folder.join(PORTABLE_DATA).is_dir() {
                return Self {
                    root: folder.join(PORTABLE_DATA),
                    portable: true,
                };
            }
        }
        let base = env::var_os("APPDATA")
            .or_else(|| env::var_os("XDG_DATA_HOME"))
            .or_else(|| env::var_os("HOME"))
            .map_or_else(env::temp_dir, PathBuf::from);
        Self {
            root: base.join(APP_FOLDER),
            portable: false,
        }
    }

    /// A data folder at a fixed place, for tests and tools.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            portable: false,
        }
    }

    pub fn settings(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    pub fn recipes(&self) -> PathBuf {
        self.root.join("recipes.json")
    }

    pub fn watch_rules(&self) -> PathBuf {
        self.root.join("watch.json")
    }

    pub fn journal(&self) -> PathBuf {
        self.root.join("journal.json")
    }

    pub fn undo(&self) -> PathBuf {
        self.root.join("undo.json")
    }

    pub fn ai(&self) -> PathBuf {
        self.root.join("ai.json")
    }

    pub fn mcp(&self) -> PathBuf {
        self.root.join("mcp.json")
    }

    pub fn chats(&self) -> PathBuf {
        self.root.join("chats.json")
    }

    pub fn thumbs(&self) -> PathBuf {
        self.root.join("cache").join("thumbs")
    }

    pub fn temp(&self) -> PathBuf {
        self.root.join("cache").join("temp")
    }

    pub fn models(&self) -> PathBuf {
        self.root.join("models")
    }

    pub fn components(&self) -> PathBuf {
        self.root.join("components")
    }

    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn generated(&self) -> PathBuf {
        self.root.join("generated")
    }

    /// The record of pictures the studio has generated.
    pub fn generated_index(&self) -> PathBuf {
        self.root.join("generated.json")
    }

    /// Styles saved by the user. The built-in styles are not in this file.
    pub fn styles(&self) -> PathBuf {
        self.root.join("styles.json")
    }

    /// Pictures downloaded from the web, unless the settings name a folder.
    pub fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }

    /// Creates the folders the application writes into.
    pub fn ensure(&self) -> Result<()> {
        for folder in [
            self.root.clone(),
            self.thumbs(),
            self.temp(),
            self.models(),
            self.components(),
            self.logs(),
            self.generated(),
        ] {
            fs::create_dir_all(folder)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_live_under_the_root() {
        let paths = Paths::at("/data");
        for path in [
            paths.settings(),
            paths.recipes(),
            paths.watch_rules(),
            paths.journal(),
            paths.undo(),
            paths.thumbs(),
            paths.models(),
            paths.logs(),
            paths.generated(),
            paths.downloads(),
        ] {
            assert!(path.starts_with("/data"), "{path:?}");
        }
    }

    #[test]
    fn ensure_creates_folders() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path().join("nested"));
        paths.ensure().unwrap();
        assert!(paths.thumbs().is_dir());
        assert!(paths.logs().is_dir());
    }
}
