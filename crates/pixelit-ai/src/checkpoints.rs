//! The models a generator can draw with.
//!
//! ComfyUI and Forge are asked for their lists. stable-diffusion.cpp uses the
//! model files next to the one already chosen. A cloud service is asked only
//! when outside services are allowed; otherwise the saved model name is shown
//! and nothing is sent.

use std::fs;
use std::time::Duration;

use pixelit_core::Result;
use pixelit_engine::Paths;
use serde::Serialize;

use crate::config::{AiSettings, GenerationProvider};
use crate::generate;
use crate::llm;
use crate::net::Net;

/// One model the interface can offer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationModel {
    pub id: String,
    pub name: String,
}

/// Models of a generator, and whether the user can pick one.
/// A custom ComfyUI workflow that has no `%checkpoint%` placeholder decides
/// the model itself, so the list is not selectable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelList {
    pub models: Vec<GenerationModel>,
    pub selectable: bool,
}

/// Lists the models of `provider`. `None` means the provider of the settings.
pub fn list(
    net: &Net,
    paths: &Paths,
    settings: &AiSettings,
    provider: GenerationProvider,
) -> Result<ModelList> {
    let provider = match provider {
        GenerationProvider::None => settings.generation.provider,
        chosen => chosen,
    };
    match provider {
        GenerationProvider::None => Ok(ModelList {
            models: Vec::new(),
            selectable: false,
        }),
        GenerationProvider::Comfy => comfy(net, settings),
        GenerationProvider::Forge => forge(net, settings),
        GenerationProvider::SdCpp => Ok(sdcpp(paths, settings)),
        GenerationProvider::Cloud => cloud(net, settings),
    }
}

fn comfy(net: &Net, settings: &AiSettings) -> Result<ModelList> {
    let workflow = settings.generation.comfy_workflow.trim();
    let selectable = workflow.is_empty() || workflow_takes_checkpoint(workflow);
    if !selectable {
        return Ok(ModelList {
            models: Vec::new(),
            selectable: false,
        });
    }
    let base = settings.generation.comfy_url.as_str();
    let key = crate::generate::access(settings, base)?;
    let answer = net.get_json(
        &format!("{base}/object_info/CheckpointLoaderSimple"),
        key.as_deref(),
        Some(Duration::from_secs(20)),
    )?;
    let models = generate::comfy_choices(&answer, "CheckpointLoaderSimple", "ckpt_name")
        .into_iter()
        .map(|name| GenerationModel {
            id: name.clone(),
            name,
        })
        .collect();
    Ok(ModelList {
        models,
        selectable: true,
    })
}

fn workflow_takes_checkpoint(path: &str) -> bool {
    fs::read_to_string(path).map_or(true, |text| text.contains("%checkpoint%"))
}

fn forge(net: &Net, settings: &AiSettings) -> Result<ModelList> {
    let base = settings.generation.forge_url.as_str();
    let key = crate::generate::access(settings, base)?;
    let answer = net.get_json(
        &format!("{base}/sdapi/v1/sd-models"),
        key.as_deref(),
        Some(Duration::from_secs(20)),
    )?;
    let models = answer
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| model["title"].as_str())
        .map(|title| GenerationModel {
            id: title.to_owned(),
            name: title.to_owned(),
        })
        .collect();
    Ok(ModelList {
        models,
        selectable: true,
    })
}

fn sdcpp(_paths: &Paths, settings: &AiSettings) -> ModelList {
    let current = settings.generation.sdcpp_model.trim();
    let mut models = Vec::new();
    for path in crate::apps::checkpoint_files(&settings.apps.models, current) {
        let id = path.display().to_string();
        let name = path
            .file_name()
            .map_or_else(|| id.clone(), |name| name.to_string_lossy().into_owned());
        models.push(GenerationModel { id, name });
    }
    models.sort_by_key(|model| model.name.to_ascii_lowercase());
    ModelList {
        models,
        selectable: true,
    }
}

fn cloud(net: &Net, settings: &AiSettings) -> Result<ModelList> {
    let current = settings.generation.cloud.model.trim();
    if !settings.allow_cloud {
        let models = (!current.is_empty())
            .then(|| GenerationModel {
                id: current.to_owned(),
                name: current.to_owned(),
            })
            .into_iter()
            .collect();
        return Ok(ModelList {
            models,
            selectable: true,
        });
    }
    let names = llm::models(net, settings, &settings.generation.cloud.url)?;
    let mut models: Vec<GenerationModel> = names
        .into_iter()
        .map(|name| GenerationModel {
            id: name.clone(),
            name,
        })
        .collect();
    if !current.is_empty() && !models.iter().any(|model| model.id == current) {
        models.insert(
            0,
            GenerationModel {
                id: current.to_owned(),
                name: current.to_owned(),
            },
        );
    }
    Ok(ModelList {
        models,
        selectable: true,
    })
}

