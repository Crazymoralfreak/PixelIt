//! The studio: generation, styles and the pictures already made.
//!
//! A request leaves this computer only when the chosen generator is outside
//! it and the user has agreed. The pictures and the request that made them
//! stay in the application's data.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::Emitter;

use pixelit_ai::generate::{self, save_generated, GenerateRequest, Outcome, Source};
use pixelit_ai::parameters::{self, Parameters};
use pixelit_ai::{history, styles, AppKind, Expand, GenerationProvider, Style};
use pixelit_core::ops::annotate::stroke_mask;
use pixelit_core::pipeline::{Mask, Stroke};
use pixelit_core::{Error, Image, Result};
use pixelit_engine::{JobContext, Lane};

use super::neural::confirm_cloud;
use super::{display, CoreState};
use crate::app::Core;

fn changed(core: &Core) {
    let _ = core.app().emit("generated:changed", ());
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_default(|name| name.to_string_lossy().into_owned())
}

fn union(mut base: Mask, extra: &Mask) -> Mask {
    for (pixel, cover) in base.data.iter_mut().zip(&extra.data) {
        if *cover > *pixel {
            *pixel = *cover;
        }
    }
    base
}

/// The picture to send, grown when asked, and the area the generator may change.
fn prepare_source(
    image: Image,
    strokes: &[Stroke],
    expand: Expand,
) -> Result<(Image, Option<Mask>)> {
    let (image, grown) = if expand.is_empty() {
        (image, None)
    } else {
        let (canvas, mask) = pixelit_ai::outpaint::prepare(&image, expand)?;
        (canvas, Some(mask))
    };
    if strokes.is_empty() {
        return Ok((image, grown));
    }
    let painted = stroke_mask(image.width(), image.height(), strokes);
    let mask = Some(match grown {
        Some(grown) => union(grown, &painted),
        None => painted,
    });
    Ok((image, mask))
}

fn note_of(outcome: &Outcome, image: &Image, seed: u64, embed: bool) -> Option<String> {
    if !embed {
        return None;
    }
    Some(parameters::write(&Parameters {
        prompt: outcome.prompt.clone(),
        negative: outcome.negative.clone(),
        steps: outcome.steps,
        cfg: outcome.cfg,
        seed,
        width: image.width(),
        height: image.height(),
        model: outcome.model.clone(),
        sampler: outcome.tuning.sampler.clone(),
    }))
}

fn remember(
    core: &Core,
    outcome: &Outcome,
    files: &[PathBuf],
    images: &[Image],
    start: &GenerateStart,
) -> Result<history::Record> {
    let pictures = files
        .iter()
        .zip(images)
        .enumerate()
        .map(|(index, (path, image))| history::Picture {
            file: file_name(path),
            seed: outcome.seeds.get(index).copied().unwrap_or(0),
            favorite: false,
            width: image.width(),
            height: image.height(),
        })
        .collect();
    let kind = if !start.expand.is_empty() {
        history::GeneratedKind::Outpaint
    } else if start.source.is_some() {
        history::GeneratedKind::Edit
    } else {
        history::GeneratedKind::Create
    };
    let entry = history::record(
        core.paths(),
        history::Record {
            prompt: start.request.prompt.clone(),
            negative: start.request.negative.clone(),
            styles: start.request.styles.clone(),
            final_prompt: outcome.prompt.clone(),
            final_negative: outcome.negative.clone(),
            provider: outcome.provider,
            model: outcome.model.clone(),
            width: outcome.width,
            height: outcome.height,
            steps: outcome.steps,
            cfg: outcome.cfg,
            strength: start.request.strength,
            source: start.source.clone().unwrap_or_default(),
            kind,
            pictures,
            tuning: outcome.tuning.clone(),
            ..history::Record::default()
        },
    )?;
    changed(core);
    Ok(entry)
}

// ---------------------------------------------------------------- generation

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GenerateStart {
    pub request: GenerateRequest,
    /// Picture to start from. Empty creates a new picture.
    pub source: Option<String>,
    /// Area to change. Empty changes the whole picture.
    pub strokes: Vec<Stroke>,
    pub expand: Expand,
    /// The user agreed to send the picture and the prompt off this computer.
    pub confirmed: bool,
    /// 2 or 4 enlarges every picture with the local model before it is kept.
    pub upscale: u32,
}

/// The enlargement a request asks for: 0, 2 or 4.
fn enlargement(asked: u32) -> u32 {
    match asked {
        0 | 1 => 0,
        2 | 3 => 2,
        _ => 4,
    }
}

