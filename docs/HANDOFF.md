# FDM — Handoff

**Read this file first.** It is the running state of the build: what is done, what
is next, and the decisions you must not silently undo. It is rewritten at the end
of every unit of work, so treat it as more current than any summary you were
given.

- **Last updated:** 2026-08-24
- **Repo:** `D:\Code\FDM` (all work belongs here — never in a scratch directory)
- **Branch:** `master`
- **Author of record:** Ahmad Farooq (this name must appear as the signing
  subject; UAC reads the certificate subject, never `VersionInfoCompany`)

---

## 1. What FDM is

A free, open-source Windows download manager aiming at IDM feature parity:
segmented parallel downloading, a browser extension that takes over downloads,
one-click installer, full settings. Rust engine + Tauri desktop UI.

Non-negotiable product requirements, all from the user:

| Requirement | Where it is satisfied |
| --- | --- |
| Fast, parallel, segmented downloads | `crates/fdm-core` (`plan.rs` dynamic splitting) |
| Chrome extension takes over downloads like IDM | `extension/` + `crates/fdm-host` |
| Extension installed automatically by the setup file | `installer/fdm.iss` `[Registry]` pre-install keys |
| On enable, an "all set up" HTML page opens | `extension/welcome.html` via `chrome.runtime.onInstalled` |
| Downloads go to `Downloads\FDM\`, auto-sorted by type | `crates/fdm-core/categorize.rs` + `config.rs` |
| Partial data in a temp folder, user-changeable, like IDM | `crates/fdm-core/scratch.rs` + `config.rs` (`temp_dir`, `use_temp_dir`); UI surface still owed — see gap list |
| A download list with pause/resume/queue that survives a restart | `crates/fdm-manager` |
| One real Windows installer that installs everything missing | `scripts/build-installer.ps1` + `installer/fdm.iss` |
| Publisher shows "Ahmad Farooq", not "Unknown" | `scripts/sign.ps1` |
| Minimal, clean, SaaS-like UI; red + black like Netflix | `brand/tokens/fdm-tokens.css` |
| Free and open source | MIT |

---

## 2. Current status by phase

| Phase | Scope | State |
| --- | --- | --- |
| 1 | Rust download engine | ✅ compiles, 55 tests green, **verified against a real 100 MB download** |
| 2 | Chrome extension + native host | ✅ written; host 20 unit tests + **5 end-to-end checks green**; not yet loaded in a real Chrome |
| 3a | Download list (`fdm-manager`) | ✅ **23 tests green** (6 unit + 17 integration against a local throttled server) |
| 3b | IPC (`fdm-ipc`, named pipe) | ⚪ not started |
| 3c | Tauri desktop UI (`fdm-desktop.exe`) | ⚪ not started |
| 4 | Installer + code signing | ✅ builds and signs; `-AllowPartial` still needed |
| 5 | IDM parity (scheduler, queues, speed limit, clipboard) | ⚪ not started |
| 6 | HTTP/3, Metalink, HLS/DASH | ⚪ not started |

`cargo test --workspace` — **100 passing, 0 failing** (fdm-core 55, fdm-host 20,
fdm-manager lib 6, fdm-manager `tests/list.rs` 17, 2 doc-tests, fdm-cli 0).

`cargo clippy --workspace --all-targets` — **zero warnings.**

`cargo build --release --workspace` — clean; produces `target\release\fdm.exe`
and `target\release\fdm-host.exe`.

### The download list — `crates/fdm-manager`

What sits between the engine (one download at a time, no memory) and the UI: the
ordered list, the `max_active` queue, pause/resume/cancel/remove, a broadcast
event stream, and `downloads.json` persistence. `Manager` is the only public
surface; `Engine` is not exposed past it.

Its 17 integration tests run against `tests/common/mod.rs` — a deliberately
throttled local HTTP server (512 KiB in 8 KiB chunks 150 ms apart ≈ 2.4 s, first
bytes immediate) rather than anything on the internet. Everything interesting
here is about *timing*: a pause has to land mid-transfer, a queue has to be
caught holding a download back, a crash has to happen mid-write. A real server
gives no control over when a chunk arrives, and a fast one finishes before the
test can act — the first run of these tests failed three ways for exactly that
reason.

Two harness pieces are load-bearing and easy to break:

- **`app_session` runs each simulated app session on its own runtime and then
  calls `rt.shutdown_timeout(Duration::ZERO)`.** Dropping a `Manager` is *not* a
  crash: its `tokio::spawn`ed download tasks outlive it and keep writing to the
  store the next session is about to read. Killing the whole runtime is what
  quitting — or crashing — actually looks like.
- **It uses `spawn_blocking(...).await`, never `std::thread::spawn(...).join()`.**
  These are `#[tokio::test]`s on a current-thread runtime that is also hosting
  the test server. Blocking that thread stops the accept loop, and the inner
  session then waits forever for a connection nothing is left to accept.

