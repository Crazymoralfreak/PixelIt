//! PixelIt AI: downloadable components and models, local models through ONNX
//! Runtime, image generation providers and vision models.
//!
//! Everything optional lives here. Without a single component installed the
//! application works as an ordinary image tool; each AI step reports what is
//! missing instead of failing in an unclear way.

pub mod apps;
pub mod browse;
pub mod catalog;
pub mod checkpoints;
pub mod config;
pub mod discover;
mod edits;
pub mod faces;
pub mod generate;
pub mod hardware;
pub mod history;
pub mod install;
pub mod llm;
pub mod net;
pub mod onnx;
pub mod outpaint;
pub mod parameters;
pub mod progress;
pub mod secrets;
pub mod styles;
pub mod upscale;
pub mod vision;
pub mod web;
pub mod worker;

use std::path::PathBuf;
use std::sync::{PoisonError, RwLock};

use serde::Serialize;

use pixelit_core::pipeline::{AiBackend, AiDetail, AiEdit, Mask, Op, ResizeFilter, Step};
use pixelit_core::{ops, CancelToken, Error, Image, Result};
use pixelit_engine::Paths;

use crate::edits::{Edits, Print, Redraw};

pub use crate::apps::Host;
pub use crate::browse::{ModelHit, SearchSource};
pub use crate::catalog::{Entry, Kind};
pub use crate::checkpoints::{GenerationModel, ModelList, Options};
pub use crate::config::{AiSettings, DeviceChoice, Endpoint, Generation, GenerationProvider};
pub use crate::config::{AppKind, AppOrigin, LocalApps, ModelKind, ModelLibrary};
pub use crate::config::{HistoryView, Lora, Tuning};
pub use crate::discover::Found;
pub use crate::faces::Face;
pub use crate::generate::{GenerateRequest, Outcome, Source};
pub use crate::hardware::{Advice, Hardware};
pub use crate::history::{GeneratedKind, Picture, Record};
pub use crate::install::Status;
pub use crate::llm::{Message, Reply, Role, ToolCall, ToolSpec};
pub use crate::net::Net;
pub use crate::onnx::{Device, Onnx};
pub use crate::outpaint::Expand;
pub use crate::styles::Style;
pub use crate::vision::VisionTask;
pub use crate::web::{WebImage, WebSource};

/// What the AI part can do right now, for the interface and the tool list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub runtime: bool,
    pub background: bool,
    pub upscale: bool,
    pub inpaint: bool,
    pub faces: bool,
    pub generation: bool,
    pub vision: bool,
    pub assistant: bool,
    pub pdf: bool,
}

pub struct Ai {
    paths: Paths,
    net: Net,
    hardware: Hardware,
    settings: RwLock<AiSettings>,
    onnx: Onnx,
    edits: Edits,
    /// ComfyUI and Forge processes this section started.
    pub apps_host: apps::Host,
}

fn remember_comfy_url(settings: &mut config::AiSettings) {
    let app = &settings.apps.comfy;
    if app.home.is_empty() || app.origin == config::AppOrigin::Missing {
        return;
    }
    settings.generation.comfy_url = format!("http://127.0.0.1:{}", app.port);
}

impl Ai {
    /// Loads the settings and looks at the hardware. Starts nothing and
    /// opens no connection.
    pub fn open(paths: Paths) -> Self {
        let settings = AiSettings::load(&paths);
        let hardware = hardware::detect();
        let ai = Self {
            onnx: Onnx::new(paths.clone()),
            edits: Edits::default(),
            net: Net::new(),
            settings: RwLock::new(settings),
            hardware,
            paths,
            apps_host: apps::Host::default(),
        };
        ai.apply_device();
        ai
    }

