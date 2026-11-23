#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Builds Sodium Remote's Windows dependencies from source with mingw-w64, on Linux, into
# remote/build/win-deps/{include,lib,bin}.
#
#   remote/packaging/windows/build-deps.sh [step ...]
#
# Steps (default: all, in order): ffnvcodec amf libvpl openh264-headers opus ffmpeg licenses
# Idempotent: a step whose stamp file exists is skipped (FORCE=1 redoes every step;
# a changed version below changes the stamp). Every source archive is pinned by SHA-256
# and checked after download and before every unpack.
# Needs: x86_64-w64-mingw32-{gcc,g++,ar,...}, nasm, cmake, make, pkgconf, curl, tar, xz.
# Jobs: JOBS (default 12). Never build in /tmp (a RAM disk): everything stays in build/.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMOTE=$(cd "$HERE/../.." && pwd)
PREFIX=${WIN_DEPS:-$REMOTE/build/win-deps}
SRC=$PREFIX/src          # downloaded archives
WORK=$PREFIX/work        # unpacked sources and build trees (deletable)
STAMP=$PREFIX/.stamps
JOBS=${JOBS:-12}
TRIPLET=x86_64-w64-mingw32

# ---- pinned sources: official release locations only ----
FFMPEG_VER=9.0.2
FFMPEG_URL=https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VER.tar.xz
FFMPEG_SHA=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e

NVCH_VER=n13.1.15.0     # FFmpeg/nv-codec-headers (MIT), headers only
NVCH_URL=https://github.com/FFmpeg/nv-codec-headers/archive/refs/tags/$NVCH_VER.tar.gz
NVCH_SHA=2255bc74d038b95aa4be30f5f66322c2176acbdb90ada1851db6993536fbeaf7

AMF_VER=1.5.3           # GPUOpen AMF (MIT), the release's headers-only archive
AMF_URL=https://github.com/GPUOpen-LibrariesAndSDKs/AMF/releases/download/v$AMF_VER/AMF-headers-v$AMF_VER.tar.gz
AMF_SHA=0f521ad98041f8001147cded4a660127f18b1c27a801a5cd691bda1fa48381be

VPL_VER=2.17.0         # intel/libvpl dispatcher (MIT)
VPL_URL=https://github.com/intel/libvpl/archive/refs/tags/v$VPL_VER.tar.gz
VPL_SHA=4de3e2faf1e8307fb282e4a43f443191810f6a6b0a484fffa7995ba1c814c6ec

# OpenH264 is not built or shipped: only its headers (BSD-2) are taken from
# this source release, to compile against; the codec itself is Cisco's prebuilt DLL, downloaded at
# run time (host-desktop/src/openh264_pin.rs, pin-openh264.sh). Keep this version equal to the pin's.
OH264_VER=2.7.0        # cisco/openh264 (BSD-2), headers only
OH264_URL=https://github.com/cisco/openh264/archive/refs/tags/v$OH264_VER.tar.gz
OH264_SHA=5178775c8b440073e70920e97d195efff721c0e56d0a78cc82c919e95d54924d

OPUS_VER=1.6.1         # xiph opus (BSD-3)
OPUS_URL=https://downloads.xiph.org/releases/opus/opus-$OPUS_VER.tar.gz
OPUS_SHA=6ffcb593207be92584df15b32466ed64bbec99109f007c82205f0194572411a1

# ---- environment: cross tools, and pkg-config looking only at our prefix ----
export PKG_CONFIG_LIBDIR=$PREFIX/lib/pkgconfig
export PKG_CONFIG_PATH=
export PKG_CONFIG_ALLOW_CROSS=1
export CC=$TRIPLET-gcc CXX=$TRIPLET-g++ AR=$TRIPLET-ar RANLIB=$TRIPLET-ranlib
export STRIP=$TRIPLET-strip WINDRES=$TRIPLET-windres DLLTOOL=$TRIPLET-dlltool NM=$TRIPLET-nm

log() { printf '\n==== %s\n' "$*" >&2; }
need() { command -v "$1" >/dev/null || { echo "missing tool: $1" >&2; exit 1; }; }
for t in $TRIPLET-gcc $TRIPLET-g++ nasm cmake make pkg-config curl tar xz sha256sum; do need "$t"; done
mkdir -p "$SRC" "$WORK" "$STAMP" "$PREFIX"/{include,lib,bin}

