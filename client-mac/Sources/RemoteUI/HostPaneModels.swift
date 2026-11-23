// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import Combine
import RemoteKit
import SwiftUI

// "This computer": the host side of Sodium Remote. The views here never see the core's types: the app's
// `HostController` (target Remote) owns the `ThisComputer` object, fills this model from it and implements
// the actions.

/// The main window's two pages (the bar's "Computers" and "This computer").
public enum MainPage: Equatable {
    case computers, thisComputer
}

/// A device allowed to control this Mac.
public struct PairedDeviceModel: Identifiable, Equatable {
    public var id: String
    public var name: String
    public var os: HostOS?
    public var lastUsed: Date?
    /// It is the one controlling this Mac now.
    public var connectedNow: Bool

    public init(id: String, name: String, os: HostOS?, lastUsed: Date?, connectedNow: Bool) {
        self.id = id
        self.name = name
        self.os = os
        self.lastUsed = lastUsed
        self.connectedNow = connectedNow
    }
}

/// Who is controlling this Mac.
public struct ControllerModel: Equatable {
    public var name: String
    public var os: HostOS
    public var since: Date
    /// The lock screen is up: the device sees the secure-screen notice.
    public var secure: Bool
    /// The connection was lost and the session is held for it to come back.
    public var reconnecting: Bool

    public init(name: String, os: HostOS, since: Date, secure: Bool, reconnecting: Bool) {
        self.name = name
        self.os = os
        self.since = since
        self.secure = secure
        self.reconnecting = reconnecting
    }
}

/// An open pairing window: a 6-digit code for 5 minutes.
public struct HostPairing: Equatable {
    public var code: String
    public var expiresAt: Date

    public init(code: String, expiresAt: Date) {
        self.code = code
        self.expiresAt = expiresAt
    }
}

@MainActor
public protocol HostPaneActions: AnyObject {
    /// "Allow remote control of this computer".
    func setAllowed(_ on: Bool)
    func openPairing()
    func closePairing()
    /// Remove a device after the user confirmed.
    func revoke(deviceID: String)
    func disconnect()
    func setName(_ name: String)
    func setStartAtLogin(_ on: Bool)
    /// Screen Recording's "Open System Settings" step: macOS's prompt the first time, then System Settings.
    func allowScreenRecording()
    func allowAccessibility()
    func quitAndReopen()
    /// Copies `IP:PORT`.
    func copyAddress()
    /// Copies `ID@IP:PORT`.
    func copyFullAddress()
}

@MainActor
public final class HostPaneModel: ObservableObject {
    @Published public var isOn = false
    /// The switch was flipped and the host is starting.
    @Published public var starting = false
    /// Why it couldn't be turned on, from the core (shown as given).
    @Published public var startError: String?
    @Published public var controller: ControllerModel?
    @Published public var pairing: HostPairing?
    /// Three wrong codes closed pairing: it stays closed until it is opened here again.
    @Published public var pairingClosedByWrongCodes = false
    @Published public var devices: [PairedDeviceModel] = []
    @Published public var name = ""
    /// The Sharing name: the field's placeholder.
    @Published public var sharingName = ""
    @Published public var startAtLogin = false
    @Published public var screenRecording: PermissionRowState = .notAllowed
    @Published public var accessibility: PermissionRowState = .notAllowed
    /// `IP:PORT`, for "connect by address" on another computer (a bare address pairs).
    @Published public var address: String?
    /// `ID@IP:PORT`: "Copy full address".
    @Published public var fullAddress: String?
    /// The device whose Remove is being asked.
    @Published public var revokeAskID: String?
    public weak var actions: HostPaneActions?

    public init() {}

    /// View only: the screen is shared, the keyboard and mouse aren't. A pending
    /// "reopen to finish" counts as not shared, so this is only the Accessibility-missing case.
    public var viewOnly: Bool { screenRecording == .allowed && accessibility != .allowed }

    public var presence: PresenceState { PresenceState.of(allowed: isOn, controlled: controller != nil) }
}
