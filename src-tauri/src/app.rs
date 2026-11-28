//! The application core of the window mode: owns the engine, the AI part,
//! the MCP connections and everything that lives outside the web view.
//!
//! Locks of this module are never held across a call that waits for the main
//! thread (windows, hotkeys, tray): the main thread may be waiting for the
//! same lock in a hotkey or tray handler.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Window, WindowEvent};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
#[cfg(windows)]
use windows::Win32::Globalization::GetUserDefaultUILanguage;

use pixelit_ai::Ai;
use pixelit_core::pipeline::{Conflict, Destination};
use pixelit_core::{CancelToken, Error, Result};
use pixelit_engine::batch::{self, BatchRequest};
use pixelit_engine::preview::PreviewSession;
use pixelit_engine::recipes::Recipes;
use pixelit_engine::watch::{WatchRule, WatchRules, WatchService};
use pixelit_engine::{scan, thumbs, Engine, Item, JobContext, JobInfo, Paths, Settings};
use pixelit_mcp::Hub;

use crate::capture;
use crate::integration;
use crate::shell::text::{error_text, known_language, text};
use crate::shell::{menu, notify, tray, window};
use crate::update::Tracker;

/// Passed by the autostart entry: start in the tray, without the window.
pub const AUTOSTART_FLAG: &str = "--autostart";

/// Time for the main window to leave the screen before the screen is captured.
const HIDE_DELAY: Duration = Duration::from_millis(250);

/// Files the system asked to open before the core was there.
static EARLY_FILES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A picture encoded for the interface, with its MIME type.
pub type Encoded = (Vec<u8>, &'static str);

/// An image opened in the editor: the reduced copy the preview is rendered
/// from, the picture before the steps and the last rendered one. Both
/// pictures are served through `pixelit://`.
pub struct Editor {
    pub session: Mutex<PreviewSession>,
    pub base: Encoded,
    pub rendered: Mutex<Encoded>,
    /// Number of the last render, part of its address.
    pub version: AtomicU64,
    /// Stops the render in progress when a newer one arrives.
    pub cancel: Mutex<CancelToken>,
}

/// The conversation with the assistant that is being answered right now.
#[derive(Default)]
pub struct ChatState {
    pub busy: AtomicBool,
    pub cancel: Mutex<CancelToken>,
    /// Tool calls waiting for the user's decision, by call id.
    pub approvals: Mutex<HashMap<String, Sender<bool>>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub data_dir: String,
    pub portable: bool,
    /// The copy was put in place by the installer and can update itself.
    pub installed: bool,
    /// The operating system: `windows`, `macos` or `linux`.
    pub platform: &'static str,
    pub system_language: String,
    /// Language of the interface: the setting, or the system language.
    pub language: String,
}

pub struct Core {
    app: AppHandle,
    pub engine: Arc<Engine>,
    pub ai: Arc<Ai>,
    pub hub: Arc<Hub>,
    pub updates: Tracker,
    pub chat: ChatState,
    pub editors: Mutex<HashMap<u64, Arc<Editor>>>,
    next_editor: AtomicU64,
    watch: Mutex<Option<WatchService>>,
    capture_key: Mutex<Option<Shortcut>>,
    /// Files given on the command line that the window has not taken yet.
    opened: Mutex<Vec<String>>,
    /// The window has taken those files: later ones go to it as an event.
    window_asked: AtomicBool,
    quitting: AtomicBool,
    /// An update is being installed: nothing new starts.
    updating: AtomicBool,
}

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn with_core(app: &AppHandle, action: impl FnOnce(&Arc<Core>)) {
    if let Some(core) = app.try_state::<Arc<Core>>() {
        action(core.inner());
    }
}

/// A dictionary code, English when PixelIt has no dictionary for this one.
fn dictionary(code: &str) -> &'static str {
    match code {
        "zh" => "zh",
        "de" => "de",
        "es" => "es",
        "fr" => "fr",
        "ja" => "ja",
        "ko" => "ko",
        "pl" => "pl",
        "pt" => "pt",
        "ru" => "ru",
        "tr" => "tr",
        _ => "en",
    }
}

