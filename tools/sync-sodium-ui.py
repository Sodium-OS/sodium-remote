#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Copies the part of Sodium UI that Sodium Remote's app uses out of a SodiumOS checkout, one way.

    tools/sync-sodium-ui.py --from <SodiumOS checkout> [--check]

sodium-ui/MANIFEST lists the files: `copy PATH` is copied from <checkout>/ui/PATH byte for byte, plus the SPDX header on top (as spdx-headers.py writes it); `derived PATH [FROM]`
is ours (trimmed), never written by this tool, and UPSTREAM keeps the hash of ui/FROM it was derived from, so a change
upstream is reported ("derived file needs a look") and the hash is updated only with --accept-derived.
UPSTREAM records the last SodiumOS commit that touched ui/. Nothing flows back: edit Sodium UI in SodiumOS, then sync.
--check changes nothing and exits 1 when sodium-ui/ differs from what a sync would write.
"""
import argparse, hashlib, importlib.util, subprocess, sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEST = HERE.parent / "sodium-ui"

# The vendored copies carry the SPDX header (tools/spdx-headers.py puts it on every source file), which upstream's
# files don't: a copy is upstream's bytes with that header on top, so a re-sync keeps it and --check expects it.
_spec = importlib.util.spec_from_file_location("spdx_headers", HERE / "spdx-headers.py")
_spdx = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_spdx)


def vendored(rel: str, upstream: bytes) -> bytes:
    """What sodium-ui/<rel> must hold for this upstream content."""
    return _spdx.with_header(rel, upstream)[1]


def sha(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def read_manifest():
    copies, derived = [], []
    for line in (DEST / "MANIFEST").read_text().splitlines():
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        w = line.split()
        if w[0] == "copy" and len(w) == 2:
            copies.append(w[1])
        elif w[0] == "derived" and len(w) in (2, 3):
            derived.append((w[1], w[2] if len(w) == 3 else w[1]))
        else:
            sys.exit(f"MANIFEST: cannot read '{line}'")
    return copies, derived


def read_upstream():
    commit, hashes = "", {}
    f = DEST / "UPSTREAM"
    if f.exists():
        for line in f.read_text().splitlines():
            w = line.split()
            if len(w) == 2 and w[0] == "commit":
                commit = w[1]
            elif len(w) == 3 and w[0] == "derived":
                hashes[w[1]] = w[2]
    return commit, hashes


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--from", dest="src", required=True, type=Path, help="a SodiumOS checkout")
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--accept-derived", action="store_true", help="record the current upstream hashes of the derived files")
    a = ap.parse_args()
    ui = a.src / "ui"
    if not (ui / "SodiumUI" / "qmldir").is_file():
        sys.exit(f"{a.src} is not a SodiumOS checkout (no ui/SodiumUI/qmldir)")
    copies, derived = read_manifest()
    old_commit, old_hashes = read_upstream()
    out = subprocess.run(["git", "-C", str(a.src), "log", "-1", "--format=%H", "--", "ui"], capture_output=True, text=True)
    commit = out.stdout.strip() or old_commit
    missing = [c for c in copies if not (ui / c).is_file()]
    if missing:
        sys.exit("missing upstream: " + ", ".join(missing))
    differ = []
    for c in copies:
        d = DEST / c
        want = vendored(f"sodium-ui/{c}", (ui / c).read_bytes())
        if not d.is_file() or d.read_bytes() != want:
            differ.append(c)
            if not a.check:
                d.parent.mkdir(parents=True, exist_ok=True)
                d.write_bytes(want)
    new_hashes = {}
    stale = []
    for path, frm in derived:
        if not (ui / frm).is_file():
            sys.exit(f"missing upstream for derived {path}: {frm}")
        new_hashes[path] = sha(ui / frm)
        if path in old_hashes and old_hashes[path] != new_hashes[path]:
            stale.append(path)
        if not (DEST / path).is_file():
            sys.exit(f"derived file {path} is missing from sodium-ui/")
    # files in sodium-ui/ that the manifest no longer lists
    listed = set(copies) | {p for p, _ in derived} | {"MANIFEST", "UPSTREAM"}
    extra = sorted(str(p.relative_to(DEST)) for p in DEST.rglob("*") if p.is_file() and str(p.relative_to(DEST)) not in listed)
    for s in stale:
        print(f"derived file needs a look: sodium-ui/{s} (upstream ui/{dict(derived)[s]} changed)")
    for e in extra:
        print(f"not in MANIFEST: sodium-ui/{e}")
    if a.check:
        for c in differ:
            print(f"differs: {c}")
        sys.exit(1 if differ or stale or extra else 0)
    keep = new_hashes if (a.accept_derived or not old_hashes) else {p: old_hashes.get(p, h) for p, h in new_hashes.items()}
    text = f"commit {commit}\n" + "".join(f"derived {p} {h}\n" for p, h in sorted(keep.items()))
    (DEST / "UPSTREAM").write_text(text)
    print(f"sodium-ui: {len(copies)} files copied ({len(differ)} changed), {len(derived)} derived, upstream {commit[:8]}")


main()
