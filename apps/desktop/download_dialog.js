// ==========================================================================
// FDM — Fast Download Manager · Dedicated Download Popup Logic (IDM Style)
// ==========================================================================

const tauri = window.__TAURI__ || window.__TAURI_INTERNALS__ || {};
const invoke = (tauri.core && tauri.core.invoke) || tauri.invoke || (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke);
const listen = (tauri.event && tauri.event.listen) || tauri.listen || (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.listen);

// Extract download ID from URL query: download_dialog.html?id=123
const urlParams = new URLSearchParams(window.location.search);
const downloadId = parseInt(urlParams.get('id'), 10);

let currentDownload = null;

// Formatters
function formatBytes(bytes) {
  if (!bytes || bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
}

function formatSpeed(bps) {
  if (!bps || bps <= 0) return '0 B/s';
  return formatBytes(bps) + '/s';
}

function formatTime(seconds) {
  if (seconds == null || seconds < 0 || !isFinite(seconds)) return '—';
  if (seconds === 0) return '0s';
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

const el = {
  viewActive: document.getElementById('view-active'),
  viewCompleted: document.getElementById('view-completed'),
  titleText: document.getElementById('dialog-title-text'),

  // Active view
  filename: document.getElementById('dlg-filename'),
  url: document.getElementById('dlg-url'),
  progressFill: document.getElementById('dlg-progress-fill'),
  progressPct: document.getElementById('dlg-progress-pct'),
  progressSegs: document.getElementById('dlg-progress-segs'),
  status: document.getElementById('dlg-status'),
  size: document.getElementById('dlg-size'),
  speed: document.getElementById('dlg-speed'),
  eta: document.getElementById('dlg-eta'),
  resume: document.getElementById('dlg-resume'),
  path: document.getElementById('dlg-path'),
  btnManager: document.getElementById('dlg-btn-manager'),
  btnFolder: document.getElementById('dlg-btn-folder'),
  btnPause: document.getElementById('dlg-btn-pause'),
  btnCancel: document.getElementById('dlg-btn-cancel'),

  // Completed view
  celebrateFilename: document.getElementById('celebrate-filename'),
  celebratePath: document.getElementById('celebrate-path'),
  celebrateSize: document.getElementById('celebrate-size'),
  celebrateBtnOpen: document.getElementById('celebrate-btn-open'),
  celebrateBtnClose: document.getElementById('celebrate-btn-close'),
  celebrateBtnManager: document.getElementById('celebrate-btn-manager'),

  // Header controls
  btnMin: document.getElementById('dialog-minimize'),
  btnClose: document.getElementById('dialog-close'),
};

function render(d) {
  if (!d) return;
  currentDownload = d;

  const total = d.total || 0;
  const downloaded = d.downloaded || 0;
  const pct = total > 0 ? (downloaded / total) * 100 : 0;
  const pctFormatted = pct.toFixed(1);

  // If completed, show celebration screen
  if (d.status === 'completed') {
    el.viewActive.style.display = 'none';
    el.viewCompleted.style.display = 'flex';
    el.titleText.textContent = 'Download Complete';

    el.celebrateFilename.textContent = d.filename || 'Downloaded file';
    el.celebrateFilename.title = d.filename || '';
    el.celebratePath.textContent = d.path || 'Downloads folder';
    el.celebratePath.title = d.path || '';
    el.celebrateSize.textContent = formatBytes(downloaded || total);
    return;
  }

  // Active / in-progress view
  el.viewActive.style.display = 'flex';
  el.viewCompleted.style.display = 'none';
  el.titleText.textContent = 'Download Status';

  el.filename.textContent = d.filename || 'Starting download…';
  el.filename.title = d.filename || '';
  el.url.textContent = d.url || '—';
  el.url.title = d.url || '';

  // Progress bar fill & labels
  el.progressFill.style.width = `${Math.min(100, Math.max(0, pct))}%`;
  el.progressPct.textContent = `${pctFormatted}%`;
  const conns = d.active_connections || d.activeConnections || d.segments || 32;
  el.progressSegs.textContent = `${conns} parallel streams`;

  if (d.status === 'paused') {
    el.progressFill.style.background = 'var(--fdm-orange)';
    el.status.textContent = 'Paused';
    el.status.style.color = 'var(--fdm-orange)';
    el.speed.textContent = '0 B/s';
    el.btnPause.textContent = 'Resume';
    el.btnPause.className = 'btn btn-primary';
  } else if (d.status === 'failed') {
    el.progressFill.style.background = 'var(--fdm-red)';
    el.status.textContent = d.error ? `Failed: ${d.error}` : 'Failed';
    el.status.style.color = 'var(--fdm-red)';
    el.speed.textContent = '0 B/s';
    el.btnPause.textContent = 'Retry';
    el.btnPause.className = 'btn btn-primary';
  } else if (d.status === 'connecting' || d.status === 'queued') {
    el.progressFill.style.background = 'var(--fdm-blue)';
    el.status.textContent = 'Connecting to server…';
    el.status.style.color = 'var(--fdm-blue)';
    el.btnPause.textContent = 'Pause';
    el.btnPause.className = 'btn btn-secondary';
  } else {
    // downloading
    el.progressFill.style.background = 'linear-gradient(90deg, #e50914 0%, #ff4b2b 50%, #2ecc71 100%)';
    el.status.textContent = `Downloading (${conns} connections)`;
    el.status.style.color = 'var(--fdm-blue)';
    el.speed.textContent = formatSpeed(d.speed_bps || d.speedBps);
    el.eta.textContent = formatTime(d.eta_secs || d.etaSecs);
    el.btnPause.textContent = 'Pause';
    el.btnPause.className = 'btn btn-secondary';
  }

  // Size details
  if (total > 0) {
    el.size.textContent = `${formatBytes(downloaded)} / ${formatBytes(total)} (${pctFormatted}%)`;
  } else {
    el.size.textContent = `${formatBytes(downloaded)} (calculating total...)`;
  }

  // Destination path
  if (d.path) {
    el.path.textContent = d.path;
    el.path.title = d.path;
  } else {
    el.path.textContent = 'Downloads folder';
  }

  el.resume.textContent = d.resumable !== false ? 'Yes' : 'No';
}

// Window controls
el.btnMin?.addEventListener('click', () => {
  invoke('minimize_window').catch(console.error);
});
el.btnClose?.addEventListener('click', () => {
  invoke('close_window').catch(console.error);
});

// Active Action buttons
el.btnPause?.addEventListener('click', async () => {
  if (!currentDownload) return;
  const st = (currentDownload.status || '').toLowerCase();
  if (st === 'paused' || st === 'failed') {
    el.btnPause.textContent = 'Starting...';
    await invoke('resume_download', { id: downloadId });
  } else {
    el.btnPause.textContent = 'Pausing...';
    await invoke('pause_download', { id: downloadId });
  }
});

el.btnCancel?.addEventListener('click', async () => {
  if (currentDownload && currentDownload.status !== 'completed') {
    await invoke('cancel_download', { id: downloadId });
  }
  invoke('close_window').catch(console.error);
});

el.btnFolder?.addEventListener('click', async () => {
  if (currentDownload?.path) {
    await invoke('open_folder', { path: currentDownload.path });
  }
});

el.btnManager?.addEventListener('click', async () => {
  await invoke('show_main_window');
});

// Completed / Celebration Action buttons
el.celebrateBtnOpen?.addEventListener('click', async () => {
  if (currentDownload?.path) {
    await invoke('open_file', { path: currentDownload.path });
  }
});

el.celebrateBtnClose?.addEventListener('click', () => {
  invoke('close_window').catch(console.error);
});

el.celebrateBtnManager?.addEventListener('click', async () => {
  await invoke('show_main_window');
});

// Initialization & Live Sync
async function init() {
  async function refresh() {
    if (!isNaN(downloadId)) {
      try {
        const d = await invoke('get_download', { id: downloadId });
        if (d) render(d);
      } catch (err) {
        console.debug('Failed to get download:', err);
      }
    }
  }

  await refresh();
  const pollInterval = setInterval(refresh, 200);

  // Real-time events
  await listen('download-event', (event) => {
    const payload = event.payload;
    if (!payload) return;

    const added = payload.added || payload.Added;
    const changed = payload.changed || payload.Changed;
    const removed = payload.removed !== undefined ? payload.removed : payload.Removed;

    if (added && added.id === downloadId) {
      render(added);
    } else if (changed && changed.id === downloadId) {
      render(changed);
    } else if (removed === downloadId) {
      clearInterval(pollInterval);
      invoke('close_window').catch(console.error);
    }
  });
}

init();
