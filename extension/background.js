'use strict';

/**
 * FDM — background service worker.
 *
 * This is the takeover: Chrome starts a download, we intercept it, cancel it,
 * and hand the URL (with the cookies and referrer that made it work) to
 * fdm-host.exe, which fetches it with many parallel connections.
 *
 * Three rules shape everything below.
 *
 * 1. NEVER LOSE A DOWNLOAD. Cancelling a download the desktop app then fails to
 *    pick up is worse than not having FDM installed at all. So the host is
 *    probed before we ever cancel anything, and if the handoff fails after the
 *    cancel, `restoreToChrome` hands it straight back to the browser.
 *
 * 2. NO LOOPS. Restoring a download re-enters this very listener. `bypass`
 *    holds the URLs we deliberately want Chrome to keep.
 *
 * 3. THE SERVICE WORKER IS NOT A PROCESS. Chrome evicts it whenever it feels
 *    like it. Anything the popup needs to render is mirrored into
 *    chrome.storage.session, and the host deliberately outlives the port so an
 *    eviction mid-download is survivable (see crates/fdm-host/src/main.rs).
 */

const HOST_NAME = 'com.fdm.native_host';

/** Must match PROTOCOL_VERSION in crates/fdm-host/src/protocol.rs. */
const PROTOCOL_VERSION = 1;

const SESSION_KEY = 'fdm.active';
const SETTINGS_KEY = 'fdm.settings';

const DEFAULT_SETTINGS = {
  /** Master switch. Off means Chrome downloads everything itself. */
  enabled: true,

  /** Media sniffer floating button on web video/audio players. */
  mediaSniffer: true,

  /**
   * Never take over these. Not a performance filter — these are the extensions
   * that show up when a *page* is served as a download (a login redirect, an
   * error page with Content-Disposition). Handing those to a download manager
   * is the exact behaviour people disable IDM's extension over.
   */
  skipExtensions: [
    'html', 'htm', 'xhtml', 'php', 'asp', 'aspx', 'jsp', 'cgi',
    'css', 'js', 'mjs', 'json', 'xml', 'rss', 'atom', 'txt',
    // Images are downloaded natively by the browser per user requirement
    'png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'ico', 'bmp', 'avif',
  ],
};

// --------------------------------------------------------------------- state
//
// All module-level, all rebuilt from scratch after an eviction. Nothing here is
// the source of truth for anything that matters.

/** The live native messaging port, or null. */
let port = null;

/** 'unknown' | 'ok' | 'missing' — whether fdm-host.exe answered a ping. */
let hostState = 'unknown';

/** The `pong` payload: version, downloadRoot, maxConnections, categories. */
let hostInfo = null;

/** id -> row shown in the popup. */
const active = new Map();

/**
 * Cached settings.
 *
 * `shouldCapture` has to answer synchronously — the download is already running
 * and every await is bytes on disk — so the settings are mirrored here and
 * refreshed on install, on startup, on every worker wake-up, and on change.
 */
let capturePolicy = { ...DEFAULT_SETTINGS };

/** id -> one-shot reply handler, for request/response messages like ping. */
const pending = new Map();

/** URLs we have just handed back to Chrome, so we don't re-capture them. */
const bypass = new Set();

/** Open popup ports, for live progress. */
const listeners = new Set();

/**
 * Correlation ids for the host.
 *
 * Derived from the clock rather than a counter because the host outlives this
 * service worker: after an eviction a counter would restart at 1 and collide
 * with a download still running in the same host process, and the host keys its
 * running-downloads map by exactly this number. Date.now() * 1000 stays well
 * inside JavaScript's exact-integer range until the year 2255.
 */
let idSalt = 0;
function nextId() {
  idSalt = (idSalt + 1) % 1000;
  return Date.now() * 1000 + idSalt;
}

// ------------------------------------------------------------------ settings

async function getSettings() {
  const stored = await chrome.storage.sync.get(SETTINGS_KEY);
  return { ...DEFAULT_SETTINGS, ...(stored[SETTINGS_KEY] || {}) };
}

async function setSettings(patch) {
  const next = { ...(await getSettings()), ...patch };
  await chrome.storage.sync.set({ [SETTINGS_KEY]: next });
  capturePolicy = next;
  await refreshBadge();
  broadcast();
  return next;
}

