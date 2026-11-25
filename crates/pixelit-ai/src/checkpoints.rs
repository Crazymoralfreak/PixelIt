//! The models a generator can draw with.
//!
//! ComfyUI and Forge are asked for their lists. stable-diffusion.cpp uses the
//! model files next to the one already chosen. A cloud service is asked only
//! when outside services are allowed; otherwise the saved model name is shown
//! and nothing is sent.

use std::fs;
use std::path::Path;
use std::time::Duration;

use pixelit_core::Result;
use pixelit_engine::Paths;
use serde::Serialize;

use crate::config::{AiSettings, GenerationProvider};
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

const MODEL_EXTENSIONS: &[&str] = &["safetensors", "gguf", "ckpt", "pth"];

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
    let models = answer["CheckpointLoaderSimple"]["input"]["required"]["ckpt_name"][0]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str())
        .map(|name| GenerationModel {
            id: name.to_owned(),
            name: name.to_owned(),
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
    let Some(folder) = Path::new(current).parent() else {
        return ModelList {
            models: Vec::new(),
            selectable: true,
        };
    };
    let mut models = Vec::new();
    if let Ok(entries) = fs::read_dir(folder) {
        for entry in entries.flatten() {
            let path = entry.path();
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .map_or_default(str::to_ascii_lowercase);
            if !MODEL_EXTENSIONS.iter().any(|item| *item == extension) || !path.is_file() {
                continue;
            }
            let id = path.display().to_string();
            let name = path
                .file_name()
                .map_or_else(|| id.clone(), |name| name.to_string_lossy().into_owned());
            models.push(GenerationModel { id, name });
        }
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
