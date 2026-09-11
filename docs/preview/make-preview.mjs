// Renders the REAL app UI for screenshots.
//
// Not a mock-up: this loads apps/desktop/index.html, its real stylesheets and
// the real app.js, and only stubs the Tauri bridge so the same rendering code
// that draws rows in the app draws them here. If the UI changes, this changes
// with it — a hand-drawn approximation would start lying the first time
// someone edited a class name.
import { readFileSync, writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const appDir = join(here, '..', '..', 'apps', 'desktop');

const DOWNLOADS = [
  { id: 1, filename: 'ubuntu-24.04.1-desktop-amd64.iso', url: 'https://releases.ubuntu.com/24.04/ubuntu-24.04.1-desktop-amd64.iso',
    status: 'downloading', total: 6115318784, downloaded: 3837652992, speed_bps: 50553651, eta_secs: 45,
    segments: 32, active_connections: 32, category: 'Programs', resumable: true },
  { id: 2, filename: 'Blender 4.2 — Full Feature Reveal (2160p).mp4', url: 'https://www.youtube.com/watch?v=r1TCiLgN2Lo',
    status: 'downloading', total: 1476395008, downloaded: 221249536, speed_bps: 18874368, eta_secs: 66,
    segments: 16, active_connections: 16, category: 'Video', resumable: true },
  { id: 3, filename: 'annual-report-2026.pdf', url: 'https://example.com/reports/annual-report-2026.pdf',
    status: 'completed', total: 8471552, downloaded: 8471552, speed_bps: 0, segments: 8,
    category: 'Documents', path: 'C:\Users\You\Downloads\FDM\Documents\annual-report-2026.pdf', resumable: false },
  { id: 4, filename: 'godot-4.4-stable-win64.zip', url: 'https://downloads.godotengine.org/godot-4.4-stable-win64.zip',
    status: 'paused', total: 134217728, downloaded: 78643200, speed_bps: 0, segments: 16,
    category: 'Compressed', resumable: true },
  { id: 5, filename: 'interstellar-soundtrack.mp3', url: 'https://example.com/audio/interstellar-soundtrack.mp3',
    status: 'completed', total: 14680064, downloaded: 14680064, speed_bps: 0, segments: 8,
    category: 'Music', path: 'C:\Users\You\Downloads\FDM\Music\interstellar-soundtrack.mp3', resumable: false },
];

const stub = `
<script>
(() => {
  // Stand in for the Tauri bridge so the real app.js runs in a plain browser.
  const DOWNLOADS = ${JSON.stringify(DOWNLOADS)};
  const CONFIG = {
    maxActive: 4, maxConnections: 32,
    downloadRoot: 'C:\\Users\\You\\Downloads\\FDM',
    tempDir: 'C:\\Users\\You\\AppData\\Local\\FDM\\Temp',
    useTempDir: true, categoryDirs: {},
    categories: ['Documents','Video','Music','Images','Compressed','Programs','Other'],
  };
  const handlers = {
    list_downloads: () => DOWNLOADS,
    get_config: () => CONFIG,
    get_download: ({ id }) => DOWNLOADS.find(d => d.id === id) || null,
  };
  const invoke = async (cmd, args) => (handlers[cmd] ? handlers[cmd](args || {}) : null);
  window.__TAURI__ = { core: { invoke }, event: { listen: async () => () => {} } };
  window.__TAURI_INTERNALS__ = { invoke };
})();
</script>
`;

let html = readFileSync(join(appDir, 'index.html'), 'utf8');
// Point relative assets back at the real app directory, and insert the stub
// before app.js so the bridge exists by the time it runs.
html = html.replace(/(href|src)="(?!http)([^"]+)"/g, (_, a, p) => `${a}="../../apps/desktop/${p}"`);
html = html.replace('<script type="module"', stub + '  <script type="module"');
writeFileSync(join(here, 'app-preview.html'), html);
console.log('wrote docs/preview/app-preview.html');

// --- the per-download popup, mid-transfer -----------------------------------
const ACTIVE = {
  id: 1, filename: 'ubuntu-24.04.1-desktop-amd64.iso',
  url: 'https://releases.ubuntu.com/24.04/ubuntu-24.04.1-desktop-amd64.iso',
  status: 'downloading', total: 6115318784, downloaded: 3837652992,
  speed_bps: 50553651, eta_secs: 45, segments: 32, active_connections: 32,
  category: 'Programs', resumable: true,
  path: String.raw`C:\Users\You\Downloads\FDM\Programs\ubuntu-24.04.1-desktop-amd64.iso`,
};

const dialogStub = `
<script>
(() => {
  const ACTIVE = ${JSON.stringify(ACTIVE)};
  const invoke = async (cmd) => (cmd === 'get_download' ? ACTIVE : (cmd === 'get_target_dir' ? ACTIVE.path : null));
  window.__TAURI__ = { core: { invoke }, event: { listen: async () => () => {} } };
  window.__TAURI_INTERNALS__ = { invoke };
})();
</script>
`;

let dlg = readFileSync(join(appDir, 'download_dialog.html'), 'utf8');
dlg = dlg.replace(/(href|src)="(?!http)([^"]+)"/g, (_, a, p) => `${a}="../../apps/desktop/${p}"`);
dlg = dlg.replace('</head>', dialogStub + '</head>');
writeFileSync(join(here, 'dialog-preview.html'), dlg);
console.log('wrote docs/preview/dialog-preview.html');