fetch() { # name url sha -> $SRC/<basename>, verified
  local url=$2 sha=$3 f="$SRC/$(basename "$2")"
  if [[ -f $f ]] && echo "$sha  $f" | sha256sum -c --status; then echo "$f"; return; fi
  rm -f "$f"
  log "download $url"
  curl -fL --retry 3 -o "$f.part" "$url" >&2
  mv "$f.part" "$f"
  echo "$sha  $f" | sha256sum -c --status || { echo "SHA-256 mismatch: $f" >&2; rm -f "$f"; exit 1; }
  echo "$f"
}
unpack() { # archive dir-name -> $WORK/<dir-name>, fresh
  local ar=$1 dir=$WORK/$2
  rm -rf "$dir"
  tar -C "$WORK" -xf "$ar"
  [[ -d $dir ]] || { echo "unexpected archive layout: $ar" >&2; exit 1; }
  echo "$dir"
}
step() { # name stamp-key body-function
  local stamp="$STAMP/$1-$2"
  if [[ -f $stamp && -z ${FORCE:-} ]]; then echo "skip $1 ($2 already built)"; return; fi
  log "$1 $2"
  "$3"
  touch "$stamp"
}

do_ffnvcodec() {
  local d; d=$(unpack "$(fetch nvch "$NVCH_URL" "$NVCH_SHA")" nv-codec-headers-$NVCH_VER)
  make -C "$d" PREFIX="$PREFIX" install >&2
}

do_amf() {
  local d; d=$(unpack "$(fetch amf "$AMF_URL" "$AMF_SHA")" amf-headers-v$AMF_VER)
  rm -rf "$PREFIX/include/AMF"
  cp -r "$d/AMF" "$PREFIX/include/AMF"
}

do_libvpl() {
  local d; d=$(unpack "$(fetch vpl "$VPL_URL" "$VPL_SHA")" libvpl-$VPL_VER)
  cat >"$WORK/mingw-toolchain.cmake" <<EOF
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)
set(CMAKE_C_COMPILER $TRIPLET-gcc)
set(CMAKE_CXX_COMPILER $TRIPLET-g++)
set(CMAKE_RC_COMPILER $TRIPLET-windres)
set(CMAKE_FIND_ROOT_PATH /usr/$TRIPLET)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
EOF
  # Static dispatcher: the GPU runtime is found by the dispatcher in the driver at run time.
  cmake -S "$d" -B "$d/_b" -DCMAKE_TOOLCHAIN_FILE="$WORK/mingw-toolchain.cmake" \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$PREFIX" -DCMAKE_INSTALL_LIBDIR=lib \
    -DBUILD_SHARED_LIBS=OFF -DBUILD_TESTS=OFF -DBUILD_EXAMPLES=OFF -DBUILD_PREVIEW=OFF \
    -DBUILD_TOOLS=OFF -DINSTALL_EXAMPLE_CODE=OFF -DBUILD_DISPATCHER_ONEVPL_EXPERIMENTAL=OFF \
    -DCMAKE_CXX_FLAGS="-static-libgcc -static-libstdc++" >&2
  cmake --build "$d/_b" -j"$JOBS" >&2
  cmake --install "$d/_b" >&2
  # FFmpeg links the static C++ dispatcher: let pkg-config say so.
  sed -i 's/^Libs.private:.*/Libs.private: -lstdc++/' "$PREFIX/lib/pkgconfig/vpl.pc"
}

do_openh264_headers() {
  local d; d=$(unpack "$(fetch oh264 "$OH264_URL" "$OH264_SHA")" openh264-$OH264_VER)
  # <wels/codec_api.h> and its three siblings; no library is built (Cisco's own DLL is fetched at run time).
  rm -rf "$PREFIX/include/wels"
  mkdir -p "$PREFIX/include/wels"
  cp "$d"/codec/api/wels/codec_*.h "$PREFIX/include/wels/"
  ls "$PREFIX/include/wels/codec_api.h" >/dev/null
}

do_opus() {
  local d; d=$(unpack "$(fetch opus "$OPUS_URL" "$OPUS_SHA")" opus-$OPUS_VER)
  ( cd "$d" && ./configure --host=$TRIPLET --prefix="$PREFIX" --disable-shared --enable-static \
      --disable-doc --disable-extra-programs >&2 && make -j"$JOBS" >&2 && make install >&2 )
}

