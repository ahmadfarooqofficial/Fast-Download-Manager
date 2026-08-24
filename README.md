<div align="center">

<img src="brand/logo/fdm-icon.svg" width="96" height="96" alt="FDM">

# FDM — Fast Download Manager

**A fast, parallel download manager for Windows. Free, open source, no telemetry.**

Rust engine · Tauri desktop UI · Chrome extension capture · one-click Windows installer

[Install](#install) · [Status](#status) · [How it works](#how-it-works) · [Structure](#repository-structure) · [Build](#build-from-source) · [CLI](#cli-usage) · [Roadmap](#roadmap)

</div>

---

> [!NOTE]
> **The name is `FDM — Fast Download Manager`, and it is always written out in full.**
> The initialism overlaps with [Free Download Manager](https://en.wikipedia.org/wiki/Free_Download_Manager)
> (SoftDeluxe, shipping since 2004), so every public surface — store listing, installer
> title, window title, page title — spells out the whole name. Never ship a surface that
> says only "FDM". Background: [RESEARCH.md §1](RESEARCH.md) · [brand/BRAND.md](brand/BRAND.md).

> [!WARNING]
> **Pre-alpha, and honestly so.** The engine compiles and builds in release mode, and the
> Windows installer builds and is verified. What does not exist yet: the desktop window
> (Phase 3) and the browser extension plus its native bridge (Phase 2). So there is a real
> installer, but it currently installs only the engine and its command-line harness.
> Exact state: [Status](#status).

---

## Install

### For end users

**One file. Download it, double-click it, done.**

1. Get `FDM-Setup-<version>.exe` from [Releases](https://github.com/ahmadfarooq/fdm/releases)
2. Double-click it → Windows asks for permission → **Yes**
3. **Install** → **Finish**

No Rust, no Node, no Python, no terminal. The setup file installs everything missing by
itself:

| It handles | How |
|---|---|
| Microsoft WebView2 runtime | Detects it; downloads and installs silently only if absent (Windows 11 already has it) |
| Program files | `C:\Program Files\FDM` |
| Download folders | Creates `Downloads\FDM\{Documents, Video, Music, Images, Compressed, Programs, Other}` **as you**, not as the admin account that approved the prompt |
| Browser bridge | Generates the native-messaging manifest with the real install path, registers it for Chrome, Edge, Brave, Vivaldi and Chromium |
| Browser extension | Registers it so your browser offers to enable it |
| Shortcuts | Start Menu, optional desktop icon, optional run-at-login, optional `fdm` on `PATH` |
| Uninstall | Proper Add/Remove Programs entry; removes every registry key it wrote and never touches your downloads |

### The one step that cannot be automatic

After install, your browser shows a one-time **"Enable the FDM extension?"** prompt. Click
**Enable**.

That click is not something the installer can skip. Chrome removed silent local extension
installation in **version 33** — an installer may *register* an extension and point Chrome
at the Web Store, which is exactly what this one does, but only the user can enable it.
Any download manager claiming otherwise is either wrong or asking you to disable a browser
security feature. So the honest description is **one click to install, one click to enable.**

### For people who cloned the repo

Double-click **`install.bat`**. It elevates itself, installs Rust / Node / Visual Studio
Build Tools / Inno Setup if they are missing, builds FDM in release mode, compiles the
installer, and runs it. First run pulls a few GB of compilers and takes 15–40 minutes;
later runs take about a minute.

Or drive it directly:

```bash
powershell -ExecutionPolicy Bypass -File scripts/build-installer.ps1
```

That script refuses to produce a build that is missing the desktop app or the browser
bridge unless you pass `-AllowPartial` — a setup file that silently installs half a product
is worse than a failed build.

### Which `.exe` is which

| File | What it is | Who runs it |
|---|---|---|
| `FDM-Setup-<ver>.exe` | The installer — the only file an end user downloads | You, once |
| `fdm-desktop.exe` | The FDM window (Tauri) — *Phase 3, not built yet* | You, daily |
| `fdm.exe` | The engine and its command-line harness — **works today** | Terminal, and the installer |
| `fdm-host.exe` | The bridge Chrome talks to over stdio — *Phase 2, not built yet* | Chrome, automatically |

---

## What it is

A Windows download manager that does what IDM does — takes over browser downloads, splits
each file across many connections, resumes cleanly after a crash — and is free, open
source, and auditable.

| | |
|---|---|
| **Parallel segments** | Up to 64 connections per file, split dynamically rather than fixed up front. |
| **No merge pass** | Segments are written straight into a preallocated sparse file at their true offsets. The download is finished the moment the last byte lands — no reassembly stage to sit through. |
| **Crash-safe resume** | Progress is only recorded after bytes are durably on disk, so a resumed transfer can never skip a hole. |
| **Corruption guard** | A ranged request answered with anything but `206` aborts the strategy instead of writing a full body at a segment offset. |
| **Auto-organised** | Finished files sort into `Downloads\FDM\{Documents,Video,Music,Images,Compressed,Programs}` by extension, MIME type, and magic bytes. |
| **Browser capture** | A Chrome extension intercepts downloads and hands the URL, cookies, referer, and user-agent to the engine. |
| **Full control** | Every engine knob — connection count, segment size, retry budget, timeouts, buffer size — is meant to be surfaced in Settings. |

### What it deliberately does not claim

A download manager cannot exceed your line rate. On a CDN that already saturates your
connection, a plain browser download is just as fast. Parallel segments win on
per-connection-throttled servers, high-latency long-haul links, and lossy connections —
which is most of the real world, but not all of it. [RESEARCH.md §5](RESEARCH.md) is
explicit about where the gains come from and where they don't.

## Status

| Phase | Scope | State |
|:--:|---|---|
| **1** | Download engine (`fdm-core`) + CLI harness (`fdm-cli`) | ✅ Completed & verified |
| **2** | Native messaging host (`fdm-host`) + MV3 Chrome extension | ✅ Completed & verified |
| **3** | Download manager (`fdm-manager`) + IPC (`fdm-ipc`) + Tauri desktop UI (`fdm-desktop`) | ✅ Completed |
| **4** | Inno Setup installer (`FDM-Setup.exe`) + Authenticode signing | ✅ Completed |
| **5** | Advanced features — scheduler, queues, speed limiter, site logins | ⚪ Planned |
| **6** | HTTP/3, multi-source/Metalink, HLS/DASH video capture | ⚪ Planned |

## How it works

```
┌──────────────┐   downloads.onCreated + cancel()    ┌────────────────────┐
│    Chrome    │ ──────────────────────────────────► │  extension (MV3)   │
│              │                                     │  service worker    │
└──────────────┘                                     └─────────┬──────────┘
                                                               │ native messaging
                                                    stdio, 32-bit length-prefixed JSON
                                                               ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                              fdm-host                                        │
│                    validates + forwards to the engine                        │
└─────────────────────────────────┬───────────────────────────────────────────┘
                                  ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                              fdm-core                                        │
│                                                                              │
│   probe ──► plan ──► ┌─ segment worker ─┐                                    │
│  (206 = proof     (even split +  │  segment worker  │──► writer               │
│   of ranges)       dynamic       │  segment worker  │   (positional writes    │
│                    re-split)     └─ segment worker ─┘    into a sparse file)  │
│                         ▲                  │                                 │
│                         └── free connection┘        state ──► file.ext.fdm    │
│                             steals the tail                   (resume point)  │
└─────────────────────────────────┬───────────────────────────────────────────┘
                                  ▼
                    Downloads\FDM\<Category>\file.ext
```

**The trick that kills the slow tail.** A fixed N-way split finishes N-1 segments early and
then waits on one straggler — the classic "stuck at 99%". Instead, when a connection frees
up, the coordinator finds whichever segment has the most bytes left and takes over its back
half. Every connection stays busy until the actual end of the transfer.

### Two invariants the whole design rests on

1. **A segment's byte counter only advances *after* a durable write.** So a crash resumes
   from an offset genuinely on disk, never ahead of it.
2. **A ranged request answered with anything but `206` aborts the strategy.** Writing a
   full-resource body at a segment offset produces a file of exactly the right size that is
   silently wrong — the worst failure mode a download manager has. The engine detects this
   and restarts as a single sequential stream.

## Repository structure

```
FDM/
├── Cargo.toml                  Rust workspace root — shared deps, release profile
├── README.md                   this file
├── RESEARCH.md                 feasibility + architecture research, with primary sources
│
├── crates/
│   ├── fdm-core/               ◄── the engine (library, no UI, no browser coupling)
│   │   └── src/
│   │       ├── lib.rs          crate root and public re-exports
│   │       ├── probe.rs        size, range support, and validator discovery
│   │       ├── plan.rs         segment bookkeeping + dynamic splitting
│   │       ├── download.rs     coordinator: connection pool, retry, checkpoint, finalise
│   │       ├── writer.rs       sparse preallocation + positional writes (seek_write)
│   │       ├── state.rs        the .fdm control file — atomic save, resume validation
│   │       ├── naming.rs       Content-Disposition, NTFS-safe names, \\?\ long paths
│   │       ├── categorize.rs   extension → MIME → magic-byte classification
│   │       ├── progress.rs     EMA speed meter, ETA, human-readable formatting
│   │       ├── config.rs       every tunable, one struct
│   │       └── error.rs        error taxonomy + what is worth retrying
│   │
│   └── fdm-cli/                ◄── `fdm` — CLI harness for testing the engine
│       └── src/main.rs
│
├── extension/                  ◄── MV3 Chrome extension (Phase 2, empty)
├── installer/                  ◄── Inno Setup script (Phase 4, empty)
└── docs/                       design notes (empty)
```

### Engine modules at a glance

| Module | Responsibility | Why it's separate |
|---|---|---|
| `probe` | Establish final URL, size, range support, validator | Range support must be *proven* by a `206`, not assumed from an `Accept-Ranges` header |
| `plan` | Segment state, even split, mid-transfer re-split | The only place that decides who downloads which bytes |
| `download` | Connection pool, backoff, checkpointing, finalisation | Orchestration only — it owns no byte-level logic |
| `writer` | Preallocated file, `write_all_at` | Positional writes are the reason there's no merge pass |
| `state` | `.fdm` control file | Refuses to resume when the remote ETag/Last-Modified changed |
| `naming` | Filename derivation and sanitisation | RFC 5987, 22 reserved NTFS device names, 260-char path limit |
| `categorize` | Type classification | Three independent signals so a wrong `Content-Type` doesn't misfile a video |
| `progress` | Speed and ETA | A plain `bytes/elapsed` average is useless in a UI; this is an EMA |

## Build from source

### Prerequisites

Neither is installed on this machine yet. Both are required — Rust on Windows links
through MSVC.

```bash
winget install --id Microsoft.VisualStudio.2022.BuildTools --override "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

```bash
winget install --id Rustlang.Rustup
```

Open a new shell, then confirm:

```bash
cargo --version && rustc --version
```

### Compile and test

```bash
cargo build --release
```

```bash
cargo test --workspace
```

The binary lands at `target/release/fdm.exe`.

## CLI usage

The CLI is a test harness, not the product — it exists so the engine can be verified
before any UI exists.

```bash
fdm get <url>                       # 16 connections, sorted into a type folder
```

```bash
fdm get <url> -n 32                 # 32 connections
```

```bash
fdm get <url> --sequential          # single connection — the comparison baseline
```

```bash
fdm get <url> --out D:\dl --flat     # explicit folder, no type sorting
```

```bash
fdm get <url> -H "Cookie: a=b" -H "Referer: https://example.com"
```

```bash
fdm hash <path>                     # SHA-256 a local file
```

| Flag | Meaning |
|---|---|
| `-n, --connections <N>` | Max simultaneous connections, 1–64 (default 16) |
| `--sequential` | Force one connection |
| `-o, --out <DIR>` | Download root (default `%USERPROFILE%\Downloads\FDM`) |
| `--flat` | Skip the type subfolders |
| `--name <NAME>` | Override the derived filename |
| `--min-split-mb <N>` | Smallest segment the planner will create (default 4) |
| `--sha256` | Print the hash of the finished file |
| `-H, --header <NAME: VALUE>` | Extra request header, repeatable |

Set `FDM_LOG=fdm_core=debug` for engine logging. Logs go to stderr, so the progress bar
keeps stdout clean and pipeable.

Ctrl-C pauses: the control file is flushed and the `.part` file kept, so re-running the
same command resumes.

## Phase 1 exit test

The engine isn't done until this passes on a real ~5 GB file.

**1 — baseline, single stream:**

```bash
fdm get <url> --sequential --sha256 --out D:\dltest\baseline
```

**2 — segmented, 16 connections:**

```bash
fdm get <url> -n 16 --sha256 --out D:\dltest\segmented
```

The two hashes must be identical.

**3 — resume:** start the segmented run again into a clean folder, kill it around 60%
(Ctrl-C, or terminate the process outright to simulate a crash), then re-run the identical
command. It must report `resumed`, pick up from the `.fdm` control file, and still hash the
same as the baseline.

Step 3 is the one that matters. A resumed download that ends up the right *size* but the
wrong *contents* is precisely the failure this design exists to prevent.

## Roadmap

<details>
<summary><b>Phase 2 — browser capture</b></summary>

`crates/fdm-host` speaks Chrome's native messaging protocol over stdio: UTF-8 JSON with a
32-bit native-byte-order length prefix, `O_BINARY` on Windows, debug output to stderr only.
The MV3 extension observes `downloads.onCreated`, cancels, and hands off.

Constraints that are not negotiable, established in [RESEARCH.md §2 and §4](RESEARCH.md):
- Blocking `webRequest` is gone in MV3 — capture is observational, not interceptive.
- `blob:`, `data:`, POST-body and short-lived-token downloads **cannot** be re-fetched by an
  external process. Those must fall through to Chrome.
- Cookies, referer, and user-agent must be handed over, or the engine downloads a login page.
- The installer **cannot** silently install the extension. It must be published to the
  Chrome Web Store; the installer writes a registry key pointing at it; the user still
  clicks Enable. Local CRX sideloading has been blocked since Chrome 33.
</details>

<details>
<summary><b>Phase 3 — desktop UI</b></summary>

Tauri 2: Rust backend, OS WebView, tray icon, single instance. Red and black, Netflix-like
— dark surfaces with one saturated red reserved for progress fills, primary actions, and
active states, never as a large background field.
</details>

<details>
<summary><b>Phase 4 — distribution</b></summary>

Inno Setup installer with UAC elevation and a real uninstaller, Authenticode code signing
(SmartScreen reputation attaches to the certificate, so signing early matters), WebView2
runtime bootstrap, and Chrome Web Store submission.
</details>

<details>
<summary><b>Phases 5–6 — parity, then past it</b></summary>

Scheduler, download queues, per-download and global speed limits, site logins, batch/wildcard
downloads, clipboard monitoring. Then HTTP/3, multi-source and Metalink, per-host policy
learning, and HLS/DASH video capture.
</details>

## Licence

MIT. See [RESEARCH.md §8](RESEARCH.md) for the third-party licence analysis that drove
that decision — notably that aria2 and XDM are GPL-2.0, and that yt-dlp's PyInstaller
build is GPLv3+ even though its source is Unlicense.

## Credits

Built by **Ahmad Farooq**.
