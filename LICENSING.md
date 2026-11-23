# Licensing

Sodium Remote is available under two licenses. You choose one.

## 1. GNU Affero General Public License v3.0 only (AGPL-3.0-only)

The full text is in [LICENSE](LICENSE). Anyone may use, study, change and share Sodium Remote under it, **including for commercial purposes**. Nothing in the AGPL forbids charging for the software or for services built on it, or using it inside a company.

The conditions are the AGPL's own. In short:

- If you pass Sodium Remote, or a modified version of it, to other people, you give them the complete corresponding source code under the same license.
- If you modify it and let other people interact with your version over a network (for example, you run a modified relay or host as a service), you must offer those users the source of your modified version. This is section 13 of the AGPL. Sodium Remote shows a link to its source code in About and prints it with `--version` to make this easy.
- Anything that is a derived work of Sodium Remote, such as a product that includes or links its code, must be released under the AGPL as a whole.
- You keep the copyright notices and license texts, and you state your changes.
- There is no warranty.

Using Sodium Remote to control your own computers, at home or at work, does not oblige you to publish anything. You only have obligations when you distribute it or offer a modified version as a network service.

This summary is a convenience, not legal advice. The AGPL text is what binds you.

## 2. Commercial license

If you can't meet the AGPL's conditions, the copyright holder, Ning Bao, sells a commercial license that replaces them. You need it if, for example, you want to:

- include Sodium Remote's code in a closed-source product, or ship it in a product whose other parts you won't release under the AGPL;
- run a modified version as a network service without publishing your changes;
- distribute it under terms the AGPL doesn't allow.

You do not need it to use Sodium Remote as it is, or to modify and share it under the AGPL.

To ask about a commercial license, write to **enterprise@sodiumos.com**. The same address handles every other question about the project.

The commercial license is possible because contributors grant the copyright holder the right to relicense their contributions. See [CLA.md](CLA.md) and [CONTRIBUTING.md](CONTRIBUTING.md).

## The hosted relay service

A hosted relay service at remote.sodiumos.com is planned. It is a separate service with its own terms. It is not covered by either license above: using the AGPL software doesn't entitle you to the service, and the service isn't required to use the software. You can always run your own relay; see the self-hosting guide in `docs/`.

## Codecs and patents

**Patents held by the project and its contributors.** Section 11 of the AGPL gives every recipient a license to the patent claims of each contributor that the contributor's code needs, and the contributor license agreement (CLA.md) grants the same to the copyright holder.

**Video codec patents held by others.** Neither license grants a patent license for H.264, HEVC or other video codecs: those patents belong to third parties and their pools, and no open-source project can pass on a license it doesn't hold. Sodium Remote uses your platform's own codecs, or your hardware's encoders and decoders, wherever it can (VideoToolbox, the GPU vendors' encoders, your system's FFmpeg). Some builds can also use software codecs. Whether you need a patent license for a codec in your own use or product is your responsibility.

## Documentation, assets and third-party code

- The documentation (the `docs/` folder and other Markdown files) is licensed under [CC-BY-SA-4.0](LICENSES/CC-BY-SA-4.0.txt), unless a file says otherwise.
- Source files and assets, such as icons, are AGPL-3.0-only, except the app icon, which is covered by the trademark note below. [REUSE.toml](REUSE.toml) records the license of every path.
- Third-party libraries keep their own licenses. They are listed in THIRD-PARTY-NOTICES, which is generated for each release binary, and their license texts are in `packaging/windows/licenses/` and the Mac app's acknowledgements. The project's dependencies are checked to be permissive, apart from MPL-2.0 for the UniFFI crates in the Mac app's library.
- The vendored part of Sodium UI in `sodium-ui/` is AGPL-3.0-only like the rest.

## Trademarks

The names Sodium Remote, SodiumOS and Sodium UI, and the app icon, are trademarks and are not licensed by either license. See [TRADEMARKS.md](TRADEMARKS.md).
