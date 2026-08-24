#!/usr/bin/env python3
"""End-to-end test of fdm-host.exe by impersonating Chrome.

This covers the one link in the chain no unit test can reach: the extension
talks to the host over stdio using Chrome's native-messaging framing, and a
mistake there produces a host that merely *appears* to hang. So we speak the
protocol ourselves — 32-bit NATIVE-order length prefix, UTF-8 JSON body — and
assert on what comes back.

What is checked:
  A  ping -> pong, and the pong carries every field welcome.html renders
  B  a wrong protocol version is rejected with versionMismatch, not ignored
  C  an explicit targetDir is honoured exactly, and progress messages flow
  D  with NO targetDir — what the extension actually sends — the file lands in
     Downloads\\FDM\\<Category>\\, which is the product requirement
  E  stdin EOF mid-download does not kill the host (HANDOFF decision 7)

Run:  python scripts/test-host.py
"""

from __future__ import annotations

import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HOST = ROOT / "target" / "release" / "fdm-host.exe"
PROTOCOL = 1

SMALL_URL = "https://proof.ovh.net/files/10Mb.dat"
LARGE_URL = "https://proof.ovh.net/files/100Mb.dat"

# Native byte order, matching crates/fdm-host/src/framing.rs. Chrome uses the
# platform's order, not network order; getting this wrong is a silent hang.
LEN = struct.Struct("=I")

failures: list[str] = []
cleanup: list[Path] = []


def fail(msg: str) -> None:
    failures.append(msg)
    print(f"FAIL  {msg}")


def ok(msg: str) -> None:
    print(f"PASS  {msg}")


# --------------------------------------------------------------------- plumbing


