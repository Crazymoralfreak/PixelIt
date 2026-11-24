//! Commands that process files: batches, recipes, watched folders, the
//! journal, the generators of file sets and the search for duplicates.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::Emitter;

use pixelit_codecs::{encode, encode_media, EncodeOptions};
use pixelit_core::pipeline::{Conflict, Destination};
use pixelit_core::{CancelToken, Error, Image, ImageFormat, Pipeline, Result};
use pixelit_engine::batch::{self, BatchRequest};
use pixelit_engine::compose::{self, AnimationSpec, CollageSpec, PdfSpec};
use pixelit_engine::compress::{self, Comparison};
use pixelit_engine::duplicates;
use pixelit_engine::fsops::{self, UndoLog, UndoReport};
use pixelit_engine::generators::{
    self, Generated, IconRequest, SocialSize, WebKitRequest, SOCIAL_SIZES,
};
use pixelit_engine::journal::{Journal, JournalEntry};
use pixelit_engine::recipes::{self, Recipe, Recipes};
use pixelit_engine::watch::{WatchRule, WatchRules};
use pixelit_engine::{scan, Item, Lane, Runner};

use super::neural::confirm_steps;
use super::{display, CoreState};
use crate::app::Core;
use crate::shell::text::text;
use crate::shell::{notify, window};

fn to_value<T: Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|error| Error::new("internal").detail(error))
}

/// The subfolder results go into, which a scan of the sources leaves out.
fn output_subfolder(pipeline: &Pipeline) -> Option<&str> {
    match &pipeline.output.destination {
        Destination::Subfolder { name } if !name.is_empty() => Some(name),
        _ => None,
    }
}

// Batches

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchStart {
    pub title: String,
    /// Files and folders to process.
    pub inputs: Vec<String>,
    pub recursive: bool,
    pub pipeline: Pipeline,
    /// The user agreed to send pictures to a cloud provider, when a step
    /// needs one.
    #[serde(default)]
    pub confirmed: bool,
}

/// Tells about a finished batch when the window is out of sight.
fn announce(core: &Core, title: &str, done: u32, failed: u32) {
    if window::main_visible(core.app()) {
        return;
    }
    let lang = core.lang();
    let body = text(
        &lang,
        "native.notify.batchDoneText",
        &[("done", &done.to_string()), ("failed", &failed.to_string())],
    );
    let headline = if title.is_empty() {
        text(&lang, "native.notify.batchDone", &[])
    } else {
        title.to_owned()
    };
    notify(core.app(), &headline, &body);
}

/// Queues a batch and returns the number of its job. The report arrives as
/// the result of the job.
#[tauri::command(async)]
pub fn batch_start(core: CoreState<'_>, request: BatchStart) -> Result<u64> {
    core.ensure_not_updating()?;
    confirm_steps(&core, &request.pipeline.steps, request.confirmed)?;
    let inputs: Vec<PathBuf> = request.inputs.iter().map(PathBuf::from).collect();
    let items = scan::collect(
        &inputs,
        request.recursive,
        output_subfolder(&request.pipeline),
    );
    if items.is_empty() {
        return Err(Error::new("noImages"));
    }
    let settings = core.engine.settings();
    let lane = if request.pipeline.uses_ai() {
        Lane::Gpu
    } else {
        Lane::Cpu
    };
    if let Some(folder) = inputs.iter().find(|input| input.is_dir()) {
        let folder = display(folder);
        let _ = core
            .engine
            .update_settings(|settings| settings.remember_folder(&folder));
    }
    let batch = BatchRequest {
        title: request.title.clone(),
        kind: String::from("batch"),
        items,
        pipeline: request.pipeline,
        threads: settings.worker_threads(),
        memory_limit: u64::from(settings.memory_limit_mb) * 1024 * 1024,
    };
    let core = Arc::clone(core.inner());
    let jobs = &core.engine.jobs;
    Ok(jobs.spawn("batch", &request.title, lane, {
        let core = Arc::clone(&core);
        move |job| {
            let report = batch::run(&core.engine.runner(), Some(core.paths()), &batch, job)?;
            let _ = core.app().emit("journal:changed", ());
            announce(&core, &batch.title, report.done, report.failed);
            to_value(&report)
        }
    }))
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    pub bytes: u64,
    pub source_bytes: u64,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub quality: Option<u8>,
    pub score: Option<f32>,
    pub target_missed: bool,
}