    fn apply_device(&self) {
        let settings = self.settings();
        let (gpu, gpu_index) = self.hardware.device(&settings);
        self.onnx.set_device(Device {
            gpu,
            gpu_index,
            threads: settings.threads,
        });
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub fn net(&self) -> &Net {
        &self.net
    }

    pub fn hardware(&self) -> &Hardware {
        &self.hardware
    }

    pub fn onnx(&self) -> &Onnx {
        &self.onnx
    }

    pub fn settings(&self) -> AiSettings {
        self.settings
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Stores new settings and returns them as saved.
    pub fn set_settings(&self, mut settings: AiSettings) -> Result<AiSettings> {
        settings.normalize();
        settings.save(&self.paths)?;
        *self
            .settings
            .write()
            .unwrap_or_else(PoisonError::into_inner) = settings.clone();
        self.apply_device();
        Ok(settings)
    }

    /// Every catalogue entry with its state.
    pub fn components(&self) -> Vec<Status> {
        install::list(&self.paths)
    }

    /// Downloads and installs a catalogue entry with what it needs.
    pub fn install(
        &self,
        id: &str,
        progress: &mut dyn FnMut(u64, u64),
        cancel: &CancelToken,
    ) -> Result<()> {
        install::install(&self.net, &self.paths, id, progress, cancel)
    }

    /// Removes a catalogue entry; the worker is stopped first so no file is
    /// in use.
    pub fn remove(&self, id: &str) -> Result<()> {
        self.onnx.stop();
        install::remove(&self.paths, id)
    }

    fn save_apps(&self, apps: config::LocalApps) -> Result<AiSettings> {
        let mut settings = self.settings();
        settings.apps = apps;
        self.set_settings(settings)
    }

    /// Points a card at a folder the user chose and reads the models already there.
    pub fn set_app_home(&self, kind: config::AppKind, home: String) -> Result<AiSettings> {
        let mut settings = self.settings();
        {
            let app = settings.apps.of_mut(kind);
            home.trim().clone_into(&mut app.home);
            app.origin = if app.home.is_empty() {
                config::AppOrigin::Missing
            } else {
                config::AppOrigin::Folder
            };
            app.release.clear();
            app.bytes = 0;
        }
        apps::link_folder(&mut settings.apps);
        remember_comfy_url(&mut settings);
        self.set_settings(settings)
    }

    /// Uses the Stability Matrix package for this program, when one was found.
    pub fn use_stability(&self, kind: config::AppKind) -> Result<AiSettings> {
        let offer = apps::detect_stability().ok_or_else(|| Error::new("notFound"))?;
        let home = match kind {
            config::AppKind::Comfy => offer.comfy,
            config::AppKind::Forge => offer.forge,
        };
        if home.is_empty() {
            return Err(Error::new("notFound"));
        }
        let mut settings = self.settings();
        {
            let app = settings.apps.of_mut(kind);
            app.home = home;
            app.origin = config::AppOrigin::StabilityMatrix;
            app.release.clear();
            app.bytes = 0;
        }
        apps::link_folder(&mut settings.apps);
        remember_comfy_url(&mut settings);
        self.set_settings(settings)
    }

    /// Reads model folders and the last port from a folder that was already chosen.
    pub fn link_saved_apps(&self) -> bool {
        let mut settings = self.settings();
        if !apps::link_folder(&mut settings.apps) {
            return false;
        }
        remember_comfy_url(&mut settings);
        self.set_settings(settings).is_ok()
    }

    /// Copies the model folders Stability Matrix already uses.
    pub fn fill_model_folders(&self) -> Result<AiSettings> {
        let offer = apps::detect_stability().ok_or_else(|| Error::new("notFound"))?;
        let mut apps = self.settings().apps;
        apps.models = offer.models;
        self.save_apps(apps)
    }

    /// Frees the memory of the loaded models.
    pub fn release(&self) {
        self.apps_host.stop_all();
        self.onnx.stop();
    }

    /// Folder of the installed PDFium, for the PDF decoder.
    pub fn pdfium_folder(&self) -> Option<PathBuf> {
        install::main_path(&self.paths, catalog::PDFIUM)
            .ok()
            .and_then(|library| library.parent().map(PathBuf::from))
    }

    pub fn capabilities(&self) -> Capabilities {
        let settings = self.settings();
        let runtime = install::is_installed(&self.paths, catalog::RUNTIME);
        let has = |kind: Kind| runtime && !install::installed_of(&self.paths, kind).is_empty();
        let generation = match settings.generation.provider {
            GenerationProvider::None => false,
            GenerationProvider::Comfy | GenerationProvider::Forge => true,
            GenerationProvider::SdCpp => {
                !install::installed_of(&self.paths, Kind::Generator).is_empty()
                    && !settings.generation.sdcpp_model.is_empty()
            }
            GenerationProvider::Cloud => settings.allow_cloud && settings.generation.cloud.is_set(),
        };
        Capabilities {
            runtime,
            background: has(Kind::Background),
            upscale: has(Kind::Upscale),
            inpaint: has(Kind::Inpaint),
            faces: has(Kind::Faces) && has(Kind::Detector),
            generation,
            vision: settings.vision.is_set(),
            assistant: settings.assistant.is_set(),
            pdf: install::is_installed(&self.paths, catalog::PDFIUM),
        }
    }

    /// The model a task uses: the requested one, the default of the settings
    /// or the first installed.
    pub fn model_for(&self, kind: Kind, requested: &str) -> Result<&'static Entry> {
        let settings = self.settings();
        let default = match kind {
            Kind::Background => settings.models.background.as_str(),
            Kind::Upscale => settings.models.upscale.as_str(),
            Kind::Inpaint => settings.models.inpaint.as_str(),
            Kind::Faces => settings.models.faces.as_str(),
            Kind::Runtime | Kind::Pdf | Kind::Generator | Kind::Detector => "",
        };
        let installed = install::installed_of(&self.paths, kind);
        let requested = requested.trim();
        if !requested.is_empty() {
            return installed
                .iter()
                .find(|entry| entry.id == requested)
                .copied()
                .ok_or_else(|| Error::new("modelMissing").with("model", requested));
        }
        installed
            .iter()
            .find(|entry| entry.id == default)
            .or(installed.first())
            .copied()
            .ok_or_else(|| {
                Error::new("modelMissing").with("model", format!("{kind:?}").to_lowercase())
            })
    }

    /// Generates pictures with the provider of the settings.
    pub fn generate(
        &self,
        request: &GenerateRequest,
        source: Option<&Source<'_>>,
        cancel: &CancelToken,
        tick: &generate::Tick<'_>,
    ) -> Result<Outcome> {
        generate::generate(
            &self.net,
            &self.paths,
            &self.settings(),
            request,
            source,
            cancel,
            tick,
        )
    }

    /// Rewrites a prompt into a detailed English prompt with the assistant model.
    pub fn improve_prompt(&self, text: &str) -> Result<String> {
        let settings = self.settings();
        if !settings.assistant.is_set() {
            return Err(Error::new("providerNotSet"));
        }
        let reply = llm::chat(
            &self.net,
            &settings,
            &settings.assistant,
            &[
                Message::new(
                    Role::System,
                    "Rewrite the user's image prompt into one detailed English prompt. Keep the subject. Return only the prompt, with no quotes and no explanation.",
                ),
                Message::new(Role::User, text.trim()),
            ],
            &[],
        )?;
        let text = reply.content.trim();
        if text.is_empty() {
            return Err(Error::new("generationFailed").detail("empty reply"));
        }
        Ok(text.to_owned())
    }

    /// Whether any enabled step would ask a generator that is outside this computer.
    pub fn steps_leave_computer(&self, steps: &[Step]) -> bool {
        let settings = self.settings();
        // A step names its own generator or leaves it to the settings.
        let leaves = |provider: &str| {
            let provider = match GenerationProvider::parse(provider) {
                GenerationProvider::None => settings.generation.provider,
                other => other,
            };
            generate::leaves_with(&settings, provider)
        };
        steps
            .iter()
            .filter(|step| step.enabled)
            .any(|step| match &step.op {
                Op::AiEdit(spec) if !spec.prompt.trim().is_empty() => leaves(&spec.provider),
                Op::AiDetail(spec) => leaves(&spec.provider),
                Op::Inpaint(spec) if !spec.prompt.trim().is_empty() => {
                    generate::leaves_computer(&settings)
                }
                Op::AiEdit(_)
                | Op::Inpaint(_)
                | Op::Resize(_)
                | Op::Crop(_)
                | Op::Rotate(_)
                | Op::Flip(_)
                | Op::Adjust(_)
                | Op::Filter(_)
                | Op::Watermark(_)
                | Op::Annotate(_)
                | Op::RemoveBackground(_)
                | Op::Upscale(_)
                | Op::RestoreFaces(_) => false,
            })
    }

    /// Asks the vision model about a picture.
    pub fn vision(&self, image: &Image, task: VisionTask, language: &str) -> Result<String> {
        vision::ask(&self.net, &self.settings(), image, task, language)
    }

    /// Sends a conversation to the assistant model.
    pub fn chat(&self, messages: &[Message], tools: &[ToolSpec]) -> Result<Reply> {
        let settings = self.settings();
        llm::chat(&self.net, &settings, &settings.assistant, messages, tools)
    }

    /// Looks for AI servers on this computer.
    pub fn discover(&self) -> Vec<Found> {
        discover::local_servers(&self.net, &self.settings())
    }

    /// Enlarges with the upscaler from the settings, and paints detail when
    /// that is switched on and a local generator can do it.
    pub fn enlarge(
        &self,
        image: &Image,
        factor: u32,
        notes: &upscale::Notes,
        index: u32,
        count: u32,
        cancel: &CancelToken,
        tick: &generate::Tick<'_>,
    ) -> Result<upscale::Enlarged> {
        let settings = self.settings();
        upscale::enlarge(
            &self.onnx,
            &self.net,
            &self.paths,
            &settings,
            image,
            factor,
            notes,
            index,
            count,
            cancel,
            tick,
        )
    }

    /// Paints detail into a picture at its own size with the generator of the
    /// settings.
    pub fn refine(
        &self,
        image: &Image,
        notes: &upscale::Notes,
        cancel: &CancelToken,
        tick: &generate::Tick<'_>,
    ) -> Result<Image> {
        upscale::refine(
            &self.net,
            &self.paths,
            &self.settings(),
            image,
            notes,
            cancel,
            tick,
        )
    }

    /// Finds the faces of a picture.
    pub fn detect_faces(&self, image: &Image, cancel: &CancelToken) -> Result<Vec<Face>> {
        if !install::is_installed(&self.paths, catalog::DETECTOR) {
            return Err(Error::new("modelMissing").with("model", catalog::DETECTOR));
        }
        self.onnx.detect_faces(image, &[], cancel)
    }

    /// Whether repainting with a prompt would send the picture off this computer.
    pub fn generation_leaves_computer(&self) -> bool {
        generate::leaves_computer(&self.settings())
    }

    /// Whether this picture and this step are already in the editor cache,
    /// so applying the step again does not call the generator.
    fn is_cached(&self, image: &Image, step: Redraw<'_>) -> bool {
        Print::of(image, step)
            .ok()
            .and_then(|print| self.edits.find(&print))
            .is_some_and(|generated| serves(step, &generated, image))
    }

    /// Whether every enabled step that would ask a generator already has its
    /// picture cached. `inputs` are the pictures those steps would receive,
    /// in the same order. A prompted inpaint is never cached.
    pub fn generator_steps_cached(&self, inputs: &[&Image], steps: &[Step]) -> bool {
        let needing: Vec<&Step> = steps
            .iter()
            .filter(|step| step.enabled && step.op.uses_generator())
            .collect();
        if needing.len() != inputs.len() {
            return false;
        }
        needing
            .iter()
            .zip(inputs)
            .all(|(step, image)| match &step.op {
                Op::AiEdit(spec) => self.is_cached(image, Redraw::Edit(spec)),
                Op::AiDetail(spec) => self.is_cached(image, Redraw::Detail(spec)),
                Op::Inpaint(_)
                | Op::Resize(_)
                | Op::Crop(_)
                | Op::Rotate(_)
                | Op::Flip(_)
                | Op::Adjust(_)
                | Op::Filter(_)
                | Op::Watermark(_)
                | Op::Annotate(_)
                | Op::RemoveBackground(_)
                | Op::Upscale(_)
                | Op::RestoreFaces(_) => false,
            })
    }

    /// Asks the generator to change a picture: inside the mask, or all of
    /// it without one.
    fn redraw(
        &self,
        image: &Image,
        mask: Option<&Mask>,
        request: &GenerateRequest,
        cancel: &CancelToken,
    ) -> Result<Image> {
        let source = Source { image, mask };
        self.generate(request, Some(&source), cancel, &generate::quiet)?
            .images
            .into_iter()
            .next()
            .ok_or_else(|| Error::new("generationFailed"))
    }

    /// Repaints the masked area following a prompt, keeping everything
    /// outside the mask untouched.
    fn repaint(
        &self,
        image: &Image,
        mask: &Mask,
        prompt: &str,
        cancel: &CancelToken,
    ) -> Result<Image> {
        let request = GenerateRequest {
            prompt: prompt.to_owned(),
            strength: 0.85,
            ..GenerateRequest::default()
        };
        let generated = self.redraw(image, Some(mask), &request, cancel)?;
        place(image, &generated, Some(mask))
    }

    /// Runs an "AI edit" or an "AI detail" step: inside the mask, or over
    /// the whole picture without one.
    fn redrawn(
        &self,
        image: &Image,
        mask: Option<&Mask>,
        step: Redraw<'_>,
        preview: bool,
        cancel: &CancelToken,
    ) -> Result<Image> {
        // The real run repeats what the editor has shown: a generator gives
        // another picture every time it is asked.
        let print = Print::of(image, step)?;
        if let Some(generated) = self
            .edits
            .find(&print)
            .filter(|generated| serves(step, generated, image))
        {
            return place(image, &generated, mask);
        }
        let side = image.width().max(image.height());
        if matches!(step, Redraw::Detail(_)) && side > upscale::REFINE_SIDE {
            return Err(Error::new("refineTooLarge")
                .with("side", side)
                .with("limit", upscale::REFINE_SIDE));
        }
        let generated = self.redraw(image, mask, &request_of(step), cancel)?;
        let result = place(image, &generated, mask)?;
        if preview {
            self.edits.remember(print, generated);
        }
        Ok(result)
    }
}

/// What a detail pass sends when the step names nothing. A generator reads a
/// prompt as a description of the picture, so these words ask for detail
/// rather than giving an order. The editor shows the same words.
const DETAIL_PROMPT: &str = "highly detailed, sharp focus, intricate details";

/// What a step asks the generator for. What an edit leaves empty is taken
/// from the settings. An empty detail prompt takes `DETAIL_PROMPT`.
fn request_of(step: Redraw<'_>) -> GenerateRequest {
    let strength = |strength: f32| (strength / 100.0).clamp(0.05, 1.0);
    match step {
        Redraw::Edit(spec) => GenerateRequest {
            prompt: spec.prompt.trim().to_owned(),
            strength: strength(spec.strength),
            seed: Some(u64::from(spec.variant)),
            provider: GenerationProvider::parse(&spec.provider),
            model: spec.model.clone(),
            ..GenerateRequest::default()
        },
        Redraw::Detail(spec) => GenerateRequest {
            prompt: {
                let prompt = spec.prompt.trim();
                if prompt.is_empty() {
                    DETAIL_PROMPT
                } else {
                    prompt
                }
                .to_owned()
            },
            negative: spec.negative.trim().to_owned(),
            steps: spec.steps,
            cfg: spec.cfg,
            tuning: spec.tuning.clone(),
            strength: strength(spec.strength),
            seed: Some(u64::from(spec.variant)),
            provider: GenerationProvider::parse(&spec.provider),
            model: spec.model.clone(),
            ..GenerateRequest::default()
        },
    }
}

/// Whether a picture the generator drew for the editor may stand for the
/// same step on `image`. A changed picture is stretched over the full file,
/// so the file shows what the editor has shown. A detail pass is worth only
/// the size it was painted at: one painted for a clearly smaller copy is
/// painted again.
fn serves(step: Redraw<'_>, generated: &Image, image: &Image) -> bool {
    match step {
        Redraw::Edit(_) => true,
        Redraw::Detail(_) => {
            let drawn = u64::from(generated.width().max(generated.height()));
            let wanted = u64::from(image.width().max(image.height()));
            drawn * DETAIL_SIZE.1 >= wanted * DETAIL_SIZE.0
        }
    }
}

/// The share of the long side of a file, as a fraction, that a detail pass
/// must have been painted at to be used for it. A generator rounds a size
/// down to what it can paint, so the sizes are rarely equal.
const DETAIL_SIZE: (u64, u64) = (9, 10);

/// Puts a generated picture over the original: inside the mask, or over all
/// of it without one. The size, the transparency and the depth stay those of
/// the original.
pub fn place(image: &Image, generated: &Image, mask: Option<&Mask>) -> Result<Image> {
    let generated = ops::resize::resize_to(
        generated,
        image.width(),
        image.height(),
        ResizeFilter::Lanczos3,
        false,
        None,
    )?
    .to_rgba8();
    let mut rgba = image.to_rgba8();
    match mask {
        Some(mask) => {
            for ((pixel, new), cover) in rgba
                .chunks_exact_mut(4)
                .zip(generated.chunks_exact(4))
                .zip(&mask.data)
            {
                let cover = u32::from(*cover);
                for channel in 0..3 {
                    pixel[channel] = ((u32::from(pixel[channel]) * (255 - cover)
                        + u32::from(new[channel]) * cover
                        + 127)
                        / 255) as u8;
                }
            }
        }
        None => {
            for (pixel, new) in rgba.chunks_exact_mut(4).zip(generated.chunks_exact(4)) {
                pixel[..3].copy_from_slice(&new[..3]);
            }
        }
    }
    let mut result = Image::from_rgba8(image.width(), image.height(), rgba)?;
    result.set_has_alpha(image.has_alpha());
    Ok(result.into_depth(image.depth()))
}

impl AiBackend for Ai {
    fn remove_background(&self, image: &Image, model: &str, cancel: &CancelToken) -> Result<Image> {
        let entry = self.model_for(Kind::Background, model)?;
        self.onnx.remove_background(image, entry, cancel)
    }

