#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, OnceLock};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager as _};

/// Open at a size that suits the monitor, then centre it.
///
/// A fixed 1000x700 is two different windows depending on where it lands: most
/// of a laptop screen, and a stamp in the corner of a 4K panel. FDM is a utility
/// that sits open beside real work, so it takes a modest share of whatever
/// screen it finds and is clamped so it never grows unwieldy or shrinks below
/// the point where the list is readable.
fn size_to_screen(win: &tauri::WebviewWindow) {
    const MIN_W: f64 = 860.0;
    const MIN_H: f64 = 560.0;
    const MAX_W: f64 = 1160.0;
    const MAX_H: f64 = 760.0;

    let Ok(Some(monitor)) = win.current_monitor() else { return };
    let scale = monitor.scale_factor();
    let screen = monitor.size().to_logical::<f64>(scale);

    // Slightly wider share than tall: the list wants horizontal room for long
    // file names, but vertical height past a dozen rows just adds emptiness.
    let w = (screen.width * 0.58).clamp(MIN_W, MAX_W).min(screen.width - 80.0);
    let h = (screen.height * 0.68).clamp(MIN_H, MAX_H).min(screen.height - 80.0);

    let _ = win.set_size(tauri::LogicalSize::new(w, h));
    let _ = win.center();
}

use fdm_desktop::commands::*;
use fdm_manager::Manager;

/// Launched to sit in the tray rather than to be looked at: the startup entry,
/// or `fdm-host` bringing the app up to hand it a download.
fn launched_in_background() -> bool {
    std::env::args().any(|arg| arg == "--background" || arg == "--silent" || arg == "-b")
}

/// Ask the copy of FDM that owns the pipe to bring its window to the front.
///
/// Best effort. Whatever goes wrong here, the right thing for this process to do
/// next is the same: leave quietly.
async fn raise_running_instance() {
    match fdm_ipc::connect().await {
        Ok(mut client) => {
            if let Err(e) = client.activate().await {
                tracing::warn!("the running FDM did not show its window: {}", e);
            }
        }
        Err(e) => tracing::warn!("could not reach the running FDM: {}", e),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let manager = match Manager::with_defaults() {
        Ok(m) => Arc::new(m),
        Err(e) => {
            tracing::error!("Failed to create manager: {}", e);
            std::process::exit(1);
        }
    };

    // The single-instance check, before anything visible exists.
    //
    // Owning the pipe is what makes this process *the* FDM. It used to be
    // claimed on a background task while the window and tray icon were already
    // being built, and a second launch — the shortcut double-clicked while FDM
    // sat in the tray, the startup entry racing the browser bridge — then called
    // `process::exit` with a tray icon already registered. Exit skips
    // destructors, so the icon was never removed, and Windows kept drawing a
    // dead "ghost" icon that answered to nothing until the mouse passed over it
    // (issue #4). Now the pipe is claimed first, and a second copy hands the
    // user over to the running one and leaves before it has drawn anything.
    //
    // The hook needs the app handle, which does not exist yet; it is filled in
    // from `setup` below. A request that arrives before then finds the slot
    // empty and is a harmless no-op.
    let app_slot: Arc<OnceLock<tauri::AppHandle>> = Arc::new(OnceLock::new());
    let activate: Arc<dyn Fn() + Send + Sync> = Arc::new({
        let slot = Arc::clone(&app_slot);
        move || {
            if let Some(app) = slot.get() {
                let _ = show_main_window(app.clone());
            }
        }
    });
    let hooks = fdm_ipc::Hooks {
        activate: Some(activate),
    };

    let server = match fdm_ipc::Server::bind(Arc::clone(&manager)) {
        Ok(server) => Some(server.with_hooks(hooks)),
        Err(fdm_ipc::BindError::AlreadyRunning) => {
            if launched_in_background() {
                tracing::info!("FDM is already running. Exiting.");
            } else {
                tracing::info!("FDM is already running; asking it to show its window.");
                raise_running_instance().await;
            }
            return;
        }
        Err(e) => {
            // Not fatal: the app keeps its list and its window, it merely cannot
            // hear from the browser extension. Say so and carry on.
            tracing::warn!("Failed to start IPC server: {}", e);
            None
        }
    };
    if let Some(server) = server {
        tokio::spawn(async move {
            if let Err(e) = server.run().await {
                tracing::warn!("IPC server stopped: {}", e);
            }
        });
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_drag::init())
        .manage(Arc::clone(&manager))
        // Popups wait for this: once the main page has loaded, WebView2 is
        // demonstrably up in this process and a second webview can be trusted
        // to come up too. See `open_download_dialog`.
        .on_page_load(|webview, payload| {
            if webview.label() == "main"
                && matches!(payload.event(), tauri::webview::PageLoadEvent::Finished)
            {
                main_webview_ready(webview.app_handle());
            }
        })
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let _ = app_slot.set(app_handle.clone());

            // If the main page never reports loaded, popups must not wait for
            // ever — a hidden main window on a slow machine is not a reason to
            // swallow the browser's downloads.
            {
                let app_handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(8)).await;
                    main_webview_ready(&app_handle);
                });
            }

            // Pay yt-dlp's first-run unpack cost now, while the user is still
            // looking at the window opening, rather than on their first video.
            fdm_manager::prewarm_video_tools();

            // The one and only tray icon.
            //
            // Do not add `app.trayIcon` back to tauri.conf.json: Tauri builds an
            // icon from that config before `setup` runs, with no menu and no
            // click handler, and this one then becomes the second icon in the
            // tray — the user sees two FDMs, one of which answers to nothing
            // (issue #4). Everything the tray needs is right here.
            let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let hide_i = MenuItem::with_id(app, "hide", "Hide", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &hide_i, &quit_i])?;

            let mut tray_builder = TrayIconBuilder::new()
                .tooltip("Fast Download Manager")
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "hide" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.hide();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                });

            if let Some(icon) = app.default_window_icon() {
                tray_builder = tray_builder.icon(icon.clone());
            }
            let _tray = tray_builder.build(app)?;

            let mut events = manager.subscribe();

            tauri::async_runtime::spawn(async move {
                while let Ok(event) = events.recv().await {
                    match event {
                        fdm_manager::Event::Added(ref entry) => {
                            open_download_dialog(&app_handle, entry.id);
                        }
                        // The popup closes itself when its page is running. When
                        // it is not — it never loaded — this is the only thing
                        // that will, and a removed download must not leave a
                        // window behind.
                        fdm_manager::Event::Removed(id) => {
                            close_download_dialog(&app_handle, id);
                        }
                        fdm_manager::Event::Changed(_) => {}
                    }
                    if let Err(e) = app_handle.emit("download-event", event) {
                        tracing::warn!("Failed to emit event: {}", e);
                    }
                }
            });

            // Show main window if launched directly by user (not --background / --silent)
            if !launched_in_background() {
                if let Some(win) = app.get_webview_window("main") {
                    size_to_screen(&win);
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            add_download,
            pause_download,
            resume_download,
            cancel_download,
            remove_download,
            list_downloads,
            get_download,
            pause_all,
            resume_all,
            clear_finished,
            get_config,
            update_config,
            get_target_dir,
            set_target_dir,
            get_setup_language,
            open_file,
            open_folder,
            minimize_window,
            toggle_maximize_window,
            close_window,
            show_main_window,
            open_download_dialog_cmd
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
