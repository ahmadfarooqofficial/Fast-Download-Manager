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
  filename: document.getElementById('dlg-filename'),
  url: document.getElementById('dlg-url'),
  bar: document.getElementById('dlg-progress-bar'),
  status: document.getElementById('dlg-status'),
  size: document.getElementById('dlg-size'),
  speed: document.getElementById('dlg-speed'),
  eta: document.getElementById('dlg-eta'),
  resume: document.getElementById('dlg-resume'),
  path: document.getElementById('dlg-path'),
  btnManager: document.getElementById('dlg-btn-manager'),
  btnFolder: document.getElementById('dlg-btn-folder'),
  btnOpen: document.getElementById('dlg-btn-open'),
  btnPause: document.getElementById('dlg-btn-pause'),
  btnCancel: document.getElementById('dlg-btn-cancel'),
  btnMin: document.getElementById('dialog-minimize'),
  btnClose: document.getElementById('dialog-close'),
};

function render(d) {
  if (!d) return;
  currentDownload = d;

  el.filename.textContent = d.filename || 'Starting download…';
  el.filename.title = d.filename || '';
  el.url.textContent = d.url || '—';
  el.url.title = d.url || '';

  const total = d.total || 0;
  const downloaded = d.downloaded || 0;
  const pct = total > 0 ? (downloaded / total) * 100 : 0;

  el.bar.style.setProperty('--fdm-value', pct.toFixed(1));
  el.bar.style.setProperty('--fdm-segments', String(Math.max(1, d.segments || 8)));

  if (d.status === 'completed') {
    el.bar.dataset.state = 'completed';
    el.status.textContent = 'Complete';
    el.status.style.color = 'var(--fdm-green)';
    el.speed.textContent = '—';
    el.eta.textContent = '0s';
    el.btnOpen.style.display = 'inline-flex';
    el.btnPause.style.display = 'none';
    el.btnCancel.textContent = 'Close';
  } else if (d.status === 'paused') {
    el.bar.dataset.state = 'paused';
    el.status.textContent = 'Paused';
    el.status.style.color = 'var(--fdm-orange)';
    el.speed.textContent = '0 B/s';
    el.btnPause.textContent = 'Resume';
    el.btnPause.style.display = 'inline-flex';
  } else if (d.status === 'failed') {
    el.bar.dataset.state = 'failed';
    el.status.textContent = d.error ? `Failed: ${d.error}` : 'Failed';
    el.status.style.color = 'var(--fdm-red)';
    el.speed.textContent = '0 B/s';
    el.btnPause.textContent = 'Retry';
    el.btnPause.style.display = 'inline-flex';
  } else if (d.status === 'connecting' || d.status === 'queued') {
    el.bar.dataset.state = 'running';
    el.status.textContent = 'Connecting…';
    el.status.style.color = 'var(--fdm-fg-muted)';
    el.btnPause.textContent = 'Pause';
    el.btnPause.style.display = 'inline-flex';
  } else {
    // downloading
    el.bar.dataset.state = 'running';
    const conns = d.activeConnections || d.segments || 8;
    el.status.textContent = `Downloading (${conns} connections)`;
    el.status.style.color = 'var(--fdm-blue)';
    el.speed.textContent = formatSpeed(d.speedBps);
    el.eta.textContent = formatTime(d.etaSecs);
    el.btnPause.textContent = 'Pause';
    el.btnPause.style.display = 'inline-flex';
  }

  // Size details
  if (total > 0) {
    el.size.textContent = `${formatBytes(downloaded)} / ${formatBytes(total)} (${pct.toFixed(1)}%)`;
  } else {
    el.size.textContent = `${formatBytes(downloaded)} (unknown total)`;
  }

  // Destination path
  if (d.path) {
    el.path.textContent = d.path;
    el.path.title = d.path;
    el.btnFolder.disabled = false;
  } else {
    el.path.textContent = 'Default Downloads folder';
    el.btnFolder.disabled = true;
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

// Action buttons
el.btnPause?.addEventListener('click', async () => {
  if (!currentDownload) return;
  if (currentDownload.status === 'paused' || currentDownload.status === 'failed') {
    await invoke('resume_download', { id: downloadId });
  } else {
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

el.btnOpen?.addEventListener('click', async () => {
  if (currentDownload?.path) {
    await invoke('open_file', { path: currentDownload.path });
  }
});

el.btnManager?.addEventListener('click', async () => {
  await invoke('show_main_window');
});

// Initialization
async function init() {
  if (!isNaN(downloadId)) {
    try {
      const d = await invoke('get_download', { id: downloadId });
      if (d) render(d);
    } catch (err) {
      console.error('Failed to get download:', err);
    }
  }

  // Real-time updates
  await listen('download-event', (event) => {
    const payload = event.payload;
    if (!payload) return;

    if (payload.Added && payload.Added.id === downloadId) {
      render(payload.Added);
    } else if (payload.Changed && payload.Changed.id === downloadId) {
      render(payload.Changed);
    } else if (payload.Removed === downloadId) {
      invoke('close_window').catch(console.error);
    }
  });
}

init();
