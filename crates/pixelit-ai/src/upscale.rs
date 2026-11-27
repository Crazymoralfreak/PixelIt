//! Enlarging a picture with a chosen model, then optionally painting detail
//! back in through ComfyUI or Forge.
//!
//! The built-in model and `.onnx` files from the upscaler folders run here.
//! PyTorch files run only while ComfyUI or Forge is up; until then they are
//! listed, but not offered.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};

use pixelit_core::{CancelToken, Error, Image, Result};
use pixelit_engine::Paths;

use crate::config::{AiSettings, GenerationProvider, ModelLibrary};
use crate::generate::{self, GenerateRequest, Source, Tick};
use crate::history;
use crate::install;
use crate::net::Net;
use crate::onnx::Onnx;
use crate::parameters::{self, Parameters};

/// The built-in Real-ESRGAN model from the catalogue.
pub const BUILTIN: &str = "builtin";

const BUILTIN_NAME: &str = "Real-ESRGAN x4plus";
const BUILTIN_ID: &str = "realesrgan-x4plus";

/// A side longer than this is repainted in tiles, so the video card can hold it.
const TILED_SIDE: u32 = 1280;

const ASK: Duration = Duration::from_secs(3);

/// Resizers a server offers that are not neural models.
const PLAIN: &[&str] = &[
    "none",
    "lanczos",
    "nearest",
    "nearest-exact",
    "bilinear",
    "bicubic",
    "area",
];

const WEIGHTS: &[&str] = &["onnx", "pt", "pth", "safetensors", "bin"];

/// Where a listed model runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Engine {
    Builtin,
    Onnx,
    Comfy,
    Forge,
    /// A file no running server can load.
    File,
}

/// One upscaler the studio can show.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Upscaler {
    pub id: String,
    pub name: String,
    pub engine: Engine,
    pub available: bool,
}

/// One step of an enlargement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    Enlarge(u32),
    Refine,
}

/// What the picture was made with, so the detail pass can repeat it.
#[derive(Clone, Debug)]
pub struct Notes {
    pub prompt: String,
    pub negative: String,
    pub steps: u32,
    pub cfg: f32,
    pub model: String,
    pub provider: GenerationProvider,
    pub seed: u64,
}

impl Notes {
    pub fn from_outcome(outcome: &generate::Outcome, index: usize) -> Self {
        Self {
            prompt: outcome.prompt.clone(),
            negative: outcome.negative.clone(),
            steps: outcome.steps,
            cfg: outcome.cfg,
            model: outcome.model.clone(),
            provider: outcome.provider,
            seed: outcome.seeds.get(index).copied().unwrap_or(0),
        }
    }
}

/// The enlarged picture, and why the detail pass did not run.
pub struct Enlarged {
    pub image: Image,
    /// Empty when the detail pass ran or was not asked for.
    /// `needsGenerator`, `noPrompt` or `refineFailed` otherwise.
    pub notice: &'static str,
}

/// Whether the chosen upscaler is the catalogue model.
pub fn uses_builtin(settings: &AiSettings) -> bool {
    let id = settings.generation.upscaler.trim();
    id.is_empty() || id == BUILTIN
}

/// The steps of one enlargement. A ×4 with detail is painted at ×2, then
/// enlarged once more without painting: a generator pass at ×4 is too large
/// for most video cards.
pub fn passes(factor: u32, refine: bool) -> Vec<Pass> {
    let factor = if factor >= 4 { 4 } else { 2 };
    if !refine {
        return vec![Pass::Enlarge(factor)];
    }
    if factor == 4 {
        vec![Pass::Enlarge(2), Pass::Refine, Pass::Enlarge(2)]
    } else {
        vec![Pass::Enlarge(2), Pass::Refine]
    }
}

/// Whether the detail pass can run, and the notice when it cannot.
pub fn refine_decision(
    asked: bool,
    provider: GenerationProvider,
    prompt: &str,
) -> (bool, &'static str) {
    if !asked {
        return (false, "");
    }
    if prompt.trim().is_empty() {
        return (false, "noPrompt");
    }
    match provider {
        GenerationProvider::Comfy | GenerationProvider::Forge => (true, ""),
        GenerationProvider::None | GenerationProvider::SdCpp | GenerationProvider::Cloud => {
            (false, "needsGenerator")
        }
    }
}

