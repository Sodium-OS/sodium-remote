# Contributing to Sodium Remote

Thank you for helping. This page covers how to build, what to sign, and the rules a change has to follow.

By taking part you agree to the [Code of Conduct](CODE_OF_CONDUCT.md). For security problems, see [SECURITY.md](SECURITY.md) and don't open a public issue.

## Building

Build instructions for each platform are in [docs/building/](docs/building/). Start with the page for your platform, and the Rust workspace at the repository root, the Qt app in `app/`, the Mac app in `client-mac/` and the packaging in `packaging/`.

## License and the CLA

Sodium Remote is [AGPL-3.0-only](LICENSE), and the copyright holder also sells a commercial license (see [LICENSING.md](LICENSING.md)). To make that possible, every contributor signs the [Contributor License Agreement](CLA.md). You keep your copyright; you grant the right to relicense your contribution. The CLA bot asks for your signature on your first pull request, once.

New source files carry SPDX headers. Run `tools/spdx-headers.py` to add them, and `tools/spdx-headers.py --check` to verify.

## Commits and pull requests

- Keep each pull request to one change. Describe what changed and why.
- Commit messages: a short summary line in the imperative mood ("Fix reconnect after sleep"), then a blank line and the details if needed. No trailers are required.
- Make sure the project builds and the tests pass on the platforms your change touches.

## Tests

Write only the tests needed to cover the primary happy path and the critical boundary cases. Don't add speculative, redundant or defensive tests for behaviour that wasn't asked for.

## The ten UI languages

Every screen supports all ten UI languages: English (US, UK and Australia), Chinese (Simplified and Traditional), Japanese, Korean, Spanish, French and German. Each app can run in its own language. Read [docs/translating.md](docs/translating.md) before you write or change any UI. In short:

- Every visible string goes through `qsTr` (QML), `tr` (C++) or `String(localized:)` / `LocalizedStringKey` (Swift). That includes tooltips, accessible names, placeholders, notifications and error states.
- Write the source string in US English, as a whole sentence, with `%1` or `%n` for values and counts. Never join pieces of text, and never pick between "item" and "items" yourself; use `%n`.
- Never use text as data. Compare ids, enums or roles, not translated strings, and never store translated text. Store an id and its arguments, and translate when shown.
- Dates, times and numbers go through Sodium UI's formatters (`Format.*`), never by concatenation.
- Layouts size to their content. Don't give text a fixed width, and don't elide text, except in sidebars and table headers.
- Design new screens with their tightest parts in German (about 30 percent longer than English) and Japanese (taller lines) in mind.

## Translations

Translations live in the catalogs under `i18n/` (and `Localizable.xcstrings` for the Mac app). Fixes to a translation are welcome as pull requests.
