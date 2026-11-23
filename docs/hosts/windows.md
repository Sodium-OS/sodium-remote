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
- **Resolution.** The host does not resize or create displays. The picture is the display's own size, scaled down only to fit the other computer's window.
- **Video.** The host uses a hardware encoder when there is one: NVENC, AMD's AMF or Intel's Quick Sync for HEVC, and the GPU's Media Foundation encoder for H.264. Without any of those it falls back to a software H.264 encoder. HEVC is used when a hardware HEVC encoder works and the client decodes HEVC; otherwise H.264.
- **Sound.** The default output device is shared (48 kHz stereo, Opus). If you change the default device, sharing follows it.
- **Clipboard.** Text, HTML and images, both ways. Entries that a password manager marks as concealed are kept out of the clipboard history on the receiving side.
- **A notice** shows when someone is connected, with **Disconnect** and **Dismiss**.

## Limits

- **Secure screens.** The lock screen, the sign-in screen and UAC prompts on the secure desktop can't be captured or controlled. The client shows "… is showing a secure screen" and the session resumes when the desktop is back.
- **Elevated programs.** Windows keeps an ordinary program from driving an elevated one, so an app running as administrator can't be controlled unless the host is elevated too.
- **Ctrl+Alt+Del.** The client's Send Ctrl+Alt+Del can't trigger Windows' secure attention sequence, which needs a service that isn't built yet.
- **Toasts and an installer** aren't built; the app is portable.
- **Testing.** Hardware encoders (NVENC, AMF, Quick Sync), several displays and display scales other than 100% haven't been verified without a virtual machine. See [the status](../../README.md#status-and-known-limits).

Details for contributors: [desktop host reference](../reference/host-desktop.md).
