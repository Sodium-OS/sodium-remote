#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Sodium Remote for Mac: builds the one static library (host-desktop with remote-core's
# surface: this Mac as a client and as a host), generates the UniFFI Swift bindings for
# both, builds the Swift package and assembles an ad-hoc-signed "Sodium Remote.app".
# A release has the bundle ID com.sodiumos.remote (Sources/RemoteKit/Brand.swift and remote/core/src/brand.rs
# hold the same names). Every test build uses --test-id (below), so it can never share data, Keychain items,
# permissions or a LaunchServices identity with an installed release.
#
#   ./build-app.sh                 release build → build/Sodium Remote.app
#   ./build-app.sh --debug         debug build (faster to compile, slower to run)
#   ./build-app.sh --skip-rust     reuse Generated/ (Swift-only changes)
#   ./build-app.sh --test          also run `swift test` (RemoteKit's tests)
#   ./build-app.sh --test-id       a TEST build: bundle ID com.sodiumos.remote.dev, name "Sodium Remote (test)"
#                                  (not translated), output build/Sodium Remote (test).app; the app then keeps
#                                  its data in ~/Library/Application Support/com.sodiumos.remote.dev (and its
#                                  Host folder), its Keychain items under that ID and its logs in
#                                  ~/Library/Logs/com.sodiumos.remote.dev
#   ./build-app.sh --install       refuses to run in this build (see the message)
#
# Environment:
#   REMOTE_SIGN_IDENTITY   a code-signing identity (default "-": ad-hoc). A
#                          self-signed identity keeps macOS permissions and the
#                          Keychain's "Always Allow" across rebuilds (BUILD.md).
#   REMOTE_NOTICES         the cargo-about output for this app's library, bundled as part of the notices file
#                          (default build/THIRD-PARTY-NOTICES-mac.md; made by the line in BUILD.md). A release
#                          build needs it; a build without it warns and ships the acknowledgements only.
#
# Everything this writes is ignored by git: Generated/, .build/, build/.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
REMOTE=$(cd "$HERE/.." && pwd)
GEN="$HERE/Generated"
OUT="$HERE/build"
CONFIG=release
SKIP_RUST=0
RUN_TESTS=0
INSTALL=0
TEST_ID=0

for a in "$@"; do
  case "$a" in
    --debug) CONFIG=debug ;;
    --skip-rust) SKIP_RUST=1 ;;
    --test) RUN_TESTS=1 ;;
    --install) INSTALL=1 ;;
    --test-id) TEST_ID=1 ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

if [ "$INSTALL" = 1 ]; then
  echo "build-app.sh: --install is not available in this build. Two apps with one bundle ID confuse" >&2
  echo "LaunchServices, and an installed release must never be replaced or share data with a test" >&2
  echo "build. Run the app from build/ instead (or drag the built app to Applications yourself)." >&2
  exit 2
fi
if [ "$TEST_ID" = 1 ]; then
  BUNDLE_ID=com.sodiumos.remote.dev
  APP_NAME="Sodium Remote (test)"
else
  BUNDLE_ID=com.sodiumos.remote
  APP_NAME="Sodium Remote"
fi

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { echo "build-app.sh: $*" >&2; exit 1; }

[ "$(uname -s)" = Darwin ] || die "run this on the Mac (it needs the macOS SDK)"
[ "$(uname -m)" = arm64 ] || die "this builds for Apple silicon (arm64) only"
command -v swift >/dev/null || die "no swift: install the Command Line Tools (xcode-select --install)"
command -v cargo >/dev/null || die "no cargo: install Rust with rustup (BUILD.md)"

# Link against the same macOS as the Swift package's minimum.
export MACOSX_DEPLOYMENT_TARGET=14.0