// ------------------------------------------------------------- the host port

/**
 * Open the port if it isn't already open.
 *
 * `connectNative` never throws for a missing host — it returns a live-looking
 * port and then fires onDisconnect asynchronously. That asymmetry is why
 * `hostState` exists: a synchronous "is FDM installed?" answer is impossible,
 * so we keep the answer from last time.
 */
function ensurePort() {
  if (port) return port;

  try {
    port = chrome.runtime.connectNative(HOST_NAME);
  } catch (e) {
    // Thrown only when the nativeMessaging permission is missing, which would
    // be a packaging bug rather than a user problem.
    console.error('[fdm] connectNative threw', e);
    port = null;
    setHostState('missing');
    return null;
  }

  port.onMessage.addListener(onHostMessage);
  port.onDisconnect.addListener(() => {
    const err = chrome.runtime.lastError;
    port = null;

    if (err) {
      // The message is Chrome's, and it is the useful one: "Specified native
      // messaging host not found" means the installer never ran or Chrome was
      // not restarted after it did.
      console.warn('[fdm] native host disconnected:', err.message);
      setHostState('missing');
    }

    // Anything still marked running lost its reporter. Mark it unknown rather
    // than failed: the host keeps downloading after the port closes, by design.
    for (const row of active.values()) {
      if (row.state === 'running' || row.state === 'starting') {
        row.state = 'detached';
        row.detail = 'Still downloading in FDM — reopen the app to see progress.';
      }
    }
    persist();
    broadcast();
    refreshBadge();
  });

  return port;
}

function sendToHost(message) {
  const p = ensurePort();
  if (!p) return false;
  try {
    p.postMessage(message);
    return true;
  } catch (e) {
    console.warn('[fdm] postMessage failed', e);
    port = null;
    return false;
  }
}

function setHostState(next) {
  if (hostState === next) return;
  hostState = next;
  broadcast();
  refreshBadge();
}

/**
 * Ask the host to identify itself.
 *
 * Resolves to the `pong` payload, or null. Used by the welcome page so it can
 * report a connection it has actually verified instead of asserting success,
 * and at startup so `shouldCapture` knows whether cancelling is safe.
 */
function pingHost(timeoutMs = 4000) {
  return new Promise((resolve) => {
    const id = nextId();
    let settled = false;

    const done = (value) => {
      if (settled) return;
      settled = true;
      pending.delete(id);
      resolve(value);
    };

    pending.set(id, (msg) => {
      if (msg.type === 'pong') {
        hostInfo = msg;
        setHostState('ok');
        done(msg);
      } else {
        // An `error` reply still proves the host is running and talking; a
        // version mismatch is a different problem from a missing app.
        setHostState('ok');
        done(msg);
      }
    });

    if (!sendToHost({ type: 'ping', id, protocol: PROTOCOL_VERSION })) {
      done(null);
      return;
    }

    setTimeout(() => {
      if (!settled) setHostState('missing');
      done(null);
    }, timeoutMs);
  });
}

function onHostMessage(msg) {
  if (!msg || typeof msg !== 'object') return;

  if (msg.id != null && pending.has(msg.id)) {
    const handler = pending.get(msg.id);
    pending.delete(msg.id);
    handler(msg);
    if (msg.type === 'pong' || msg.type === 'status') return;
  }

  const row = msg.id != null ? active.get(msg.id) : null;

  switch (msg.type) {
    case 'pong':
      hostInfo = msg;
      setHostState('ok');
      return;

    case 'accepted':
      setHostState('ok');
      if (row) {
        row.state = 'running';
        row.detail = null;
      }
      break;

    case 'progress':
      if (row) {
        row.state = 'running';
        row.downloaded = msg.downloaded;
        row.total = msg.total ?? null;
        row.speedBps = msg.speedBps;
        row.etaSeconds = msg.etaSeconds ?? null;
        row.segments = msg.segments;
        row.activeConnections = msg.activeConnections;
      }
      break;

    case 'completed':
      if (row) {
        row.state = 'completed';
        row.path = msg.path;
        row.downloaded = msg.bytes;
        row.total = msg.bytes;
        row.speedBps = 0;
        row.etaSeconds = null;
        row.segments = msg.segments;
        row.detail = msg.category;
      }
      notify('Download finished', msg.path);
      break;

    case 'failed':
      if (row) {
        row.state = 'failed';
        row.detail = msg.message;
        row.resumable = !!msg.resumable;
      }
      notify('Download failed', msg.message);
      break;

    case 'cancelled':
      if (row) active.delete(msg.id);
      break;

    case 'error':
      console.warn('[fdm] host error:', msg.message);
      if (msg.versionMismatch) {
        setHostState('mismatch');
        hostInfo = { ...(hostInfo || {}), mismatchMessage: msg.message };
      }
      if (row) {
        row.state = 'failed';
        row.detail = msg.message;
      }
      break;

    default:
      console.debug('[fdm] unhandled host message', msg.type);
      return;
  }

  persist();
  broadcast();
  refreshBadge();
}

