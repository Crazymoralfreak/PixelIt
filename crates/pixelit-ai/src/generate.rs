//! Image generation through the provider the user chose: ComfyUI, Forge or
//! AUTOMATIC1111, stable-diffusion.cpp on this computer, or a cloud API.

use std::fs;
#[cfg(windows)]
use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use pixelit_codecs::{decode_bytes, encode, DecodeOptions, EncodeOptions};
use pixelit_core::pipeline::{Mask, ResizeFilter};
use pixelit_core::{ops, CancelToken, Error, Image, ImageFormat, Result};
use pixelit_engine::Paths;

use crate::catalog::Kind;
use crate::checkpoints::chosen_model;
use crate::config::{AiSettings, GenerationProvider};
use crate::install;
use crate::net::{self, Net, Part, SocketMessage};
use crate::progress::{self, comfy_preview, comfy_step, forge_progress, sampler_step};
use crate::secrets;
use crate::styles;

/// Keeps stable-diffusion.cpp from opening a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Longest wait for one generation.
const GENERATION_TIMEOUT: Duration = Duration::from_mins(30);

const SHORT_TIMEOUT: Duration = Duration::from_secs(20);

/// Most pictures of one request.
const MAX_COUNT: u32 = 8;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GenerateRequest {
    pub prompt: String,
    pub negative: String,
    /// Zero takes the size from the settings.
    pub width: u32,
    pub height: u32,
    pub steps: u32,
    pub cfg: f32,
    /// `None` picks a random seed.
    pub seed: Option<u64>,
    pub count: u32,
    /// How far the result may move from the source picture, from 0 to 1.
    pub strength: f32,
    /// `None` uses the generator of the settings.
    pub provider: GenerationProvider,
    /// Empty uses the model of the settings.
    pub model: String,
    /// Styles to apply, in order. Empty applies none.
    pub styles: Vec<String>,
}

/// What a generation used and the pictures it returned.
#[derive(Debug)]
pub struct Outcome {
    pub images: Vec<Image>,
    pub seeds: Vec<u64>,
    pub provider: GenerationProvider,
    pub model: String,
    pub prompt: String,
    pub negative: String,
    pub width: u32,
    pub height: u32,
    pub steps: u32,
    pub cfg: f32,
}

/// A moment of a generation: picture index, picture count, sampler step,
/// sampler steps, and an encoded preview when the provider sent one.
pub type Tick<'a> = dyn Fn(u32, u32, u32, u32, Option<&[u8]>) + Sync + 'a;

/// A tick that ignores the progress.
pub fn quiet(_index: u32, _count: u32, _step: u32, _steps: u32, _preview: Option<&[u8]>) {}

impl Default for GenerateRequest {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            negative: String::new(),
            width: 0,
            height: 0,
            steps: 0,
            cfg: 0.0,
            seed: None,
            count: 1,
            strength: 0.6,
            provider: GenerationProvider::None,
            model: String::new(),
            styles: Vec::new(),
        }
    }
}

/// A picture to change, with an optional area to repaint.
#[derive(Clone, Copy)]
pub struct Source<'a> {
    pub image: &'a Image,
    pub mask: Option<&'a Mask>,
}

#[derive(Clone, Copy)]
struct Job<'a> {
    prompt: &'a str,
    negative: &'a str,
    width: u32,
    height: u32,
    steps: u32,
    cfg: f32,
    seed: u64,
    count: u32,
    strength: f32,
    checkpoint: &'a str,
}

pub fn decode_image(bytes: &[u8]) -> Result<Image> {
    decode_bytes(bytes, None, &DecodeOptions::default(), &CancelToken::new())?
        .media
        .into_first()
        .ok_or_else(|| Error::new("badImage"))
}

pub fn png(image: &Image) -> Result<Vec<u8>> {
    encode(
        image,
        &EncodeOptions::new(ImageFormat::Png),
        &CancelToken::new(),
    )
}

/// The key stored for the server of `url`. Servers outside this computer are
/// refused until the user allows them in the settings.
pub fn access(settings: &AiSettings, url: &str) -> Result<Option<String>> {
    let host = net::host_of(url).ok_or_else(|| Error::new("badUrl").with("url", url))?;
    if !net::is_local(url) && !settings.allow_cloud {
        return Err(Error::new("cloudDisabled").with("host", host));
    }
    Ok(secrets::get(&host))
}

/// The source as PNG whose transparent pixels mark the area to repaint.
fn png_with_holes(source: &Source<'_>) -> Result<Vec<u8>> {
    let mut rgba = source.image.to_rgba8();
    if let Some(mask) = source.mask {
        for (pixel, cover) in rgba.chunks_exact_mut(4).zip(&mask.data) {
            pixel[3] = 255 - *cover;
        }
    } else {
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
    }
    png(&Image::from_rgba8(
        source.image.width(),
        source.image.height(),
        rgba,
    )?)
}

/// The mask as a white-on-black PNG.
fn mask_png(mask: &Mask) -> Result<Vec<u8>> {
    let mut rgba = Vec::with_capacity(mask.data.len() * 4);
    for value in &mask.data {
        rgba.extend_from_slice(&[*value, *value, *value, 255]);
    }
    let mut image = Image::from_rgba8(mask.width, mask.height, rgba)?;
    image.set_has_alpha(false);
    png(&image)
}

fn opaque_png(image: &Image) -> Result<Vec<u8>> {
    let mut flat = image.clone();
    flat.flatten(pixelit_core::Color::WHITE);
    png(&flat)
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    out
}

