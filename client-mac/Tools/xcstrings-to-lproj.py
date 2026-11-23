#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Localizable.xcstrings -> <language>.lproj/Localizable.strings, for build-app.sh.

The Command Line Tools have no xcstringstool and `swift build` doesn't compile string catalogs for a hand-made
.app, so this writes what Xcode would: one UTF-8 .strings file per language in the app's Resources. SwiftUI's
Text("...") and String(localized:) then find them in the main bundle.

Plural and device variations (and substitutions) can't be written to a .strings file. The converter never drops
them silently: a catalog entry that has any is an error, the message names the key (and the language), the exit
status is 1 and nothing is written.

  xcstrings-to-lproj.py Localizable.xcstrings "Sodium Remote.app/Contents/Resources"
"""
import json
import os
import sys

# Anything in an entry or one of its localizations that this converter can't write.
UNSUPPORTED = ("variations", "substitutions")


def quote(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\t", "\\t") + '"'


def problems(cat: dict) -> list:
    """Every entry this converter can't convert faithfully, as printable lines (empty when the catalog is fine)."""
    found = []
    for key, entry in sorted(cat.get("strings", {}).items()):
        for field in UNSUPPORTED:
            if field in entry:
                found.append(f"{key!r} has {field} (not supported by this converter)")
        for lang, loc in sorted(entry.get("localizations", {}).items()):
            for field in UNSUPPORTED:
                if field in loc:
                    found.append(f"{key!r} has {field} in {lang} (not supported by this converter)")
            if "stringUnit" not in loc and not any(f in loc for f in UNSUPPORTED):
                found.append(f"{key!r} has no stringUnit in {lang} (not supported by this converter)")
    return found


def convert(cat: dict, out: str) -> int:
    source = cat.get("sourceLanguage", "en")
    langs = {source}
    for entry in cat["strings"].values():
        langs.update(entry.get("localizations", {}))
    for lang in sorted(langs):
        lines = []
        for key, entry in sorted(cat["strings"].items()):
            loc = entry.get("localizations", {}).get(lang)
            if loc is None:
                value = key if lang == source else None
            else:
                value = loc["stringUnit"]["value"]
            if value is not None:
                lines.append(f"{quote(key)} = {quote(value)};")
        d = os.path.join(out, f"{lang}.lproj")
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, "Localizable.strings"), "w", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")
    return len(langs)


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    src, out = sys.argv[1], sys.argv[2]
    with open(src, encoding="utf-8") as f:
        cat = json.load(f)
    bad = problems(cat)
    if bad:
        for line in bad:
            print(f"{src}: {line}", file=sys.stderr)
        return 1
    n = convert(cat, out)
    print(f"{len(cat['strings'])} strings, {n} languages")
    return 0


if __name__ == "__main__":
    sys.exit(main())
