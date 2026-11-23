#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# sodium-remote-host on the Mac, over SSH or from a terminal.
# A process started from SSH has no window server and its privacy grants would be
# sshd's, so the host runs as a LaunchAgent in the logged-in user's GUI session instead.
# Everything stays under build/mac-host in this checkout: use a separate checkout for testing, so an
# installed Sodium Remote is never touched.
#
#   host.sh build              Opus static lib, release build, signed (see SIGN below)
#   host.sh start [ARGS...]    run the host in the GUI session (default ARGS: --pair)
#   host.sh stop
#   host.sh log                follow its output
#   host.sh run ARGS...        one-shot in the GUI session, output here
#                              (e.g. --permissions, --ask-permissions, --list)
#
# SIGN: the code-signing identity (default "Sodium Remote Dev" when the keychain has it,
# else ad hoc). macOS ties Screen Recording and Accessibility to the signature: an ad hoc
# signature changes with every build, so every build would need granting again.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
WS=$(cd "$HERE/../../.." && pwd)
OUT=$WS/build/mac-host
BIN=$WS/target/release/sodium-remote-host
LABEL=com.sodiumos.remote.host-test
DOMAIN=gui/$(id -u)
PLIST=$OUT/$LABEL.plist

[ "$(uname)" = Darwin ] || { echo "host.sh: run this on the Mac" >&2; exit 1; }
mkdir -p "$OUT/data"

sign() {
  local id=${SIGN:-}
  if [ -z "$id" ]; then
    if security find-identity -v -p codesigning | grep -q '"Sodium Remote Dev"'; then id="Sodium Remote Dev"; else id=-; fi
  fi
  [ "$id" = - ] && echo "host.sh: ad hoc signature: Screen Recording and Accessibility need granting again after each build" >&2
  codesign --force --sign "$id" --identifier "$LABEL" "$BIN"
}

agent() { # ProgramArguments..., as a LaunchAgent in the GUI session
  launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
  {
    echo '<?xml version="1.0" encoding="UTF-8"?>'
    echo '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
    echo '<plist version="1.0"><dict>'
    echo "<key>Label</key><string>$LABEL</string>"
    echo '<key>ProgramArguments</key><array>'
    for a in "$@"; do printf '<string>%s</string>\n' "$(printf '%s' "$a" | sed 's/&/\&amp;/g; s/</\&lt;/g')"; done
    echo '</array>'
    echo '<key>RunAtLoad</key><true/>'
    echo '<key>LimitLoadToSessionType</key><string>Aqua</string>'
    echo '<key>ProcessType</key><string>Interactive</string>'
    echo "<key>StandardOutPath</key><string>$OUT/host.out</string>"
    echo "<key>StandardErrorPath</key><string>$OUT/host.log</string>"
    echo '<key>EnvironmentVariables</key><dict><key>SODIUM_REMOTE_HOST_DEBUG</key><string>1</string></dict>'
    echo '</dict></plist>'
  } > "$PLIST"
  : > "$OUT/host.out"
  : > "$OUT/host.log"
  launchctl bootstrap "$DOMAIN" "$PLIST"
}

case "${1:-}" in
  build)
    export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-14.0}
    "$WS/host-desktop/build-opus-mac.sh"
    cargo build --manifest-path "$WS/Cargo.toml" --release -p host-desktop --bin sodium-remote-host
    sign
    ;;
  start)
    shift
    [ $# -gt 0 ] || set -- --pair
    agent "$BIN" --data "$OUT/data" "$@"
    sleep 2
    cat "$OUT/host.log"
    [ -s "$OUT/data/pairing-code.txt" ] && echo "pairing code: $(cat "$OUT/data/pairing-code.txt")"
    ;;
  stop) launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true ;;
  log) tail -n 100 -f "$OUT/host.log" ;;
  run)
    shift
    agent "$BIN" --data "$OUT/data" "$@"
    # One-shot: wait for it to exit (launchd keeps the job loaded; boot it out after).
    for _ in $(seq 600); do
      launchctl print "$DOMAIN/$LABEL" 2>/dev/null | grep -qE 'last exit code = -?[0-9]' && break
      sleep 0.1
    done
    launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
    cat "$OUT/host.out" "$OUT/host.log"
    ;;
  *) sed -n '2,17p' "$0"; exit 2 ;;
esac