/// Digits of a percent-encoded byte.
const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn failed(detail: impl std::fmt::Display) -> Error {
    Error::new("generationFailed").detail(detail)
}

// ------------------------------------------------------------------ ComfyUI

fn comfy_graph(job: &Job<'_>, checkpoint: &str, upload: Option<&str>, masked: bool) -> Value {
    let mut graph = json!({
        "1": { "class_type": "CheckpointLoaderSimple", "inputs": { "ckpt_name": checkpoint } },
        "2": { "class_type": "CLIPTextEncode", "inputs": { "text": job.prompt, "clip": ["1", 1] } },
        "3": { "class_type": "CLIPTextEncode", "inputs": { "text": job.negative, "clip": ["1", 1] } },
        "5": { "class_type": "KSampler", "inputs": {
            "seed": job.seed, "steps": job.steps, "cfg": job.cfg,
            "sampler_name": "euler", "scheduler": "normal",
            "denoise": if upload.is_some() { job.strength } else { 1.0 },
            "model": ["1", 0], "positive": ["2", 0], "negative": ["3", 0],
            "latent_image": [if masked { "9" } else { "4" }, 0],
        } },
        "6": { "class_type": "VAEDecode", "inputs": { "samples": ["5", 0], "vae": ["1", 2] } },
        "7": { "class_type": "SaveImage", "inputs": { "filename_prefix": "PixelIt", "images": ["6", 0] } },
    });
    match upload {
        Some(name) => {
            graph["8"] = json!({ "class_type": "LoadImage", "inputs": { "image": name } });
            graph["4"] = json!({ "class_type": "VAEEncode", "inputs": { "pixels": ["8", 0], "vae": ["1", 2] } });
            if masked {
                graph["9"] = json!({ "class_type": "SetLatentNoiseMask", "inputs": { "samples": ["4", 0], "mask": ["8", 1] } });
            }
        }
        None => {
            graph["4"] = json!({ "class_type": "EmptyLatentImage", "inputs": {
                "width": job.width, "height": job.height, "batch_size": 1,
            } });
        }
    }
    graph
}

/// Fills the placeholders of a user workflow.
fn fill_workflow(template: &str, job: &Job<'_>, upload: Option<&str>) -> Result<Value> {
    // Text goes in as the inside of a JSON string, so quotes stay valid.
    let quoted = |text: &str| {
        let json = Value::String(text.to_owned()).to_string();
        json[1..json.len() - 1].to_owned()
    };
    let filled = template
        .replace("%prompt%", &quoted(job.prompt))
        .replace("%negative%", &quoted(job.negative))
        .replace("%image%", &quoted(upload.unwrap_or_default()))
        .replace("%seed%", &job.seed.to_string())
        .replace("%width%", &job.width.to_string())
        .replace("%height%", &job.height.to_string())
        .replace("%steps%", &job.steps.to_string())
        .replace("%cfg%", &job.cfg.to_string())
        .replace("%checkpoint%", &quoted(job.checkpoint));
    serde_json::from_str(&filled).map_err(|error| Error::new("badWorkflow").detail(error))
}

fn comfy_checkpoint(net: &Net, base: &str, key: Option<&str>, wanted: &str) -> Result<String> {
    if !wanted.trim().is_empty() {
        return Ok(wanted.trim().to_owned());
    }
    let info = net.get_json(
        &format!("{base}/object_info/CheckpointLoaderSimple"),
        key,
        Some(SHORT_TIMEOUT),
    )?;
    info["CheckpointLoaderSimple"]["input"]["required"]["ckpt_name"][0][0]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::new("noCheckpoint"))
}

fn watch_comfy(
    net: &Net,
    base: &str,
    key: Option<&str>,
    client: &str,
    picture: &AtomicU32,
    count: u32,
    stop: &AtomicBool,
    cancel: &CancelToken,
    tick: &Tick<'_>,
) {
    let Ok(mut socket) = net.connect_socket(&net::websocket_of(base, client), key) else {
        return;
    };
    while !stop.load(Ordering::Relaxed) && !cancel.is_cancelled() {
        let message = match socket.recv() {
            Ok(Some(message)) => message,
            Ok(None) => continue,
            Err(_) => break,
        };
        let index = picture.load(Ordering::Relaxed);
        match message {
            SocketMessage::Text(text) => {
                let id = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|value| value["data"]["prompt_id"].as_str().map(str::to_owned))
                    .unwrap_or_default();
                if let Some((step, total)) = comfy_step(&text, &id) {
                    tick(index, count, step, total, None);
                }
            }
            SocketMessage::Binary(bytes) => {
                if let Some(preview) = comfy_preview(&bytes) {
                    tick(index, count, 0, 0, Some(preview));
                }
            }
        }
    }
}

