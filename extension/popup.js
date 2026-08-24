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
  list: document.getElementById('list'),
  empty: document.getElementById('empty'),
  emptyHint: document.getElementById('empty-hint'),
  clear: document.getElementById('clear'),
  template: document.getElementById('row-template'),
};

const port = chrome.runtime.connect({ name: 'fdm-ui' });

port.onMessage.addListener((msg) => {
  if (msg?.type === 'state') render(msg);
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

  return node;
}
