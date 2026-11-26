# Hosting on other Linux desktops

A Linux computer running GNOME or KDE Plasma 6 on Wayland can be controlled by any paired Sodium Remote client. The host uses the desktop's own screen-sharing permission, the same one video-call apps ask for, so the desktop asks before anything is shared. Version 0.1.0 is a preview: the Linux host has so far been tested only in virtual machines (GNOME 49 and KDE Plasma 6.8). The Sodium Remote app has a **This computer** page for it; the host also runs from the command line.

If you run SodiumOS, it has its own host: see [the SodiumOS page](sodiumos.md).

## What works where

| | GNOME 46 or later | KDE Plasma 6 | Hyprland, Sway, other wlroots | X11 sessions |
|---|---|---|---|---|
| Screen | Yes | Yes | Not in 0.1 | Not in 0.1 |
| Mouse and keyboard | Yes, if allowed in the dialog | Yes | | |
| Clipboard (text, HTML, images) | Yes | Yes | | |
| Sound | Yes | Yes | | |
| Reconnecting without the dialog | Yes, once remembered | Yes, once remembered | | |

## Turning it on in the app

1. Install Sodium Remote ([install](../install.md)) and open it. Choose **This computer**.
2. Turn on **Allow remote control of this computer**, then **Pair a device…**. The page shows a code that works once, for five minutes ([user guide](../user-guide.md)).
3. Pair the other computer with it, then connect from there. The first time, your desktop asks to share the screen, so someone must be at this computer. The **Screen sharing** group on the page says **Not asked yet** until then, **Waiting for you** while the dialog is open, **Allowed** once you allowed it and **Not allowed** if you didn't:
   - **GNOME:** choose the screen, turn on **Allow Remote Interaction** (without it the client can only watch, and the page says so), turn on **Remember This Selection**, then **Share**.
   - **KDE Plasma:** KDE shows **Remote Control Requested**: "Allow Sodium Remote to gain remote control privileges?", "Sodium Remote will be able to: See what's on the screen, Move the pointer and type keystrokes". Leave **Allow the application to do this without asking next time** on (it is) and choose **Allow**; **Deny** refuses. With several screens KDE shares all of them as one picture.
4. With the choice remembered, devices reconnect later without anyone at the computer.

Closing the app stops sharing; the page's switch comes back on when you open it again. On a desktop that can't share its screen (Hyprland, Sway, other wlroots compositors, X11), the page says so and has no switch; you can still connect to other computers from it. The page has no tray icon and no start at sign-in on Linux.

## Turning it on from the command line

1. Build Sodium Remote from source ([the Linux build guide](../building/linux.md)). The host is the program `sodium-remote-host`; the Arch package carries the app, not this program.
2. Start it in your desktop session as the app, so the desktop can name it and remember your choice:

       systemd-run --user --unit=app-org.sodiumos.Remote@host sodium-remote-host --pair

   It prints `pairing code NNNNNN` on a terminal (it never goes to the journal; it is also in `pairing-code.txt`, readable only by you). The code works once, for five minutes ([user guide](../user-guide.md)). Pair the client with it; later starts don't need `--pair`.
3. Connect from the client. The computer shows the desktop's sharing dialog:
   - **GNOME:** choose the screen, turn on **Allow Remote Interaction** (without it the client can only watch), turn on **Remember This Selection**, then **Share**.
   - **KDE Plasma:** KDE shows **Remote Control Requested** (see above). Choose **Allow**, with **Allow the application to do this without asking next time** on.
4. With the choice remembered, clients reconnect later without anyone at the computer, also after the host restarts.

The host is a per-user program in your graphical session, so the computer must be signed in. It does not run on the sign-in screen. Its data is in `~/.local/share/sodium-remote/host`, and only one host can use it at a time: if the app's **This computer** switch is on, `sodium-remote-host` prints that another host is already sharing this computer and exits, and the page says the same if the program is running when you turn the switch on.

If KDE reports that it can't identify the application, the host was started outside its `app-org.sodiumos.Remote@….service` unit or the app's `.desktop` file isn't installed. It still works, but KDE won't remember the choice.

## Stopping and asking again

- **Stop a session at the computer:** in GNOME, the sharing indicator in the top bar; in KDE, the item in the system tray, then **End**. The client is told the session was ended at the computer.
- **Cancelling the dialog** does the same, and the next connection asks again.
- **Ask every time again:** on the **This computer** page, once the permission shows **Allowed**, choose **Forget permission…** in the **Permission** row and confirm. It works with remote control off. From the command line: `sodium-remote-host --forget-consent`. The next connection shows the dialog.
- **Turn it off:** stop the host (`systemctl --user stop 'app-org.sodiumos.Remote@host'`). Paired clients stay paired.

While a client is connected the desktop shows that the screen is being shared.

## What the host shows

- **Displays.** GNOME: one display, the one chosen in the dialog; the client can't switch to another. KDE with several screens: all of them as one picture.
- **Resolution.** The host does not resize or create displays. The picture follows the controlling computer's window: when that window is resized or goes full screen, the host re-encodes at the largest size with the display's shape that fits it, never larger than the display's own size. The stream changes about 150 ms after the window stops changing.
- **Video.** H.264 or HEVC on the graphics card where there is a VA-API driver (Intel, AMD) or NVIDIA's driver; otherwise H.264 in software (x264 or OpenH264 from the system). HEVC is used when a hardware HEVC encoder works and the client decodes HEVC. Fedora ships without H.264 encoders; to get software H.264 there, enable the OpenH264 repository from Cisco or RPM Fusion and install `openh264`.
- **Keyboard.** Keys arrive as key positions, so the computer's own keyboard layout applies, as with a keyboard plugged into it. Text from an input method on the client arrives as characters, as long as they are on the computer's keyboard layout.
- **Sound.** The default output is shared (48 kHz stereo, Opus), and follows it when you change it.
- **Clipboard.** Text, HTML and images, both ways. Entries that a password manager marks as concealed are kept out of the clipboard history on the receiving side.

## Limits

- **Desktops.** Only GNOME and KDE Plasma 6 on Wayland. Hyprland, Sway and other wlroots compositors, X11 sessions and the sign-in screen are not supported in 0.1.
- **Lock screen.** On GNOME locking the computer ends the session: the other computer shows "is locked" with "Unlock it at that computer, then reconnect.", and a connection to a locked computer gets the same card. The permission you gave and the saved choice stay, so reconnecting after unlocking needs no dialog. On KDE the session continues and shows the lock screen; a paired device can unlock it by typing your account password (it shows on the monitor). Nobody can connect before you sign in on either.
- **No pinch to zoom.**
- **Text.** On GNOME only characters on the computer's keyboard layout can be typed from the other computer (an input method's é, ñ or Chinese, Japanese and Korean text is dropped); KDE types any character. Workaround on GNOME: copy the text on the other computer and paste it; the clipboard works from the other computer to GNOME. The app's Limits list says the same on GNOME.
- **Testing.** Hardware encoding (VA-API, NVENC) hasn't been verified with this host; several displays and a 2x display scale were tried in a virtual machine only. See [the status](../../README.md#status-and-known-limits).

Details for contributors: [generic Linux host reference](../reference/host-linux.md).