    fn upscale(
        &self,
        image: &Image,
        model: &str,
        factor: u32,
        cancel: &CancelToken,
    ) -> Result<Image> {
        let entry = self.model_for(Kind::Upscale, model)?;
        self.onnx.upscale(image, entry, factor.clamp(2, 4), cancel)
    }

    fn restore_faces(
        &self,
        image: &Image,
        model: &str,
        strength: f32,
        cancel: &CancelToken,
    ) -> Result<Image> {
        let entry = self.model_for(Kind::Faces, model)?;
        if !install::is_installed(&self.paths, catalog::DETECTOR) {
            return Err(Error::new("modelMissing").with("model", catalog::DETECTOR));
        }
        self.onnx.restore_faces(image, entry, strength, cancel)
    }

    fn inpaint(
        &self,
        image: &Image,
        mask: &Mask,
        model: &str,
        prompt: &str,
        cancel: &CancelToken,
    ) -> Result<Image> {
        if prompt.trim().is_empty() {
            let entry = self.model_for(Kind::Inpaint, model)?;
            self.onnx.inpaint(image, mask, entry, cancel)
        } else {
            self.repaint(image, mask, prompt.trim(), cancel)
        }
    }

    fn edit(
        &self,
        image: &Image,
        mask: Option<&Mask>,
        spec: &AiEdit,
        preview: bool,
        cancel: &CancelToken,
    ) -> Result<Image> {
        self.redrawn(image, mask, Redraw::Edit(spec), preview, cancel)
    }

