# Building for Windows

The Windows app, `SodiumRemote.exe`, is **cross-compiled from Linux** with `mingw-w64` (target `x86_64-pc-windows-gnu`). There is no MSVC build. This page takes a Linux machine from a fresh clone to `SodiumRemote-<version>-win64.zip`. The scripts are in `packaging/windows/`.

## What you need

On the Linux machine:

- Rust, with the target: `rustup target add x86_64-pc-windows-gnu`.
- `mingw-w64` (`x86_64-w64-mingw32-gcc`), `nasm`, `cmake`, `ninja`, `make`, `pkgconf`, `curl`, `zip`. On Arch Linux: `pacman -S --needed mingw-w64-gcc nasm cmake ninja make pkgconf curl zip`.
- A Linux **Qt 6.12.0** (`qt6-base qt6-declarative qt6-shadertools qt6-svg qt6-tools`), the exact version of the Windows Qt the scripts build, for `moc`, `rcc` and `qsb`. The scripts stop with a message when the two differ; when Arch moves to a newer Qt, the version in `build-qt-ucrt.sh` and `fetch-qt.sh` moves with it.
- `p7zip` (the zip and Qt's archives), `python` and `mingw-w64-binutils` (`objdump`, for the gate below).
- About 15 GB of free disk space and patience: dependencies take around five minutes, ICU around half an hour, Qt built from source a few hours.
- The app's icons are in the tree (`app/data/icons/SodiumOS`; `ICONS=` points to another folder).

## Build

```sh
git clone https://github.com/sodium-os/sodium-remote.git
cd sodium-remote

# Once: FFmpeg, Opus and the codec headers, built from pinned sources (checked by SHA-256); then ICU.
packaging/windows/build-deps.sh
packaging/windows/build-icu.sh

# Qt for Windows. Release builds use Qt built from source on UCRT, so that no file in the package needs msvcrt:
packaging/windows/fetch-qt.sh                  # Qt's own build: d3dcompiler_47.dll and opengl32sw.dll come from it
packaging/windows/build-qt-ucrt.sh             # qtbase, qtshadertools, qtdeclarative, qtsvg 6.12.0 -> build/qt-win-ucrt/6.12.0
cp build/qt-win/6.12.0/mingw_64/{d3dcompiler_47.dll,opengl32sw.dll} build/qt-win-ucrt/6.12.0/
export QT_WIN_ROOT=$PWD/build/qt-win-ucrt/6.12.0

# Everything else: Sodium UI, the Rust engine, the app, the translations, the package.
packaging/windows/stage-app.sh
packaging/windows/check-no-msvcrt.sh build/win-app/SodiumRemote
```

`build-qt-ucrt.sh` refuses to run until the SHA-256 of Qt's source archives are pinned in the script (`build-qt-ucrt.sh --print-pins` prints them from Qt's own published files; paste them in once). Without `QT_WIN_ROOT`, `stage-app.sh` uses Qt's official build (`fetch-qt.sh`), which imports msvcrt: fine for trying the app out, but `check-no-msvcrt.sh` fails on it, and a release is never made from it.

The result:

- `build/win-app/SodiumRemote/` is the staged program with its DLLs, QML, fonts, icons, translations and the license texts in `licenses/`;
- `build/SodiumRemote-<version>-win64.zip` is the same, zipped. It is portable: there is no installer, and it is not code-signed. (The release also puts `THIRD-PARTY-NOTICES.md`, made by `cargo about`, into the folder before zipping; see [releasing](../releasing.md).)

`stage-app.sh` options: `--no-ui` (reuse an earlier Sodium UI build) and `--no-zip`. `JOBS` sets the parallelism.

### The software H.264 encoder

The package carries no OpenH264. With no hardware H.264 encoder and "software video" on, the host downloads Cisco's own build on first need and checks it against the SHA-256 in `host-desktop/src/openh264_pin.rs`, and checks the unpacked DLL against its own SHA-256 every time it loads it (by full path, with only System32 searched for its imports). The download is refused if it is larger than 8 MiB, before it is hashed. That line is a placeholder in the source until a maintainer runs `packaging/windows/pin-openh264.sh` and pastes its output; the release workflow refuses to run while it is. See the README in `packaging/windows/`.

### What CI does

`.github/workflows/ci.yml` runs the same scripts in an Arch Linux container on every push and pull request, with `build/win-deps`, `build/qt-win` and `build/qt-win-ucrt` cached by the hash of the scripts that make them. While Qt's source checksums are not pinned it builds with Qt's official build and the msvcrt check only warns. `.github/workflows/release.yml` builds the zip from a tag with the checks blocking.

## What the build uses

