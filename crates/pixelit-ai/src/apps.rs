//! ComfyUI and Forge: where they live, how they are started, and how a copy
//! this section installed is updated. A folder the user pointed at, and a
//! package that belongs to Stability Matrix, are never replaced.

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use pixelit_core::{CancelToken, Error, Result};
use pixelit_engine::clock::LocalTime;
use pixelit_engine::Paths;

use crate::browse::{self, ModelHit, SearchPage, SearchQuery, SearchSource};
use crate::config::{
    native_path, AppKind, AppOrigin, LocalApp, LocalApps, ModelKind, ModelLibrary,
};
use crate::net::{self, Net};

/// Largest archive this section will unpack: 16 GiB.
const MAX_UNPACKED: u64 = 16 * 1024 * 1024 * 1024;

const COMFY_REPO: &str = "https://api.github.com/repos/Comfy-Org/ComfyUI/releases/latest";
const FORGE_REPO: &str =
    "https://api.github.com/repos/lllyasviel/stable-diffusion-webui-forge/releases/latest";
const DESKTOP_REPO: &str = "https://api.github.com/repos/Comfy-Org/desktop/releases/latest";

struct Pinned {
    release: &'static str,
    url: &'static str,
    bytes: u64,
    sha256: &'static str,
    /// `7z` is unpacked. `dmg` is the macOS disk image.
    extension: &'static str,
    nvidia: bool,
}

const COMFY_WINDOWS: Pinned = Pinned {
    release: "v0.46.0",
    url: "https://github.com/Comfy-Org/ComfyUI/releases/download/v0.46.0/ComfyUI_windows_portable_nvidia.7z",
    bytes: 2_085_169_944,
    sha256: "6a5255c7ec01fdc54bdbd8e4507073cf6acda2fbeea74ae1829a50681af5fac6",
    extension: "7z",
    nvidia: true,
};

// Filled from the publisher's file when the hash is known. An empty hash
// means this build is not offered.
const COMFY_MAC: Pinned = Pinned {
    release: "v0.9.4",
    url: "https://dl.todesktop.com/241130tqe9q3y/mac/dmg/arm64",
    bytes: 224_726_237,
    sha256: "21bd5f10d9692d7eb4a16240e9ab463b321a5fe5dff631e9fb4a43b22b4cbca3",
    extension: "dmg",
    nvidia: false,
};

const FORGE_WINDOWS: Pinned = Pinned {
    release: "latest",
    url: "https://github.com/lllyasviel/stable-diffusion-webui-forge/releases/download/latest/webui_forge_cu124_torch24.7z",
    bytes: 1_891_112_892,
    sha256: "04e8ad25f86abbb57b2def57a1f8da7d87284b298e45959e195276ab28a00cb0",
    extension: "7z",
    nvidia: true,
};

/// The archive for this operating system, when one is pinned.
pub fn pinned(kind: AppKind) -> Option<&'static str> {
    archive(kind).map(|pinned| pinned.release)
}

fn archive(kind: AppKind) -> Option<&'static Pinned> {
    let pinned = match (kind, std::env::consts::OS, std::env::consts::ARCH) {
        (AppKind::Comfy, "windows", _) => Some(&COMFY_WINDOWS),
        (AppKind::Comfy, "macos", "aarch64") => Some(&COMFY_MAC),
        (AppKind::Forge, "windows", _) => Some(&FORGE_WINDOWS),
        (AppKind::Comfy | AppKind::Forge, _, _) => None,
    }?;
    (pinned.sha256.len() == 64).then_some(pinned)
}

/// Whether `latest` is a newer dotted release than `installed`.
pub fn release_is_newer(installed: &str, latest: &str) -> bool {
    let installed = numbers(installed);
    let latest = numbers(latest);
    !installed.is_empty() && !latest.is_empty() && latest > installed
}