/// Language of the system interface when PixelIt has a dictionary for it,
/// English for any other.
#[cfg(windows)]
pub fn system_language() -> &'static str {
    const PRIMARY_LANGUAGE_MASK: u16 = 0x3FF;
    // SAFETY: no preconditions.
    let language = unsafe { GetUserDefaultUILanguage() } & PRIMARY_LANGUAGE_MASK;
    // Primary language ids from WinNls.h.
    let code = match language {
        0x04 => "zh",
        0x07 => "de",
        0x0A => "es",
        0x0C => "fr",
        0x11 => "ja",
        0x12 => "ko",
        0x15 => "pl",
        0x16 => "pt",
        0x19 => "ru",
        0x1F => "tr",
        _ => "en",
    };
    dictionary(code)
}

/// Language of the system interface when PixelIt has a dictionary for it,
/// English for any other. `LANG` looks like `ru_RU.UTF-8`.
#[cfg(not(windows))]
pub fn system_language() -> &'static str {
    let raw = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    let code = raw.split(['_', '.', '@']).next().unwrap_or("en");
    dictionary(code)
}

/// Paths among the arguments of a launch: everything that is not a flag.
pub fn files_of(arguments: &[String]) -> Vec<String> {
    arguments
        .iter()
        .skip(1)
        .filter(|argument| !argument.starts_with("--"))
        .filter(|argument| Path::new(argument.as_str()).exists())
        .cloned()
        .collect()
}

impl Core {
    /// Builds the core and puts it into the app state. Called once, from `setup`.
    pub fn create(app: &AppHandle, engine: Arc<Engine>, ai: Arc<Ai>) -> Arc<Core> {
        let hub = Arc::new(Hub::new(engine.paths.clone(), ai.net().clone()));
        engine.jobs.set_listener({
            let app = app.clone();
            Arc::new(move |info: &JobInfo| {
                let _ = app.emit("jobs:changed", info);
            })
        });
        let mut opened = files_of(&std::env::args().collect::<Vec<_>>());
        opened.append(&mut lock(&EARLY_FILES));
        let core = Arc::new(Core {
            app: app.clone(),
            engine,
            ai,
            hub,
            updates: Tracker::new(),
            chat: ChatState::default(),
            editors: Mutex::new(HashMap::new()),
            next_editor: AtomicU64::new(1),
            watch: Mutex::new(None),
            capture_key: Mutex::new(None),
            opened: Mutex::new(opened),
            window_asked: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
            updating: AtomicBool::new(false),
        });
        app.manage(Arc::clone(&core));
        core
    }

    pub fn app(&self) -> &AppHandle {
        &self.app
    }

    pub fn paths(&self) -> &Paths {
        &self.engine.paths
    }

    pub fn lang(&self) -> String {
        let language = self.engine.settings().language;
        if known_language(&language) {
            language
        } else {
            system_language().to_owned()
        }
    }

    pub fn info(&self) -> AppInfo {
        AppInfo {
            version: self.app.package_info().version.to_string(),
            data_dir: self.paths().root.display().to_string(),
            portable: self.paths().portable,
            installed: crate::update::installed_copy(),
            platform: std::env::consts::OS,
            system_language: system_language().to_owned(),
            language: self.lang(),
        }
    }

    /// Files from the command line, handed to the window once.
    pub fn take_opened(&self) -> Vec<String> {
        let mut opened = lock(&self.opened);
        self.window_asked.store(true, Ordering::SeqCst);
        std::mem::take(&mut *opened)
    }

    /// Files the system asked to open while PixelIt runs. They wait for the
    /// window when it has not started yet, and go to it as an event after.
    fn add_opened(&self, files: Vec<String>) {
        let mut opened = lock(&self.opened);
        if self.window_asked.load(Ordering::SeqCst) {
            drop(opened);
            let _ = self.app.emit("open:files", &files);
        } else {
            opened.extend(files);
        }
    }

    // Settings