fn comfy(
    net: &Net,
    settings: &AiSettings,
    job: &Job<'_>,
    source: Option<&Source<'_>>,
    cancel: &CancelToken,
    tick: &Tick<'_>,
) -> Result<Vec<Image>> {
    let base = settings.generation.comfy_url.as_str();
    let key = access(settings, base)?;
    let key = key.as_deref();
    let upload = match source {
        Some(source) => {
            let bytes = png_with_holes(source)?;
            let name = format!("pixelit-{:08x}.png", fastrand::u32(..));
            let answer = net.post_form(
                &format!("{base}/upload/image"),
                key,
                &[
                    Part::file("image", &name, "image/png", &bytes),
                    Part::field("overwrite", "true"),
                ],
                Some(Duration::from_secs(120)),
            )?;
            let stored = answer["name"].as_str().unwrap_or(&name).to_owned();
            Some(match answer["subfolder"].as_str() {
                Some(folder) if !folder.is_empty() => format!("{folder}/{stored}"),
                _ => stored,
            })
        }
        None => None,
    };
    let masked = source.is_some_and(|source| source.mask.is_some());
    let workflow = settings.generation.comfy_workflow.trim();
    let template = if workflow.is_empty() {
        None
    } else {
        Some(fs::read_to_string(workflow)?)
    };
    let checkpoint = if template
        .as_ref()
        .is_some_and(|text| !text.contains("%checkpoint%"))
    {
        String::new()
    } else {
        comfy_checkpoint(net, base, key, job.checkpoint)?
    };
    let client = format!("pixelit-{:016x}", fastrand::u64(..));
    let stop = AtomicBool::new(false);
    let picture = AtomicU32::new(0);
    let mut images = Vec::new();
    let result = thread::scope(|scope| {
        scope.spawn(|| {
            watch_comfy(
                net, base, key, &client, &picture, job.count, &stop, cancel, tick,
            );
        });
        let result = (|| -> Result<Vec<Image>> {
            for index in 0..job.count {
                picture.store(index, Ordering::Relaxed);
                tick(index, job.count, 0, job.steps, None);
                cancel.check()?;
                let job = Job {
                    seed: job.seed.wrapping_add(u64::from(index)),
                    ..*job
                };
                let graph = match &template {
                    Some(template) => fill_workflow(template, &job, upload.as_deref())?,
                    None => comfy_graph(&job, &checkpoint, upload.as_deref(), masked),
                };
                let queued = net.post_json(
                    &format!("{base}/prompt"),
                    key,
                    &json!({ "prompt": graph, "client_id": client }),
                    Some(SHORT_TIMEOUT),
                )?;
                let id = queued["prompt_id"]
                    .as_str()
                    .ok_or_else(|| failed("ComfyUI did not accept the workflow"))?
                    .to_owned();
                let started = Instant::now();
                let outputs = loop {
                    if cancel.is_cancelled() {
                        let _ = net.post(
                            &format!("{base}/interrupt"),
                            key,
                            &json!({}),
                            Some(SHORT_TIMEOUT),
                        );
                        return Err(Error::cancelled());
                    }
                    if started.elapsed() > GENERATION_TIMEOUT {
                        return Err(failed("timed out"));
                    }
                    thread::sleep(Duration::from_millis(700));
                    let history =
                        net.get_json(&format!("{base}/history/{id}"), key, Some(SHORT_TIMEOUT))?;
                    let entry = &history[id.as_str()];
                    if entry.is_null() {
                        continue;
                    }
                    if entry["status"]["status_str"].as_str() == Some("error") {
                        return Err(failed(entry["status"]["messages"].to_string()));
                    }
                    break entry["outputs"].clone();
                };
                for node in outputs.as_object().into_iter().flat_map(|map| map.values()) {
                    for file in node["images"].as_array().into_iter().flatten() {
                        let (Some(name), kind) = (file["filename"].as_str(), file["type"].as_str())
                        else {
                            continue;
                        };
                        let url = format!(
                            "{base}/view?filename={}&subfolder={}&type={}",
                            percent_encode(name),
                            percent_encode(file["subfolder"].as_str().unwrap_or_default()),
                            percent_encode(kind.unwrap_or("output")),
                        );
                        images.push(decode_image(&net.get(
                            &url,
                            key,
                            Some(Duration::from_secs(120)),
                        )?)?);
                    }
                }
            }
            if images.is_empty() {
                return Err(failed("the workflow saved no pictures"));
            }
            Ok(images)
        })();
        stop.store(true, Ordering::Relaxed);
        result
    })?;
    Ok(result)
}

// ----------------------------------------------------- Forge / AUTOMATIC1111