fn numbers(tag: &str) -> Vec<u64> {
    tag.split(|symbol: char| !symbol.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// A ComfyUI `extra_model_paths` file. Absolute folders are listed as they are.
pub fn comfy_paths_yaml(models: &ModelLibrary) -> String {
    let mut text = String::from("pixelit:\n    base_path: .\n");
    for (key, folders) in [
        ("checkpoints", models.of(ModelKind::Checkpoints)),
        ("loras", models.of(ModelKind::Loras)),
        ("vae", models.of(ModelKind::Vaes)),
        ("embeddings", models.of(ModelKind::Embeddings)),
        ("controlnet", models.of(ModelKind::ControlNet)),
        ("upscale_models", models.of(ModelKind::Upscale)),
    ] {
        let folders: Vec<&str> = folders
            .iter()
            .map(String::as_str)
            .map(str::trim)
            .filter(|folder| !folder.is_empty())
            .collect();
        if folders.is_empty() {
            continue;
        }
        text.push_str("    ");
        text.push_str(key);
        text.push_str(": |\n");
        for folder in folders {
            text.push_str("      ");
            text.push_str(folder);
            text.push('\n');
        }
    }
    text
}

/// One folder Forge can be given. Several folders become links in `root`.
pub fn forge_folder(root: &Path, kind: ModelKind, folders: &[String]) -> PathBuf {
    let existing: Vec<&Path> = folders
        .iter()
        .map(Path::new)
        .filter(|folder| folder.is_dir())
        .collect();
    match existing.as_slice() {
        [] => PathBuf::new(),
        [only] => only.to_path_buf(),
        many => {
            let gathered = root.join(kind.name());
            if fs::remove_dir_all(&gathered).is_err() && gathered.exists() {
                return many[0].to_path_buf();
            }
            if fs::create_dir_all(&gathered).is_err() {
                return many[0].to_path_buf();
            }
            let mut failed = false;
            for (index, folder) in many.iter().enumerate() {
                let name = folder.file_name().unwrap_or_default().to_string_lossy();
                let link = gathered.join(format!("{index}-{name}"));
                if !link_dir(&link, folder) {
                    failed = true;
                    break;
                }
            }
            if failed {
                many[0].to_path_buf()
            } else {
                gathered
            }
        }
    }
}

fn link_dir(link: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}

/// Arguments after the Forge program: the API, the port and one folder per kind.
pub fn forge_arguments(port: u16, root: &Path, models: &ModelLibrary) -> Vec<String> {
    let mut args = vec![
        String::from("--api"),
        String::from("--port"),
        port.to_string(),
    ];
    for (flag, kind) in [
        ("--ckpt-dir", ModelKind::Checkpoints),
        ("--lora-dir", ModelKind::Loras),
        ("--vae-dir", ModelKind::Vaes),
        ("--embeddings-dir", ModelKind::Embeddings),
        ("--controlnet-dir", ModelKind::ControlNet),
        ("--esrgan-models-path", ModelKind::Upscale),
    ] {
        let folder = forge_folder(root, kind, models.of(kind));
        if folder.as_os_str().is_empty() {
            continue;
        }
        args.push(String::from(flag));
        args.push(folder.display().to_string());
    }
    args
}

/// What Stability Matrix already has, when its library file can be read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StabilityOffer {
    pub library: String,
    pub comfy: String,
    pub forge: String,
    pub models: ModelLibrary,
}

impl StabilityOffer {
    pub fn found(&self) -> bool {
        !self.library.is_empty()
    }
}

fn field(value: &Value, names: &[&str]) -> String {
    for name in names {
        if let Some(text) = value.get(*name).and_then(Value::as_str) {
            let text = text.trim();
            if !text.is_empty() {
                return text.to_owned();
            }
        }
    }
    String::new()
}

/// The library folder named by Stability Matrix's `library.json`.
pub fn library_path(json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(json).ok()?;
    let path = field(&value, &["libraryPath", "LibraryPath"]);
    (!path.is_empty()).then_some(path)
}

fn package_kind(name: &str) -> Option<AppKind> {
    let name = name.to_ascii_lowercase();
    if name.contains("comfy") {
        Some(AppKind::Comfy)
    } else if name.contains("forge") || name.contains("automatic") {
        Some(AppKind::Forge)
    } else {
        None
    }
}

fn push_dir(list: &mut Vec<String>, path: PathBuf) {
    if path.is_dir() {
        let text = native_path(&path);
        if !list.iter().any(|known| known == &text) {
            list.push(text);
        }
    }
}

/// Reads an installed-package list and the model folders under `library`.
pub fn offer_at(library: &Path, settings: &Value) -> StabilityOffer {
    let mut offer = StabilityOffer {
        library: library.display().to_string(),
        ..StabilityOffer::default()
    };
    let packages = settings
        .get("installedPackages")
        .or_else(|| settings.get("InstalledPackages"))
        .and_then(Value::as_array);
    for package in packages.into_iter().flatten() {
        let name = field(
            package,
            &["packageName", "PackageName", "displayName", "DisplayName"],
        );
        let Some(kind) = package_kind(&name) else {
            continue;
        };
        let relative = field(package, &["libraryPath", "LibraryPath"]);
        if relative.is_empty() {
            continue;
        }
        let full = library.join(&relative);
        if !full.is_dir() {
            continue;
        }
        let slot = match kind {
            AppKind::Comfy if offer.comfy.is_empty() => &mut offer.comfy,
            AppKind::Forge if offer.forge.is_empty() => &mut offer.forge,
            AppKind::Comfy | AppKind::Forge => continue,
        };
        *slot = full.display().to_string();
    }
    let override_dir = field(
        settings,
        &["modelDirectoryOverride", "ModelDirectoryOverride"],
    );
    let models = if override_dir.is_empty() {
        library.join("Models")
    } else {
        PathBuf::from(override_dir)
    };
    push_dir(
        &mut offer.models.checkpoints,
        models.join("StableDiffusion"),
    );
    push_dir(&mut offer.models.loras, models.join("Lora"));
    push_dir(&mut offer.models.loras, models.join("LyCORIS"));
    push_dir(&mut offer.models.vaes, models.join("VAE"));
    push_dir(&mut offer.models.embeddings, models.join("Embeddings"));
    push_dir(&mut offer.models.control_net, models.join("ControlNet"));
    push_dir(&mut offer.models.upscale, models.join("ESRGAN"));
    push_dir(&mut offer.models.upscale, models.join("RealESRGAN"));
    push_dir(&mut offer.models.upscale, models.join("SwinIR"));
    offer
}

fn stability_homes() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        homes.push(PathBuf::from(appdata).join("StabilityMatrix"));
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        homes.push(
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("StabilityMatrix"),
        );
    }
    homes
}