/// Generates pictures as a job. The result is the history entry, including
/// the paths of the saved files.
#[tauri::command(async)]
pub fn generate_start(core: CoreState<'_>, start: GenerateStart) -> Result<u64> {
    core.ensure_not_updating()?;
    if start.request.prompt.trim().is_empty() {
        return Err(Error::new("emptyPrompt"));
    }
    let settings = core.ai.settings();
    let provider = match start.request.provider {
        GenerationProvider::None => settings.generation.provider,
        chosen => chosen,
    };
    confirm_cloud(
        &core,
        generate::leaves_with(&settings, provider),
        start.confirmed,
    )?;
    let enlarge = enlargement(start.upscale);
    // Asked before the generation, so a long run does not end without its picture.
    if enlarge > 0
        && pixelit_ai::upscale::uses_builtin(&settings)
        && !core.ai.capabilities().upscale
    {
        return Err(Error::new("modelMissing").with("model", "upscale"));
    }
    let title: String = start.request.prompt.chars().take(60).collect();
    let core = Arc::clone(core.inner());
    Ok(core.engine.jobs.spawn("generate", &title, Lane::Gpu, {
        let core = Arc::clone(&core);
        move |job: &JobContext| {
            let loaded = match &start.source {
                Some(path) => Some(
                    core.engine
                        .runner()
                        .load(Path::new(path), 0, &job.cancel)?
                        .media
                        .into_first()
                        .ok_or_else(|| Error::new("emptyImage").with("path", path))?,
                ),
                None => None,
            };
            let (picture, mask) = match loaded {
                Some(image) => {
                    let (image, mask) = prepare_source(image, &start.strokes, start.expand)?;
                    (Some(image), mask)
                }
                None => (None, None),
            };
            let source = picture.as_ref().map(|image| Source {
                image,
                mask: mask.as_ref(),
            });
            let app = core.app().clone();
            let tick = |index: u32, count: u32, step: u32, steps: u32, preview: Option<&[u8]>| {
                emit_preview(&app, index, count, step, steps, preview);
            };
            let outcome = core
                .ai
                .generate(&start.request, source.as_ref(), &job.cancel, &tick)?;
            let placed: Vec<Image> = match &picture {
                Some(picture) => outcome
                    .images
                    .iter()
                    .map(|image| pixelit_ai::place(picture, image, mask.as_ref()))
                    .collect::<Result<Vec<_>>>()?,
                None => outcome.images.clone(),
            };
            let embed = core.ai.settings().generation.embed_parameters;
            let notes: Vec<String> = placed
                .iter()
                .enumerate()
                .map(|(index, image)| {
                    note_of(
                        &outcome,
                        image,
                        outcome.seeds.get(index).copied().unwrap_or(0),
                        embed,
                    )
                    .unwrap_or_default()
                })
                .collect();
            // The note keeps the size that was generated, so a repeat asks for it again.
            let mut notice = "";
            let kept = if enlarge > 0 {
                let total = placed.len() as u64;
                let label = format!("×{enlarge}");
                let mut enlarged = Vec::with_capacity(placed.len());
                for (index, image) in placed.iter().enumerate() {
                    job.progress(index as u64, total, &label);
                    let done = core.ai.enlarge(
                        image,
                        enlarge,
                        &pixelit_ai::upscale::Notes::from_outcome(&outcome, index),
                        index as u32,
                        placed.len() as u32,
                        &job.cancel,
                        &tick,
                    )?;
                    if notice.is_empty() {
                        notice = done.notice;
                    }
                    enlarged.push(done.image);
                }
                enlarged
            } else {
                placed
            };
            let files = save_generated(core.paths(), &kept, Some(&notes))?;
            let mut entry = remember(&core, &outcome, &files, &kept, &start)?;
            let folder = core.paths().generated();
            for picture in &mut entry.pictures {
                picture.file = display(&folder.join(&picture.file));
            }
            with_notice(&entry, notice)
        }
    }))
}

#[tauri::command(async)]
pub fn generation_models(core: CoreState<'_>, provider: String) -> Result<pixelit_ai::ModelList> {
    pixelit_ai::checkpoints::list(
        core.ai.net(),
        core.paths(),
        &core.ai.settings(),
        GenerationProvider::parse(&provider),
    )
}

/// The samplers, schedules, VAE and LoRA files of a generator. A generator
/// that is not running gives an error, and the form keeps what it has.
#[tauri::command(async)]
pub fn generation_options(core: CoreState<'_>, provider: String) -> Result<pixelit_ai::Options> {
    pixelit_ai::checkpoints::options(
        core.ai.net(),
        &core.ai.settings(),
        GenerationProvider::parse(&provider),
    )
}

