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

function t(key, vars) {
  return window.fdmI18n ? window.fdmI18n.t(key, vars) : key;
}

function statusLabelText(status, d) {
  // A video extraction reports which step it is on before any byte moves.
  // Showing it is the difference between a progress bar that looks stuck and
  // one that is visibly working. Only while it is actually running, though —
  // a stage left over from a since-paused download would be a lie.
  if (d.stage && !d.downloaded && ['queued', 'connecting', 'downloading'].includes(status)) {
    return t(`stage_${d.stage}`);
  }
  if (status === 'downloading') {
    const segments = d.segments || 1;
    const activeConns = d.active_connections || 0;
    return `${formatSpeed(d.speed_bps)} · ETA ${formatTime(d.eta_secs)} · ${activeConns}/${segments} conns`;
  }
  if (status === 'failed') {
    return d.error ? `${t('row_failed_prefix')}${escapeHtml(d.error)}` : t('status_failed');
  }
  const known = ['queued', 'connecting', 'paused', 'completed', 'cancelled'];
  return known.includes(status) ? t(`status_${status}`) : status.charAt(0).toUpperCase() + status.slice(1);
}

function getRowHtml(d) {
  const total = d.total || 0;
  const downloaded = d.downloaded || 0;
  const percent = total > 0 ? ((downloaded / total) * 100).toFixed(1) : 0;
  const status = (d.status || 'queued').toLowerCase();
  const segments = d.segments || 1;

  const statusText = statusLabelText(status, d);

  const actions = [];
  if (['queued', 'connecting', 'downloading'].includes(status)) {
    actions.push(`<button onclick="window.fdm.pause(${d.id})">${t('action_pause')}</button>`);
    actions.push(`<button class="btn-danger" onclick="window.fdm.cancel(${d.id})">${t('action_cancel')}</button>`);
  } else if (['paused', 'failed', 'cancelled'].includes(status)) {
    actions.push(`<button onclick="window.fdm.resume(${d.id})">${t('action_resume')}</button>`);
    actions.push(`<button class="btn-danger" onclick="window.fdm.remove(${d.id}, true)">${t('action_delete')}</button>`);
  } else if (status === 'completed') {
    if (d.path) {
      actions.push(`<button onclick="window.fdm.openFile('${escapeHtml(d.path)}')">${t('action_open_file')}</button>`);
      actions.push(`<button onclick="window.fdm.openFolder('${escapeHtml(d.path)}')">${t('action_open_folder')}</button>`);
    }
    actions.push(`<button class="btn-danger" onclick="window.fdm.remove(${d.id}, false)">${t('action_remove')}</button>`);
  }

  return `
    <div class="row-top">
      <div class="file-info">
        <div class="file-icon">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"></path>
            <polyline points="13 2 13 9 20 9"></polyline>
          </svg>
        </div>
        <div class="file-meta">
          <div class="filename" title="${escapeHtml(d.filename)}">${escapeHtml(d.filename || t('row_resolving_name'))}</div>
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
        <span class="stat-pct">${percent}%</span>
        <span class="stat-bytes">${formatBytes(downloaded)} ${t('row_of')} ${total > 0 ? formatBytes(total) : t('status_unknown')}</span>
      </div>
    </div>
  `;
}

