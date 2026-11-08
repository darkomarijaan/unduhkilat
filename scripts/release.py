#!/usr/bin/env python3
"""Rilis UnduhKilat dalam satu perintah.

    python scripts/release.py 0.3.5 --notes CATATAN.md --trailer "Co-Authored-By: ..." [--dry-run]

Langkah: cek awal -> versi di Cargo.toml -> cargo build --release -> zip portabel (pemisah '/')
-> SHA-256 -> tanda tangan (diverifikasi, kunci harus sama dengan PUBLIC_KEY_B64 updater)
-> commit "chore: versi X" + push -> GitHub pre-release -> cek ukuran aset terunggah.

Catatan rilis cukup berisi daftar perubahan; baris "Pembaruan dari", pembaruan otomatis, dan
SHA-256 ditambahkan otomatis. Kunci privat dibaca dari ~/.unduhkilat-signing/release.key
(atau --key) dan tidak pernah disalin ke repo.
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import time
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REPO = "darkomarijaan/unduhkilat"
DIST = ROOT / "dist"
EXE = ROOT / "target" / "release"
DEFAULT_KEY = Path.home() / ".unduhkilat-signing" / "release.key"

T0 = time.time()
_step_start = T0


def step(name):
    global _step_start
    now = time.time()
    print(f"\n== {name}  (+{now - T0:.0f} d)", flush=True)
    _step_start = now


def die(msg):
    print(f"\nGAGAL: {msg}", file=sys.stderr)
    sys.exit(1)


def run(*cmd, capture=False, check=True, cwd=ROOT):
    r = subprocess.run(cmd, cwd=cwd, text=True, capture_output=capture, encoding="utf-8", errors="replace")
    if check and r.returncode != 0:
        die(f"{' '.join(cmd)} keluar dengan kode {r.returncode}\n{(r.stdout or '')[-2000:]}{(r.stderr or '')[-2000:]}")
    return r.stdout.strip() if capture else ""


def current_version():
    m = re.search(r'\[workspace\.package\]\s*\nversion = "([0-9.]+)"', (ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    if not m:
        die("versi [workspace.package] tidak ditemukan di Cargo.toml")
    return m.group(1)


def vtuple(v):
    return tuple(int(x) for x in v.split("."))


def set_version(v):
    p = ROOT / "Cargo.toml"
    s = p.read_text(encoding="utf-8")
    s2 = re.sub(r'(\[workspace\.package\]\s*\nversion = ")[0-9.]+(")', rf"\g<1>{v}\g<2>", s, count=1)
    p.write_bytes(s2.encode("utf-8"))


def public_key():
    m = re.search(r'PUBLIC_KEY_B64: &str = "([^"]+)"', (ROOT / "crates/unduhkilat-core/src/update.rs").read_text(encoding="utf-8"))
    if not m:
        die("PUBLIC_KEY_B64 tidak ditemukan di update.rs")
    return m.group(1)


def preflight(a, version):
    step("Cek awal")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        die(f"versi {version!r} bukan x.y.z")
    prev = current_version()
    if vtuple(version) <= vtuple(prev):
        die(f"versi {version} tidak lebih baru dari {prev}")
    branch = run("git", "rev-parse", "--abbrev-ref", "HEAD", capture=True)
    if branch != "main":
        die(f"harus di branch main, sekarang {branch}")
    dirty = run("git", "status", "--porcelain", capture=True)
    if dirty:
        die("working tree belum bersih; commit perubahan fitur dulu:\n" + dirty)
    if not a.key.is_file():
        die(f"kunci privat tidak ada: {a.key}")
    if not a.notes.is_file() or not a.notes.read_text(encoding="utf-8").strip():
        die(f"catatan rilis kosong atau tidak ada: {a.notes}")
    if run("git", "ls-remote", "--tags", "origin", f"refs/tags/v{version}", capture=True):
        die(f"tag v{version} sudah ada di GitHub")
    run("gh", "auth", "status", capture=True)
    print(f"   {prev} -> {version}, branch main bersih, kunci ada, gh siap")
    return prev


def build():
    step("cargo build --release")
    run("cargo", "build", "--release")
    for exe in ("UnduhKilat.exe", "unduhkilat-host.exe", "unduhkilat-sign.exe"):
        if not (EXE / exe).is_file():
            die(f"{exe} tidak ada setelah build")


def make_zip(version):
    step("Zip portabel")
    DIST.mkdir(exist_ok=True)
    zpath = DIST / f"UnduhKilat-portable-{version}-x64.zip"
    zpath.unlink(missing_ok=True)
    with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED) as z:
        z.write(ROOT / "packaging" / "BACA-SAYA.txt", "BACA-SAYA.txt")
        z.write(EXE / "UnduhKilat.exe", "UnduhKilat.exe")
        z.write(EXE / "unduhkilat-host.exe", "unduhkilat-host.exe")
        z.writestr("unduhkilat-portable.marker", b"")
        ext = ROOT / "extension"
        for f in sorted(ext.rglob("*")):
            rel = f.relative_to(ext)
            if f.is_file() and rel.parts[0] != "tests":
                z.write(f, "extension/" + rel.as_posix())
    names = zipfile.ZipFile(zpath).namelist()
    if any("\\" in n for n in names) or "extension/manifest.json" not in names:
        die(f"isi zip tidak sesuai: {names}")
    print(f"   {zpath.name}: {len(names)} berkas, {zpath.stat().st_size / 1e6:.1f} MB")
    return zpath


def hash_and_sign(zpath, key):
    step("SHA-256 dan tanda tangan")
    h = hashlib.sha256(zpath.read_bytes()).hexdigest()
    Path(f"{zpath}.sha256").write_bytes(f"{h} *{zpath.name}".encode("ascii"))
    # `sign` memverifikasi tanda tangannya sendiri dan menolak kunci yang tidak dipercaya updater.
    out = run(str(EXE / "unduhkilat-sign.exe"), "sign", str(key), str(zpath), "--expect-public", public_key(), capture=True)
    print(f"   {out}\n   SHA-256 {h}")
    return h


def commit_and_push(version, trailers):
    step("Commit versi dan push")
    msg = f"chore: versi {version}"
    if trailers:
        msg += "\n\n" + "\n".join(trailers)
    run("git", "add", "Cargo.toml", "Cargo.lock")
    run("git", "commit", "-q", "-m", msg)
    run("git", "push", "-q", "origin", "main")
    print(f"   {run('git', 'log', '--oneline', '-1', capture=True)}")


def publish(version, prev, notes, zpath, sha, prerelease):
    step("GitHub release")
    body = f"Pembaruan dari {prev}:\n\n{notes.strip()}\n\n" \
           f"Pembaruan otomatis: versi {prev} akan menawarkan rilis ini lewat banner, dengan verifikasi tanda tangan.\n" \
           f"Portabel, Windows 10/11 x64. SHA-256 zip: {sha}\n"
    nfile = DIST / f"notes-{version}.md"
    nfile.write_bytes(body.encode("utf-8"))
    files = [str(zpath), f"{zpath}.sha256", f"{zpath}.sig"]
    cmd = ["gh", "release", "create", f"v{version}", "-R", REPO, "--target", "main", "--title", f"UnduhKilat {version}", "--notes-file", str(nfile)]
    if prerelease:
        cmd.append("--prerelease")
    run(*cmd, *files)
    # Cek ukuran lewat API (tanpa mengunduh ulang).
    info = json.loads(run("gh", "release", "view", f"v{version}", "-R", REPO, "--json", "assets,url", capture=True))
    got = {x["name"]: x["size"] for x in info["assets"]}
    for f in files:
        name, size = Path(f).name, Path(f).stat().st_size
        if got.get(name) != size:
            die(f"aset {name} di GitHub berukuran {got.get(name)}, lokal {size}")
    print(f"   3 aset terunggah dengan ukuran cocok\n   {info['url']}")


def main():
    ap = argparse.ArgumentParser(description="Rilis UnduhKilat")
    ap.add_argument("version")
    ap.add_argument("--notes", type=Path, required=True, help="Markdown daftar perubahan (tanpa header/footer)")
    ap.add_argument("--trailer", action="append", default=[], help="baris atribusi commit, boleh berulang")
    ap.add_argument("--key", type=Path, default=DEFAULT_KEY)
    ap.add_argument("--stable", action="store_true", help="rilis biasa, bukan pre-release")
    ap.add_argument("--dry-run", action="store_true", help="build, zip, dan tanda tangan saja; tanpa commit/push/rilis; versi dikembalikan")
    a = ap.parse_args()
    os.chdir(ROOT)

    prev = preflight(a, a.version)
    set_version(a.version)
    try:
        build()
        zpath = make_zip(a.version)
        sha = hash_and_sign(zpath, a.key)
    except SystemExit:
        set_version(prev)
        run("git", "checkout", "--", "Cargo.lock", check=False)
        raise
    if a.dry_run:
        set_version(prev)
        run("git", "checkout", "--", "Cargo.lock", check=False)
        print(f"\nDRY RUN selesai dalam {time.time() - T0:.0f} d; versi dikembalikan ke {prev}. Zip ada di {zpath}")
        return
    commit_and_push(a.version, a.trailer)
    publish(a.version, prev, a.notes.read_text(encoding="utf-8"), zpath, sha, not a.stable)
    print(f"\nSELESAI dalam {time.time() - T0:.0f} detik.")


if __name__ == "__main__":
    main()
