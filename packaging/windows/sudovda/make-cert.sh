#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Makes Sodium Remote's self-signed code-signing certificate for the matched display's driver (SudoVDA, built and signed
# by build-sign.ps1). Run once, by the maintainer, with the go-ahead to make the real key. Nothing here is uploaded.
#
#   packaging/windows/sudovda/make-cert.sh [--out DIR]
#
# - The private key stays in $SODIUM_REMOTE_SIGNING_DIR (default ~/.local/share/sodium-remote-signing/, 0700): key.pem
#   (encrypted) and signing.pfx (for signtool in the build VM), both 0600, both locked with the password in
#   $SODIUM_REMOTE_SIGNING_PASS (asked for when unset). Never in the repository.
# - The public certificate goes to DIR (default: this folder) as sodium-remote-driver-signing.cer (DER), the file the app
#   imports into the local machine's Root and TrustedPublisher stores when "Match the display to the connecting computer"
#   is turned on. Commit it; its SHA-256 and SHA-1 thumbprint are printed for pin.sh and vdd_pin.rs.
# The certificate: RSA 3072, SHA-256, code signing only (key usage digitalSignature, extended key usage codeSigning), valid
# 10 years, CN "Sodium Remote Driver Signing (self-signed)". It refuses to replace a key that exists.
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
OUT=$HERE
while [[ $# -gt 0 ]]; do
  case $1 in
    --out) OUT=${2:?--out needs a folder}; shift 2 ;;
    *) echo "usage: make-cert.sh [--out DIR]" >&2; exit 2 ;;
  esac
done
DIR=${SODIUM_REMOTE_SIGNING_DIR:-$HOME/.local/share/sodium-remote-signing}
CN="Sodium Remote Driver Signing (self-signed)"
CER=sodium-remote-driver-signing.cer
command -v openssl >/dev/null || { echo "missing tool: openssl" >&2; exit 1; }
case $(cd "$(dirname "$DIR")" 2>/dev/null && pwd)/ in
  "$(cd "$HERE/../../.." && pwd)"/*) echo "refusing: $DIR is inside the repository" >&2; exit 1 ;;
esac
[[ -e $DIR/key.pem || -e $DIR/signing.pfx ]] && { echo "refusing: a signing key already exists in $DIR" >&2; exit 1; }
if [[ -z ${SODIUM_REMOTE_SIGNING_PASS:-} ]]; then
  read -r -s -p "Password for the signing key: " p1; echo
  read -r -s -p "Again: " p2; echo
  [[ $p1 == "$p2" && ${#p1} -ge 12 ]] || { echo "the passwords differ or are shorter than 12 characters" >&2; exit 1; }
  export SODIUM_REMOTE_SIGNING_PASS=$p1
fi

umask 077
mkdir -p "$DIR"
chmod 700 "$DIR"
cfg=$(mktemp "$DIR/openssl.XXXXXX.cnf")
trap 'rm -f "$cfg"' EXIT
cat >"$cfg" <<CNF
[req]
distinguished_name = dn
prompt = no
x509_extensions = sign
[dn]
CN = $CN
[sign]
keyUsage = critical, digitalSignature
extendedKeyUsage = codeSigning
subjectKeyIdentifier = hash
CNF
openssl req -x509 -newkey rsa:3072 -sha256 -days 3650 -config "$cfg" \
  -keyout "$DIR/key.pem" -passout env:SODIUM_REMOTE_SIGNING_PASS -out "$DIR/cert.pem" 2>/dev/null
openssl pkcs12 -export -inkey "$DIR/key.pem" -passin env:SODIUM_REMOTE_SIGNING_PASS -in "$DIR/cert.pem" \
  -name "$CN" -keypbe AES-256-CBC -certpbe AES-256-CBC -macalg sha256 \
  -out "$DIR/signing.pfx" -passout env:SODIUM_REMOTE_SIGNING_PASS
chmod 600 "$DIR/key.pem" "$DIR/cert.pem" "$DIR/signing.pfx"
mkdir -p "$OUT"
openssl x509 -in "$DIR/cert.pem" -outform DER -out "$OUT/$CER"
chmod 644 "$OUT/$CER"

openssl x509 -in "$DIR/cert.pem" -noout -subject -dates -ext keyUsage,extendedKeyUsage
echo "key and PFX: $DIR (0600)"
echo "certificate: $OUT/$CER"
echo "CER_SHA256: $(sha256sum "$OUT/$CER" | cut -d' ' -f1)"
echo "CER_SHA1:   $(openssl x509 -in "$DIR/cert.pem" -noout -fingerprint -sha1 | sed 's/.*=//; s/://g' | tr 'A-F' 'a-f')"