    /// Checks, applies and stores new settings. Returns what was stored.
    pub fn set_settings(&self, mut new: Settings) -> Result<Settings> {
        let old = self.engine.settings();
        let language = new.language.trim().to_owned();
        new.language = if language.is_empty() || known_language(&language) {
            language
        } else {
            old.language.clone()
        };
        let hotkey = new.capture_hotkey.trim().to_owned();
        new.capture_hotkey = hotkey;
        if new.capture_hotkey != old.capture_hotkey {
            self.bind_capture(&new.capture_hotkey)?;
        }
        if new.autostart != old.autostart {
            let manager = self.app.autolaunch();
            let changed = if new.autostart {
                manager.enable()
            } else {
                manager.disable()
            };
            changed.map_err(|error| Error::new("autostartFailed").detail(error))?;
        }
        if new.context_menu != old.context_menu {
            integration::set_context_menu(
                new.context_menu,
                &text(&self.lang(), "native.contextMenu", &[]),
            )?;
        }
        let saved = self.engine.set_settings(new)?;
        if saved.language != old.language {
            self.refresh_tray();
        }
        let _ = self.app.emit("settings:changed", &saved);
        Ok(saved)
    }

    /// Registers the screen capture hotkey; an empty text frees it.
    fn bind_capture(&self, hotkey: &str) -> Result<()> {
        let shortcut = if hotkey.is_empty() {
            None
        } else {
            let shortcut: Shortcut = hotkey
                .parse()
                .map_err(|_| Error::new("hotkeyInvalid").with("hotkey", hotkey))?;
            self.app
                .global_shortcut()
                .register(shortcut)
                .map_err(|_| Error::new("hotkeyTaken").with("hotkey", hotkey))?;
            Some(shortcut)
        };
        let previous = std::mem::replace(&mut *lock(&self.capture_key), shortcut);
        if let Some(previous) = previous.filter(|previous| Some(*previous) != shortcut) {
            let _ = self.app.global_shortcut().unregister(previous);
        }
        Ok(())
    }

    /// Shows the menus outside the window in the language of the interface.
    pub fn refresh_tray(&self) {
        tray::set_menu(&self.app, self.lang());
        if cfg!(target_os = "macos") {
            menu::set(&self.app, self.lang());
        }
    }

    // Folders that are watched

    /// Starts watching the folders of the enabled rules again, after a change
    /// of the rules or at startup.
    pub fn restart_watch(self: &Arc<Self>) {
        // The old service stops before the new one starts.
        drop(lock(&self.watch).take());
        let rules = WatchRules::load(self.paths()).rules;
        if !rules.iter().any(|rule| rule.enabled) {
            return;
        }
        let core = Arc::downgrade(self);
        let handler = Box::new(move |rule: &WatchRule, file: &Path| -> Vec<PathBuf> {
            core.upgrade()
                .map_or_default(|core| core.watched_file(rule, file))
        });
        match WatchService::start(&rules, handler) {
            Ok(service) => *lock(&self.watch) = Some(service),
            Err(error) => {
                tracing::warn!("watch folders failed: {error}");
                let lang = self.lang();
                notify(&self.app, "PixelIt", &error_text(&lang, &error));
            }
        }
    }

    /// Processes a new file of a watched folder with the recipe of its rule.
    fn watched_file(&self, rule: &WatchRule, file: &Path) -> Vec<PathBuf> {
        if !scan::is_image(file) || self.updating.load(Ordering::SeqCst) {
            return Vec::new();
        }
        let Some(recipe) = Recipes::load(self.paths()).get(&rule.recipe_id) else {
            tracing::warn!(rule = %rule.name, "the recipe of a watch rule is gone");
            return Vec::new();
        };
        let mut pipeline = recipe.pipeline;
        // Nobody is there to answer a question: existing files are kept.
        if pipeline.output.conflict == Conflict::Overwrite {
            pipeline.output.conflict = Conflict::Rename;
        }
        if pipeline.output.destination == Destination::SameFolder {
            // Results beside the sources would be picked up again.
            pipeline.output.destination = Destination::Subfolder {
                name: String::from("PixelIt"),
            };
        }
        let settings = self.engine.settings();
        let request = BatchRequest {
            title: rule.name.clone(),
            kind: String::from("watch"),
            items: vec![Item {
                root: Some(PathBuf::from(&rule.folder)),
                ..Item::new(file)
            }],
            pipeline,
            threads: 1,
            memory_limit: u64::from(settings.memory_limit_mb) * 1024 * 1024,
        };
        match batch::run(
            &self.engine.runner(),
            Some(self.paths()),
            &request,
            &JobContext::detached(),
        ) {
            Ok(report) => {
                let _ = self.app.emit("journal:changed", ());
                report
                    .items
                    .into_iter()
                    .filter_map(|item| item.outcome.map(|outcome| outcome.output))
                    .collect()
            }
            Err(error) => {
                tracing::warn!(file = %file.display(), "watch folder: {error}");
                Vec::new()
            }
        }
    }

