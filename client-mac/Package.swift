// swift-tools-version:5.9
// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Sodium Remote for Mac (Swift, AppKit, Metal, VideoToolbox; the
// Rust core and the macOS host through UniFFI). `build-app.sh` builds the one static
// library, generates `Generated/` and assembles the app; run it before any `swift`
// command.
import PackageDescription

/// `build-app.sh` puts libremote_ffi.a here: host-desktop with remote-core's surface
/// inside it, so there is one copy of each symbol.
let rustLib = Context.packageDirectory + "/Generated/lib"

let package = Package(
    name: "RemoteMac",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Remote", targets: ["Remote"]),
        .executable(name: "s6-probe", targets: ["S6Probe"]),
    ],
    targets: [
        // The C side of the UniFFI bindings (generated: RemoteCoreFFI.h).
        .systemLibrary(name: "RemoteCoreFFI", path: "Generated/RemoteCoreFFI"),
        // The Swift side (generated: RemoteCore.swift and SodiumRemoteHost.swift, which uses
        // core's `Os` converter, so both are in this one module), linked with the static library.
        .target(
            name: "RemoteCore",
            dependencies: ["RemoteCoreFFI"],
            path: "Generated/RemoteCore",
            swiftSettings: [.unsafeFlags(["-suppress-warnings"])],
            linkerSettings: [
                .unsafeFlags(["-L", rustLib]),
                .linkedLibrary("remote_ffi"),
                .linkedFramework("Security"),
                .linkedFramework("SystemConfiguration"),
                .linkedFramework("CoreFoundation"),
                // The macOS host backend (rustc's own list is in
                // Generated/native-static-libs.txt).
                .linkedFramework("ScreenCaptureKit"),
                .linkedFramework("VideoToolbox"),
                .linkedFramework("CoreMedia"),
                .linkedFramework("CoreVideo"),
                .linkedFramework("CoreGraphics"),
                .linkedFramework("IOSurface"),
                .linkedFramework("IOKit"),
                .linkedFramework("ApplicationServices"),
                .linkedFramework("AppKit"),
                .linkedFramework("Foundation"),
                .linkedLibrary("iconv"),
                .linkedLibrary("resolv"),
                .linkedLibrary("c++"),
            ]
        ),
        // Pure logic, no AppKit and no core: layout, the timeline, the key
        // table, bitstream parsing, stats. `swift test` covers it.
        .target(name: "RemoteKit", path: "Sources/RemoteKit"),
        // SwiftUI views and their state (no core types: its names would clash
        // with SwiftUI's Button, Axis and Path).
        .target(name: "RemoteUI", dependencies: ["RemoteKit"], path: "Sources/RemoteUI"),
        // The app: AppKit, the stream (Metal, VideoToolbox), input, the core.
        .executableTarget(
            name: "Remote",
            dependencies: ["RemoteCore", "RemoteKit", "RemoteUI"],
            path: "Sources/Remote"
        ),
        // The decode probe: VideoToolbox HEVC 4:4:4 / 4:2:0 decode, hardware or not, and its time.
        .executableTarget(name: "S6Probe", dependencies: ["RemoteKit"], path: "Tools/S6Probe"),
        .testTarget(name: "RemoteKitTests", dependencies: ["RemoteKit"], path: "Tests/RemoteKitTests"),
    ]
)
