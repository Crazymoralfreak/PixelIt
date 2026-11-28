//! What PixelIt shows outside its main window: the tray icon, the menu bar of
//! macOS and notifications, with texts from the dictionaries of the interface.

pub mod menu;
pub mod text;
pub mod tray;
pub mod window;

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// Shows a notification next to the tray icon. A failure is not worth stopping for.
pub fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}