/// Whether the generator of the studio answers, and whether it can be
/// started or stopped from here.
#[tauri::command(async)]
pub fn generator_status(core: CoreState<'_>, provider: String) -> pixelit_ai::apps::Service {
    let settings = core.ai.settings();
    let provider = match GenerationProvider::parse(&provider) {
        GenerationProvider::None => settings.generation.provider,
        chosen => chosen,
    };
    let (kind, url) = match provider {
        GenerationProvider::Comfy => (AppKind::Comfy, &settings.generation.comfy_url),
        GenerationProvider::Forge => (AppKind::Forge, &settings.generation.forge_url),
        GenerationProvider::None | GenerationProvider::SdCpp | GenerationProvider::Cloud => {
            return pixelit_ai::apps::Service::remote();
        }
    };
    core.ai
        .apps_host
        .service(core.ai.net(), &settings.apps, kind, url)
}

#[tauri::command(async)]
pub fn prompt_improve(core: CoreState<'_>, text: String, confirmed: bool) -> Result<String> {
    let settings = core.ai.settings();
    if !settings.assistant.is_set() {
        return Err(Error::new("providerNotSet"));
    }
    let remote = !pixelit_ai::net::is_local(&settings.assistant.url);
    confirm_cloud(&core, remote, confirmed)?;
    core.ai.improve_prompt(&text)
}

/// The generation settings stored in a PNG, when it has them.
#[tauri::command(async)]
pub fn picture_parameters(path: String) -> Result<Option<ParametersOut>> {
    let bytes = std::fs::read(&path)?;
    let Some(text) = pixelit_codecs::png_text(&bytes, "parameters") else {
        return Ok(None);
    };
    Ok(parameters::read(&text).map(ParametersOut::from))
}

// ------------------------------------------------------------------- history

#[tauri::command(async)]
pub fn generated_list(core: CoreState<'_>) -> Vec<history::Record> {
    let mut records = history::list(core.paths());
    let folder = core.paths().generated();
    for record in &mut records {
        for picture in &mut record.pictures {
            picture.file = display(&folder.join(&picture.file));
        }
    }
    records
}

#[tauri::command(async)]
pub fn generated_delete(core: CoreState<'_>, files: Vec<String>) -> Result<u32> {
    let removed = history::forget(core.paths(), &files)?;
    changed(&core);
    Ok(removed)
}

#[tauri::command(async)]
pub fn generated_favorite(core: CoreState<'_>, file: String, on: bool) -> Result<bool> {
    let on = history::favorite(core.paths(), &file, on)?;
    changed(&core);
    Ok(on)
}

#[tauri::command(async)]
pub fn generated_export(
    core: CoreState<'_>,
    files: Vec<String>,
    folder: String,
) -> Result<Vec<String>> {
    let written = history::export(core.paths(), &files, Path::new(&folder))?;
    Ok(written.iter().map(|path| display(path)).collect())
}

/// Enlarges one generated picture with the chosen upscaler and keeps the result.
#[tauri::command(async)]
pub fn generated_upscale(core: CoreState<'_>, file: String, factor: u32) -> Result<u64> {
    core.ensure_not_updating()?;
    let settings = core.ai.settings();
    if pixelit_ai::upscale::uses_builtin(&settings) && !core.ai.capabilities().upscale {
        return Err(Error::new("modelMissing").with("model", "upscale"));
    }
    let factor = if factor >= 4 { 4 } else { 2 };
    let title = file_name(Path::new(&file));
    let core = Arc::clone(core.inner());
    Ok(core.engine.jobs.spawn("upscale", &title, Lane::Gpu, {
        let core = Arc::clone(&core);
        move |job: &JobContext| {
            let image = core
                .engine
                .runner()
                .load(Path::new(&file), 0, &job.cancel)?
                .media
                .into_first()
                .ok_or_else(|| Error::new("emptyImage").with("path", &file))?;
            let notes = pixelit_ai::upscale::notes_of_file(core.paths(), Path::new(&file));
            let app = core.app().clone();
            let tick = |index: u32, count: u32, step: u32, steps: u32, preview: Option<&[u8]>| {
                emit_preview(&app, index, count, step, steps, preview);
            };
            let done = core
                .ai
                .enlarge(&image, factor, &notes, 0, 1, &job.cancel, &tick)?;
            let entry = keep_remade(
                &core,
                notes,
                &file,
                done.image,
                history::GeneratedKind::Upscale,
            )?;
            with_notice(&entry, done.notice)
        }
    }))
}