// -------------------------------------------------------------- the takeover

chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
  // Chrome resolved a filename, so this is the last moment before bytes hit
  // disk — and the first moment we know what the file is called.
  //
  // The listener itself must stay synchronous. Returning without calling
  // suggest() lets Chrome use its own filename, which is exactly what we want
  // for anything we are not taking over.
  decide(item, suggest);
});

function decide(item, suggest) {
  if (bypass.has(item.url) || bypass.has(item.finalUrl)) {
    bypass.delete(item.url);
    bypass.delete(item.finalUrl);
    suggest();
    return;
  }

  // Read settings asynchronously, but cancel synchronously. Waiting on storage
  // before cancelling would let Chrome start writing, which is why
  // `capturePolicy` is a cached copy rather than a fresh read.
  const verdict = shouldCapture(item);
  if (verdict.capture) {
    // Satisfy onDeterminingFilename so Chrome finishes filename resolution cleanly
    suggest();

    // Cancel Chrome's download and hand off to FDM
    try {
      chrome.downloads.cancel(item.id, () => {
        const _err = chrome.runtime.lastError;
        try {
          chrome.downloads.erase({ id: item.id }, () => {
            const _ = chrome.runtime.lastError;
          });
        } catch {}
        takeOver(item).catch((e) => console.error('[fdm] takeover failed', e));
      });
    } catch (e) {
      console.debug('[fdm] cancel threw', e);
      takeOver(item).catch((e) => console.error('[fdm] takeover failed', e));
    }
    return;
  }

  console.debug('[fdm] passing to Chrome:', verdict.reason, item.url);
  suggest();
}

/**
 * Whether FDM should take this download.
 *
 * Deliberately conservative: every `capture: false` below is a case where
 * cancelling would either lose the download outright or produce a file the user
 * did not ask for.
 */
function shouldCapture(item) {
  if (!capturePolicy.enabled) return { capture: false, reason: 'capture off' };

  if (hostState === 'missing') {
    // Cancelling now would delete the download and hand it to an app that isn't
    // there. Let Chrome do its job.
    return { capture: false, reason: 'FDM app not reachable' };
  }
  if (hostState === 'mismatch') {
    return { capture: false, reason: 'extension and app versions differ' };
  }

  const url = item.finalUrl || item.url || '';
  if (!/^https?:\/\//i.test(url)) {
    // blob: and data: URLs exist only inside the page that created them. A
    // separate process cannot re-fetch them, so cancelling destroys the file.
    return { capture: false, reason: 'not an http(s) URL' };
  }

  if (item.incognito) {
    // An incognito download would leave a .fdm control file and a partial file
    // on disk after the window closed. Declining is the honest reading of what
    // incognito means.
    return { capture: false, reason: 'incognito' };
  }

  const ext = fileExtension(item.filename || url);
  if (ext && capturePolicy.skipExtensions.includes(ext)) {
    return { capture: false, reason: `.${ext} is excluded` };
  }

  return { capture: true, reason: null };
}