The tests earned their keep immediately: `pause_all` → `resume_all` exposed a
real concurrency bug in `manager.rs`, now fixed and pinned by
`resuming_the_instant_pause_returns_does_not_lose_the_download`. See decisions
12 and 13.

### Where partial data goes

Default `temp_dir` is **`%LOCALAPPDATA%\FDM\Temp`**, not the install directory.
`.part` and `.fdm` live there while a download runs; the finished file is moved
into `Downloads\FDM\<Category>\`. Set `use_temp_dir: false` to keep scratch files
beside the destination instead (the pre-`scratch.rs` behaviour, still tested).

This is a considered deviation from "temp folder where FDM is installed": a
standard user cannot write inside `C:\Program Files`, so a temp dir there would
fail for every non-elevated download. IDM does the same thing — its default is
under `AppData`. The path is a plain config field, so the Settings UI can point
it anywhere, including a folder on C: chosen by the user.


### Measured download performance (2026-08-23)

Test file: `https://proof.ovh.net/files/100Mb.dat` (100 MiB, `Accept-Ranges: bytes`).
This server throttles a single TCP stream hard, which is exactly the condition
segmented downloading exists to beat.

| Run | Time | Throughput | SHA-256 |
| --- | --- | --- | --- |
| `curl` (1 connection, reference) | 77.4 s | 1.29 MiB/s | — |
| `fdm get --sequential` | 1 m 11 s | 1.39 MiB/s | `3e432abb…4608bc` |
| `fdm get -n 16` | **24 s** | **4.16 MiB/s** | `3e432abb…4608bc` |
| `fdm get -n 16`, killed at 67.2% then resumed | 15 s for the rest | 6.62 MiB/s | `3e432abb…4608bc` |

**2.96× faster than one connection, and all three hashes are identical** — the
segmented and resumed files are byte-for-byte the same as the sequential one.
The kill was `taskkill /F` mid-flight; it left `100Mb.dat.part` plus a 1955-byte
`100Mb.dat.part.fdm` holding per-segment `done` offsets and the ETag validator,
and the resumed run logged `resuming done=70447661 segments=16` and cleaned both
scratch files up on success.

### Extension files, and what each one is for

| File | Role |
| --- | --- |
| `manifest.json` | MV3, classic (non-module) service worker, `minimum_chrome_version: 116` |
| `background.js` | the takeover: `onDeterminingFilename` → `cancel` → `erase` → `connectNative` |
| `lib/format.js` | byte/speed/duration formatting, mirrors `fdm-core/progress.rs` exactly |
| `welcome.html/.css/.js` | the "all set up" page opened on enable; state comes from a live ping |
| `popup.html/.css/.js` | toolbar popup: capture switch, live rows, per-row cancel |
| `styles/tokens.css` | verbatim copy of `brand/tokens/fdm-tokens.css` (see gap list) |

All four scripts pass `node --check`. Both HTML surfaces were measured in a
headless Chromium (`python -m http.server 5273 --directory extension`, see
`.claude/launch.json`) at 340 px, 375 px and 1100 px: no horizontal overflow, the
status card stacks below 30 rem, the popup's `main` is the only scroll region, and
`.fdm-progress` fills resolve correctly for running / completed / failed / paused.

