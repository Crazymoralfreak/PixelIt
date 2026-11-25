//! AI settings, stored in `ai.json`.
//!
//! Nothing here turns the network on by itself: cloud use is off until the
//! user enables it, and every endpoint is an address the user typed or
//! accepted from the search for local servers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use pixelit_core::Result;
use pixelit_engine::store::{load_or_default, no_migration, save_json};
use pixelit_engine::Paths;

/// Version of the `ai.json` format.
pub const AI_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceChoice {
    /// The video card when there is one, the processor otherwise.
    #[default]
    Auto,
    Gpu,
    Cpu,
}

/// Default model of every local task; an empty id takes the first installed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskModels {
    pub background: String,
    pub upscale: String,
    pub inpaint: String,
    pub faces: String,
}

/// An OpenAI-compatible server: Ollama, LM Studio, llama.cpp or a cloud API.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Endpoint {
    /// Base address ending with the API root, such as `http://127.0.0.1:11434/v1`.
    pub url: String,
    pub model: String,
}

impl Endpoint {
    pub fn is_set(&self) -> bool {
        !self.url.trim().is_empty() && !self.model.trim().is_empty()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GenerationProvider {
    #[default]
    None,
    Comfy,
    Forge,
    SdCpp,
    Cloud,
}

impl GenerationProvider {
    /// The name used in settings, requests and the interface.
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Comfy => "comfy",
            Self::Forge => "forge",
            Self::SdCpp => "sdCpp",
            Self::Cloud => "cloud",
        }
    }

    /// `None` when the text is empty or not a generator name.
    pub fn parse(text: &str) -> Self {
        match text.trim() {
            "comfy" => Self::Comfy,
            "forge" => Self::Forge,
            "sdCpp" => Self::SdCpp,
            "cloud" => Self::Cloud,
            _ => Self::None,
        }
    }
}

/// Size, steps and CFG remembered for one model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelDefaults {
    pub width: u32,
    pub height: u32,
    pub steps: u32,
    pub cfg: f32,
}

impl Default for ModelDefaults {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 1024,
            steps: 24,
            cfg: 6.0,
        }
    }
}

fn remember_parameters() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Generation {
    pub provider: GenerationProvider,
    pub comfy_url: String,
    /// Checkpoint name as ComfyUI lists it; empty takes the first one.
    pub comfy_checkpoint: String,
    /// Path of a workflow in the ComfyUI API format with `%prompt%`,
    /// `%negative%`, `%image%`, `%checkpoint%`, `%seed%`, `%width%`,
    /// `%height%`, `%steps%`, `%cfg%` placeholders; empty uses the built-in
    /// text-to-image graph.
    pub comfy_workflow: String,
    pub forge_url: String,
    /// Checkpoint name as Forge lists it; empty keeps the model loaded there.
    pub forge_checkpoint: String,
    /// Path of the checkpoint stable-diffusion.cpp loads.
    pub sdcpp_model: String,
    /// Extra command line arguments of stable-diffusion.cpp.
    pub sdcpp_arguments: String,
    /// Image API of a cloud service: base address and model.
    pub cloud: Endpoint,
    pub width: u32,
    pub height: u32,
    pub steps: u32,
    pub cfg: f32,
    pub negative: String,
    /// Styles switched on in the studio, in the order they are applied.
    pub styles: Vec<String>,
    /// Size, steps and CFG last used with a generator and a model.
    pub model_defaults: BTreeMap<String, ModelDefaults>,
    /// Write the prompt into the PNG, in the text block other tools read.
    #[serde(default = "remember_parameters")]
    pub embed_parameters: bool,
}

/// Key of [`Generation::model_defaults`] for one generator and model.
pub fn defaults_key(provider: GenerationProvider, model: &str) -> String {
    format!("{}\u{1}{}", provider.name(), model.trim())
}

