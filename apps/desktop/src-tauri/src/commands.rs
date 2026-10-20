use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;
use url::Url;

use fdm_manager::{DownloadEntry, DownloadId, Manager, NewDownload};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigInfo {
    pub max_active: usize,
    pub max_connections: u32,
    pub download_root: PathBuf,
    pub temp_dir: PathBuf,
    pub use_temp_dir: bool,
    /// Per-category overrides, keyed by folder name ("Video", "Programs", …).
    /// Only categories the user actually redirected appear here.
    pub category_dirs: BTreeMap<String, PathBuf>,
    /// Every category name the UI should offer a folder row for, in display
    /// order. Sent from here so the settings screen cannot drift out of sync
    /// with the engine's own list.
    pub categories: Vec<String>,
}

#[tauri::command]
pub fn add_download(
    manager: State<'_, Arc<Manager>>,
    url: String,
    headers: BTreeMap<String, String>,
) -> Result<DownloadId, String> {
    let parsed_url = Url::parse(&url).map_err(|e| e.to_string())?;
    let header_map = fdm_core::sanitize_headers(&headers);
    let new_download = NewDownload::new(parsed_url).with_headers(header_map);
    Ok(manager.add_paused(new_download))
}

#[tauri::command]
pub fn pause_download(manager: State<'_, Arc<Manager>>, id: DownloadId) -> Result<(), String> {
    manager.pause(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn resume_download(manager: State<'_, Arc<Manager>>, id: DownloadId) -> Result<(), String> {
    manager.resume(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cancel_download(manager: State<'_, Arc<Manager>>, id: DownloadId) -> Result<(), String> {
    manager.cancel(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_download(
    manager: State<'_, Arc<Manager>>,
    id: DownloadId,
    delete_file: bool,
) -> Result<(), String> {
    manager.remove(id, delete_file).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_downloads(manager: State<'_, Arc<Manager>>) -> Vec<DownloadEntry> {
    manager.list()
}

#[tauri::command]
pub fn pause_all(manager: State<'_, Arc<Manager>>) -> Result<(), String> {
    manager.pause_all();
    Ok(())
}

#[tauri::command]
pub fn resume_all(manager: State<'_, Arc<Manager>>) -> Result<(), String> {
    manager.resume_all();
    Ok(())
}

#[tauri::command]
pub fn clear_finished(manager: State<'_, Arc<Manager>>) -> usize {
    manager.clear_finished()
}

#[tauri::command]
pub fn get_config(manager: State<'_, Arc<Manager>>) -> ConfigInfo {
    let cfg = manager.engine_config();
    ConfigInfo {
        max_active: manager.max_active(),
        max_connections: cfg.max_connections,
        download_root: cfg.download_root.clone(),
        temp_dir: cfg.temp_dir.clone(),
        use_temp_dir: cfg.use_temp_dir,
        category_dirs: cfg.category_dirs.clone(),
        categories: fdm_core::Category::ALL
            .iter()
            .map(|c| c.folder().to_string())
            .collect(),
    }
}

#[tauri::command]
pub fn update_config(
    manager: State<'_, Arc<Manager>>,
    max_active: Option<usize>,
    max_connections: Option<u32>,
    download_root: Option<PathBuf>,
    temp_dir: Option<PathBuf>,
    category_dirs: Option<BTreeMap<String, PathBuf>>,
) -> Result<(), String> {
    if let Some(active) = max_active {
        manager.set_max_active(active);
    }
    if let Some(conns) = max_connections {
        manager.set_max_connections(conns);
    }
    if let Some(root) = download_root {
        // Fail loudly here rather than at the end of a download, when the bytes
        // have nowhere to land.
        std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
        manager.set_download_root(root);
    }
    if let Some(dir) = temp_dir {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        manager.set_temp_dir(dir);
    }
    if let Some(dirs) = category_dirs {
        for dir in dirs.values() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        manager.set_category_dirs(dirs);
    }
    Ok(())
}

/// The language the user chose in the installer, as an Inno language name
/// ("russian", "german", …), or `None` when there is nothing to go on.
///
/// Setup writes this beside the executable. It exists because the two choices
/// were previously unrelated: installing in Russian still opened an English
/// app, since the app only ever guessed from the OS locale. The frontend
/// consults this once, on a first run with no saved preference — after that the
/// Settings choice is the only thing that matters.
#[tauri::command]
pub fn get_setup_language() -> Option<String> {
    let path = std::env::current_exe().ok()?.parent()?.join("setup-language.txt");
    let raw = std::fs::read_to_string(path).ok()?;
    let name = raw.trim().to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    Some(name)
}

/// Where a download will be saved if started now — shown in the popup so the
/// user can see and change it before committing.
#[tauri::command]
pub fn get_target_dir(manager: State<'_, Arc<Manager>>, id: DownloadId) -> Option<PathBuf> {
    manager.resolve_target_dir(id)
}

/// Redirect one download, leaving every other download's destination alone.
#[tauri::command]
pub fn set_target_dir(
    manager: State<'_, Arc<Manager>>,
    id: DownloadId,
    dir: PathBuf,
) -> Result<(), String> {
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    manager.set_target_dir(id, dir).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &path])
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
}

#[tauri::command]
pub fn open_folder(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;

        let p = std::path::Path::new(&path);
        if p.exists() {
            // Explorer parses its own command line and wants the switch and
            // the path as one token, `/select,"C:\dir\file"`. Handed as two
            // arguments it ignores the selection and opens a default folder.
            std::process::Command::new("explorer")
                .raw_arg(format!("/select,\"{path}\""))
                .spawn()
                .map_err(|e| e.to_string())?;
        } else if let Some(parent) = p.parent() {
            std::process::Command::new("explorer")
                .arg(parent)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
}

#[tauri::command]
pub fn minimize_window(window: tauri::WebviewWindow) -> Result<(), String> {
    window.minimize().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn toggle_maximize_window(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.is_maximized().map_err(|e| e.to_string())? {
        window.unmaximize().map_err(|e| e.to_string())
    } else {
        window.maximize().map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub fn close_window(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        window.hide().map_err(|e| e.to_string())
    } else {
        window.close().map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub fn get_download(manager: State<'_, Arc<Manager>>, id: DownloadId) -> Option<DownloadEntry> {
    manager.list().into_iter().find(|d| d.id == id)
}

#[tauri::command]
pub fn show_main_window(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager as _;
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    Ok(())
}

/// Popup windows: when they may open, and what happens when one fails to load.
///
/// A download handed over by the browser can arrive before the main window's
/// own page has finished loading — the pipe is up before Tauri is — and a
/// second webview created while WebView2 is still bringing up the first one, on
/// a cold boot, could come up blank: a white rectangle that never ran a line of
/// the popup's script (#10). Popups therefore wait for the main page to report
/// loaded (with a bound, in case it never does), each one is given a chance to
/// reload if its page has not loaded in a few seconds, and one that still shows
/// nothing is closed rather than left on the screen.
struct Popups {
    /// Set once the main webview has loaded a page, or the wait ran out.
    main_ready: bool,
    /// Downloads whose popup is owed as soon as `main_ready` flips.
    pending: Vec<u64>,
    /// Popup labels whose script has reported in via `dialog_ready`. That is
    /// the only proof a popup is really showing something: WebView2 reports a
    /// navigation as finished whether or not it succeeded.
    loaded: std::collections::HashSet<String>,
}

fn popups() -> &'static std::sync::Mutex<Popups> {
    static POPUPS: std::sync::OnceLock<std::sync::Mutex<Popups>> = std::sync::OnceLock::new();
    POPUPS.get_or_init(|| {
        std::sync::Mutex::new(Popups {
            main_ready: false,
            pending: Vec::new(),
            loaded: std::collections::HashSet::new(),
        })
    })
}

fn dialog_label(id: u64) -> String {
    format!("download-dialog-{}", id)
}

/// How long a popup gets to load before it is reloaded, and then closed.
const POPUP_LOAD_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// The main window has a page, so WebView2 is up and popups may open. Called
/// from the main window's page-load hook, and from a timer in case that hook
/// never fires. Opens every popup that was waiting.
pub fn main_webview_ready(app: &tauri::AppHandle) {
    let waiting = {
        let mut p = popups().lock().unwrap();
        if p.main_ready {
            return;
        }
        p.main_ready = true;
        std::mem::take(&mut p.pending)
    };
    if waiting.is_empty() {
        return;
    }
    // Off this thread, not in it: this is reached from inside the main
    // webview's own page-load callback, and creating another webview from
    // within a WebView2 event handler is exactly the kind of re-entrancy that
    // leaves a window blank. Let the callback return first.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        for id in waiting {
            open_download_dialog(&app, id);
        }
    });
}

/// The popup's script is running. Its first act is to call this, which is what
/// the watchdog in [`open_download_dialog`] waits for.
#[tauri::command]
pub fn dialog_ready(window: tauri::WebviewWindow) {
    popups()
        .lock()
        .unwrap()
        .loaded
        .insert(window.label().to_string());
}

/// The download is gone, so its popup goes too — from here, not from the page's
/// own script, which may never have run.
pub fn close_download_dialog(app: &tauri::AppHandle, id: u64) {
    use tauri::Manager as _;
    let label = dialog_label(id);
    {
        let mut p = popups().lock().unwrap();
        p.pending.retain(|x| *x != id);
        p.loaded.remove(&label);
    }
    if let Some(win) = app.get_webview_window(&label) {
        let _ = win.close();
    }
}

pub fn open_download_dialog(app: &tauri::AppHandle, id: u64) {
    use tauri::Manager as _;
    let label = dialog_label(id);

    {
        let mut p = popups().lock().unwrap();
        if !p.main_ready {
            if !p.pending.contains(&id) {
                p.pending.push(id);
            }
            return;
        }
    }

    if let Some(win) = app.get_webview_window(&label) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        return;
    }

    // A previous popup for this download may have reported in and then been
    // closed by the user; its label must not vouch for the new one.
    popups().lock().unwrap().loaded.remove(&label);

    let url = tauri::WebviewUrl::App(format!("download_dialog.html?id={}", id).into());
    let built = tauri::WebviewWindowBuilder::new(app, &label, url)
        .title("FDM — Download Progress")
        .inner_size(560.0, 310.0)
        .min_inner_size(460.0, 240.0)
        .resizable(true)
        .center()
        .always_on_top(true)
        .decorations(false)
        // The app's own background, so a popup that is still loading is a dark
        // card rather than a white rectangle.
        .background_color(tauri::webview::Color(2, 6, 23, 255))
        .build();

    let Ok(win) = built else {
        return;
    };
    let _ = win.show();
    let _ = win.set_focus();

    // Watch it come up. A popup whose script has not reported in after the
    // grace period is reloaded once; one that still has not is closed. Either
    // beats a blank window sitting on top of everything.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let loaded = || popups().lock().unwrap().loaded.contains(&label);

        tokio::time::sleep(POPUP_LOAD_GRACE).await;
        if loaded() {
            return;
        }
        let Some(win) = app.get_webview_window(&label) else {
            return;
        };
        tracing::warn!(%label, "download popup did not load; reloading it");
        let _ = win.reload();

        tokio::time::sleep(POPUP_LOAD_GRACE).await;
        if loaded() {
            return;
        }
        if let Some(win) = app.get_webview_window(&label) {
            tracing::warn!(%label, "download popup still blank after a reload; closing it");
            let _ = win.close();
        }
    });
}

#[tauri::command]
pub fn open_download_dialog_cmd(app: tauri::AppHandle, id: fdm_manager::DownloadId) -> Result<(), String> {
    open_download_dialog(&app, id);
    Ok(())
}