if [ "$SKIP_RUST" = 0 ]; then
  say "libremote_ffi for aarch64-apple-darwin (host-desktop + remote-core), and the Swift bindings of both"
  rustup target list --installed 2>/dev/null | grep -qx aarch64-apple-darwin || rustup target add aarch64-apple-darwin
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  # host-desktop's script: libremote_ffi.a, both crates' Swift
  # files and headers, one module.modulemap listing both headers, native-static-libs.txt.
  # The bindings come from the compiled library's metadata, so they always match it.
  "$REMOTE/host-desktop/build-swift.sh" mac "$tmp"
  rm -rf "$GEN"
  mkdir -p "$GEN/lib" "$GEN/RemoteCoreFFI" "$GEN/RemoteCore"
  cp "$tmp/libremote_ffi.a" "$GEN/lib/"
  cp "$tmp/RemoteCoreFFI.h" "$tmp/SodiumRemoteHostFFI.h" "$tmp/module.modulemap" "$GEN/RemoteCoreFFI/"
  # Both Swift files in one module: SodiumRemoteHost.swift uses core's `Os` converter.
  cp "$tmp/RemoteCore.swift" "$tmp/SodiumRemoteHost.swift" "$GEN/RemoteCore/"
  cp "$tmp/native-static-libs.txt" "$GEN/" 2>/dev/null || true
fi
[ -f "$GEN/lib/libremote_ffi.a" ] || die "Generated/ is missing: run without --skip-rust once"

say "swift build ($CONFIG)"
cd "$HERE"
swift build -c "$CONFIG" --product Remote
swift build -c "$CONFIG" --product s6-probe
BIN=$(swift build -c "$CONFIG" --show-bin-path)

if [ "$RUN_TESTS" = 1 ]; then
  say "swift test"
  swift test
fi

say "$APP_NAME.app ($BUNDLE_ID)"
APP="$OUT/$APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN/Remote" "$APP/Contents/MacOS/Remote"
build_no=$(git -C "$HERE" rev-list --count HEAD 2>/dev/null || echo 1)
sed -e "s/__BUILD__/$build_no/" -e "s/__BUNDLE_ID__/$BUNDLE_ID/" -e "s/__NAME__/$APP_NAME/" "$HERE/App/Info.plist" > "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null
# The strings: Localizable.xcstrings → <language>.lproj/Localizable.strings (the Command Line Tools have no
# xcstringstool; SwiftUI's Text("…") and String(localized:) read these from the main bundle).
python3 "$HERE/Tools/check-xcstrings-languages.py" "$HERE/App/Localizable.xcstrings"
python3 "$HERE/Tools/xcstrings-to-lproj.py" "$HERE/App/Localizable.xcstrings" "$APP/Contents/Resources"
# About › Third-party notices › Open the notices file: the acknowledgements (libopus and the others by hand) and
# cargo-about's list of the Rust libraries, as one text file.
NOTICES_IN=${REMOTE_NOTICES:-$REMOTE/build/THIRD-PARTY-NOTICES-mac.md}
{
  cat "$HERE/App/Acknowledgements.md"
  if [ -f "$NOTICES_IN" ]; then
    printf '\n\n'
    cat "$NOTICES_IN"
  else
    echo "build-app.sh: warning: $NOTICES_IN not found; the notices file has the acknowledgements only (a release needs cargo-about's list, BUILD.md)" >&2
  fi
} > "$APP/Contents/Resources/THIRD-PARTY-NOTICES.txt"
cp "$BIN/s6-probe" "$OUT/s6-probe"

say "codesign (${REMOTE_SIGN_IDENTITY:--})"
codesign --force --sign "${REMOTE_SIGN_IDENTITY:--}" --identifier "$BUNDLE_ID" --timestamp=none "$OUT/s6-probe"
codesign --force --deep --sign "${REMOTE_SIGN_IDENTITY:--}" --identifier "$BUNDLE_ID" --timestamp=none "$APP"
codesign --verify --verbose=1 "$APP"

say "done"
echo "  app:    $APP"
echo "  run:    open \"$APP\"    (or \"$APP/Contents/MacOS/Remote\" to see its log here)"
echo "  S6:     $OUT/s6-probe Tools/samples/yuv444.hevc"
