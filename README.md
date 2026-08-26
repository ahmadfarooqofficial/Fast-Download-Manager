<div align="center">

<img src="brand/logo/fdm-icon.svg" width="108" height="108" alt="FDM — Fast Download Manager">

# FDM — Fast Download Manager

**High-speed parallel download acceleration & media streaming manager for Windows.**  
*Free, open source, privacy-first, zero telemetry.*

[![GitHub Release](https://img.shields.io/github/v/release/ahmadfarooq/fdm?style=flat-square&color=e50914)](https://github.com/ahmadfarooq/fdm/releases)
[![Build Status](https://img.shields.io/badge/build-passing-brightgreen?style=flat-square)](https://github.com/ahmadfarooq/fdm)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows%2010%20%7C%2011-0078d7?style=flat-square&logo=windows)](https://github.com/ahmadfarooq/fdm)
[![Rust](https://img.shields.io/badge/Engine-Rust%201.80+-orange?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![Tauri](https://img.shields.io/badge/GUI-Tauri%202.0-24C8D8?style=flat-square&logo=tauri)](https://tauri.app/)

[Quick Install](#-quick-install) • [Key Features](#-key-features) • [Architecture](#-architecture) • [How IDM Method Works](#-the-idm-direct-stream-method) • [Build from Source](#-build-from-source) • [CLI Commands](#-cli-usage) • [License](#-license)

</div>

---

> [!NOTE]
> **FDM — Fast Download Manager** is an independent, modern, open-source download manager built from the ground up in Rust and Tauri. To prevent confusion with legacy products, it is always identified as **FDM — Fast Download Manager**.

---

## ⚡ Highlights

* **🚀 32-64 Multi-Connection Acceleration**: Splits files into dynamic parallel streams, saturating your bandwidth and eliminating server-side per-connection rate limits.
* **🎥 Real-Time Video Quality Sniffer**: In-browser media overlay on YouTube and web video players with exact resolution detection (4K, 1440p, 1080p, 720p, 480p, 360p, and MP3 audio).
* **⚡ IDM Direct Stream Handover**: Intercepts active CDN streams (`googlevideo.com/videoplayback`) directly from the browser for instant **< 100ms** server connections.
* **📊 60/120 FPS Physics Interpolation (Lerp)**: Real-time, smooth progress bar movement and continuous speedometer/odometer transfer rate rendering without stuttering.
* **💾 Direct Sparse File Positional Writes**: Writes segments directly into allocated sparse files on disk with zero post-download merge delay or disk thrashing.
* **🔒 Crash-Proof Resumes**: Transactional control files (`.fdm`) record bytes strictly after durable disk flush. Resumes instantly from exact byte offsets.
* **🌐 Browser Extension & Policy Auto-Deployment**: Native extension bridge supporting Google Chrome, Microsoft Edge, Brave, Vivaldi, and Chromium.

---

## 📥 Quick Install

### Windows One-Click Installer (Recommended)

1. Download **`FDM-Setup-0.1.5.exe`** from [Latest Releases](https://github.com/ahmadfarooq/fdm/releases).
2. Run the installer (Click **Yes** when Windows UAC prompts for admin).
3. Open your browser (Chrome / Edge / Brave). The extension is automatically registered and ready.

| Component | Automated Installation Details |
|---|---|
| **Program Files** | Installed to `C:\Program Files\FDM` |
| **Download Hub** | Organized under `Downloads\FDM\{Video, Music, Compressed, Programs, Documents, Other}` |
| **Native Bridge** | Registers `com.fdm.native_host` in Windows Registry for Chrome, Edge, and Brave |
| **Media Tools** | Bundles high-speed extraction engines (`yt-dlp.exe`, `deno.exe`, `ffmpeg.exe`) |
| **System Runtime** | Detects Microsoft WebView2 runtime; auto-bootstraps silently if missing |

---

## 🏗️ Architecture

FDM is organized into modular Rust crates and frontend components:

```
FDM/
├── crates/
│   ├── fdm-core/          ◄── High-performance multi-connection download engine
│   │   ├── probe.rs       ◄── HTTP 206 range validation & remote metadata discovery
│   │   ├── plan.rs        ◄── Dynamic segment splitting & tail-stealing coordinator
│   │   ├── download.rs    ◄── Asynchronous connection pool & retry supervisor
│   │   ├── writer.rs      ◄── Preallocated sparse file writer (positional seek_write)
│   │   ├── state.rs       ◄── Atomic transactional .fdm resume checkpointing
│   │   └── categorize.rs  ◄── Extension, MIME type & magic-byte classification
│   ├── fdm-manager/       ◄── Central download state manager, task queue & media engines
│   ├── fdm-ipc/           ◄── Local named pipe IPC protocol with DACL security
│   ├── fdm-host/          ◄── Native messaging host bridge for browser extensions
│   └── fdm-cli/           ◄── CLI test harness & terminal download utility
│
├── apps/desktop/          ◄── Tauri 2.0 desktop GUI application (HTML/CSS/JS)
│   ├── src-tauri/         ◄── Rust application backend & event dispatch
│   ├── download_dialog.js ◄── 60 FPS sub-frame physics interpolation download dialog
│   └── tokens.css         ◄── Tailored dark-mode design system tokens
│
├── extension/             ◄── Manifest V3 browser integration extension
│   ├── background.js      ◄── Stream sniffer, cookie/header handover & host bridge
│   ├── content.js         ◄── YouTube & HTML5 IDM-style player overlay bar
│   └── content.css        ◄── Media pill styling & resolution selector
│
└── installer/             ◄── Inno Setup Windows installer script (fdm.iss)
```

---

## 🔍 The IDM Direct Stream Method

Traditional download managers launch heavy external scrapers when you click a video, causing **15–20 seconds** of connection lag. 

FDM uses the **Direct In-Browser Stream Handover**:

```
 ┌────────────────────────────────────────────────────────────┐
 │  Chrome / Edge / Brave Browser                             │
 │  Active YouTube HTML5 Player                               │
 └──────────────────────────────┬─────────────────────────────┘
                                │ 1. Intercepts googlevideo.com/videoplayback
                                │ 2. Pre-warms resolutions on page load (0ms menu)
                                ▼
 ┌────────────────────────────────────────────────────────────┐
 │  FDM Browser Extension (MV3)                               │
 │  Extracts direct CDN stream URL & user session headers     │
 └──────────────────────────────┬─────────────────────────────┘
                                │ 3. Native Messaging Bridge (com.fdm.native_host)
                                ▼
 ┌────────────────────────────────────────────────────────────┐
 │  fdm-core Multi-Connection Engine (Rust)                   │
 │  Connects directly to Google CDN with 32 parallel streams  │
 └────────────────────────────────────────────────────────────┘
          ⚡ Instant Connection (< 100ms) • Maximum Line Speed
```

---

## 🛠️ Build from Source

### Prerequisites
* Windows 10 (Build 17763+) or Windows 11
* [Rust](https://www.rust-lang.org/) (1.80+)
* [Inno Setup 6](https://jrsoftware.org/isinfo.php) (for building the installer)
* MSVC C++ Build Tools (via Visual Studio Installer)

### Steps

```powershell
# 1. Clone the repository
git clone https://github.com/ahmadfarooq/fdm.git
cd fdm

# 2. Run all unit and integration tests (86 tests)
cargo test --workspace

# 3. Build release binaries
cargo build --release --workspace

# 4. Build the complete standalone Windows setup installer
powershell -ExecutionPolicy Bypass -File .\scripts\build-installer.ps1 -AllowPartial -SkipBuild
```

The output installer will be generated at:  
`installer\output\FDM-Setup-<version>.exe`

---

## 💻 CLI Usage

FDM includes a high-speed CLI tool (`fdm.exe`) for terminal usage, scripting, and server automation:

```powershell
# Download with 32 parallel connections
fdm get "https://example.com/largefile.iso" -n 32

# Download to a specific directory without subfolder categorization
fdm get "https://example.com/archive.zip" --out "D:\Downloads" --flat

# Download with custom HTTP headers
fdm get "https://example.com/data.tar.gz" -H "Cookie: session=123" -H "Referer: https://example.com"

# Check SHA-256 integrity hash of a local file
fdm hash "D:\Downloads\largefile.iso"
```

### Command Flags

| Flag | Description | Default |
|---|---|---|
| `-n, --connections <N>` | Maximum parallel connections (1–64) | `16` |
| `--sequential` | Force single-connection mode (comparison baseline) | `false` |
| `-o, --out <DIR>` | Target download directory | `%USERPROFILE%\Downloads\FDM` |
| `--flat` | Save directly in output folder without category subfolders | `false` |
| `--name <NAME>` | Explicit custom output filename | Auto-derived |
| `--sha256` | Calculate and print SHA-256 hash upon completion | `false` |
| `-H, --header <K: V>` | Custom HTTP request header (repeatable) | `None` |

---

## 📜 License

This project is licensed under the **MIT License** — see the [LICENSE](LICENSE) file for details.

---

<div align="center">
  <sub>Built with high-precision engineering by <b>Ahmad Farooq</b>.</sub>
</div>
