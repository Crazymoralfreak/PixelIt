//! PixelIt engine: everything between the pixel operations of `pixelit-core`
//! and the interfaces (window, command line, MCP server).
//!
//! The engine runs pipelines on files, processes batches, keeps recipes,
//! settings and the journal, watches folders and owns the job queue.

#![forbid(unsafe_code)]

pub mod batch;
pub mod clock;
pub mod compose;
pub mod compress;
pub mod duplicates;
pub mod environment;
pub mod fonts;
pub mod fsops;
pub mod generators;
pub mod jobs;
pub mod journal;
pub mod logging;
pub mod paths;
pub mod preview;
pub mod process;
pub mod recipes;
pub mod scan;
pub mod settings;
pub mod store;
pub mod thumbs;
pub mod watch;

use std::sync::{Arc, PoisonError, RwLock};

use pixelit_core::{Limits, Result};

pub use crate::fonts::FileResources;
pub use crate::jobs::{JobContext, JobInfo, JobState, Jobs, Lane};
pub use crate::paths::Paths;
pub use crate::process::{Item, Outcome, Runner, SharedAi};
pub use crate::settings::Settings;

/// The shared state of a running PixelIt process.
pub struct Engine {
    pub paths: Paths,
    pub jobs: Jobs,
    resources: Arc<FileResources>,
    settings: RwLock<Settings>,
    ai: RwLock<Option<SharedAi>>,
}

impl Engine {
    /// Opens the data folder and loads the settings.
    pub fn open(paths: Paths) -> Result<Self> {
        paths.ensure()?;
        let settings = Settings::load(&paths);
        Ok(Self {
            paths,
            jobs: Jobs::new(),
            resources: Arc::new(FileResources::new()),
            settings: RwLock::new(settings),
            ai: RwLock::new(None),
        })
    }

    /// A copy of the current settings.
    pub fn settings(&self) -> Settings {
        self.settings
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Replaces the settings and saves them.
    pub fn set_settings(&self, mut settings: Settings) -> Result<Settings> {
        settings.normalize();
        settings.save(&self.paths)?;
        *self
            .settings
            .write()
            .unwrap_or_else(PoisonError::into_inner) = settings.clone();
        Ok(settings)
    }

    /// Changes the settings in place and saves them.
    pub fn update_settings(&self, change: impl FnOnce(&mut Settings)) -> Result<Settings> {
        let mut settings = self.settings();
        change(&mut settings);
        self.set_settings(settings)
    }

    /// Connects the neural backend used by AI steps.
    pub fn set_ai(&self, ai: Option<SharedAi>) {
        *self.ai.write().unwrap_or_else(PoisonError::into_inner) = ai;
    }

    pub fn resources(&self) -> Arc<FileResources> {
        Arc::clone(&self.resources)
    }

    /// What is needed to run pipelines with the current settings.
    pub fn runner(&self) -> Runner {
        Runner {
            resources: Arc::clone(&self.resources),
            ai: self
                .ai
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
            limits: Limits::default(),
            pdfium: self.settings().pdfium(),
            paths: Some(self.paths.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_keeps_settings() {
        let folder = tempfile::tempdir().unwrap();
        let engine = Engine::open(Paths::at(folder.path())).unwrap();
        assert_eq!(engine.settings(), Settings::default());
        let saved = engine
            .update_settings(|settings| {
                settings.language = "de".into();
                settings.threads = 100_000;
            })
            .unwrap();
        assert_eq!(saved.threads, 256);
        let again = Engine::open(Paths::at(folder.path())).unwrap();
        assert_eq!(again.settings().language, "de");
        assert!(again.runner().ai.is_none());
        assert!(!again.jobs.is_busy());
    }
}