/// Runs a pipeline on one file without writing anything: the size and the
/// quality the real run would give.
#[tauri::command(async)]
pub fn export_estimate(core: CoreState<'_>, path: String, pipeline: Pipeline) -> Result<Estimate> {
    let item = Item::new(&path);
    let encoded = core
        .engine
        .runner()
        .export(&item, &pipeline, &CancelToken::new())?;
    Ok(Estimate {
        bytes: encoded.bytes.len() as u64,
        source_bytes: std::fs::metadata(&path).map_or(0, |metadata| metadata.len()),
        format: encoded.format,
        width: encoded.width,
        height: encoded.height,
        quality: encoded.quality,
        score: encoded.score,
        target_missed: encoded.target_missed,
    })
}

/// Encodes one file into several formats with the same settings.
#[tauri::command(async)]
pub fn compare_formats(
    core: CoreState<'_>,
    path: String,
    pipeline: Pipeline,
    formats: Vec<ImageFormat>,
) -> Result<Vec<Comparison>> {
    let cancel = CancelToken::new();
    let decoded = core
        .engine
        .runner()
        .render(&Item::new(&path), &pipeline, &cancel)?;
    Ok(compress::compare_formats(
        &decoded.media,
        &pipeline.output,
        &formats,
        &decoded.metadata,
        &cancel,
    ))
}

// Recipes

#[tauri::command(async)]
pub fn recipes_list(core: CoreState<'_>) -> Vec<Recipe> {
    Recipes::load(core.paths()).all()
}

#[tauri::command(async)]
pub fn recipe_save(core: CoreState<'_>, recipe: Recipe) -> Result<Recipe> {
    let mut all = Recipes::load(core.paths());
    let saved = all.upsert(recipe);
    all.save(core.paths())?;
    Ok(saved)
}

#[tauri::command(async)]
pub fn recipe_remove(core: CoreState<'_>, id: String) -> Result<bool> {
    let mut all = Recipes::load(core.paths());
    let removed = all.remove(&id);
    all.save(core.paths())?;
    Ok(removed)
}

#[tauri::command(async)]
pub fn recipe_export(core: CoreState<'_>, id: String, path: String) -> Result<()> {
    let recipe = Recipes::load(core.paths())
        .get(&id)
        .ok_or_else(|| Error::new("recipeNotFound").with("name", &id))?;
    recipes::export(&recipe, Path::new(&path))
}

#[tauri::command(async)]
pub fn recipe_import(core: CoreState<'_>, path: String) -> Result<Recipe> {
    let recipe = recipes::import(Path::new(&path))?;
    recipe_save(core, recipe)
}

// Watched folders

#[tauri::command(async)]
pub fn watch_list(core: CoreState<'_>) -> Vec<WatchRule> {
    WatchRules::load(core.paths()).rules
}

#[tauri::command(async)]
pub fn watch_save(core: CoreState<'_>, rule: WatchRule) -> Result<WatchRule> {
    let mut rules = WatchRules::load(core.paths());
    let saved = rules.upsert(rule);
    rules.save(core.paths())?;
    core.inner().restart_watch();
    Ok(saved)
}

#[tauri::command(async)]
pub fn watch_remove(core: CoreState<'_>, id: String) -> Result<bool> {
    let mut rules = WatchRules::load(core.paths());
    let removed = rules.remove(&id);
    rules.save(core.paths())?;
    core.inner().restart_watch();
    Ok(removed)
}

// Journal

#[tauri::command(async)]
pub fn journal_list(core: CoreState<'_>) -> Vec<JournalEntry> {
    Journal::load(core.paths()).entries
}

#[tauri::command(async)]
pub fn journal_clear(core: CoreState<'_>) -> Result<()> {
    Journal::clear(core.paths())
}

/// Takes back a run: its files go to the Recycle Bin, replaced originals
/// come back from it.
#[tauri::command(async)]
pub fn journal_undo(core: CoreState<'_>, undo_id: String) -> Result<UndoReport> {
    let report = UndoLog::undo(core.paths(), &undo_id)?;
    Journal::forget_undo(core.paths(), &undo_id)?;
    let _ = core.app().emit("journal:changed", ());
    Ok(report)
}

