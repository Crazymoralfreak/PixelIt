//! Commands of the AI part: components and models, keys, generation and the
//! vision model.
//!
//! Nothing here goes to a server outside this computer unless the user has
//! switched the cloud on, and with "ask before upload" set, every such
//! request waits for the answer to a question the interface shows.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Emitter;

use pixelit_ai::generate::save_generated;
use pixelit_ai::{
    llm, secrets, vision, Advice, AiSettings, Capabilities, Found, GenerateRequest, Hardware,
    Source, Status, VisionTask,
};
use pixelit_core::pipeline::Step;
use pixelit_core::{Error, Op, Result};
use pixelit_engine::{thumbs, JobContext, Lane};

use super::{display, CoreState};
use crate::app::Core;

/// Longest side of a picture sent to the vision model.
const VISION_SIDE: u32 = 2048;

/// Refuses a request that leaves this computer until the user has agreed.
/// The interface shows the question and repeats the call with `confirmed`.
fn confirm_cloud(core: &Core, leaves_computer: bool, confirmed: bool) -> Result<()> {
    if leaves_computer && core.ai.settings().confirm_cloud && !confirmed {
        Err(Error::new("cloudConfirm"))
    } else {
        Ok(())
    }
}

/// The same check for editor steps: only a repaint by prompt uses a provider.
pub fn confirm_steps(core: &Core, steps: &[Step], confirmed: bool) -> Result<()> {
    let prompted = steps.iter().any(|step| {
        step.enabled
            && matches!(&step.op, Op::Inpaint(inpaint) if !inpaint.prompt.trim().is_empty())
    });
    confirm_cloud(
        core,
        prompted && core.ai.generation_leaves_computer(),
        confirmed,
    )
}

// State and settings

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    pub capabilities: Capabilities,
    pub hardware: Hardware,
    pub advice: Advice,
    pub settings: AiSettings,
    pub components: Vec<Status>,
    /// The chosen generation provider is outside this computer.
    pub generation_remote: bool,
    /// The vision model is outside this computer.
    pub vision_remote: bool,
}

#[tauri::command(async)]
pub fn ai_status(core: CoreState<'_>) -> AiStatus {
    let settings = core.ai.settings();
    AiStatus {
        capabilities: core.ai.capabilities(),
        hardware: core.ai.hardware().clone(),
        advice: core.ai.hardware().advice(),
        components: core.ai.components(),
        generation_remote: core.ai.generation_leaves_computer(),
        vision_remote: vision::leaves_computer(&settings),
        settings,
    }
}

#[tauri::command(async)]
pub fn ai_settings_set(core: CoreState<'_>, settings: AiSettings) -> Result<AiSettings> {
    let saved = core.ai.set_settings(settings)?;
    let _ = core.app().emit("ai:changed", ());
    Ok(saved)
}

// Components and models

/// Downloads a component or a model as a job with progress in bytes.
#[tauri::command(async)]
pub fn component_install(core: CoreState<'_>, id: String) -> Result<u64> {
    core.ensure_not_updating()?;
    let core = Arc::clone(core.inner());
    let jobs = &core.engine.jobs;
    Ok(jobs.spawn("install", &id, Lane::Io, {
        let core = Arc::clone(&core);
        let id = id.clone();
        move |job: &JobContext| {
            let mut progress = |done: u64, total: u64| job.progress(done, total, &id);
            core.ai.install(&id, &mut progress, &job.cancel)?;
            core.sync_pdfium();
            let _ = core.app().emit("ai:changed", ());
            Ok(json!({ "installed": id }))
        }
    }))
}

#[tauri::command(async)]
pub fn component_remove(core: CoreState<'_>, id: String) -> Result<()> {
    core.ai.remove(&id)?;
    core.sync_pdfium();
    let _ = core.app().emit("ai:changed", ());
    Ok(())
}

// Keys, kept in the Windows Credential Manager

#[tauri::command(async)]
pub fn ai_key_set(host: String, key: String) -> Result<()> {
    secrets::set(&host, &key)
}

#[tauri::command(async)]
pub fn ai_key_has(host: String) -> bool {
    secrets::has(&host)
}

#[tauri::command(async)]
pub fn ai_key_delete(host: String) {
    secrets::delete(&host);
}

// Servers

/// Looks for model servers running on this computer.
#[tauri::command(async)]
pub fn ai_discover(core: CoreState<'_>) -> Vec<Found> {
    core.ai.discover()
}

/// Models an OpenAI-compatible server offers.
#[tauri::command(async)]
pub fn ai_models(core: CoreState<'_>, url: String) -> Result<Vec<String>> {
    llm::models(core.ai.net(), &core.ai.settings(), &url)
}

