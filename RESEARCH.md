# FDM — Feasibility & Architecture Research

**Developer:** Ahmad Farooq
**Goal:** Windows download manager + Chrome extension, IDM-class features, faster than IDM, auto-sorts files by type.
**Date:** 2026-08-23
**Verdict:** Buildable. Nothing in the plan is technically blocked. Four constraints below change *how* we build it.

---

## 1. BLOCKER TO DECIDE: the name "FDM" is taken

"FDM" is the established abbreviation for **Free Download Manager** by **SoftDeluxe** — shipping since 2004, C++, ~30 languages, Windows/macOS/Linux/Android, and it is a *direct competitor* in exactly this category.

Consequences if we ship as "FDM":
- **Chrome Web Store**: listing name conflict; CWS policy prohibits misleading listings and impersonation. High rejection risk.
- **SEO**: unwinnable. Every search for "FDM download manager" returns freedownloadmanager.org.
- **Trademark**: they have 20+ years of common-law use in this exact product category. This is the classic setup for a takedown.

**Recommendation:** keep `FDM` as the internal repo/codename, pick a distinct public brand. Also note Free Download Manager's own history is a cautionary tale: their site distributed a Linux backdoor from 2020, discovered Sept 2023 (supply-chain attack) — do not inherit their name's baggage.

Source: https://en.wikipedia.org/wiki/Free_Download_Manager

---

## 2. BLOCKER: the installer CANNOT silently install the Chrome extension

This is the single biggest gap between the plan and reality.

> "On Windows and macOS, the `update_URL` must point to the Chrome Web Store."
> "As of Chrome 33, no external installs are allowed from a path to a local CRX file on Windows."
> Users "will have to enable the extension using the following confirmation dialog."

— https://developer.chrome.com/docs/extensions/how-to/distribute/install-extensions

**What this means concretely:**

| Plan | Reality |
|---|---|
| Bundle `.crx` in setup, install silently | ❌ Blocked since Chrome 33 |
| Setup writes registry → extension appears | ✅ Works, **but** extension must already be published on Chrome Web Store |
| No user interaction needed | ❌ Chrome always shows an enable-confirmation dialog |
| Force it on if user declines | ❌ Removal blocklists it; Chrome docs say respect that |

**The actual working flow (this is exactly what IDM does):**

1. Publish the extension to the Chrome Web Store → get the extension ID.
2. Installer writes:
   - `HKLM\Software\Wow6432Node\Google\Chrome\Extensions\<extension-id>` (64-bit Windows)
   - value `update_url` = `https://clients2.google.com/service/update2/crx`
3. Installer registers the **native messaging host**:
   - `HKLM\SOFTWARE\Google\Chrome\NativeMessagingHosts\com.fdm.host` → full path to host manifest JSON
4. On next Chrome launch, user sees an enable prompt and clicks Enable.
5. Uninstaller must delete both registry keys.

Also needed: separate registry paths for **Edge** (`HKLM\SOFTWARE\Microsoft\Edge\...`), **Brave/Vivaldi/Opera** (Chromium paths), and **Firefox** (different manifest location + `allowed_extensions` instead of `allowed_origins`).

The app must also handle the "user never enabled it" case gracefully — in-app banner with a one-click store link, plus clipboard-monitoring fallback so the product still works without the extension.

---

## 3. `setup.bat` is the wrong installer — use Inno Setup

A `.bat` installer will fail in practice:
- No UAC elevation → cannot write `HKLM` or `Program Files`
- No entry in Add/Remove Programs, no uninstaller
- Cannot be code-signed → SmartScreen + AV will flag it
- AV heuristics treat "batch file that writes registry and drops an exe" as textbook malware behavior

**Use Inno Setup** (used by VS Code, Git for Windows): registry writes, per-user and per-machine installs, Pascal scripting for the browser-detection logic, signed install *and* uninstall, ~1.78 MB overhead. Commercial users are asked to buy a commercial license.
Source: https://jrsoftware.org/isinfo.php

### Code signing is not optional for this product category

SmartScreen "provides reputation checks for apps, checking downloaded programs **and the digital signature used to sign a file**. If a URL, a file, an app, or a certificate has an established reputation, users don't see any warnings. If there's no reputation, the item is marked as a higher risk and presents a warning."
Source: https://learn.microsoft.com/en-us/windows/security/operating-system-security/virus-and-threat-protection/microsoft-defender-smartscreen/

