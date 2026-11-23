#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# The gate for "one C runtime": no .exe or .dll in the Windows package may
# import msvcrt.dll. Our toolchain is UCRT; Qt's official mingw build is msvcrt, so a package that
# still carries it fails here until Qt comes from build-qt-ucrt.sh.
#
#   check-no-msvcrt.sh [DIR]      DIR defaults to remote/build/win-app/SodiumRemote
#   check-no-msvcrt.sh --selftest
#
# Runs `objdump -p` over every .exe and .dll under DIR (any case) and lists each file with a
# `DLL Name: msvcrt.dll` import (any case). Exit 0: none do. Exit 1: some do, or a file could
# not be read. Exit 2: nothing to check or a bad call. --selftest builds two tiny programs in
# remote/build/crt-selftest (one on UCRT, one on msvcrt) and checks that the gate passes the
# first and fails only the second.
# Needs binutils for PE: x86_64-w64-mingw32-objdump (or OBJDUMP=...), and for --selftest x86_64-w64-mingw32-gcc.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMOTE=$(cd "$HERE/../.." && pwd)
OBJDUMP=${OBJDUMP:-}
if [[ -z $OBJDUMP ]]; then
  if command -v x86_64-w64-mingw32-objdump >/dev/null; then OBJDUMP=x86_64-w64-mingw32-objdump
  elif command -v objdump >/dev/null; then OBJDUMP=objdump
  else echo "check-no-msvcrt: no objdump (install mingw-w64 binutils)" >&2; exit 2; fi
fi

# check DIR: prints the offenders; returns 0 (none), 1 (some or unreadable), 2 (no files).
check() {
  local dir=$1 n=0 bad=0 f out
  [[ -d $dir ]] || { echo "check-no-msvcrt: no such directory: $dir" >&2; return 2; }
  while IFS= read -r -d '' f; do
    n=$((n + 1))
    if ! out=$("$OBJDUMP" -p "$f" 2>/dev/null); then
      echo "UNREADABLE  $f" >&2
      bad=$((bad + 1))
      continue
    fi
    if grep -qiE '^[[:space:]]*DLL Name:[[:space:]]*msvcrt(\.dll)?[[:space:]]*$' <<<"$out"; then
      echo "msvcrt      $f"
      bad=$((bad + 1))
    fi
  done < <(find "$dir" -type f \( -iname '*.exe' -o -iname '*.dll' \) -print0 | sort -z)
  if [[ $n -eq 0 ]]; then echo "check-no-msvcrt: no .exe or .dll under $dir" >&2; return 2; fi
  if [[ $bad -gt 0 ]]; then
    echo "check-no-msvcrt: $bad of $n files import msvcrt.dll or could not be read" >&2
    return 1
  fi
  echo "check-no-msvcrt: $n files, none import msvcrt.dll"
}

selftest() {
  command -v x86_64-w64-mingw32-gcc >/dev/null || { echo "selftest needs x86_64-w64-mingw32-gcc" >&2; exit 2; }
  local d=$REMOTE/build/crt-selftest
  rm -rf "$d"
  mkdir -p "$d/ucrt" "$d/msvcrt"
  printf '#include <stdio.h>\nint main(void) { puts("ucrt"); return 0; }\n' >"$d/hello.c"
  # A normal program: UCRT (api-ms-win-crt-*.dll).
  x86_64-w64-mingw32-gcc -mcrtdll=ucrt "$d/hello.c" -o "$d/ucrt/hello.exe"
  # Arch's mingw-w64 is UCRT only (even -mcrtdll=msvcrt still links its UCRT startup), so the msvcrt
  # one is made by hand: no startup files, the msvcrt import library libmsvcrt-os.a.
  printf 'int puts(const char *);\nint mainCRTStartup(void) { puts("msvcrt"); return 0; }\n' >"$d/old.c"
  x86_64-w64-mingw32-gcc -nostdlib -nostartfiles "$d/old.c" -o "$d/msvcrt/old.exe" -lmsvcrt-os -lkernel32
  local r
  r=0; check "$d/ucrt" >/dev/null 2>&1 || r=$?
  [[ $r -eq 0 ]] || { echo "selftest FAILED: the UCRT program was not passed (exit $r)" >&2; exit 1; }
  r=0; check "$d/msvcrt" >/dev/null 2>&1 || r=$?
  [[ $r -eq 1 ]] || { echo "selftest FAILED: the msvcrt program was not caught (exit $r)" >&2; exit 1; }
  r=0; check "$d" >/dev/null 2>&1 || r=$?
  [[ $r -eq 1 ]] || { echo "selftest FAILED: the mixed directory was not failed (exit $r)" >&2; exit 1; }
  echo "selftest ok: UCRT passes, msvcrt fails"
}

case ${1:-} in
  --selftest) selftest ;;
  -h | --help) sed -n '2,13p' "${BASH_SOURCE[0]}" ;;
  *) check "${1:-$REMOTE/build/win-app/SodiumRemote}" ;;
esac
