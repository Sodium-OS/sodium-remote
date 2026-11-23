// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
pragma Singleton
import QtQuick
import SodiumUI

// The words and numbers the app shows, in one place: states and ids come from the controller, and only here
// (and in the QML that uses these) do they become text. Nothing here is stored or compared as text.
QtObject {
    /// An OS's name for the pill. Product names: never translated.
    function osName(os) {
        return os === "sodiumos" ? "SodiumOS" : os === "linux" ? "Linux" : os === "mac" ? "macOS" : os === "windows" ? "Windows" : ""
    }

    function routeName(route) {
        switch (route) {
        case "lan": case "LAN": return qsTr("LAN")
        case "direct": return qsTr("Direct")
        case "vpn": case "VPN": return qsTr("VPN")
        case "tailscale": case "Tailscale": return "Tailscale"
        case "relay": case "relay udp": case "relay tcp": case "relay tls": return qsTr("Relay")
        case "lost": return qsTr("Lost")
        default: return ""
        }
    }

    /// "LAN · 2 ms", or just the route.
    function routeChip(route, rtt) {
        const name = routeName(route)
        if (name === "" || route === "lost" || !(Math.round(rtt) >= 1))
            return name
        //: Route and round-trip time in the title bar, for example "LAN · 2 ms". %1 is the route, %2 the number of milliseconds
        return qsTr("%1 · %2 ms").arg(name).arg(Format.number(Math.round(rtt)))
    }

    function routeTone(route) {
        return route === "lost" ? "red" : route === "relay" || route.startsWith("relay") ? "amber" : "normal"
    }

    /// "Last connected today, 09:12", "…yesterday, 21:40", or how long ago for older ones.
    function lastConnected(secs) {
        if (!(secs > 0))
            return qsTr("Never connected")
        const when = new Date(secs * 1000)
        const now = new Date()
        const day = (d) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()
        const days = Math.round((day(now) - day(when)) / 86400000)
        if (days === 0)
            return qsTr("Last connected today, %1").arg(Format.timeOfDay(when))
        if (days === 1)
            return qsTr("Last connected yesterday, %1").arg(Format.timeOfDay(when))
        return qsTr("Last connected %1").arg(Format.relativeTime(when))
    }

    /// The state pill's word on a computer's picture.
    function statusName(state) {
        switch (state) {
        case "online": return qsTr("Online")
        case "busy": return qsTr("In use")
        case "asleep": return qsTr("Asleep")
        case "connecting": return qsTr("Connecting…")
        default: return qsTr("Offline")
        }
    }

    /// How good a path is: "good", "fair", "poor", or "none" (nothing known, or lost). Relay is never better than fair.
    function routeQuality(route, rtt) {
        if (route === "" || route === "lost")
            return "none"
        if (rtt >= 150)
            return "poor"
        return route.startsWith("relay") || rtt >= 80 ? "fair" : "good"
    }

    /// The dot's colour said in words (colour alone isn't enough): the tooltip of the bar's path chip.
    function qualityText(route, rtt) {
        if (route === "lost")
            return qsTr("Connection lost")
        switch (routeQuality(route, rtt)) {
        case "good": return qsTr("Good connection")
        case "fair": return qsTr("Fair connection")
        case "poor": return qsTr("Poor connection")
        default: return qsTr("Connection quality not known yet")
        }
    }

    function qualityBars(q) {
        return q === "good" ? 3 : q === "fair" ? 2 : q === "poor" ? 1 : 0
    }

    /// The card's second line under the name.
    function stateText(c) {
        switch (c.state) {
        case "online": return lastConnected(c.lastConnected)
        case "asleep": return c.asleepSince > 0 ? qsTr("Asleep since %1").arg(Format.timeOfDay(new Date(c.asleepSince * 1000))) : qsTr("Asleep")
        case "connecting": return qsTr("Finding the best path…")
        default: return c.lastSeen > 0 ? qsTr("Last seen %1").arg(Format.relativeTime(new Date(c.lastSeen * 1000))) : qsTr("Offline")
        }
    }

    /// A display's label: the host's own name, else "Display 1".
    function displayName(d) {
        return d.name !== "" ? d.name : qsTr("Display %1").arg(d.number)
    }

    function displaySize(d) {
        //: A display's size in pixels, for example "2560 × 1440"
        return qsTr("%1 × %2").arg(d.width).arg(d.height)
    }

    readonly property string leaveChord: "Ctrl+Alt+Shift+Esc"
    /// What the app is called on a Windows or a Mac computer: a product name, never translated (copy about that host).
    readonly property string otherHostAppName: "Sodium Remote"
}
