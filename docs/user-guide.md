# User guide

How to pair computers, connect, and get the most out of a session. The app looks slightly different on each platform, but the words below are the ones it uses.

## The basics

Each computer has two roles. It can **control** other computers, and it can **be controlled**. One app does both. The app's main window lists the computers you have paired, with their state (online, asleep or offline), and the computers it finds nearby.

- A computer you want to control must have **Allow remote control** turned on. On Windows and Mac this is on the **This computer** page; on SodiumOS it is Settings › Remote.
- One device controls a computer at a time. If a second paired device connects, it takes over, and the first sees "Taken over by …". The first can take it back from its own window.

## Pairing two computers

You pair two computers once. The computer you want to control shows a code; the other types it.

1. On the computer to be controlled, make sure **Allow remote control** is on, then choose **Pair a device**. It shows a six-digit **pairing code**.
2. On your other computer, choose **Add a computer**. Computers on your network appear under "Found nearby · not paired yet". Pick one, or choose **Connect by address** and type its address (see below).
3. Type the code. The app says "Checking with the computer…" and then "… is paired".

A code works **once, for five minutes**, and **three wrong tries withdraw it**; choose **New code** for another. The code never leaves your devices: the two computers prove to each other that they know it without sending it, so a server in between can't learn it. See [security](security.md).

Pairing is mutual: each side stores the other's key, so each computer lists the other under **Paired devices** and can remove it (**Remove**, on the controlled computer; **Forget**, on the controlling one). Removing a device on the controlled computer disconnects it at once and it can't connect again until you pair again.

### Pairing and connecting by address

You can skip discovery by typing the other computer's address: a name or IP address, and a port if it isn't the default (47311), for example `192.0.2.10:47311`. Pairing by address works the same way, and the code is what authenticates the computer you reach. On the controlled computer, **Address for connecting directly** shows the address to give; **Copy full address** copies it with the computer's ID.

## Connecting

Double-click a computer (or select it and press Return, or choose **Connect**). The first picture usually appears in about a second on a local network. Each computer's card names what it runs: SodiumOS, Windows, macOS or Linux (any other Linux desktop).

While connected:

- The **bar** at the top of the stream shows how you're connected (LAN, Direct or Relay), the round-trip time and the frame rate. Its menus are **Displays**, **Keyboard** (key mapping and Send Ctrl+Alt+Del) and **Stats**, and it has buttons for **Settings**, **Full screen** and **Disconnect**.
- The **cursor** is drawn by your computer, so pointing doesn't wait for video.
- **Sound** from the other computer plays on yours.
- The **clipboard** is shared both ways: text and images.
- **Disconnect** ends the session.
- If the network drops, the app says "Reconnecting…", and keeps trying while the host holds the session (about two minutes). When the network returns, the stream resumes.

### Full screen and the keyboard

Choose **Full screen** (or **Connect in full screen**).

- **Leave the stream:** press Ctrl+Alt+Shift+Esc. The keyboard goes back to your computer and the window leaves full screen.
- **Send Ctrl+Alt+Del** is in the keyboard menu, for a Windows computer's sign-in.
- **Key mapping** (in the Keyboard menu and in Settings): with **Automatic**, ⌘ on a Mac acts as Ctrl on a PC (and the other way round), so copying and pasting feel the same. With **Pass through**, every key goes unchanged. Set it in Settings › Input (it applies at the next connection).
- **On a Mac client**, shortcuts that macOS would otherwise keep (⌘Tab, ⌘Space and others) reach the other computer only when you've allowed Accessibility. With a Mac input method active, you can type Chinese, Japanese and Korean with your own input method, and the text lands in the other computer's app. See [the macOS page](hosts/macos.md).

### Displays and quality

Settings › Display and Quality affect every computer, at the next connection.

- **Resolution.** A **SodiumOS** computer creates a display at the size and refresh rate of your window, so nothing is scaled; with **Match the window** it follows every resize. Windows and macOS computers show their own screens at their own resolution, and the picture is scaled to fit the window.
- **Which display.** When the other computer has several displays, choose which one to see from the display menu.
- **Refresh rate.** Automatic follows your display.
- **Video codec.** Automatic uses HEVC when both computers support it and H.264 otherwise. You can force one.
- **Bandwidth limit.** The stream never goes above it. Automatic adapts to the network.

### Secure screens

A Windows lock screen, sign-in screen or UAC prompt, and a Mac's lock screen, can't be shown or controlled remotely. The app says "… is showing a secure screen", and the session resumes when the desktop is back. On SodiumOS a paired device can connect to a locked desktop and you unlock it with your password inside the session.

## Being controlled

When someone is connected to your computer, an indicator shows who ("… is controlling this computer"), with **Disconnect** to end it. You can use the computer yourself at the same time unless the home monitor is set to ignore local input (SodiumOS).

- **Hide your screen.** The computer being controlled can hide its own screen while a session runs. A SodiumOS computer's hosting options include Privacy and Off for the monitor in front of you; Windows, Mac and other Linux computers show their own screens.
- **The session ends** when the controlling device disconnects, when you switch **Allow remote control** off (paired devices stay paired), or when you remove the device.
- See the page for your OS: [Windows](hosts/windows.md), [macOS](hosts/macos.md), [SodiumOS](hosts/sodiumos.md), [other Linux desktops](hosts/linux.md).

## Servers: finding computers away from home

On your own network, and by address, you don't need a server. To find and reach your computers from anywhere, point Sodium Remote at a server you run yourself ([self-hosting](self-hosting.md)). A hosted relay service at remote.sodiumos.com is planned; until then, use your own server or connect on your network or by address.

In **Settings › Server**:

- With no server, the page says so; everything on your network and by address still works.
- **Add a server** takes a `sodium-remote://server?…` link (paste it, or open it), or a server address with an optional CA file and enrolment token. The server's operator gives you the link; it carries the address, the CA to trust and, for a host, an enrolment token. If the link carries a CA, the app asks before it trusts it.
- The page shows whether the server answers, and any notice from its operator.
- **Change** or **Remove** the server at any time. Your pairings don't depend on it.

A computer you want to control, if the server requires it, needs an enrolment token to register as a host. The operator gives you one; it goes in the same place (it is part of the link). Your other computers don't need one.

## Privacy

- Sodium Remote has no telemetry and no automatic update check.
- **Show previews** (Advanced › Computers on the Mac) keeps the last picture of each computer on its card. Turn it off and no picture is saved or shown, and the saved ones are deleted.
- The server you configure sees metadata: which of your devices talk to each other, and when. It cannot see your screen, keyboard or clipboard. See [security](security.md).
