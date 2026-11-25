//! Commands of the "From the web" page: what a link leads to, search in the
//! open catalogues, previews and downloads.
//!
//! The window itself has no access to the network. Every request here is
//! started by the user: the address they pasted or the text they typed is
//! all that leaves the computer.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use tauri::Emitter;

use pixelit_ai::web::{self, Inspected, SearchPage, Step};
use pixelit_ai::{WebImage, WebSource};
use pixelit_core::{CancelToken, Error, Result};
use pixelit_engine::{JobContext, Lane};

use super::{display, CoreState};

/// Longest title of a download in the list of jobs, in characters.
const TITLE_CHARS: usize = 60;

/// The user pastes the links, so a picture on their own network is theirs
/// to ask for.
const LOCAL: bool = true;

/// Looks what a link leads to: a picture, or a page with pictures.
#[tauri::command(async)]
pub fn web_inspect(core: CoreState<'_>, url: String) -> Result<Inspected> {
    web::inspect(core.ai.net(), &url, LOCAL, &CancelToken::new())
}

/// Searches a catalogue; `page` starts at 1.
#[tauri::command(async)]
pub fn web_search(
    core: CoreState<'_>,
    source: WebSource,
    query: String,
    page: u32,
) -> Result<SearchPage> {
    web::search(core.ai.net(), source, &query, page)
}

/// Downloads a picture for the list of what was found and returns the file,
/// which the window shows through its own protocol.
#[tauri::command(async)]
pub fn web_preview(core: CoreState<'_>, url: String, page: Option<String>) -> Result<String> {
    let page = page.unwrap_or_default();
    web::preview(
        core.ai.net(),
        core.paths(),
        &url,
        &page,
        LOCAL,
        &CancelToken::new(),
    )
    .map(|path| display(&path))
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDownload {
    pub images: Vec<WebImage>,
    /// What the pictures were found by: the title of the page or the query.
    #[serde(default)]
    pub title: String,
}

/// What a picture is called while it is being downloaded.
fn label(image: &WebImage) -> &str {
    if image.title.trim().is_empty() {
        &image.url
    } else {
        image.title.trim()
    }
}

/// What the progress of a download is counted in: sizes are learnt one
/// picture at a time, so the whole is a thousand shares, not bytes.
const WHOLE: u64 = 1000;

/// The shares done: the pictures before this one and the received part of it.
fn done_of(step: &Step<'_>) -> u64 {
    // A size the server did not tell counts as nothing received yet.
    let part = step
        .bytes
        .min(step.total)
        .saturating_mul(WHOLE)
        .checked_div(step.total)
        .unwrap_or(0);
    let before = (step.index as u64).saturating_mul(WHOLE);
    ((before + part) / (step.count as u64).max(1)).min(WHOLE)
}

/// Downloads pictures as a job into the folder of the settings. The saved
/// files arrive as the result of the job, and the journal keeps where every
/// one of them came from.
#[tauri::command(async)]
pub fn web_download(core: CoreState<'_>, download: WebDownload) -> Result<u64> {
    core.ensure_not_updating()?;
    let Some(first) = download.images.first() else {
        return Err(Error::new("noImages"));
    };
    let named = if download.title.trim().is_empty() {
        label(first)
    } else {
        download.title.trim()
    };
    let title: String = named.chars().take(TITLE_CHARS).collect();
    let core = Arc::clone(core.inner());
    let jobs = &core.engine.jobs;
    Ok(jobs.spawn("download", &title, Lane::Io, {
        let core = Arc::clone(&core);
        let title = title.clone();
        move |job: &JobContext| {
            let folder = core.engine.settings().downloads(core.paths());
            let mut progress = |step: &Step<'_>| {
                let name = label(step.image);
                if step.count > 1 {
                    let current = format!("{}/{} · {name}", step.index + 1, step.count);
                    job.progress(done_of(step), WHOLE, &current);
                } else if step.total == 0 {
                    // The server did not tell the size: the bar only shows activity.
                    job.progress(0, 0, name);
                } else {
                    job.progress(done_of(step), WHOLE, name);
                }
            };
            let downloaded = web::download_all(
                core.ai.net(),
                core.paths(),
                &title,
                &download.images,
                &folder,
                LOCAL,
                &mut progress,
                &job.cancel,
            );
            // Files saved before a failure or a stop are in the journal too.
            let _ = core.app().emit("journal:changed", ());
            let downloaded = downloaded?;
            let files: Vec<String> = downloaded.files.iter().map(|path| display(path)).collect();
            Ok(json!({
                "files": files,
                "failed": downloaded.failed,
                "folder": display(&downloaded.folder),
            }))
        }
    }))
}