class Host:
    """A running fdm-host.exe, with stdout drained on its own thread so a stall
    surfaces as a timeout instead of a deadlock."""

    def __init__(self) -> None:
        # stderr is the host's log channel and must never carry protocol bytes;
        # keeping it separate means a stray println! shows up as a parse error.
        self.proc = subprocess.Popen(
            [str(HOST)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.msgs: list[dict] = []
        threading.Thread(target=self._drain, daemon=True).start()

    def _drain(self) -> None:
        while True:
            head = self.proc.stdout.read(4)
            if len(head) < 4:
                return
            (n,) = LEN.unpack(head)
            body = b""
            while len(body) < n:
                chunk = self.proc.stdout.read(n - len(body))
                if not chunk:
                    return
                body += chunk
            try:
                self.msgs.append(json.loads(body))
            except json.JSONDecodeError as e:
                self.msgs.append({
                    "type": "PARSE_ERROR",
                    "error": str(e),
                    "raw": body[:200].decode("utf-8", "replace"),
                })

    def send(self, msg: dict) -> None:
        body = json.dumps(msg).encode("utf-8")
        self.proc.stdin.write(LEN.pack(len(body)) + body)
        self.proc.stdin.flush()

    def wait(self, kinds: set[str], timeout: float, label: str,
             *, mid: int | None = None, start: int = 0) -> dict | None:
        """Block for a message of one of `kinds` at or after index `start`.

        Matching on `id` as well as type matters: replies accumulate, so a scan
        from the beginning happily re-matches the answer to an earlier request
        and the assertion then runs against the wrong message.
        """
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for msg in self.msgs[start:]:
                if msg.get("type") in kinds and (mid is None or msg.get("id") == mid):
                    return msg
            if self.proc.poll() is not None and not self.msgs[start:]:
                break
            time.sleep(0.05)
        seen = [(m.get("type"), m.get("id")) for m in self.msgs[start:]]
        fail(f"no {label} within {timeout}s (saw {seen})")
        return None

    def stderr_text(self) -> str:
        try:
            return self.proc.stderr.read().decode("utf-8", "replace")
        except Exception:
            return ""

    def kill(self) -> None:
        if self.proc.poll() is None:
            self.proc.kill()
        self.proc.wait(timeout=10)


def download_cmd(mid: int, url: str, target: str | None, filename: str | None) -> dict:
    return {
        "type": "download",
        "id": mid,
        "url": url,
        "headers": {"User-Agent": "FDM-host-test/1.0"},
        "filename": filename,
        "totalBytes": None,
        "targetDir": target,
        "protocol": PROTOCOL,
    }


# ------------------------------------------------------------------------ tests


def test_handshake() -> dict | None:
    print("\n--- A/B  handshake -------------------------------------------------")
    host = Host()
    try:
        host.send({"type": "ping", "id": 1, "protocol": PROTOCOL})
        pong = host.wait({"pong", "error"}, 10, "pong", mid=1)
        if not pong:
            return None
        if pong.get("type") != "pong":
            fail(f"ping returned {pong}")
            return None
        ok("ping -> pong")
        # welcome.js reads every one of these; a missing field renders an em dash.
        for field in ("protocol", "version", "hostPath", "downloadRoot",
                      "maxConnections", "categories"):
            if field not in pong:
                fail(f"pong is missing '{field}' — welcome.html renders it")
        print(f"      version={pong.get('version')} protocol={pong.get('protocol')} "
              f"maxConnections={pong.get('maxConnections')}")
        print(f"      downloadRoot={pong.get('downloadRoot')}")
        print(f"      categories={pong.get('categories')}")

        host.send({"type": "ping", "id": 2, "protocol": 999})
        bad = host.wait({"error", "pong"}, 10, "mismatch reply", mid=2)
        if bad and bad.get("type") == "error" and bad.get("versionMismatch"):
            ok("protocol 999 -> error{versionMismatch: true}")
        elif bad:
            fail(f"a wrong protocol version was not rejected: {bad}")
        return pong
    finally:
        host.kill()


def test_explicit_target() -> None:
    print("\n--- C  explicit targetDir is honoured exactly ----------------------")
    target = Path(tempfile.mkdtemp(prefix="fdm-host-test-"))
    cleanup.append(target)
    host = Host()
    try:
        host.send(download_cmd(100, SMALL_URL, str(target), None))
        if not host.wait({"accepted"}, 15, "accepted", mid=100):
            return
        ok("download -> accepted")

        done = host.wait({"completed", "failed"}, 300, "completed", mid=100)
        if not done or done.get("type") != "completed":
            fail(f"download did not complete: {done}")
            return

        progress = [m for m in host.msgs if m.get("type") == "progress"]
        if not progress:
            fail("no progress messages — the popup would show a frozen bar")
        else:
            last = progress[-1]
            for field in ("downloaded", "total", "speedBps", "segments"):
                if field not in last:
                    fail(f"progress is missing '{field}' — popup.js reads it")
            ok(f"{len(progress)} progress messages "
               f"(last: {last.get('downloaded')}/{last.get('total')} bytes, "
               f"{last.get('segments')} segments, {last.get('activeConnections')} active)")

        path = Path(done["path"])
        # An explicit targetDir means "put it exactly here" — the engine sets
        # explicit_dir and skips category sorting on purpose. See
        # fdm-core/download.rs::destination_dir.
        if path.parent != target:
            fail(f"explicit targetDir was not honoured: asked for {target}, got {path.parent}")
        elif not path.exists():
            fail(f"host reported {path} but it does not exist")
        elif path.stat().st_size != done.get("bytes"):
            fail(f"size on disk {path.stat().st_size} != reported {done.get('bytes')}")
        else:
            ok(f"landed exactly at {path}, {path.stat().st_size} bytes, "
               f"usedRanges={done.get('usedRanges')}")
    finally:
        host.kill()


def test_category_sorting(pong: dict) -> None:
    print("\n--- D  no targetDir -> Downloads\\FDM\\<Category>\\ -------------------")
    root = Path(pong["downloadRoot"])
    # The filename decides the category, and the extension always sends one it
    # got from Chrome. A .zip must sort into Compressed.
    name = "fdm-selftest-delete-me.zip"
    host = Host()
    try:
        # No targetDir — exactly what extension/background.js sends.
        host.send(download_cmd(200, SMALL_URL, None, name))
        if not host.wait({"accepted"}, 15, "accepted", mid=200):
            return
        done = host.wait({"completed", "failed"}, 300, "completed", mid=200)
        if not done or done.get("type") != "completed":
            fail(f"download did not complete: {done}")
            return

        path = Path(done["path"])
        cleanup.append(path)
        if path.parent == root:
            fail("file landed in the download root, not a category subfolder — "
                 "the 'auto-arranged by type' requirement is broken")
        elif path.parent.parent != root:
            fail(f"expected {root}\\<Category>\\, got {path.parent}")
        elif path.name != name:
            fail(f"filename from the extension was not honoured: sent {name}, got {path.name}")
        elif not path.exists():
            fail(f"host reported {path} but it does not exist")
        else:
            ok(f"sorted into '{path.parent.name}\\' -> {path}")
            ok(f"category reported as '{done.get('category')}', filename honoured")
    finally:
        host.kill()


def test_survives_stdin_eof() -> None:
    print("\n--- E  stdin EOF mid-download does not kill the host ---------------")
    target = Path(tempfile.mkdtemp(prefix="fdm-host-eof-"))
    cleanup.append(target)
    host = Host()
    try:
        host.send(download_cmd(300, LARGE_URL, str(target), None))
        if not host.wait({"accepted"}, 15, "accepted", mid=300):
            return
        # Wait for real bytes to be moving, so EOF genuinely lands mid-flight
        # rather than after the work is already done.
        first = host.wait({"progress"}, 30, "first progress", mid=300)
        if not first:
            return
        moved = first.get("downloaded", 0)
        total = first.get("total") or 0
        if total and moved >= total:
            fail("download finished before stdin could be closed — test is not "
                 "exercising the EOF path; use a larger file")
            return
        print(f"      closing stdin at {moved}/{total} bytes "
              f"({100 * moved / total:.0f}%)" if total else "")

        # This is what Chrome does when it evicts the service worker.
        host.proc.stdin.close()

        time.sleep(1.0)
        if host.proc.poll() is not None:
            fail(f"host exited on stdin EOF (code {host.proc.returncode}) while a "
                 f"download was in flight — HANDOFF decision 7")
            return
        ok("host still alive 1s after stdin EOF")

        done = host.wait({"completed", "failed"}, 300, "completed after EOF", mid=300)
        if not done or done.get("type") != "completed":
            fail(f"download did not survive stdin EOF: {done}")
            return
        path = Path(done["path"])
        if not path.exists() or path.stat().st_size != done.get("bytes"):
            fail(f"file after EOF is wrong: exists={path.exists()}")
        else:
            ok(f"finished after EOF: {path.stat().st_size} bytes at {path}")

        # Having drained its work, it should now shut down on its own.
        for _ in range(100):
            if host.proc.poll() is not None:
                break
            time.sleep(0.1)
        if host.proc.poll() is None:
            fail("host did not exit after finishing its last download post-EOF")
        else:
            ok(f"exited cleanly once its work was done (code {host.proc.returncode})")
    finally:
        stderr = host.stderr_text()
        host.kill()
        if stderr.strip():
            tail = "\n".join(stderr.strip().splitlines()[-4:])
            print(f"      host stderr tail:\n      {tail}")


# ------------------------------------------------------------------------- main


def main() -> int:
    if not HOST.exists():
        raise SystemExit(f"FAIL: {HOST} not built. Run: cargo build --release --workspace")
    print(f"host: {HOST}")

    pong = test_handshake()
    if not pong:
        return report()
    test_explicit_target()
    test_category_sorting(pong)
    test_survives_stdin_eof()

    for p in cleanup:
        try:
            if p.is_dir():
                shutil.rmtree(p, ignore_errors=True)
            elif p.exists():
                os.remove(p)
        except OSError:
            pass

    return report()


def report() -> int:
    print()
    if failures:
        print(f"FAILED ({len(failures)}):")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("ALL CHECKS PASSED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
