# Changelog

All notable changes are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/) from 1.0.

## [Unreleased]

Nothing yet.

## [0.1.0] - Preview

The first public preview. Windows and the generic Linux host have so far been tested in virtual machines.

### Added

- Remote desktop sessions between your own computers: video, audio, keyboard, mouse and clipboard (text, HTML and images, both ways).
- **Platforms.** Windows 11, macOS 14 or later, SodiumOS and other Linux desktops. Each computer can control the others and be controlled.
- **Clients.** A Qt app built on Sodium UI for Linux and Windows, and a native app for macOS.
- **Hosts.**
  - Windows and macOS: the app hosts the computer from its **This computer** page (Windows) or its host pane (Mac).
  - SodiumOS: its own host, which needs an NVIDIA graphics card and the SodiumOS window manager.
  - Other Linux desktops: GNOME and KDE Plasma 6 on Wayland, through the desktop portals (screen sharing, remote input and clipboard) and PipeWire. The desktop asks for permission, and the app's **This computer** page shows the state and has **Forget permission…**. Hyprland, Sway, X11 sessions and the sign-in screen are not supported in 0.1.
- **Server.** A rendezvous and relay server you run yourself (`sodium-remote-server`, with deploy scripts and coturn), for connections across the internet. It supports enrollment tokens, several relay regions and quotas. The apps have no server built in: add one in **Settings › Server**, with a `sodium-remote://` link, a file or by hand.
- Each computer's card shows what it runs (SodiumOS, Windows, macOS or Linux). Linux computers send their distribution ID as `os_flavor`.
- Hardware video encoding and decoding where the platform provides it.
- Pairing with a six-digit code, end-to-end encrypted connections, and direct connection on your network or by address.
- The device key is encrypted with DPAPI on Windows and kept in the Keychain on a Mac.
- On Windows, the software H.264 encoder (Cisco's OpenH264) is downloaded on first need, checked against a pinned SHA-256, and refused if the download is larger than 8 MiB.
- The interface in ten languages: English (US, UK, Australia), Chinese (Simplified and Traditional), Japanese, Korean, Spanish, French and German.
- Licensed under AGPL-3.0-only, with a commercial license available (see LICENSING.md).

### Not included

- No hosted relay service yet; one is planned at remote.sodiumos.com.
- No signed Windows build and no signed macOS build; the Mac app is built from source.
- No automatic updates and no telemetry.
