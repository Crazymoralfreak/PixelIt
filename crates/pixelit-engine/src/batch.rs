//! Batch processing: one pipeline over many files, in parallel, with pause,
//! cancel, progress and an undo record.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Instant;

use pixelit_codecs::probe_file;
use pixelit_core::{Error, Pipeline, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::fsops::{UndoLog, UndoRecord};
use crate::jobs::JobContext;
use crate::journal::{Journal, JournalEntry, JournalError, ERRORS_LISTED};
use crate::paths::Paths;
use crate::process::{Item, Outcome, OutcomeStatus, Runner};

/// Memory assumed for a file whose size cannot be read in advance.
const UNKNOWN_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Copies of the pixels alive while one file is processed: the decoded
/// source, the working copy of a step and the buffer of the encoder.
const COPIES_IN_FLIGHT: u64 = 3;

/// Worker threads of a batch with neural steps: the model is the bottleneck,
/// and more threads only queue up for it.
const AI_THREADS: usize = 2;

#[derive(Clone, Debug, Default)]
pub struct BatchRequest {
    pub title: String,
    /// Journal kind: `batch`, `watch`, `agent`, ...
    pub kind: String,
    pub items: Vec<Item>,
    pub pipeline: Pipeline,
    /// Worker threads; `0` uses every processor core.
    pub threads: usize,
    /// Memory the batch may hold in decoded pixels, in bytes; `0` is no limit.
    pub memory_limit: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchItem {
    pub source: PathBuf,
    pub outcome: Option<Outcome>,
    pub error: Option<Error>,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchReport {
    pub items: Vec<BatchItem>,
    pub done: u32,
    pub skipped: u32,
    pub failed: u32,
    pub source_bytes: u64,
    pub output_bytes: u64,
    pub elapsed_ms: u64,
    pub cancelled: bool,
    /// Folder of the first result.
    pub folder: String,
    pub undo_id: Option<String>,
}

/// Keeps the decoded pixels of a batch within a memory limit: a file waits
/// until the files before it release enough room. A file larger than the
/// whole limit runs alone.
struct Budget {
    limit: u64,
    used: Mutex<u64>,
    freed: Condvar,
}

struct Reserved<'a> {
    budget: &'a Budget,
    bytes: u64,
}

impl Budget {
    fn new(limit: u64) -> Self {
        Self {
            limit,
            used: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    fn reserve(&self, bytes: u64) -> Reserved<'_> {
        if self.limit == 0 {
            return Reserved {
                budget: self,
                bytes: 0,
            };
        }
        let bytes = bytes.min(self.limit);
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        while *used > 0 && *used + bytes > self.limit {
            used = self
                .freed
                .wait(used)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *used += bytes;
        Reserved {
            budget: self,
            bytes,
        }
    }
}

impl Drop for Reserved<'_> {
    fn drop(&mut self) {
        if self.bytes == 0 {
            return;
        }
        let mut used = self
            .budget
            .used
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *used = used.saturating_sub(self.bytes);
        drop(used);
        self.budget.freed.notify_all();
    }
}

/// Memory one file needs while it is processed.
fn memory_estimate(item: &Item) -> u64 {
    probe_file(&item.source).map_or(UNKNOWN_FILE_BYTES, |info| {
        u64::from(info.width)
            .saturating_mul(u64::from(info.height))
            .saturating_mul(u64::from(info.frames.max(1)))
            .saturating_mul(4 * COPIES_IN_FLIGHT)
    })
}

fn summarize(items: Vec<BatchItem>, started: Instant, cancelled: bool) -> BatchReport {
    let mut report = BatchReport {
        elapsed_ms: started.elapsed().as_millis() as u64,
        cancelled,
        ..BatchReport::default()
    };
    for item in &items {
        match (&item.outcome, &item.error) {
            (Some(outcome), _) => {
                if report.folder.is_empty() {
                    if let Some(folder) = outcome.output.parent() {
                        report.folder = folder.display().to_string();
                    }
                }
                if outcome.status == OutcomeStatus::Skipped {
                    report.skipped += 1;
                } else {
                    report.done += 1;
                    report.source_bytes += outcome.source_bytes;
                    report.output_bytes += outcome.output_bytes;
                }
            }
            (None, Some(_)) => report.failed += 1,
            (None, None) => {}
        }
    }
    report.items = items;
    report
}

fn journal_entry(request: &BatchRequest, report: &BatchReport) -> JournalEntry {
    let kind = if request.kind.is_empty() {
        "batch"
    } else {
        &request.kind
    };
    let mut entry = JournalEntry::new(kind, &request.title);
    entry.done = report.done;
    entry.skipped = report.skipped;
    entry.failed = report.failed;
    entry.source_bytes = report.source_bytes;
    entry.output_bytes = report.output_bytes;
    entry.elapsed_ms = report.elapsed_ms;
    entry.cancelled = report.cancelled;
    entry.folder.clone_from(&report.folder);
    entry.undo_id.clone_from(&report.undo_id);
    entry.errors = report
        .items
        .iter()
        .filter(|item| item.error.is_some())
        .take(ERRORS_LISTED)
        .map(|item| JournalError {
            path: item.source.display().to_string(),
            error: item.error.clone(),
        })
        .collect();
    entry
}

/// Runs a batch. Files are independent: a failed file is reported and the
/// rest goes on. With `paths` the run is written into the journal and gets an
/// undo record. A cancelled run keeps the files finished so far and reports
/// them the same way.
pub fn run(
    runner: &Runner,
    paths: Option<&Paths>,
    request: &BatchRequest,
    job: &JobContext,
) -> Result<BatchReport> {
    let started = Instant::now();
    let total = request.items.len() as u64;
    let mut threads = if request.threads == 0 {
        std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get)
    } else {
        request.threads
    };
    if request.pipeline.uses_ai() {
        threads = threads.min(AI_THREADS);
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads.clamp(1, 256))
        .thread_name(|index| format!("batch-{index}"))
        .build()
        .map_err(|error| Error::new("internal").detail(error))?;
    let budget = Budget::new(request.memory_limit);
    let finished = AtomicU64::new(0);
    job.progress(0, total, "");

    let items: Vec<BatchItem> = pool.install(|| {
        request
            .items
            .par_iter()
            .map(|item| {
                let item_started = Instant::now();
                let result = job.checkpoint().and_then(|()| {
                    let _reserved = budget.reserve(memory_estimate(item));
                    job.checkpoint()?;
                    runner.process(item, &request.pipeline, &job.cancel)
                });
                let done = finished.fetch_add(1, Ordering::Relaxed) + 1;
                let name = item
                    .source
                    .file_name()
                    .map_or_default(|name| name.to_string_lossy().into_owned());
                job.progress(done, total, &name);
                let (outcome, error) = match result {
                    Ok(outcome) => (Some(outcome), None),
                    Err(error) if error.is_cancelled() => (None, None),
                    Err(error) => {
                        tracing::warn!(path = %item.source.display(), %error, "file failed");
                        (None, Some(error))
                    }
                };
                BatchItem {
                    source: item.source.clone(),
                    outcome,
                    error,
                    elapsed_ms: item_started.elapsed().as_millis() as u64,
                }
            })
            .collect()
    });

    let cancelled = job.cancel.is_cancelled();
    let mut report = summarize(items, started, cancelled);
    if let Some(paths) = paths {
        let mut record = UndoRecord::new(&request.title);
        for outcome in report.items.iter().filter_map(|item| item.outcome.as_ref()) {
            outcome.note(&mut record);
        }
        if !record.is_empty() {
            report.undo_id = Some(record.id.clone());
            UndoLog::push(paths, record)?;
        }
        Journal::push(paths, journal_entry(request, &report))?;
    }
    tracing::info!(
        done = report.done,
        skipped = report.skipped,
        failed = report.failed,
        elapsed_ms = report.elapsed_ms,
        cancelled,
        "batch finished"
    );
    if cancelled {
        return Err(Error::cancelled());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use pixelit_codecs::{encode, EncodeOptions};
    use pixelit_core::pipeline::OutputFormat;
    use pixelit_core::{CancelToken, Color, Depth, Image, ImageFormat};

    use super::*;
    use crate::scan;

    fn write_png(path: &Path) {
        let image = Image::new(24, 16, Depth::Eight, Color::rgb(200, 80, 40)).unwrap();
        let bytes = encode(
            &image,
            &EncodeOptions::new(ImageFormat::Png),
            &CancelToken::new(),
        )
        .unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn batch_reports_every_file() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("in");
        fs::create_dir_all(root.join("sub")).unwrap();
        for name in ["a.png", "b.png", "sub/c.png"] {
            write_png(&root.join(name));
        }
        fs::write(root.join("broken.png"), b"not a picture").unwrap();
        let data = Paths::at(folder.path().join("data"));

        let mut pipeline = Pipeline::default();
        pipeline.output.format = OutputFormat::Webp;
        let request = BatchRequest {
            title: "To WebP".into(),
            items: scan::collect(std::slice::from_ref(&root), true, Some("PixelIt")),
            pipeline,
            threads: 2,
            memory_limit: 1024 * 1024,
            ..BatchRequest::default()
        };
        assert_eq!(request.items.len(), 4);
        let report = run(
            &Runner::default(),
            Some(&data),
            &request,
            &JobContext::detached(),
        )
        .unwrap();
        assert_eq!((report.done, report.failed, report.skipped), (3, 1, 0));
        assert!(root.join("PixelIt").join("a.webp").is_file());
        assert!(root.join("PixelIt").join("sub").join("c.webp").is_file());
        assert!(report.output_bytes > 0);

        let undo = UndoLog::load(&data);
        assert_eq!(undo.records.len(), 1);
        assert_eq!(undo.records[0].created.len(), 3);
        assert_eq!(report.undo_id.as_deref(), Some(undo.records[0].id.as_str()));
        let journal = Journal::load(&data);
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(journal.entries[0].failed, 1);
        assert_eq!(journal.entries[0].errors.len(), 1);
        assert_eq!(journal.entries[0].kind, "batch");
    }

    #[test]
    fn cancelled_batch_is_an_error() {
        let folder = tempfile::tempdir().unwrap();
        write_png(&folder.path().join("a.png"));
        let request = BatchRequest {
            items: scan::collect(&[folder.path().to_path_buf()], false, None),
            ..BatchRequest::default()
        };
        let job = JobContext::detached();
        job.cancel.cancel();
        let error = run(&Runner::default(), None, &request, &job).unwrap_err();
        assert!(error.is_cancelled());
        assert!(!folder.path().join("PixelIt").exists());
    }

    #[test]
    fn budget_lets_a_large_file_run_alone() {
        let budget = Budget::new(100);
        let first = budget.reserve(1000);
        assert_eq!(first.bytes, 100);
        drop(first);
        let a = budget.reserve(60);
        let empty = Budget::new(0);
        let b = empty.reserve(5000);
        assert_eq!((a.bytes, b.bytes), (60, 0));
    }
}
