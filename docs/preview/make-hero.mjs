// Generates docs/assets/hero.svg — the download popup, start to finish.
//
// Three scenes in one loop: the Start Download prompt with a cursor moving in
// and clicking, the transfer running with the numbers racing, then the finished
// state. Generated rather than hand-written because the per-frame keyframes are
// mechanical and easy to get subtly wrong by hand.
//
// SVG rather than a GIF: it stays sharp at any width, weighs a few KB instead of
// megabytes, and is a text file the repo can diff.
import { writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const out = join(dirname(fileURLToPath(import.meta.url)), '..', 'assets', 'hero.svg');

const DUR = 9;            // seconds per loop
const A_END = 24;         // prompt scene ends (%)
const B_END = 84;         // transfer scene ends (%)
const FRAMES = 20;        // number updates during the transfer
const TOTAL = 6115318784; // 5.7 GB

const gb = b => (b / 1024 ** 3).toFixed(2);

/** A keyframe that shows an element only between `from`% and `to`%. */
function window_(name, from, to) {
  const p = [];
  if (from > 0) p.push(`0%,${(from - 0.01).toFixed(3)}%{opacity:0}`);
  p.push(`${from.toFixed(3)}%,${(to - 0.01).toFixed(3)}%{opacity:1}`);
  if (to < 100) p.push(`${to.toFixed(3)}%,100%{opacity:0}`);
  return `@keyframes ${name}{${p.join(' ')}}`;
}

// Numbers race with an ease-out curve, so it looks like a real transfer finding
// its speed rather than a linear sweep.
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
    speed: (38 + Math.sin(i * 2.1) * 9 + t * 14).toFixed(1),
    eta: left > 59 ? `${Math.floor(left / 60)}m ${left % 60}s` : `${left}s`,
  };
});

const kf = [
  window_('sceneA', 0, A_END),
  window_('sceneB', A_END, B_END),
  window_('sceneC', B_END, 100),
  // The bar only moves while the transfer scene is on screen.
  `@keyframes fill{0%,${A_END}%{width:0}${B_END}%,100%{width:760px}}`,
  // Cursor glides to the button, then the button flashes on contact.
  `@keyframes cursor{0%{transform:translate(560px,238px)}` +
    `14%{transform:translate(560px,238px)}` +
    `${(A_END * 0.72).toFixed(1)}%{transform:translate(676px,250px)}` +
    `100%{transform:translate(676px,250px)}}`,
  `@keyframes press{0%,${(A_END * 0.72).toFixed(1)}%{opacity:0}` +
    `${(A_END * 0.78).toFixed(1)}%{opacity:.55}` +
    `${(A_END * 0.95).toFixed(1)}%,100%{opacity:0}}`,
  ...frames.map((f, i) => window_(`n${i}`, f.at, f.to)),
].join('\n      ');

const cls = [
  `.sA{animation:sceneA ${DUR}s steps(1,end) infinite}`,
  `.sB{animation:sceneB ${DUR}s steps(1,end) infinite}`,
  `.sC{animation:sceneC ${DUR}s steps(1,end) infinite}`,
  `.bar{animation:fill ${DUR}s cubic-bezier(.2,.7,.3,1) infinite}`,
  `.cur{animation:cursor ${DUR}s cubic-bezier(.4,0,.2,1) infinite,sceneA ${DUR}s steps(1,end) infinite}`,
  `.press{animation:press ${DUR}s linear infinite}`,
  ...frames.map((_, i) => `.n${i}{animation:n${i} ${DUR}s steps(1,end) infinite}`),
].join('\n      ');

// Segment boundaries painted over the fill in the background colour — the same
// trick the real progress bar uses to make parallel connections visible.
const ticks = Array.from({ length: 15 }, (_, i) =>
  `<rect x="${(60 + (i + 1) * 47.5).toFixed(1)}" y="150" width="2" height="20"/>`).join('');