### End-to-end host test — `scripts/test-host.py`

```bash
python scripts/test-host.py
```

Impersonates Chrome over stdio (native-order length prefix + UTF-8 JSON) and
covers the one link no unit test reaches. All five checks pass:

| # | Check | Result |
| --- | --- | --- |
| A | `ping` → `pong` carrying every field `welcome.js` renders | ✅ |
| B | `protocol: 999` → `error{versionMismatch: true}`, not ignored | ✅ |
| C | explicit `targetDir` honoured exactly; 11 progress messages flowed | ✅ |
| D | **no** `targetDir` (what the extension sends) → `Downloads\FDM\Compressed\`, filename honoured | ✅ |
| E | stdin EOF mid-download does not kill the host; it finished 100 MB, then exited 0 | ✅ |

Check D is the guard on the "auto-arranged by data type" requirement, and check E
is the guard on decision 7 below. Both were initially reported as failures by an
earlier version of the harness that asserted the wrong thing — see decision 11.

---

## 3. Layout

```
D:\Code\FDM
├─ Cargo.toml                  workspace: fdm-core, fdm-cli, fdm-host, fdm-manager
├─ README.md                   public-facing; Status table needs a refresh
├─ RESEARCH.md                 how IDM works + what we copy
├─ docs\HANDOFF.md             this file
├─ brand\
│  ├─ BRAND.md                 source of truth for colour/type; tokens.css follows it
│  ├─ logo\*.svg               fdm-icon, mark, horizontal, stacked
│  ├─ icons\*.png .ico         rasterised by scripts\rasterize-logo.mjs
│  └─ tokens\fdm-tokens.css    THE design tokens. Dark-only by design.
├─ crates\
│  ├─ fdm-core\src\            engine
│  │   config.rs   EngineConfig, default_download_root, temp_dir
│  │   probe.rs    HEAD/Range support detection
│  │   plan.rs     segments + dynamic splitting (the speed)
│  │   writer.rs   positioned writes into one .part file
│  │   state.rs    .fdm control file -> resume
│  │   scratch.rs  where .part/.fdm live; deterministic hashed names
│  │   naming.rs   Content-Disposition, RFC 5987, NTFS reserved names
│  │   categorize.rs  extension -> Video/ Music/ Documents/ ...
│  │   download.rs coordinator: workers, retries, progress
│  │   progress.rs speed + ETA
│  │   error.rs    Error::is_retryable
│  ├─ fdm-manager\            the download list — what the UI and IPC talk to
│  │  ├─ src\
│  │  │   manager.rs  registry, queue, pause/resume/cancel, events. READ ITS
│  │  │                MODULE DOC ("Four rules") BEFORE EDITING.
│  │  │   model.rs    DownloadEntry, Status, ListEvent — the wire/UI shapes
│  │  │   store.rs    downloads.json; atomic replace, writes on transitions only
│  │  │   error.rs    ManagerError
│  │  └─ tests\
│  │      list.rs       17 integration tests; app_session = a simulated crash
│  │      common\mod.rs the throttled test server (timing is the whole point)
│  ├─ fdm-cli\src\main.rs      fdm.exe — test harness, not the product
│  └─ fdm-host\src\            fdm-host.exe — Chrome native messaging bridge
│      framing.rs  32-bit NATIVE-order length prefix (not network order)
│      protocol.rs the JSON messages; PROTOCOL_VERSION = 1
│      lock.rs     one exclusive claim per URL (share_mode 0)
│      main.rs     dispatch loop, engine in-process, header denylist
├─ extension\                  MV3 extension (icons\ already rasterised)
│  ├─ manifest.json            MV3; classic service worker; minimum_chrome_version 116
│  ├─ background.js            THE TAKEOVER. Read its module doc before editing.
│  ├─ lib\format.js            mirrors fdm-core\progress.rs byte-for-byte
│  ├─ welcome.html .css .js    the "all set up" page; every claim comes from a live ping
│  ├─ popup.html .css .js      toolbar popup; holds no state of its own
│  └─ styles\tokens.css        COPY of brand\tokens\fdm-tokens.css — can drift
├─ installer\fdm.iss           Inno Setup script
└─ scripts\
   ├─ build-installer.ps1      one command: build, sign, stage, compile installer
   ├─ sign.ps1                 Authenticode; Invoke-Native wrapper is load-bearing
   ├─ test-host.py             end-to-end: impersonates Chrome over stdio
   └─ rasterize-logo.mjs       SVG -> PNG/ICO, also writes extension\icons\
