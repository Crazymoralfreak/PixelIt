//! The menu bar of macOS: the application menu, Edit and Window.
//!
//! Quit is an item of ours, not the one of the system: that one ends the
//! process at once, and PixelIt first stops its jobs and frees the models.
//! The Edit menu is what makes the keys for copy and paste work in the window.

use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Wry};

use super::text::text;
use super::tray;

const QUIT_KEYS: &str = "CmdOrCtrl+Q";

fn build(app: &AppHandle, lang: &str) -> tauri::Result<Menu<Wry>> {
    let label = |key: &str| text(lang, key, &[]);
    let separator = || PredefinedMenuItem::separator(app);

    let about = PredefinedMenuItem::about(
        app,
        Some(&label("native.menu.about")),
        Some(AboutMetadata {
            name: Some(String::from("PixelIt")),
            version: Some(app.package_info().version.to_string()),
            ..AboutMetadata::default()
        }),
    )?;
    let services = PredefinedMenuItem::services(app, Some(&label("native.menu.services")))?;
    let hide = PredefinedMenuItem::hide(app, Some(&label("native.menu.hide")))?;
    let hide_others = PredefinedMenuItem::hide_others(app, Some(&label("native.menu.hideOthers")))?;
    let show_all = PredefinedMenuItem::show_all(app, Some(&label("native.menu.showAll")))?;
    // The same id as the item of the tray: both end up in one handler.
    let quit = MenuItem::with_id(
        app,
        tray::QUIT,
        label("native.menu.quit"),
        true,
        Some(QUIT_KEYS),
    )?;
    let application = Submenu::with_items(
        app,
        "PixelIt",
        true,
        &[
            &about,
            &separator()?,
            &services,
            &separator()?,
            &hide,
            &hide_others,
            &show_all,
            &separator()?,
            &quit,
        ],
    )?;

    let edit = Submenu::with_items(
        app,
        label("native.menu.edit"),
        true,
        &[
            &PredefinedMenuItem::undo(app, Some(&label("native.menu.undo")))?,
            &PredefinedMenuItem::redo(app, Some(&label("native.menu.redo")))?,
            &separator()?,
            &PredefinedMenuItem::cut(app, Some(&label("native.menu.cut")))?,
            &PredefinedMenuItem::copy(app, Some(&label("native.menu.copy")))?,
            &PredefinedMenuItem::paste(app, Some(&label("native.menu.paste")))?,
            &PredefinedMenuItem::select_all(app, Some(&label("native.menu.selectAll")))?,
        ],
    )?;

    let window = Submenu::with_items(
        app,
        label("native.menu.window"),
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some(&label("native.menu.minimize")))?,
            &PredefinedMenuItem::maximize(app, Some(&label("native.menu.zoom")))?,
            &separator()?,
            &PredefinedMenuItem::close_window(app, Some(&label("native.menu.close")))?,
        ],
    )?;

    Menu::with_items(app, &[&application, &edit, &window])
}

/// Puts the menu bar in place, or rebuilds it in another language. Safe to
/// call from any thread: the work is posted to the main one.
pub fn set(app: &AppHandle, lang: String) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || match build(&handle, &lang) {
        Ok(menu) => {
            let _ = handle.set_menu(menu);
        }
        Err(error) => tracing::warn!("the menu bar could not be built: {error}"),
    });
}
