# Releasing

For maintainers. How a release of Sodium Remote is made, from a tag to the published files. Everything here happens in the canonical repository.

## What a release contains

- **Source**, tagged `vX.Y.Z`: `sodium-remote-X.Y.Z.tar.gz`, the tracked files plus the third-party notices of each program (GitHub's own source archives are there too, without the notices).
- **Windows:** a portable zip, `SodiumRemote-X.Y.Z-win64.zip`, built from the tag. Unsigned until a signing certificate exists.
- **Arch Linux:** a `PKGBUILD` and `sodium-remote.SRCINFO` with the source tarball's checksum filled in (the template is `packaging/arch/PKGBUILD`); `makepkg` next to the tarball builds the packages `sodium-remote` and `sodium-remote-server`.
- **macOS:** a disk image **only when a Developer ID** certificate is available to sign and notarize it. Until then the release says to build from source.
- **Checksums** (SHA-256) for every file, and **attestations** (build provenance) from GitHub Actions.
- **Third-party notices** for each binary, in the package.

## Before tagging

1. **Update the version.** The workspace version is in `Cargo.toml` (`[workspace.package]`). The Mac app's version comes from its `Info.plist`, filled in by `build-app.sh`; keep them in step.
2. **Write the changelog.** Move the `[Unreleased]` entries in `CHANGELOG.md` under the new version and date. Say when the control protocol or the rendezvous API changed ([compatibility](compatibility.md)).
3. **Check protocol compatibility.** If the control version changed, the new release must still speak the previous version (N and N−1), and the rendezvous API only grows.
4. **Check that CI is green on the commit you tag.** It runs `cargo build` and `cargo test`, `cargo deny check` (licenses and advisories; the exception for MPL-2.0 is the UniFFI crates in the Mac library), `reuse lint`, `tools/spdx-headers.py --check`, `i18n/i18n.sh check --strict`, the Qt app's tests and smoke test, the Windows cross build and the Arch package. The Mac job is manual: run it from the Actions tab.
5. **Pin the two checksums the release refuses to run without.** `host-desktop/src/openh264_pin.rs` holds two SHA-256 values for Cisco's OpenH264 DLL, which Windows downloads on first need (the `.bz2`, and the unpacked DLL, which the host checks again at every load): run `packaging/windows/pin-openh264.sh` on a networked machine and paste the two lines it prints. `packaging/windows/build-qt-ucrt.sh` holds the SHA-256 of Qt's source archives: run it with `--print-pins` and paste the result. Both are placeholders in the source until then, and the release workflow stops with a message naming the file.
6. **Check the docs** for anything the release changes: [install](install.md), [configuration](configuration.md), the platform table in the README.

## Tagging and building

Push the tag:

```sh
git tag -s v0.1.0 -m "Sodium Remote 0.1.0"
git push origin v0.1.0
```

The release workflow (`.github/workflows/release.yml`) runs on the tag:

- **Checks first.** The tag must be `vX.Y.Z` and equal `[workspace.package] version`; `CHANGELOG.md` must have a `## [X.Y.Z]` entry; the OpenH264 pin and Qt's source checksums must not be placeholders. Any failure stops the release before anything is built.
- **Windows:** the cross-build ([building on Windows](building/windows.md)) in a clean container: FFmpeg, ICU and Qt built from source on UCRT (cached by the hash of their scripts, so the first run is hours and later ones minutes), then `stage-app.sh`, the `check-no-msvcrt.sh` gate and the zip.
- **Third-party notices:** `cargo about generate` (`about.toml`, `about.hbs`) makes the list of Rust libraries for each program, for its own target: `THIRD-PARTY-NOTICES.md` in the Windows zip, `notices/THIRD-PARTY-NOTICES-app.md` and `-server.md` in the source tarball (the Arch packages install them under `/usr/share/doc/`), and in the Mac app's Resources. The Windows zip's `THIRD-PARTY-NOTICES.md` is followed by the full text of every other license (Qt, FFmpeg, the GCC and MinGW-w64 run-time, Mesa, ICU, Opus, the fonts), which `stage-app.sh` appends from `licenses/` and checks against the shipped DLLs. The release also carries the exact Qt and FFmpeg source tarballs as assets (LGPL).
- **Source and Arch files:** `packaging/arch/make-source-tarball.sh` makes `sodium-remote-X.Y.Z.tar.gz` (reproducible: the same commit gives the same checksum); the `PKGBUILD` from the tree is copied with the version and that checksum filled in, and `makepkg --printsrcinfo` writes `sodium-remote.SRCINFO`.
- **macOS:** a signed, notarized `Sodium-Remote-X.Y.Z.dmg`, only when the signing secrets are set in the repository (listed in [building on macOS](building/macos.md)). Without them the job is skipped and the release notes say to build from source.
- **Checksums:** a `SHA256SUMS` file for the release's files.
- **Attestations:** build provenance for each file (`actions/attest-build-provenance`), so that anyone can verify it came from the workflow at that tag: `gh attestation verify FILE --repo sodium-os/sodium-remote`.
- **A draft release** is created with the files, the changelog entry as its notes (and a short list of the files) and the checksums. Versions below 1.0 are marked as pre-releases. Nothing is published automatically.

The macOS build is manual while the repository is private (and until a Developer ID exists): on a Mac, `client-mac/build-app.sh`, then sign and notarize if you can.

## Publishing

1. Open the draft release. Read the notes, check the file list and the checksums.
2. Download the Windows zip, compare its SHA-256 with the checksum file, run it on a clean Windows 11 machine (SmartScreen will warn: the build is unsigned), pair it with another computer and connect.
3. Publish. If you can't sign a Windows or Mac build, say so in the notes.
4. Announce on the website's page ([sodiumos.com/remote](https://sodiumos.com/remote/)) and in the repository's release notes.

## After a release

- Update the platform table in the README if a platform's status changed.
- Update the website.
- Keep `[Unreleased]` in the changelog open for the next change.
- Sodium Remote has no automatic updater: users learn of releases by watching the repository. Say what changed, clearly, in the release notes.

## Security fixes

A security fix ships as a patch release and the notes credit the reporter unless they prefer not to be named ([SECURITY.md](../SECURITY.md)). Only the latest release is supported.
