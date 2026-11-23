# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Source this before `cargo build --target x86_64-pc-windows-gnu` (or cc/make for Windows):
#   . remote/packaging/windows/env.sh
# Uses the dependencies that build-deps.sh put in remote/build/win-deps.
_h=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
export WIN_DEPS=${WIN_DEPS:-$(cd "$_h/../.." && pwd)/build/win-deps}
_t=x86_64-w64-mingw32
# C/C++ for the cc crate and build scripts, per target (the host keeps its own).
export CC_x86_64_pc_windows_gnu=$_t-gcc CXX_x86_64_pc_windows_gnu=$_t-g++ AR_x86_64_pc_windows_gnu=$_t-ar
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=$_t-gcc
# pkg-config: the `pkg-config` crate reads the per-target variables; build scripts that run the
# `pkg-config` binary themselves should use $PKG_CONFIG_x86_64_pc_windows_gnu (a wrapper that
# sees only $WIN_DEPS/lib/pkgconfig).
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_x86_64_pc_windows_gnu=$_h/win-pkg-config
export PKG_CONFIG_LIBDIR_x86_64_pc_windows_gnu=$WIN_DEPS/lib/pkgconfig
export PKG_CONFIG_PATH_x86_64_pc_windows_gnu=$WIN_DEPS/lib/pkgconfig
# Header path for C code that doesn't use pkg-config.
export CFLAGS_x86_64_pc_windows_gnu="-I$WIN_DEPS/include"
# wine as the test runner: the DLLs of the deps and of mingw's runtime on its PATH.
_w() { local p=$1; printf 'Z:%s' "${p//\//\\}"; }
export WINEPATH="$(_w "$WIN_DEPS/bin");$(_w /usr/$_t/bin)"
export WINEDEBUG=${WINEDEBUG:--all}
# A private prefix (never the user's ~/.wine), no Mono/Gecko download prompts.
export WINEPREFIX=${WINEPREFIX_SODIUM_REMOTE:-$WIN_DEPS/../wine-prefix}
export WINEDLLOVERRIDES="mscoree,mshtml=d"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER:-wine}
unset -f _w; unset _h _t