/// What a generator offers besides checkpoints, in the names it uses itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// False when the generator takes none of this: stable-diffusion.cpp, a
    /// cloud service, or a ComfyUI workflow of the user's own.
    pub supported: bool,
    pub samplers: Vec<String>,
    pub schedulers: Vec<String>,
    pub vaes: Vec<String>,
    pub loras: Vec<String>,
    pub embeddings: Vec<String>,
    /// The sampler and the schedule used when none is chosen; empty when the
    /// generator decides that itself.
    pub sampler: String,
    pub scheduler: String,
}

/// Asks `provider` for its samplers, schedules, VAE and LoRA files.
/// `None` means the provider of the settings.
pub fn options(net: &Net, settings: &AiSettings, provider: GenerationProvider) -> Result<Options> {
    let provider = match provider {
        GenerationProvider::None => settings.generation.provider,
        chosen => chosen,
    };
    if !generate::takes_tuning(settings, provider) {
        return Ok(Options::default());
    }
    match provider {
        GenerationProvider::Comfy => comfy_options(net, settings),
        GenerationProvider::Forge => forge_options(net, settings),
        GenerationProvider::None | GenerationProvider::SdCpp | GenerationProvider::Cloud => {
            Ok(Options::default())
        }
    }
}

const ASK: Duration = Duration::from_secs(20);

fn comfy_options(net: &Net, settings: &AiSettings) -> Result<Options> {
    let base = settings.generation.comfy_url.as_str();
    let key = generate::access(settings, base)?;
    let ask = |node: &str| {
        net.get_json(
            &format!("{base}/object_info/{node}"),
            key.as_deref(),
            Some(ASK),
        )
    };
    // The sampler node is in every ComfyUI: when it does not answer, nothing does.
    let sampler = ask("KSampler")?;
    let choices = |node: &str, input: &str| {
        ask(node).map_or_default(|answer| generate::comfy_choices(&answer, node, input))
    };
    Ok(Options {
        supported: true,
        samplers: generate::comfy_choices(&sampler, "KSampler", "sampler_name"),
        schedulers: generate::comfy_choices(&sampler, "KSampler", "scheduler"),
        vaes: choices("VAELoader", "vae_name"),
        loras: choices("LoraLoader", "lora_name"),
        embeddings: generate::comfy_embeddings(net, base, key.as_deref()),
        sampler: generate::COMFY_SAMPLER.to_owned(),
        scheduler: generate::COMFY_SCHEDULER.to_owned(),
    })
}

fn forge_options(net: &Net, settings: &AiSettings) -> Result<Options> {
    let base = settings.generation.forge_url.as_str();
    let key = generate::access(settings, base)?;
    let ask = |path: &str| {
        net.get_json(
            &format!("{base}/sdapi/v1/{path}"),
            key.as_deref(),
            Some(ASK),
        )
    };
    let samplers = ask("samplers")?;
    let list =
        |path: &str, fields: &[&str]| ask(path).map_or_default(|answer| named(&answer, fields));
    Ok(Options {
        supported: true,
        samplers: named(&samplers, &["name"]),
        // Older versions have no list of schedules; the field is then not shown.
        schedulers: list("schedulers", &["label", "name"]),
        // Forge versions disagree on how a VAE is named in a request.
        vaes: Vec::new(),
        loras: list("loras", &["name"]),
        embeddings: ask("embeddings").map_or_default(|answer| forge_embeddings(&answer)),
        sampler: String::new(),
        scheduler: String::new(),
    })
}

/// The embeddings Forge has loaded for the model it holds now. Its answer is
/// `{"loaded": {name: {..}}, "skipped": {..}}`; the skipped ones do not fit
/// that model and would be read as plain words.
fn forge_embeddings(answer: &serde_json::Value) -> Vec<String> {
    answer["loaded"]
        .as_object()
        .into_iter()
        .flat_map(|loaded| loaded.keys())
        .filter(|name| !name.is_empty())
        .cloned()
        .collect()
}