function fileExtension(nameOrUrl) {
  const base = String(nameOrUrl).split(/[?#]/)[0].split(/[\\/]/).pop() || '';
  const dot = base.lastIndexOf('.');
  if (dot <= 0 || dot === base.length - 1) return null;
  return base.slice(dot + 1).toLowerCase();
}

async function takeOver(item) {
  const url = item.finalUrl || item.url;
  const id = nextId();

  const row = {
    id,
    url,
    filename: baseName(item.filename) || baseName(url) || 'download',
    state: 'starting',
    downloaded: 0,
    total: item.fileSize > 0 ? item.fileSize : null,
    speedBps: 0,
    etaSeconds: null,
    segments: 0,
    activeConnections: 0,
    startedAt: Date.now(),
    detail: null,
  };
  active.set(id, row);
  persist();
  broadcast();
  refreshBadge();

  const headers = await collectHeaders(item, url);

  const ok = sendToHost({
    type: 'download',
    id,
    url,
    headers,
    filename: baseName(item.filename) || null,
    totalBytes: item.fileSize > 0 ? item.fileSize : null,
    protocol: PROTOCOL_VERSION,
  });

  if (!ok) {
    // Rule 1: we already cancelled Chrome's download, so we owe the user one.
    active.delete(id);
    persist();
    broadcast();
    refreshBadge();
    setHostState('missing');
    restoreToChrome(url, baseName(item.filename));
    notify(
      'FDM could not start',
      'The download was handed back to Chrome. Is the FDM app installed?'
    );
    return;
  }

  // If `accepted` never arrives the row stays in 'starting'. Give it a bounded
  // wait, then treat the silence as a failure and give the download back.
  setTimeout(() => {
    const current = active.get(id);
    if (current && current.state === 'starting') {
      active.delete(id);
      persist();
      broadcast();
      refreshBadge();
      restoreToChrome(url, baseName(item.filename));
      notify(
        'FDM did not respond',
        'The download was handed back to Chrome.'
      );
    }
  }, 10000);
}

/**
 * Collect the headers that make the request work.
 *
 * The host re-issues the request as a fresh anonymous client, so without these
 * a download behind a login quietly fetches the login page and saves *that* — a
 * file of plausible size containing the wrong bytes. The host drops anything
 * unsafe (notably Accept-Encoding, which would make byte ranges refer to
 * compressed offsets).
 */
async function collectHeaders(item, url) {
  const headers = {};

  if (item.referrer) headers['Referer'] = item.referrer;
  headers['User-Agent'] = navigator.userAgent;

  try {
    const cookies = await chrome.cookies.getAll({ url });
    if (cookies.length) {
      headers['Cookie'] = cookies.map((c) => `${c.name}=${c.value}`).join('; ');
    }
  } catch (e) {
    // A missing cookie header is recoverable — the download may just 403. A
    // thrown exception here would lose it entirely.
    console.warn('[fdm] could not read cookies', e);
  }

  return headers;
}

/** Give a download back to the browser without re-triggering our listener. */
function restoreToChrome(url, filename) {
  bypass.add(url);
  // Belt and braces: if the listener never fires (the URL 404s, say) the bypass
  // entry would otherwise leak and disable capture for that URL forever.
  setTimeout(() => bypass.delete(url), 60000);

  const options = { url };
  if (filename) options.filename = filename;

  chrome.downloads.download(options, () => {
    const err = chrome.runtime.lastError;
    if (err) console.error('[fdm] handing back to Chrome failed:', err.message);
  });
}

function baseName(p) {
  if (!p) return null;
  const clean = String(p).split(/[?#]/)[0];
  const last = clean.split(/[\\/]/).pop();
  return last ? decodeURIComponentSafe(last) : null;
}

function decodeURIComponentSafe(s) {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

// -------------------------------------------------------- popup / page comms

function safePost(p, msg) {
  try {
    p.postMessage(msg);
  } catch {
    listeners.delete(p);
  }
}

chrome.runtime.onConnect.addListener((p) => {
  if (p.name !== 'fdm-ui') return;

  listeners.add(p);
  p.onDisconnect.addListener(() => listeners.delete(p));
  p.onMessage.addListener((msg) => onUiMessage(msg, p));

  safePost(p, snapshot());
});

async function onUiMessage(msg, p) {
  switch (msg?.type) {
    case 'refresh':
      await pingHost();
      safePost(p, snapshot());
      break;

    case 'setEnabled':
      capturePolicy = await setSettings({ enabled: !!msg.enabled });
      safePost(p, snapshot());
      break;

    case 'setMediaSniffer':
      capturePolicy = await setSettings({ mediaSniffer: !!msg.enabled });
      safePost(p, snapshot());
      break;

    case 'cancel':
      sendToHost({ type: 'cancel', id: msg.id });
      break;

    case 'clear': {
      for (const [id, row] of active) {
        if (row.state === 'completed' || row.state === 'failed') active.delete(id);
      }
      persist();
      broadcast();
      refreshBadge();
      break;
    }

    default:
      break;
  }
}

function snapshot() {
  return {
    type: 'state',
    hostState,
    hostInfo,
    enabled: capturePolicy.enabled,
    mediaSniffer: capturePolicy.mediaSniffer !== false,
    protocol: PROTOCOL_VERSION,
    extensionVersion: chrome.runtime.getManifest().version,
    downloads: [...active.values()].sort((a, b) => b.startedAt - a.startedAt),
  };
}

function broadcast() {
  if (!listeners.size) return;
  const state = snapshot();
  for (const p of listeners) {
    try {
      p.postMessage(state);
    } catch {
      listeners.delete(p);
    }
  }
}

/**
 * Mirror the rows into session storage.
 *
 * Not a cache — a handover. When Chrome evicts this worker the Maps above are
 * gone, but the downloads are not: the host keeps going. Without this the popup
 * would show "nothing downloading" while 4 GB was in flight.
 */
function persist() {
  const rows = [...active.values()];
  chrome.storage.session
    .set({ [SESSION_KEY]: rows })
    .catch((e) => console.debug('[fdm] session persist failed', e));
}

async function restore() {
  try {
    const stored = await chrome.storage.session.get(SESSION_KEY);
    for (const row of stored[SESSION_KEY] || []) {
      if (!active.has(row.id)) active.set(row.id, row);
    }
  } catch (e) {
    console.debug('[fdm] session restore failed', e);
  }
}

// --------------------------------------------------------------- badge, toast

async function refreshBadge() {
  const running = [...active.values()].filter(
    (r) => r.state === 'running' || r.state === 'starting'
  ).length;

  let text = '';
  let colour = '#e50914';

  if (hostState === 'missing' || hostState === 'mismatch') {
    text = '!';
  } else if (!capturePolicy.enabled) {
    text = 'off';
    colour = '#475569';
  } else if (running > 0) {
    text = String(running);
  }

  try {
    await chrome.action.setBadgeText({ text });
    await chrome.action.setBadgeBackgroundColor({ color: colour });
  } catch {
    // The action can be unavailable while the worker is starting up.
  }
}

function notify(title, message) {
  chrome.notifications
    .create({
      type: 'basic',
      iconUrl: chrome.runtime.getURL('icons/icon-128.png'),
      title,
      message: String(message ?? '').slice(0, 300),
      silent: true,
    })
    .catch(() => {
      // Notifications can be blocked at the OS level. Not worth a console error
      // on every completed download.
    });
}

// ---------------------------------------------------------------- lifecycle

chrome.runtime.onInstalled.addListener(async (details) => {
  capturePolicy = await getSettings();
  await refreshBadge();

  if (details.reason === 'install') {
    // The IDM moment: the user has just clicked Enable in Chrome, and this is
    // the page that tells them it worked. It runs its own ping, so it reports a
    // connection it verified rather than one we assumed.
    chrome.tabs.create({ url: chrome.runtime.getURL('welcome.html') });
  }

  await pingHost();
});

chrome.runtime.onStartup.addListener(async () => {
  capturePolicy = await getSettings();
  await restore();
  await pingHost();
  await refreshBadge();
});

// Also runs on every worker wake-up, which is the common case.
(async () => {
  capturePolicy = await getSettings();
  await restore();
  await refreshBadge();
})();

// -------------------------------------------------------- Media Sniffer Comms

const tabMediaStreams = new Map();
const tabVideoDirectUrls = new Map();

// ── YouTube itag → height lookup (comprehensive: H.264, VP9, AV1, combined) ──
const ITAG_HEIGHT = {
  // H.264 (AVC)
  160: 144, 133: 240, 134: 360, 135: 480, 136: 720, 298: 720,
  137: 1080, 299: 1080, 264: 1440, 266: 2160, 138: 2160,
  // VP9
  278: 144, 242: 240, 243: 360, 244: 480, 245: 480, 246: 480,
  247: 720, 302: 720, 248: 1080, 303: 1080, 271: 1440, 308: 1440,
  313: 2160, 315: 2160, 272: 4320,
  // AV1
  394: 144, 395: 240, 396: 360, 397: 480, 398: 720, 399: 1080,
  400: 1440, 401: 2160, 571: 4320,
  // Combined video+audio
  18: 360, 22: 720,
};
// Prefer H.264 for best mp4 compatibility (no re-mux needed)
const H264_ITAGS = new Set([160,133,134,135,136,298,137,299,264,266,138,18,22]);

chrome.webRequest?.onBeforeRequest?.addListener(
  (details) => {
    if (!details.url || !details.tabId || details.tabId < 0) return;
    try {
      if (details.url.includes('googlevideo.com/videoplayback')) {
        const u = new URL(details.url);
        const itag = u.searchParams.get('itag');
        if (itag) {
          u.searchParams.delete('range');
          u.searchParams.delete('rn');
          u.searchParams.delete('rbuf');
          const cleanStreamUrl = u.toString();

          if (!tabVideoDirectUrls.has(details.tabId)) {
            tabVideoDirectUrls.set(details.tabId, new Map());
          }
          tabVideoDirectUrls.get(details.tabId).set(parseInt(itag, 10), cleanStreamUrl);
        }
      }
    } catch (_) {}
  },
  { urls: ['*://*.googlevideo.com/videoplayback*'] }
);

// Find the best sniffed direct CDN URL for a given height from the tab's captured streams
function findDirectUrlForHeight(tabId, requestedHeight) {
  const directMap = tabVideoDirectUrls.get(tabId);
  if (!directMap || directMap.size === 0) return null;

  let bestUrl = null;
  let bestIsH264 = false;

  for (const [itag, url] of directMap.entries()) {
    const h = ITAG_HEIGHT[itag];
    if (h === requestedHeight) {
      const isH264 = H264_ITAGS.has(itag);
      // Prefer H.264 over VP9/AV1 for direct mp4 download
      if (!bestUrl || (isH264 && !bestIsH264)) {
        bestUrl = url;
        bestIsH264 = isH264;
      }
    }
  }
  return bestUrl;
}

function readPlayer() {
  try {
    const urlParams = new URLSearchParams(window.location.search);
    const expectedVideoId = urlParams.get('v');

    const player = document.querySelector('#movie_player') || document.querySelector('.html5-video-player');
    const data = (player && typeof player.getVideoData === 'function') ? (player.getVideoData() || {}) : {};
    
    // Check if player has transitioned to the expected video
    const actualVideoId = data.video_id || '';
    if (expectedVideoId && actualVideoId && actualVideoId !== expectedVideoId) {
      return { notReady: true, expectedVideoId, actualVideoId, levels: [] };
    }

    let levels = [];
    if (player && typeof player.getAvailableQualityLevels === 'function') {
      levels = player.getAvailableQualityLevels() || [];
    }

    let qualityData = [];
    if (player && typeof player.getAvailableQualityData === 'function') {
      qualityData = player.getAvailableQualityData() || [];
    }

    // Read current video's streamingData and player response
    let streamFormats = [];
    try {
      let resp = null;
      if (player && typeof player.getPlayerResponse === 'function') {
        resp = player.getPlayerResponse();
      }
      if (resp && expectedVideoId && resp.videoDetails?.videoId && resp.videoDetails.videoId !== expectedVideoId) {
        resp = null;
      }
      if (!resp) {
        const watchFlexy = document.querySelector('ytd-watch-flexy');
        if (watchFlexy && watchFlexy.playerData) {
          if (!expectedVideoId || watchFlexy.playerData.videoDetails?.videoId === expectedVideoId) {
            resp = watchFlexy.playerData;
          }
        }
      }
      if (!resp && window.ytInitialPlayerResponse) {
        if (!expectedVideoId || window.ytInitialPlayerResponse.videoDetails?.videoId === expectedVideoId) {
          resp = window.ytInitialPlayerResponse;
        }
      }

      if (resp && resp.streamingData) {
        streamFormats = [
          ...(resp.streamingData.adaptiveFormats || []),
          ...(resp.streamingData.formats || []),
        ].map(f => ({
          itag: f.itag,
          height: f.height,
          quality: f.quality,
          qualityLabel: f.qualityLabel,
          mimeType: f.mimeType,
          bitrate: f.bitrate,
          contentLength: f.contentLength,
          url: f.url || '',
        }));
      }
    } catch (_) {}

    return {
      levels: levels || [],
      qualityData: qualityData || [],
      streamFormats: streamFormats || [],
      duration: Math.round((player && typeof player.getDuration === 'function') ? player.getDuration() : 0),
      title: data.title || (document.title ? document.title.replace(/ - YouTube$/, '') : '') || '',
      author: data.author || '',
      videoId: data.video_id || expectedVideoId || '',
    };
  } catch (e) {
    return { levels: [], error: e.message };
  }
}

chrome.tabs?.onRemoved?.addListener((tabId) => {
  tabMediaStreams.delete(tabId);
  tabVideoDirectUrls.delete(tabId);
});

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (msg?.type === 'pageQualities') {
    const tabId = sender.tab?.id ?? msg.tabId;
    if (!tabId) {
      sendResponse({ ok: false, error: 'No tabId' });
      return true;
    }
    chrome.scripting.executeScript({ target: { tabId }, world: 'MAIN', func: readPlayer })
      .then(([injection]) => {
        const res = injection?.result || { levels: [] };
        const directMap = tabVideoDirectUrls.get(tabId);
        if (directMap && Array.isArray(res.streamFormats)) {
          res.streamFormats = res.streamFormats.map(f => {
            if (!f.url && directMap.has(f.itag)) {
              return { ...f, url: directMap.get(f.itag) };
            }
            return f;
          });
        }
        sendResponse({ ok: true, ...res });
      })
      .catch((error) => {
        sendResponse({ ok: false, error: error.message, levels: [] });
      });
    return true;
  }

  if (msg?.type === 'getTabMediaStreams') {
    const tabId = sender.tab?.id;
    sendResponse({ streams: tabId ? tabMediaStreams.get(tabId) || [] : [] });
    return true;
  }

  if (msg?.type === 'downloadMedia') {
    (async () => {
      let targetUrl = msg.url || msg.pageUrl;
      const tabId = sender.tab?.id;
      let audioUrl = null; // For adaptive streams that need separate audio

      // ── Critical: resolve direct CDN URL for YouTube to avoid yt-dlp delay ──
      const isYouTubePageUrl = targetUrl && (
        targetUrl.includes('youtube.com/watch') ||
        targetUrl.includes('youtube.com/shorts') ||
        targetUrl.includes('youtu.be/')
      );

      if (isYouTubePageUrl && tabId) {
        const heightMatch = msg.filename?.match(/(\d{3,4})p/);
        const requestedHeight = heightMatch ? parseInt(heightMatch[1], 10) : null;
        const isAudioOnly = msg.filename?.includes('Audio');

        if (requestedHeight || isAudioOnly) {
          const directMap = tabVideoDirectUrls.get(tabId);
          let directUrl = null;

          // For audio-only downloads, find the best audio stream
          if (isAudioOnly && directMap) {
            // Audio itags: 140=m4a/128k, 251=opus/160k, 250=opus/70k, 249=opus/50k
            for (const audioItag of [140, 251, 250, 249]) {
              if (directMap.has(audioItag)) {
                directUrl = directMap.get(audioItag);
                break;
              }
            }
          }

          // For video: find BOTH video and audio CDN URLs
          if (!isAudioOnly && requestedHeight) {
            // Step 1: Check sniffed CDN URLs
            if (directMap && directMap.size > 0) {
              // Find video stream for this height
              let bestVideoUrl = null;
              let bestIsH264 = false;
              for (const [itag, url] of directMap.entries()) {
                if (ITAG_HEIGHT[itag] === requestedHeight) {
                  const isH264 = H264_ITAGS.has(itag);
                  if (!bestVideoUrl || (isH264 && !bestIsH264)) {
                    bestVideoUrl = url;
                    bestIsH264 = isH264;
                  }
                }
              }

              // Find audio stream
              let bestAudioUrl = null;
              for (const audioItag of [140, 251, 250, 249]) {
                if (directMap.has(audioItag)) {
                  bestAudioUrl = directMap.get(audioItag);
                  break;
                }
              }

              if (bestVideoUrl && bestAudioUrl) {
                directUrl = bestVideoUrl;
                audioUrl = bestAudioUrl;
              }
            }

            // Step 2: If not sniffed, try player response for direct URLs
            if (!directUrl) {
              try {
                const [injection] = await chrome.scripting.executeScript({
                  target: { tabId }, world: 'MAIN', func: readPlayer,
                });
                const res = injection?.result;
                if (res && Array.isArray(res.streamFormats)) {
                  let videoUrl = null;
                  let foundAudioUrl = null;

                  for (const f of res.streamFormats) {
                    let url = (directMap && directMap.has(f.itag)) ? directMap.get(f.itag) : f.url;
                    if (!url) continue;

                    // Video match
                    if (!videoUrl && f.height === requestedHeight) {
                      videoUrl = url;
                    }
                    if (!videoUrl && f.qualityLabel) {
                      const m = f.qualityLabel.match(/(\d+)p/i);
                      if (m && parseInt(m[1], 10) === requestedHeight) {
                        videoUrl = url;
                      }
                    }

                    // Audio match
                    if (!foundAudioUrl && f.mimeType && f.mimeType.includes('audio') && url) {
                      foundAudioUrl = url;
                    }
                  }

                  if (videoUrl && foundAudioUrl) {
                    directUrl = videoUrl;
                    audioUrl = foundAudioUrl;
                  }
                }
              } catch (_) {}
            }
          }

          if (directUrl) {
            console.log(`[fdm] ⚡ Direct CDN URL found for ${requestedHeight || 'audio'}p — bypassing yt-dlp`);
            targetUrl = directUrl;
          } else {
            console.log(`[fdm] ⚠ No direct CDN URL for ${requestedHeight || 'audio'}p — yt-dlp fallback`);
          }
        }
      }

      const headers = await collectHeaders(targetUrl, msg.pageUrl || sender.tab?.url || 'https://www.youtube.com/');
      headers['Referer'] = msg.pageUrl || sender.tab?.url || 'https://www.youtube.com/';

      const downloadId = nextId();
      const filename = msg.filename || baseName(targetUrl) || 'video.mp4';

      console.log('[fdm] downloadMedia sending to host:', targetUrl.substring(0, 120), filename);

      const payload = {
        type: 'download',
        id: downloadId,
        url: targetUrl,
        filename: filename,
        headers,
      };
      // Pass audio URL so the backend can merge video+audio without yt-dlp
      if (audioUrl) {
        payload.audioUrl = audioUrl;
      }

      const sent = sendToHost(payload);
      sendResponse({ success: sent, url: targetUrl });
    })();
    return true;
  }

  if (msg?.type === 'getMediaSettings') {
    getSettings().then((s) => sendResponse(s));
    return true;
  }
});

try {
  chrome.webRequest?.onHeadersReceived?.addListener(
    (details) => {
      if (!capturePolicy.enabled || capturePolicy.mediaSniffer === false) return;
      if (details.tabId < 0) return;

      const ct = details.responseHeaders?.find(h => h.name.toLowerCase() === 'content-type')?.value?.toLowerCase() || '';
      const isMedia = ct.startsWith('video/') || ct.startsWith('audio/') || 
                      ct.includes('vnd.apple.mpegurl') || ct.includes('x-mpegurl') ||
                      details.url.includes('googlevideo.com/videoplayback') ||
                      details.url.match(/\.(mp4|m4v|webm|mkv|m4a|mp3|m3u8|flv|ts)(\?.*)?$/i);

      if (isMedia && !details.url.startsWith('blob:')) {
        let cleanUrl = details.url;
        // Strip chunk ranges from YouTube streams so FDM downloads the full file with parallel streams
        if (cleanUrl.includes('googlevideo.com/videoplayback')) {
          cleanUrl = cleanUrl.replace(/([?&])range=[^&]+&?/g, '$1').replace(/[?&]$/, '');
        }

        const streams = tabMediaStreams.get(details.tabId) || [];
        if (!streams.some(s => s.url === cleanUrl)) {
          streams.unshift({ url: cleanUrl, contentType: ct, addedAt: Date.now() });
          tabMediaStreams.set(details.tabId, streams.slice(0, 15));
        }

        chrome.tabs.sendMessage(details.tabId, {
          type: 'mediaStreamDetected',
          url: cleanUrl,
          contentType: ct,
        }).catch(() => {});
      }
    },
    { urls: ['http://*/*', 'https://*/*'] },
    ['responseHeaders']
  );
} catch (e) {
  console.debug('[fdm] webRequest media listener setup:', e);
}


