//! Search for AI servers running on this computer. Only the usual local
//! ports are asked; nothing leaves the machine.
//!
//! Chat servers are recognised by their fixed ports. Picture servers share
//! ports, so each candidate is asked what it is: ComfyUI, Forge or Fooocus.

use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::config::AiSettings;
use crate::generate::comfy_choices;
use crate::net::Net;

const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// Ports picture programs commonly take. Fooocus starts at 7860 and walks
/// up; ComfyUI uses 8188, and the desktop build uses 8000.
const PICTURE_PORTS: [u16; 13] = [
    7860, 7861, 7862, 7863, 7864, 7865, 7866, 7867, 7868, 7869, 7870, 8188, 8000,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ServerKind {
    Ollama,
    LmStudio,
    LlamaCpp,
    Comfy,
    Forge,
    Fooocus,
}

impl ServerKind {
    /// The name used in the interface.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::LmStudio => "lmStudio",
            Self::LlamaCpp => "llamaCpp",
            Self::Comfy => "comfy",
            Self::Forge => "forge",
            Self::Fooocus => "fooocus",
        }
    }
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

struct ChatProbe {
    kind: ServerKind,
    url: &'static str,
    /// Path asked to see whether the server is there.
    path: &'static str,
}

static CHAT_PROBES: [ChatProbe; 3] = [
    ChatProbe {
        kind: ServerKind::Ollama,
        url: "http://127.0.0.1:11434/v1",
        path: "/models",
    },
    ChatProbe {
        kind: ServerKind::LmStudio,
        url: "http://127.0.0.1:1234/v1",
        path: "/models",
    },
    ChatProbe {
        kind: ServerKind::LlamaCpp,
        url: "http://127.0.0.1:8080/v1",
        path: "/models",
    },
];

/// One question asked of a picture server, in the order they are tried.
#[derive(Clone, Copy)]
enum PictureAsk {
    Comfy,
    Forge,
    Fooocus,
}

impl PictureAsk {
    const ORDER: [Self; 3] = [Self::Comfy, Self::Forge, Self::Fooocus];

    const fn path(self) -> &'static str {
        match self {
            Self::Comfy => "/object_info/CheckpointLoaderSimple",
            Self::Forge => "/sdapi/v1/sd-models",
            Self::Fooocus => "/config",
        }
    }
}

fn model_names(kind: ServerKind, answer: &Value) -> Option<Vec<String>> {
    let list = match kind {
        ServerKind::Ollama | ServerKind::LmStudio | ServerKind::LlamaCpp => answer["data"]
            .as_array()?
            .iter()
            .filter_map(|model| model["id"].as_str().map(str::to_owned))
            .collect(),
        ServerKind::Comfy => {
            answer.get("CheckpointLoaderSimple")?.get("input")?;
            comfy_choices(answer, "CheckpointLoaderSimple", "ckpt_name")
        }
        ServerKind::Forge => answer
            .as_array()?
            .iter()
            .filter_map(|model| model["title"].as_str().map(str::to_owned))
            .collect(),
        ServerKind::Fooocus => Vec::new(),
    };
    Some(list)
}

/// What a picture server's answer says it is, when the answer is one of ours.
fn picture_kind(ask: PictureAsk, answer: &Value) -> Option<(ServerKind, Vec<String>)> {
    match ask {
        PictureAsk::Comfy => {
            let models = model_names(ServerKind::Comfy, answer)?;
            Some((ServerKind::Comfy, models))
        }
        PictureAsk::Forge => {
            let models = model_names(ServerKind::Forge, answer)?;
            Some((ServerKind::Forge, models))
        }
        PictureAsk::Fooocus => {
            let title = answer.get("title").and_then(Value::as_str)?;
            if title.to_ascii_lowercase().starts_with("fooocus") {
                Some((ServerKind::Fooocus, Vec::new()))
            } else {
                None
            }
        }
    }
}

/// The port of a local address, when the address names one.
fn local_port(url: &str) -> Option<u16> {
    if !crate::net::is_local(url) {
        return None;
    }
    let rest = url.trim().split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let port = if let Some(rest) = host.strip_prefix('[') {
        rest.split_once(']')?.1.strip_prefix(':')?
    } else {
        let (_, port) = host.rsplit_once(':')?;
        port
    };
    port.parse().ok()
}

