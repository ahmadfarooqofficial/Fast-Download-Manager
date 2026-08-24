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
    Ok(manager.add(new_download))
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
    }
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
        let p = std::path::Path::new(&path);
        if p.exists() {
            std::process::Command::new("explorer")
                .args(["/select,", &path])
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
    window.close().map_err(|e| e.to_string())
}