```

---

## 4. Decisions you must not silently undo

Each of these was reached by hitting the failure it prevents.

1. **`scripts/sign.ps1` must call native tools through `Invoke-Native`.**
   With `$ErrorActionPreference = 'Stop'`, *any* stderr line from a native exe
   becomes a terminating error. signtool writes its untrusted-root warning to
   stderr, which is the expected result for a self-signed dev cert. Calling
   signtool directly makes the build fail on a warning it is designed to accept.

2. **`build-installer.ps1` passes the signing command to ISCC using Inno's `$q`,
   not a literal quote.** A literal `"` gets mangled by PowerShell's
   native-argument escaping and ISCC then reads the tail as a second script
   filename ("You may not specify more than one script filename").

3. **`Accept-Encoding` is engine-controlled and on the host's header denylist.**
   A gzipped body makes byte ranges refer to *compressed* offsets, so every
   segment boundary lands in the wrong place. This is the single most damaging
   header the browser could forward.

4. **The native-messaging length prefix is native byte order, and counts bytes
   not characters.** `framing.rs` has a test for each; both mistakes produce a
   host that appears to hang.

5. **`plan.rs::split_largest` splits the *remaining* work (`mid = cursor +
   remaining/2`), not the whole segment.** Halving the whole segment would hand
   the new connection bytes already on disk and could put `end` behind the
   cursor. Two tests previously asserted the wrong behaviour; they were rewritten,
   not the implementation.

6. **`lock.rs::is_contention` keys on `raw_os_error()`, not `ErrorKind`.**
   Windows reports a share-mode violation as `ERROR_SHARING_VIOLATION` (32),
   which Rust maps to the unstable, unmatchable `ErrorKind::Uncategorized` — and
   emphatically *not* `PermissionDenied`. Matching on `ErrorKind` here reports a
   failed download when it was merely already running.

7. **The host does not exit when stdin reaches EOF.** Chrome kills the port when
   its service worker is evicted, which is routine. The host finishes downloads
   it already owns, then exits. If it is killed anyway, the `.fdm` control file
   makes the download resumable: the worst case is a resumed download, never a
   corrupt one.

8. **stdout belongs to the protocol.** All logging goes to stderr
   (`FDM_LOG=debug` to enable). One stray `println!` corrupts the stream and the
   extension sees a parse error, not a message.

9. **Cookies never travel on a command line.** Any process can read another's
   command line via WMI. This ruled out a detached-CLI handoff and is why the
   engine runs in-process inside `fdm-host.exe`.

10. **UI is dark-only.** `brand/tokens/fdm-tokens.css` has no
    `prefers-color-scheme` branch. A half-finished light theme is worse than
    none. Red (`--fdm-red` `#e50914`) is 4.21:1 on the background — fills,
    borders, icons and large numerals only. For red **text** at body size use
    `--fdm-red-text` (`#ff4b4b`, 6.11:1).

