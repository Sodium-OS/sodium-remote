// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Foundation

// The pure rules behind Sodium Remote's second direction (cross-platform
//,): which computers we know the OS of, the
// display picker, the permission rows of "This computer", the menu-bar state.
// No AppKit and no core types: `swift test` covers it.

/// The OS a computer runs, as remembered from its `Welcome.host_os` or a device's `Hello.client_os`.
/// Stored as a case name, never as text; the pill's words are product names and are not translated.
public enum HostOS: String, Codable, Equatable, Sendable {
    case sodiumOS, linux, windows, mac

    /// The text pill on a computer's picture (OS names are not translated).
    public var pillText: String {
        switch self {
        case .sodiumOS: return "SodiumOS"
        case .linux: return "Linux"
        case .windows: return "Windows"
        case .mac: return "macOS"
        }
    }

    ///: a Windows, Mac or other Linux computer's copy names "Sodium Remote" (untranslated, passed as `%1`);
    /// a SodiumOS computer's copy names "Settings › Remote" on that computer, never "Sodium Remote".
    public var namesSodiumRemote: Bool { self != .sodiumOS }

    /// "Send Ctrl+Alt+Del" is for SodiumOS computers only (: a Windows host keeps it out of the
    /// menu until an injected chord is known to reach its security screen; a Mac has none).
    public var offersCtrlAltDel: Bool { self == .sodiumOS }
}

/// One of the host's displays (`Welcome.displays`).
public struct DisplayChoice: Equatable, Identifiable, Sendable {
    public var id: UInt32
    /// "" when the host has no name for it.
    public var name: String
    public var width: UInt32
    public var height: UInt32
    public var primary: Bool

    public init(id: UInt32, name: String, width: UInt32, height: UInt32, primary: Bool) {
        self.id = id
        self.name = name
        self.width = width
        self.height = height
        self.primary = primary
    }
}

/// How the bar names a display: the host's own name, else "Display N" (N from 1, in the host's order).
public enum DisplayLabel: Equatable {
    case named(String)
    case numbered(Int)
}

/// The Mac client's display picker: none with one display, none for a
/// Virtual host (SodiumOS), one entry per display otherwise.
public enum DisplayPicker {
    public static func isShown(displays: Int, virtual: Bool) -> Bool {
        !virtual && displays > 1
    }

    public static func label(_ d: DisplayChoice, index: Int) -> DisplayLabel {
        let n = d.name.trimmingCharacters(in: .whitespacesAndNewlines)
        return n.isEmpty ? .numbered(index + 1) : .named(n)
    }

    /// "1920 × 1080", for the menu's trailing text.
    public static func sizeText(_ d: DisplayChoice) -> String {
        "\(d.width) × \(d.height)"
    }
}

/// A permission row of the This computer pane.
public enum PermissionRowState: Equatable, Sendable {
    case allowed, notAllowed
    /// "Allow…" was pressed and System Settings is open; the page updates by itself.
    case waiting
    /// Screen Recording only: allowed in System Settings, not working until the app restarts.
    case reopenToFinish
}

/// What the core says about a permission (`PermissionState`), without its type.
public enum PermissionAnswer: Equatable, Sendable {
    case allowed, notAllowed, reopenToFinish
}

public enum HostPermissionRules {
    /// The row for an answer: "Waiting" only for a permission not yet allowed whose System Settings step was started.
    public static func row(_ a: PermissionAnswer, settingsOpened: Bool) -> PermissionRowState {
        switch a {
        case .allowed: return .allowed
        case .reopenToFinish: return .reopenToFinish
        case .notAllowed: return settingsOpened ? .waiting : .notAllowed
        }
    }

    /// View only: the screen is shared but the keyboard and mouse aren't.
    public static func viewOnly(screenRecording: PermissionAnswer, accessibility: PermissionAnswer) -> Bool {
        screenRecording == .allowed && accessibility != .allowed
    }
}

/// The three states of the menu-bar icon: off is dim, on is solid, controlled is accent with a dot.
public enum PresenceState: Equatable, Sendable {
    case off, on, controlled

    public static func of(allowed: Bool, controlled: Bool) -> PresenceState {
        controlled ? .controlled : (allowed ? .on : .off)
    }
}

/// The address other computers connect to: `IP:PORT` is what the pane shows (a bare
/// address pairs), `ID@IP:PORT` is "Copy full address". The port is the host's bound LAN port (`ThisComputer.lanPort()`).
public enum HostAddress {
    public static func hostPort(ip: String, port: Int) -> String {
        ip.contains(":") ? "[\(ip)]:\(port)" : "\(ip):\(port)"
    }

    public static func string(id: String, ip: String, port: Int) -> String {
        id + "@" + hostPort(ip: ip, port: port)
    }
}
