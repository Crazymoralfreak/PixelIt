//! AI settings, stored in `ai.json`.
//!
//! Nothing here turns the network on by itself: cloud use is off until the
//! user enables it, and every endpoint is an address the user typed or
//! accepted from the search for local servers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// The tuning of a generator is part of the "AI edit" step, so it lives with
// the pipeline; the settings and the studio use the same types.
pub use pixelit_core::pipeline::{Embedding, Lora, Tuning, MAX_EMBEDDINGS, MAX_LORAS};
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

/// Size, steps, CFG and tuning remembered for one model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelDefaults {
    pub width: u32,
    pub height: u32,
    pub steps: u32,
    pub cfg: f32,
    pub tuning: Tuning,
}

impl Default for ModelDefaults {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 1024,
            steps: 24,
            cfg: 6.0,
            tuning: Tuning::default(),
        }
    }
}

/// How the history of the studio is laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HistoryView {
    /// Pictures under the request that made them.
    #[default]
    Groups,
    /// Every picture as a tile.
    Grid,
    /// One row for a picture, with its request beside it.
    List,
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
    /// Which upscaler enlarges pictures. Empty uses the built-in model.
    /// `onnx:`, `comfy:` and `forge:` name a file or a model on that server.
    pub upscaler: String,
    /// After enlarging, ask the generator to paint detail back in.
    pub upscale_refine: bool,
    /// How far that pass may move from the enlarged picture, from 0 to 1.
    pub upscale_strength: f32,
    pub history_view: HistoryView,
}

/// Which local program a card is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppKind {
    Comfy,
    Forge,
}

impl AppKind {
    /// The name used in commands and job titles.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Comfy => "comfy",
            Self::Forge => "forge",
        }
    }

    /// `None` when the text is not a program name.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "comfy" => Some(Self::Comfy),
            "forge" => Some(Self::Forge),
            _ => None,
        }
    }
}

/// Where the copy on a card came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppOrigin {
    #[default]
    Missing,
    /// This section downloaded it.
    Ours,
    /// A folder the user pointed at.
    Folder,
    /// A package already installed by Stability Matrix.
    StabilityMatrix,
}

fn comfy_port() -> u16 {
    8188
}

fn forge_port() -> u16 {
    7860
}

/// One installed ComfyUI or Forge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalApp {
    pub home: String,
    pub origin: AppOrigin,
    /// Release tag of a copy this section installed. Empty for any other copy.
    pub release: String,
    /// Size of the archive this section installed, so a later release can be told apart.
    pub bytes: u64,
    pub port: u16,
}

impl Default for LocalApp {
    fn default() -> Self {
        Self {
            home: String::new(),
            origin: AppOrigin::Missing,
            release: String::new(),
            bytes: 0,
            port: comfy_port(),
        }
    }
}

/// A kind of model file the shared folders hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelKind {
    Checkpoints,
    Loras,
    Vaes,
    Embeddings,
    ControlNet,
    Upscale,
}

impl ModelKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Checkpoints => "checkpoints",
            Self::Loras => "loras",
            Self::Vaes => "vaes",
            Self::Embeddings => "embeddings",
            Self::ControlNet => "controlNet",
            Self::Upscale => "upscale",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "checkpoints" => Some(Self::Checkpoints),
            "loras" => Some(Self::Loras),
            "vaes" => Some(Self::Vaes),
            "embeddings" => Some(Self::Embeddings),
            "controlNet" => Some(Self::ControlNet),
            "upscale" => Some(Self::Upscale),
            _ => None,
        }
    }
}

/// Folders of checkpoints, LoRA and the other kinds. Several of one kind are kept.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelLibrary {
    pub checkpoints: Vec<String>,
    pub loras: Vec<String>,
    pub vaes: Vec<String>,
    pub embeddings: Vec<String>,
    pub control_net: Vec<String>,
    pub upscale: Vec<String>,
}

impl ModelLibrary {
    pub fn of(&self, kind: ModelKind) -> &[String] {
        match kind {
            ModelKind::Checkpoints => &self.checkpoints,
            ModelKind::Loras => &self.loras,
            ModelKind::Vaes => &self.vaes,
            ModelKind::Embeddings => &self.embeddings,
            ModelKind::ControlNet => &self.control_net,
            ModelKind::Upscale => &self.upscale,
        }
    }

    pub fn of_mut(&mut self, kind: ModelKind) -> &mut Vec<String> {
        match kind {
            ModelKind::Checkpoints => &mut self.checkpoints,
            ModelKind::Loras => &mut self.loras,
            ModelKind::Vaes => &mut self.vaes,
            ModelKind::Embeddings => &mut self.embeddings,
            ModelKind::ControlNet => &mut self.control_net,
            ModelKind::Upscale => &mut self.upscale,
        }
    }
}

/// ComfyUI, Forge and the folders their models are read from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalApps {
    pub comfy: LocalApp,
    pub forge: LocalApp,
    pub models: ModelLibrary,
    /// Adult results stay hidden until this is switched on.
    pub show_nsfw: bool,
}

impl Default for LocalApps {
    fn default() -> Self {
        Self {
            comfy: LocalApp {
                port: comfy_port(),
                ..LocalApp::default()
            },
            forge: LocalApp {
                port: forge_port(),
                ..LocalApp::default()
            },
            models: ModelLibrary::default(),
            show_nsfw: false,
        }
    }
}

