//! Log files: one file per day under the `logs` folder, a week kept.

use std::path::Path;

use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};

/// Days of logs kept on disk.
const KEPT_FILES: usize = 7;

/// Starts logging into `folder`. The returned guard flushes the log when it
/// is dropped, so the caller keeps it until the process ends. `None` means
/// logging could not start; the application works without it.
pub fn init(folder: &Path, verbose: bool) -> Option<WorkerGuard> {
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("pixelit")
        .filename_suffix("log")
        .max_log_files(KEPT_FILES)
        .build(folder)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let level = if verbose { Level::DEBUG } else { Level::INFO };
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_max_level(level)
        .try_init()
        .ok()?;
    Some(guard)
}
