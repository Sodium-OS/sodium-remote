#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Test of xcstrings-to-lproj.py on small fixtures: `python3 Tools/test_xcstrings_to_lproj.py` (no Mac needed)."""
import json
import os
import subprocess
import sys
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "xcstrings-to-lproj.py")


def unit(v):
    return {"stringUnit": {"state": "translated", "value": v}}


def run(catalog):
    with tempfile.TemporaryDirectory() as d:
        src = os.path.join(d, "Localizable.xcstrings")
        out = os.path.join(d, "out")
        with open(src, "w", encoding="utf-8") as f:
            json.dump(catalog, f)
        p = subprocess.run([sys.executable, SCRIPT, src, out], capture_output=True, text=True)
        files = {}
        if os.path.isdir(out):
            for lang in os.listdir(out):
                with open(os.path.join(out, lang, "Localizable.strings"), encoding="utf-8") as f:
                    files[lang] = f.read()
        return p, files


class ConverterTests(unittest.TestCase):
    def test_plain_catalog_is_written(self):
        p, files = run({"sourceLanguage": "en", "version": "1.0", "strings": {
            "Hello %@": {"localizations": {"en": unit("Hello %@"), "de": unit("Hallo %@")}}}})
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertIn('"Hello %@" = "Hallo %@";', files["de.lproj"])
        self.assertIn('"Hello %@" = "Hello %@";', files["en.lproj"])

    def test_plural_variations_fail_and_name_the_key(self):
        p, files = run({"sourceLanguage": "en", "version": "1.0", "strings": {
            "%lld files": {"localizations": {"en": {"variations": {"plural": {"one": unit("%lld file"), "other": unit("%lld files")}}}}}}})
        self.assertNotEqual(p.returncode, 0)
        self.assertIn("%lld files", p.stderr)
        self.assertEqual(files, {})

    def test_device_variations_fail_even_beside_a_string_unit(self):
        p, files = run({"sourceLanguage": "en", "version": "1.0", "strings": {
            "Tap": {"localizations": {"en": {**unit("Tap"), "variations": {"device": {"mac": unit("Click")}}}}}}})
        self.assertNotEqual(p.returncode, 0)
        self.assertIn("Tap", p.stderr)
        self.assertEqual(files, {})


if __name__ == "__main__":
    unittest.main()
