//! PixelIt AI: downloadable components and models, local models through ONNX
//! Runtime, image generation providers and vision models.
//!
//! Everything optional lives here. Without a single component installed the
//! application works as an ordinary image tool; each AI step reports what is
//! missing instead of failing in an unclear way.

pub mod catalog;
pub mod config;
pub mod discover;
mod edits;
pub mod faces;
pub mod generate;
pub mod hardware;
pub mod install;
pub mod llm;
pub mod net;
pub mod onnx;
pub mod secrets;
pub mod vision;
pub mod worker;

use std::path::PathBuf;
use std::sync::{PoisonError, RwLock};

use serde::Serialize;

use pixelit_core::pipeline::{AiBackend, AiEdit, Mask, ResizeFilter};
use pixelit_core::{ops, CancelToken, Error, Image, Result};
use pixelit_engine::Paths;

use crate::edits::{Edits, Print};

pub use crate::catalog::{Entry, Kind};
pub use crate::config::{AiSettings, DeviceChoice, Endpoint, Generation, GenerationProvider};
pub use crate::discover::Found;
pub use crate::faces::Face;
pub use crate::generate::{GenerateRequest, Source};
pub use crate::hardware::{Advice, Hardware};
pub use crate::install::Status;
pub use crate::llm::{Message, Reply, Role, ToolCall, ToolSpec};
pub use crate::net::Net;
pub use crate::onnx::{Device, Onnx};
pub use crate::vision::VisionTask;

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

    /// Frees the memory of the loaded models.
    pub fn release(&self) {
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
    ) -> Result<Vec<Image>> {
        generate::generate(
            &self.net,
            &self.paths,
            &self.settings(),
            request,
            source,
            cancel,
        )
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
        discover::local_servers(&self.net)
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
        self.generate(request, Some(&source), cancel)?
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
}

/// Puts a generated picture over the original: inside the mask, or over all
/// of it without one. The size, the transparency and the depth stay those of
/// the original.
fn place(image: &Image, generated: &Image, mask: Option<&Mask>) -> Result<Image> {
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
        // The real run repeats what the editor has shown: a generator gives
        // another picture every time it is asked.
        let print = Print::of(image, spec)?;
        if let Some(generated) = self.edits.find(&print) {
            return place(image, &generated, mask);
        }
        let request = GenerateRequest {
            prompt: spec.prompt.trim().to_owned(),
            strength: (spec.strength / 100.0).clamp(0.05, 1.0),
            seed: Some(u64::from(spec.variant)),
            ..GenerateRequest::default()
        };
        let generated = self.redraw(image, mask, &request, cancel)?;
        let result = place(image, &generated, mask)?;
        if preview {
            self.edits.remember(print, generated);
        }
        Ok(result)
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
