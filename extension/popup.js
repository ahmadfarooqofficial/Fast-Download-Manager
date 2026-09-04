'use strict';

/**
 * FDM — toolbar popup.
 *
 * A read-only view of the service worker's state plus two controls: the capture
 * switch and per-row cancel. It holds no state of its own — a popup is destroyed
 * every time it closes, so anything it remembered would be a lie the next time
 * it opened.
 */

const el = {
  capture: document.getElementById('capture'),
  sniffer: document.getElementById('sniffer'),
  banner: document.getElementById('banner'),
  bannerText: document.getElementById('banner-text'),
  dragHint: document.getElementById('drag-hint'),
  dragHintFix: document.getElementById('drag-hint-fix'),
  list: document.getElementById('list'),
  empty: document.getElementById('empty'),
  emptyHint: document.getElementById('empty-hint'),
  clear: document.getElementById('clear'),
  template: document.getElementById('row-template'),
};

/**
 * Whether this popup can hand the browser a `file://` URL — required for
 * dragging a finished download out to the OS. Off by default; the user has to
 * flip "Allow access to file URLs" for FDM in chrome://extensions. Resolved
 * asynchronously, so the first render or two may not know yet, which is why
 * `render` is re-run once the answer arrives instead of blocking on it.
 */
let fileAccessAllowed = null;
let lastState = null;

const port = chrome.runtime.connect({ name: 'fdm-ui' });

port.onMessage.addListener((msg) => {
  if (msg?.type === 'state') {
    lastState = msg;
    render(msg);
  }
});

// `chrome.runtime` grew this check in Chrome 96 or so; `chrome.extension` is
// the older home for the same call and still works, so fall back to it.
const fdmCheckFileAccess = chrome.runtime?.isAllowedFileSchemeAccess
  ? (cb) => chrome.runtime.isAllowedFileSchemeAccess(cb)
  : chrome.extension?.isAllowedFileSchemeAccess
    ? (cb) => chrome.extension.isAllowedFileSchemeAccess(cb)
    : null;

if (fdmCheckFileAccess) {
  fdmCheckFileAccess((allowed) => {
    fileAccessAllowed = !!allowed;
    if (lastState) render(lastState);
  });
} else {
  // No such API in this Chrome build; assume the worst rather than silently
  // drawing rows that can never actually be dragged out.
  fileAccessAllowed = false;
}

el.dragHintFix?.addEventListener('click', () => {
  try {
    chrome.tabs.create({ url: `chrome://extensions/?id=${chrome.runtime.id}` });
  } catch (e) {
    console.debug('[fdm popup] could not open extension settings', e);
  }
});

function safePost(msg) {
  try {
    port.postMessage(msg);
  } catch (e) {
    console.debug('[fdm popup] postMessage failed', e);
  }
}

el.capture?.addEventListener('change', () => {
  safePost({ type: 'setEnabled', enabled: el.capture.checked });
});

el.sniffer?.addEventListener('change', () => {
  safePost({ type: 'setMediaSniffer', enabled: el.sniffer.checked });
});

el.clear?.addEventListener('click', () => {
  safePost({ type: 'clear' });
});

// ------------------------------------------------------------------- render

const BANNERS = {
  missing: 'FDM’s desktop app isn’t answering. Downloads are staying with Chrome.',
  mismatch: 'The extension and the FDM app are different versions.',
};

function render(state) {
  if (el.capture) el.capture.checked = !!state.enabled;
  if (el.sniffer) el.sniffer.checked = state.mediaSniffer !== false;

  const banner = BANNERS[state.hostState];
  el.banner.hidden = !banner;
  if (banner) el.bannerText.textContent = banner;

  const rows = state.downloads || [];
  el.list.replaceChildren(...rows.map(rowNode));

  el.empty.hidden = rows.length > 0;
  el.emptyHint.textContent = state.enabled
    ? 'Start a download in your browser and it will appear here.'
    : 'Takeover is off, so Chrome is handling downloads itself.';

  el.clear.hidden = !rows.some(
    (r) => r.state === 'completed' || r.state === 'failed'
  );

  // Only worth nagging about once there is something to actually drag.
  const hasDraggable = rows.some((r) => r.state === 'completed' && r.path);
  if (el.dragHint) el.dragHint.hidden = !(hasDraggable && fileAccessAllowed === false);
}

/** Per-state wording. The word is the signal; the colour only reinforces it. */
function describe(row) {
  switch (row.state) {
    case 'starting':
      return { detail: 'Starting…', bar: 'running' };
    case 'running':
      return { detail: null, bar: 'running' };
    case 'completed':
      return {
        detail: row.detail ? `Saved to ${row.detail}` : 'Finished',
        bar: 'completed',
      };
    case 'failed':
      return {
        detail: row.resumable
          ? `Failed — can be resumed. ${row.detail || ''}`.trim()
          : `Failed. ${row.detail || ''}`.trim(),
        bar: 'failed',
      };
    case 'detached':
      return { detail: row.detail, bar: 'paused' };
    default:
      return { detail: row.detail, bar: 'running' };
  }
}

