#!/usr/bin/env python3
"""Smoke-test install.sh / install.ps1 against a fake release served locally.

    python3 packaging/installer_smoke.py

Builds a release archive for this machine holding a stand-in binary, serves it
over HTTP on localhost, and runs the real installer twice: once expecting a
clean install, once with a wrong checksum expecting it to refuse. Uses
install.ps1 on Windows and install.sh elsewhere. Needs no network access.
"""

import hashlib
import http.server
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import zipfile
from functools import partial
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VERSION = "v0.0.1"


def host_target() -> str:
    machine = platform.machine().lower()
    arch = "aarch64" if machine in ("arm64", "aarch64") else "x86_64"
    system = platform.system()
    if system == "Darwin":
        # Match install.sh, which sees through Rosetta.
        try:
            out = subprocess.run(
                ["sysctl", "-n", "hw.optional.arm64"], capture_output=True, text=True
            ).stdout.strip()
            if out == "1":
                arch = "aarch64"
        except OSError:
            pass
        return f"{arch}-apple-darwin"
    if system == "Windows":
        return f"{arch}-pc-windows-msvc"
    return f"{arch}-unknown-linux-gnu"


def make_release(root: Path, target: str) -> Path:
    """Write download/<VERSION>/<archive> and its .sha256 under `root`."""
    folder = root / "download" / VERSION
    folder.mkdir(parents=True)
    name = f"sakshi-{VERSION}-{target}"
    if "windows" in target:
        archive = folder / f"{name}.zip"
        with zipfile.ZipFile(archive, "w") as z:
            z.writestr("sakshi.exe", b"stand-in")
            z.writestr("LICENSE", b"MIT")
    else:
        staging = root / "staging" / name
        staging.mkdir(parents=True)
        binary = staging / "sakshi"
        binary.write_text('#!/bin/sh\necho "sakshi stand-in"\n')
        binary.chmod(0o755)
        archive = folder / f"{name}.tar.gz"
        with tarfile.open(archive, "w:gz") as t:
            t.add(staging, arcname=name)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (folder / (archive.name + ".sha256")).write_text(f"{digest}  {archive.name}\n")
    return archive


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def serve(directory: Path):
    handler = partial(QuietHandler, directory=str(directory))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def run_installer(base: str, install_dir: Path) -> subprocess.CompletedProcess:
    env = dict(
        os.environ,
        SAKSHI_DOWNLOAD_BASE=base,
        SAKSHI_VERSION=VERSION,
        SAKSHI_INSTALL_DIR=str(install_dir),
        SAKSHI_NO_MODIFY_PATH="1",
    )
    if platform.system() == "Windows":
        shell = shutil.which("pwsh") or "powershell"
        cmd = [shell, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(ROOT / "install.ps1")]
    else:
        cmd = ["sh", str(ROOT / "install.sh")]
    return subprocess.run(cmd, env=env, capture_output=True, text=True)


def main() -> int:
    target = host_target()
    exe = "sakshi.exe" if "windows" in target else "sakshi"
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        archive = make_release(tmp / "release", target)
        server = serve(tmp / "release")
        base = f"http://127.0.0.1:{server.server_address[1]}"
        try:
            good = run_installer(base, tmp / "good")
            print(good.stdout, good.stderr, sep="")
            if good.returncode != 0 or not (tmp / "good" / exe).is_file():
                print(f"FAIL: clean install of {target} did not produce {exe}")
                return 1
            if "windows" not in target:
                out = subprocess.run([str(tmp / "good" / exe)], capture_output=True, text=True)
                if "stand-in" not in out.stdout:
                    print("FAIL: installed binary does not run")
                    return 1

            sums = archive.with_name(archive.name + ".sha256")
            sums.write_text("0" * 64 + f"  {archive.name}\n")
            bad = run_installer(base, tmp / "bad")
            print(bad.stdout, bad.stderr, sep="")
            if bad.returncode == 0 or (tmp / "bad" / exe).exists():
                print("FAIL: a wrong checksum was accepted")
                return 1
        finally:
            server.shutdown()
    print(f"OK: installer verified for {target}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