do_ffmpeg() {
  local d; d=$(unpack "$(fetch ffmpeg "$FFMPEG_URL" "$FFMPEG_SHA")" ffmpeg-$FFMPEG_VER)
  mkdir -p "$d/_b"
  # Lean on purpose: libavcodec, libavutil, libswscale only; no GPL, no nonfree, no programs.
  # Decoders: h264, hevc, av1 (AV1 through hwaccels only; no dav1d yet).
  # Encoders: Media Foundation, NVENC, AMF, QSV (libvpl). No libopenh264: host-desktop loads Cisco's DLL itself.
  local hwacc=""
  for c in h264 hevc av1; do for a in d3d11va d3d11va2 dxva2 nvdec; do hwacc+="--enable-hwaccel=${c}_$a "; done; done
  local enc=""
  for e in h264_mf hevc_mf av1_mf h264_nvenc hevc_nvenc av1_nvenc h264_amf hevc_amf av1_amf \
           h264_qsv hevc_qsv av1_qsv; do enc+="--enable-encoder=$e "; done
  ( cd "$d/_b" && ../configure \
      --prefix="$PREFIX" --arch=x86_64 --target-os=mingw32 --cross-prefix=$TRIPLET- \
      --pkg-config=pkg-config --pkg-config-flags=--static \
      --enable-shared --disable-static \
      --disable-gpl --disable-nonfree --disable-version3 \
      --disable-programs --disable-doc --disable-debug \
      --disable-avdevice --disable-avformat --disable-avfilter --disable-swresample \
      --disable-network --disable-autodetect \
      --disable-everything \
      --enable-avcodec --enable-avutil --enable-swscale \
      --enable-d3d11va --enable-dxva2 --enable-mediafoundation \
      --enable-nvenc --enable-ffnvcodec --enable-nvdec --enable-amf --enable-libvpl \
      --enable-decoder=h264 --enable-decoder=hevc --enable-decoder=av1 \
      --enable-parser=h264 --enable-parser=hevc --enable-parser=av1 \
      --enable-bsf=extract_extradata --enable-bsf=h264_mp4toannexb --enable-bsf=hevc_mp4toannexb \
      --enable-bsf=av1_frame_merge --enable-bsf=null \
      $hwacc $enc \
      --extra-cflags="-I$PREFIX/include" --extra-ldflags="-L$PREFIX/lib -static-libgcc -static-libstdc++" \
      --optflags=-O2 --enable-runtime-cpudetect >&2 \
    && make -j"$JOBS" >&2 && make install >&2 )
  # Windows DLLs: the shared libraries land in bin/, import libraries stay in lib/.
  ls "$PREFIX"/bin/avcodec-*.dll >/dev/null
}

do_licenses() {
  local L=$HERE/licenses
  mkdir -p "$L"
  local d
  d=$WORK/ffmpeg-$FFMPEG_VER;     [[ -d $d ]] || d=$(unpack "$(fetch ffmpeg "$FFMPEG_URL" "$FFMPEG_SHA")" ffmpeg-$FFMPEG_VER)
  cp "$d/COPYING.LGPLv2.1" "$L/FFmpeg-LGPL-2.1.txt"; cp "$d/LICENSE.md" "$L/FFmpeg-LICENSE.md"
  d=$(unpack "$(fetch vpl "$VPL_URL" "$VPL_SHA")" libvpl-$VPL_VER);      cp "$d/LICENSE" "$L/libvpl-MIT.txt"
  # The OpenH264 headers are BSD-2 and compiled into host-desktop; the codec is Cisco's own binary (OpenH264-Cisco-NOTICE.txt).
  d=$(unpack "$(fetch oh264 "$OH264_URL" "$OH264_SHA")" openh264-$OH264_VER); cp "$d/LICENSE" "$L/openh264-headers-BSD-2-Clause.txt"
  d=$(unpack "$(fetch opus "$OPUS_URL" "$OPUS_SHA")" opus-$OPUS_VER);   cp "$d/COPYING" "$L/opus-BSD-3-Clause.txt"
  # nv-codec-headers and AMF carry their MIT text in each header's comment block.
  d=$(unpack "$(fetch nvch "$NVCH_URL" "$NVCH_SHA")" nv-codec-headers-$NVCH_VER)
  sed -n '1,/\*\//p' "$d/include/ffnvcodec/dynlink_loader.h" >"$L/nv-codec-headers-MIT.txt"
  d=$(unpack "$(fetch amf "$AMF_URL" "$AMF_SHA")" amf-headers-v$AMF_VER)
  sed -n '1,31p' "$d/AMF/core/Result.h" >"$L/AMF-MIT.txt"
}

main() {
  local steps=("$@")
  [[ ${#steps[@]} -gt 0 ]] || steps=(ffnvcodec amf libvpl openh264-headers opus ffmpeg licenses)
  for s in "${steps[@]}"; do
    case $s in
      ffnvcodec) step ffnvcodec "$NVCH_VER" do_ffnvcodec ;;
      amf)       step amf "$AMF_VER" do_amf ;;
      libvpl)    step libvpl "$VPL_VER" do_libvpl ;;
      openh264-headers) step openh264-headers "$OH264_VER" do_openh264_headers ;;
      opus)      step opus "$OPUS_VER" do_opus ;;
      ffmpeg)    step ffmpeg "$FFMPEG_VER" do_ffmpeg ;;
      licenses)  do_licenses ;;
      *) echo "unknown step: $s" >&2; exit 2 ;;
    esac
  done
  log "done: $PREFIX"
}
main "$@"