    // Editor sessions

    pub fn add_editor(&self, editor: Editor) -> u64 {
        let id = self.next_editor.fetch_add(1, Ordering::Relaxed);
        lock(&self.editors).insert(id, Arc::new(editor));
        id
    }

    pub fn editor(&self, id: u64) -> Result<Arc<Editor>> {
        lock(&self.editors)
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::new("sessionMissing"))
    }

    pub fn close_editor(&self, id: u64) {
        if let Some(editor) = lock(&self.editors).remove(&id) {
            lock(&editor.cancel).cancel();
        }
    }

    // Screen capture

    /// Takes a picture of the whole desktop and hands it to the window. The
    /// main window leaves the screen for the shot.
    pub fn capture_screen(&self) -> Result<String> {
        let visible = window::main_visible(&self.app);
        if visible {
            window::hide_main(&self.app);
            thread::sleep(HIDE_DELAY);
        }
        let shot = capture::screen().and_then(|image| capture::save(self.paths(), &image));
        window::show_main(&self.app);
        let path = shot?.display().to_string();
        let _ = self.app.emit("capture:done", &path);
        Ok(path)
    }

    // AI components

    /// Tells the engine where the PDF library is once the component is there.
    pub fn sync_pdfium(&self) {
        let folder = self
            .ai
            .pdfium_folder()
            .map_or_default(|folder| folder.display().to_string());
        if self.engine.settings().pdfium_folder != folder {
            let _ = self
                .engine
                .update_settings(|settings| settings.pdfium_folder = folder);
        }
    }

    // Updates and exit

    pub fn is_quitting(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }

    /// Claims the application for the installer of an update. Succeeds only
    /// when no job is running; from then on nothing new starts.
    pub fn begin_install(&self) -> bool {
        if self.engine.jobs.is_busy() || self.chat.busy.load(Ordering::SeqCst) {
            return false;
        }
        self.updating.store(true, Ordering::SeqCst);
        true
    }

    /// The installer could not be started: PixelIt goes on as before.
    pub fn cancel_install(&self) {
        self.updating.store(false, Ordering::SeqCst);
    }

    pub fn ensure_not_updating(&self) -> Result<()> {
        if self.updating.load(Ordering::SeqCst) {
            Err(Error::new("updating"))
        } else {
            Ok(())
        }
    }

    /// Stops the jobs, frees the models and ends the process.
    pub fn quit(self: &Arc<Self>) {
        if self.quitting.swap(true, Ordering::SeqCst) {
            return;
        }
        let core = Arc::clone(self);
        thread::spawn(move || {
            core.engine.jobs.cancel_all();
            lock(&core.chat.cancel).cancel();
            drop(lock(&core.watch).take());
            core.ai.release();
            core.app.exit(0);
        });
    }
}

