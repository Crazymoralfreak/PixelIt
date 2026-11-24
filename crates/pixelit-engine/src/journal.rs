//! The journal: what PixelIt did, shown on the history page.

use pixelit_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;
use crate::store;

/// Version of `journal.json`.
pub const JOURNAL_VERSION: u32 = 1;

/// Entries kept.
const JOURNAL_LIMIT: usize = 500;

/// Failed files listed per entry.
pub const ERRORS_LISTED: usize = 20;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JournalError {
    pub path: String,
    pub error: Option<Error>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JournalEntry {
    pub id: String,
    /// Unix time in seconds.
    pub time: i64,
    /// Stable kind: `batch`, `watch`, `edit`, `generate`, `agent`, ...
    pub kind: String,
    pub title: String,
    pub done: u32,
    pub skipped: u32,
    pub failed: u32,
    pub source_bytes: u64,
    pub output_bytes: u64,
    pub elapsed_ms: u64,
    pub cancelled: bool,
    /// Folder holding the results, to open from the journal.
    pub folder: String,
    /// Undo record of the entry, while the action can still be reversed.
    pub undo_id: Option<String>,
    pub errors: Vec<JournalError>,
}

impl JournalEntry {
    pub fn new(kind: &str, title: &str) -> Self {
        let time = chrono::Utc::now().timestamp();
        Self {
            id: format!("{time:x}-{:08x}", fastrand::u32(..)),
            time,
            kind: kind.to_owned(),
            title: title.to_owned(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Journal {
    pub entries: Vec<JournalEntry>,
}

impl Journal {
    pub fn load(paths: &Paths) -> Self {
        store::load_or_default(&paths.journal(), JOURNAL_VERSION, store::no_migration)
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        store::save_json(&paths.journal(), JOURNAL_VERSION, self)
    }

    /// Adds an entry to the journal on disk.
    pub fn push(paths: &Paths, entry: JournalEntry) -> Result<()> {
        let mut journal = Self::load(paths);
        journal.entries.push(entry);
        let extra = journal.entries.len().saturating_sub(JOURNAL_LIMIT);
        journal.entries.drain(..extra);
        journal.save(paths)
    }

    /// Marks the action of an undo record as reversed.
    pub fn forget_undo(paths: &Paths, undo_id: &str) -> Result<()> {
        let mut journal = Self::load(paths);
        let mut changed = false;
        for entry in &mut journal.entries {
            if entry.undo_id.as_deref() == Some(undo_id) {
                entry.undo_id = None;
                changed = true;
            }
        }
        if changed {
            journal.save(paths)?;
        }
        Ok(())
    }

    pub fn clear(paths: &Paths) -> Result<()> {
        Self::default().save(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_entries_and_forgets_undo() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        let mut entry = JournalEntry::new("batch", "Resize");
        entry.undo_id = Some("u1".into());
        entry.done = 3;
        Journal::push(&paths, entry.clone()).unwrap();
        Journal::push(&paths, JournalEntry::new("edit", "Crop")).unwrap();
        let journal = Journal::load(&paths);
        assert_eq!(journal.entries.len(), 2);
        assert_eq!(journal.entries[0], entry);

        Journal::forget_undo(&paths, "u1").unwrap();
        assert_eq!(Journal::load(&paths).entries[0].undo_id, None);
        Journal::clear(&paths).unwrap();
        assert!(Journal::load(&paths).entries.is_empty());
    }
}
