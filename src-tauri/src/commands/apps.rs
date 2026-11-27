//! ComfyUI and Forge: install, start, stop, update and model search.
//!
//! A download happens only when a button asks for it. A process this section
//! did not start is left running.

use std::process::Command;
use std::sync::Arc;

use serde_json::json;
use tauri::Emitter;

use pixelit_ai::browse::{self, ModelHit};
use pixelit_ai::{apps, AppKind, ModelKind, SearchSource};
use pixelit_core::{Error, Result};
use pixelit_engine::{JobContext, Lane};

use super::CoreState;
use crate::app::Core;

fn parse_kind(text: &str) -> Result<AppKind> {
    AppKind::parse(text).ok_or_else(|| Error::new("badArguments").with("name", text))
}

fn model_kind(text: &str) -> Result<ModelKind> {
    ModelKind::parse(text).ok_or_else(|| Error::new("badArguments").with("name", text))
}

fn parse_source(text: &str) -> Result<SearchSource> {
    SearchSource::parse(text).ok_or_else(|| Error::new("badArguments").with("name", text))
}

fn remember_address(core: &Core, app: AppKind) {
    let mut settings = core.ai.settings();
    let port = settings.apps.of(app).port;
    let url = format!("http://127.0.0.1:{port}");
    match app {
        AppKind::Comfy => settings.generation.comfy_url = url,
        AppKind::Forge => settings.generation.forge_url = url,
    }
    let _ = core.ai.set_settings(settings);
}

#[tauri::command(async)]
pub fn apps_status(core: CoreState<'_>) -> apps::AppsStatus {
    if core.ai.link_saved_apps() {
        let _ = core.app().emit("ai:changed", ());
    }
    core.ai
        .apps_host
        .status(core.ai.net(), &core.ai.settings().apps)
}

/// The end of what the program wrote since its last start.
#[tauri::command(async)]
pub fn apps_log(core: CoreState<'_>, kind: String) -> Result<apps::AppLog> {
    Ok(apps::read_log(core.ai.paths(), parse_kind(&kind)?))
}

#[tauri::command(async)]
pub fn apps_log_clear(core: CoreState<'_>, kind: String) -> Result<()> {
    apps::clear_log(core.ai.paths(), parse_kind(&kind)?)
}

#[tauri::command(async)]
pub fn apps_set_home(core: CoreState<'_>, kind: String, home: String) -> Result<()> {
    let kind = parse_kind(&kind)?;
    core.ai.set_app_home(kind, home)?;
    let _ = core.app().emit("ai:changed", ());
    Ok(())
}

#[tauri::command(async)]
pub fn apps_use_stability(core: CoreState<'_>, kind: String) -> Result<()> {
    core.ai.use_stability(parse_kind(&kind)?)?;
    let _ = core.app().emit("ai:changed", ());
    Ok(())
}

#[tauri::command(async)]
pub fn apps_fill_models(core: CoreState<'_>) -> Result<()> {
    core.ai.fill_model_folders()?;
    let _ = core.app().emit("ai:changed", ());
    Ok(())
}

#[tauri::command(async)]
pub fn apps_install(core: CoreState<'_>, kind: String) -> Result<u64> {
    core.ensure_not_updating()?;
    let kind = parse_kind(&kind)?;
    let core = Arc::clone(core.inner());
    Ok(core.engine.jobs.spawn("install", kind.name(), Lane::Io, {
        let core = Arc::clone(&core);
        move |job: &JobContext| {
            let _ = core.ai.apps_host.stop(kind);
            let mut progress = |done: u64, total: u64| job.progress(done, total, kind.name());
            let mut installed = apps::install_pinned(
                core.ai.net(),
                core.ai.paths(),
                kind,
                &mut progress,
                &job.cancel,
            )?;
            let current = core.ai.settings().apps.of(kind).port;
            if current > 0 {
                installed.port = current;
            }
            let mut apps = core.ai.settings().apps;
            *apps.of_mut(kind) = installed;
            core.ai.set_settings({
                let mut settings = core.ai.settings();
                settings.apps = apps;
                settings
            })?;
            let _ = core.app().emit("ai:changed", ());
            Ok(json!({ "installed": kind.name() }))
        }
    }))
}

#[tauri::command(async)]
pub fn apps_start(core: CoreState<'_>, kind: String) -> Result<()> {
    let kind = parse_kind(&kind)?;
    let settings = core.ai.settings();
    apps::start(
        &core.ai.apps_host,
        core.ai.net(),
        core.ai.paths(),
        &settings.apps,
        kind,
    )?;
    remember_address(core.inner(), kind);
    let _ = core.app().emit("ai:changed", ());
    Ok(())
}

