//! The main window. It is created hidden by the configuration and shown once
//! the core is ready, so a start into the tray never flashes a window.

use tauri::{AppHandle, Manager};

pub const MAIN: &str = "main";

pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn hide_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.hide();
    }
}

/// A minimized window still reports itself as visible, so both are checked.
pub fn main_visible(app: &AppHandle) -> bool {
    app.get_webview_window(MAIN).is_some_and(|window| {
        window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false)
    })
}
