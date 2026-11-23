// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Every public identifier of Sodium Remote for the Qt app, mirroring remote/core/src/brand.rs, which is
// the reference: change a value there first. The Qt app ID and the D-Bus names stay org.sodiumos.* and live in the
// .desktop file and the CMake ID.
#pragma once

namespace brand {
inline constexpr const char ProductName[] = "Sodium Remote";
inline constexpr const char License[] = "AGPL-3.0-only";
inline constexpr const char SourceUrl[] = "https://github.com/sodium-os/sodium-remote";
inline constexpr const char SelfHostingUrl[] = "https://github.com/sodium-os/sodium-remote/blob/main/docs/self-hosting.md";
inline constexpr const char LinuxHostUrl[] = "https://github.com/sodium-os/sodium-remote/blob/main/docs/hosts/linux.md";
inline constexpr const char DefaultServerPath[] = "/remote";
inline constexpr const char LinkScheme[] = "sodium-remote";
inline constexpr const char LinuxDirName[] = "sodium-remote";
inline constexpr const char WindowsDirName[] = "SodiumRemote";
inline constexpr const char BinApp[] = "sodium-remote";
inline constexpr const char BinAppWindows[] = "SodiumRemote.exe";
inline constexpr const char EnvData[] = "SODIUM_REMOTE_DATA";
inline constexpr const char AppBinaryName[] = "sodium-remote";
}