#[tauri::command(async)]
pub fn apps_stop(core: CoreState<'_>, kind: String) -> Result<()> {
    core.ai.apps_host.stop(parse_kind(&kind)?)?;
    let _ = core.app().emit("ai:changed", ());
    Ok(())
}

#[tauri::command(async)]
pub fn apps_open(core: CoreState<'_>, kind: String) -> Result<()> {
    let kind = parse_kind(&kind)?;
    let settings = core.ai.settings();
    let port = settings.apps.of(kind).port;
    let address = format!("http://127.0.0.1:{port}/");
    browser(&address)
        .spawn()
        .map(|_| ())
        .map_err(|error| Error::from(error).with("path", address))
}

fn browser(address: &str) -> Command {
    #[cfg(windows)]
    let program = "explorer.exe";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(windows, target_os = "macos")))]
    let program = "xdg-open";
    let mut command = Command::new(program);
    command.arg(address);
    command
}

#[tauri::command(async)]
pub fn apps_check_update(core: CoreState<'_>, kind: String) -> Result<apps::UpdateInfo> {
    let kind = parse_kind(&kind)?;
    let settings = core.ai.settings();
    apps::check_update(core.ai.net(), settings.apps.of(kind), kind)
}

#[tauri::command(async)]
pub fn apps_update(core: CoreState<'_>, kind: String) -> Result<u64> {
    core.ensure_not_updating()?;
    let kind = parse_kind(&kind)?;
    let info = {
        let settings = core.ai.settings();
        apps::check_update(core.ai.net(), settings.apps.of(kind), kind)?
    };
    if !info.newer {
        return Err(Error::new("notFound"));
    }
    let core = Arc::clone(core.inner());
    Ok(core.engine.jobs.spawn("install", kind.name(), Lane::Io, {
        let core = Arc::clone(&core);
        move |job: &JobContext| {
            let _ = core.ai.apps_host.stop(kind);
            let settings = core.ai.settings();
            let port = settings.apps.of(kind).port;
            let info = apps::check_update(core.ai.net(), settings.apps.of(kind), kind)?;
            let mut progress = |done: u64, total: u64| job.progress(done, total, kind.name());
            let installed = apps::install_update(
                core.ai.net(),
                core.ai.paths(),
                kind,
                &info,
                port,
                &mut progress,
                &job.cancel,
            )?;
            let mut apps = core.ai.settings().apps;
            *apps.of_mut(kind) = installed;
            let mut settings = core.ai.settings();
            settings.apps = apps;
            core.ai.set_settings(settings)?;
            let _ = core.app().emit("ai:changed", ());
            Ok(json!({ "updated": kind.name() }))
        }
    }))
}

#[tauri::command(async)]
pub fn apps_search(
    core: CoreState<'_>,
    source: String,
    text: String,
    kind: String,
    sort: String,
    period: String,
    base_model: String,
    hide_installed: bool,
    cursor: String,
) -> Result<browse::SearchPage> {
    apps::search_models(
        core.ai.net(),
        &core.ai.settings().apps,
        parse_source(&source)?,
        &text,
        model_kind(&kind)?,
        &sort,
        &period,
        &base_model,
        hide_installed,
        &cursor,
    )
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewImage {
    pub mime: String,
    pub data: String,
}

/// A catalogue thumbnail. The address is checked before anything is read.
#[tauri::command(async)]
pub fn apps_preview(core: CoreState<'_>, url: String) -> Result<PreviewImage> {
    let (mime, data) = browse::load_preview(core.ai.net(), &url)?;
    Ok(PreviewImage { mime, data })
}

#[tauri::command(async)]
pub fn apps_download(core: CoreState<'_>, hit: ModelHit) -> Result<u64> {
    core.ensure_not_updating()?;
    let core = Arc::clone(core.inner());
    let title = hit.file_name.clone();
    let label = title.clone();
    Ok(core.engine.jobs.spawn("install", &label, Lane::Io, {
        let core = Arc::clone(&core);
        move |job: &JobContext| {
            let mut progress = |done: u64, total: u64| job.progress(done, total, &title);
            let path = apps::download_model(
                core.ai.net(),
                &core.ai.settings().apps,
                hit,
                &mut progress,
                &job.cancel,
            )?;
            let _ = core.app().emit("ai:changed", ());
            Ok(json!({ "file": path.display().to_string() }))
        }
    }))
}