/// Keeps the enlarged picture when the detail pass fails for any reason other
/// than the user stopping it.
pub fn keep_enlarged(refine: Result<Image>, enlarged: Image) -> Result<(Image, &'static str)> {
    match refine {
        Ok(image) => Ok((image, "")),
        Err(error) if error.code == "cancelled" => Err(error),
        Err(_) => Ok((enlarged, "refineFailed")),
    }
}

/// ComfyUI graph: load the model, enlarge, then scale to the exact size.
pub fn comfy_upscale_graph(model: &str, upload: &str, width: u32, height: u32) -> Value {
    json!({
        "1": { "class_type": "LoadImage", "inputs": { "image": upload } },
        "2": { "class_type": "UpscaleModelLoader", "inputs": { "model_name": model } },
        "3": { "class_type": "ImageUpscaleWithModel", "inputs": {
            "upscale_model": ["2", 0], "image": ["1", 0],
        } },
        "4": { "class_type": "ImageScale", "inputs": {
            "image": ["3", 0], "upscale_method": "lanczos",
            "width": width, "height": height, "crop": "disabled",
        } },
        "5": { "class_type": "SaveImage", "inputs": { "filename_prefix": "PixelIt", "images": ["4", 0] } },
    })
}

/// ComfyUI graph that repaints an uploaded picture. Large pictures use the
/// tiled encoder and decoder.
pub fn comfy_refine_graph(
    prompt: &str,
    negative: &str,
    checkpoint: &str,
    upload: &str,
    steps: u32,
    cfg: f32,
    seed: u64,
    strength: f32,
    tiled: bool,
) -> Value {
    let encode = if tiled { "VAEEncodeTiled" } else { "VAEEncode" };
    let decode = if tiled { "VAEDecodeTiled" } else { "VAEDecode" };
    let mut encode_inputs = json!({ "pixels": ["8", 0], "vae": ["1", 2] });
    let mut decode_inputs = json!({ "samples": ["5", 0], "vae": ["1", 2] });
    if tiled {
        encode_inputs["tile_size"] = json!(512);
        encode_inputs["overlap"] = json!(64);
        decode_inputs["tile_size"] = json!(512);
        decode_inputs["overlap"] = json!(64);
    }
    json!({
        "1": { "class_type": "CheckpointLoaderSimple", "inputs": { "ckpt_name": checkpoint } },
        "2": { "class_type": "CLIPTextEncode", "inputs": { "text": prompt, "clip": ["1", 1] } },
        "3": { "class_type": "CLIPTextEncode", "inputs": { "text": negative, "clip": ["1", 1] } },
        "8": { "class_type": "LoadImage", "inputs": { "image": upload } },
        "4": { "class_type": encode, "inputs": encode_inputs },
        "5": { "class_type": "KSampler", "inputs": {
            "seed": seed, "steps": steps, "cfg": cfg,
            "sampler_name": "euler", "scheduler": "normal", "denoise": strength,
            "model": ["1", 0], "positive": ["2", 0], "negative": ["3", 0],
            "latent_image": ["4", 0],
        } },
        "6": { "class_type": decode, "inputs": decode_inputs },
        "7": { "class_type": "SaveImage", "inputs": { "filename_prefix": "PixelIt", "images": ["6", 0] } },
    })
}

/// The body of Forge's single-image upscale, without the picture itself.
pub fn forge_body(model: &str, factor: u32) -> Value {
    json!({
        "resize_mode": 0,
        "upscaling_resize": factor,
        "upscaler_1": model,
        "upscaler_2": "None",
        "extras_upscaler_2_visibility": 0,
        "gfpgan_visibility": 0,
        "codeformer_visibility": 0,
        "codeformer_weight": 0,
    })
}

/// Models the studio can offer: the built-in one, ONNX files, whatever ComfyUI
/// and Forge report, and the remaining folder files shown as unavailable.
pub fn list(net: &Net, paths: &Paths, settings: &AiSettings) -> Vec<Upscaler> {
    let files = folder_files(&settings.apps.models);
    let comfy = remote_comfy(net, settings);
    let forge = remote_forge(net, settings);
    assemble(
        install::is_installed(paths, BUILTIN_ID),
        &files,
        &comfy,
        &forge,
    )
}

