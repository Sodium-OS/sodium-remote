# Sodium Remote documentation

Sodium Remote is an open-source remote desktop: [README](../README.md) first, then the pages below. This documentation is licensed under CC-BY-SA-4.0.

## Using Sodium Remote

| Page | What it covers |
|---|---|
| [Install](install.md) | Getting Sodium Remote onto Windows, macOS and Linux, and updating it |
| [User guide](user-guide.md) | Pairing, connecting, displays, keyboard, clipboard, full screen, and the server settings |
| [Hosts](hosts/) | Being controlled: setup and limits for [Windows](hosts/windows.md), [macOS](hosts/macos.md), [SodiumOS](hosts/sodiumos.md) and [other Linux desktops](hosts/linux.md) |
| [Troubleshooting and FAQ](troubleshooting.md) | When it doesn't connect, and short answers |

## Networks and servers

| Page | What it covers |
|---|---|
| [Networking](networking.md) | Discovery, direct connections, hole punching, relays, ports and firewalls |
| [Self-hosting](self-hosting.md) | Running your own rendezvous and relay server |
| [Configuration](configuration.md) | Every configuration file, environment variable and data directory |

## How it works

| Page | What it covers |
|---|---|
| [Security](security.md) | The threat model, pairing, encryption, keys and what a server can see |
| [Architecture](architecture.md) | The crates, and how a frame gets from one computer to the other |
| [Compatibility](compatibility.md) | Which versions work together |
| [Design decisions](design-decisions.md) | What was chosen, and why |
| [Reference](reference/) | The protocol, the rendezvous API and the internals of each part |

## Building and contributing

| Page | What it covers |
|---|---|
| Building on [Linux](building/linux.md), [Windows](building/windows.md) and [macOS](building/macos.md) | From a fresh clone to a running binary |
| [Translating](translating.md) | The ten UI languages and how to add or fix a translation |
| [Releasing](releasing.md) | How maintainers make a release |
| [Contributing](../CONTRIBUTING.md), [CLA](../CLA.md), [Code of Conduct](../CODE_OF_CONDUCT.md) | How to take part |

## Project files

[Changelog](../CHANGELOG.md) · [Security policy](../SECURITY.md) · [Licensing](../LICENSING.md) · [Trademarks](../TRADEMARKS.md)

## Reference documents

These are the technical documents for people reading or changing the code.

- [Protocol](reference/protocol.md): ICE, the Noise session, every channel, congestion control
- [Rendezvous API](reference/rendezvous-api.md): the contract between devices and `sodium-remote-server`
- [Session API](reference/session-api.md): pairing and connecting through `remote-core`
- [Video pipeline](reference/video-pipeline.md): capture, encode, refinement and rate control on a SodiumOS host
- [Capture IPC](reference/capture-ipc.md): the SodiumOS compositor's contract with its host
- [SodiumOS host session](reference/host-session.md): the session lifecycle, displays, input and the D-Bus API
- [Input method and drag and drop](reference/ime-and-dnd.md)
- [Desktop host](reference/host-desktop.md) and [macOS host](reference/host-mac.md)
- [Qt client app](reference/client-app.md), [Linux client](reference/client-linux.md), [Windows client](reference/client-windows.md) and [Mac app](reference/mac-app.md)