    fn detail(
        &self,
        image: &Image,
        mask: Option<&Mask>,
        spec: &AiDetail,
        preview: bool,
        cancel: &CancelToken,
    ) -> Result<Image> {
        self.redrawn(image, mask, Redraw::Detail(spec), preview, cancel)
    }
}

#[cfg(test)]
mod tests {
    use pixelit_core::{Color, Depth};

    use super::*;

    fn ai() -> (tempfile::TempDir, Ai) {
        let folder = tempfile::tempdir().unwrap();
        let ai = Ai::open(Paths::at(folder.path()));
        (folder, ai)
    }

    #[test]
    fn nothing_is_available_at_first() {
        let (_folder, ai) = ai();
        assert_eq!(ai.capabilities(), Capabilities::default());
        assert!(ai.components().iter().all(|status| !status.installed));
        assert!(ai.pdfium_folder().is_none());
        assert!(!ai.generation_leaves_computer());
    }

    #[test]
    fn steps_report_the_missing_model() {
        let (_folder, ai) = ai();
        let image = Image::new(16, 16, Depth::Eight, Color::WHITE).unwrap();
        let cancel = CancelToken::new();
        let error = ai.remove_background(&image, "", &cancel).unwrap_err();
        assert_eq!(error.code, "modelMissing");
        assert_eq!(error.params["model"], "background");
        let named = ai
            .upscale(&image, "realesrgan-x4plus", 2, &cancel)
            .unwrap_err();
        assert_eq!(named.params["model"], "realesrgan-x4plus");
        let mask = Mask {
            width: 16,
            height: 16,
            data: vec![255; 256],
        };
        let repaint = ai.inpaint(&image, &mask, "", "a cat", &cancel).unwrap_err();
        assert_eq!(repaint.code, "providerNotSet");
        let spec = AiEdit {
            prompt: String::from("make it night"),
            ..AiEdit::default()
        };
        let edit = ai.edit(&image, None, &spec, true, &cancel).unwrap_err();
        assert_eq!(edit.code, "providerNotSet");
        let pass = AiDetail {
            prompt: String::from("a night street"),
            ..AiDetail::default()
        };
        let detail = ai.detail(&image, None, &pass, true, &cancel).unwrap_err();
        assert_eq!(detail.code, "providerNotSet");
    }