function updateRowInPlace(rowEl, d) {
  const total = d.total || 0;
  const downloaded = d.downloaded || 0;
  const percent = total > 0 ? ((downloaded / total) * 100).toFixed(1) : 0;
  const status = (d.status || 'queued').toLowerCase();
  const segments = d.segments || 1;

  const currentStatus = rowEl.dataset.status;
  if (currentStatus !== status) {
    rowEl.dataset.status = status;
    rowEl.innerHTML = getRowHtml(d);
    return;
  }

  const prog = rowEl.querySelector('.fdm-progress');
  if (prog) {
    prog.style.setProperty('--fdm-value', String(percent));
    prog.style.setProperty('--fdm-segments', String(segments));
    prog.dataset.state = status;
  }

  const statusLabel = rowEl.querySelector('.status-label');
  if (statusLabel) {
    statusLabel.textContent = statusLabelText(status, d);
  }

  const pctEl = rowEl.querySelector('.stat-pct');
  if (pctEl) pctEl.textContent = `${percent}%`;

  const bytesEl = rowEl.querySelector('.stat-bytes');
  if (bytesEl) bytesEl.textContent = `${formatBytes(downloaded)} ${t('row_of')} ${total > 0 ? formatBytes(total) : t('status_unknown')}`;

  const fnEl = rowEl.querySelector('.filename');
  if (fnEl && d.filename && fnEl.textContent !== d.filename) {
    fnEl.textContent = d.filename;
    fnEl.title = d.filename;
  }
}

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
    listEl.innerHTML = '';
    return;
  }

  listEl.style.display = 'flex';
  emptyEl.classList.remove('visible');

  // Fast in-place DOM sync
  const existingRows = new Map();
  listEl.querySelectorAll('.download-row').forEach(el => {
    const id = parseInt(el.dataset.id, 10);
    if (!isNaN(id)) existingRows.set(id, el);
  });

  const currentIds = Array.from(existingRows.keys());
  const filteredIds = filtered.map(d => d.id);
  const sameIds = currentIds.length === filteredIds.length && currentIds.every((id, idx) => id === filteredIds[idx]);

  if (sameIds) {
    for (const d of filtered) {
      const rowEl = existingRows.get(d.id);
      if (rowEl) updateRowInPlace(rowEl, d);
    }
  } else {
    listEl.innerHTML = filtered.map(d => `
      <div class="download-row" data-id="${d.id}" data-status="${(d.status || 'queued').toLowerCase()}" ondblclick="window.fdm.openDialog(${d.id})" title="Double click to open download window">
        ${getRowHtml(d)}
      </div>
    `).join('');
  }
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
      alert(t('alert_add_failed') + err);
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
  const downloadRoot = document.getElementById('cfg-download-root')?.textContent;
  const tempDir = document.getElementById('cfg-temp-dir')?.textContent;

  localStorage.setItem('fdm_max_active', String(maxActive));
  localStorage.setItem('fdm_max_connections', String(maxConnections));
  // The engine holds these in memory only, so the UI is what remembers them
  // across restarts — same arrangement the connection counts already use.
  if (downloadRoot && downloadRoot !== '—') localStorage.setItem('fdm_download_root', downloadRoot);
  if (tempDir && tempDir !== '—') localStorage.setItem('fdm_temp_dir', tempDir);
  localStorage.setItem('fdm_category_dirs', JSON.stringify(categoryDirs));

  try {
    await invoke('update_config', {
      maxActive,
      maxConnections,
      downloadRoot: downloadRoot && downloadRoot !== '—' ? downloadRoot : undefined,
      tempDir: tempDir && tempDir !== '—' ? tempDir : undefined,
      categoryDirs,
    });
  } catch (err) {
    console.error('Failed to update config:', err);
    alert(t('alert_folder_failed') + err);
  }
}

const cfgLanguage = document.getElementById('cfg-language');
const cfgCategoryList = document.getElementById('cfg-category-list');

// Native folder picker. The dialog plugin's JS wrapper is an npm package and
// this app has no bundler, so call the plugin command directly — the same way
// the window drag region does.
async function pickFolder(defaultPath) {
  try {
    const picked = await invoke('plugin:dialog|open', {
      options: { directory: true, multiple: false, recursive: false, defaultPath: defaultPath || undefined },
    });
    // The plugin returns null when the user cancels, and (depending on version)
    // either a string or a {path} record when they choose.
    if (!picked) return null;
    if (typeof picked === 'string') return picked;
    if (Array.isArray(picked)) return picked[0]?.path || picked[0] || null;
    return picked.path || null;
  } catch (err) {
    console.error('Folder picker failed:', err);
    return null;
  }
}

// Category overrides live here between opening the dialog and applying it, so
// the whole set can be sent at once — an override the user cleared has to be
// absent from the map, not merely empty.
let categoryDirs = {};

function renderCategoryRows(categories) {
  if (!cfgCategoryList) return;
  cfgCategoryList.innerHTML = categories
    .map((name) => {
      const chosen = categoryDirs[name];
      const label = chosen || t('settings_category_default');
      return `
        <div class="category-row" data-category="${escapeHtml(name)}">
          <span class="category-name">${escapeHtml(t(`nav_${name.toLowerCase()}`))}</span>
          <div class="category-path ${chosen ? '' : 'is-default'}" title="${escapeHtml(label)}">${escapeHtml(label)}</div>
          <button type="button" class="btn-browse" data-action="pick">📂</button>
          <button type="button" class="btn-browse" data-action="clear" ${chosen ? '' : 'disabled'}>✕</button>
        </div>`;
    })
    .join('');
}

cfgCategoryList?.addEventListener('click', async (e) => {
  const btn = e.target.closest('button[data-action]');
  if (!btn) return;
  const row = btn.closest('.category-row');
  const name = row?.dataset.category;
  if (!name) return;

  if (btn.dataset.action === 'clear') {
    delete categoryDirs[name];
  } else {
    const dir = await pickFolder(categoryDirs[name]);
    if (!dir) return;
    categoryDirs[name] = dir;
  }
  renderCategoryRows(currentCategories);
  await applySettings();
});

