# Hosting on macOS

A Mac with macOS 14 or later can be controlled by any paired Sodium Remote client, and it controls others from the same app. There is no signed build for 0.1.0: [build it](../building/macos.md).

## Permissions

The app asks for these in its welcome screen. The host needs two of them:

| Permission | Needed for | If it is missing |
|---|---|---|
| **Screen Recording** | showing the screen (and sound) | The Mac still runs and pairs, but a client that connects is turned away with a card telling it to allow Screen Recording on the Mac |
| **Accessibility** | letting the other computer type, click and scroll | The session runs **view only**: the client sees the screen, shows a "View only" chip and can't control it |

Turn them on in System Settings › Privacy & Security. After allowing Screen Recording, macOS needs the app reopened (the app shows "Reopen to finish"). A third permission, **Local Network**, lets other computers on your network find this one; without it, connect by address.

## Turning it on

1. Open Sodium Remote.
2. On the This computer page, turn on **Allow remote control**.
3. To pair a client, choose **Pair a device** and type the code on the other computer ([user guide](../user-guide.md)).
4. Turn on **Start at login** if you want the Mac to be available after you sign in. A menu-bar item shows when someone is connected, and the Mac shows a notification.

The host runs inside the app, so the permissions belong to the app. The host's key is kept in the macOS Keychain ("Sodium Remote host key").

## What the host does

- **Capture.** ScreenCaptureKit, one display at a time, in the display's own pixels. When there are several displays, the controlling computer chooses which one to see. The pointer is sent as shape and position, not drawn into the picture.
- **Video.** VideoToolbox. HEVC is offered only when the Mac has a hardware HEVC encoder; H.264 otherwise.
- **Sound.** The system's audio is shared.
- **Clipboard.** Text, HTML and images, both ways. Concealed entries from password managers are marked concealed.
- **Sleep.** While a session runs, the Mac keeps its display awake.
- **Keys.** A PC keyboard on the Mac works as a PC keyboard: Ctrl acts as ⌘, and so on; macOS shortcuts reach the Mac as macOS shortcuts. The client's key mapping setting controls this.
- **Pinch.** A pinch on a trackpad client arrives as a pinch gesture. If an app doesn't zoom, set `SODIUM_REMOTE_PINCH=cmd-scroll` when running the host as a program to use ⌘+scroll instead.

## Limits

- **Secure screens.** The lock screen, and another user on the console after fast user switching, can't be shown or controlled. The client shows "… is showing a secure screen" and the session resumes when the desktop is back.
- **No login-window access.** The Mac must be signed in.
- **One display at a time**, and no virtual display or resizing: the client shows the display at its own size, scaled to fit.
- **Unsigned.** Until a signed and notarized build exists, each build you make needs its permissions granted again if its signature changes: build with a stable signing identity ([build guide](../building/macos.md)).

Details for contributors: [macOS host reference](../reference/host-mac.md) and [the Mac app](../reference/mac-app.md).
