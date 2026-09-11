// Generates docs/assets/hero.svg — the real download popup, start to finish.
//
// Two scenes in one loop: the transfer running with the numbers racing, then
// the finished state. It opens mid-download on purpose — the prompt beforehand
// is a form, and a form is not what anyone came to the README to see.
// Generated rather than hand-written because the per-frame keyframes are
// mechanical and easy to get subtly wrong by hand.
//
// The geometry deliberately matches apps/desktop/download_dialog.html at its
// real 560x330 window size — an earlier version was 880x290, which squashed the
// whole dialog into a letterbox and dropped the status grid and buttons
// entirely. If the dialog's layout changes, this should be re-measured against
// docs/assets/screenshot-popup.png.
//
// SVG rather than a GIF: sharp at any width, ~20 KB instead of megabytes, and a
// text file the repo can diff.
import { writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const out = join(dirname(fileURLToPath(import.meta.url)), '..', 'assets', 'hero.svg');

const W = 560, H = 330;
const DUR = 7;      // seconds per loop — short enough to read as fast
const A_END = 0;    // no prompt scene; the transfer starts immediately
const B_END = 82;   // transfer scene ends (%)
const FRAMES = 26;  // number updates during the transfer
const TOTAL = 6115318784;

const gb = b => (b / 1024 ** 3).toFixed(2);

/** Keyframes that show an element only between `from`% and `to`% of the loop. */
function show(name, from, to) {
  const p = [];
  if (from > 0) p.push(`0%,${(from - 0.01).toFixed(3)}%{opacity:0}`);
  p.push(`${from.toFixed(3)}%,${(to - 0.01).toFixed(3)}%{opacity:1}`);
  if (to < 100) p.push(`${to.toFixed(3)}%,100%{opacity:0}`);
  return `@keyframes ${name}{${p.join(' ')}}`;
}

// Eased so it reads like a transfer finding its speed, not a linear sweep.
const span = B_END - A_END;
const frames = Array.from({ length: FRAMES }, (_, i) => {
  const t = (i + 1) / FRAMES;
  const pct = (1 - Math.pow(1 - t, 1.7)) * 100;
  const left = Math.max(0, Math.round(74 * (1 - pct / 100)));
  return {
    at: A_END + span * (i / FRAMES),
    to: A_END + span * ((i + 1) / FRAMES),
    pct: pct.toFixed(1),
    done: gb(TOTAL * (pct / 100)),
    speed: (38 + Math.sin(i * 2.1) * 9 + t * 14).toFixed(2),
    eta: left > 59 ? `${Math.floor(left / 60)}m ${left % 60}s` : `${left}s`,
  };
});

const BAR = { x: 16, y: 118, w: 528, h: 14 };

const kf = [
  show('sceneB', A_END, B_END),
  show('sceneC', B_END, 100),
  `@keyframes fill{0%,${A_END}%{width:0}${B_END}%,100%{width:${BAR.w}px}}`,
  ...frames.map((f, i) => show(`n${i}`, f.at, f.to)),
].join('\n      ');

const cls = [
  `.sB{animation:sceneB ${DUR}s steps(1,end) infinite}`,
  `.sC{animation:sceneC ${DUR}s steps(1,end) infinite}`,
  `.bar{animation:fill ${DUR}s cubic-bezier(.2,.7,.3,1) infinite}`,
  ...frames.map((_, i) => `.n${i}{animation:n${i} ${DUR}s steps(1,end) infinite}`),
].join('\n      ');

const numbers = frames.map((f, i) => `
    <g class="n${i}">
      <text class="pct" x="16"  y="150">${f.pct}%</text>
      <text class="v"   x="528" y="188" text-anchor="end">${f.done} GB / 5.70 GB</text>
      <text class="v"   x="246" y="211" text-anchor="end">${f.speed} MB/s</text>
      <text class="v"   x="528" y="211" text-anchor="end">${f.eta}</text>
    </g>`).join('');

/** A pill button. */
const btn = (x, y, w, label, { fill = '#1a1e2f', stroke = '#334155', text = '#94a3b8', bold = false } = {}) => `
    <rect x="${x}" y="${y}" width="${w}" height="28" rx="6" fill="${fill}"${stroke ? ` stroke="${stroke}"` : ''}/>
    <text class="btn${bold ? ' b' : ''}" x="${x + w / 2}" y="${y + 18}" text-anchor="middle" fill="${text}">${label}</text>`;

const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" role="img"
     aria-label="The FDM download window: a 5.7 GB file transferring over 32 parallel connections at around 50 MB per second, then finishing.">
  <title>FDM — one file, many connections</title>
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0%" stop-color="#e50914"/><stop offset="55%" stop-color="#ff4b2b"/><stop offset="100%" stop-color="#2ecc71"/>
    </linearGradient>
    <clipPath id="c"><rect x="${BAR.x}" y="${BAR.y}" width="${BAR.w}" height="${BAR.h}" rx="4"/></clipPath>
    <style>
      ${kf}
      ${cls}
      .wm{font:800 17px 'Segoe UI',system-ui,sans-serif}
      .sub{font:600 11px 'Segoe UI',system-ui,sans-serif;fill:#94a3b8}
      .name{font:600 13px 'Segoe UI',system-ui,sans-serif;fill:#f8fafc}
      .url{font:400 10px 'Segoe UI',system-ui,sans-serif;fill:#64748b}
      .l{font:400 11px 'Segoe UI',system-ui,sans-serif;fill:#94a3b8}
      .v{font:600 11px 'Segoe UI',system-ui,sans-serif;fill:#f8fafc;font-variant-numeric:tabular-nums}
      .pct{font:700 13px 'Segoe UI',system-ui,sans-serif;fill:#f8fafc;font-variant-numeric:tabular-nums}
      .btn{font:500 11px 'Segoe UI',system-ui,sans-serif}
      .btn.b{font-weight:600}
      .big{font:700 17px 'Segoe UI',system-ui,sans-serif;fill:#f8fafc}
      /* Reduced motion gets the finished state, not a frozen empty bar. */
      @media (prefers-reduced-motion:reduce){
        .sB{animation:none;opacity:0}
        .sC{animation:none;opacity:1}
        .bar{animation:none;width:${BAR.w}px}
        ${frames.map((_, i) => `.n${i}{animation:none;opacity:0}`).join('')}
      }
    </style>
  </defs>

  <rect width="${W}" height="${H}" rx="10" fill="#020617"/>
  <rect x=".5" y=".5" width="${W - 1}" height="${H - 1}" rx="10" fill="none" stroke="#334155"/>

  <!-- Title bar -->
  <path d="M0 10a10 10 0 0 1 10-10h540a10 10 0 0 1 10 10v26H0z" fill="#1a1e2f"/>
  <g transform="translate(14 10)">
    <path d="M8 .8L1.4 4.6V12.2L8 16L14.6 12.2V4.6L8 .8Z" fill="none" stroke="#e50914" stroke-width="1.5" stroke-linejoin="round"/>
    <path d="M8 4.2V11M8 11L5 8.3M8 11L11 8.3" fill="none" stroke="#e50914" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
  </g>
  <text class="wm" x="36" y="24" fill="#e50914">FDM</text>
  <text class="sub sB" x="78" y="24">Download Status</text>
  <text class="sub sC" x="78" y="24">Download Complete</text>
  <path d="M496 18h12" stroke="#94a3b8" stroke-width="1.3"/>
  <path d="M526 13l10 10M536 13l-10 10" stroke="#94a3b8" stroke-width="1.3"/>

  <!-- ===== Scene B — the transfer ===== -->
  <g class="sB">
    <rect x="16" y="48" width="528" height="46" rx="8" fill="#0e1223" stroke="#334155"/>
    <rect x="30" y="60" width="22" height="22" rx="3" fill="none" stroke="#f59e0b" stroke-width="1.6"/>
    <path d="M30 66h22" stroke="#f59e0b" stroke-width="1.6"/>
    <text class="name" x="62" y="70">ubuntu-24.04.1-desktop-amd64.iso</text>
    <text class="url"  x="62" y="84">https://releases.ubuntu.com/24.04/ubuntu-24.04.1-desktop-amd64.iso</text>

    <rect x="${BAR.x}" y="${BAR.y}" width="${BAR.w}" height="${BAR.h}" rx="4" fill="#1a1e2f"/>
    <text class="l" x="544" y="150" text-anchor="end">32 parallel streams</text>

    <rect x="16" y="166" width="528" height="84" rx="8" fill="#0e1223" stroke="#334155"/>
    <text class="l" x="32"  y="188">Status:</text>
    <text class="v" x="246" y="188" text-anchor="end" style="fill:#38bdf8">Downloading (32 connections)</text>
    <text class="l" x="286" y="188">File size:</text>
    <text class="l" x="32"  y="211">Transfer rate:</text>
    <text class="l" x="286" y="211">Time left:</text>
    <text class="l" x="32"  y="234">Resume capability:</text>
    <text class="v" x="246" y="234" text-anchor="end">Yes</text>
    <text class="l" x="286" y="234">Save to:</text>
    <text class="v" x="528" y="234" text-anchor="end" style="font-weight:400">...\\FDM\\Programs\\ubuntu-24.04.1.iso</text>
${btn(16, 276, 104, 'Open Manager')}
${btn(316, 276, 90, 'Open Folder')}
${btn(414, 276, 60, 'Pause')}
${btn(482, 276, 62, 'Cancel', { fill: '#475569', stroke: '', text: '#f8fafc' })}
  </g>
  <g class="sB" clip-path="url(#c)">
    <rect class="bar" x="${BAR.x}" y="${BAR.y}" height="${BAR.h}" fill="url(#g)"/>
  </g>
  <g class="sB">${numbers}
  </g>

  <!-- ===== Scene C — done ===== -->
  <g class="sC">
    <circle cx="280" cy="92" r="22" fill="none" stroke="#2ecc71" stroke-width="2.5"/>
    <path d="M270 92l7.5 7.5 14-15" fill="none" stroke="#2ecc71" stroke-width="3.2" stroke-linecap="round" stroke-linejoin="round"/>
    <text class="big" x="280" y="142" text-anchor="middle">Download Complete</text>

    <rect x="132" y="158" width="296" height="30" rx="6" fill="#0e1223" stroke="#e50914"/>
    <text class="v" x="152" y="177">ubuntu-24.04.1-desktop-amd64.iso</text>
    <rect x="364" y="165" width="52" height="16" rx="8" fill="#1d0b12"/>
    <text class="btn b" x="390" y="176" text-anchor="middle" fill="#ff4b4b">DRAG</text>

    <rect x="16" y="200" width="528" height="52" rx="8" fill="#0e1223" stroke="#334155"/>
    <text class="l" x="32"  y="220">Saved to:</text>
    <text class="v" x="528" y="220" text-anchor="end" style="font-weight:400">C:\\Users\\You\\Downloads\\FDM\\Programs</text>
    <text class="l" x="32"  y="240">Total size:</text>
    <text class="v" x="528" y="240" text-anchor="end">5.70 GB  ·  1m 14s</text>
${btn(112, 276, 92, 'Open File', { fill: '#2ecc71', stroke: '', text: '#052e16', bold: true })}
${btn(212, 276, 92, 'Open Folder')}
${btn(312, 276, 100, 'Open Manager')}
${btn(420, 276, 64, 'Close')}
  </g>
</svg>
`;

writeFileSync(out, svg);
console.log(`wrote docs/assets/hero.svg (${(svg.length / 1024).toFixed(1)} KB, ${W}x${H})`);
