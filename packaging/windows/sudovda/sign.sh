#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Signs an unsigned build of the matched display's driver (the "sodium-remote-display-unsigned" artifact of
# .github/workflows/sudovda.yml, or build.ps1's output zipped) with Sodium Remote's own key, on the machine that keeps it:
# the DLL and the catalog, with osslsigncode, SHA-256, timestamped. Records the artifact's SHA-256 and the commit it was
# built from in UPSTREAM, and puts the signed files in OUT for pin.sh and stage-app.sh.
#
#   gh api repos/OWNER/REPO/actions/artifacts/ID/zip > build/sodium-remote-display-unsigned.zip
#   packaging/windows/sudovda/sign.sh build/sodium-remote-display-unsigned.zip [--out DIR] [--no-timestamp]
#   packaging/windows/sudovda/pin.sh --write
#
# OUT defaults to remote/build/sudovda. The key: signing.pfx in $SODIUM_REMOTE_SIGNING_DIR (default
# ~/.local/share/sodium-remote-signing, from make-cert.sh), its password in $SODIUM_REMOTE_SIGNING_PASS (asked for when
# unset; handed to osslsigncode through a pipe, never on a command line). Compare the zip's SHA-256 printed here with the
# digest the workflow run shows. Needs osslsigncode (2.5 or later), python3, sha256sum and openssl.
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMOTE=$(cd "$HERE/../../.." && pwd)
OUT=$REMOTE/build/sudovda ZIP= TS=http://timestamp.digicert.com
while [[ $# -gt 0 ]]; do
  case $1 in
    --out) OUT=${2:?--out needs a folder}; shift 2 ;;
    --no-timestamp) TS=; shift ;;
    -*) echo "usage: sign.sh ARTIFACT.zip [--out DIR] [--no-timestamp]" >&2; exit 2 ;;
    *) ZIP=$1; shift ;;
  esac
done
[[ -f $ZIP ]] || { echo "usage: sign.sh ARTIFACT.zip [--out DIR] [--no-timestamp]" >&2; exit 2; }
for t in osslsigncode python3 sha256sum openssl; do command -v $t >/dev/null || { echo "missing tool: $t" >&2; exit 1; }; done
KEYDIR=${SODIUM_REMOTE_SIGNING_DIR:-$HOME/.local/share/sodium-remote-signing}
PFX=$KEYDIR/signing.pfx
[[ -f $PFX ]] || { echo "no key at $PFX: run make-cert.sh first" >&2; exit 1; }
FILES=(SodiumRemoteDisplay.inf SodiumRemoteDisplay.dll sodiumremotedisplay.cat)

mkdir -p "$REMOTE/build"
work=$(mktemp -d "$REMOTE/build/sign.XXXXXX")
trap 'rm -rf "$work"' EXIT
python3 -I -m zipfile -e "$ZIP" "$work/in"
in=$work/in
for f in "${FILES[@]}" BUILDINFO SHA256SUMS; do [[ -f $in/$f ]] || { echo "the artifact has no $f" >&2; exit 1; }; done
# Written by PowerShell: CRLF.
sed -i 's/\r$//' "$in/BUILDINFO" "$in/SHA256SUMS"
(cd "$in" && sha256sum --quiet -c SHA256SUMS) || { echo "the artifact's files don't match its SHA256SUMS" >&2; exit 1; }

# Built from the pinned commit with exactly this tree's patches.
commit=$(sed -n 's/^pub const COMMIT: &str = "\(.*\)";/\1/p' "$REMOTE/host-desktop/src/vdd_pin.rs")
read -r _ _ built <<<"$(grep '^upstream ' "$in/BUILDINFO")"
[[ $built == "$commit" ]] || { echo "built from SudoVDA $built, but vdd_pin.rs pins $commit" >&2; exit 1; }
want=$(cd "$HERE/patches" && for p in *.patch; do echo "patch $(sha256sum "$p" | cut -d' ' -f1) $p"; done)
[[ $(grep '^patch ' "$in/BUILDINFO") == "$want" ]] || { echo "built with other patches than packaging/windows/sudovda/patches/" >&2; exit 1; }
self=$(sed -n 's/^sodium-remote //p' "$in/BUILDINFO")
run=$(sed -n 's/^run //p' "$in/BUILDINFO")

if [[ -z ${SODIUM_REMOTE_SIGNING_PASS:-} ]]; then
  read -r -s -p "Password for the signing key: " SODIUM_REMOTE_SIGNING_PASS; echo
fi
sign() {
  osslsigncode sign -pkcs12 "$PFX" -readpass <(printf '%s' "$SODIUM_REMOTE_SIGNING_PASS") -h sha256 \
    ${TS:+-ts "$TS"} -in "$in/$1" -out "$in/$1.signed" >/dev/null
  mv "$in/$1.signed" "$in/$1"
}
# The DLL first. The catalog holds the DLL's Authenticode hash, which leaves out the signature, so it should stay valid
# (Windows checks it at install; docs/reference/host-desktop.md, "Building the driver").
sign SodiumRemoteDisplay.dll
sign sodiumremotedisplay.cat

rm -rf "$OUT"
mkdir -p "$OUT"
for f in "${FILES[@]}"; do cp "$in/$f" "$OUT/"; done
cp "$in/BUILDINFO" "$OUT/"
zipsum=$(sha256sum "$ZIP" | cut -d' ' -f1)
line="signed build $(date -u +%F): artifact sha256 $zipsum, built from sodium-remote $self${run:+ ($run)}"
sed -i '/^signed build /d' "$HERE/UPSTREAM"
echo "$line" >>"$HERE/UPSTREAM"
echo "artifact zip SHA-256: $zipsum (compare with the workflow run's digest)"
echo "signed into $OUT; recorded in $HERE/UPSTREAM. Next: packaging/windows/sudovda/pin.sh --write"