impl LocalApps {
    pub fn of(&self, kind: AppKind) -> &LocalApp {
        match kind {
            AppKind::Comfy => &self.comfy,
            AppKind::Forge => &self.forge,
        }
    }

    pub fn of_mut(&mut self, kind: AppKind) -> &mut LocalApp {
        match kind {
            AppKind::Comfy => &mut self.comfy,
            AppKind::Forge => &mut self.forge,
        }
    }

    fn normalize(&mut self) {
        for app in [&mut self.comfy, &mut self.forge] {
            app.home = app.home.trim().to_owned();
            if app.home.is_empty() {
                app.origin = AppOrigin::Missing;
            }
        }
        if self.comfy.port == 0 {
            self.comfy.port = comfy_port();
        }
        if self.forge.port == 0 {
            self.forge.port = forge_port();
        }
        for folders in [
            &mut self.models.checkpoints,
            &mut self.models.loras,
            &mut self.models.vaes,
            &mut self.models.embeddings,
            &mut self.models.control_net,
            &mut self.models.upscale,
        ] {
            for folder in folders.iter_mut() {
                *folder = native_path(Path::new(folder.trim()));
            }
            folders.retain(|folder| !folder.is_empty());
            let mut seen = Vec::new();
            folders.retain(|folder| {
                let fresh = !seen.contains(folder);
                if fresh {
                    seen.push(folder.clone());
                }
                fresh
            });
        }
    }
}

/// A path written with the separators of this system: `D:\a/b` reads `D:\a\b`
/// on Windows. Explorer does not find a file by a path with mixed separators.
pub fn native_path(path: &Path) -> String {
    path.components().collect::<PathBuf>().display().to_string()
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
            upscaler: String::new(),
            upscale_refine: true,
            upscale_strength: 0.35,
            history_view: HistoryView::Groups,
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
    /// ComfyUI, Forge and the shared model folders.
    pub apps: LocalApps,
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
            apps: LocalApps::default(),
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
            defaults.tuning = defaults.tuning.tidy();
        }
        generation.cfg = if generation.cfg.is_finite() {
            generation.cfg.clamp(0.0, 30.0)
        } else {
            6.0
        };
        generation.upscale_strength = if generation.upscale_strength.is_finite() {
            generation.upscale_strength.clamp(0.05, 1.0)
        } else {
            0.35
        };
        self.apps.normalize();
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
    fn tuning_is_tidied_with_the_settings() {
        let mut settings = AiSettings::default();
        let key = defaults_key(GenerationProvider::Comfy, "sdxl.safetensors");
        settings.generation.model_defaults.insert(
            key.clone(),
            ModelDefaults {
                tuning: Tuning {
                    sampler: String::from(" euler_ancestral "),
                    scheduler: String::new(),
                    vae: String::from("sdxl_vae.safetensors"),
                    loras: vec![
                        Lora {
                            name: String::from(" detail.safetensors "),
                            weight: 9.0,
                        },
                        Lora {
                            name: String::from("  "),
                            weight: 1.0,
                        },
                        Lora {
                            name: String::from("style.safetensors"),
                            weight: f32::NAN,
                        },
                    ],
                    embeddings: vec![
                        Embedding {
                            name: String::from(" zPDXL-neg "),
                            weight: 0.0,
                            negative: true,
                        },
                        Embedding::default(),
                    ],
                },
                ..ModelDefaults::default()
            },
        );
        settings.normalize();
        let tuning = &settings.generation.model_defaults[&key].tuning;
        assert_eq!(tuning.sampler, "euler_ancestral");
        assert_eq!(tuning.loras.len(), 2);
        assert_eq!(tuning.loras[0].name, "detail.safetensors");
        assert_eq!(tuning.loras[0].weight, 4.0);
        assert_eq!(tuning.loras[1].weight, 1.0);
        assert_eq!(tuning.embeddings.len(), 1);
        assert_eq!(tuning.embeddings[0].name, "zPDXL-neg");
        assert_eq!(tuning.embeddings[0].weight, 0.1);
        assert!(tuning.embeddings[0].negative);
        // A file written before these fields existed still loads.
        let old: ModelDefaults =
            serde_json::from_str(r#"{"width":832,"height":1216,"steps":28,"cfg":5.0}"#).unwrap();
        assert_eq!(old.tuning, Tuning::default());
        assert_eq!(old.width, 832);
    }

    #[cfg(windows)]
    #[test]
    fn model_folders_are_written_with_one_kind_of_separator() {
        let mut settings = AiSettings::default();
        settings.apps.models.upscale = vec![
            String::from(r" D:\Programs\ComfyUI\models/upscale_models "),
            String::from(r"D:\Programs\ComfyUI\models\upscale_models"),
            String::from("  "),
        ];
        settings.normalize();
        assert_eq!(
            settings.apps.models.upscale,
            [r"D:\Programs\ComfyUI\models\upscale_models"]
        );
        assert_eq!(
            native_path(Path::new(r"D:\a/b\4x-UltraSharp.pt")),
            r"D:\a\b\4x-UltraSharp.pt"
        );
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
