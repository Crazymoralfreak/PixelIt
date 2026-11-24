//! Updates from the GitHub releases of PixelIt, asked for only when the
//! setting allows it or the user presses "Check for updates".
//!
//! A copy put in place by the installer downloads the new installer, checks
//! its signature and runs it once no job is running; the installer starts
//! PixelIt again. A portable copy cannot replace itself, so it only tells
//! that a newer version exists.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use tauri::{Emitter, Url};
use tauri_plugin_updater::{Update, UpdaterExt};

use pixelit_engine::settings::UpdateChannel;

use crate::app::Core;
use crate::shell::text::text;
use crate::shell::{notify, window};

/// The check must not compete with the start of the application.
const STARTUP_DELAY: Duration = Duration::from_secs(5);
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(15);
/// How often a downloaded update looks whether PixelIt has become idle.
const IDLE_POLL: Duration = Duration::from_secs(2);
/// Time for the main window to save what is being edited before the process ends.
const SAVE_DELAY: Duration = Duration::from_millis(1500);
/// The installer leaves this file next to the program; a portable copy has none.
const UNINSTALLER: &str = "uninstall.exe";

/// The description of the newest release, pre-releases included. It is kept
/// in the permanent release `beta-channel`; the release workflow refreshes it
/// with every release, so the beta channel never falls behind the stable one.
const BETA_ENDPOINT: &str =
    "https://github.com/Crazymoralfreak/PixelIt/releases/download/beta-channel/latest.json";
const STABLE_ENDPOINT: &str =
    "https://github.com/Crazymoralfreak/PixelIt/releases/latest/download/latest.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Ready,
    Installing,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub state: UpdateState,
    pub version: Option<String>,
    /// Release notes of the version found.
    pub notes: Option<String>,
    /// Downloaded and total bytes of the installer.
    pub downloaded: u64,
    pub total: u64,
    pub portable: bool,
}

/// True for a copy that was put in place by the installer.
pub fn installed_copy() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .map(|folder| folder.join(UNINSTALLER).is_file())
        })
        .unwrap_or(false)
}

/// What is known about updates in this session.
pub struct Tracker {
    status: Mutex<UpdateStatus>,
    /// One check at a time, whoever asked for it.
    busy: AtomicBool,
}

impl Tracker {
    pub fn new() -> Self {
        Self {
            status: Mutex::new(UpdateStatus {
                state: UpdateState::Idle,
                version: None,
                notes: None,
                downloaded: 0,
                total: 0,
                portable: !installed_copy(),
            }),
            busy: AtomicBool::new(false),
        }
    }

    pub fn status(&self) -> UpdateStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn change(&self, change: impl FnOnce(&mut UpdateStatus)) -> UpdateStatus {
        let mut status = self.status.lock().unwrap_or_else(PoisonError::into_inner);
        change(&mut status);
        status.clone()
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

/// The check at startup: after a short delay and only when the settings allow it.
pub fn check_at_startup(core: &Arc<Core>) {
    let core = Arc::clone(core);
    spawn(move || {
        thread::sleep(STARTUP_DELAY);
        if core.engine.settings().check_updates {
            run(&core);
        }
    });
}

/// The check asked for by the user. The result arrives as `update:status` events.
pub fn check_now(core: &Arc<Core>) {
    let core = Arc::clone(core);
    spawn(move || run(&core));
}

fn spawn(work: impl FnOnce() + Send + 'static) {
    // Without a thread there is simply no check.
    let _ = thread::Builder::new()
        .name(String::from("pixelit-update"))
        .spawn(work);
}

fn set(core: &Core, state: UpdateState, version: Option<&str>) {
    let status = core.updates.change(|status| {
        status.state = state;
        status.version = version.map(str::to_owned);
        if version.is_none() {
            status.notes = None;
        }
    });
    let _ = core.app().emit("update:status", &status);
}

fn run(core: &Arc<Core>) {
    if core.updates.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    check_and_install(core);
    core.updates.busy.store(false, Ordering::SeqCst);
}

fn find(core: &Core) -> tauri_plugin_updater::Result<Option<Update>> {
    let mut builder = core.app().updater_builder().timeout(CHECK_TIMEOUT);
    if core.engine.settings().update_channel == UpdateChannel::Beta {
        // The first endpoint that answers is used; the stable one stays as a
        // spare for the time before the first pre-release exists.
        let endpoints = [BETA_ENDPOINT, STABLE_ENDPOINT]
            .iter()
            .filter_map(|endpoint| Url::parse(endpoint).ok())
            .collect();
        builder = builder.endpoints(endpoints)?;
    }
    tauri::async_runtime::block_on(builder.build()?.check())
}

fn check_and_install(core: &Arc<Core>) {
    set(core, UpdateState::Checking, None);
    let mut update = match find(core) {
        Ok(Some(update)) => update,
        Ok(None) => {
            set(core, UpdateState::UpToDate, None);
            return;
        }
        Err(error) => {
            tracing::info!("update check failed: {error}");
            set(core, UpdateState::Failed, None);
            return;
        }
    };
    let version = update.version.clone();
    tracing::info!(%version, "update found");
    core.updates.change(|status| {
        status.notes.clone_from(&update.body);
        status.downloaded = 0;
        status.total = 0;
    });

    if core.updates.status().portable {
        set(core, UpdateState::Available, Some(&version));
        announce(
            core,
            "native.notify.updateAvailable",
            "native.notify.updateAvailableText",
            &version,
        );
        return;
    }

    set(core, UpdateState::Downloading, Some(&version));
    update.timeout = Some(DOWNLOAD_TIMEOUT);
    let progress = |chunk: usize, total: Option<u64>| {
        let status = core.updates.change(|status| {
            status.downloaded += chunk as u64;
            status.total = total.unwrap_or(0);
        });
        let _ = core.app().emit("update:status", &status);
    };
    // The signature of the downloaded installer is checked here, before anything is run.
    let bytes = match tauri::async_runtime::block_on(update.download(progress, || {})) {
        Ok(bytes) => bytes,
        Err(error) => {
            failed(core, &version, &error.to_string());
            return;
        }
    };

    set(core, UpdateState::Ready, Some(&version));
    // A job is never cut short by an update.
    while !core.begin_install() {
        if core.is_quitting() {
            return;
        }
        thread::sleep(IDLE_POLL);
    }

    set(core, UpdateState::Installing, Some(&version));
    announce(
        core,
        "native.notify.updateInstalling",
        "native.notify.updateInstallingText",
        &version,
    );
    thread::sleep(SAVE_DELAY);
    core.ai.release();
    // On success this call does not return: the installer takes over and the process ends.
    if let Err(error) = update.install(&bytes) {
        core.cancel_install();
        failed(core, &version, &error.to_string());
    }
}

fn failed(core: &Core, version: &str, message: &str) {
    tracing::warn!(version, "update failed: {message}");
    set(core, UpdateState::Failed, Some(version));
}

/// The main window shows its own message; a notification is for when it is hidden.
fn announce(core: &Core, title_key: &str, body_key: &str, version: &str) {
    if window::main_visible(core.app()) {
        return;
    }
    let lang = core.lang();
    notify(
        core.app(),
        &text(&lang, title_key, &[]),
        &text(&lang, body_key, &[("version", version)]),
    );
}
