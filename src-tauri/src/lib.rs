//! PixelIt: a desktop image toolkit for Windows.
//!
//! One program has two faces. Started without arguments, or with files to
//! open, it shows its window. Started with a command (`convert`, `run`,
//! `mcp`, ...) it works in the terminal and exits: the same engine without
//! the interface.

pub mod app;
pub mod capture;
pub mod commands;
pub mod console;
pub mod integration;
pub mod protocol;
pub mod shell;
pub mod update;

use std::sync::Arc;

use tauri::RunEvent;
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_global_shortcut::ShortcutState;

use pixelit_ai::Ai;
use pixelit_codecs::decode::{avif_helper_main, set_avif_helper, AVIF_HELPER_ARGUMENT};
use pixelit_core::Result;
use pixelit_engine::{logging, Engine, Paths};

/// Exit code of a start that failed before anything could run.
const EXIT_FAILED: i32 = 1;

/// Opens the data folder and the AI part: what both faces need.
fn open() -> Result<(Arc<Engine>, Arc<Ai>)> {
    let engine = Arc::new(Engine::open(Paths::detect())?);
    let ai = Arc::new(Ai::open(engine.paths.clone()));
    engine.set_ai(Some(ai.clone()));
    // The PDF library is a component: the engine learns where it is.
    let pdfium = ai
        .pdfium_folder()
        .map_or_default(|folder| folder.display().to_string());
    if engine.settings().pdfium_folder != pdfium {
        engine.update_settings(|settings| settings.pdfium_folder = pdfium)?;
    }
    Ok((engine, ai))
}

/// Runs a command of the command line and returns its exit code.
fn run_console(arguments: Vec<String>) -> i32 {
    let attached = console::attach();
    let code = match open() {
        Ok((engine, ai)) => {
            let _log = logging::init(&engine.paths.logs(), false);
            pixelit_cli::run(arguments, &engine, &ai)
        }
        Err(error) => {
            eprintln!("error: {error}");
            EXIT_FAILED
        }
    };
    if attached {
        // The terminal has already printed its prompt: leave it a clean line.
        println!();
    }
    code
}

fn run_window() -> i32 {
    let (engine, ai) = match open() {
        Ok(opened) => opened,
        Err(error) => {
            eprintln!("error: {error}");
            return EXIT_FAILED;
        }
    };
    let _log = logging::init(&engine.paths.logs(), false);
    let built = tauri::Builder::default()
        // Has to be the first plugin: a second copy hands over to the running one and exits.
        .plugin(tauri_plugin_single_instance::init(
            |handle, arguments, _cwd| {
                app::second_instance(handle, &arguments);
            },
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|handle, shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        app::hotkey_pressed(handle, shortcut);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![app::AUTOSTART_FLAG]),
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        // Only the core talks to the updater: the web view gets no permission for it.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_drag::init())
        .register_asynchronous_uri_scheme_protocol(
            protocol::SCHEME,
            |context, request, responder| {
                protocol::handle(context.app_handle(), &request, responder);
            },
        )
        .setup(move |setup| app::setup(setup.handle(), engine, ai))
        .on_window_event(app::window_event)
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::app_quit,
            commands::window_show,
            commands::opened_files,
            commands::settings_get,
            commands::settings_set,
            commands::changelog,
            commands::update_status,
            commands::update_check,
            commands::update_install,
            commands::jobs_list,
            commands::job_cancel,
            commands::job_pause,
            commands::jobs_clear,
            commands::pick_files,
            commands::pick_file,
            commands::pick_folder,
            commands::pick_save,
            commands::reveal_path,
            commands::reveal_logs,
            commands::cache_info,
            commands::cache_clear,
            commands::fonts_list,
            commands::files::folder_list,
            commands::files::files_expand,
            commands::files::file_info,
            commands::files::files_trash,
            commands::files::editor_open,
            commands::files::editor_render,
            commands::files::editor_close,
            commands::files::paste_image,
            commands::files::capture_screen,
            commands::work::batch_start,
            commands::work::export_estimate,
            commands::work::compare_formats,
            commands::work::recipes_list,
            commands::work::recipe_save,
            commands::work::recipe_remove,
            commands::work::recipe_export,
            commands::work::recipe_import,
            commands::work::watch_list,
            commands::work::watch_save,
            commands::work::watch_remove,
            commands::work::journal_list,
            commands::work::journal_clear,
            commands::work::journal_undo,
            commands::work::icons_generate,
            commands::work::social_sizes,
            commands::work::social_generate,
            commands::work::web_kit_generate,
            commands::work::collage_create,
            commands::work::pdf_create,
            commands::work::animation_create,
            commands::work::duplicates_find,
            commands::neural::ai_status,
            commands::neural::ai_settings_set,
            commands::neural::component_install,
            commands::neural::component_remove,
            commands::neural::ai_key_set,
            commands::neural::ai_key_has,
            commands::neural::ai_key_delete,
            commands::neural::ai_discover,
            commands::neural::ai_models,
            commands::neural::generate_start,
            commands::neural::vision_start,
            commands::agents::chats_list,
            commands::agents::chat_get,
            commands::agents::chat_remove,
            commands::agents::chat_busy,
            commands::agents::chat_send,
            commands::agents::chat_approve,
            commands::agents::chat_cancel,
            commands::agents::mcp_status,
            commands::agents::mcp_servers,
            commands::agents::mcp_server_save,
            commands::agents::mcp_server_remove,
            commands::agents::mcp_tool_permission,
            commands::agents::mcp_reconnect,
            commands::agents::mcp_snippet,
        ])
        .build(tauri::generate_context!());
    match built {
        Ok(built) => {
            // Closing the last window is not a reason to quit: PixelIt lives in the tray.
            built.run(|_, event| {
                if let RunEvent::ExitRequested {
                    api, code: None, ..
                } = event
                {
                    api.prevent_exit();
                }
            });
            0
        }
        Err(error) => {
            tracing::error!("PixelIt could not start: {error}");
            EXIT_FAILED
        }
    }
}

/// Starts PixelIt in the mode its arguments ask for and returns the exit code.
pub fn run() -> i32 {
    let arguments: Vec<String> = std::env::args().collect();
    // AVIF files are decoded by a second copy of this program: the decoder
    // may stop its process on a damaged file, and that must not be this one.
    if arguments.get(1).map(String::as_str) == Some(AVIF_HELPER_ARGUMENT) {
        return avif_helper_main();
    }
    if let Ok(program) = std::env::current_exe() {
        set_avif_helper(program);
    }
    if arguments
        .get(1)
        .is_some_and(|first| pixelit_cli::is_command(first))
    {
        run_console(arguments)
    } else {
        run_window()
    }
}