fn forge(
    net: &Net,
    settings: &AiSettings,
    job: &Job<'_>,
    source: Option<&Source<'_>>,
    cancel: &CancelToken,
    tick: &Tick<'_>,
) -> Result<Vec<Image>> {
    let base = settings.generation.forge_url.as_str();
    let key = access(settings, base)?;
    let mut body = json!({
        "prompt": job.prompt,
        "negative_prompt": job.negative,
        "width": job.width,
        "height": job.height,
        "steps": job.steps,
        "cfg_scale": job.cfg,
        "seed": job.seed,
        "batch_size": job.count,
    });
    let route = match source {
        Some(source) => {
            body["init_images"] = json!([STANDARD.encode(opaque_png(source.image)?)]);
            body["denoising_strength"] = json!(job.strength);
            body["width"] = json!(source.image.width());
            body["height"] = json!(source.image.height());
            if let Some(mask) = source.mask {
                body["mask"] = json!(STANDARD.encode(mask_png(mask)?));
                body["inpainting_fill"] = json!(1);
                body["inpaint_full_res"] = json!(true);
                body["inpaint_full_res_padding"] = json!(32);
                body["mask_blur"] = json!(4);
            }
            "img2img"
        }
        None => "txt2img",
    };
    apply_forge_model(&mut body, job.checkpoint);
    let url = format!("{base}/sdapi/v1/{route}");
    tick(0, job.count, 0, job.steps, None);
    let (sender, receiver) = mpsc::channel();
    let answer = thread::scope(|scope| {
        scope.spawn(|| {
            let _ =
                sender.send(net.post_json(&url, key.as_deref(), &body, Some(GENERATION_TIMEOUT)));
        });
        loop {
            match receiver.recv_timeout(Duration::from_millis(300)) {
                Ok(answer) => break answer,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Ok(progress) = net.get_json(
                        &format!("{base}/sdapi/v1/progress"),
                        key.as_deref(),
                        Some(SHORT_TIMEOUT),
                    ) {
                        if let Some((step, total, preview)) = forge_progress(&progress) {
                            let bytes = preview.and_then(|text| {
                                let text = text.rsplit(',').next().unwrap_or(text);
                                STANDARD.decode(text).ok()
                            });
                            tick(0, job.count, step, total, bytes.as_deref());
                        }
                    }
                    if cancel.is_cancelled() {
                        let _ = net.post(
                            &format!("{base}/sdapi/v1/interrupt"),
                            key.as_deref(),
                            &json!({}),
                            Some(SHORT_TIMEOUT),
                        );
                        // The interrupted request still answers; wait for it.
                        let _ = receiver.recv();
                        break Err(Error::cancelled());
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Err(failed("request thread stopped"))
                }
            }
        }
    })?;
    let mut images = Vec::new();
    for encoded in answer["images"].as_array().into_iter().flatten() {
        let text = encoded.as_str().unwrap_or_default();
        // Some versions prefix the data with a `data:` header.
        let text = text.rsplit(',').next().unwrap_or(text);
        let bytes = STANDARD.decode(text).map_err(failed)?;
        images.push(decode_image(&bytes)?);
    }
    // With several pictures the server may put a contact sheet first.
    if images.len() > job.count as usize {
        images.drain(..images.len() - job.count as usize);
    }
    if images.is_empty() {
        return Err(failed("the server returned no pictures"));
    }
    Ok(images)
}

// ------------------------------------------------------ stable-diffusion.cpp

fn sdcpp_program(paths: &Paths) -> Result<PathBuf> {
    install::installed_of(paths, Kind::Generator)
        .first()
        .ok_or_else(|| Error::new("componentMissing").with("id", "sdcpp-vulkan"))
        .and_then(|entry| install::main_path(paths, entry.id))
}

fn tail(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().rev().take(6).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join("\n")
}

fn sdcpp(
    paths: &Paths,
    settings: &AiSettings,
    job: &Job<'_>,
    source: Option<&Source<'_>>,
    cancel: &CancelToken,
    tick: &Tick<'_>,
) -> Result<Vec<Image>> {
    let program = sdcpp_program(paths)?;
    let model = job.checkpoint.trim();
    if model.is_empty() || !Path::new(model).is_file() {
        return Err(Error::new("noCheckpoint"));
    }
    let folder = paths.temp().join(format!("sd-{:08x}", fastrand::u32(..)));
    fs::create_dir_all(&folder)?;
    let result = (|| -> Result<Vec<Image>> {
        let mut extra: Vec<String> = Vec::new();
        if let Some(source) = source {
            let init = folder.join("init.png");
            fs::write(&init, opaque_png(source.image)?)?;
            extra.extend(["-i".to_owned(), init.to_string_lossy().into_owned()]);
            extra.extend(["--strength".to_owned(), job.strength.to_string()]);
            if let Some(mask) = source.mask {
                let mask_file = folder.join("mask.png");
                fs::write(&mask_file, mask_png(mask)?)?;
                extra.extend([
                    "--mask".to_owned(),
                    mask_file.to_string_lossy().into_owned(),
                ]);
            }
        }
        let mut images = Vec::new();
        for index in 0..job.count {
            cancel.check()?;
            let out = folder.join(format!("out-{index}.png"));
            let log = folder.join("log.txt");
            let mut command = Command::new(&program);
            command
                .arg("-m")
                .arg(model)
                .arg("-p")
                .arg(job.prompt)
                .arg("-W")
                .arg(job.width.to_string())
                .arg("-H")
                .arg(job.height.to_string())
                .arg("--steps")
                .arg(job.steps.to_string())
                .arg("--cfg-scale")
                .arg(job.cfg.to_string())
                .arg("-s")
                .arg(job.seed.wrapping_add(u64::from(index)).to_string())
                .arg("-o")
                .arg(&out)
                .args(&extra)
                .args(settings.generation.sdcpp_arguments.split_whitespace())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(fs::File::create(&log).map_or_else(|_| Stdio::null(), Stdio::from));
            if !job.negative.is_empty() {
                command.arg("-n").arg(job.negative);
            }
            if let Some(home) = program.parent() {
                command.current_dir(home);
            }
            #[cfg(windows)]
            command.creation_flags(CREATE_NO_WINDOW);
            let mut child = command.spawn().map_err(failed)?;
            let started = Instant::now();
            let status = loop {
                if let Some(status) = child.try_wait().map_err(failed)? {
                    break status;
                }
                if let Some((step, total)) = sampler_step(&tail(&log)) {
                    tick(index, job.count, step, total, None);
                }
                if cancel.is_cancelled() || started.elapsed() > GENERATION_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    cancel.check()?;
                    return Err(failed("timed out"));
                }
                thread::sleep(Duration::from_millis(200));
            };
            if !status.success() || !out.is_file() {
                return Err(failed(tail(&log)));
            }
            images.push(decode_image(&fs::read(&out)?)?);
        }
        Ok(images)
    })();
    let _ = fs::remove_dir_all(&folder);
    result
}

// -------------------------------------------------------------------- cloud