/// Paints detail into one generated picture at its own size and keeps the
/// result next to it.
#[tauri::command(async)]
pub fn generated_refine(core: CoreState<'_>, file: String, confirmed: bool) -> Result<u64> {
    core.ensure_not_updating()?;
    let settings = core.ai.settings();
    confirm_cloud(&core, generate::leaves_computer(&settings), confirmed)?;
    let title = file_name(Path::new(&file));
    let core = Arc::clone(core.inner());
    Ok(core.engine.jobs.spawn("refine", &title, Lane::Gpu, {
        let core = Arc::clone(&core);
        move |job: &JobContext| {
            let notes = pixelit_ai::upscale::notes_of_file(core.paths(), Path::new(&file));
            let image = core
                .engine
                .runner()
                .load(Path::new(&file), 0, &job.cancel)?
                .media
                .into_first()
                .ok_or_else(|| Error::new("emptyImage").with("path", &file))?;
            let app = core.app().clone();
            let tick = |index: u32, count: u32, step: u32, steps: u32, preview: Option<&[u8]>| {
                emit_preview(&app, index, count, step, steps, preview);
            };
            let painted = core.ai.refine(&image, &notes, &job.cancel, &tick)?;
            let entry = keep_remade(&core, notes, &file, painted, history::GeneratedKind::Refine)?;
            with_notice(&entry, "")
        }
    }))
}

fn emit_preview(
    app: &tauri::AppHandle,
    index: u32,
    count: u32,
    step: u32,
    steps: u32,
    preview: Option<&[u8]>,
) {
    let image = preview
        .filter(|bytes| bytes.len() <= 1_500_000)
        .map(|bytes| STANDARD.encode(bytes));
    let _ = app.emit(
        "generate:preview",
        json!({ "index": index, "count": count, "step": step, "steps": steps, "image": image }),
    );
}

/// Saves a picture made from the generated `source` and writes it into the
/// history with the request of that picture.
fn keep_remade(
    core: &Core,
    notes: pixelit_ai::upscale::Notes,
    source: &str,
    image: Image,
    kind: history::GeneratedKind,
) -> Result<history::Record> {
    let width = image.width();
    let height = image.height();
    let written = pixelit_ai::upscale::parameters_of(&notes, &image);
    let files = save_generated(core.paths(), &[image], Some(&[written]))?;
    let mut entry = history::record(
        core.paths(),
        history::Record {
            prompt: notes.prompt.clone(),
            negative: notes.negative,
            final_prompt: notes.prompt,
            provider: notes.provider,
            model: notes.model,
            steps: notes.steps,
            cfg: notes.cfg,
            source: source.to_owned(),
            kind,
            width,
            height,
            pictures: files
                .iter()
                .map(|path| history::Picture {
                    file: file_name(path),
                    seed: notes.seed,
                    width,
                    height,
                    ..history::Picture::default()
                })
                .collect(),
            tuning: notes.tuning,
            ..history::Record::default()
        },
    )?;
    let folder = core.paths().generated();
    for picture in &mut entry.pictures {
        picture.file = display(&folder.join(&picture.file));
    }
    changed(core);
    Ok(entry)
}

/// The upscalers the studio can offer right now.
#[tauri::command(async)]
pub fn upscalers_list(core: CoreState<'_>) -> Vec<pixelit_ai::upscale::Upscaler> {
    pixelit_ai::upscale::list(core.ai.net(), core.paths(), &core.ai.settings())
}

fn with_notice(entry: &history::Record, notice: &str) -> Result<serde_json::Value> {
    let mut value =
        serde_json::to_value(entry).map_err(|error| Error::new("internal").detail(error))?;
    if !notice.is_empty() {
        value["notice"] = json!(notice);
    }
    Ok(value)
}

// --------------------------------------------------------------------- styles

#[tauri::command(async)]
pub fn styles_list(core: CoreState<'_>) -> Vec<Style> {
    styles::list(core.paths())
}

#[tauri::command(async)]
pub fn style_save(core: CoreState<'_>, style: Style) -> Result<Style> {
    styles::save(core.paths(), style)
}

#[tauri::command(async)]
pub fn style_remove(core: CoreState<'_>, id: String) -> Result<bool> {
    styles::remove(core.paths(), &id)
}

#[tauri::command(async)]
pub fn styles_import(core: CoreState<'_>, path: String) -> Result<Vec<Style>> {
    styles::import_file(core.paths(), Path::new(&path))
}

#[tauri::command(async)]
pub fn styles_export(core: CoreState<'_>, path: String, ids: Vec<String>) -> Result<()> {
    styles::export_file(core.paths(), &ids, Path::new(&path))
}

// The wire form of a `parameters` block. The core type stays free of serde.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParametersOut {
    prompt: String,
    negative: String,
    steps: u32,
    cfg: f32,
    seed: u64,
    width: u32,
    height: u32,
    model: String,
}

impl From<Parameters> for ParametersOut {
    fn from(value: Parameters) -> Self {
        Self {
            prompt: value.prompt,
            negative: value.negative,
            steps: value.steps,
            cfg: value.cfg,
            seed: value.seed,
            width: value.width,
            height: value.height,
            model: value.model,
        }
    }
}
