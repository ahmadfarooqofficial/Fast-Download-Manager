#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager as _};

use fdm_desktop::commands::*;
use fdm_manager::Manager;

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

    let ipc_manager = Arc::clone(&manager);
    tokio::spawn(async move {
        match fdm_ipc::pipe::serve_forever(ipc_manager).await {
            Ok(_) => unreachable!(),
            Err(fdm_ipc::pipe::BindError::AlreadyRunning) => {
                tracing::info!("FDM is already running. Exiting.");
                std::process::exit(0);
            }
            Err(e) => {
                tracing::warn!("Failed to start IPC server: {}", e);
            }
        }
    });

    tauri::Builder::default()
        .manage(Arc::clone(&manager))
        .setup(move |app| {
            let app_handle = app.handle().clone();

            // Set up system tray
            let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let hide_i = MenuItem::with_id(app, "hide", "Hide", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &hide_i, &quit_i])?;

            let mut tray_builder = TrayIconBuilder::new()
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
                    if let fdm_manager::Event::Added(ref entry) = event {
                        open_download_dialog(&app_handle, entry.id);
                    }
                    if let Err(e) = app_handle.emit("download-event", event) {
                        tracing::warn!("Failed to emit event: {}", e);
                    }
                }
            });

            // Show main window if launched directly by user (not --background / --silent)
            let is_background = std::env::args().any(|arg| arg == "--background" || arg == "--silent" || arg == "-b");
            if !is_background {
                if let Some(win) = app.get_webview_window("main") {
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