/// The prompt stored with a picture: the history entry, or the PNG text.
pub fn notes_of_file(paths: &Paths, file: &Path) -> Notes {
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if let Some(record) = history::list(paths)
        .into_iter()
        .find(|record| record.pictures.iter().any(|picture| picture.file == name))
    {
        let seed = record
            .pictures
            .iter()
            .find(|picture| picture.file == name)
            .map_or(0, |picture| picture.seed);
        let prompt = if record.final_prompt.is_empty() {
            record.prompt
        } else {
            record.final_prompt
        };
        let negative = if record.final_negative.is_empty() {
            record.negative
        } else {
            record.final_negative
        };
        return Notes {
            prompt,
            negative,
            steps: record.steps,
            cfg: record.cfg,
            model: record.model,
            provider: record.provider,
            seed,
        };
    }
    if let Some(read) = fs::read(file).ok().and_then(|bytes| {
        pixelit_codecs::png_text(&bytes, "parameters").and_then(|text| parameters::read(&text))
    }) {
        return Notes {
            prompt: read.prompt,
            negative: read.negative,
            steps: read.steps,
            cfg: read.cfg,
            model: read.model,
            provider: GenerationProvider::None,
            seed: read.seed,
        };
    }
    Notes {
        prompt: String::new(),
        negative: String::new(),
        steps: 0,
        cfg: 0.0,
        model: String::new(),
        provider: GenerationProvider::None,
        seed: 0,
    }
}

/// Enlarges `image` by 2 or 4, with the detail pass when the settings ask and
/// the generator can do it.
pub fn enlarge(
    onnx: &Onnx,
    net: &Net,
    paths: &Paths,
    settings: &AiSettings,
    image: &Image,
    factor: u32,
    notes: &Notes,
    index: u32,
    count: u32,
    cancel: &CancelToken,
    tick: &Tick<'_>,
) -> Result<Enlarged> {
    let (refine, mut notice) = refine_decision(
        settings.generation.upscale_refine,
        settings.generation.provider,
        &notes.prompt,
    );
    let mut current = image.clone();
    for pass in passes(factor, refine) {
        cancel.check()?;
        match pass {
            Pass::Enlarge(times) => {
                current = scale_once(
                    onnx, net, settings, &current, times, index, count, cancel, tick,
                )?;
            }
            Pass::Refine => {
                let refined = repaint(
                    net, paths, settings, &current, notes, index, count, cancel, tick,
                );
                let (next, why) = keep_enlarged(refined, current)?;
                if notice.is_empty() {
                    notice = why;
                }
                current = next;
            }
        }
    }
    Ok(Enlarged {
        image: current,
        notice,
    })
}

fn scale_once(
    onnx: &Onnx,
    net: &Net,
    settings: &AiSettings,
    image: &Image,
    factor: u32,
    index: u32,
    count: u32,
    cancel: &CancelToken,
    tick: &Tick<'_>,
) -> Result<Image> {
    let id = settings.generation.upscaler.trim();
    if id.is_empty() || id == BUILTIN {
        return onnx_builtin(onnx, image, factor, cancel);
    }
    if let Some(path) = id.strip_prefix("onnx:") {
        let path = Path::new(path);
        if !path.is_file() {
            return Err(missing(path));
        }
        return onnx.upscale_file(image, path, factor, cancel);
    }
    if let Some(model) = id.strip_prefix("comfy:") {
        return comfy_scale(
            net, settings, image, model, factor, index, count, cancel, tick,
        );
    }
    if let Some(model) = id.strip_prefix("forge:") {
        return forge_scale(
            net, settings, image, model, factor, index, count, cancel, tick,
        );
    }
    Err(Error::new("upscalerMissing").with("model", id))
}

fn missing(path: &Path) -> Error {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    Error::new("upscalerMissing").with("model", name)
}

