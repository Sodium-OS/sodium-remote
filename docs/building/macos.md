# Building for macOS

The Mac app is native Swift. It is built on a Mac, with the Command Line Tools and Rust. There is no signed build for 0.1.0, so this is how you get the app. The details are in [`client-mac/BUILD.md`](../../client-mac/BUILD.md); this page is the path from a clean Mac.

## What you need

1. **macOS 14 or later.** The app uses `CAMetalDisplayLink`, which needs it.
2. **The Command Line Tools** (Swift 5.9 or later, the macOS SDK, `codesign`): `xcode-select --install`. Full Xcode isn't needed. Metal shaders are compiled at run time, because the Command Line Tools have no Metal compiler. (`swift test` needs XCTest, which some Command Line Tools releases lack; install Xcode only if you want the tests.)
3. **Rust**, with the Mac's own target:
   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
   source ~/.cargo/env
   rustup target add aarch64-apple-darwin
   ```

## Build

```sh
git clone https://github.com/sodium-os/sodium-remote.git
cd sodium-remote/client-mac
./build-app.sh
```

The script builds `libremote_ffi.a` (the core and the Mac host together) with `host-desktop/build-swift.sh mac`, generates the Swift bindings from it so they always match, checks that every interface string is in all ten languages, runs `swift build`, makes `build/Sodium Remote.app` and signs it ad hoc.

Options: `./build-app.sh --test` also runs `swift test`; `--skip-rust` reuses the generated library for Swift-only changes.

## Run it

```sh
open "build/Sodium Remote.app"
```

macOS may show a Gatekeeper prompt if you copied the app from elsewhere: `xattr -dr com.apple.quarantine "build/Sodium Remote.app"`, or right-click and choose Open once. On first launch the welcome screen asks for Accessibility and Local Network. Screen Recording, which a Mac needs to be controlled, is covered on [the macOS host page](../hosts/macos.md). The Keychain asks once per build to let the app use its key: choose Always Allow.

### Stable permissions across rebuilds

An ad-hoc signature changes with every build, so macOS asks again for Accessibility and the Keychain asks again. A self-signed identity fixes both:

1. In Keychain Access, choose Certificate Assistant › Create a Certificate…, name it `Sodium Remote Dev`, type Code Signing.
2. Build with it: `REMOTE_SIGN_IDENTITY="Sodium Remote Dev" ./build-app.sh`.

### A test build next to your real one

`./build-app.sh --test-id` makes a build with bundle ID `com.sodiumos.remote.dev`, named "Sodium Remote (test)", with its own data folder (`~/Library/Application Support/com.sodiumos.remote.dev`), so it never touches an app you use.

## The notices file

About › Third-party notices opens `THIRD-PARTY-NOTICES.txt` from the app's Resources: the acknowledgements and a generated list of the Rust libraries. To include the list, make it first, from the repository's root:

```sh
cargo install cargo-about            # once
cargo about generate --manifest-path host-desktop/Cargo.toml --target aarch64-apple-darwin --output-file build/THIRD-PARTY-NOTICES-mac.md about.hbs
```

Without it, `build-app.sh` warns and the notices hold the acknowledgements only.

## Where things are

- Settings: `~/Library/Preferences/com.sodiumos.remote.plist`.
- Pairings and card pictures: `~/Library/Application Support/com.sodiumos.remote/` (the host's in `Host`); the keys are in the Keychain.
- Session statistics and logs: `~/Library/Logs/com.sodiumos.remote/`.

## If computers don't appear on the network

Check the Local Network permission (System Settings › Privacy & Security › Local Network). If discovery still fails, give the app the host's address: `defaults write com.sodiumos.remote RemoteLanHints -array '<HOST-ID>@<IP>:47311'`, then restart the app; or just connect by address.

## Checking a build

The first launch shows a welcome with two rows (Accessibility and Local Network). Then pair with a computer, connect on your network and check the first frame appears in about a second. [client-mac/BUILD.md](../../client-mac/BUILD.md) has the longer checklist (sharpness, keys, trackpad, clipboard, audio, network drops), and [the macOS host reference](../reference/host-mac.md) has the host's.

## What CI does

While the repository is private the Mac job is manual (**Actions › CI › Run workflow**): it checks that every string in `client-mac/App/Localizable.xcstrings` has all ten languages, then runs `./build-app.sh --test --test-id` on a macOS runner (the Rust library and the Swift bindings, `swift build`, `swift test`, the ad-hoc-signed test app). The release workflow builds a signed, notarized disk image only when the signing secrets exist (`MACOS_CERTIFICATE_P12_BASE64`, `MACOS_CERTIFICATE_PASSWORD`, `MACOS_SIGNING_IDENTITY`, `MACOS_NOTARY_APPLE_ID`, `MACOS_NOTARY_TEAM_ID`, `MACOS_NOTARY_PASSWORD`); without a Developer ID there is no Mac binary and the release notes say to build from source. See [releasing](../releasing.md).