let currentCategories = [];

document.getElementById('btn-browse-root')?.addEventListener('click', async () => {
  const el = document.getElementById('cfg-download-root');
  const dir = await pickFolder(el.textContent);
  if (!dir) return;
  el.textContent = dir;
  await applySettings();
});

document.getElementById('btn-browse-temp')?.addEventListener('click', async () => {
  const el = document.getElementById('cfg-temp-dir');
  const dir = await pickFolder(el.textContent);
  if (!dir) return;
  el.textContent = dir;
  await applySettings();
});

document.getElementById('btn-open-settings').addEventListener('click', async () => {
  try {
    const cfg = await invoke('get_config');
    document.getElementById('cfg-download-root').textContent = cfg.downloadRoot || '—';
    document.getElementById('cfg-temp-dir').textContent = cfg.tempDir || '—';
    if (cfg.maxActive) cfgMaxActive.value = String(cfg.maxActive);
    if (cfg.maxConnections) cfgMaxConn.value = String(cfg.maxConnections);
    categoryDirs = cfg.categoryDirs || {};
    currentCategories = cfg.categories || [];
    renderCategoryRows(currentCategories);
  } catch (err) {
    console.error('Failed to load settings:', err);
  }
  window.fdmI18n?.populateSelect(cfgLanguage);
  settingsDialog.showModal();
});

cfgMaxActive?.addEventListener('change', applySettings);
cfgMaxConn?.addEventListener('change', applySettings);
cfgLanguage?.addEventListener('change', () => {
  window.fdmI18n?.setLanguage(cfgLanguage.value);
});

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

// ------------------------------------------------------------- Drag & Drop
// Dropping a link (dragged from a browser tab, address bar, etc.) anywhere on
// the window starts a download, the same way pasting it into the Add dialog
// does. Tauri/WebView2 would otherwise navigate the window to the dropped URL,
// so both dragover and drop must call preventDefault().
const dropOverlay = document.getElementById('drop-overlay');
let dragDepth = 0;

function extractUrl(dataTransfer) {
  const uriList = dataTransfer.getData('text/uri-list');
  const plain = dataTransfer.getData('text/plain');
  const candidate = (uriList || plain || '').split('\n').find(line => line && !line.startsWith('#'));
  if (!candidate) return null;
  try {
    const url = new URL(candidate.trim());
    if (url.protocol === 'http:' || url.protocol === 'https:') {
      return url.toString();
    }
  } catch (_err) {
    // Not a URL — ignore.
  }
  return null;
}

document.addEventListener('dragenter', (e) => {
  e.preventDefault();
  dragDepth++;
  dropOverlay?.classList.add('is-active');
});

document.addEventListener('dragover', (e) => {
  e.preventDefault();
  if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
});

document.addEventListener('dragleave', (e) => {
  e.preventDefault();
  dragDepth = Math.max(0, dragDepth - 1);
  if (dragDepth === 0) dropOverlay?.classList.remove('is-active');
});

document.addEventListener('drop', (e) => {
  e.preventDefault();
  dragDepth = 0;
  dropOverlay?.classList.remove('is-active');

  const url = extractUrl(e.dataTransfer);
  if (!url) return;

  invoke('add_download', { url, headers: {} }).catch(err => {
    alert('Failed to add download: ' + err);
  });
});

// Dynamically-generated row markup (status text, action buttons) isn't
// covered by data-i18n attributes, so force a full rebuild when the language
// changes instead of relying on the incremental in-place update path.
document.addEventListener('fdm-language-changed', () => {
  listEl.innerHTML = '';
  render();
});

// ------------------------------------------------------------- Initialization
async function init() {
  // Restore saved settings. The engine starts from its own defaults every
  // launch, so anything the user chose has to be pushed back in before the
  // first download resolves a destination.
  const savedConns = parseInt(localStorage.getItem('fdm_max_connections'), 10);
  const savedActive = parseInt(localStorage.getItem('fdm_max_active'), 10);
  const savedRoot = localStorage.getItem('fdm_download_root');
  const savedTemp = localStorage.getItem('fdm_temp_dir');
  let savedCategoryDirs;
  try {
    savedCategoryDirs = JSON.parse(localStorage.getItem('fdm_category_dirs') || 'null');
  } catch (_err) {
    savedCategoryDirs = null;
  }

  if (savedConns || savedActive || savedRoot || savedTemp || savedCategoryDirs) {
    invoke('update_config', {
      maxConnections: savedConns || undefined,
      maxActive: savedActive || undefined,
      downloadRoot: savedRoot || undefined,
      tempDir: savedTemp || undefined,
      categoryDirs: savedCategoryDirs || undefined,
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
