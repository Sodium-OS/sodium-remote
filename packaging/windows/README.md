# Sodium Remote: the Windows toolchain

Everything needed to cross-compile Sodium Remote's Windows pieces from Linux with `x86_64-pc-windows-gnu` (mingw-w64), No MSVC.

## Use

```sh
rustup target add x86_64-pc-windows-gnu          # once
remote/packaging/windows/build-deps.sh           # once: sources -> remote/build/win-deps (about 5 minutes, 0.5 GB)
. remote/packaging/windows/env.sh                # in every shell that builds for Windows
cargo build --target x86_64-pc-windows-gnu -p remote-core
remote/packaging/windows/smoke/build.sh run      # builds and runs the smoke test under wine
```

`build-deps.sh` is idempotent (a finished step is skipped; `FORCE=1` redoes all; `JOBS` sets the parallelism, default 12). Every source archive is pinned by SHA-256 and checked after download and before every unpack. Sources come only from the projects' release locations. Build needs `x86_64-w64-mingw32-gcc`, `nasm`, `cmake`, `make`, `pkgconf`, `curl`.

`env.sh` sets the per-target `CC`, `CXX`, `AR` and linker, `PKG_CONFIG_ALLOW_CROSS`, the per-target `PKG_CONFIG_*_x86_64_pc_windows_gnu` variables (the `pkg-config` crate reads these), and wine (`WINEPATH` with the DLL folders, a private `WINEPREFIX` in `build/wine-prefix`, the runner for `cargo test`). Build scripts that run the `pkg-config` binary themselves use `$PKG_CONFIG_x86_64_pc_windows_gnu` (a wrapper that sees only `build/win-deps/lib/pkgconfig`, never the host's `.pc` files).

## What is built (into `remote/build/win-deps/{include,lib,bin}`)

| Component | Version | License | Form |
|---|---|---|---|
| FFmpeg (libavcodec 63, libavutil 61, libswscale 10) | 9.0.2 | LGPL 2.1+ | shared DLLs (`avcodec-63.dll` ...), import libraries in `bin/*.lib`, `.pc` files |
| nv-codec-headers (ffnvcodec) | n13.1.15.0 | MIT | headers (NVENC and NVDEC load `nvcuda.dll` and `nvEncodeAPI64.dll` at run time) |
| AMF headers | 1.5.3 | MIT | headers (the AMD driver supplies `amfrt64.dll`) |
| libvpl dispatcher | 2.17.0 | MIT | static `libvpl.a` (the Intel driver supplies the runtime) |
| OpenH264 headers | 2.7.0 | BSD-2-Clause | `include/wels/` only: no OpenH264 is built or shipped (see "OpenH264: Cisco's DLL, fetched on first need") |
| Opus | 1.6.1 | BSD-3-Clause | static `libopus.a` |

FFmpeg flags: `--disable-gpl --disable-nonfree --disable-version3 --disable-programs --disable-doc --disable-debug --disable-everything --disable-autodetect`, with `avformat`, `avfilter`, `avdevice` and `swresample` off. On: `--enable-d3d11va --enable-dxva2 --enable-mediafoundation --enable-nvenc --enable-ffnvcodec --enable-nvdec --enable-amf --enable-libvpl` (no `libopenh264`).

- **Decoders:** `h264`, `hevc`, `av1` (AV1 only through hardware: there is no dav1d yet), with parsers and the `extract_extradata`, `h264_mp4toannexb`, `hevc_mp4toannexb`, `av1_frame_merge` BSFs.
- **Hwaccels:** `{h264,hevc,av1}_{d3d11va,d3d11va2,dxva2,nvdec}`.
- **Encoders:** `{h264,hevc,av1}_{mf,nvenc,amf,qsv}`. No software encoder: host-desktop uses OpenH264 directly, from Cisco's DLL.
- **Colour conversion:** `swscale`.

The full configure line is in `avcodec_configuration()`, which the smoke test prints. `-static-libgcc -static-libstdc++` is used, so the only mingw runtime DLL the package needs is `libwinpthread-1.dll` (MIT/BSD-style license, from `/usr/x86_64-w64-mingw32/bin`; `smoke/build.sh` copies it next to the executable).

## Licenses

- FFmpeg is built without any GPL or nonfree part (`avcodec_license()` reports "LGPL version 2.1 or later") and **linked dynamically** (DLLs next to the executables, replaceable by the user), which is what the LGPL asks. Qt will be dynamically linked as well. The license texts are in `licenses/` and go into every Windows package, with the source locations above.
- **H.264 and HEVC patents are a separate matter** from copyright licenses (see `LICENSING.md`). Cisco's royalty coverage for OpenH264 applies to the binaries Cisco itself builds and distributes, so Sodium Remote no longer builds or ships OpenH264: see the next section. The hardware encoders (NVENC, AMF, QSV, Media Foundation) rely on the GPU vendor's and Windows' own licenses. `licenses/` holds Cisco's required notice (`OpenH264-Cisco-NOTICE.txt`: "OpenH264 Video Codec provided by Cisco Systems, Inc.") and the BSD-2 text of the OpenH264 headers that are compiled in (`openh264-headers-BSD-2-Clause.txt`).

## OpenH264: Cisco's DLL, fetched on first need

Windows does not ship a self-built OpenH264, because Cisco's patent cover applies only to the binaries Cisco builds. The H.264 encoder order of the Windows host is `h264_nvenc`, `h264_amf`, `h264_qsv`, `h264_mf` (the GPU's own MFT), then OpenH264.