    #[test]
    fn a_detail_pass_carries_its_settings_and_refuses_a_huge_picture() {
        let mut pass = AiDetail {
            prompt: String::from(" a night street "),
            negative: String::from(" blur "),
            steps: 30,
            cfg: 5.5,
            variant: 3,
            provider: String::from("forge"),
            ..AiDetail::default()
        };
        pass.tuning.sampler = String::from("dpmpp_2m");
        let request = request_of(Redraw::Detail(&pass));
        assert_eq!(request.prompt, "a night street");
        assert_eq!(request.negative, "blur");
        assert_eq!((request.steps, request.cfg), (30, 5.5));
        assert_eq!(request.tuning.sampler, "dpmpp_2m");
        assert_eq!(request.seed, Some(3));
        assert_eq!(request.provider, GenerationProvider::Forge);
        assert!((request.strength - 0.35).abs() < 1e-6);

        // Nothing written still asks for detail, in the words the editor shows.
        let bare = AiDetail::default();
        assert_eq!(request_of(Redraw::Detail(&bare)).prompt, DETAIL_PROMPT);
        let spaces = AiDetail {
            prompt: String::from("   "),
            ..AiDetail::default()
        };
        assert_eq!(request_of(Redraw::Detail(&spaces)).prompt, DETAIL_PROMPT);

        // An edit leaves all of that to the settings.
        let edit = AiEdit {
            prompt: String::from("make it night"),
            ..AiEdit::default()
        };
        let request = request_of(Redraw::Edit(&edit));
        assert_eq!((request.steps, request.cfg), (0, 0.0));
        assert_eq!(request.negative, "");
        assert_eq!(request.tuning, Tuning::default());

        let (_folder, ai) = ai();
        let cancel = CancelToken::new();
        let huge = Image::new(upscale::REFINE_SIDE + 1, 8, Depth::Eight, Color::WHITE).unwrap();
        let error = ai.detail(&huge, None, &pass, false, &cancel).unwrap_err();
        assert_eq!(error.code, "refineTooLarge");
    }