/// Ports to ask: the usual ones, then any the settings already name.
fn picture_ports(settings: &AiSettings) -> Vec<u16> {
    let mut ports = Vec::from(PICTURE_PORTS);
    let saved = [
        local_port(&settings.generation.comfy_url),
        local_port(&settings.generation.forge_url),
        Some(settings.apps.comfy.port).filter(|port| *port != 0),
        Some(settings.apps.forge.port).filter(|port| *port != 0),
    ];
    for port in saved.into_iter().flatten() {
        if !ports.contains(&port) {
            ports.push(port);
        }
    }
    ports
}

fn ask_picture(net: &Net, port: u16) -> Option<Found> {
    let base = format!("http://127.0.0.1:{port}");
    for ask in PictureAsk::ORDER {
        let Ok(answer) = net.get_json(&format!("{base}{}", ask.path()), None, Some(PROBE_TIMEOUT))
        else {
            continue;
        };
        if let Some((kind, models)) = picture_kind(ask, &answer) {
            return Some(Found {
                kind,
                url: base,
                models,
            });
        }
    }
    None
}

/// Asks the usual local ports and returns the servers that answered.
pub fn local_servers(net: &Net, settings: &AiSettings) -> Vec<Found> {
    thread::scope(|scope| {
        let mut handles = Vec::new();
        for probe in &CHAT_PROBES {
            handles.push(scope.spawn(move || {
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
                    models: model_names(probe.kind, &answer)?,
                })
            }));
        }
        for port in picture_ports(settings) {
            handles.push(scope.spawn(move || ask_picture(net, port)));
        }
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok().flatten())
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
        assert_eq!(
            model_names(ServerKind::Ollama, &chat).unwrap(),
            ["llava", "qwen"]
        );
        let comfy = json!({ "CheckpointLoaderSimple": { "input": { "required": {
            "ckpt_name": [["sdxl.safetensors"]],
        } } } });
        assert_eq!(
            picture_kind(PictureAsk::Comfy, &comfy).unwrap().0,
            ServerKind::Comfy
        );
        assert_eq!(
            picture_kind(PictureAsk::Comfy, &comfy).unwrap().1,
            ["sdxl.safetensors"]
        );
        // Nodes on the newer ComfyUI schema list their files under "options".
        let newer = json!({ "CheckpointLoaderSimple": { "input": { "required": {
            "ckpt_name": ["COMBO", { "multiselect": false, "options": ["flux.safetensors"] }],
        } } } });
        assert_eq!(
            picture_kind(PictureAsk::Comfy, &newer).unwrap().1,
            ["flux.safetensors"]
        );
        let forge = json!([{ "title": "dream [abc]" }]);
        assert_eq!(
            picture_kind(PictureAsk::Forge, &forge).unwrap().1,
            ["dream [abc]"]
        );
        assert!(picture_kind(PictureAsk::Forge, &chat).is_none());
        let fooocus = json!({ "title": "Fooocus 2.6.5 (mashb1t)" });
        assert_eq!(
            picture_kind(PictureAsk::Fooocus, &fooocus).unwrap().0,
            ServerKind::Fooocus
        );
        // Forge also answers /config, and its title is not Fooocus.
        let gradio = json!({ "title": "Stable Diffusion" });
        assert!(picture_kind(PictureAsk::Fooocus, &gradio).is_none());
        assert!(picture_kind(PictureAsk::Comfy, &forge).is_none());
    }

    #[test]
    fn saved_local_ports_are_added_once() {
        let mut settings = AiSettings::default();
        settings.generation.comfy_url = String::from("http://127.0.0.1:8000/");
        settings.generation.forge_url = String::from("http://192.168.1.5:7860");
        settings.apps.comfy.port = 8000;
        settings.apps.forge.port = 7865;
        let ports = picture_ports(&settings);
        assert_eq!(ports.iter().filter(|port| **port == 8000).count(), 1);
        assert!(ports.contains(&7865));
        assert!(ports.contains(&8188));
        assert_eq!(ports.iter().filter(|port| **port == 7860).count(), 1);
        assert!(local_port("http://192.168.1.5:7860").is_none());
        assert_eq!(local_port("http://localhost:9000/api").unwrap(), 9000);
    }

    #[test]
    fn probes_stay_on_this_computer() {
        for probe in &CHAT_PROBES {
            assert!(crate::net::is_local(probe.url), "{}", probe.url);
        }
        for port in picture_ports(&AiSettings::default()) {
            assert!(crate::net::is_local(&format!("http://127.0.0.1:{port}")));
        }
    }
}
