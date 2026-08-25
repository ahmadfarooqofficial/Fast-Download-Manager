// ==========================================================================
// FDM — Fast Download Manager · Dedicated Download Popup Logic (IDM Style)
// ==========================================================================

const tauri = window.__TAURI__ || window.__TAURI_INTERNALS__ || {};
const invoke = (tauri.core && tauri.core.invoke) || tauri.invoke || (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke);
const listen = (tauri.event && tauri.event.listen) || tauri.listen || (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.listen);

// Extract download ID from URL query: download_dialog.html?id=123
const urlParams = new URLSearchParams(window.location.search);
const downloadId = parseInt(urlParams.get('id'), 10);

// Native window dragging on entire background & header
document.addEventListener('mousedown', (e) => {
  if (e.target.closest('button, a, input, select, textarea, .btn')) return;
  try {
    if (window.__TAURI_INTERNALS__?.invoke) {
      window.__TAURI_INTERNALS__.invoke('plugin:window|start_dragging');
    } else if (tauri.window?.getCurrentWindow) {
      tauri.window.getCurrentWindow().startDragging();
    }
  } catch (err) {}
});

let currentDownload = null;
let userStarted = false; // Controls prompt vs active downloading view

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

function getFileIcon(filename, category) {
  const name = (filename || '').toLowerCase();
  const cat = (category || '').toLowerCase();

  // Video (VLC/Media player style)
  if (cat === 'video' || name.endsWith('.mp4') || name.endsWith('.mkv') || name.endsWith('.webm') || name.endsWith('.avi') || name.endsWith('.mov') || name.endsWith('.flv') || name.endsWith('.ts')) {
    return `<svg width="28" height="28" viewBox="0 0 24 24" fill="none">
      <rect x="2" y="4" width="20" height="16" rx="3" fill="#ff4d4d" fill-opacity="0.15" stroke="#ff4d4d" stroke-width="1.8"/>
      <polygon points="10 8 16 12 10 16 10 8" fill="#ff4d4d"/>
      <circle cx="5" cy="7" r="1" fill="#ff4d4d"/><circle cx="5" cy="17" r="1" fill="#ff4d4d"/>
      <circle cx="19" cy="7" r="1" fill="#ff4d4d"/><circle cx="19" cy="17" r="1" fill="#ff4d4d"/>
    </svg>`;
  }

  // Audio / Music (Headphones & music notes)
  if (cat === 'music' || cat === 'audio' || name.endsWith('.mp3') || name.endsWith('.m4a') || name.endsWith('.wav') || name.endsWith('.flac') || name.endsWith('.aac') || name.endsWith('.ogg')) {
    return `<svg width="28" height="28" viewBox="0 0 24 24" fill="none">
      <rect x="2" y="3" width="20" height="18" rx="3" fill="#a855f7" fill-opacity="0.15" stroke="#a855f7" stroke-width="1.8"/>
      <path d="M9 18V5l10-2v13" stroke="#a855f7" stroke-width="1.8" stroke-linecap="round"/>
      <circle cx="6" cy="18" r="3" fill="#a855f7"/><circle cx="16" cy="16" r="3" fill="#a855f7"/>
    </svg>`;
  }

  // Compressed / Archives (Zip folder with zipper)
  if (cat === 'compressed' || name.endsWith('.zip') || name.endsWith('.rar') || name.endsWith('.7z') || name.endsWith('.tar') || name.endsWith('.gz') || name.endsWith('.iso')) {
    return `<svg width="28" height="28" viewBox="0 0 24 24" fill="none">
      <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" fill="#f59e0b" fill-opacity="0.15" stroke="#f59e0b" stroke-width="1.8"/>
      <line x1="12" y1="11" x2="12" y2="17" stroke="#f59e0b" stroke-width="2" stroke-dasharray="2 2"/>
      <rect x="10.5" y="14" width="3" height="4" rx="1" fill="#f59e0b"/>
    </svg>`;
  }

  // Programs / Executable (App gear & setup box)
  if (cat === 'programs' || name.endsWith('.exe') || name.endsWith('.msi') || name.endsWith('.bat') || name.endsWith('.cmd') || name.endsWith('.dmg')) {
    return `<svg width="28" height="28" viewBox="0 0 24 24" fill="none">
      <rect x="3" y="3" width="18" height="18" rx="3" fill="#10b981" fill-opacity="0.15" stroke="#10b981" stroke-width="1.8"/>
      <path d="M9 9h6v6H9z" fill="#10b981"/>
      <path d="M9 3v3M15 3v3M9 18v3M15 18v3M3 9h3M3 15h3M18 9h3M18 15h3" stroke="#10b981" stroke-width="1.8" stroke-linecap="round"/>
    </svg>`;
  }

  // Documents
  if (cat === 'documents' || name.endsWith('.pdf') || name.endsWith('.doc') || name.endsWith('.docx') || name.endsWith('.xls') || name.endsWith('.xlsx') || name.endsWith('.ppt') || name.endsWith('.txt')) {
    return `<svg width="28" height="28" viewBox="0 0 24 24" fill="none">
      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" fill="#3b82f6" fill-opacity="0.15" stroke="#3b82f6" stroke-width="1.8"/>
      <polyline points="14 2 14 8 20 8" stroke="#3b82f6" stroke-width="1.8"/>
      <line x1="8" y1="13" x2="16" y2="13" stroke="#3b82f6" stroke-width="1.8" stroke-linecap="round"/>
      <line x1="8" y1="17" x2="13" y2="17" stroke="#3b82f6" stroke-width="1.8" stroke-linecap="round"/>
    </svg>`;
  }

  // Default File Icon
  return `<svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="#e50914" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
    <path d="M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"></path>
    <polyline points="13 2 13 9 20 9"></polyline>
  </svg>`;
}