Download managers are among the most false-positive-prone categories on VirusTotal (they spawn network connections, write to Downloads, install browser components, register native hosts). Budget for an Authenticode certificate and expect to submit false-positive reports to Microsoft and major AV vendors early. Reputation accrues to the certificate, so use the same cert for every build and never let it lapse.

---

## 4. Browser integration under Manifest V3 — what still works

MV3 removed blocking `webRequest`. `declarativeNetRequest` is declarative-only and **cannot** hand a request off to a native app. But observation survives:

> Remove `"webRequest"` only "if you no longer need to observe network requests."
Source: https://developer.chrome.com/docs/extensions/develop/migrate/blocking-web-requests

### The working capture pipeline

```
User clicks link
   ↓
chrome.downloads.onCreated  → inspect URL / MIME / size
   ↓
chrome.downloads.onDeterminingFilename → suggest(), learn Chrome's filename
   ↓
chrome.downloads.cancel(id)
   ↓
chrome.runtime.connectNative("com.fdm.host")
   ↓  post { url, filename, cookies, referer, userAgent, method, headers }
Native host (persistent port) takes over the download
```

**Hard constraints found in the API docs** (https://developer.chrome.com/docs/extensions/reference/api/downloads):

- `onDeterminingFilename`: **one listener per extension**; must call `suggest()` **exactly once**; return `true` for async. Paths must be **relative to the Downloads dir** — absolute paths and `..` are ignored. So *Chrome* can't put the file in our sorted folders; **our native engine does the sorting**.
- `cancel()` is **racy**: when it resolves, "the download is cancelled, completed, interrupted or doesn't exist anymore." Small/fast files will sometimes slip through to Chrome. Need dedup logic so the user doesn't get two copies.
- `onChanged` does **not** fire for `bytesReceived` — it is not a progress meter. All progress UI must come from our own engine.
- `setUiOptions` (Chrome 105+, needs `downloads.ui` permission) hides Chrome's own download bubble profile-wide — needed so the handoff looks seamless.

### Things that will *not* capture cleanly (document these as known limits)

- `blob:` and `data:` URLs — no re-fetchable URL exists
- Downloads requiring a POST body or short-lived auth token (Google Drive, some SharePoint/OneDrive links)
- Anything behind a service-worker-generated response

For these, let Chrome handle it and don't cancel. Detect and skip rather than corrupt.

### Session handover is mandatory for correctness

The native engine re-issues the HTTP request from scratch, so the extension must pass **cookies** (`cookies` permission + host permissions), `Referer`, `User-Agent`, and any auth headers it can see. Without this, protected downloads return HTML login pages instead of the file. This is exactly why IDM asks for broad site access.

### Native messaging protocol details

- Transport: stdio, JSON, UTF-8, prefixed with a **32-bit length in native byte order**
- Limits: **1 MB** host→Chrome per message, **64 MiB** Chrome→host
- `allowed_origins` **cannot contain wildcards** → the published extension ID must be baked into the host manifest
- Use `connectNative()` (persistent port, one long-lived process), not `sendNativeMessage()` (spawns a process per message)
- **Windows gotcha:** set stdio to `O_BINARY` via `__setmode`, or default text mode rewrites `\n` → `\r\n` and corrupts every message
- Debug output goes to **stderr** only — anything on stdout breaks the protocol
- Chrome passes `--parent-window=<handle>` on Windows (0 when called from a service worker, which is our case)

Source: https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging

### Chrome Web Store review risk

Highest-risk areas for this listing: permission necessity/justification (`downloads`, `cookies`, `nativeMessaging`, broad host permissions), full disclosure of what gets sent to the native host, a compliant privacy policy with Limited Use, and no remotely hosted code. Note Free Download Manager **removed YouTube downloading in Oct 2021 after a complaint from Google** — assume video-from-YouTube is a store-review liability and keep it out of the extension listing's advertised feature set.
Source: https://developer.chrome.com/docs/webstore/program-policies

---

## 5. How to actually be faster than IDM

IDM advertises "up to 8x" via dynamic segmentation: it splits during transfer, and "as each new connection opens, IDM locates the biggest remaining segment and splits it in two," reusing existing connections without re-doing connect/login.
Source: https://www.internetdownloadmanager.com/features.html

We must match that, then add what IDM lacks.

### Baseline: correct segmented downloading

Per https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/Range_requests:

1. `HEAD url` → require `Accept-Ranges: bytes` (absent or `none` = no parallelism) and read `Content-Length`
2. Capture a **strong validator** (`ETag`, else `Last-Modified`)
3. Split into N segments, `Range: bytes=<start>-<end>` + `If-Range: <validator>` on every request
4. Per response:
   - `206` → verify `Content-Range`, write body at `<start>`
   - `200` → **range ignored or resource changed.** Abort all segments; write this one stream sequentially. Writing a `200` body at a segment offset silently corrupts the file — this is the #1 bug in amateur downloaders.
   - `416` → file shrank; restart discovery
5. Resume = re-request only missing ranges, again with `If-Range`
6. No `Accept-Ranges` → single sequential GET, pause/resume disabled in UI

### Where we beat IDM

| Technique | IDM | Us | Why it wins |
|---|---|---|---|
| **HTTP/3 (QUIC)** | ✗ | ✓ | No head-of-line blocking, faster handshake, far better on lossy/mobile links. Biggest real-world gap. |
| **Sparse file + direct offset writes** | Merges segments at the end | Preallocate sparse file, each segment writes at its true offset | Zero merge pass. On a 10 GB file IDM spends real time reassembling; we finish when the last byte lands. |
| **Dynamic segment splitting** | ✓ | ✓ (must match) | Kills the "99% stuck on one slow chunk" tail. |
| **Adaptive connection count** | Fixed (max 32) | Probe throughput, add/remove connections until marginal gain flattens; learn per-host | Some CDNs throttle or 403 on too many connections. Static N is wrong for both fast and hostile hosts. |
| **Per-host policy learning** | ✗ | ✓ | Remember "this host caps at 4 connections" / "this host ignores Range" so retries are instant next time. |
| **Connection reuse + keep-alive pool, DNS cache, Happy Eyeballs (v4/v6 race)** | Partial | ✓ | Cuts per-segment setup cost, which dominates on many small files. |
| **Multi-source / mirror + Metalink** | ✗ | ✓ | Pull one file from several mirrors at once. |
| **Windows overlapped I/O / IOCP, coalesced writes** | ✗ | ✓ | Avoids disk becoming the bottleneck at high segment counts, reduces fragmentation. |
| **Large socket buffers / window tuning** | ✗ | ✓ | Needed to saturate high bandwidth-delay-product links (long-haul, satellite). |

Honest note: on a well-provisioned single-origin CDN that already saturates your line, no download manager is faster than a plain browser download. The wins are concentrated in: per-connection-throttled servers, lossy links, high-latency links, many-small-files batches, and resume-after-failure. Market it on those, not on fake "500% faster" claims.

---

## 6. Auto-organization by file type

Fully achievable, and it must live in the **native engine** — `onDeterminingFilename` can only suggest paths relative to Chrome's Downloads dir, so Chrome cannot place files in our tree.

Proposed default under `%USERPROFILE%\Downloads\FDM\`:

```
Documents/    pdf doc docx xls xlsx ppt pptx txt rtf odt csv epub
Video/        mp4 mkv avi mov wmv flv webm m4v mpg ts
Music/        mp3 flac wav aac m4a ogg opus wma
Images/       jpg jpeg png gif webp svg bmp tiff heic
Compressed/   zip rar 7z tar gz bz2 xz iso
Programs/     exe msi msix appx bat cmd
Other/        everything unmatched
```

Design rules:
- Classify on **extension first**, then `Content-Type`, then magic-bytes sniff of the first segment (extensions lie; `Content-Type: application/octet-stream` is everywhere).
- Every category, its extension list, and its target folder must be **user-editable** in Settings — this is table stakes for an IDM alternative.
- Optional per-category subfolder templates (`Video/{site}/`, `{yyyy-MM}/`).
- Collision policy: `file (2).ext`, or overwrite, or ask — user choice.
- Never write outside the configured root; sanitize server-supplied filenames (path traversal, reserved Windows names `CON`/`PRN`/`AUX`/`NUL`/`COM1`, trailing dots/spaces, >260 char paths → use `\\?\` prefix).
- Migration tool for users switching from IDM's flat Downloads folder.

---

## 7. Competitive landscape

| Product | Stack | License | Notes |
|---|---|---|---|
| **IDM** | Native Windows | Paid, 30-day trial | The target. Dynamic segmentation, "up to 8x", scheduler, site grabber, quota limiting, categories, ~30 languages. No HTTP/3. Dated UI. |
| **Free Download Manager** | C++ | Freeware, proprietary (was GPL through 3.9.7) | Owns the "FDM" name. YouTube support removed Oct 2021 after Google complaint. 2023 supply-chain malware incident on their own site. |
| **XDM** | Java + Maven | GPL-2.0 | Closest open-source IDM clone. Claims "up to 500%". Java runtime is a distribution burden; **819 open issues / 497 commits** — effectively under-maintained. Good behavior reference, bad code to inherit. |
| **Motrix Turbo v2** | Electron 43, React 19, TS, forked **aria2** | MIT | **Already implements our exact architecture:** MV3 Chrome+Firefox extensions paired via native messaging, 4-layer renderer→core→engine-adapter→aria2, JSON-RPC protocol (MDXP), SQLite session recovery, plugin sandbox. Study this closely. Weakness: Electron footprint, no HTTP/3, no IDM-style dynamic splitting. |
| **aria2** | C++ | GPL-2.0 | Best available engine: HTTP(S)/FTP/SFTP/BitTorrent/Metalink, segmented, multi-source, JSON-RPC over HTTP+WebSocket, `libaria2` C++ library. **GPL-2.0 is the catch — see §8.** |

**Gap in the market:** no download manager currently ships HTTP/3, a modern native Windows UI, IDM-grade dynamic segmentation, *and* a permissive license. That's the opening.

---

## 8. Licensing landmines

| Component | License | Verdict for a commercial product |
|---|---|---|
| **aria2 / libaria2** | GPL-2.0 | ⚠️ Linking `libaria2` makes our app GPL. Separate-process + JSON-RPC is the usual workaround but is legally contested and forces us to ship aria2 source + license. **Recommendation: write our own engine.** We need HTTP/3 and IDM-style dynamic splitting anyway, neither of which aria2 gives us. |
| **XDM** | GPL-2.0 | Cannot copy code into a proprietary product. Read for behavior only. |
| **yt-dlp** (repo / PyPI wheel) | **Unlicense** | ✅ Safe to bundle |
| **yt-dlp** (prebuilt PyInstaller `.exe`) | **GPLv3+** | ❌ "the PyInstaller-bundled executables include GPLv3+ licensed code" — do not ship this binary in a proprietary app |
| **ffmpeg** | LGPL **or** GPL depending on build | ⚠️ Must use an LGPL build and dynamically link, or shell out to a separately-installed binary. "ffmpeg's license depends on the build." |
| **Inno Setup** | Custom; commercial users asked to purchase | Budget for it |
| **Tauri** | Check LICENSE in repo (docs site is CC-BY/MIT) | Verify before shipping |

Sources: https://github.com/yt-dlp/yt-dlp · https://github.com/aria2/aria2 · https://github.com/agalwood/Motrix

---

## 9. Recommended architecture

```
┌─────────────────────────────────────────────────────────┐
│ Browser extensions (MV3)                                │
│ Chrome · Edge · Brave · Opera · Vivaldi · Firefox       │
│ downloads.onCreated / onDeterminingFilename / cancel    │
│ + cookies + webRequest (observe-only, for HLS/DASH)     │
└──────────────────────┬──────────────────────────────────┘
                       │ native messaging (stdio, JSON, 4-byte LE length)
┌──────────────────────▼──────────────────────────────────┐
│ Native messaging host  (thin stdio ↔ IPC bridge)        │
│ MUST set O_BINARY. Debug → stderr only.                 │
└──────────────────────┬──────────────────────────────────┘
                       │ local IPC (named pipe)
┌──────────────────────▼──────────────────────────────────┐
│ Download engine — background service, single instance   │
│ • segment planner (Range + If-Range, dynamic splitting) │
│ • connection pool: HTTP/1.1 · HTTP/2 · HTTP/3           │
│ • sparse-file writer, offset writes, IOCP               │
│ • scheduler · queues · bandwidth limiter · retry        │
│ • categorizer (ext → MIME → magic bytes)                │
│ • per-host policy cache · SQLite state (crash-resume)   │
└──────────────────────┬──────────────────────────────────┘
                       │ IPC
┌──────────────────────▼──────────────────────────────────┐
│ Desktop UI — tray, queues, live speed graph, settings   │
└─────────────────────────────────────────────────────────┘
```

Key properties: engine survives UI close (tray), single-instance enforced, all state in SQLite so a crash or reboot resumes mid-download, UI is a thin client over the same IPC the extension uses.

### Stack options

**A. Rust engine + Tauri UI** — recommended
`reqwest`/`hyper` for HTTP/1.1+2, `quinn`/`h3` for HTTP/3, `tokio` for concurrency, `rusqlite`. Tauri: minimal app "less than 600KB", uses the OS WebView (no bundled Chromium), supports **sidecar binaries, system tray, and single-instance** as first-party features, and ships Windows installers. Fast, small, memory-safe, permissive license, HTTP/3 available today.
Cost: Rust learning curve.

**B. C#/.NET 8 + WPF/WinUI 3**
Fastest path if Ahmad already knows C#. `HttpClient` supports HTTP/3 on Windows 11. Excellent Windows integration. Cost: larger runtime, needs self-contained publish or framework dependency.

**C. Electron + own Node engine**
Fastest UI development, worst footprint (~150 MB+), and Node's HTTP/3 story is weak. Motrix already occupies this niche. Not recommended for a product whose whole pitch is speed.

---

## 10. Phased roadmap

**Phase 0 — decisions** (§11) · pick public name, stack, license model

**Phase 1 — engine** (the hard part; do it first)
Range detection · N-segment parallel download · sparse writer · pause/resume with `If-Range` · `200`-response corruption guard · retry/backoff · SQLite state · CLI harness for testing.
*Exit test:* download a 5 GB file, kill the process at 60%, resume, verify SHA-256 matches a single-stream download.

**Phase 2 — native host + extension**
Native messaging host with `O_BINARY` stdio · Chrome MV3 extension (capture, cancel, cookie handover, dedup on racy cancel) · handle `blob:`/POST skip cases · `setUiOptions` to hide Chrome's bubble.

**Phase 3 — UI**
Tray · queue list with live per-segment progress · speed graph · settings (categories, folders, connection limits, bandwidth caps) · context menus.

**Phase 4 — installer**
Inno Setup: elevation, `HKLM` native-host registration, per-browser extension registry keys, detect installed browsers, clean uninstall, Add/Remove Programs entry. Then Authenticode signing and CWS submission.

**Phase 5 — IDM feature parity**
Scheduler · queues · bandwidth quota · clipboard monitor · batch/"download all" · site grabber · proxy + NTLM/Kerberos/Basic auth · post-download AV scan · shutdown-on-complete · portable mode.

**Phase 6 — differentiators**
HTTP/3 · multi-source/Metalink · adaptive connections · HLS/DASH video capture (via observe-only `webRequest` on `.m3u8`/`.mpd`) · per-host learning.

---

## 11. Open decisions

1. **Public brand name** — "FDM" cannot ship as-is (§1). Codename stays FDM.
2. **Stack** — Rust+Tauri (recommended) vs C#/.NET vs Electron. Depends partly on what Ahmad wants to maintain long-term.
3. **License / business model** — free & open-source, freeware, or paid like IDM? This decides whether GPL components (aria2, ffmpeg GPL builds, yt-dlp binaries) are usable at all.
4. **Video downloading scope** — HLS/DASH capture is a top user draw but is the highest CWS-review and legal risk (see FDM's forced YouTube removal). In or out of v1?
5. **BitTorrent** — in scope or not? Large surface area; IDM itself doesn't have it.
6. **Code signing budget** — required, not optional, for this category (§3).

---

## Primary sources

- Native messaging — https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging
- chrome.downloads — https://developer.chrome.com/docs/extensions/reference/api/downloads
- Installing extensions from a desktop installer — https://developer.chrome.com/docs/extensions/how-to/distribute/install-extensions
- MV3 blocking webRequest migration — https://developer.chrome.com/docs/extensions/develop/migrate/blocking-web-requests
- CWS program policies — https://developer.chrome.com/docs/webstore/program-policies
- HTTP Range requests — https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/Range_requests
- SmartScreen — https://learn.microsoft.com/en-us/windows/security/operating-system-security/virus-and-threat-protection/microsoft-defender-smartscreen/
- Inno Setup — https://jrsoftware.org/isinfo.php
- IDM features — https://www.internetdownloadmanager.com/features.html
- Free Download Manager — https://en.wikipedia.org/wiki/Free_Download_Manager
- aria2 — https://github.com/aria2/aria2
- Motrix — https://github.com/agalwood/Motrix
- XDM — https://github.com/subhra74/xdm
- yt-dlp — https://github.com/yt-dlp/yt-dlp
- Tauri — https://v2.tauri.app/start/