    #[test]
    fn generated_pictures_are_placed_over_the_original() {
        let image = Image::new(4, 2, Depth::Eight, Color::WHITE).unwrap();
        let generated = Image::new(8, 4, Depth::Eight, Color::rgb(10, 20, 30)).unwrap();
        let whole = place(&image, &generated, None).unwrap();
        assert_eq!((whole.width(), whole.height()), (4, 2));
        assert_eq!(whole.pixel8(3, 1), [10, 20, 30, 255]);

        let mut mask = Mask {
            width: 4,
            height: 2,
            data: vec![0; 8],
        };
        mask.data[0] = 255;
        let part = place(&image, &generated, Some(&mask)).unwrap();
        assert_eq!(part.pixel8(0, 0), [10, 20, 30, 255]);
        assert_eq!(part.pixel8(1, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn a_shown_edit_is_reused_without_the_generator() {
        let (_folder, ai) = ai();
        let image = Image::new(32, 24, Depth::Eight, Color::rgb(40, 80, 120)).unwrap();
        let spec = AiEdit {
            prompt: String::from("make it night"),
            ..AiEdit::default()
        };
        let print = Print::of(&image, Redraw::Edit(&spec)).unwrap();
        ai.edits.remember(print, image.clone());
        let step = Step::new("edit", Op::AiEdit(spec));
        assert!(ai.generator_steps_cached(&[&image], std::slice::from_ref(&step)));

        let other = Image::new(32, 24, Depth::Eight, Color::rgb(200, 10, 10)).unwrap();
        assert!(!ai.generator_steps_cached(&[&other], &[step]));

        // A change shown on a reduced copy is used for the full file; a
        // detail pass is used only at about the size it was painted at.
        let full = Image::new(320, 240, Depth::Eight, Color::rgb(40, 80, 120)).unwrap();
        let near = Image::new(33, 25, Depth::Eight, Color::rgb(40, 80, 120)).unwrap();
        let change = AiEdit {
            prompt: String::from("make it night"),
            ..AiEdit::default()
        };
        assert!(ai.is_cached(&full, Redraw::Edit(&change)));
        let detail = AiDetail {
            prompt: String::from("a night street"),
            ..AiDetail::default()
        };
        let pass = Redraw::Detail(&detail);
        assert!(!ai.is_cached(&image, pass));
        ai.edits
            .remember(Print::of(&image, pass).unwrap(), image.clone());
        assert!(ai.is_cached(&image, pass));
        assert!(ai.is_cached(&near, pass));
        assert!(!ai.is_cached(&full, pass));
        let step = Step::new("detail", Op::AiDetail(detail.clone()));
        assert!(ai.generator_steps_cached(&[&near], std::slice::from_ref(&step)));
        assert!(!ai.generator_steps_cached(&[&full], &[step]));

        let prompted = Step::new(
            "fill",
            Op::Inpaint(pixelit_core::pipeline::Inpaint {
                prompt: String::from("a cat"),
                ..pixelit_core::pipeline::Inpaint::default()
            }),
        );
        assert!(!ai.generator_steps_cached(&[&image], &[prompted]));
        assert!(ai.generator_steps_cached(&[], &[]));
    }

    #[test]
    fn settings_are_saved_normalized() {
        let (_folder, ai) = ai();
        let mut settings = ai.settings();
        settings.generation.steps = 1000;
        settings.device = DeviceChoice::Cpu;
        let saved = ai.set_settings(settings).unwrap();
        assert_eq!(saved.generation.steps, 150);
        assert_eq!(AiSettings::load(ai.paths()).device, DeviceChoice::Cpu);
    }
}
