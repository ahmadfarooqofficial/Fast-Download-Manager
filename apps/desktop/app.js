// ==========================================================================
// FDM — Fast Download Manager · Desktop UI Logic
// ==========================================================================

const tauri = window.__TAURI__ || window.__TAURI_INTERNALS__ || {};
const invoke = (tauri.core && tauri.core.invoke) || tauri.invoke || (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke);
const listen = (tauri.event && tauri.event.listen) || tauri.listen || (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.listen);

// --------------------------------------------------------------- State
let downloads = [];
let activeCategory = 'all';
let searchQuery = '';

// ------------------------------------------------------------- Formatters
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
  if (seconds === null || seconds === undefined || isNaN(seconds) || seconds < 0) {
    return '--:--:--';
  }
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  if (h > 0) {
    return `${h}h ${m}m ${s}s`;
  }
  return `${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
}

function escapeHtml(str) {
  if (!str) return '';
  return String(str)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

// --------------------------------------------------------- Category Filtering
function matchesCategory(d, category) {
  const status = (d.status || '').toLowerCase();
  const cat = (d.category || '').toLowerCase();

  switch (category) {
    case 'all':
      return true;
    case 'active':
      return ['queued', 'connecting', 'downloading'].includes(status);
    case 'completed':
      return status === 'completed';
    case 'paused':
      return ['paused', 'failed', 'cancelled'].includes(status);
    case 'video':
    case 'music':
    case 'documents':
    case 'programs':
    case 'compressed':
    case 'other':
      return cat === category;
    default:
      return true;
  }
}

function updateCategoryCounts() {
  const counts = {
    all: downloads.length,
    active: 0,
    completed: 0,
    paused: 0,
    video: 0,
    music: 0,
    documents: 0,
    programs: 0,
    compressed: 0,
    other: 0,
  };

  downloads.forEach(d => {
    const s = (d.status || '').toLowerCase();
    const c = (d.category || 'other').toLowerCase();

    if (['queued', 'connecting', 'downloading'].includes(s)) counts.active++;
    if (s === 'completed') counts.completed++;
    if (['paused', 'failed', 'cancelled'].includes(s)) counts.paused++;

    if (counts[c] !== undefined) {
      counts[c]++;
    } else {
      counts.other++;
    }
  });

  Object.keys(counts).forEach(key => {
    const el = document.getElementById(`count-${key}`);
    if (el) el.textContent = counts[key];
  });
}

// ------------------------------------------------------------- UI Rendering
const listEl = document.getElementById('download-list');
const emptyEl = document.getElementById('empty-state');

function render() {
  updateCategoryCounts();

  const filtered = downloads.filter(d => {
    if (!matchesCategory(d, activeCategory)) return false;
    if (searchQuery) {
      const q = searchQuery.toLowerCase();
      const name = (d.filename || '').toLowerCase();
      const url = (d.url || '').toLowerCase();
      return name.includes(q) || url.includes(q);
    }
    return true;
  });

  if (filtered.length === 0) {
    listEl.style.display = 'none';
    emptyEl.classList.add('visible');
  } else {
    listEl.style.display = 'flex';
    emptyEl.classList.remove('visible');
  }

  listEl.innerHTML = filtered.map(d => {
    const total = d.total || 0;
    const downloaded = d.downloaded || 0;
    const percent = total > 0 ? ((downloaded / total) * 100).toFixed(1) : 0;
    const status = (d.status || 'queued').toLowerCase();
    const segments = d.segments || 1;
    const activeConns = d.active_connections || 0;

    let statusText = status.charAt(0).toUpperCase() + status.slice(1);
    if (status === 'downloading') {
      statusText = `${formatSpeed(d.speed_bps)} · ETA ${formatTime(d.eta_secs)} · ${activeConns}/${segments} conns`;
    } else if (status === 'failed') {
      statusText = d.error ? `Failed: ${escapeHtml(d.error)}` : 'Failed';
    }

    // Action buttons
    const actions = [];
    if (['queued', 'connecting', 'downloading'].includes(status)) {
      actions.push(`<button onclick="window.fdm.pause(${d.id})">Pause</button>`);
      actions.push(`<button class="btn-danger" onclick="window.fdm.cancel(${d.id})">Cancel</button>`);
    } else if (['paused', 'failed', 'cancelled'].includes(status)) {
      actions.push(`<button onclick="window.fdm.resume(${d.id})">Resume</button>`);
      actions.push(`<button class="btn-danger" onclick="window.fdm.remove(${d.id}, true)">Delete</button>`);
    } else if (status === 'completed') {
      if (d.path) {
        actions.push(`<button onclick="window.fdm.openFile('${escapeHtml(d.path)}')">Open File</button>`);
        actions.push(`<button onclick="window.fdm.openFolder('${escapeHtml(d.path)}')">Open Folder</button>`);
      }
      actions.push(`<button class="btn-danger" onclick="window.fdm.remove(${d.id}, false)">Remove</button>`);
    }

    return `
      <div class="download-row" data-id="${d.id}" ondblclick="window.fdm.openDialog(${d.id})" title="Double click to open download window">
        <div class="row-top">
          <div class="file-info">
            <div class="file-icon">
              <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"></path>
                <polyline points="13 2 13 9 20 9"></polyline>
              </svg>
            </div>
            <div class="file-meta">
              <div class="filename" title="${escapeHtml(d.filename)}">${escapeHtml(d.filename || 'Resolving name...')}</div>
              <div class="file-url" title="${escapeHtml(d.url)}">${escapeHtml(d.url)}</div>
            </div>
          </div>
          <div class="row-actions">
            ${actions.join('')}
          </div>
        </div>

        <div class="fdm-progress" data-state="${status}" style="--fdm-value: ${percent}; --fdm-segments: ${segments};">
          <div class="fdm-progress__fill"></div>
        </div>

        <div class="row-bottom">
          <div class="status-badge">
            <span class="status-dot ${status}"></span>
            <span class="status-label">${statusText}</span>
          </div>
          <div class="row-stats fdm-num">
            <span>${percent}%</span>
            <span>${formatBytes(downloaded)} of ${total > 0 ? formatBytes(total) : 'Unknown'}</span>
          </div>
        </div>
      </div>
    `;
  }).join('');
}

// ------------------------------------------------------------- Global Handlers
window.fdm = {
  openDialog: (id) => invoke('open_download_dialog_cmd', { id }).catch(console.error),
  pause: (id) => invoke('pause_download', { id }).catch(console.error),
  resume: (id) => invoke('resume_download', { id }).catch(console.error),
  cancel: (id) => invoke('cancel_download', { id }).catch(console.error),
  remove: (id, deleteFile) => invoke('remove_download', { id, deleteFile }).catch(console.error),
  openFile: (path) => invoke('open_file', { path }).catch(console.error),
  openFolder: (path) => invoke('open_folder', { path }).catch(console.error),
};

// ------------------------------------------------------------- Toolbar Actions
document.getElementById('btn-add').addEventListener('click', () => {
  document.getElementById('add-url').value = '';
  document.getElementById('add-dialog').showModal();
});

document.getElementById('btn-empty-add').addEventListener('click', () => {
  document.getElementById('add-url').value = '';
  document.getElementById('add-dialog').showModal();
});

document.getElementById('btn-add-close').addEventListener('click', () => {
  document.getElementById('add-dialog').close();
});

document.getElementById('btn-add-cancel').addEventListener('click', () => {
  document.getElementById('add-dialog').close();
});

document.getElementById('add-form').addEventListener('submit', (e) => {
  e.preventDefault();
  const url = document.getElementById('add-url').value.trim();
  if (url) {
    invoke('add_download', { url, headers: {} }).catch(err => {
      alert('Failed to add download: ' + err);
    });
    document.getElementById('add-dialog').close();
  }
});

document.getElementById('btn-pause-all').addEventListener('click', () => {
  invoke('pause_all').catch(console.error);
});

document.getElementById('btn-resume-all').addEventListener('click', () => {
  invoke('resume_all').catch(console.error);
});

document.getElementById('btn-clear').addEventListener('click', () => {
  invoke('clear_finished').catch(console.error);
});

// Search input
document.getElementById('search-input').addEventListener('input', (e) => {
  searchQuery = e.target.value.trim();
  render();
});

// Category navigation
document.querySelectorAll('.nav-item').forEach(btn => {
  btn.addEventListener('click', () => {
    document.querySelectorAll('.nav-item').forEach(b => b.classList.remove('active'));
    btn.classList.add('active');
    activeCategory = btn.dataset.category || 'all';
    render();
  });
});

// Settings Modal
const settingsDialog = document.getElementById('settings-dialog');
const cfgMaxActive = document.getElementById('cfg-max-active');
const cfgMaxConn = document.getElementById('cfg-max-conn');

async function applySettings() {
  const maxActive = parseInt(cfgMaxActive.value, 10) || 4;
  const maxConnections = parseInt(cfgMaxConn.value, 10) || 32;
  localStorage.setItem('fdm_max_active', String(maxActive));
  localStorage.setItem('fdm_max_connections', String(maxConnections));
  try {
    await invoke('update_config', { maxActive, maxConnections });
  } catch (err) {
    console.error('Failed to update config:', err);
  }
}

document.getElementById('btn-open-settings').addEventListener('click', async () => {
  try {
    const cfg = await invoke('get_config');
    document.getElementById('cfg-download-root').textContent = cfg.downloadRoot || '—';
    document.getElementById('cfg-temp-dir').textContent = cfg.tempDir || '—';
    if (cfg.maxActive) cfgMaxActive.value = String(cfg.maxActive);
    if (cfg.maxConnections) cfgMaxConn.value = String(cfg.maxConnections);
  } catch (err) {
    console.error('Failed to load settings:', err);
  }
  settingsDialog.showModal();
});

cfgMaxActive?.addEventListener('change', applySettings);
cfgMaxConn?.addEventListener('change', applySettings);

document.getElementById('btn-settings-close').addEventListener('click', () => {
  applySettings();
  settingsDialog.close();
});
document.getElementById('btn-settings-ok').addEventListener('click', () => {
  applySettings();
  settingsDialog.close();
});

// Window controls
document.getElementById('titlebar-minimize')?.addEventListener('click', () => {
  invoke('minimize_window').catch(console.error);
});
document.getElementById('titlebar-maximize')?.addEventListener('click', () => {
  invoke('toggle_maximize_window').catch(console.error);
});
document.getElementById('titlebar-close')?.addEventListener('click', () => {
  invoke('close_window').catch(console.error);
});

// ------------------------------------------------------------- Initialization
async function init() {
  // Restore saved performance settings
  const savedConns = parseInt(localStorage.getItem('fdm_max_connections'), 10);
  const savedActive = parseInt(localStorage.getItem('fdm_max_active'), 10);
  if (savedConns || savedActive) {
    invoke('update_config', {
      maxConnections: savedConns || undefined,
      maxActive: savedActive || undefined,
    }).catch(console.error);
  }

  async function refresh() {
    try {
      downloads = await invoke('list_downloads');
      render();
    } catch (err) {
      console.debug('Failed to load downloads:', err);
    }
  }

  await refresh();
  setInterval(refresh, 500);

  // Real-time event subscription
  await listen('download-event', (event) => {
    const payload = event.payload;
    if (!payload) return;

    const added = payload.added || payload.Added;
    const changed = payload.changed || payload.Changed;
    const removed = payload.removed !== undefined ? payload.removed : payload.Removed;

    if (added) {
      const existing = downloads.findIndex(d => d.id === added.id);
      if (existing === -1) {
        downloads.unshift(added);
      } else {
        downloads[existing] = added;
      }
    } else if (changed) {
      const idx = downloads.findIndex(d => d.id === changed.id);
      if (idx !== -1) {
        downloads[idx] = changed;
      } else {
        downloads.unshift(changed);
      }
    } else if (removed !== undefined) {
      downloads = downloads.filter(d => d.id !== removed);
    }
    render();
  });
}

init();