/// The first of `fields` every item of a list has.
fn named(answer: &serde_json::Value, fields: &[&str]) -> Vec<String> {
    answer
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| fields.iter().find_map(|field| item[*field].as_str()))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The model a request uses: the one it names, or the one in the settings.
pub fn chosen_model<'a>(
    settings: &'a AiSettings,
    provider: GenerationProvider,
    requested: &'a str,
) -> &'a str {
    let requested = requested.trim();
    if !requested.is_empty() {
        return requested;
    }
    match provider {
        GenerationProvider::None => "",
        GenerationProvider::Comfy => settings.generation.comfy_checkpoint.as_str(),
        GenerationProvider::Forge => settings.generation.forge_checkpoint.as_str(),
        GenerationProvider::SdCpp => settings.generation.sdcpp_model.as_str(),
        GenerationProvider::Cloud => settings.generation.cloud.model.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_model_wins_over_the_settings() {
        let mut settings = AiSettings::default();
        settings.generation.comfy_checkpoint = String::from("settings.safetensors");
        settings.generation.forge_checkpoint = String::from("forge.safetensors");
        assert_eq!(
            chosen_model(&settings, GenerationProvider::Comfy, " asked.safetensors "),
            "asked.safetensors"
        );
        assert_eq!(
            chosen_model(&settings, GenerationProvider::Comfy, ""),
            "settings.safetensors"
        );
        assert_eq!(
            chosen_model(&settings, GenerationProvider::Forge, ""),
            "forge.safetensors"
        );
        assert_eq!(chosen_model(&settings, GenerationProvider::SdCpp, ""), "");
    }

    #[test]
    fn forge_lists_are_read_by_name() {
        let samplers =
            serde_json::json!([{ "name": "Euler a" }, { "name": "DPM++ 2M" }, { "name": "" }]);
        assert_eq!(named(&samplers, &["name"]), ["Euler a", "DPM++ 2M"]);
        let schedules =
            serde_json::json!([{ "name": "karras", "label": "Karras" }, { "name": "sgm_uniform" }]);
        assert_eq!(
            named(&schedules, &["label", "name"]),
            ["Karras", "sgm_uniform"]
        );
        assert!(named(&serde_json::json!({ "detail": "Not Found" }), &["name"]).is_empty());
    }

    #[test]
    fn forge_offers_the_embeddings_it_has_loaded() {
        let answer = serde_json::json!({
            "loaded": { "negativeXL_D": { "step": null }, "zPDXL-neg": {} },
            "skipped": { "EasyNegative": {} },
        });
        assert_eq!(forge_embeddings(&answer), ["negativeXL_D", "zPDXL-neg"]);
        assert!(forge_embeddings(&serde_json::json!({ "detail": "Not Found" })).is_empty());
    }

    #[test]
    fn a_generator_without_tuning_offers_none() {
        let mut settings = AiSettings::default();
        settings.generation.provider = GenerationProvider::SdCpp;
        let net = Net::new();
        let none = options(&net, &settings, GenerationProvider::None).unwrap();
        assert!(!none.supported);
        assert!(none.samplers.is_empty());
        settings.generation.comfy_workflow = String::from("flow.json");
        assert!(
            !options(&net, &settings, GenerationProvider::Comfy)
                .unwrap()
                .supported
        );
    }

    #[test]
    fn a_workflow_without_the_placeholder_is_not_selectable() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("flow.json");
        fs::write(&path, r#"{"text":"%prompt%"}"#).unwrap();
        assert!(!workflow_takes_checkpoint(&path.display().to_string()));
        fs::write(&path, r#"{"ckpt":"%checkpoint%"}"#).unwrap();
        assert!(workflow_takes_checkpoint(&path.display().to_string()));
        assert!(workflow_takes_checkpoint("missing-workflow.json"));
    }

    #[test]
    fn model_files_beside_the_current_one_are_listed() {
        let folder = tempfile::tempdir().unwrap();
        let model = folder.path().join("one.safetensors");
        fs::write(&model, b"model").unwrap();
        fs::write(folder.path().join("two.gguf"), b"model").unwrap();
        fs::write(folder.path().join("notes.txt"), b"no").unwrap();
        let mut settings = AiSettings::default();
        settings.generation.sdcpp_model = model.display().to_string();
        let list = sdcpp(&Paths::at(folder.path()), &settings);
        assert_eq!(list.models.len(), 2);
        assert!(list.selectable);
    }
}
