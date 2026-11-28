//! The tray icon: open the window, take a screenshot, quit. On macOS it is
//! the icon in the menu bar.

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Wry};

use super::text::text;
use super::window;

const TRAY_ID: &str = "main";

pub const OPEN: &str = "open";
pub const CAPTURE: &str = "capture";
pub const QUIT: &str = "quit";

/// The letter alone, in black: macOS colours it for a light or a dark menu bar.
const TEMPLATE_ICON: &[u8] = include_bytes!("../../icons/tray-template.png");

fn build_menu(app: &AppHandle, lang: &str) -> tauri::Result<Menu<Wry>> {
    let item =
        |id: &str, key: &str| MenuItem::with_id(app, id, text(lang, key, &[]), true, None::<&str>);
    let menu = Menu::new(app)?;
    menu.append(&item(OPEN, "native.tray.open")?)?;
    menu.append(&item(CAPTURE, "native.tray.capture")?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&item(QUIT, "native.tray.quit")?)?;
    Ok(menu)
}

/// Creates the icon. `on_menu` gets the id of the chosen item.
pub fn create(
    app: &AppHandle,
    lang: &str,
    on_menu: impl Fn(&AppHandle, &str) + Send + Sync + 'static,
) -> tauri::Result<()> {
    // An icon of the menu bar opens its menu with either button; in the tray
    // of Windows the left button opens the window.
    let menu_bar = cfg!(target_os = "macos");
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("PixelIt")
        .menu(&build_menu(app, lang)?)
        .show_menu_on_left_click(menu_bar)
        .on_menu_event(move |app, event| on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(move |tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if !menu_bar {
                    window::show_main(tray.app_handle());
                }
            }
        });
    if menu_bar {
        builder = builder
            .icon(Image::from_bytes(TEMPLATE_ICON)?)
            .icon_as_template(true);
    } else if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// Rebuilds the menu in another language. Safe to call from any thread: the
/// work is posted to the main one.
pub fn set_menu(app: &AppHandle, lang: String) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let (Some(tray), Ok(menu)) = (handle.tray_by_id(TRAY_ID), build_menu(&handle, &lang)) {
            let _ = tray.set_menu(Some(menu));
        }
    });
}