const numbers = frames.map((f, i) => `
    <g class="n${i}">
      <text class="pct"  x="60"  y="196">${f.pct}%</text>
      <text class="val"  x="820" y="196" text-anchor="end">${f.done} GB of 5.70 GB</text>
      <text class="val"  x="146" y="240">${f.speed} MB/s</text>
      <text class="val"  x="420" y="240">${f.eta}</text>
    </g>`).join('');

const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 880 290" width="880" height="290" role="img"
     aria-label="The FDM download window: pressing Start Download, then a 5.7 GB file transferring over 32 parallel connections at around 50 MB per second, then finishing.">
  <title>FDM — one file, many connections</title>
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0%" stop-color="#e50914"/><stop offset="55%" stop-color="#ff4b2b"/><stop offset="100%" stop-color="#2ecc71"/>
    </linearGradient>
    <clipPath id="c"><rect x="60" y="150" width="760" height="20" rx="4"/></clipPath>
    <style>
      ${kf}
      ${cls}
      .wm{font:800 24px 'Segoe UI',system-ui,sans-serif}
      .sub{font:600 13px 'Segoe UI',system-ui,sans-serif}
      .name{font:600 16px 'Segoe UI',system-ui,sans-serif}
      .url{font:400 12px 'Segoe UI',system-ui,sans-serif}
      .lab{font:400 13px 'Segoe UI',system-ui,sans-serif;fill:#64748b}
      .val{font:600 13px 'Segoe UI',system-ui,sans-serif;font-variant-numeric:tabular-nums;fill:#f8fafc}
      .pct{font:700 15px 'Segoe UI',system-ui,sans-serif;font-variant-numeric:tabular-nums;fill:#f8fafc}
      .btn{font:600 13px 'Segoe UI',system-ui,sans-serif}
      .big{font:700 20px 'Segoe UI',system-ui,sans-serif;fill:#f8fafc}
      /* Reduced motion gets the finished state, not a frozen empty one. */
      @media (prefers-reduced-motion:reduce){
        .sA,.cur,.press{animation:none;opacity:0}
        .sB{animation:none;opacity:0}
        .sC{animation:none;opacity:1}
        .bar{animation:none;width:760px}
        ${frames.map((_, i) => `.n${i}{animation:none;opacity:0}`).join('')}
      }
    </style>
  </defs>

  <rect width="880" height="290" rx="12" fill="#020617"/>
  <rect x=".5" y=".5" width="879" height="289" rx="12" fill="none" stroke="#1a1e2f"/>
  <path d="M0 12a12 12 0 0 1 12-12h856a12 12 0 0 1 12 12v40H0z" fill="#1a1e2f"/>
  <g transform="translate(26 13)">
    <path d="M12 1L2 7V17L12 23L22 17V7L12 1Z" fill="none" stroke="#e50914" stroke-width="2" stroke-linejoin="round"/>
    <path d="M12 6V16M12 16L7.5 12M12 16L16.5 12" fill="none" stroke="#e50914" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
  </g>
  <text class="wm" x="60" y="34" fill="#e50914">FDM</text>
  <text class="sub sA" x="120" y="34" fill="#94a3b8">Download File Info</text>
  <text class="sub sB" x="120" y="34" fill="#94a3b8">Download Status</text>
  <text class="sub sC" x="120" y="34" fill="#94a3b8">Download Complete</text>

  <!-- Scene A — the prompt, and a cursor arriving at Start Download -->
  <g class="sA">
    <rect x="30" y="72" width="820" height="126" rx="8" fill="#0e1223" stroke="#334155"/>
    <text class="lab" x="56" y="102">URL:</text>
    <rect x="150" y="86" width="670" height="24" rx="4" fill="#020617" stroke="#334155"/>
    <text class="val" x="162" y="103" style="font-weight:400;fill:#94a3b8">https://releases.ubuntu.com/24.04/ubuntu-24.04.1-desktop-amd64.iso</text>
    <text class="lab" x="56" y="140">File Name:</text>
    <rect x="150" y="124" width="670" height="24" rx="4" fill="#020617" stroke="#334155"/>
    <text class="val" x="162" y="141" style="font-weight:400">ubuntu-24.04.1-desktop-amd64.iso</text>
    <text class="lab" x="56" y="178">Save To:</text>
    <rect x="150" y="162" width="640" height="24" rx="4" fill="#020617" stroke="#334155"/>
    <text class="val" x="162" y="179" style="font-weight:400;fill:#94a3b8">C:\\Users\\You\\Downloads\\FDM\\Programs</text>
    <rect x="796" y="162" width="24" height="24" rx="4" fill="#1a1e2f" stroke="#334155"/>
    <text x="803" y="179" style="font-size:13px">📂</text>

    <rect x="30" y="236" width="130" height="32" rx="6" fill="#1a1e2f" stroke="#334155"/>
    <text class="btn" x="52" y="256" fill="#94a3b8">Download Later</text>
    <rect x="646" y="236" width="150" height="32" rx="6" fill="#e50914"/>
    <rect class="press" x="646" y="236" width="150" height="32" rx="6" fill="#ffffff"/>
    <text class="btn" x="678" y="256" fill="#ffffff">▶  Start Download</text>
    <rect x="806" y="236" width="44" height="32" rx="6" fill="#1a1e2f" stroke="#334155"/>
    <text class="btn" x="815" y="256" fill="#94a3b8">Esc</text>
  </g>
  <g class="cur">
    <path d="M0 0 L0 14 L3.6 10.6 L6 16 L8.4 15 L6 9.6 L11 9.6 Z" fill="#f8fafc" stroke="#020617" stroke-width="1.2"/>
  </g>

  <!-- Scene B — the transfer -->
  <g class="sB">
    <rect x="30" y="76" width="820" height="54" rx="8" fill="#0e1223" stroke="#334155"/>
    <text class="name" x="60" y="100" fill="#f8fafc">ubuntu-24.04.1-desktop-amd64.iso</text>
    <text class="url"  x="60" y="118" fill="#64748b">https://releases.ubuntu.com/24.04/ubuntu-24.04.1-desktop-amd64.iso</text>
    <rect x="60" y="150" width="760" height="20" rx="4" fill="#1a1e2f"/>
    <text class="lab" x="60"  y="240">Speed</text>
    <text class="lab" x="360" y="240">Time left</text>
    <text class="lab" x="620" y="240">Connections</text>
    <text class="val" x="820" y="240" text-anchor="end">32 parallel</text>
  </g>
  <g class="sB" clip-path="url(#c)">
    <rect class="bar" x="60" y="150" height="20" fill="url(#g)"/>
    <g fill="#020617">${ticks}</g>
  </g>
  <g class="sB">${numbers}
  </g>

  <!-- Scene C — done -->
  <g class="sC">
    <circle cx="440" cy="112" r="26" fill="none" stroke="#2ecc71" stroke-width="3"/>
    <path d="M428 112l9 9 17-18" fill="none" stroke="#2ecc71" stroke-width="4" stroke-linecap="round" stroke-linejoin="round"/>
    <text class="big" x="440" y="172" text-anchor="middle">Download Complete</text>
    <text class="val" x="440" y="198" text-anchor="middle" style="font-weight:400;fill:#94a3b8">ubuntu-24.04.1-desktop-amd64.iso  ·  5.70 GB  ·  1m 14s</text>
    <rect x="318" y="222" width="110" height="32" rx="6" fill="#2ecc71"/>
    <text class="btn" x="344" y="242" fill="#052e16">Open File</text>
    <rect x="440" y="222" width="122" height="32" rx="6" fill="#1a1e2f" stroke="#334155"/>
    <text class="btn" x="462" y="242" fill="#94a3b8">Open Folder</text>
  </g>
</svg>
`;

writeFileSync(out, svg);
console.log(`wrote docs/assets/hero.svg (${(svg.length / 1024).toFixed(1)} KB)`);
