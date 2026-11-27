# Troubleshooting and FAQ

## Finding and pairing

**A computer I want to control doesn't appear under "Found nearby".**
- On that computer, **Allow remote control** must be on.
- Both computers must be on the same network, and the network must let devices see each other: some guest Wi-Fi and corporate networks block it. Try **Connect by address** (name or IP, and the port if it isn't 47311).
- On a Mac, allow **Local Network** (System Settings › Privacy & Security › Local Network). On Windows, the network must be **Private**, and the program must be allowed through the firewall.
- Discovery uses mDNS (UDP 5353 multicast). A VPN or firewall that drops multicast hides computers; by address still works.

**"That code didn't match. … attempts left."** Type the code as shown on the computer to be controlled. A code works once, for five minutes, and three wrong tries close pairing on that computer until someone there opens it again. If it says "This code expired" or that pairing closed after wrong codes, choose **New code** there.

**"Couldn't pair with …. Check that it is on and on this network."** The computer isn't reachable, or pairing isn't open. On that computer choose **Pair a device** again.

**"… doesn't trust this computer" / "It was removed there. Pair again to connect."** The other computer removed this one from its paired devices (or its key changed). Pair again.

## Connecting

**"Couldn't reach …".** Check the address; that **Allow remote control** is on; that the computer is awake and online ("It may be turned off or without internet"). A firewall on that computer looks the same as nothing answering: allow the program on private networks.

**"… is in use" / "Taken over by …".** One device controls a computer at a time. **Take over** to disconnect the other, or **Take it back** if you were taken over.

**"… needs an update" / "It runs an older version that can't talk to this one."** The two versions can't talk. The message says which side to update. Computers one release apart work together ([compatibility](compatibility.md)).

**"Remote control needs a Private network" (Windows).** Set the network to Private in Windows network settings.

**It connects on the local network but not from elsewhere.** Away from home you need a server ([self-hosting](self-hosting.md)): hole punching and relays need one. Without a server, connect by address through a VPN or a port you forwarded yourself.

**The path says "Relay" and it's slow.** No direct path could be found (a symmetric NAT or a firewall blocking UDP on one side), so traffic goes through the server's relay. A relay's capacity limits the stream. A closer server (a relay region near you) and a bandwidth limit that suits it help.

## Servers

**Settings › Server says "No server".** That is the default. Nothing is built in; add a server with a link or a file ([user guide](user-guide.md), [configuration](configuration.md)).

**"Not enrolled" or a token error on a host.** The server needs an enrolment token for new hosts. Get one from the server's operator and add it with the server's link, or put it in `server.toml`. `token_invalid` is a token the server doesn't know (or that was revoked); `token_expired` is past its expiry or used up: ask for a new one. A host stops retrying after a refused token, so it doesn't use up the address's registrations: put the new token in Settings › Server (a running host picks it up within seconds; otherwise turn hosting off and on). "Too many computers have registered from this network address" means the server's limit (five new computers an hour per address) was reached; the host tries again every few minutes.

**The app can't connect to the server, or says the certificate isn't trusted.**
- The URL must be `wss://name:port/remote`, and the name must match the certificate.
- The certificate must be valid and chain to a public CA (on Windows and macOS the operating system checks the chain, so a root it hasn't installed yet, such as Let's Encrypt's ISRG Root X1 on a new Windows, is fetched on first use), or you must give the device the server's private CA (`ca_file`, or in the link). A pinned CA replaces the system's CAs.
- If an operator renewed the certificate but didn't restart the server, it may still serve the old one.

For servers you run: `deploy.sh status`, the checks in `relay/deploy/verify.sh`, and the logs ([self-hosting](self-hosting.md#logs)).

## While connected

**The picture is blurry or blocky.** The stream adapts to the network, and quality gives way before smoothness does. When the screen stops moving it sharpens within a fraction of a second. If it stays poor, check the Stats (round-trip time, loss), switch the bandwidth limit to Automatic or higher, try a wired connection, or choose a codec your hardware decodes (Settings › Quality).

**The screen is black or says "secure screen".** A lock screen, sign-in screen or UAC prompt can't be shown or controlled remotely; the session resumes when the desktop is back. A Mac that hasn't allowed Screen Recording turns clients away with a card saying so.

**I can see but not control a Mac ("View only").** Allow Accessibility for Sodium Remote on that Mac (System Settings › Privacy & Security › Accessibility).

**Shortcuts like ⌘Tab or Alt+Tab act on my computer, not the other one.** Use full screen, and on a Mac allow Accessibility on the controlling Mac. Choose the key mapping (Automatic or Pass through) in the Keyboard menu.

**The keyboard is stuck in the stream.** Press Ctrl+Alt+Shift+Esc to leave it.

**No sound.** Sound plays on the controlling computer. Check its output device, and that the host has a sound output. A Windows host shares the default output device.

**The clipboard doesn't sync.** Text, HTML and images sync both ways. Password-manager entries marked concealed are marked so on the other side. Very large clips are sent in chunks and may take a moment.

**It shows "Reconnecting…".** The network dropped. The host holds the session for about two minutes; the stream resumes when the network returns.

## Windows

**SmartScreen blocks it.** The 0.1.0 build is unsigned. Choose More info, then Run anyway, after checking the checksum ([install](install.md)).

**It runs but nothing can connect to it.** Check **Allow remote control** is on, the network is Private, and the program is allowed through the firewall.

## macOS

**It asks for permissions again after every build.** An ad-hoc signature changes with each build. Sign with a stable identity ([building on macOS](building/macos.md)).

**"Quit and Reopen".** Screen Recording only takes effect after the app restarts.

## Linux

**The app doesn't stream.** It needs a Wayland session.

**Can I control a Linux desktop that isn't SodiumOS?** Yes, on GNOME and KDE Plasma 6 under Wayland. Hyprland, Sway, X11 sessions and the sign-in screen are not supported in 0.1. See [the Linux host page](hosts/linux.md).

## FAQ

**Is Sodium Remote open source?** Yes. The hosts, the apps, the protocol and the relay server are open source under the AGPL-3.0, and a commercial license is available ([LICENSING.md](../LICENSING.md)).

**Which platforms does it support?** SodiumOS, macOS 14 or later and Windows 11. On each of them one app controls other computers and lets the computer itself be controlled. Other Linux desktops work too (GNOME and KDE Plasma 6 on Wayland). Version 0.1.0 is a preview.

**Do I need to open a port on my router?** No. Sodium Remote connects directly on a local network or by address, uses hole punching to get through home routers, and falls back to an encrypted relay when no direct path exists.

**Do I need a server?** Not on your own network, or when you connect by address: computers talk to each other directly. To find and reach your computers from anywhere, point Sodium Remote at a server you run yourself. A hosted relay service at remote.sodiumos.com is planned; until then, use your own server or connect on your network or by address.

**How is it different from VNC or RDP?** VNC and RDP send screen updates over a reliable connection. Sodium Remote sends hardware-encoded video over its own UDP protocol with error correction, draws the cursor on your side and adapts quality on every frame, which keeps latency low on imperfect networks.

**Can I connect while the computer is locked?** On SodiumOS, yes: a paired device can connect to a locked desktop, and you unlock it with your password inside the session. On Windows and macOS the lock screen can't be shown or controlled remotely.

**Does it send anything to its authors?** No. There is no telemetry and no update check.

**Is it safe to use over the internet?** Sessions are encrypted end to end between computers you paired, and servers see only metadata. See [security](security.md). Version 0.1.0 is a preview and hasn't had an independent audit.

**Where are my keys and pairings?** In the data directory ([configuration](configuration.md)), or the Keychain on a Mac.

**How do I get help or report a bug?** Open an issue on [GitHub](https://github.com/sodium-os/sodium-remote/issues). For a security problem, don't: see [SECURITY.md](../SECURITY.md).