fn onnx_builtin(onnx: &Onnx, image: &Image, factor: u32, cancel: &CancelToken) -> Result<Image> {
    let entry = crate::catalog::of_kind(crate::catalog::Kind::Upscale)
        .find(|entry| entry.id == BUILTIN_ID)
        .ok_or_else(|| Error::new("modelMissing").with("model", "upscale"))?;
    onnx.upscale(image, entry, factor, cancel)
}

fn comfy_scale(
    net: &Net,
    settings: &AiSettings,
    image: &Image,
    model: &str,
    factor: u32,
    index: u32,
    count: u32,
    cancel: &CancelToken,
    tick: &Tick<'_>,
) -> Result<Image> {
    let base = settings.generation.comfy_url.as_str();
    let key = generate::access(settings, base)?;
    let upload = generate::upload_png(net, base, key.as_deref(), &generate::opaque_png(image)?)?;
    let graph = comfy_upscale_graph(
        model,
        &upload,
        image.width().saturating_mul(factor),
        image.height().saturating_mul(factor),
    );
    generate::comfy_once(net, settings, &graph, cancel, tick, index, count, 1)
}

fn forge_scale(
    net: &Net,
    settings: &AiSettings,
    image: &Image,
    model: &str,
    factor: u32,
    index: u32,
    count: u32,
    cancel: &CancelToken,
    tick: &Tick<'_>,
) -> Result<Image> {
    let base = settings.generation.forge_url.as_str();
    let key = generate::access(settings, base)?;
    let mut body = forge_body(model, factor);
    body["image"] = json!(STANDARD.encode(generate::opaque_png(image)?));
    let answer = post_forge(
        net,
        base,
        key.as_deref(),
        &format!("{base}/sdapi/v1/extra-single-image"),
        &body,
        cancel,
        tick,
        index,
        count,
    )?;
    let text = answer["image"].as_str().unwrap_or("");
    let text = text.rsplit(',').next().unwrap_or(text);
    generate::decode_image(&STANDARD.decode(text).map_err(failed)?)
}

fn repaint(
    net: &Net,
    paths: &Paths,
    settings: &AiSettings,
    image: &Image,
    notes: &Notes,
    index: u32,
    count: u32,
    cancel: &CancelToken,
    tick: &Tick<'_>,
) -> Result<Image> {
    match settings.generation.provider {
        GenerationProvider::Comfy => {
            comfy_repaint(net, settings, image, notes, index, count, cancel, tick)
        }
        GenerationProvider::Forge => {
            forge_repaint(net, paths, settings, image, notes, cancel, tick)
        }
        GenerationProvider::None | GenerationProvider::SdCpp | GenerationProvider::Cloud => {
            Err(Error::new("providerNotSet"))
        }
    }
}

fn comfy_repaint(
    net: &Net,
    settings: &AiSettings,
    image: &Image,
    notes: &Notes,
    index: u32,
    count: u32,
    cancel: &CancelToken,
    tick: &Tick<'_>,
) -> Result<Image> {
    let base = settings.generation.comfy_url.as_str();
    let key = generate::access(settings, base)?;
    let wanted = if notes.provider == GenerationProvider::Comfy && !notes.model.trim().is_empty() {
        notes.model.as_str()
    } else {
        settings.generation.comfy_checkpoint.as_str()
    };
    let checkpoint = generate::comfy_checkpoint(net, base, key.as_deref(), wanted)?;
    let upload = generate::upload_png(net, base, key.as_deref(), &generate::opaque_png(image)?)?;
    let steps = if notes.steps == 0 {
        settings.generation.steps
    } else {
        notes.steps
    };
    let cfg = if notes.cfg > 0.0 {
        notes.cfg
    } else {
        settings.generation.cfg
    };
    let graph = comfy_refine_graph(
        &notes.prompt,
        &notes.negative,
        &checkpoint,
        &upload,
        steps,
        cfg,
        notes.seed,
        settings.generation.upscale_strength,
        image.width().max(image.height()) > TILED_SIDE,
    );
    generate::comfy_once(net, settings, &graph, cancel, tick, index, count, steps)
}

