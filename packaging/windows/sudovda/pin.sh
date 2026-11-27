#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Pins a signed build of the matched display's driver (sign.sh's output) for the Windows app: checks that the DLL and the
# catalog are signed by our certificate (this folder's sodium-remote-driver-signing.cer), then prints the SHA-256 of every
# file and the certificate's SHA-1 thumbprint as the lines of host-desktop/src/vdd_pin.rs; with --write it puts them there.
#
#   packaging/windows/sudovda/pin.sh [--write] [DIR]      DIR: the signed build (default remote/build/sudovda)
#
# Until vdd_pin.rs is pinned the app installs nothing (it fails closed). stage-app.sh ships DIR's three files and the
# certificate as drivers\sudovda\ when DIR has them. Needs openssl, sha256sum and python3.
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMOTE=$(cd "$HERE/../../.." && pwd)
PIN=$REMOTE/host-desktop/src/vdd_pin.rs
CER=$HERE/sodium-remote-driver-signing.cer
WRITE=0 DIR=$REMOTE/build/sudovda
for a in "$@"; do
  case $a in --write) WRITE=1 ;; -*) echo "usage: pin.sh [--write] [DIR]" >&2; exit 2 ;; *) DIR=$a ;; esac
done
[[ -f $CER ]] || { echo "no certificate at $CER: run make-cert.sh first" >&2; exit 1; }
for f in SodiumRemoteDisplay.inf SodiumRemoteDisplay.dll sodiumremotedisplay.cat; do [[ -f $DIR/$f ]] || { echo "missing $DIR/$f" >&2; exit 1; }; done

# The signer of the catalog (PKCS#7) and of the DLL (the PKCS#7 in its PE certificate table) must be our certificate.
ours=$(openssl x509 -inform DER -in "$CER" -outform PEM)
signers() { openssl pkcs7 -inform DER -print_certs 2>/dev/null | openssl x509 -outform PEM 2>/dev/null || true; }
pe_pkcs7() {
  python3 -I - "$1" <<'PY'
import struct, sys
b = open(sys.argv[1], "rb").read()
pe = struct.unpack_from("<I", b, 0x3C)[0]
magic = struct.unpack_from("<H", b, pe + 24)[0]
dirs = pe + 24 + (112 if magic == 0x20B else 96)
off, size = struct.unpack_from("<II", b, dirs + 4 * 8)   # IMAGE_DIRECTORY_ENTRY_SECURITY: a file offset
if not off:
    sys.exit("the DLL isn't signed")
length = struct.unpack_from("<I", b, off)[0]
sys.stdout.buffer.write(b[off + 8: off + length])
PY
}
[[ $(signers <"$DIR/sodiumremotedisplay.cat") == "$ours" ]] || { echo "sodiumremotedisplay.cat isn't signed by $CER" >&2; exit 1; }
[[ $(pe_pkcs7 "$DIR/SodiumRemoteDisplay.dll" | signers) == "$ours" ]] || { echo "SodiumRemoteDisplay.dll isn't signed by $CER" >&2; exit 1; }
tr -d '\r' <"$DIR/SodiumRemoteDisplay.inf" | grep -q '^CatalogFile *= *sodiumremotedisplay.cat' || { echo "SodiumRemoteDisplay.inf doesn't name sodiumremotedisplay.cat" >&2; exit 1; }

sum() { sha256sum "$1" | cut -d' ' -f1; }
thumb=$(openssl x509 -inform DER -in "$CER" -noout -fingerprint -sha1 | sed 's/.*=//; s/://g' | tr 'A-F' 'a-f')
lines=$(cat <<RS
pub const INF_SHA256: &str = "$(sum "$DIR/SodiumRemoteDisplay.inf")";
pub const DLL_SHA256: &str = "$(sum "$DIR/SodiumRemoteDisplay.dll")";
pub const CAT_SHA256: &str = "$(sum "$DIR/sodiumremotedisplay.cat")";
pub const CER_SHA256: &str = "$(sum "$CER")";
pub const CER_SHA1: &str = "$thumb";
RS
)
echo "$lines"
if [[ $WRITE == 1 ]]; then
  while IFS= read -r l; do
    name=$(sed 's/^pub const \([A-Z0-9_]*\):.*/\1/' <<<"$l")
    sed -i "s|^pub const $name: &str = \".*\";|${l//&/\\&}|" "$PIN"
  done <<<"$lines"
  echo "written to $PIN"
fi