/// Longest side of a picture sent to a cloud service: larger ones travel
/// long and most services refuse them.
const CLOUD_SIDE: u32 = 2048;

/// Pixel counts every known image service accepts: from 1024 x 1024 to
/// 1536 x 1024.
const CLOUD_MIN_AREA: u64 = 1024 * 1024;
const CLOUD_MAX_AREA: u64 = 1536 * 1024;

/// A size of the same shape inside the accepted pixel counts, with sides
/// divisible by 16.
fn budget_size(width: u32, height: u32) -> (u32, u32) {
    let area = u64::from(width.max(1)) * u64::from(height.max(1));
    let scale = if area < CLOUD_MIN_AREA {
        (CLOUD_MIN_AREA as f64 / area as f64).sqrt()
    } else if area > CLOUD_MAX_AREA {
        (CLOUD_MAX_AREA as f64 / area as f64).sqrt()
    } else {
        1.0
    };
    let snap = |side: u32| ((f64::from(side) * scale / 16.0).round().max(1.0) as u32) * 16;
    let (mut width, mut height) = (snap(width), snap(height));
    let area = |width: u32, height: u32| u64::from(width) * u64::from(height);
    while area(width, height) > CLOUD_MAX_AREA && width.max(height) > 16 {
        if width >= height {
            width -= 16;
        } else {
            height -= 16;
        }
    }
    while area(width, height) < CLOUD_MIN_AREA {
        if width <= height {
            width += 16;
        } else {
            height += 16;
        }
    }
    (width, height)
}

/// The size of the closest shape among those every service knows.
fn standard_size(width: u32, height: u32) -> (u32, u32) {
    let ratio = f64::from(width.max(1)) / f64::from(height.max(1));
    if ratio > 1.25 {
        (1536, 1024)
    } else if ratio < 0.8 {
        (1024, 1536)
    } else {
        (1024, 1024)
    }
}

/// Sizes to ask a cloud service for, the most wanted first. Services differ
/// in what they accept, so a refused size is followed by the next one;
/// `None` leaves the choice to the service. `exact` puts the requested size
/// itself first.
fn cloud_sizes(width: u32, height: u32, exact: bool) -> Vec<Option<(u32, u32)>> {
    let mut sizes = Vec::with_capacity(4);
    if exact {
        sizes.push(Some((width, height)));
    }
    for size in [
        Some(budget_size(width, height)),
        Some(standard_size(width, height)),
        None,
    ] {
        if !sizes.contains(&size) {
            sizes.push(size);
        }
    }
    sizes
}

/// Whether a service turned the request down because of the picture size.
fn refuses_size(error: &Error) -> bool {
    if error.code != "httpStatus" {
        return false;
    }
    let status = error.params.get("status").map_or("", String::as_str);
    let detail = error
        .params
        .get("detail")
        .map_or_default(|detail| detail.to_lowercase());
    matches!(status, "400" | "422")
        && ["size", "resolution", "dimension"]
            .iter()
            .any(|word| detail.contains(word))
}

fn resize_mask(mask: &Mask, width: u32, height: u32) -> Result<Mask> {
    let mut rgba = Vec::with_capacity(mask.data.len() * 4);
    for value in &mask.data {
        rgba.extend_from_slice(&[*value, *value, *value, 255]);
    }
    let small = ops::resize::resize_to(
        &Image::from_rgba8(mask.width, mask.height, rgba)?,
        width,
        height,
        ResizeFilter::Bilinear,
        false,
        None,
    )?;
    Ok(Mask {
        width,
        height,
        data: small
            .to_rgba8()
            .chunks_exact(4)
            .map(|pixel| pixel[0])
            .collect(),
    })
}

/// The mask as the image services take it: transparent where the picture
/// is to be changed.
fn hole_mask_png(mask: &Mask) -> Result<Vec<u8>> {
    let mut rgba = Vec::with_capacity(mask.data.len() * 4);
    for value in &mask.data {
        rgba.extend_from_slice(&[0, 0, 0, 255 - *value]);
    }
    png(&Image::from_rgba8(mask.width, mask.height, rgba)?)
}

/// What is uploaded for an edit: the picture and, with an area to change,
/// its mask.
struct Upload {
    picture: Vec<u8>,
    mask: Option<Vec<u8>>,
}

impl Upload {
    fn of(source: &Source<'_>) -> Result<Self> {
        let image = source.image;
        let longest = image.width().max(image.height());
        if longest <= CLOUD_SIDE {
            return Ok(Self {
                picture: opaque_png(image)?,
                mask: source.mask.map(hole_mask_png).transpose()?,
            });
        }
        let side = |value: u32| {
            ((u64::from(value) * u64::from(CLOUD_SIDE) / u64::from(longest)).max(1)) as u32
        };
        let (width, height) = (side(image.width()), side(image.height()));
        let small =
            ops::resize::resize_to(image, width, height, ResizeFilter::Lanczos3, false, None)?;
        let mask = match source.mask {
            Some(mask) => Some(hole_mask_png(&resize_mask(mask, width, height)?)?),
            None => None,
        };
        Ok(Self {
            picture: opaque_png(&small)?,
            mask,
        })
    }
}

