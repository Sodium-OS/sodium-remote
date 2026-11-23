#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Checks Localizable.xcstrings: every key has a non-empty translation in all ten UI languages, and each translation
keeps the placeholders (%@, %lld, %1$@ ...) of the English text. Exits 1 and names each problem (key and language).

  check-xcstrings-languages.py App/Localizable.xcstrings

build-app.sh runs it before assembling the app.
"""
import json
import re
import sys

LANGUAGES = ["en", "en-GB", "en-AU", "zh-Hans", "zh-Hant", "ja", "ko", "es", "fr", "de"]
PLACEHOLDER = re.compile(r"%(?:\d+\$)?(?:l{0,2}d|@|f|\.\d+f)")


def placeholders(text: str) -> list:
    """The placeholder kinds, sorted; %1$@ and %@ are the same kind (a translation may reorder with positions)."""
    return sorted(re.sub(r"%\d+\$", "%", p) for p in PLACEHOLDER.findall(text))


def check(cat: dict) -> list:
    problems = []
    for key, entry in sorted(cat.get("strings", {}).items()):
        locs = entry.get("localizations", {})
        want = placeholders(key)
        for lang in LANGUAGES:
            unit = locs.get(lang, {}).get("stringUnit")
            value = (unit or {}).get("value", "")
            if not value.strip():
                problems.append(f"{key!r}: no {lang} translation")
                continue
            if placeholders(value) != want:
                problems.append(f"{key!r}: {lang} has different placeholders than the key")
    return problems


def main(argv: list) -> int:
    if len(argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    with open(argv[1], encoding="utf-8") as f:
        problems = check(json.load(f))
    for p in problems:
        print("check-xcstrings-languages: " + p, file=sys.stderr)
    if not problems:
        print("check-xcstrings-languages: every key is in all ten languages")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