/// Creates the core, the tray icon and the background services, and shows
/// the main window unless started by the autostart entry.
pub fn setup(
    app: &AppHandle,
    engine: Arc<Engine>,
    ai: Arc<Ai>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let core = Core::create(app, engine, ai);
    tracing::info!(version = %app.package_info().version, "PixelIt started");
    tray::create(app, &core.lang(), tray_menu)?;
    if cfg!(target_os = "macos") {
        menu::set(app, core.lang());
    }

    let hotkey = core.engine.settings().capture_hotkey;
    if let Err(error) = core.bind_capture(&hotkey) {
        // Another program holds the key: say so, the user can pick another one.
        notify(app, "PixelIt", &error_text(&core.lang(), &error));
    }

    let background = Arc::clone(&core);
    thread::Builder::new()
        .name(String::from("pixelit-startup"))
        .spawn(move || {
            background.sync_pdfium();
            background.restart_watch();
            let limit = u64::from(background.engine.settings().thumb_cache_mb) * 1024 * 1024;
            thumbs::trim_cache(background.paths(), limit);
            // Previews of web pictures live for one run of the application.
            pixelit_ai::web::clear_previews(background.paths());
            // Servers the user switched on are started in the background; a
            // server outside this computer needs the cloud permission.
            let remote = background.ai.settings().allow_cloud;
            background.hub.sync(remote, &CancelToken::new());
            let _ = background.app.emit("mcp:changed", ());
        })?;

    if !std::env::args().any(|argument| argument == AUTOSTART_FLAG)
        && !core.engine.settings().start_minimized
    {
        window::show_main(app);
    }
    crate::update::check_at_startup(&core);
    Ok(())
}

/// A second copy was started: the running one comes to the front and opens
/// the files the second one was given.
pub fn second_instance(app: &AppHandle, arguments: &[String]) {
    if arguments.iter().any(|argument| argument == AUTOSTART_FLAG) {
        return;
    }
    let files = files_of(arguments);
    if !files.is_empty() {
        let _ = app.emit("open:files", &files);
    }
    window::show_main(app);
}

/// The system asked the running copy to open files: on macOS Finder does
/// that instead of passing them on the command line.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn files_opened(app: &AppHandle, files: Vec<String>) {
    if files.is_empty() {
        return;
    }
    let Some(core) = app.try_state::<Arc<Core>>() else {
        // Sent before `setup`: the core takes them when it is created.
        lock(&EARLY_FILES).extend(files);
        return;
    };
    core.add_opened(files);
    window::show_main(app);
}

/// What only macOS sends: files to open, and a click on the icon in the Dock
/// while the window is hidden.
#[cfg(target_os = "macos")]
pub fn mac_event(app: &AppHandle, event: &tauri::RunEvent) {
    match event {
        tauri::RunEvent::Opened { urls } => {
            let files = urls
                .iter()
                .filter_map(|url| url.to_file_path().ok())
                .map(|path| path.display().to_string())
                .collect();
            files_opened(app, files);
        }
        tauri::RunEvent::Reopen { .. } => window::show_main(app),
        _ => {}
    }
}

/// A global hotkey was pressed. Runs on the main thread, so the work goes to another one.
pub fn hotkey_pressed(app: &AppHandle, shortcut: &Shortcut) {
    with_core(app, |core| {
        if *lock(&core.capture_key) == Some(*shortcut) {
            let core = Arc::clone(core);
            thread::spawn(move || {
                if let Err(error) = core.capture_screen() {
                    notify(core.app(), "PixelIt", &error_text(&core.lang(), &error));
                }
            });
        }
    });
}

/// An item of the tray menu, or of the menu bar of macOS, was chosen. Runs
/// on the main thread.
fn tray_menu(app: &AppHandle, id: &str) {
    with_core(app, |core| {
        if id == tray::OPEN {
            window::show_main(app);
        } else if id == tray::QUIT {
            core.quit();
        } else if id == tray::CAPTURE {
            let core = Arc::clone(core);
            thread::spawn(move || {
                if let Err(error) = core.capture_screen() {
                    notify(core.app(), "PixelIt", &error_text(&core.lang(), &error));
                }
            });
        }
    });
}

/// Closing the main window hides it to the tray when the settings say so,
/// and quits otherwise.
pub fn window_event(closing: &Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        if closing.label() != window::MAIN {
            return;
        }
        api.prevent_close();
        with_core(closing.app_handle(), |core| {
            if core.engine.settings().close_to_tray {
                let _ = closing.hide();
            } else {
                core.quit();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_and_missing_paths_are_not_files() {
        let folder = std::env::temp_dir();
        let arguments = vec![
            String::from("PixelIt.exe"),
            String::from("--autostart"),
            folder.display().to_string(),
            String::from("Z:\\no\\such\\file.png"),
        ];
        assert_eq!(files_of(&arguments), [folder.display().to_string()]);
        assert!(known_language(system_language()));
    }
}