fn forge_repaint(
    net: &Net,
    paths: &Paths,
    settings: &AiSettings,
    image: &Image,
    notes: &Notes,
    cancel: &CancelToken,
    tick: &Tick<'_>,
) -> Result<Image> {
    let model = if notes.provider == GenerationProvider::Forge {
        notes.model.clone()
    } else {
        String::new()
    };
    let request = GenerateRequest {
        prompt: notes.prompt.clone(),
        negative: notes.negative.clone(),
        width: image.width(),
        height: image.height(),
        steps: notes.steps,
        cfg: notes.cfg,
        seed: Some(notes.seed),
        count: 1,
        strength: settings.generation.upscale_strength,
        provider: GenerationProvider::Forge,
        model,
        styles: Vec::new(),
    };
    let outcome = generate::generate(
        net,
        paths,
        settings,
        &request,
        Some(&Source { image, mask: None }),
        cancel,
        tick,
    )?;
    outcome
        .images
        .into_iter()
        .next()
        .ok_or_else(|| failed("the server returned no pictures"))
}

fn post_forge(
    net: &Net,
    base: &str,
    key: Option<&str>,
    url: &str,
    body: &Value,
    cancel: &CancelToken,
    tick: &Tick<'_>,
    index: u32,
    count: u32,
) -> Result<Value> {
    tick(index, count, 0, 1, None);
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        scope.spawn(|| {
            let _ = sender.send(net.post_json(url, key, body, Some(Duration::from_mins(30))));
        });
        loop {
            match receiver.recv_timeout(Duration::from_millis(300)) {
                Ok(answer) => break answer,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Ok(progress) =
                        net.get_json(&format!("{base}/sdapi/v1/progress"), key, Some(ASK))
                    {
                        if let Some((step, total, _)) = crate::progress::forge_progress(&progress) {
                            tick(index, count, step, total, None);
                        }
                    }
                    if cancel.is_cancelled() {
                        let _ = net.post(
                            &format!("{base}/sdapi/v1/interrupt"),
                            key,
                            &json!({}),
                            Some(ASK),
                        );
                        let _ = receiver.recv();
                        break Err(Error::cancelled());
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Err(failed("request thread stopped"))
                }
            }
        }
    })
}

fn remote_comfy(net: &Net, settings: &AiSettings) -> Vec<String> {
    let base = settings.generation.comfy_url.trim();
    if base.is_empty() {
        return Vec::new();
    }
    let Ok(key) = generate::access(settings, base) else {
        return Vec::new();
    };
    let Ok(answer) = net.get_json(
        &format!("{base}/object_info/UpscaleModelLoader"),
        key.as_deref(),
        Some(ASK),
    ) else {
        return Vec::new();
    };
    names_of(&answer["UpscaleModelLoader"]["input"]["required"]["model_name"][0])
}

fn remote_forge(net: &Net, settings: &AiSettings) -> Vec<String> {
    let base = settings.generation.forge_url.trim();
    if base.is_empty() {
        return Vec::new();
    }
    let Ok(key) = generate::access(settings, base) else {
        return Vec::new();
    };
    let Ok(answer) = net.get_json(
        &format!("{base}/sdapi/v1/upscalers"),
        key.as_deref(),
        Some(ASK),
    ) else {
        return Vec::new();
    };
    answer
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| model["name"].as_str())
        .filter(|name| !plain(name))
        .map(str::to_owned)
        .collect()
}

