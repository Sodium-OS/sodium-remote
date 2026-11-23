#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Makes the release source tarball the Arch package (packaging/arch/PKGBUILD) and the release's "Source" file are built from:
#   sodium-remote-<version>.tar.gz   the tracked files of the checkout (git archive), under sodium-remote-<version>/, plus
#                                    notices/THIRD-PARTY-NOTICES-app.md and notices/THIRD-PARTY-NOTICES-server.md
#                                    (cargo-about, one list per shipped binary, for the Linux target)
#
#   packaging/arch/make-source-tarball.sh VERSION OUTDIR
#
# Run from anywhere; the repository is the one this script is in. Needs cargo-about (pacman -S cargo-about), git, tar and gzip.
# Outside a git checkout (an exported tree) the files are taken as they are, without target/ and build/.
# The tarball is reproducible: files sorted, owner 0, a fixed time. The same commit gives the same checksum.
set -euo pipefail

version=${1:?usage: make-source-tarball.sh VERSION OUTDIR}
out=$(mkdir -p "${2:?usage: make-source-tarball.sh VERSION OUTDIR}" && cd "$2" && pwd)
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
name=sodium-remote-$version
work=$out/.work-$name
rm -rf "$work"; mkdir -p "$work/notices"

cd "$root"
about() { cargo about generate --locked about.hbs --manifest-path "$1" --target x86_64-unknown-linux-gnu --output-file "$work/notices/$2"; }
about client-qt/Cargo.toml THIRD-PARTY-NOTICES-app.md
about relay/Cargo.toml THIRD-PARTY-NOTICES-server.md

mkdir -p "$work/tree"
if [[ $(git -C "$root" rev-parse --show-toplevel 2>/dev/null || true) == "$root" ]]; then
  git -C "$root" archive --format=tar --prefix="$name/" HEAD | tar -xf - -C "$work/tree"
else
  mkdir -p "$work/tree/$name"
  tar -C "$root" --exclude=./target --exclude=./build --exclude=./build-app -cf - . | tar -xf - -C "$work/tree/$name"
fi
mkdir -p "$work/tree/$name/notices"
cp "$work/notices/"* "$work/tree/$name/notices/"

( cd "$work/tree" && find "$name" -print0 | LC_ALL=C sort -z \
    | tar --null --no-recursion -T - --owner=0 --group=0 --numeric-owner --mtime='2026-01-01 00:00:00Z' -cf - ) \
  | gzip -9n > "$out/$name.tar.gz"
rm -rf "$work"
echo "$out/$name.tar.gz"
sha256sum "$out/$name.tar.gz"