fn cloud(
    net: &Net,
    settings: &AiSettings,
    job: &Job<'_>,
    source: Option<&Source<'_>>,
    cancel: &CancelToken,
    tick: &Tick<'_>,
) -> Result<Vec<Image>> {
    let endpoint = &settings.generation.cloud;
    let model = if job.checkpoint.is_empty() {
        endpoint.model.as_str()
    } else {
        job.checkpoint
    };
    if endpoint.url.trim().is_empty() || model.is_empty() {
        return Err(Error::new("providerNotSet"));
    }
    let base = endpoint.url.as_str();
    let key = access(settings, base)?;
    let upload = source.map(Upload::of).transpose()?;
    let ask = |size: Option<(u32, u32)>, count: u32| -> Result<Value> {
        let count_text = count.to_string();
        let size = size.map(|(width, height)| format!("{width}x{height}"));
        if let Some(upload) = &upload {
            let mut parts = vec![
                Part::field("model", model),
                Part::field("prompt", job.prompt),
                Part::field("n", &count_text),
            ];
            if let Some(size) = &size {
                parts.push(Part::field("size", size));
            }
            parts.push(Part::file(
                "image",
                "image.png",
                "image/png",
                &upload.picture,
            ));
            if let Some(mask) = &upload.mask {
                parts.push(Part::file("mask", "mask.png", "image/png", mask));
            }
            net.post_form(
                &format!("{base}/images/edits"),
                key.as_deref(),
                &parts,
                Some(GENERATION_TIMEOUT),
            )
        } else {
            let mut body = json!({ "model": model, "prompt": job.prompt, "n": count });
            if let Some(size) = size {
                body["size"] = json!(size);
            }
            net.post_json(
                &format!("{base}/images/generations"),
                key.as_deref(),
                &body,
                Some(GENERATION_TIMEOUT),
            )
        }
    };
    let once = |count: u32| -> Result<Vec<Image>> {
        let mut refusal = None;
        let mut answer = None;
        for size in cloud_sizes(job.width, job.height, source.is_none()) {
            match ask(size, count) {
                Ok(value) => {
                    answer = Some(value);
                    break;
                }
                Err(error) if refuses_size(&error) => refusal = Some(error),
                Err(error) => return Err(error),
            }
        }
        let Some(answer) = answer else {
            return Err(refusal.unwrap_or_else(|| failed("the service accepted no size")));
        };
        let mut images = Vec::new();
        for item in answer["data"].as_array().into_iter().flatten() {
            if let Some(encoded) = item["b64_json"].as_str() {
                images.push(decode_image(&STANDARD.decode(encoded).map_err(failed)?)?);
            } else if let Some(url) = item["url"].as_str() {
                images.push(decode_image(&net.get(
                    url,
                    None,
                    Some(Duration::from_secs(120)),
                )?)?);
            }
        }
        Ok(images)
    };
    let mut images = Vec::new();
    let mut single = false;
    while (images.len() as u32) < job.count {
        cancel.check()?;
        let left = job.count - images.len() as u32;
        let count = if single { 1 } else { left };
        tick(images.len() as u32, job.count, 0, 0, None);
        match once(count) {
            Ok(batch) if batch.is_empty() => {
                if images.is_empty() {
                    return Err(failed("the service returned no pictures"));
                }
                break;
            }
            Ok(batch) => {
                if (batch.len() as u32) < count {
                    single = true;
                }
                images.extend(batch);
            }
            Err(error) if images.is_empty() && count > 1 && progress::refuses_count(&error) => {
                single = true;
            }
            Err(_error) if !images.is_empty() => break,
            Err(error) => return Err(error),
        }
    }
    images.truncate(job.count as usize);
    Ok(images)
}

/// Whether the chosen provider sends pictures and prompts off this computer.
pub fn leaves_computer(settings: &AiSettings) -> bool {
    leaves_with(settings, settings.generation.provider)
}

/// Whether this generator, rather than the one in the settings, sends work
/// off this computer.
pub fn leaves_with(settings: &AiSettings, provider: GenerationProvider) -> bool {
    let generation = &settings.generation;
    match provider {
        GenerationProvider::None | GenerationProvider::SdCpp => false,
        GenerationProvider::Comfy => !net::is_local(&generation.comfy_url),
        GenerationProvider::Forge => !net::is_local(&generation.forge_url),
        GenerationProvider::Cloud => !net::is_local(&generation.cloud.url),
    }
}

fn apply_forge_model(body: &mut Value, checkpoint: &str) {
    if checkpoint.is_empty() {
        return;
    }
    body["override_settings"] = json!({ "sd_model_checkpoint": checkpoint });
    body["override_settings_restore_afterwards"] = json!(false);
}

