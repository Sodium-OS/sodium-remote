# Building on Linux

From a fresh clone to a running binary. This page builds the Rust parts (the server, the host and client programs, and the engine for the Qt app) and the Qt app.

## What you need

- A Rust toolchain, recent enough for edition 2024 (`rustup` stable; Rust 1.85 or later), a C compiler, `pkg-config`, `cmake` and `ninja`.
- Libraries and headers, found with `pkg-config`: **FFmpeg** (`libavcodec`, `libavutil`, `libswscale`), **OpenH264**, **Opus**, **PipeWire**, **Wayland** (`wayland-client`, `wayland-egl`) and **EGL**. On Arch Linux: `pacman -S --needed base-devel rustup cmake ninja ffmpeg openh264 opus pipewire wayland libglvnd libxkbcommon icu`. On other distributions install the matching `-dev` or `-devel` packages.
- For the **Qt app**: **Qt 6.8 or later** (CI uses the current Arch Qt, 6.12): Core, Gui, Network, Qml, Quick, Quick Controls, Svg, ShaderTools and the private GUI headers, ICU, and `qt6-tools` for `lrelease`. On Arch Linux: `pacman -S --needed qt6-base qt6-declarative qt6-svg qt6-shadertools qt6-tools qt6-wayland`.
- At run time, for hardware decoding: `nvidia-utils` (NVDEC) or Mesa's VA-API driver, and `dav1d` for software AV1. Without them the client decodes in software.
- For the **SodiumOS host** (`remoted`, optional): it only works on SodiumOS, with its window manager.

## Get the source

```sh
git clone https://github.com/sodium-os/sodium-remote.git
cd sodium-remote
rustup default stable
```

## Build the Rust workspace

```sh
cargo build --release
```

This builds the workspace's **default members**: the core, the relay (the server), the client engine, the Linux test client, the Windows client (it compiles on Linux, for the type check), the engine behind the Qt app and the desktop host. The SodiumOS-only crates are left out on purpose (below). The programs end up in `target/release/`:

| Program | Crate | What it is |
|---|---|---|
| `remote-client` | `remote-client` | A command-line client: pairs, connects, shows the stream in a Wayland window, sends input. Useful for testing |
| `sodium-remote-server` | `remote-rendezvous` | The rendezvous server ([self-hosting](../self-hosting.md)) |
| `sodium-remote-host` | `host-desktop` | The desktop host program. On Linux it uses the desktop's screen-sharing portal (GNOME and KDE Plasma 6 on Wayland; `--backend fake` draws a test screen instead). See [the Linux host page](../hosts/linux.md) |

To build just one: `cargo build --release -p remote-client`. The SodiumOS host (`remoted`), its Wayland clipboard helper (`remote-wlclip`) and the input-method add-on are not default members; build them deliberately:

```sh
cargo build --release -p remoted -p remote-wlclip        # SodiumOS only
cmake -S fcitx5-remote -B build/fcitx5-remote -G Ninja && cmake --build build/fcitx5-remote   # needs fcitx5's development files
```

Run the tests:

```sh
cargo test
```

## Build the Qt app

The Qt app, "Sodium Remote" (`sodium-remote`), is in `app/`. It links the Rust engine (`remote-qt`, built by cargo) and Sodium UI (vendored in `sodium-ui/`).

```sh
cmake -S app -B build-app -G Ninja -DCMAKE_BUILD_TYPE=Release
cmake --build build-app
build-app/bin/sodium-remote
```

CMake builds the engine with `cargo build --release -p remote-qt` and the vendored Sodium UI (`sodium-ui/`) as part of the build. Run the app's tests and the offscreen smoke test, which opens About and Settings in a private headless Wayland session (`cage` on the headless backend, software rendering; it needs `cage`, `dbus` and `mesa`) and checks the pictures and the saved switch:

```sh
ctest --test-dir build-app --output-on-failure
app/tests/smoke.sh build-app
```

The app needs a **Wayland** session to stream. Useful options: `--data DIR` (use a different data directory), `--lan-only` (don't use a server).

The translations are not built by CMake: `i18n/i18n.sh release` makes `build/i18n/qm/*.qm` from the catalogs in `i18n/ts/` (it needs `lrelease6`), and the app finds them there when run from the build tree ([translating](../translating.md)). Without them the app shows its source text, in US English.

## Arch Linux package

`packaging/arch/PKGBUILD` builds two packages with the system's Qt, FFmpeg and PipeWire: `sodium-remote` (the app, its desktop entry, icon, translations and notices) and `sodium-remote-server` (the rendezvous server, a systemd unit, `/etc/sodium-remote-server/server.env`). It builds from the release's source tarball, which holds the tracked files plus the third-party notices:

```sh
# from a release: download PKGBUILD and sodium-remote-<version>.tar.gz into one folder, then
makepkg -si

# from a checkout:
packaging/arch/make-source-tarball.sh 0.1.0 packaging/arch      # needs cargo-about
cd packaging/arch && makepkg -si                                # sha256sums is SKIP in the repository
```

The app in the Arch package hosts the computer itself from its **This computer** page. The standalone host program (`sodium-remote-host`) is not in a package: build it from source. The server needs a certificate and a TURN secret before it starts: see [self-hosting](../self-hosting.md).

## What CI runs

`.github/workflows/ci.yml` runs this page on every push and pull request, in an Arch Linux container: `cargo build` and `cargo test` (default members), the SodiumOS-only crates, `cargo deny check`, `reuse lint`, `tools/spdx-headers.py --check`, `i18n/i18n.sh check --strict`, the Qt app's build, tests and smoke test, and the Arch package.

## First run

1. Start `build/bin/sodium-remote` (or the package's launcher).
2. On the other computer, turn on **Allow remote control** and **Pair a device**; in the app choose **Add a computer** and type the code ([user guide](../user-guide.md)).

To check the engine without the Qt app, use the test client: `target/release/remote-client --data /tmp/rc pair --code 123456` then `target/release/remote-client --data /tmp/rc connect`. See [the Linux client reference](../reference/client-linux.md) for every option.

## Troubleshooting a build

- **`pkg-config … failed`:** a development package is missing. The error names the library.
- **No hardware decoding:** check `ffmpeg` was built with NVDEC or VA-API; the client falls back to software.
- **The Qt app can't find its translations or QML modules:** run it from the build tree, or install it with the package.