| Component | License | How it is used |
|---|---|---|
| FFmpeg 9 (libavcodec, libavutil, libswscale) | LGPL 2.1+ | Shared DLLs next to the program. Built without any GPL or non-free part. Decoders: HEVC, H.264, AV1 (AV1 through hardware only). Encoders: through the GPU's own hardware (NVENC, AMF, Quick Sync, Media Foundation) |
| OpenH264 | BSD-2-Clause | Only its headers are used; Cisco's own DLL is fetched at run time (see above) |
| Opus | BSD-3-Clause | Static |
| Qt 6.12 | LGPL-3 | Built from source on UCRT with the same compiler, dynamic |
| ICU | | Shared DLLs |

The license texts go into every package. Video codec patents are a separate matter from these licenses: see [LICENSING.md](../../LICENSING.md#codecs-and-patents).

Notes:

- The dependency script (`build-deps.sh`) is idempotent: a finished step is skipped, `FORCE=1` redoes all. Every source archive is checked against a pinned SHA-256.
- The scripts use a private `WINEPREFIX` under `build/` (never your `~/.wine`) when they run anything under wine.
- Qt's official Windows build uses msvcrt, the mingw compiler here UCRT. A package with both is what the release avoids by building Qt on UCRT; `check-no-msvcrt.sh` is the gate. Details: `packaging/windows/README.md`.

## The command-line pieces only

If you only need the engine, the Windows test client or the desktop host (no Qt app):

```sh
rustup target add x86_64-pc-windows-gnu
packaging/windows/build-deps.sh
packaging/windows/cargo-win.sh build --release -p client-windows           # remote-client.exe
packaging/windows/cargo-win.sh build --release -p host-desktop --bins       # sodium-remote-host.exe
```

## Test it

The program can be run under Wine for smoke tests (`packaging/windows/smoke/build.sh run`), but the window, input, clipboard and audio need a real Windows machine or a virtual machine; Wine has no usable adapter or audio device for them.

On Windows: unzip, run `SodiumRemote.exe`, and see [install](../install.md) for SmartScreen and the firewall.

## Building natively on Windows

The scripts target a Linux build machine. Building the Rust crates natively with MSVC, or the Qt app on Windows, is not documented or tested yet.

## Tested on (2026-11-23)

Windows 11 Enterprise Evaluation (build 26100) in the QEMU test VM (no GPU: WARP, no hardware encoder), the zip from `stage-app.sh` unpacked into a guest folder, Qt 6.12 from Qt's own build (msvcrt; the UCRT build was not rebuilt for this run), a Linux `remote-client` and core's `connect_demo` host on the host-only bridge as the other ends. Raw logs and screenshots stay in `build/` (gitignored).

| Check | Result |
|---|---|
| App starts from the unpacked zip | Pass (after two fixes: the exe no longer links `libopenh264.dll`; `build-deps.sh` finds the OpenH264 headers in `codec/api/wels`) |
| This computer: switch, pairing code, pair from a Linux client over mDNS | Pass. The client connects, but the host refuses it ("no video codec in common") because the OpenH264 pin is a placeholder and the VM has no hardware encoder |
| Tray icon, menu, closing to the tray, Open from the tray | Pass |
| Start at sign-in (Run key written, then removed from the tray menu) | Pass |
| `sodium-remote://` handler registered as `exe -- "%1"` | Pass |
| First launch from a link shows the confirmation sheet; a second launch forwards its link (WM_COPYDATA) to the running app | Pass |
| M3: a link carrying `" --data ... --script ... --lan-only` after the quote | Pass: the process saw the options after `--`, ignored them, no data folder was created, the sheet showed only the link's server |
| Settings › Server: Import and Export file choosers from the staged zip | Pass (native dialogs; the exported file has the server URL) |
| OpenH264 with the placeholder pin | Refuses: nothing downloaded or loaded; the app shows "Couldn't download the video codec. Check this computer's internet connection." (the host log says the build is not pinned) |
| M7: a stand-in DLL matching a test pin loads; the same file with one byte appended is refused before any load | Pass (a throwaway build with a test pin; not committed) |
| L13: `d3dcompiler_47.dll` with a decoy beside the exe | Pass: the loaded module is System32's |
| Screenshots in ja_JP, zh_CN, ko_KR, de_DE (main, This computer, six Settings tabs) | Reviewed: no clipping, no missing glyphs |
| T2b-1: host on, no server configured | Pass: no rendezvous output, no outbound TCP, only the LAN listener and mDNS |

Findings, not fixed: the codec failure card blames the internet connection when the cause is a missing pin or a refused DLL; the codec notice in ja_JP says "Advanced" in English inside a translated sentence; the "firewall is blocking" row looks for the app's own named rule, so another Allow rule for the same program does not clear it.