/// Generates pictures. `tick` is told the sampler step and, when the provider
/// sends one, a preview. A provider of `None` and an empty model use the settings.
pub fn generate(
    net: &Net,
    paths: &Paths,
    settings: &AiSettings,
    request: &GenerateRequest,
    source: Option<&Source<'_>>,
    cancel: &CancelToken,
    tick: &Tick<'_>,
) -> Result<Outcome> {
    if request.prompt.trim().is_empty() {
        return Err(Error::new("emptyPrompt"));
    }
    let defaults = &settings.generation;
    let provider = match request.provider {
        GenerationProvider::None => defaults.provider,
        chosen => chosen,
    };
    let pick = |value: u32, fallback: u32| if value == 0 { fallback } else { value };
    let (width, height) = match source {
        Some(source) => (source.image.width(), source.image.height()),
        None => (
            (pick(request.width, defaults.width).clamp(64, 4096) / 8) * 8,
            (pick(request.height, defaults.height).clamp(64, 4096) / 8) * 8,
        ),
    };
    let negative = if request.negative.is_empty() {
        defaults.negative.clone()
    } else {
        request.negative.clone()
    };
    let styles = styles::selected(paths, &request.styles);
    let (prompt, negative) = styles::apply(request.prompt.trim(), &negative, &styles);
    let model = chosen_model(settings, provider, &request.model).to_owned();
    let steps = pick(request.steps, defaults.steps).clamp(1, 150);
    let cfg = if request.cfg > 0.0 {
        request.cfg
    } else {
        defaults.cfg
    };
    let job = Job {
        prompt: &prompt,
        negative: &negative,
        width,
        height,
        steps,
        cfg,
        seed: request.seed.unwrap_or_else(|| u64::from(fastrand::u32(..))),
        count: request.count.clamp(1, MAX_COUNT),
        strength: request.strength.clamp(0.05, 1.0),
        checkpoint: &model,
    };
    cancel.check()?;
    let images = match provider {
        GenerationProvider::None => Err(Error::new("providerNotSet")),
        GenerationProvider::Comfy => comfy(net, settings, &job, source, cancel, tick),
        GenerationProvider::Forge => forge(net, settings, &job, source, cancel, tick),
        GenerationProvider::SdCpp => sdcpp(paths, settings, &job, source, cancel, tick),
        GenerationProvider::Cloud => cloud(net, settings, &job, source, cancel, tick),
    }?;
    let seeds = (0..images.len())
        .map(|index| job.seed.wrapping_add(index as u64))
        .collect();
    Ok(Outcome {
        images,
        seeds,
        provider,
        model,
        prompt,
        negative,
        width,
        height,
        steps,
        cfg,
    })
}

