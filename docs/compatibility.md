# Compatibility

Which versions of Sodium Remote work together, and how that is kept true.

## The two versions that matter

| What | Current | Rule |
|---|---|---|
| **Control protocol** (what two computers say to each other) | 6 | From version 6, every release speaks its own version **N** and the one before, **N−1** |
| **Rendezvous API** (what devices say to a server) | v1 | Changes only by **addition**: new fields, frames, error codes and requests. Devices ignore what they don't know |

The wire format underneath ("wire v3": the handshake and packets) is separate and changes only by a deliberate bump of its prologue.

## Computers: N and N−1

Each side tells the other the range of control versions it speaks (`min_version` to `version`). The host picks the highest version both speak and says so in its `Welcome`. If there is none, the connection is refused with a clear reason, and the app says which side to update: "It runs an older version that can't talk to this one. Update it, then try again." (or "needs an update"). Refusals work in both directions between any two versions, so there is never a silent timeout.

- **0.1.0 speaks only version 6**, because it is the first release with ranges. Earlier, private builds (version 5 and before) are refused in both directions, and they don't see each other on the local network either: the discovery name changed with the project's name.
- **The next release that changes the control protocol (version 7) will speak 6 and 7:** it decodes version 6's messages and answers a version 6 client with version 6.
- So a host and a client that are **one release apart keep working together**. Two releases apart may not: update the older one.

What changes in a control version are messages and fields in the control schema (display modes, key tables, new disconnect reasons). The reference documents each ([protocol](reference/protocol.md), "Changes").

## Servers and devices

The rendezvous API **v1** is additive. A newer server works with older devices (it adds fields they ignore), and a newer device works with an older server (and a server ignores fields it doesn't know). A server upgrade never requires a device upgrade, and the reverse.

Features that arrived later in v1, and what an older side does with them:

| Added | An older device or server |
|---|---|
| `account`, `notice`, `redirect` | Ignored |
| `relays` (regions) in the TURN credentials | The plain `urls` list still comes first for older devices |
| An enrolment token sent at registration | An older server ignores the field |
| New error codes (`token_invalid`, `token_expired`, `quota_exceeded`, `payment_required`) | A device that doesn't know a code can only report it |

## Pairings, keys and data

Pairings and keys carry across versions: a paired device stays paired when either side updates. The data directory keeps its layout. The first start of a new version that changed a directory name copies the old one across ([configuration](configuration.md)).

## Platforms

A Windows, macOS, SodiumOS or Linux computer on the same control version can talk to any other: the platform doesn't matter. A few things depend on the pair, such as the key table defaults (Mac to PC maps ⌘ and Ctrl; same-family pairs pass keys through) and whether a Windows or Mac host can resize its display (it can't; a SodiumOS host can).

## Untested or limited in 0.1.0

These are built, but only partly or not yet tried enough to promise:

- **Linux, graphics-card video through the desktop's sharing dialog.** Hardware video (VA-API with DMA-BUF pictures, or NVIDIA's encoder) hasn't been tested with the desktop's own screen sharing; software video is what has been tried.
- **Linux, typed text from another computer's input method.** Text arrives as key presses for what is on this computer's keyboard layout; characters it doesn't have may not arrive.
- **Linux, copying pictures and formatted text.** Plain text is copied both ways; pictures and formatted text haven't been tested.
- **Linux, the lock screen.** When the computer locks, the picture stops until someone unlocks it there, and the other computer isn't told why.
- **A computer's operating system on cards you haven't connected to yet.** Until a computer has connected once, its card doesn't say which system it runs.
- **Linux, sharing without the app open.** Sharing works only while the app is open or `sodium-remote-host` runs; there is no background mode, start at sign-in or tray icon yet.
- **Ctrl+Alt+Del on Linux computers.** The keyboard menu offers it for SodiumOS computers only.
- **Download size.** The Windows app includes full locale data for more languages than it shows.

## Releases and compatibility

Each release's [changelog](../CHANGELOG.md) says when the control protocol or the rendezvous API changed. Release notes name the protocol version.