function rowNode(row) {
  const node = el.template.content.firstElementChild.cloneNode(true);
  const view = describe(row);

  node.dataset.state = row.state;

  const name = node.querySelector('.row__name');
  name.textContent = row.filename;
  // Ellipsised in CSS, so the full name has to be reachable some other way.
  name.title = row.filename;

  const pct = fdmPercent(row.downloaded, row.total);
  const bar = node.querySelector('.fdm-progress');
  bar.dataset.state = view.bar;
  bar.style.setProperty('--fdm-value', pct.toFixed(1));
  // The tick marks that make parallel connections visible. Never 0 — that would
  // divide by zero in the gradient and paint nothing.
  bar.style.setProperty('--fdm-segments', String(Math.max(1, row.segments || 1)));
  bar.setAttribute('aria-valuenow', Math.round(pct));
  bar.setAttribute(
    'aria-label',
    row.total
      ? `${row.filename}: ${Math.round(pct)}% of ${fdmFormatBytes(row.total)}`
      : `${row.filename}: ${fdmFormatBytes(row.downloaded)} so far`
  );

  node.querySelector('.row__size').textContent = row.total
    ? `${fdmFormatBytes(row.downloaded)} of ${fdmFormatBytes(row.total)}`
    : fdmFormatBytes(row.downloaded);

  const speed = node.querySelector('.row__speed');
  const eta = node.querySelector('.row__eta');
  if (row.state === 'running' && row.speedBps > 0) {
    speed.textContent = fdmFormatSpeed(row.speedBps);
    eta.textContent =
      row.etaSeconds != null ? `${fdmFormatDuration(row.etaSeconds)} left` : '';
  } else {
    speed.textContent = '';
    eta.textContent = '';
  }

  const detail = node.querySelector('.row__detail');
  if (view.detail) {
    detail.textContent = view.detail;
    detail.hidden = false;
  }

  const cancel = node.querySelector('.row__cancel');
  if (row.state === 'running' || row.state === 'starting') {
    cancel.hidden = false;
    // Icon-only control: it needs a name, and the name has to say what it acts on.
    cancel.setAttribute('aria-label', `Cancel ${row.filename}`);
    cancel.title = `Cancel ${row.filename}`;
    cancel.addEventListener('click', () => {
      cancel.disabled = true;
      safePost({ type: 'cancel', id: row.id });
    });
  }

  if (row.state === 'completed' && row.path) {
    attachDragOut(node, row);
  }

  return node;
}

// --------------------------------------------------------------- drag-out

/**
 * Let a finished download be dragged straight out of the popup onto the
 * desktop, a folder window, or another app — the way Chrome's own downloads
 * shelf lets you drag out a completed file.
 *
 * Chrome recognises the `DownloadURL` data-transfer type on drop and fetches
 * the given URL itself to materialise the file wherever it was dropped. For a
 * file that already exists on disk, that URL is a `file://` one, which Chrome
 * will only fetch for an extension that has "Allow access to file URLs"
 * turned on — hence the hint banner in `render` for everyone who hasn't.
 */
function attachDragOut(node, row) {
  const handle = node.querySelector('.row__handle');
  const fileUrl = fdmPathToFileUrl(row.path);
  if (!fileUrl) return;

  if (handle) handle.hidden = false;
  node.classList.add('row--draggable');
  node.draggable = true;
  node.title = fileAccessAllowed === false
    ? `Enable "Allow access to file URLs" in chrome://extensions to drag ${row.filename} out`
    : `Drag to save ${row.filename} anywhere`;

  node.addEventListener('dragstart', (e) => {
    const mime = fdmMimeFor(row.filename);
    e.dataTransfer.effectAllowed = 'copy';
    e.dataTransfer.setData('DownloadURL', `${mime}:${row.filename}:${fileUrl}`);
    // Belt and braces for drop targets that read a plain link instead.
    e.dataTransfer.setData('text/uri-list', fileUrl);
    e.dataTransfer.setData('text/plain', row.filename);
  });
}

/** `C:\Users\me\Downloads\clip.mp4` -> `file:///C:/Users/me/Downloads/clip.mp4`. */
function fdmPathToFileUrl(path) {
  if (!path) return null;
  let posix = String(path).replace(/\\/g, '/');
  if (!posix.startsWith('/')) posix = `/${posix}`;
  const encoded = posix
    .split('/')
    .map((segment) => encodeURIComponent(segment).replace(/%3A/gi, ':'))
    .join('/');
  return `file://${encoded}`;
}

const FDM_MIME_BY_EXT = {
  mp4: 'video/mp4', m4v: 'video/mp4', mkv: 'video/x-matroska', webm: 'video/webm',
  avi: 'video/x-msvideo', mov: 'video/quicktime', flv: 'video/x-flv',
  mp3: 'audio/mpeg', m4a: 'audio/mp4', wav: 'audio/wav', flac: 'audio/flac', ogg: 'audio/ogg',
  zip: 'application/zip', rar: 'application/vnd.rar', '7z': 'application/x-7z-compressed',
  tar: 'application/x-tar', gz: 'application/gzip',
  pdf: 'application/pdf', exe: 'application/x-msdownload', msi: 'application/x-msi',
  iso: 'application/x-iso9660-image',
};

function fdmMimeFor(filename) {
  const ext = String(filename).split('.').pop()?.toLowerCase();
  return FDM_MIME_BY_EXT[ext] || 'application/octet-stream';
}