/// Looks for Stability Matrix. Nothing is written.
pub fn detect_stability() -> Option<StabilityOffer> {
    for home in stability_homes() {
        let Some(path) = fs::read_to_string(home.join("library.json"))
            .ok()
            .as_deref()
            .and_then(library_path)
        else {
            continue;
        };
        let library = PathBuf::from(&path);
        if !library.is_dir() {
            continue;
        }
        let settings = fs::read_to_string(library.join("settings.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        return Some(offer_at(&library, &settings));
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AppState {
    Missing,
    Stopped,
    Running,
    /// The port answers, and this section did not start the process.
    Foreign,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCard {
    pub kind: AppKind,
    pub home: String,
    pub origin: AppOrigin,
    pub port: u16,
    pub release: String,
    pub bytes: u64,
    pub state: AppState,
    pub nvidia: bool,
    pub can_install: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppsStatus {
    pub comfy: AppCard,
    pub forge: AppCard,
    pub models: ModelLibrary,
    pub show_nsfw: bool,
    pub stability: bool,
    pub stability_comfy: String,
    pub stability_forge: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// False when the copy was not installed by this section.
    pub owned: bool,
    pub newer: bool,
    pub release: String,
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
}

/// What the studio shows about the generator it is about to ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceState {
    /// The address answers.
    Ready,
    /// Started here, and the address does not answer yet.
    Starting,
    /// Installed, not running: it can be started from the studio.
    Stopped,
    /// Nothing answers, and no program is known that could be started.
    Missing,
    /// Not a program of this computer: another machine, a cloud service,
    /// stable-diffusion.cpp. Nothing is shown.
    Remote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub state: ServiceState,
    /// Started by this program, so it can be stopped here.
    pub ours: bool,
    /// `comfy` or `forge`; empty when there is nothing to start.
    pub app: &'static str,
}

impl Service {
    pub fn remote() -> Self {
        Self {
            state: ServiceState::Remote,
            ours: false,
            app: "",
        }
    }
}

/// The state of a generator on this computer.
pub fn service_state(answers: bool, ours: bool, installed: bool) -> ServiceState {
    if answers {
        ServiceState::Ready
    } else if ours {
        ServiceState::Starting
    } else if installed {
        ServiceState::Stopped
    } else {
        ServiceState::Missing
    }
}

struct Running {
    kind: AppKind,
    child: Child,
}

/// The processes this section started.
#[derive(Default)]
pub struct Host {
    running: Mutex<Vec<Running>>,
}

impl Host {
    pub fn status(&self, net: &Net, apps: &LocalApps) -> AppsStatus {
        let stability = detect_stability();
        AppsStatus {
            comfy: self.card(net, apps, AppKind::Comfy),
            forge: self.card(net, apps, AppKind::Forge),
            models: apps.models.clone(),
            show_nsfw: apps.show_nsfw,
            stability: stability.as_ref().is_some_and(StabilityOffer::found),
            stability_comfy: stability
                .as_ref()
                .map_or(String::new(), |offer| offer.comfy.clone()),
            stability_forge: stability
                .as_ref()
                .map_or(String::new(), |offer| offer.forge.clone()),
        }
    }

    fn card(&self, net: &Net, apps: &LocalApps, kind: AppKind) -> AppCard {
        let app = apps.of(kind);
        let state = self.state(net, kind, app);
        let pinned = archive(kind);
        AppCard {
            kind,
            home: app.home.clone(),
            origin: app.origin,
            port: app.port,
            release: if app.release.is_empty() {
                noted_release(kind, Path::new(&app.home))
            } else {
                app.release.clone()
            },
            bytes: app.bytes,
            state,
            nvidia: app.origin == AppOrigin::Ours && pinned.is_some_and(|pinned| pinned.nvidia),
            can_install: pinned.is_some(),
        }
    }

    fn state(&self, net: &Net, kind: AppKind, app: &LocalApp) -> AppState {
        if app.home.is_empty() || app.origin == AppOrigin::Missing {
            return AppState::Missing;
        }
        if !program_found(kind, Path::new(&app.home)) {
            return AppState::Missing;
        }
        let ours = self.owns(kind);
        let url = format!("http://127.0.0.1:{}/", app.port);
        let answers = net
            .get(&url, None, Some(Duration::from_millis(800)))
            .is_ok();
        if ours {
            AppState::Running
        } else if answers {
            AppState::Foreign
        } else {
            AppState::Stopped
        }
    }

    fn owns(&self, kind: AppKind) -> bool {
        let mut running = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        running.retain_mut(|item| matches!(item.child.try_wait(), Ok(None)));
        running.iter().any(|item| item.kind == kind)
    }

    /// How the generator at `url` stands for the studio, when `kind` is the
    /// program that serves it.
    pub fn service(&self, net: &Net, apps: &LocalApps, kind: AppKind, url: &str) -> Service {
        if !net::is_local(url) {
            return Service::remote();
        }
        let ours = self.owns(kind);
        let answers = net.get(url, None, Some(Duration::from_millis(800))).is_ok();
        let app = apps.of(kind);
        let installed = !answers
            && !ours
            && !app.home.is_empty()
            && app.origin != AppOrigin::Missing
            && program_found(kind, Path::new(&app.home));
        Service {
            state: service_state(answers, ours, installed),
            ours,
            app: kind.name(),
        }
    }

    /// Stops a process this section started. A process it did not start is left alone.
    pub fn stop(&self, kind: AppKind) -> Result<()> {
        let mut running = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(index) = running.iter().position(|item| item.kind == kind) else {
            return Err(Error::new("appNotOurs"));
        };
        let mut item = running.remove(index);
        stop_child(&mut item.child);
        Ok(())
    }

    pub fn stop_all(&self) {
        let mut running = self
            .running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for mut item in running.drain(..) {
            stop_child(&mut item.child);
        }
    }
}

fn stop_child(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{}", child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.wait();
}

fn log_path(paths: &Paths, kind: AppKind) -> PathBuf {
    paths.logs().join(format!("{}.log", kind.name()))
}

/// The end of a log that is read for the page: 128 KiB and 1500 lines.
const LOG_BYTES: u64 = 128 * 1024;
const LOG_LINES: usize = 1500;

/// What one program wrote since its last start.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppLog {
    pub text: String,
    /// The file, so it can be shown in its folder. Empty until a first start.
    pub path: String,
    pub bytes: u64,
    /// True when `text` is only the end of the file.
    pub cut: bool,
}

pub fn read_log(paths: &Paths, kind: AppKind) -> AppLog {
    log_at(&log_path(paths, kind))
}

fn log_at(path: &Path) -> AppLog {
    let Ok(mut file) = fs::File::open(path) else {
        return AppLog::default();
    };
    let bytes = file.metadata().map_or(0, |meta| meta.len());
    let from = bytes.saturating_sub(LOG_BYTES);
    let mut raw = Vec::new();
    if file.seek(SeekFrom::Start(from)).is_err() || file.read_to_end(&mut raw).is_err() {
        return AppLog::default();
    }
    let mut text = String::from_utf8_lossy(&raw).into_owned();
    let mut cut = from > 0;
    if cut {
        // The first line was cut in the middle.
        if let Some(end) = text.find('\n') {
            text.drain(..=end);
        }
    }
    let mut lines = tidy_log(&text);
    if lines.len() > LOG_LINES {
        lines.drain(..lines.len() - LOG_LINES);
        cut = true;
    }
    AppLog {
        text: lines.join("\n"),
        path: path.display().to_string(),
        bytes,
        cut,
    }
}

/// Lines as a terminal would leave them: colour codes are dropped, and a
/// progress bar that redraws itself keeps only its last state.
fn tidy_log(text: &str) -> Vec<String> {
    let mut plain = String::with_capacity(text.len());
    let mut symbols = text.chars();
    while let Some(symbol) = symbols.next() {
        if symbol != '\u{1b}' {
            plain.push(symbol);
            continue;
        }
        if symbols.next() == Some('[') {
            for code in symbols.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        }
    }
    let mut lines: Vec<String> = plain
        .split('\n')
        .map(|line| {
            line.rsplit('\r')
                .find(|part| !part.is_empty())
                .unwrap_or("")
                .trim_end()
                .to_owned()
        })
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// Empties the log. The program may still be writing; it goes on from the start.
pub fn clear_log(paths: &Paths, kind: AppKind) -> Result<()> {
    let path = log_path(paths, kind);
    if !path.is_file() {
        return Ok(());
    }
    OpenOptions::new().write(true).open(&path)?.set_len(0)?;
    Ok(())
}

/// A start begins a new log. The one before it stays beside it as `.prev.log`.
fn keep_previous_log(log: &Path) {
    if fs::metadata(log).map_or(0, |meta| meta.len()) == 0 {
        return;
    }
    let previous = log.with_extension("prev.log");
    if fs::rename(log, &previous).is_err() {
        let _ = OpenOptions::new()
            .write(true)
            .open(log)
            .and_then(|file| file.set_len(0));
    }
}

fn launch_header(command: &Command) -> String {
    let now = LocalTime::now();
    let mut line = command.get_program().to_string_lossy().into_owned();
    for argument in command.get_args() {
        line.push(' ');
        let argument = argument.to_string_lossy();
        if argument.contains(' ') {
            line.push('"');
            line.push_str(&argument);
            line.push('"');
        } else {
            line.push_str(&argument);
        }
    }
    format!(
        "==== {} {} ====\n> {line}\n",
        now.date,
        now.time.replace('-', ":")
    )
}

/// Installs the pinned archive for this system into `folder`.
pub fn install_pinned(
    net: &Net,
    paths: &Paths,
    kind: AppKind,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> Result<LocalApp> {
    let pinned = archive(kind).ok_or_else(|| Error::new("noArchive").with("id", kind.name()))?;
    let folder = install_folder(paths, kind);
    if folder.exists() {
        fs::remove_dir_all(&folder)?;
    }
    fs::create_dir_all(&folder)?;
    let archive_path = paths.temp().join(format!(
        "{}-{}.{}",
        kind.name(),
        pinned.release,
        pinned.extension
    ));
    let mut report = |have: u64| progress(have, pinned.bytes);
    net.download_sha256(
        pinned.url,
        &archive_path,
        Some(pinned.bytes),
        Some(pinned.sha256),
        &mut report,
        cancel,
    )?;
    unpack_archive(&archive_path, &folder, pinned.extension, cancel)?;
    let _ = fs::remove_file(&archive_path);
    if launch_of(kind, &folder).is_none() && !folder.join("ComfyDesktop.dmg").is_file() {
        return Err(Error::new("badArchive").detail("the program was not in the archive"));
    }
    let mut app = LocalApp {
        home: folder.display().to_string(),
        origin: AppOrigin::Ours,
        release: pinned.release.to_owned(),
        bytes: pinned.bytes,
        port: default_port(kind),
    };
    if kind == AppKind::Forge {
        app.port = 7860;
    }
    Ok(app)
}

fn default_port(kind: AppKind) -> u16 {
    match kind {
        AppKind::Comfy => 8188,
        AppKind::Forge => 7860,
    }
}

pub fn install_folder(paths: &Paths, kind: AppKind) -> PathBuf {
    paths.components().join(match kind {
        AppKind::Comfy => "comfyui",
        AppKind::Forge => "forge",
    })
}

fn unpack_archive(
    archive: &Path,
    folder: &Path,
    extension: &str,
    cancel: &CancelToken,
) -> Result<()> {
    cancel.check()?;
    match extension {
        "7z" => {
            let limits = sevenz_rust2::ExtractLimits::new().max_total_size(MAX_UNPACKED);
            sevenz_rust2::decompress_file_with_limits(archive, folder, limits)
                .map_err(|error| Error::new("badArchive").detail(error))?;
        }
        "dmg" => install_dmg(archive, folder)?,
        _ => return Err(Error::new("badArchive").detail(extension)),
    }
    Ok(())
}

fn install_dmg(archive: &Path, folder: &Path) -> Result<()> {
    let kept = folder.join("ComfyDesktop.dmg");
    fs::rename(archive, &kept).or_else(|_| fs::copy(archive, &kept).map(|_| ()))?;
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("hdiutil")
            .args(["attach", "-nobrowse", "-readonly"])
            .arg(&kept)
            .status();
    }
    let _ = folder;
    Ok(())
}

struct ComfyLaunch {
    main: PathBuf,
    python: PathBuf,
    /// Set when `main.py` lives outside the folder the user chose.
    base: Option<PathBuf>,
    /// The interface ComfyUI Desktop ships beside the program. Without it a
    /// Desktop copy stops at once: its Python has no front end package.
    front_end: Option<PathBuf>,
}

fn shipped_front_end(main: &Path) -> Option<PathBuf> {
    let folder = main
        .parent()?
        .join("web_custom_versions")
        .join("desktop_app");
    folder.join("index.html").is_file().then_some(folder)
}

/// True when the folder can be started: a ComfyUI or Forge tree, a kept disk
/// image, or a ComfyUI Desktop data folder whose program is installed.
pub fn program_found(kind: AppKind, home: &Path) -> bool {
    match kind {
        AppKind::Comfy => home.join("ComfyDesktop.dmg").is_file() || resolve_comfy(home).is_some(),
        AppKind::Forge => launch_of(kind, home).is_some(),
    }
}

/// The `main.py` that sits next to `folder_paths.py`. A `main.py` inside a
/// virtualenv is not the program.
fn comfy_main(root: &Path) -> Option<PathBuf> {
    fn walk(folder: &Path, depth: usize) -> Option<PathBuf> {
        if depth == 0 || !folder.is_dir() {
            return None;
        }
        let main = folder.join("main.py");
        if main.is_file() && folder.join("folder_paths.py").is_file() {
            return Some(main);
        }
        for entry in fs::read_dir(folder).ok()?.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(
                name.as_ref(),
                ".venv"
                    | "venv"
                    | "site-packages"
                    | "models"
                    | "custom_nodes"
                    | "node_modules"
                    | "input"
                    | "output"
                    | "temp"
                    | "user"
                    | ".git"
            ) {
                continue;
            }
            if let Some(found) = walk(&path, depth - 1) {
                return Some(found);
            }
        }
        None
    }
    walk(root, 5)
}

fn is_comfy_data(home: &Path) -> bool {
    home.join("models").is_dir()
        && (home.join("user").is_dir() || home.join(".venv").is_dir() || home.join("venv").is_dir())
}

/// ComfyUI Desktop keeps the program next to its installer and the models in
/// another folder. The program is recognized by `resources/ComfyUI/main.py`.
fn comfy_desktop_main() -> Option<PathBuf> {
    let mut folders = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        folders.push(PathBuf::from(local).join("Programs"));
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        folders.push(PathBuf::from(home).join("Applications"));
    }
    folders.push(PathBuf::from("/Applications"));
    folders
        .into_iter()
        .find_map(|folder| desktop_main_under(&folder))
}

fn desktop_main_under(folder: &Path) -> Option<PathBuf> {
    if let Some(found) = comfy_app_main(folder) {
        return Some(found);
    }
    for entry in fs::read_dir(folder).ok()?.flatten() {
        if let Some(found) = comfy_app_main(&entry.path()) {
            return Some(found);
        }
    }
    None
}

fn comfy_app_main(folder: &Path) -> Option<PathBuf> {
    [
        folder.join("resources").join("ComfyUI").join("main.py"),
        folder
            .join("Contents")
            .join("Resources")
            .join("ComfyUI")
            .join("main.py"),
    ]
    .into_iter()
    .find(|path| {
        path.is_file()
            && path
                .parent()
                .is_some_and(|dir| dir.join("folder_paths.py").is_file())
    })
}

fn venv_python(home: &Path) -> Option<PathBuf> {
    [
        home.join(".venv").join("Scripts").join("python.exe"),
        home.join(".venv").join("bin").join("python"),
        home.join(".venv").join("bin").join("python3"),
        home.join("venv").join("Scripts").join("python.exe"),
        home.join("python_embeded").join("python.exe"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn resolve_comfy(home: &Path) -> Option<ComfyLaunch> {
    resolve_comfy_at(home, comfy_desktop_main())
}

fn resolve_comfy_at(home: &Path, desktop: Option<PathBuf>) -> Option<ComfyLaunch> {
    let (main, base) = if let Some(main) = comfy_main(home) {
        (main, None)
    } else if is_comfy_data(home) {
        let main = desktop?;
        if !main.is_file() {
            return None;
        }
        (main, Some(home.to_path_buf()))
    } else {
        return None;
    };
    let python = venv_python(home)
        .or_else(|| main.parent().and_then(venv_python))
        .or_else(|| python_near(&main))
        .unwrap_or_else(|| {
            PathBuf::from(if cfg!(windows) {
                "python.exe"
            } else {
                "python"
            })
        });
    let front_end = shipped_front_end(&main);
    Some(ComfyLaunch {
        main,
        python,
        base,
        front_end,
    })
}

/// Port written into `user/comfyui_<port>.log` by a copy that already ran.
fn remembered_port(home: &Path) -> Option<u16> {
    let mut newest: Option<(std::time::SystemTime, u16)> = None;
    for entry in fs::read_dir(home.join("user")).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(rest) = name.strip_prefix("comfyui_") else {
            continue;
        };
        let Some(port_text) = rest.strip_suffix(".log") else {
            continue;
        };
        let Ok(port) = port_text.parse::<u16>() else {
            continue;
        };
        if port == 0 {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        if newest.as_ref().is_none_or(|item| modified > item.0) {
            newest = Some((modified, port));
        }
    }
    newest.map(|item| item.1)
}

fn noted_release(kind: AppKind, home: &Path) -> String {
    if kind != AppKind::Comfy {
        return String::new();
    }
    let Ok(text) = fs::read_to_string(
        home.join("user")
            .join("default")
            .join("comfy.settings.json"),
    ) else {
        return String::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return String::new();
    };
    value
        .get("Comfy.Release.Version")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

/// Adds the model folders that already live inside an install. A folder that
/// is already listed is left where it is.
pub fn adopt_models(kind: AppKind, home: &Path, models: &mut ModelLibrary) {
    let pairs: &[(&str, ModelKind)] = match kind {
        AppKind::Comfy => &[
            ("models/checkpoints", ModelKind::Checkpoints),
            ("models/loras", ModelKind::Loras),
            ("models/vae", ModelKind::Vaes),
            ("models/embeddings", ModelKind::Embeddings),
            ("models/controlnet", ModelKind::ControlNet),
            ("models/upscale_models", ModelKind::Upscale),
        ],
        AppKind::Forge => &[
            ("models/Stable-diffusion", ModelKind::Checkpoints),
            ("models/Lora", ModelKind::Loras),
            ("models/VAE", ModelKind::Vaes),
            ("embeddings", ModelKind::Embeddings),
            ("models/ControlNet", ModelKind::ControlNet),
            ("models/ESRGAN", ModelKind::Upscale),
        ],
    };
    for (relative, model_kind) in pairs {
        let folder = home.join(relative);
        if !folder.is_dir() {
            continue;
        }
        let text = native_path(&folder);
        let list = models.of_mut(*model_kind);
        if !list.iter().any(|item| item == &text) {
            list.push(text);
        }
    }
}

/// Fills model folders and, for a ComfyUI data folder, the port it last used.
/// Nothing is written when the folder is not a program we can start.
pub fn link_folder(apps: &mut LocalApps) -> bool {
    let mut changed = false;
    for kind in [AppKind::Comfy, AppKind::Forge] {
        let app = apps.of(kind).clone();
        if app.home.is_empty() || app.origin == AppOrigin::Missing || app.origin == AppOrigin::Ours
        {
            continue;
        }
        let home = PathBuf::from(&app.home);
        if !program_found(kind, &home) {
            continue;
        }
        let mut models = apps.models.clone();
        adopt_models(kind, &home, &mut models);
        if models != apps.models {
            apps.models = models;
            changed = true;
        }
        if kind == AppKind::Comfy && app.port == default_port(kind) {
            if let Some(port) = remembered_port(&home) {
                if port != app.port {
                    apps.of_mut(kind).port = port;
                    changed = true;
                }
            }
        }
    }
    changed
}

fn launch_of(kind: AppKind, home: &Path) -> Option<PathBuf> {
    match kind {
        AppKind::Comfy => find_named(home, &["main.py"], 5),
        AppKind::Forge => find_named(
            home,
            &[
                "webui.sh",
                "webui.bat",
                "webui-user.sh",
                "webui-user.bat",
                "launch.py",
            ],
            4,
        ),
    }
}

fn find_named(root: &Path, names: &[&str], depth: usize) -> Option<PathBuf> {
    if depth == 0 || !root.is_dir() {
        return None;
    }
    for name in names {
        let direct = root.join(name);
        if direct.is_file() {
            return Some(direct);
        }
    }
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_named(&path, names, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

fn python_near(main: &Path) -> Option<PathBuf> {
    let home = main.ancestors().nth(3).unwrap_or(main);
    find_named(home, &["python.exe", "python3", "python"], 4).or_else(|| {
        for name in ["python.exe", "python3", "python"] {
            if let Some(found) = find_named(main.parent()?, &[name], 5) {
                return Some(found);
            }
        }
        None
    })
}

/// Starts the program. Refuses when the port already answers.
pub fn start(host: &Host, net: &Net, paths: &Paths, apps: &LocalApps, kind: AppKind) -> Result<()> {
    let app = apps.of(kind);
    if app.home.is_empty() {
        return Err(Error::new("appMissing"));
    }
    let url = format!("http://127.0.0.1:{}/", app.port);
    if net
        .get(&url, None, Some(Duration::from_millis(800)))
        .is_ok()
        && !host.owns(kind)
    {
        return Err(Error::new("appAlreadyRunning"));
    }
    if host.owns(kind) {
        return Ok(());
    }
    let home = PathBuf::from(&app.home);
    let log = log_path(paths, kind);
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut command = launch_command(kind, &home, app.port, paths, &apps.models)?;
    keep_previous_log(&log);
    let mut file = OpenOptions::new().create(true).append(true).open(&log)?;
    file.write_all(launch_header(&command).as_bytes())?;
    let errors = file.try_clone()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(errors))
        .env("PYTHONUNBUFFERED", "1");
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    let child = command
        .spawn()
        .map_err(|error| Error::new("appLaunchFailed").detail(error))?;
    host.running
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(Running { kind, child });
    Ok(())
}

fn launch_command(
    kind: AppKind,
    home: &Path,
    port: u16,
    paths: &Paths,
    models: &ModelLibrary,
) -> Result<Command> {
    match kind {
        AppKind::Comfy => {
            if home.join("ComfyDesktop.dmg").is_file() {
                let mut command = Command::new(if cfg!(windows) {
                    "explorer.exe"
                } else {
                    "open"
                });
                command.arg(home.join("ComfyDesktop.dmg"));
                return Ok(command);
            }
            let launch = resolve_comfy(home).ok_or_else(|| Error::new("appMissing"))?;
            let yaml = paths.components().join("comfy-paths.yaml");
            fs::write(&yaml, comfy_paths_yaml(models))?;
            let mut command = Command::new(&launch.python);
            command
                .arg(&launch.main)
                .arg("--listen")
                .arg("127.0.0.1")
                .arg("--port")
                .arg(port.to_string())
                .arg("--disable-auto-launch");
            if let Some(base) = &launch.base {
                command.arg("--base-directory").arg(base);
            }
            if let Some(front_end) = &launch.front_end {
                command.arg("--front-end-root").arg(front_end);
            }
            command.arg("--extra-model-paths-config").arg(&yaml);
            if let Some(directory) = launch.main.parent() {
                command.current_dir(directory);
            }
            Ok(command)
        }
        AppKind::Forge => {
            let script = launch_of(kind, home).ok_or_else(|| Error::new("appMissing"))?;
            let args = forge_arguments(port, &paths.components().join("forge-models"), models);
            let mut command = if cfg!(windows) && script.extension().is_some_and(|ext| ext == "bat")
            {
                let mut command = Command::new("cmd");
                command.arg("/C").arg(&script);
                command
            } else if script.extension().is_some_and(|ext| ext == "py") {
                let python = python_near(&script).unwrap_or_else(|| PathBuf::from("python"));
                let mut command = Command::new(python);
                command.arg(&script);
                command
            } else {
                Command::new(&script)
            };
            command.args(&args);
            if let Some(directory) = script.parent() {
                command.current_dir(directory);
            }
            Ok(command)
        }
    }
}

/// A newer release of a copy this section installed.
pub fn check_update(net: &Net, app: &LocalApp, kind: AppKind) -> Result<UpdateInfo> {
    if app.origin != AppOrigin::Ours {
        return Ok(UpdateInfo {
            owned: false,
            newer: false,
            release: String::new(),
            url: String::new(),
            bytes: 0,
            sha256: String::new(),
        });
    }
    let (repo, marker) = match kind {
        AppKind::Comfy if cfg!(windows) => (COMFY_REPO, "windows_portable_nvidia.7z"),
        AppKind::Comfy => (DESKTOP_REPO, "mac/dmg/arm64"),
        AppKind::Forge => (FORGE_REPO, "webui_forge_cu124_torch24.7z"),
    };
    let value = net.get_json(repo, None, Some(Duration::from_secs(20)))?;
    let release = field(&value, &["tag_name"]);
    let asset = github_asset(&value, marker);
    let (url, bytes, sha256) = asset.unwrap_or_else(|| {
        if kind == AppKind::Comfy && !cfg!(windows) {
            (
                String::from("https://download.comfy.org/mac/dmg/arm64"),
                0,
                String::new(),
            )
        } else {
            (String::new(), 0, String::new())
        }
    });
    let newer = release_is_newer(&app.release, &release)
        || (app.bytes > 0 && bytes > 0 && bytes != app.bytes);
    Ok(UpdateInfo {
        owned: true,
        newer,
        release,
        url,
        bytes,
        sha256,
    })
}

fn github_asset(value: &Value, marker: &str) -> Option<(String, u64, String)> {
    let asset = value
        .get("assets")?
        .as_array()?
        .iter()
        .find(|asset| field(asset, &["name"]).contains(marker))?;
    let url = field(asset, &["browser_download_url"]);
    if url.is_empty() {
        return None;
    }
    let digest = field(asset, &["digest"]);
    let sha = digest.strip_prefix("sha256:").unwrap_or("").to_owned();
    Some((
        url,
        asset.get("size").and_then(Value::as_u64).unwrap_or(0),
        sha,
    ))
}

/// Downloads the release `check_update` found into the same folder as an install.
pub fn install_update(
    net: &Net,
    paths: &Paths,
    kind: AppKind,
    info: &UpdateInfo,
    port: u16,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> Result<LocalApp> {
    if !info.owned || info.url.is_empty() {
        return Err(Error::new("noArchive").with("id", kind.name()));
    }
    let folder = install_folder(paths, kind);
    if folder.exists() {
        fs::remove_dir_all(&folder)?;
    }
    fs::create_dir_all(&folder)?;
    let extension = if Path::new(&info.url)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("7z"))
    {
        "7z"
    } else {
        "dmg"
    };
    let archive_path = paths
        .temp()
        .join(format!("{}-update.{extension}", kind.name()));
    let total = info.bytes.max(1);
    let mut report = |have: u64| progress(have, total);
    let bytes = (info.bytes > 0).then_some(info.bytes);
    let sha = (!info.sha256.is_empty()).then_some(info.sha256.as_str());
    net.download_sha256(&info.url, &archive_path, bytes, sha, &mut report, cancel)?;
    let stored = fs::metadata(&archive_path).map_or(info.bytes, |meta| meta.len());
    unpack_archive(&archive_path, &folder, extension, cancel)?;
    let _ = fs::remove_file(&archive_path);
    Ok(LocalApp {
        home: folder.display().to_string(),
        origin: AppOrigin::Ours,
        release: info.release.clone(),
        bytes: stored,
        port,
    })
}

/// Files of one kind already in the shared folders, by file name.
pub fn installed_names(models: &ModelLibrary, kind: ModelKind) -> Vec<String> {
    let mut names = Vec::new();
    for folder in models.of(kind) {
        let Ok(entries) = fs::read_dir(folder) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_ascii_lowercase());
            }
        }
    }
    names
}

pub fn search_models(
    net: &Net,
    apps: &LocalApps,
    source: SearchSource,
    text: &str,
    kind: ModelKind,
    sort: &str,
    period: &str,
    base_model: &str,
    hide_installed: bool,
    cursor: &str,
) -> Result<SearchPage> {
    let mut page = browse::search(
        net,
        &SearchQuery {
            source,
            text: text.to_owned(),
            kind,
            sort: sort.to_owned(),
            period: period.to_owned(),
            base_model: base_model.to_owned(),
            show_nsfw: apps.show_nsfw,
            cursor: cursor.to_owned(),
        },
    )?;
    for hit in &mut page.hits {
        hit.installed = installed_path(&apps.models, kind, hit);
    }
    if hide_installed {
        page.hits.retain(|hit| hit.installed.is_empty());
    }
    Ok(page)
}

/// The file of this result that is already in a shared folder, if there is one.
fn installed_path(models: &ModelLibrary, kind: ModelKind, hit: &ModelHit) -> String {
    let mut names = vec![hit.file_name.as_str()];
    for file in &hit.files {
        if !names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&file.file_name))
        {
            names.push(file.file_name.as_str());
        }
    }
    for folder in models.of(kind) {
        for name in &names {
            if name.is_empty() {
                continue;
            }
            let path = Path::new(folder).join(name);
            if path.is_file() {
                return native_path(&path);
            }
        }
    }
    String::new()
}

/// Downloads one search result into the first folder of its kind.
pub fn download_model(
    net: &Net,
    apps: &LocalApps,
    mut hit: ModelHit,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> Result<PathBuf> {
    if hit.url.contains("huggingface.co/api/models/") {
        let value = net.get_json(&hit.url, None, None)?;
        let kind = hit.kind;
        hit = browse::huggingface_file(&value).ok_or_else(|| Error::new("badResponse"))?;
        hit.kind = kind;
    }
    let folders = apps.models.of(hit.kind);
    let folder = folders
        .first()
        .map(PathBuf::from)
        .ok_or_else(|| Error::new("noModelFolder"))?;
    fs::create_dir_all(&folder)?;
    let target = folder.join(&hit.file_name);
    if !hit.sha256.is_empty()
        && target.is_file()
        && net::verify_sha256(&target, &hit.sha256, cancel)?
    {
        return Ok(target);
    }
    let total = hit.bytes.max(1);
    let mut report = |have: u64| progress(have, total);
    let bytes = (hit.bytes > 0).then_some(hit.bytes);
    let sha = (!hit.sha256.is_empty()).then_some(hit.sha256.as_str());
    net.download_sha256(&hit.url, &target, bytes, sha, &mut report, cancel)?;
    Ok(target)
}

/// Model files in the shared checkpoint folders and beside the chosen file.
pub fn checkpoint_files(models: &ModelLibrary, chosen: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut folders: Vec<PathBuf> = models
        .checkpoints
        .iter()
        .map(PathBuf::from)
        .filter(|folder| folder.is_dir())
        .collect();
    if let Some(parent) = Path::new(chosen).parent() {
        if parent.is_dir() {
            folders.push(parent.to_path_buf());
        }
    }
    for folder in folders {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .map_or_default(str::to_ascii_lowercase);
            if matches!(extension.as_str(), "safetensors" | "gguf" | "ckpt" | "pth")
                && path.is_file()
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_installed_file_is_found_by_any_of_its_names() {
        let folder = tempfile::tempdir().unwrap();
        fs::write(folder.path().join("sharp.onnx"), b"model").unwrap();
        let models = ModelLibrary {
            upscale: vec![folder.path().display().to_string()],
            ..ModelLibrary::default()
        };
        let hit = ModelHit {
            source: SearchSource::OpenModelDb,
            name: String::from("Sharp"),
            kind: ModelKind::Upscale,
            file_name: String::from("sharp.pth"),
            url: String::new(),
            bytes: 0,
            sha256: String::new(),
            base_model: String::new(),
            previews: Vec::new(),
            files: vec![crate::browse::ModelFile {
                file_name: String::from("sharp.onnx"),
                url: String::new(),
                bytes: 0,
                sha256: String::new(),
                format: String::from("onnx"),
            }],
            installed: String::new(),
        };
        let path = installed_path(&models, ModelKind::Upscale, &hit);
        assert!(path.ends_with("sharp.onnx"));
        let missing = ModelHit {
            file_name: String::from("other.pth"),
            files: Vec::new(),
            ..hit
        };
        assert!(installed_path(&models, ModelKind::Upscale, &missing).is_empty());
    }

    #[test]
    fn a_later_release_is_newer_and_a_word_is_not() {
        assert!(release_is_newer("v0.45.0", "v0.46.0"));
        assert!(!release_is_newer("v0.46.0", "v0.46.0"));
        assert!(!release_is_newer("v0.46.0", "latest"));
        assert!(!release_is_newer("", "v0.1.0"));
    }

    #[test]
    fn the_comfy_file_lists_every_folder() {
        let models = ModelLibrary {
            checkpoints: vec![String::from("D:/checkpoints"), String::from("E:/more")],
            loras: vec![String::from("D:/lora")],
            ..ModelLibrary::default()
        };
        let yaml = comfy_paths_yaml(&models);
        assert!(yaml.contains("checkpoints: |"));
        assert!(yaml.contains("D:/checkpoints"));
        assert!(yaml.contains("E:/more"));
        assert!(yaml.contains("loras: |"));
        assert!(!yaml.contains("vae:"));
    }

    #[test]
    fn one_forge_folder_is_passed_through() {
        let folder = tempfile::tempdir().unwrap();
        let models = ModelLibrary {
            checkpoints: vec![folder.path().display().to_string()],
            ..ModelLibrary::default()
        };
        let args = forge_arguments(7860, Path::new("unused"), &models);
        assert_eq!(args[0], "--api");
        assert!(args.iter().any(|arg| arg == "--ckpt-dir"));
        assert!(args
            .iter()
            .any(|arg| arg == &folder.path().display().to_string()));
        assert!(!args.iter().any(|arg| arg == "--lora-dir"));
    }

    #[test]
    fn the_library_file_names_the_folder() {
        assert_eq!(
            library_path(r#"{"libraryPath":"D:/Matrix"}"#).as_deref(),
            Some("D:/Matrix")
        );
        assert_eq!(library_path("{}"), None);
    }

    #[test]
    fn a_stability_package_and_its_models_are_offered() {
        let library = tempfile::tempdir().unwrap();
        fs::create_dir_all(library.path().join("Models").join("StableDiffusion")).unwrap();
        fs::create_dir_all(library.path().join("Models").join("Lora")).unwrap();
        fs::create_dir_all(library.path().join("Packages").join("ComfyUI")).unwrap();
        let settings = serde_json::json!({
            "installedPackages": [{
                "packageName": "ComfyUI",
                "libraryPath": "Packages/ComfyUI"
            }]
        });
        let offer = offer_at(library.path(), &settings);
        assert!(offer.comfy.ends_with("ComfyUI"));
        assert!(offer.forge.is_empty());
        assert_eq!(offer.models.checkpoints.len(), 1);
        assert_eq!(offer.models.loras.len(), 1);
    }

    #[test]
    fn the_studio_is_told_how_its_generator_stands() {
        assert_eq!(service_state(true, false, true), ServiceState::Ready);
        assert_eq!(service_state(true, true, false), ServiceState::Ready);
        assert_eq!(service_state(false, true, false), ServiceState::Starting);
        assert_eq!(service_state(false, false, true), ServiceState::Stopped);
        assert_eq!(service_state(false, false, false), ServiceState::Missing);
        let host = Host::default();
        let apps = LocalApps::default();
        let far = host.service(
            &Net::new(),
            &apps,
            AppKind::Comfy,
            "http://192.168.1.20:8188",
        );
        assert_eq!(far, Service::remote());
    }

    #[test]
    fn stopping_a_program_we_did_not_start_is_refused() {
        let host = Host::default();
        let error = host.stop(AppKind::Comfy).unwrap_err();
        assert_eq!(error.code, "appNotOurs");
    }

    #[test]
    fn the_windows_comfy_archive_is_pinned() {
        assert_eq!(COMFY_WINDOWS.sha256.len(), 64);
        assert_ne!(COMFY_WINDOWS.bytes, 0);
    }

    #[test]
    fn a_data_folder_uses_the_desktop_program_and_keeps_its_models() {
        let data = tempfile::tempdir().unwrap();
        let code = tempfile::tempdir().unwrap();
        fs::create_dir_all(data.path().join("models").join("checkpoints")).unwrap();
        fs::create_dir_all(data.path().join("models").join("loras")).unwrap();
        fs::create_dir_all(data.path().join("user")).unwrap();
        fs::write(data.path().join("user").join("comfyui_8000.log"), "port").unwrap();
        fs::write(code.path().join("main.py"), "#").unwrap();
        fs::write(code.path().join("folder_paths.py"), "#").unwrap();
        let launch = resolve_comfy_at(data.path(), Some(code.path().join("main.py"))).unwrap();
        assert_eq!(launch.main, code.path().join("main.py"));
        assert_eq!(launch.base.as_deref(), Some(data.path()));
        assert_eq!(remembered_port(data.path()), Some(8000));
        let mut models = ModelLibrary::default();
        adopt_models(AppKind::Comfy, data.path(), &mut models);
        assert_eq!(models.checkpoints.len(), 1);
        assert_eq!(models.loras.len(), 1);
        assert!(models.vaes.is_empty());
    }

    #[test]
    fn the_desktop_interface_is_passed_when_it_is_shipped() {
        let code = tempfile::tempdir().unwrap();
        let main = code.path().join("main.py");
        fs::write(&main, "#").unwrap();
        assert_eq!(shipped_front_end(&main), None);
        let front = code.path().join("web_custom_versions").join("desktop_app");
        fs::create_dir_all(&front).unwrap();
        fs::write(front.join("index.html"), "<html>").unwrap();
        assert_eq!(shipped_front_end(&main), Some(front));
    }

    #[test]
    fn a_log_reads_as_a_terminal_left_it() {
        let lines = tidy_log(
            "\u{1b}[32mStarting\u{1b}[0m server\r\n 10%|#\r 50%|###\r100%|#####\nlast  \n\n",
        );
        assert_eq!(lines, ["Starting server", "100%|#####", "last"]);
    }

    #[test]
    fn a_long_log_shows_its_end_and_says_so() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("comfy.log");
        let mut text = (0..20_000)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        text.push('\n');
        fs::write(&path, &text).unwrap();
        let log = log_at(&path);
        assert!(log.cut);
        assert_eq!(log.bytes, text.len() as u64);
        assert!(log.text.ends_with("line 19999"));
        assert!(log.text.lines().count() <= LOG_LINES);
        assert!(log.text.starts_with("line "));
        assert!(!log_at(&folder.path().join("none.log")).cut);
    }

    #[test]
    fn a_start_keeps_the_log_before_it() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("comfy.log");
        fs::write(&path, "first run\n").unwrap();
        keep_previous_log(&path);
        assert!(!path.exists());
        assert_eq!(
            fs::read_to_string(folder.path().join("comfy.prev.log")).unwrap(),
            "first run\n"
        );
        fs::write(&path, "second run\n").unwrap();
        keep_previous_log(&path);
        assert_eq!(
            fs::read_to_string(folder.path().join("comfy.prev.log")).unwrap(),
            "second run\n"
        );
    }

    #[test]
    fn the_start_line_names_the_program_and_quotes_a_path_with_a_space() {
        let mut command = Command::new("python.exe");
        command.arg("main.py").arg("C:/My Files/models");
        let header = launch_header(&command);
        assert!(header.starts_with("==== "));
        assert!(header.contains("> python.exe main.py \"C:/My Files/models\"\n"));
    }

    #[test]
    fn a_nested_main_py_is_not_the_program() {
        let root = tempfile::tempdir().unwrap();
        let nested = root
            .path()
            .join(".venv")
            .join("Lib")
            .join("site-packages")
            .join("dotenv");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("main.py"), "#").unwrap();
        fs::create_dir_all(root.path().join("models")).unwrap();
        fs::create_dir_all(root.path().join("user")).unwrap();
        assert!(comfy_main(root.path()).is_none());
        assert!(is_comfy_data(root.path()));
    }

    #[test]
    fn the_comfy_program_is_the_folder_with_its_paths() {
        let root = tempfile::tempdir().unwrap();
        let comfy = root.path().join("ComfyUI");
        fs::create_dir_all(&comfy).unwrap();
        fs::write(comfy.join("main.py"), "#").unwrap();
        fs::write(comfy.join("folder_paths.py"), "#").unwrap();
        assert_eq!(comfy_main(root.path()), Some(comfy.join("main.py")));
    }
}
