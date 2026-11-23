#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Pins Cisco's OpenH264 DLL for the Windows host. MAINTAINER / CI STEP: run it
# once on a machine with network access, then paste the printed line into
# remote/host-desktop/src/openh264_pin.rs. Until then the host fails closed: it downloads and
# loads nothing, and the app shows "failed" for software video (hardware encoders still work).
#
#   remote/packaging/windows/pin-openh264.sh [--file PATH-TO-THE.bz2]
#
# The version and the URL are read from host-desktop/src/openh264_pin.rs, the one place that holds
# them (change them there, then run this again). It downloads the `.dll.bz2` Cisco publishes
# (https://ciscobinary.openh264.org/openh264-<version>-win64.dll.bz2, the Firefox pattern), checks
# that it is a bzip2 file holding a Windows DLL, and prints its SHA-256. With --file it
# checks a copy you already have instead of fetching. Nothing is installed or changed.
# Needs: curl (unless --file), bzip2, sha256sum. The download goes into remote/build/ (never /tmp).
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMOTE=$(cd "$HERE/../.." && pwd)
PIN=$REMOTE/host-desktop/src/openh264_pin.rs

const() { sed -n "s/^pub const $1: &str = \"\\(.*\\)\";.*/\\1/p" "$PIN" | head -n1; }
VERSION=$(const VERSION)
URL=$(const URL)
DLL=$(const DLL_NAME)
NOTICE=$(const NOTICE)
[[ -n $VERSION && -n $URL && -n $DLL ]] || { echo "cannot read VERSION, URL and DLL_NAME from $PIN" >&2; exit 1; }

FILE=
if [[ ${1:-} == --file ]]; then
  FILE=${2:?--file needs a path}
  [[ -f $FILE ]] || { echo "no such file: $FILE" >&2; exit 1; }
elif [[ $# -gt 0 ]]; then
  echo "usage: pin-openh264.sh [--file PATH]" >&2
  exit 2
fi
for t in bzip2 sha256sum; do command -v "$t" >/dev/null || { echo "missing tool: $t" >&2; exit 1; }; done

if [[ -z $FILE ]]; then
  command -v curl >/dev/null || { echo "missing tool: curl" >&2; exit 1; }
  DL=$REMOTE/build/openh264-pin
  mkdir -p "$DL"
  FILE=$DL/$(basename "$URL")
  echo "downloading $URL" >&2
  curl -fL --retry 3 -o "$FILE.part" "$URL"
  mv "$FILE.part" "$FILE"
fi

bzip2 -t "$FILE" || { echo "not a valid bzip2 file: $FILE" >&2; exit 1; }
# A Windows DLL starts with "MZ".
head2=$(bzip2 -dc "$FILE" 2>/dev/null | head -c 2 || true)
[[ $head2 == MZ ]] || { echo "the unpacked file is not a Windows executable (no MZ header)" >&2; exit 1; }
SUM=$(sha256sum "$FILE" | cut -d' ' -f1)
DLLSUM=$(bzip2 -dc "$FILE" | sha256sum | cut -d' ' -f1)
SIZE=$(stat -c %s "$FILE")

cat <<EOF
OpenH264 $VERSION
  file    $FILE ($SIZE bytes, unpacks to $DLL)
  url     $URL
  SHA-256 $SUM (the .bz2)
  SHA-256 $DLLSUM (the unpacked DLL, checked at every load)

Paste into remote/host-desktop/src/openh264_pin.rs (replace the placeholder):

pub const SHA256: &str = "$SUM";
pub const DLL_SHA256: &str = "$DLLSUM";

Then rebuild. Also check that Cisco's wording is still: $NOTICE
EOF