- `build-deps.sh` takes only OpenH264's headers (`include/wels/`, step `openh264-headers`); FFmpeg is configured without `libopenh264`. FFmpeg's software H.264/HEVC/AV1 decoders stay as they were.
- `host-desktop/src/openh264_pin.rs` is the one place with the version, the URL (`https://ciscobinary.openh264.org/openh264-<version>-win64.dll.bz2`), the SHA-256 and Cisco's notice. `pin-openh264.sh` (a maintainer or CI, on a networked machine) reads version and URL from it, fetches the file, checks it and prints the line to paste. **Until the SHA-256 is pasted the pin is a placeholder and the host fails closed**: nothing is downloaded, nothing is loaded, and software video reports `failed`.
- At run time, with no hardware H.264 encoder and "software video" on (the default), the host downloads the `.bz2` with WinHTTP, checks the SHA-256, unpacks it into `%LOCALAPPDATA%\SodiumRemote\codecs\` (temporary file, then rename) and loads it by path (`LoadLibraryExW`; never linked). The setting is `settings.json` beside `paired.json`; the app follows it through `codec` events (`rh_set_software_video`, `rh_software_video`, `rh_codec_retry`; the shapes are in `client-qt/src/host.rs` and `app/src/remote_qt.h`).

## Two C runtimes: the Qt build from source and the gate

Qt's official mingw build uses msvcrt, our toolchain UCRT; the verdict above (one `libstdc++`, no msvcrt-only static pieces) made that work, but a package that imports both runtimes is what the release gate rules out. Two scripts close it:

| Script | What it does |
|---|---|
| `build-qt-ucrt.sh` | CI recipe: qtbase, qtshadertools, qtdeclarative and qtsvg (6.12.0, the modules the app and the vendored Sodium UI use) from source with the same UCRT GCC (`-mcrtdll=ucrt`), host tools from the Linux Qt at `/usr` (`QT_HOST_PATH`, same version), into `remote/build/qt-win-ucrt/6.12.0/` (use it as `QT_WIN_ROOT`). Qt is built without ICU, as Qt's own Windows builds are (`QT_ICU=1` links it to `build-icu.sh`'s ICU). Sources: the official submodule tarballs, pinned by SHA-256 in the script; with the placeholder pins it refuses to run (a maintainer or CI runs `build-qt-ucrt.sh --print-pins`), or `QT_SRC_DIR` points at an unpacked tree. Hours and several GB: not run by hand here. `d3dcompiler_47.dll` and `opengl32sw.dll` still come from `fetch-qt.sh`. |
| `check-no-msvcrt.sh [DIR]` | The gate: `objdump -p` over every `.exe` and `.dll` under `DIR` (default `remote/build/win-app/SodiumRemote`); lists each file importing `msvcrt.dll` (any case) and fails, passes when none do. `--selftest` builds a UCRT and an msvcrt program in `remote/build/crt-selftest` and checks that only the second fails (Arch's toolchain is UCRT only, so the msvcrt one is linked by hand against `libmsvcrt-os.a`). With Qt's official DLLs the gate fails; it passes once the package uses `build-qt-ucrt.sh`'s Qt. |

## Smoke test

`smoke/smoke.c` prints FFmpeg's version and configuration, lists which encoders and decoders exist, tries `h264_mf` on 10 synthetic 1280x720 frames and, when that made a stream, decodes it with the `h264` decoder, and round-trips one Opus frame. Under wine the Media Foundation encoder cannot open (no MFT): that is expected, the decode is then reported as skipped, and the exit status is still 0 when Opus works. (OpenH264 is Cisco's DLL, fetched by the host at run time; it is not part of this build.)

## Qt and Sodium UI for Windows

The shared client app is Qt/QML on Sodium UI. Sodium UI's portable build and its gallery cross-build from Linux like everything else here, with Qt's own Windows binaries:

```sh
remote/packaging/windows/build-sodiumui.sh          # Qt, ICU, Sodium UI, the gallery, staged into remote/build/win-gallery/
remote/packaging/windows/wine-gallery.sh 90 --grab=remote/build/shots/w.png   # headless wine smoke test
remote/packaging/windows/gallery-shots.sh           # English, German, Japanese, dark and light, under wine
```

| Script | What it does |
|---|---|
| `fetch-qt.sh` | Qt 6.12.0's official `mingw_64` build from `download.qt.io`'s online repository (the plain 7z archives; no installer, no helper tool): qtbase, qtdeclarative, qtsvg, qtshadertools, `d3dcompiler_47.dll` and `opengl32sw.dll`, 170 MB downloaded, 1.4 GB unpacked into `remote/build/qt-win/6.12.0/mingw_64/`. Each archive is checked against Qt's published `.sha1` and a SHA-256 pinned in the script. |
| `build-icu.sh` | ICU 78.3 (the Linux desktop's version) from the official release, SHA-256 pinned: a native build first (its tools make the data), then the mingw cross build. Shared DLLs `icuuc78.dll`, `icuin78.dll`, `icudt78.dll` (33 MB of data) into `win-deps/bin`. It passes `--build` so `configure` never runs its test programs (with wine in `binfmt_misc` it would, in `~/.wine`). About 30 minutes on 6 jobs. |
| `toolchain-mingw.cmake` | Arch's mingw-w64 GCC, Qt for Windows and `win-deps` on the search paths, the Linux Qt (`QT_HOST_PATH=/usr`, the same version, 6.12.0) for `moc`, `rcc` and `qsb`. |
| `build-sodiumui.sh` | Runs the above, configures and builds `ui/` (`SODIUMUI_PORTABLE`) and stages it. |
| `stage-sodiumui.py` | What `windeployqt` does: the DLL closure from the PE import tables (Qt, ICU, GCC's runtime), the Qt plugins (`qwindows`, image formats, SVG icons), only the QML modules the QML imports (`qmlimportscanner`; for the app also `QtQuick.Dialogs` and its `quickimpl` folder by name, with their plugin DLLs, for the file choosers in `ServerFiles.qml`), the module, fonts, icon theme, translations and licenses. |
| `wine-gallery.sh`, `gallery-shots.sh` | Run the staged gallery under wine in a headless `cage` with a private prefix (never `~/.wine`); screenshots through the gallery's own `--grab`. |

### Qt and the C runtime (the ABI verdict)

Qt's official mingw build is made by GCC 15.1 against the **msvcrt** C runtime; Arch's mingw-w64 GCC 16.2 targets **UCRT**. Cross-building works, and the two runtimes coexist because:
- everything C++ goes through one `libstdc++-6.dll`, Arch's GCC 16 build (newer than Qt's 15.1, and backward compatible), shipped with `libgcc_s_seh-1.dll` and `libwinpthread-1.dll` beside the program. Qt's DLLs load it by name, so one copy serves all;
- our code links Qt only through its DLL import libraries. The one static Qt piece built for msvcrt, `libQt6EntryPoint.a` (the `WinMain` shim of `WIN32_EXECUTABLE` targets; it needs msvcrt's `__imp___argc` and does not link against UCRT), is **not** used: a GUI program is built with `-mwindows` and a plain `main()`, which mingw's own startup runs. Use `add_executable`, not `qt_add_executable(... WIN32)`;
- the usual rule for code that mixes C runtimes: never pass a CRT-owned object (`FILE *`, a `malloc`ed block to be `free`d, `errno`) across a DLL boundary. Qt's API passes none.

Qt's own 13.1 runtime archive is not used. If a Qt module ever needs another static Qt library built for msvcrt, that part is built inside the Windows VM with Qt's bundled MinGW. For the release the whole of Qt is built on UCRT instead (see "Two C runtimes" above).

### What is in the package (`remote/build/win-gallery/`)

`sodiumui-gallery.exe`; the Qt, ICU and GCC DLLs beside it; `platforms/qwindows.dll`, `imageformats/`, `iconengines/`; `qml/` (10 Qt modules for the gallery; the app's package adds `QtQuick/Dialogs` and `QtQuick/Dialogs/quickimpl`, and `Qt6QuickDialogs2*.dll` comes in with the DLL closure); `SodiumUI/` (QML and `sodiumuinative.dll`); `gallery/`; `fonts/` (Inter, JetBrains Mono, Noto Sans CJK: OFL, texts in `licenses/`); `data/icons/SodiumOS/`; `translations/`; `d3dcompiler_47.dll` and `opengl32sw.dll`. About 195 MB, 69 MB of it fonts and 33 MB ICU data. Qt Quick draws with Direct3D 11 (its shaders go through `qsb` for HLSL and the DLL compiles them); `opengl32sw.dll` is the fallback for a machine with no usable adapter (`QSG_RHI_BACKEND=opengl QT_OPENGL=software`), which is also how the wine runs draw (wined3d on this PC's headless Mesa crashes in LLVM).

Qt is LGPL-3 here: dynamic linking, the DLLs replaceable; its license texts go into the package with the app's.

## The app

`stage-app.sh [--no-ui] [--no-zip]` builds `remote-qt` (Rust), the Qt app (`packaging/windows/app/CMakeLists.txt`, which pulls `app/`), the ten language catalogues, stages `remote/build/win-app/SodiumRemote/` and writes `remote/build/SodiumRemote-<version>-win64.zip` with the licenses in `licenses/`. `ICONS` points at another icon folder (default `app/data/icons/sodium-remote`).

VM helpers are in `remote/tools/vm/windows-app/`: `redeploy.sh` (stage and copy to the guest's `windows-app` folder, `$REMOTE_VM_GUEST_DIR`), `run-app.ps1` (starts the app in session 1 through a scheduled task; `-Lang`, `-Theme`, `-AppArgs`), `qmp.py` (keys and pointer through QMP). Hold `win.sh lock windows-app` first.