fn names_of(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

fn plain(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    name.is_empty() || PLAIN.contains(&name.as_str())
}

fn folder_files(models: &ModelLibrary) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for folder in &models.upscale {
        let Ok(entries) = fs::read_dir(folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let extension = path
                .extension()
                .map_or_default(|ext| ext.to_string_lossy().to_ascii_lowercase());
            if WEIGHTS.contains(&extension.as_str()) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_default(|name| name.to_string_lossy().into_owned())
}

fn assemble(
    builtin_ready: bool,
    files: &[PathBuf],
    comfy: &[String],
    forge: &[String],
) -> Vec<Upscaler> {
    let mut listed = Vec::new();
    listed.push(Upscaler {
        id: BUILTIN.to_owned(),
        name: BUILTIN_NAME.to_owned(),
        engine: Engine::Builtin,
        available: builtin_ready,
    });
    let onnx_here = cfg!(windows);
    let mut onnx_names = HashSet::new();
    let mut used = HashSet::new();
    for path in files {
        let extension = path
            .extension()
            .map_or_default(|ext| ext.to_string_lossy().to_ascii_lowercase());
        if extension != "onnx" {
            continue;
        }
        let name = file_name(path);
        onnx_names.insert(name.to_ascii_lowercase());
        let label = unique_label(path, &name, &mut used);
        listed.push(Upscaler {
            id: format!("onnx:{}", path.display()),
            name: label,
            engine: Engine::Onnx,
            available: onnx_here,
        });
    }
    let claimed = push_remote(&mut listed, comfy, &onnx_names, Engine::Comfy, "comfy");
    let claimed = push_remote(&mut listed, forge, &claimed, Engine::Forge, "forge");
    for path in files {
        let extension = path
            .extension()
            .map_or_default(|ext| ext.to_string_lossy().to_ascii_lowercase());
        if extension == "onnx" {
            continue;
        }
        let name = file_name(path);
        if claimed.contains(&name.to_ascii_lowercase())
            || onnx_names.contains(&name.to_ascii_lowercase())
        {
            continue;
        }
        let label = unique_label(path, &name, &mut used);
        listed.push(Upscaler {
            id: format!("file:{}", path.display()),
            name: label,
            engine: Engine::File,
            available: false,
        });
    }
    listed
}

/// Adds server models and returns the set of file names now spoken for.
fn push_remote(
    listed: &mut Vec<Upscaler>,
    models: &[String],
    skip: &HashSet<String>,
    engine: Engine,
    prefix: &str,
) -> HashSet<String> {
    let mut claimed = skip.clone();
    let mut models = models.to_vec();
    models.sort_by_key(|name| name.to_ascii_lowercase());
    for name in models {
        let key = name.to_ascii_lowercase();
        if !claimed.insert(key) {
            continue;
        }
        listed.push(Upscaler {
            id: format!("{prefix}:{name}"),
            name,
            engine,
            available: true,
        });
    }
    claimed
}

fn unique_label(path: &Path, name: &str, used: &mut HashSet<String>) -> String {
    if used.insert(name.to_ascii_lowercase()) {
        return name.to_owned();
    }
    let parent = path
        .parent()
        .and_then(|folder| folder.file_name())
        .map_or_default(|folder| folder.to_string_lossy().into_owned());
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{name} ({parent})")
    }
}

fn failed(detail: impl std::fmt::Display) -> Error {
    Error::new("generationFailed").detail(detail)
}

/// The parameters text of an enlarged picture, so a later pass can read it.
pub fn parameters_of(notes: &Notes, image: &Image) -> String {
    parameters::write(&Parameters {
        prompt: notes.prompt.clone(),
        negative: notes.negative.clone(),
        steps: notes.steps,
        cfg: notes.cfg,
        seed: notes.seed,
        width: image.width(),
        height: image.height(),
        model: notes.model.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_times_with_detail_is_painted_at_double_size() {
        assert_eq!(
            passes(4, true),
            vec![Pass::Enlarge(2), Pass::Refine, Pass::Enlarge(2)]
        );
        assert_eq!(passes(2, true), vec![Pass::Enlarge(2), Pass::Refine]);
        assert_eq!(passes(4, false), vec![Pass::Enlarge(4)]);
        assert_eq!(passes(9, false), vec![Pass::Enlarge(4)]);
    }

    #[test]
    fn detail_needs_a_local_generator_and_a_prompt() {
        assert_eq!(
            refine_decision(true, GenerationProvider::Comfy, "a cat"),
            (true, "")
        );
        assert_eq!(
            refine_decision(true, GenerationProvider::Forge, "a cat"),
            (true, "")
        );
        assert_eq!(
            refine_decision(true, GenerationProvider::SdCpp, "a cat"),
            (false, "needsGenerator")
        );
        assert_eq!(
            refine_decision(true, GenerationProvider::Cloud, "a cat"),
            (false, "needsGenerator")
        );
        assert_eq!(
            refine_decision(true, GenerationProvider::None, "a cat"),
            (false, "needsGenerator")
        );
        assert_eq!(
            refine_decision(true, GenerationProvider::Comfy, "  "),
            (false, "noPrompt")
        );
        assert_eq!(
            refine_decision(false, GenerationProvider::Comfy, "a cat"),
            (false, "")
        );
    }

    #[test]
    fn a_failed_refine_keeps_the_enlarged_picture() {
        let enlarged = Image::from_rgba8(1, 1, vec![1, 2, 3, 255]).unwrap();
        let (kept, notice) =
            keep_enlarged(Err(Error::new("generationFailed")), enlarged.clone()).unwrap();
        assert_eq!(notice, "refineFailed");
        assert_eq!(kept.width(), enlarged.width());
        let cancelled = keep_enlarged(Err(Error::cancelled()), enlarged);
        assert_eq!(cancelled.unwrap_err().code, "cancelled");
    }

    #[test]
    fn the_comfy_graph_scales_to_the_asked_size() {
        let graph = comfy_upscale_graph("4x.pt", "in.png", 1024, 768);
        assert_eq!(graph["2"]["inputs"]["model_name"], "4x.pt");
        assert_eq!(graph["3"]["class_type"], "ImageUpscaleWithModel");
        assert_eq!(graph["4"]["inputs"]["width"], 1024);
        assert_eq!(graph["4"]["inputs"]["height"], 768);
        assert_eq!(graph["5"]["inputs"]["images"][0], "4");
    }

    #[test]
    fn a_large_picture_uses_the_tiled_nodes() {
        let small = comfy_refine_graph(
            "cat",
            "",
            "model.safetensors",
            "in.png",
            20,
            6.0,
            1,
            0.35,
            false,
        );
        assert_eq!(small["4"]["class_type"], "VAEEncode");
        assert_eq!(small["6"]["class_type"], "VAEDecode");
        assert!(small["4"]["inputs"].get("tile_size").is_none());
        let denoise = small["5"]["inputs"]["denoise"].as_f64().unwrap();
        assert!((denoise - 0.35).abs() < 0.001);
        let large = comfy_refine_graph(
            "cat",
            "",
            "model.safetensors",
            "in.png",
            20,
            6.0,
            1,
            0.35,
            true,
        );
        assert_eq!(large["4"]["class_type"], "VAEEncodeTiled");
        assert_eq!(large["6"]["class_type"], "VAEDecodeTiled");
        assert_eq!(large["4"]["inputs"]["tile_size"], 512);
        assert_eq!(large["6"]["inputs"]["overlap"], 64);
    }

    #[test]
    fn the_forge_request_names_the_model_and_the_factor() {
        let body = forge_body("4x-UltraSharp", 2);
        assert_eq!(body["upscaler_1"], "4x-UltraSharp");
        assert_eq!(body["upscaling_resize"], 2);
        assert_eq!(body["resize_mode"], 0);
        assert!(body.get("image").is_none());
    }

    #[test]
    fn folder_files_a_server_already_has_are_not_listed_twice() {
        let folder = tempfile::tempdir().unwrap();
        let onnx = folder.path().join("sharp.onnx");
        let pt = folder.path().join("4x.pt");
        let other = folder.path().join("slow.pth");
        std::fs::write(&onnx, b"x").unwrap();
        std::fs::write(&pt, b"x").unwrap();
        std::fs::write(&other, b"x").unwrap();
        let listed = assemble(
            true,
            &[onnx.clone(), pt, other],
            &["4x.pt".to_owned(), "sharp.onnx".to_owned()],
            &[],
        );
        let ids: Vec<&str> = listed.iter().map(|item| item.id.as_str()).collect();
        assert!(ids.iter().any(|id| id.starts_with("onnx:")));
        assert!(ids.contains(&"comfy:4x.pt"));
        assert!(!ids.iter().any(|id| id.starts_with("comfy:sharp")));
        assert!(listed
            .iter()
            .any(|item| item.engine == Engine::File && !item.available));
        assert!(listed
            .iter()
            .any(|item| item.id == BUILTIN && item.available));
    }

    #[test]
    fn plain_resizers_are_not_models() {
        assert!(plain("Lanczos"));
        assert!(plain("None"));
        assert!(!plain("4x-UltraSharp"));
    }
}
