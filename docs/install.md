# Install

Sodium Remote 0.1.0 is a preview. This page covers installing it on each platform and keeping it up to date.

Install it on every computer you want to use, whether you'll control it, control others from it, or both: one app does both.

## Requirements

| Platform | Needs |
|---|---|
| Windows | Windows 11. The network you use must be set to **Private** in Windows network settings for the computer to be controlled (the app tells you when it isn't) |
| macOS | macOS 14 or later. Apple silicon is what the build scripts target |
| SodiumOS | Hosting needs an NVIDIA graphics card (the host encodes with NVENC) and SodiumOS's window manager, `sodiumwm`. Controlling other computers has no such need |
| Other Linux | A Wayland session for the client |

## Windows 11

1. Download `SodiumRemote-0.1.0-win64.zip` from the [Releases](https://github.com/sodium-os/sodium-remote/releases) page.
2. Check it against the SHA-256 checksum published with the release. In PowerShell: `Get-FileHash SodiumRemote-0.1.0-win64.zip`.
3. Unzip it anywhere you like. It is portable: there is no installer.
4. Run `SodiumRemote.exe`.

**SmartScreen.** The 0.1.0 build is not signed, so Windows SmartScreen shows "Windows protected your PC" the first time. Choose **More info**, then **Run anyway**. This is expected for an unsigned preview; checking the checksum first is how you know the file is the one we published. A signed build is planned.

**Allowing control.** In the app, open **This computer** and turn on **Allow remote control**. If Windows asks to allow the program through the firewall, allow it on private networks. The app can start when you sign in (**Start at login**) so the computer stays available.

The app keeps its files under `%LOCALAPPDATA%\SodiumRemote` and its configuration under `%APPDATA%\SodiumRemote`; see [configuration](configuration.md).

## macOS

There is no signed build for 0.1.0, so you build the app yourself. It takes a Mac with the Command Line Tools and Rust; [the build guide](building/macos.md) goes from a fresh clone to `build/Sodium Remote.app`.

On first launch macOS asks for permissions, in the app's welcome screen:

- **Accessibility**, to let your keyboard shortcuts such as ⌘Tab and ⌘Space reach the other computer, and, when the Mac is controlled, to let the other computer type and click.
- **Screen Recording**, when the Mac is controlled, to share its screen.
- **Local Network**, so computers on your network can be found. Without it, connect by address.

See [the macOS host page](hosts/macos.md).

## Arch Linux

Build the package from the `PKGBUILD` in `packaging/arch/`:

```sh
git clone https://github.com/sodium-os/sodium-remote.git
cd sodium-remote/packaging/arch
makepkg -s
sudo pacman -U sodium-remote-[0-9]*.pkg.tar.zst      # the app; add sodium-remote-server-* or sodium-remote-sodiumos-* if you want them
```

The `PKGBUILD` makes three packages from the release's source tarball. Install the ones you want (`makepkg -si` installs all three):

- `sodium-remote`: the Qt app (`sodium-remote`), its desktop entry, icon, translations and notices. The app can also host the computer itself from its **This computer** page, on GNOME and KDE Plasma 6.
- `sodium-remote-server`: the rendezvous server for [self-hosting](self-hosting.md), with a systemd unit.
- `sodium-remote-sodiumos`: the SodiumOS host, `remoted`, with its user service and the input-method add-on. Only useful on SodiumOS; see [the SodiumOS host page](hosts/sodiumos.md).

The standalone host program `sodium-remote-host` is not in a package; [build it from source](building/linux.md) if you want it.

## Other Linux

Build it from source: [the Linux build guide](building/linux.md). You need a Wayland session. A Linux desktop other than SodiumOS can be controlled too, on GNOME or KDE Plasma 6 (Wayland); see [the Linux host page](hosts/linux.md).

## SodiumOS

SodiumOS's Remote settings and the host's window-manager plug-in come with SodiumOS; the host itself, `remoted`, is the `sodium-remote-sodiumos` package above. Install it, turn Remote on in Settings › Remote and pair your first device. See [the SodiumOS host page](hosts/sodiumos.md).

## Updating

Sodium Remote has **no automatic updater** and sends no telemetry, not even a check for new versions. To know when a new version is out:

- watch the [Releases](https://github.com/sodium-os/sodium-remote/releases) page: on GitHub, choose Watch › Custom › Releases;
- or read the [changelog](../CHANGELOG.md).

To update, replace the app: unzip the new Windows zip over the old folder, rebuild on a Mac or Linux, or upgrade the package. Your pairings and keys live in your data directory, not the app folder, so they survive.

Computers on adjacent versions keep working together: from protocol version 6 every release also speaks the previous one ([compatibility](compatibility.md)). If a computer is too old to talk to a new one, the app says which side to update.

## Uninstalling

Delete the app, then (if you want to remove your pairings and keys too) the data directory listed in [configuration](configuration.md). On a Mac, also remove the app's Keychain items "Sodium Remote device key" and "Sodium Remote host key".