/// Saves generated pictures as PNG files in the application's folder and
/// returns their paths. `notes` are the `parameters` blocks, one per picture;
/// a missing or empty note writes a plain PNG.
pub fn save_generated(
    paths: &Paths,
    images: &[Image],
    notes: Option<&[String]>,
) -> Result<Vec<PathBuf>> {
    let folder = paths.generated();
    fs::create_dir_all(&folder)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let mut saved = Vec::with_capacity(images.len());
    for (index, image) in images.iter().enumerate() {
        let mut number = index + 1;
        let path = loop {
            let candidate = folder.join(format!("pixelit-{stamp}-{number}.png"));
            if !candidate.exists() {
                break candidate;
            }
            number += images.len();
        };
        let mut bytes = png(image)?;
        if let Some(note) = notes
            .and_then(|notes| notes.get(index))
            .filter(|note| !note.is_empty())
        {
            bytes = pixelit_codecs::png_set_text(bytes, "parameters", note);
        }
        fs::write(&path, bytes)?;
        saved.push(path);
    }
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use pixelit_core::{Color, Depth};

    use super::*;

    fn job() -> Job<'static> {
        Job {
            prompt: "a \"red\" fox",
            negative: "blur",
            width: 512,
            height: 768,
            steps: 20,
            cfg: 7.0,
            seed: 42,
            count: 1,
            strength: 0.5,
            checkpoint: "model.safetensors",
        }
    }

    #[test]
    fn text_to_image_graph_is_complete() {
        let graph = comfy_graph(&job(), "model.safetensors", None, false);
        assert_eq!(graph["4"]["class_type"], "EmptyLatentImage");
        assert_eq!(graph["4"]["inputs"]["height"], 768);
        assert_eq!(graph["5"]["inputs"]["denoise"], 1.0);
        assert_eq!(graph["5"]["inputs"]["latent_image"][0], "4");
        assert!(graph.get("8").is_none());
    }

    #[test]
    fn inpaint_graph_uses_the_mask() {
        let graph = comfy_graph(&job(), "model.safetensors", Some("in.png"), true);
        assert_eq!(graph["8"]["inputs"]["image"], "in.png");
        assert_eq!(graph["4"]["class_type"], "VAEEncode");
        assert_eq!(graph["9"]["class_type"], "SetLatentNoiseMask");
        assert_eq!(graph["5"]["inputs"]["latent_image"][0], "9");
        assert_eq!(graph["5"]["inputs"]["denoise"], 0.5);
    }

    #[test]
    fn workflows_take_quoted_text() {
        let template = r#"{"1":{"inputs":{"text":"%prompt%","seed":%seed%,"w":%width%}}}"#;
        let filled = fill_workflow(template, &job(), None).unwrap();
        assert_eq!(filled["1"]["inputs"]["text"], "a \"red\" fox");
        assert_eq!(filled["1"]["inputs"]["seed"], 42);
        assert_eq!(filled["1"]["inputs"]["w"], 512);
        let with_model = fill_workflow(r#"{"ckpt":"%checkpoint%"}"#, &job(), None).unwrap();
        assert_eq!(with_model["ckpt"], "model.safetensors");
        assert_eq!(
            fill_workflow("{", &job(), None).unwrap_err().code,
            "badWorkflow"
        );
    }

    #[test]
    fn names_are_encoded_for_addresses() {
        assert_eq!(percent_encode("a b/ч.png"), "a%20b%2F%D1%87.png");
    }

    #[test]
    fn cloud_needs_permission() {
        let mut settings = AiSettings::default();
        assert!(access(&settings, "http://127.0.0.1:8188").is_ok());
        let error = access(&settings, "https://api.example.com/v1").unwrap_err();
        assert_eq!(error.code, "cloudDisabled");
        settings.allow_cloud = true;
        assert!(access(&settings, "https://api.example.com/v1").is_ok());
        assert_eq!(access(&settings, "").unwrap_err().code, "badUrl");
    }

    #[test]
    fn provider_must_be_chosen() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        let settings = AiSettings::default();
        let cancel = CancelToken::new();
        let mut request = GenerateRequest::default();
        let net = Net::new();
        let empty = generate(&net, &paths, &settings, &request, None, &cancel, &quiet).unwrap_err();
        assert_eq!(empty.code, "emptyPrompt");
        request.prompt = String::from("a fox");
        let unset = generate(&net, &paths, &settings, &request, None, &cancel, &quiet).unwrap_err();
        assert_eq!(unset.code, "providerNotSet");
        assert!(!leaves_computer(&settings));
        let mut body = json!({ "prompt": "a fox" });
        apply_forge_model(&mut body, "dream.safetensors");
        assert_eq!(
            body["override_settings"]["sd_model_checkpoint"],
            "dream.safetensors"
        );
        assert_eq!(body["override_settings_restore_afterwards"], false);
        apply_forge_model(&mut body, "");
        assert_eq!(
            body["override_settings"]["sd_model_checkpoint"],
            "dream.safetensors"
        );
    }

    #[test]
    fn holes_follow_the_mask() {
        let image = Image::new(2, 1, Depth::Eight, Color::rgb(10, 20, 30)).unwrap();
        let mask = Mask {
            width: 2,
            height: 1,
            data: vec![255, 0],
        };
        let bytes = png_with_holes(&Source {
            image: &image,
            mask: Some(&mask),
        })
        .unwrap();
        let back = decode_image(&bytes).unwrap();
        assert_eq!(back.pixel8(0, 0)[3], 0);
        assert_eq!(back.pixel8(1, 0), [10, 20, 30, 255]);
        let plain = decode_image(&mask_png(&mask).unwrap()).unwrap();
        assert_eq!(plain.pixel8(0, 0)[0], 255);
        assert_eq!(plain.pixel8(1, 0)[0], 0);
    }

    #[test]
    fn cloud_sizes_start_with_the_wanted_one() {
        assert_eq!(budget_size(512, 512), (1024, 1024));
        assert_eq!(budget_size(1024, 1024), (1024, 1024));
        assert_eq!(budget_size(1536, 1024), (1536, 1024));
        let (width, height) = budget_size(6000, 4000);
        assert_eq!((width % 16, height % 16), (0, 0));
        let area = u64::from(width) * u64::from(height);
        assert!((CLOUD_MIN_AREA..=CLOUD_MAX_AREA).contains(&area));
        assert!((f64::from(width) / f64::from(height) - 1.5).abs() < 0.05);
        let (width, height) = budget_size(300, 800);
        assert!(u64::from(width) * u64::from(height) >= CLOUD_MIN_AREA);
        assert!(height > width);

        assert_eq!(standard_size(1920, 1080), (1536, 1024));
        assert_eq!(standard_size(900, 1600), (1024, 1536));
        assert_eq!(standard_size(1000, 1100), (1024, 1024));

        assert_eq!(
            cloud_sizes(512, 512, true),
            [Some((512, 512)), Some((1024, 1024)), None]
        );
        assert_eq!(cloud_sizes(1024, 1024, true), [Some((1024, 1024)), None]);
        assert_eq!(cloud_sizes(2048, 1365, false), [Some((1536, 1024)), None]);
    }

    #[test]
    fn only_size_refusals_are_retried() {
        let refusal = Error::new("httpStatus")
            .with("status", 400)
            .detail("Invalid size '512x512'. Requested resolution is below the minimum.");
        assert!(refuses_size(&refusal));
        let key = Error::new("httpStatus")
            .with("status", 401)
            .detail("Missing bearer authentication in header");
        assert!(!refuses_size(&key));
        let busy = Error::new("httpStatus")
            .with("status", 400)
            .detail("The prompt was rejected");
        assert!(!refuses_size(&busy));
        assert!(!refuses_size(&Error::new("network").detail("size")));
    }

    #[test]
    fn uploads_are_reduced_and_carry_the_mask() {
        let image = Image::new(4096, 1024, Depth::Eight, Color::rgb(10, 20, 30)).unwrap();
        let mut mask = Mask {
            width: 4096,
            height: 1024,
            data: vec![0; 4096 * 1024],
        };
        mask.data[..4096 * 512].fill(255);
        let upload = Upload::of(&Source {
            image: &image,
            mask: Some(&mask),
        })
        .unwrap();
        let picture = decode_image(&upload.picture).unwrap();
        assert_eq!((picture.width(), picture.height()), (2048, 512));
        let holes = decode_image(&upload.mask.unwrap()).unwrap();
        assert_eq!((holes.width(), holes.height()), (2048, 512));
        assert_eq!(holes.pixel8(100, 10)[3], 0);
        assert_eq!(holes.pixel8(100, 500)[3], 255);

        let small = Image::new(8, 8, Depth::Eight, Color::WHITE).unwrap();
        let plain = Upload::of(&Source {
            image: &small,
            mask: None,
        })
        .unwrap();
        assert!(plain.mask.is_none());
        assert_eq!(decode_image(&plain.picture).unwrap().width(), 8);
    }

    #[test]
    fn generated_pictures_get_unique_names() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        let image = Image::new(4, 4, Depth::Eight, Color::WHITE).unwrap();
        let first = save_generated(&paths, &[image.clone(), image.clone()], None).unwrap();
        let second = save_generated(&paths, &[image], None).unwrap();
        assert_eq!(first.len(), 2);
        assert!(first.iter().chain(&second).all(|path| path.is_file()));
        assert_ne!(first[0], second[0]);
    }
}
