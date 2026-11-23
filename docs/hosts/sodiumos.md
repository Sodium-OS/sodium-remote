# Hosting on SodiumOS

SodiumOS is the desktop Sodium Remote was built with, and it has the deepest host: the stream starts inside the window manager, and the host makes a display at the size and refresh rate of the controlling computer's window, so nothing is scaled and nothing is letterboxed.

## What you need

- **SodiumOS**, with its window manager, `sodiumwm`. The host (`remoted`) takes frames from the window manager's capture plug-in over a socket. **The plug-in is not part of this repository**, and the SodiumOS host is **off by default in the build**: it is only useful on SodiumOS, so you build it deliberately (`cargo build --release -p remoted`). On a SodiumOS install it ships already built.
- **An NVIDIA graphics card.** The host's encoder is NVENC.
- On a SodiumOS install, nothing else: Sodium Remote comes with SodiumOS. For other Linux desktops, see [the Linux page](linux.md).

## Turning it on

Open Settings › Remote and turn on the switch. Settings › Remote is a view of the host: it shows the state (off, ready, connected, reconnecting), the computer's name and ID, the paired devices, and the server's state. **Pair a device** opens a pairing window and shows the code ([user guide](../user-guide.md)).

## What the host does

- **A display of the right size.** The host creates a virtual display (`REMOTE-1`) sized to the client's window and follows every resize. It moves your windows there while someone is connected, and puts them back when they disconnect.
- **The monitor in front of you.** While a session runs, the monitor in front of you does what its **home mode** says: **Mirror** shows what the controlling computer sees, **Privacy** shows an "In use remotely" screen, and **Off** shows the privacy screen with the panel off and local input ignored.
- **Input method.** A client with its own input method (a Mac, for example) types Chinese, Japanese and Korean into SodiumOS apps through the host's fcitx5 add-on.
- **Locked desktops.** A paired device can connect to a locked desktop, and you unlock it with your password inside the session.
- **Clipboard, sound and drag and drop.** Clipboard text and images both ways; sound plays on the client; files dropped onto the stream arrive on the host.
- **Sleep.** A host that is about to sleep tells its server, so its clients show "Asleep since …" instead of offline.
- **A held session.** If the connection drops, the host holds the session for two minutes, so the client can resume.

## Files

The host's data is in `~/.local/share/sodium-remote/host` ([configuration](../configuration.md)). Its D-Bus name is `org.sodiumos.Remote`, and its unit is `sodiumos-remoted.service`.

## Limits

- It needs the SodiumOS window manager. Other Linux desktops (GNOME and KDE Plasma 6) are hosted through the desktop portals instead: see [the Linux page](linux.md).
- NVIDIA only, for now.

For the full contract, see the [host session reference](../reference/host-session.md), the [video pipeline](../reference/video-pipeline.md) and the [capture IPC](../reference/capture-ipc.md).