// Generation

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateStart {
    pub request: GenerateRequest,
    /// Picture to start from, for image-to-image.
    pub source: Option<String>,
    /// The user agreed to send the picture and the prompt to a cloud server.
    #[serde(default)]
    pub confirmed: bool,
}

/// Generates pictures as a job. The paths of the saved files arrive as the
/// result of the job.
#[tauri::command(async)]
pub fn generate_start(core: CoreState<'_>, start: GenerateStart) -> Result<u64> {
    core.ensure_not_updating()?;
    if start.request.prompt.trim().is_empty() {
        return Err(Error::new("emptyPrompt"));
    }
    confirm_cloud(&core, core.ai.generation_leaves_computer(), start.confirmed)?;
    let title: String = start.request.prompt.chars().take(60).collect();
    let core = Arc::clone(core.inner());
    let jobs = &core.engine.jobs;
    Ok(jobs.spawn("generate", &title, Lane::Gpu, {
        let core = Arc::clone(&core);
        move |job: &JobContext| {
            let source = match &start.source {
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
            let source = source.as_ref().map(|image| Source { image, mask: None });
            let images = core
                .ai
                .generate(&start.request, source.as_ref(), &job.cancel)?;
            let files: Vec<String> = save_generated(core.paths(), &images)?
                .iter()
                .map(|path| display(path))
                .collect();
            Ok(json!({ "files": files }))
        }
    }))
}

// Vision

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionAnswer {
    /// The file, under its new name after a rename.
    pub path: String,
    pub text: String,
    pub applied: bool,
}

/// Asks the vision model about one file and, with `apply`, writes the answer
/// into the metadata or renames the file.
fn describe(
    core: &Core,
    path: &Path,
    task: VisionTask,
    language: &str,
    apply: bool,
    job: &JobContext,
) -> Result<VisionAnswer> {
    let preview = thumbs::load_preview(&core.engine.runner(), path, VISION_SIDE, 0, &job.cancel)?;
    let text = core.ai.vision(&preview.image, task, language)?;
    let mut answer = VisionAnswer {
        path: display(path),
        text,
        applied: false,
    };
    if apply {
        match task {
            VisionTask::Describe | VisionTask::AltText => {
                vision::write_caption(path, &answer.text, &[])?;
                answer.applied = true;
            }
            VisionTask::Tags => {
                vision::write_caption(path, "", &vision::tags(&answer.text))?;
                answer.applied = true;
            }
            VisionTask::Name => {
                answer.path = display(&vision::rename(path, &answer.text)?);
                answer.applied = true;
            }
            VisionTask::Ocr => {}
        }
    }
    Ok(answer)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionStart {
    pub files: Vec<String>,
    pub task: VisionTask,
    /// Write descriptions and tags into the files, or rename them.
    pub apply: bool,
    #[serde(default)]
    pub confirmed: bool,
}

/// Runs the vision model over files as a job. The result of the job lists
/// an answer or an error for every file.
#[tauri::command(async)]
pub fn vision_start(core: CoreState<'_>, start: VisionStart) -> Result<u64> {
    core.ensure_not_updating()?;
    if start.files.is_empty() {
        return Err(Error::new("noImages"));
    }
    let settings = core.ai.settings();
    if !settings.vision.is_set() {
        return Err(Error::new("providerNotSet"));
    }
    confirm_cloud(&core, vision::leaves_computer(&settings), start.confirmed)?;
    let language = core.lang();
    let core = Arc::clone(core.inner());
    let jobs = &core.engine.jobs;
    Ok(jobs.spawn("vision", "", Lane::Io, {
        let core = Arc::clone(&core);
        move |job: &JobContext| {
            let total = start.files.len() as u64;
            let mut answers: Vec<Value> = Vec::with_capacity(start.files.len());
            for (index, file) in start.files.iter().enumerate() {
                job.checkpoint()?;
                let path = PathBuf::from(file);
                let name = path
                    .file_name()
                    .map_or_default(|name| name.to_string_lossy().into_owned());
                job.progress(index as u64, total, &name);
                answers.push(
                    match describe(&core, &path, start.task, &language, start.apply, job) {
                        Ok(answer) => json!({ "source": file, "answer": answer }),
                        Err(error) if error.is_cancelled() => return Err(error),
                        Err(error) => json!({ "source": file, "error": error }),
                    },
                );
            }
            job.progress(total, total, "");
            Ok(json!({ "answers": answers }))
        }
    }))
}