11. **A `targetDir` in a `download` command means "put the file exactly here",
    and the extension deliberately sends none.** `fdm-core/download.rs::
    destination_dir` returns `target_dir` verbatim when set and only sorts into
    `download_root\<Category>\` when it is absent; `explicit_dir` is threaded
    into `finalize` for the same reason. So a test that passes a `targetDir` and
    then expects a category subfolder is asserting the wrong thing — check D in
    `scripts/test-host.py` sends no `targetDir`, which is what
    `extension/background.js` does. Do not "fix" the engine to always
    categorise: that would make an explicit destination unhonourable.

12. **Only the current attempt may write to a download's row, and the check is a
    generation counter.** Cancelling is asynchronous: `pause` returns long before
    the engine has stopped, so a `resume` issued straight afterwards creates a
    second attempt while the first is still winding down.
    `Registry::new_generation` stamps each attempt with a `u64` and every write
    path — `on_start`, `on_progress`, `finalise`, and both checkpoints in the task
    preamble — calls `Registry::is_current` first. Without it, the stale attempt's
    `finalise` writes `Paused` over a row that is running or already `Completed`,
    and the download is stuck in the list forever. Generation 0 is never held by a
    task, so a row restored from disk cannot be mistaken for a live attempt.
    Found by `pause_all` → `resume_all`; pinned by
    `resuming_the_instant_pause_returns_does_not_lose_the_download`.

13. **The per-download run lock is acquired BEFORE the semaphore permit. Never
    invert this.** The lock serialises attempts so two engines never share one
    `.part` file. Permit-then-lock deadlocks: a ghost attempt holding a permit
    waits for a run lock held by another ghost that is itself waiting for a
    permit. Lock-then-permit cannot, because a ghost holding the run lock only
    ever waits on permits other *downloads* will release, and it drops the lock as
    soon as it sees the generation has moved on.

14. **`Runtime` (the manager's per-download non-UI state) is reused across
    attempts, never replaced.** The run lock has to outlive the attempt that took
    it, and the scratch paths have to outlive the attempt that learned them — so
    that a cancel arriving while the previous attempt is still stopping still
    knows which files to delete. Replacing the struct on `resume` is what caused
    the bug in decision 12: the stale task read the *new* runtime's empty intent
    and concluded the user had asked for a pause.

15. **Scratch file names are a deterministic FNV-1a-64 hash of the destination
    path, and the hash must stay stable.** It is what lets a resume find the
    `.part` file a previous run left in the temp dir. `fnv1a64(b"fdm") ==
    0xdcca_7718_feee_6abe` is pinned by a test for exactly this reason. Swapping
    in a "better" hash silently orphans every partial download on every user's
    disk.

16. **Browser headers are not persisted to `downloads.json`.** They live only in
    the in-memory `Runtime`, so a download resumed after a restart re-requests
    with none. That is deliberate: `Cookie` and `Authorization` arrive from the
    extension and writing them to a plaintext file in `%APPDATA%` would leave
    session credentials on disk indefinitely. The cost is that a resume of a
    cookie-gated URL after a restart may 403 — correct behaviour is to fail
    visibly and let the browser hand the download over again.

17. **Cross-volume moves are detected by `raw_os_error()` (17 on Windows, 18 on
    Unix), not `ErrorKind`.** The `ErrorKind` that covers this — `CrossesDevices` —
    is not stable on the toolchain this workspace supports (`rust-version =
    "1.80"`), so the raw code is the only thing available to match on. This path
    matters now that the temp dir defaults to `%LOCALAPPDATA%`, which is routinely
    on a different drive from the download root: the rename fails and
    `scratch.rs`'s copy-then-delete fallback has to run.

---

## 5. How to build and test

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets
```

Zero warnings is the standard here, not a goal — the workspace is currently
clean, so any warning you see is yours.

The manager's integration tests need no network (they serve their own bytes) but
they are *slow on purpose* — about 6 s for the 17 of them. Only this subset:

```bash
cargo test -p fdm-manager --test list
```

```bash
cargo build --release --workspace
```

Full installer (from any directory, absolute path required):

```bash
powershell -ExecutionPolicy Bypass -File "D:\Code\FDM\scripts\build-installer.ps1" -AllowPartial
```

`-AllowPartial` is required until `fdm-desktop.exe` exists. The gate blocks on
`fdm-desktop.exe`, `fdm-host.exe`, and `extension\manifest.json`.

Engine test harness:

```bash
cargo run --release -p fdm-cli -- get <url> -n 16 --sha256
```

End-to-end host test (needs `target\release\fdm-host.exe` and a network):

```bash
python scripts/test-host.py
```

---