impl Default for Generation {
    fn default() -> Self {
        Self {
            provider: GenerationProvider::None,
            comfy_url: String::from("http://127.0.0.1:8188"),
            comfy_checkpoint: String::new(),
            comfy_workflow: String::new(),
            forge_url: String::from("http://127.0.0.1:7860"),
            forge_checkpoint: String::new(),
            sdcpp_model: String::new(),
            sdcpp_arguments: String::new(),
            cloud: Endpoint::default(),
            width: 1024,
            height: 1024,
            steps: 24,
            cfg: 6.0,
            negative: String::new(),
            styles: Vec::new(),
            model_defaults: BTreeMap::new(),
            embed_parameters: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiSettings {
    pub device: DeviceChoice,
    /// Index of the video card; `-1` lets the system choose.
    pub gpu: i32,
    /// Processor threads of a model; `0` lets the runtime choose.
    pub threads: usize,
    pub models: TaskModels,
    /// Allows requests to servers outside this computer. Off by default.
    pub allow_cloud: bool,
    /// Asks before every upload of a picture to such a server.
    pub confirm_cloud: bool,
    /// Model that looks at pictures: descriptions, tags, text recognition.
    pub vision: Endpoint,
    /// Model of the assistant chat.
    pub assistant: Endpoint,
    pub generation: Generation,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            device: DeviceChoice::Auto,
            gpu: -1,
            threads: 0,
            models: TaskModels::default(),
            allow_cloud: false,
            confirm_cloud: true,
            vision: Endpoint::default(),
            assistant: Endpoint::default(),
            generation: Generation::default(),
        }
    }
}

impl AiSettings {
    pub fn load(paths: &Paths) -> Self {
        let mut settings: Self = load_or_default(&paths.ai(), AI_VERSION, no_migration);
        settings.normalize();
        settings
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        save_json(&paths.ai(), AI_VERSION, self)
    }

    /// Brings values typed by hand into their ranges.
    pub fn normalize(&mut self) {
        self.gpu = self.gpu.clamp(-1, 16);
        self.threads = self.threads.min(256);
        let generation = &mut self.generation;
        generation.width = (generation.width.clamp(64, 4096) / 8) * 8;
        generation.height = (generation.height.clamp(64, 4096) / 8) * 8;
        generation.steps = generation.steps.clamp(1, 150);
        generation.styles.retain(|id| !id.trim().is_empty());
        for defaults in generation.model_defaults.values_mut() {
            defaults.width = (defaults.width.clamp(64, 4096) / 8) * 8;
            defaults.height = (defaults.height.clamp(64, 4096) / 8) * 8;
            defaults.steps = defaults.steps.clamp(1, 150);
            defaults.cfg = if defaults.cfg.is_finite() {
                defaults.cfg.clamp(0.0, 30.0)
            } else {
                6.0
            };
        }
        generation.cfg = if generation.cfg.is_finite() {
            generation.cfg.clamp(0.0, 30.0)
        } else {
            6.0
        };
        for url in [
            &mut generation.comfy_url,
            &mut generation.forge_url,
            &mut generation.cloud.url,
            &mut self.vision.url,
            &mut self.assistant.url,
        ] {
            let trimmed = url.trim().trim_end_matches('/').to_owned();
            *url = trimmed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_is_off_by_default() {
        let settings = AiSettings::default();
        assert!(!settings.allow_cloud);
        assert!(settings.confirm_cloud);
        assert_eq!(settings.generation.provider, GenerationProvider::None);
        assert!(!settings.vision.is_set());
    }

    #[test]
    fn values_are_normalized() {
        let mut settings = AiSettings::default();
        settings.generation.width = 1001;
        settings.generation.height = 9;
        settings.generation.steps = 0;
        settings.generation.cfg = f32::NAN;
        settings.vision.url = String::from(" http://127.0.0.1:11434/v1/ ");
        settings.normalize();
        assert_eq!(settings.generation.width, 1000);
        assert_eq!(settings.generation.height, 64);
        assert_eq!(settings.generation.steps, 1);
        assert_eq!(settings.generation.cfg, 6.0);
        assert_eq!(settings.vision.url, "http://127.0.0.1:11434/v1");
    }

    #[test]
    fn settings_survive_a_round_trip() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        let mut settings = AiSettings::default();
        settings.models.upscale = String::from("realesrgan-x4plus");
        settings.allow_cloud = true;
        settings.save(&paths).unwrap();
        assert_eq!(AiSettings::load(&paths), settings);
    }
}