const el = {
  viewPrompt: document.getElementById('view-prompt'),
  viewActive: document.getElementById('view-active'),
  viewCompleted: document.getElementById('view-completed'),
  titleText: document.getElementById('dialog-title-text'),

  // Prompt view
  promptUrl: document.getElementById('prompt-url'),
  promptCategory: document.getElementById('prompt-category'),
  promptFilename: document.getElementById('prompt-filename'),
  promptPath: document.getElementById('prompt-path'),
  promptBtnStart: document.getElementById('prompt-btn-start'),
  promptBtnLater: document.getElementById('prompt-btn-later'),
  promptBtnCancel: document.getElementById('prompt-btn-cancel'),

  // Active view
  fileIcon: document.getElementById('dlg-file-icon'),
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
    el.viewPrompt.style.display = 'none';
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

  // Populate prompt fields
  if (el.promptUrl) el.promptUrl.value = d.url || '';
  if (el.promptFilename) el.promptFilename.value = d.filename || '';
  if (el.promptCategory) el.promptCategory.textContent = (d.category || 'Video').toUpperCase();
  if (el.promptPath) el.promptPath.value = d.path || 'Downloads folder';

  // If user hasn't clicked Start Download yet and the download is in initial state
  if (!userStarted && (d.downloaded === 0 || d.status === 'starting' || d.status === 'connecting' || d.status === 'queued')) {
    el.viewPrompt.style.display = 'flex';
    el.viewActive.style.display = 'none';
    el.viewCompleted.style.display = 'none';
    el.titleText.textContent = 'Download File Info';
    return;
  }

  // Active / in-progress view
  el.viewPrompt.style.display = 'none';
  el.viewActive.style.display = 'flex';
  el.viewCompleted.style.display = 'none';
  el.titleText.textContent = 'Download Status';

  if (el.fileIcon) {
    el.fileIcon.innerHTML = getFileIcon(d.filename, d.category);
  }

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
    el.progressFill.style.background = 'var(--fdm-warning)';
    el.progressFill.classList.remove('shimmer');
    el.status.textContent = 'Paused';
    el.status.style.color = 'var(--fdm-warning)';
    el.speed.textContent = '0 B/s';
    el.btnPause.textContent = 'Resume';
    el.btnPause.className = 'btn btn-primary';
  } else if (d.status === 'failed') {
    el.progressFill.style.background = 'var(--fdm-red)';
    el.progressFill.classList.remove('shimmer');
    el.status.textContent = d.error ? `Failed: ${d.error}` : 'Failed';
    el.status.style.color = 'var(--fdm-red)';
    el.speed.textContent = '0 B/s';
    el.btnPause.textContent = 'Retry';
    el.btnPause.className = 'btn btn-primary';
  } else if (d.status === 'connecting' || d.status === 'queued' || d.status === 'starting' || (d.status === 'downloading' && downloaded === 0)) {
    // Connecting / resolving — show shimmer animation
    el.progressFill.style.width = '100%';
    el.progressFill.style.background = 'var(--fdm-surface-2)';
    el.progressFill.classList.add('shimmer');
    el.status.textContent = 'Connecting to server…';
    el.status.style.color = 'var(--fdm-info)';
    el.speed.textContent = '—';
    el.eta.textContent = '—';
    el.btnPause.textContent = 'Pause';
    el.btnPause.className = 'btn btn-secondary';
  } else {
    // downloading with actual progress
    el.progressFill.style.background = 'linear-gradient(90deg, #e50914 0%, #ff4b2b 50%, #2ecc71 100%)';
    el.progressFill.classList.remove('shimmer');
    el.status.textContent = `Downloading (${conns} connections)`;
    el.status.style.color = 'var(--fdm-info)';
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

// Prompt Action Buttons
el.promptBtnStart?.addEventListener('click', async () => {
  userStarted = true;
  if (currentDownload) {
    render(currentDownload);
    if (currentDownload.status === 'paused') {
      await invoke('resume_download', { id: downloadId });
    }
  }
});

el.promptBtnLater?.addEventListener('click', async () => {
  await invoke('pause_download', { id: downloadId });
  invoke('close_window').catch(console.error);
});

el.promptBtnCancel?.addEventListener('click', async () => {
  await invoke('cancel_download', { id: downloadId });
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
