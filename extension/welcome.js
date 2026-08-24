'use strict';

/**
 * FDM — welcome page logic.
 *
 * One job: say something true. The page opens the moment the user clicks Enable
 * in Chrome, which is *before* we know whether the desktop app is reachable, so
 * every claim on it comes from a live ping to fdm-host.exe. The states are
 * checking → ok / missing / mismatch, and "missing" is a first-class outcome
 * with a fix, not an error toast.
 */

const el = {
  status: document.getElementById('status'),
  glyph: document.querySelector('.status__glyph use'),
  headline: document.getElementById('headline'),
  lede: document.getElementById('lede'),
  fixit: document.getElementById('fixit'),
  retry: document.getElementById('retry'),
  facts: document.getElementById('facts'),
  factRoot: document.getElementById('fact-root'),
  factConnections: document.getElementById('fact-connections'),
  factVersion: document.getElementById('fact-version'),
  foldersSection: document.getElementById('folders-section'),
  folders: document.getElementById('folders'),
  extVersion: document.getElementById('ext-version'),
};

const VIEWS = {
  checking: {
    icon: '#i-spinner',
    title: 'FDM — Setup',
    headline: 'Checking your setup…',
    lede: 'Talking to the FDM desktop app.',
  },
  ok: {
    icon: '#i-check',
    title: 'FDM is set up',
    headline: 'All set — FDM has your downloads',
    lede:
      'The extension and the desktop app are talking to each other. Download ' +
      'something and FDM will take it from here.',
  },
  missing: {
    icon: '#i-alert',
    title: 'FDM — one step left',
    headline: 'Can’t reach the FDM desktop app',
    lede:
      'The extension is installed correctly, but nothing answered on the other ' +
      'side. Until this is fixed your downloads stay with Chrome — nothing is ' +
      'lost, they are just not accelerated.',
  },
  mismatch: {
    icon: '#i-alert',
    title: 'FDM — version mismatch',
    headline: 'The extension and the app are different versions',
    lede: 'Update whichever is older and they will start talking again.',
  },
};

/** The port to the service worker. It pushes state; we never poll. */
let port = null;

function render(state) {
  // 'unknown' is the worker's answer before the first ping lands. To the user
  // that is the same thing as "checking", and mapping it here keeps the spinner
  // animating instead of leaving a dead grey circle.
  const phase = state.hostState === 'unknown' ? 'checking' : state.hostState;
  const view = VIEWS[phase] || VIEWS.checking;

  el.status.dataset.state = phase;
  el.glyph.setAttribute('href', view.icon);
  document.title = view.title;
  el.headline.textContent = view.headline;
  el.lede.textContent =
    phase === 'mismatch' && state.hostInfo?.mismatchMessage
      ? state.hostInfo.mismatchMessage
      : view.lede;

  const needsFix = phase === 'missing' || phase === 'mismatch';
  el.fixit.hidden = !needsFix;

  el.extVersion.textContent = `extension ${state.extensionVersion}`;

  const info = phase === 'ok' ? state.hostInfo : null;
  el.facts.hidden = !info;

  if (info) {
    el.factRoot.textContent = info.downloadRoot || '—';
    el.factConnections.textContent = info.maxConnections ?? '—';
    // Version and protocol in one cell: the protocol number only ever matters
    // next to the version it belongs to, and three facts fill the two-column
    // grid without leaving an empty cell.
    el.factVersion.textContent = `${info.version || '—'} · protocol v${
      info.protocol ?? state.protocol
    }`;
    renderFolders(info.categories);
  } else {
    el.foldersSection.hidden = true;
  }
}

function renderFolders(categories) {
  if (!Array.isArray(categories) || categories.length === 0) {
    el.foldersSection.hidden = true;
    return;
  }

  // Rebuilt wholesale rather than diffed: this runs at most a handful of times
  // in the life of the page.
  el.folders.replaceChildren(
    ...categories.map((name) => {
      const li = document.createElement('li');

      const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      svg.setAttribute('width', '15');
      svg.setAttribute('height', '15');
      svg.setAttribute('aria-hidden', 'true');
      const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
      use.setAttribute('href', '#i-folder');
      svg.appendChild(use);

      li.append(svg, document.createTextNode(name));
      return li;
    })
  );
  el.foldersSection.hidden = false;
}

function connect() {
  port = chrome.runtime.connect({ name: 'fdm-ui' });
  port.onMessage.addListener((msg) => {
    if (msg?.type === 'state') {
      render(msg);
      el.retry.disabled = false;
    }
  });
  port.onDisconnect.addListener(() => {
    port = null;
    // The service worker was evicted, which is routine. Reconnecting wakes it.
    setTimeout(connect, 250);
  });
}

el.retry.addEventListener('click', () => {
  el.retry.disabled = true;
  el.status.dataset.state = 'checking';
  el.glyph.setAttribute('href', VIEWS.checking.icon);
  document.title = VIEWS.checking.title;
  el.headline.textContent = VIEWS.checking.headline;
  el.lede.textContent = VIEWS.checking.lede;
  el.fixit.hidden = true;

  if (!port) connect();
  port?.postMessage({ type: 'refresh' });

  // If nothing comes back the button must not stay disabled forever.
  setTimeout(() => {
    el.retry.disabled = false;
  }, 6000);
});

connect();

// The very first state push may say 'unknown' if the worker just woke up, so ask
// for a real check straight away.
port?.postMessage({ type: 'refresh' });
