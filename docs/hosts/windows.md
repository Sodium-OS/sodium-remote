# Hosting on Windows

A Windows 11 computer can be controlled by any paired Sodium Remote client, and it controls others from the same app. Version 0.1.0 is a preview: Windows has so far been tested in a virtual machine.

## Turning it on

1. Run `SodiumRemote.exe` ([install](../install.md)).
2. Open **This computer** and turn on **Allow remote control**. While it is on, the app shows "Remote control is on".
3. To pair a client, choose **Pair a device**. The code works once, for five minutes ([user guide](../user-guide.md)).
4. Optional: turn on **Start at login** so the computer is available after you sign in. The app then starts in the background and keeps a tray icon.

The host is a per-user program in your interactive session. It doesn't run as a service, so the computer must be signed in. It does not run on the sign-in screen.

## The network profile and the firewall

- Windows must treat the network as **Private**. If it is Public, the app says "This network is Public" and "Remote control needs a Private network. Set this network to Private in Windows network settings."
- The host listens on TCP port 47311 on your network (any free port if that's taken), for pairing and for connecting by address. Media goes over UDP between the two computers, on ports chosen at run time. If a firewall blocks it, the app says "The firewall is blocking …" and "Other computers on your network can't reach this one until you allow it." Allow the program on private networks.
- The tray menu and **This computer** page show **Address for connecting directly**.

## What the host shows

- **Displays.** The host streams your real screen. When there are several displays, the controlling computer chooses which to see. One display at a time.
- **Resolution.** Unless [the display is matched to the connecting computer](#matching-the-display-to-the-connecting-computer), the host does not resize or create displays. The picture follows the controlling computer's window: when that window is resized or goes full screen, the host re-encodes at the largest size with the display's shape that fits it, so the other computer shows it pixel for pixel (with bars only where the shapes differ). It is never larger than the display's own size. The stream changes about 150 ms after the window stops changing, with one keyframe.
- **Video.** The host uses a hardware encoder when there is one: NVENC, AMD's AMF or Intel's Quick Sync for HEVC, and the GPU's Media Foundation encoder for H.264. Without any of those it falls back to a software H.264 encoder. HEVC is used when a hardware HEVC encoder works and the client decodes HEVC; otherwise H.264.
- **Sound.** The default output device is shared (48 kHz stereo, Opus). If you change the default device, sharing follows it.
- **Clipboard.** Text, HTML and images, both ways. Entries that a password manager marks as concealed are kept out of the clipboard history on the receiving side.
- **A notice** shows when someone is connected, with **Disconnect** and **Dismiss**.

## Matching the display to the connecting computer

**Match the display to the connecting computer** on **This computer** is off by default. When it is on, Windows uses a virtual display the size of the other computer's window while someone is connected, so that computer shows the desktop pixel for pixel, with no bars and no scaling.

- **Turning it on** installs a display driver, SudoVDA, which Sodium Remote builds from its open source and signs with its own certificate. Windows asks once for administrator permission. The app first checks the driver's files against checksums built into the app, then adds Sodium Remote's signing certificate to this computer's Trusted Root Certification Authorities and Trusted Publishers stores, and installs the driver. If you choose No, nothing changes and the switch stays off.
- **While someone is connected**, the virtual display is the only display: this computer's own screens turn off, and Windows moves the open windows onto the virtual display. When the session ends, the screens come back as they were and Windows moves the windows back. A window left outside every screen is moved to the main screen.
- **The keyboard and mouse at this computer keep working** while its screens are off. What is typed there goes to the shared desktop.
- **Resizing.** While the other computer's window is being resized, the picture is scaled to fit it. About 400 ms after the size stops changing, the virtual display takes the new size. The virtual display's width and height are even, at least 800 × 600, and its refresh rate is at most 120 Hz.
- **If the virtual display can't start** within 3 seconds, or the driver is missing, the session shows this computer's own screens instead, scaled to the window, and **This computer** says "The virtual display couldn't start. Showing this computer's screen instead."
- **A lost connection** keeps the virtual display for 15 seconds, so the other computer can come back to it. **Disconnect** and **Quit** turn the screens back on at once.
- **If the app stops unexpectedly** (it crashes, it is ended in Task Manager, or you sign out), the virtual display goes away by itself about 3 seconds later and Windows turns the screens back on: the change was never saved to Windows' display settings. The next time the app starts, it puts the screens back as they were if they aren't.
- **Turning it off** asks for administrator permission again, then removes the driver, its device and the certificate. The setting can't be changed while someone is connected.
- **If the app folder was deleted while the setting was on**, remove the rest by hand: in Device Manager, under Display adapters, uninstall "SudoMaker Virtual Display Adapter" and choose to remove its driver; in `certlm.msc`, delete "Sodium Remote Driver Signing (self-signed)" from Trusted Root Certification Authorities and from Trusted Publishers.

While the setting is on, the certificate is trusted for code signing on this computer, and any program running on it can add virtual displays through the driver. If another program already installed SudoVDA (Apollo does), the setting can't be turned on. One virtual display at a time; no HDR. Windows picks the virtual display's scale. The matched display has not been tested on real hardware yet.

## Limits

- **Secure screens.** The lock screen, the sign-in screen and UAC prompts on the secure desktop can't be captured or controlled. The client shows "… is showing a secure screen" and the session resumes when the desktop is back.
- **Elevated programs.** Windows keeps an ordinary program from driving an elevated one, so an app running as administrator can't be controlled unless the host is elevated too.
- **Ctrl+Alt+Del.** The client's Send Ctrl+Alt+Del can't trigger Windows' secure attention sequence, which needs a service that isn't built yet.
- **Toasts and an installer** aren't built; the app is portable.
- **Testing.** Hardware encoders (NVENC, AMF, Quick Sync), several displays and display scales other than 100% haven't been verified without a virtual machine. See [the status](../../README.md#status-and-known-limits).

Details for contributors: [desktop host reference](../reference/host-desktop.md).