// Sets of files

#[tauri::command(async)]
pub fn icons_generate(core: CoreState<'_>, request: IconRequest) -> Result<Generated> {
    generators::icons(&core.engine.runner(), &request, &CancelToken::new())
}

#[tauri::command(async)]
pub fn social_sizes() -> &'static [SocialSize] {
    SOCIAL_SIZES
}

#[tauri::command(async)]
pub fn social_generate(
    core: CoreState<'_>,
    source: String,
    folder: String,
    ids: Vec<String>,
    quality: u8,
) -> Result<Generated> {
    generators::social(
        &core.engine.runner(),
        Path::new(&source),
        Path::new(&folder),
        &ids,
        quality,
        &CancelToken::new(),
    )
}

#[tauri::command(async)]
pub fn web_kit_generate(core: CoreState<'_>, request: WebKitRequest) -> Result<Generated> {
    generators::web_kit(&core.engine.runner(), &request, &CancelToken::new())
}

// Several pictures into one file

fn load_all(runner: &Runner, files: &[String], cancel: &CancelToken) -> Result<Vec<Image>> {
    files
        .iter()
        .map(|file| {
            runner
                .load(Path::new(file), 0, cancel)?
                .media
                .into_first()
                .ok_or_else(|| Error::new("emptyImage").with("path", file))
        })
        .collect()
}

fn format_of(output: &str) -> Result<ImageFormat> {
    Path::new(output)
        .extension()
        .and_then(|extension| extension.to_str())
        .and_then(ImageFormat::from_extension)
        .filter(|format| format.can_encode())
        .ok_or_else(|| Error::new("cannotEncode").with("path", output))
}

/// Writes a file next to nothing it could replace and returns its path.
fn write_new(output: &str, bytes: &[u8]) -> Result<String> {
    fsops::write_file(Path::new(output), bytes, Conflict::Rename)
        .map(|written| display(written.path()))
}

#[tauri::command(async)]
pub fn collage_create(
    core: CoreState<'_>,
    files: Vec<String>,
    spec: CollageSpec,
    output: String,
    quality: u8,
) -> Result<String> {
    let cancel = CancelToken::new();
    let format = format_of(&output)?;
    let images = load_all(&core.engine.runner(), &files, &cancel)?;
    let image = compose::collage(&images, &spec, &cancel)?;
    let options = EncodeOptions::new(format).with_quality(quality);
    write_new(&output, &encode(&image, &options, &cancel)?)
}

#[tauri::command(async)]
pub fn pdf_create(
    core: CoreState<'_>,
    files: Vec<String>,
    spec: PdfSpec,
    output: String,
) -> Result<String> {
    let cancel = CancelToken::new();
    let images = load_all(&core.engine.runner(), &files, &cancel)?;
    write_new(&output, &compose::pdf(&images, &spec, &cancel)?)
}

#[tauri::command(async)]
pub fn animation_create(
    core: CoreState<'_>,
    files: Vec<String>,
    spec: AnimationSpec,
    output: String,
    quality: u8,
) -> Result<String> {
    let cancel = CancelToken::new();
    let format = format_of(&output)?;
    let images = load_all(&core.engine.runner(), &files, &cancel)?;
    let media = compose::animation(images, &spec, &cancel)?;
    let options = EncodeOptions::new(format).with_quality(quality);
    write_new(&output, &encode_media(&media, &options, &cancel)?)
}

// Duplicates

/// Looks for equal and similar pictures in a folder, as a job. The groups
/// arrive as the result of the job.
#[tauri::command(async)]
pub fn duplicates_find(
    core: CoreState<'_>,
    folder: String,
    recursive: bool,
    tolerance: u32,
) -> Result<u64> {
    core.ensure_not_updating()?;
    let files = scan::folder_files(Path::new(&folder), recursive, None);
    if files.is_empty() {
        return Err(Error::new("noImages"));
    }
    let core = Arc::clone(core.inner());
    let jobs = &core.engine.jobs;
    Ok(jobs.spawn("duplicates", &folder, Lane::Cpu, {
        let core = Arc::clone(&core);
        move |job| {
            let distance = tolerance.min(duplicates::MAX_DISTANCE);
            let groups = duplicates::find(&core.engine.runner(), &files, distance, job)?;
            to_value(&groups)
        }
    }))
}
