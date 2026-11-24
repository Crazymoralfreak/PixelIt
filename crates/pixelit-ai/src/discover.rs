//! Search for AI servers running on this computer. Only the usual local
//! ports are asked; nothing leaves the machine.

use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::net::Net;

const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ServerKind {
    Ollama,
    LmStudio,
    LlamaCpp,
    Comfy,
    Forge,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub kind: ServerKind,
    /// Address to put into the settings.
    pub url: String,
    /// Language models, or checkpoints of an image server.
    pub models: Vec<String>,
}

struct Probe {
    kind: ServerKind,
    url: &'static str,
    /// Path asked to see whether the server is there.
    path: &'static str,
}

static PROBES: [Probe; 5] = [
    Probe {
        kind: ServerKind::Ollama,
        url: "http://127.0.0.1:11434/v1",
        path: "/models",
    },
    Probe {
        kind: ServerKind::LmStudio,
        url: "http://127.0.0.1:1234/v1",
        path: "/models",
    },
    Probe {
        kind: ServerKind::LlamaCpp,
        url: "http://127.0.0.1:8080/v1",
        path: "/models",
    },
    Probe {
        kind: ServerKind::Comfy,
        url: "http://127.0.0.1:8188",
        path: "/object_info/CheckpointLoaderSimple",
    },
    Probe {
        kind: ServerKind::Forge,
        url: "http://127.0.0.1:7860",
        path: "/sdapi/v1/sd-models",
    },
];

fn names(kind: ServerKind, answer: &Value) -> Option<Vec<String>> {
    let list = match kind {
        ServerKind::Ollama | ServerKind::LmStudio | ServerKind::LlamaCpp => answer["data"]
            .as_array()?
            .iter()
            .filter_map(|model| model["id"].as_str().map(str::to_owned))
            .collect(),
        ServerKind::Comfy => answer["CheckpointLoaderSimple"]["input"]["required"]["ckpt_name"][0]
            .as_array()?
            .iter()
            .filter_map(|name| name.as_str().map(str::to_owned))
            .collect(),
        ServerKind::Forge => answer
            .as_array()?
            .iter()
            .filter_map(|model| model["title"].as_str().map(str::to_owned))
            .collect(),
    };
    Some(list)
}

/// Asks the usual local ports and returns the servers that answered.
pub fn local_servers(net: &Net) -> Vec<Found> {
    thread::scope(|scope| {
        let probes: Vec<_> = PROBES
            .iter()
            .map(|probe| {
                scope.spawn(move || {
                    let answer = net
                        .get_json(
                            &format!("{}{}", probe.url, probe.path),
                            None,
                            Some(PROBE_TIMEOUT),
                        )
                        .ok()?;
                    Some(Found {
                        kind: probe.kind,
                        url: probe.url.to_owned(),
                        models: names(probe.kind, &answer)?,
                    })
                })
            })
            .collect();
        probes
            .into_iter()
            .filter_map(|probe| probe.join().ok().flatten())
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn answers_are_read_per_server() {
        let chat = json!({ "data": [{ "id": "llava" }, { "id": "qwen" }] });
        assert_eq!(names(ServerKind::Ollama, &chat).unwrap(), ["llava", "qwen"]);
        let comfy = json!({ "CheckpointLoaderSimple": { "input": { "required": {
            "ckpt_name": [["sdxl.safetensors"]],
        } } } });
        assert_eq!(
            names(ServerKind::Comfy, &comfy).unwrap(),
            ["sdxl.safetensors"]
        );
        let forge = json!([{ "title": "dream [abc]" }]);
        assert_eq!(names(ServerKind::Forge, &forge).unwrap(), ["dream [abc]"]);
        assert!(names(ServerKind::Forge, &chat).is_none());
    }

    #[test]
    fn probes_stay_on_this_computer() {
        for probe in &PROBES {
            assert!(crate::net::is_local(probe.url), "{}", probe.url);
        }
    }
}