## 6. Next actions, in order

1. **Phase 3b — `fdm-ipc`.** A named pipe at `\\.\pipe\fdm.manager` so a second
   process can reach the one download list. `fdm-host.exe` becomes a relay: hand
   the download to the running desktop app if the pipe answers, fall back to
   downloading in-process if it does not. Falling back matters — the extension
   must keep working when the desktop app is closed.
2. **Phase 3c — Tauri desktop UI** (`fdm-desktop.exe`). Consult the
   `ui-ux-pro-max` skill first (standing instruction). This is the last piece the
   installer's completeness gate is waiting on. `fdm-manager`'s `ListEvent`
   stream is the shape the UI subscribes to; do not invent a second one.
3. **Surface `temp_dir` and `use_temp_dir` in Settings** — a folder picker, plus
   "keep partial files next to the finished file instead". The engine and config
   already support both; the UI is the missing half of the requirement.
4. Re-run `build-installer.ps1` **without** `-AllowPartial`.
5. **Load `extension/` unpacked in a real Chrome** and confirm the takeover path
   end to end: install the native-host registry key, click a download link, and
   watch the popup fill in. Everything up to this point has been tested by
   impersonating Chrome, not by Chrome itself.
6. Add a build step that re-copies `brand/tokens/fdm-tokens.css` →
   `extension/styles/tokens.css` so the two cannot drift.
7. Refresh the `README.md` Status table.
8. Phases 5–6: scheduler, queues, speed limiter, site logins, clipboard
   monitoring; then HTTP/3, multi-source/Metalink, HLS/DASH.
9. Publish to the Chrome Web Store for a real extension ID, then rebuild with
   `-ExtensionId <id>`.
10. Buy a real code-signing certificate (Certum Open Source, ~€30–90/yr) and
    rebuild with `-SignThumbprint <hex>`.

---

## 7. Known gaps

- `README.md`'s Status table is stale.
- **The temp directory is configurable but not yet *configurable by the user*** —
  `temp_dir` and `use_temp_dir` are real `EngineConfig` fields with tests behind
  them, and nothing in a UI reads or writes them yet. See next action 3.
- **`cancel` immediately followed by `resume` may continue from the old `.part`
  instead of starting over.** The cancelled attempt is still winding down when the
  new one claims the row, so it never gets to discard its scratch files. The bytes
  are identical either way and the finished file is correct — a restart that
  behaved as a resume, not corruption. Worth closing when `cancel` grows a "wait
  for the engine to let go" path, and not before.
- **A download resumed after a restart carries no browser headers** (decision 16),
  so a cookie-gated URL can come back 401/403. The row records the error and the
  user can hand it over from the browser again. Acceptable; a credential store is
  not.
- `crates/fdm-manager` has no IPC yet, so the extension's downloads and the
  desktop app's list are two separate worlds. That is next action 1 and it is the
  single biggest structural gap left.
- **`extension/styles/tokens.css` is a manual copy** of
  `brand/tokens/fdm-tokens.css`. An unpacked extension cannot reference files
  outside its own directory, so the copy is necessary — but nothing yet stops
  the two from drifting. See next action 4.
- The Chrome extension ID is `UNPUBLISHED`; the installer therefore skips the
  extension pre-install registry keys, and the native host manifest's
  `allowed_origins` points at a placeholder. Both are correct until publication.
- The dev signing certificate is self-signed, so
  `Get-AuthenticodeSignature` reports `Status : UnknownError` (untrusted root).
  Expected. A purchased certificate runs the identical code path.
- **The extension has never run inside a real Chrome.** `scripts/test-host.py`
  covers the host side of the wire; the `chrome.downloads` takeover, the
  `onInstalled` welcome tab and the popup's live rows have only been verified by
  measurement and static analysis. See next action 3.
- `blob:` and `data:` downloads are declined by design and always will be — a
  separate process cannot re-fetch a URL that exists only inside the page.
- Incognito downloads are declined by design: a cancelled incognito download
  would leave a `.fdm` control file and a partial file on disk after the window
  closed.

