#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Flags user-visible text in the Swift sources that has no key in Localizable.xcstrings, so a string can't be shown
untranslated.

  lint-swift-strings.py App/Localizable.xcstrings Sources

It reads every string literal in Sources/**/*.swift and keeps the ones that look like words for a person:
`Text("Done")`, `Button("Cancel")`, `String(localized: "Open")`, a `LocalizedStringKey` argument, `title: "Mirror"`.
Each of those must be a key of the catalog (an interpolation `\\(x)` stands for the catalog's %@, %lld or %d).
`Text(verbatim: "…")` with letters is flagged too, unless the text is a product name, a unit or a file type.

What is skipped, on purpose:
  - literals that are not text: identifiers (`"RemoteHostsWindow"`), SF Symbol names, format-only strings, file
    extensions, URLs, log lines (`Log.info(...)`), user-defaults keys, selectors, queue labels;
  - the ALLOWED names below (product and technology names, units);
  - a line with `// i18n:ok`, and every line between `// i18n:off` and `// i18n:on` (support text such
    as the diagnostics, written in English on purpose).
Exit status 1 and one line per problem (file:line: the text) when something is missing.
"""
import json
import re
import sys
from pathlib import Path

# Words that are never translated: product, technology and OS names, units, file names.
ALLOWED = {
    "Sodium Remote", "SodiumOS", "Opus", "BSD-3-Clause", "Wi-Fi", "Ethernet", "Mac", "macOS", "Windows", "Linux", "LAN",
    "VPN", "Tailscale", "AV1", "AV1 4:2:0", "HEVC", "HEVC 4:2:0", "HEVC 4:4:4", "H.264 4:2:0", "AirDrop", "Handoff",
    "Globe", "PEM", "UAC", "toml", "pem", "crt", "cer", "png", "Hz", "px", "pt", "ms", "fps", "Mb/s", "kbps",
    "THIRD-PARTY-NOTICES", "txt", "MIT, Apache-2.0, others", "Remote",
    "AGPL-3.0 or commercial license", "relay.example.org", "sodium-remote://server?…", "∅",
}
UNIT_WORDS = {"ms", "s", "Hz", "kHz", "fps", "Mb", "kbps", "px", "pt", "x", "FEC", "QP", "C_est"}
SKIP_LINE = re.compile(
    r"Log\.|\blog\.|os_log|print\(|fatalError|precondition|assertionFailure|NSLog|systemName|systemSymbolName|forKey|"
    r"UserDefaults|Notification\.Name|#selector|Selector\(|DispatchQueue|AppIdentity\.|forResource|URL\(|kSec|"
    r"PasteboardType|fileURLWithPath|UTType|AutosaveName|setFrameUsingName|appendingPathComponent|"
    r"pathExtension|Key = \"|\bcase \"|import |SPDX|identifier|x-apple|label\(\"|\.write\(|\.hasPrefix|\.hasSuffix|"
    r"infoDictionary|AXTrusted|Array\(\"|static let license|\.contains\(|\.range\(|replacingOccurrences|ObjectIdentifier|\bdefaults\b|withExtension"
)
LITERAL = re.compile(r'"((?:[^"\\]|\\.)*)"')
SPEC = re.compile(r"%(?:\d+\$)?(?:@|l{0,2}d|\.\d+f|f|\.\df)")
_PAREN = r"[^()]*"
for _ in range(4):
    _PAREN = r"(?:[^()]|\(" + _PAREN + r"\))*"
INTERP = re.compile(r"\\\(" + _PAREN + r"\)")


def norm(s: str) -> str:
    """A literal or a key with every placeholder as one `%`."""
    return SPEC.sub("%", INTERP.sub("%", s))


def only_units(s: str) -> bool:
    """True for "% × % px", "%.1f Mb/s": placeholders, digits, symbols and unit words."""
    return all(w in UNIT_WORDS for w in re.findall(r"[A-Za-z_]+", norm(s)))


def looks_like_text(s: str) -> bool:
    if not re.search(r"[A-Za-z]{2}", s):
        return False
    t = norm(s).replace("%", "").strip()
    if not re.search(r"[A-Za-z]{2}", t):
        return False
    if " " not in t and re.fullmatch(r"[a-z0-9_.:/#|\-]+", t):  # an identifier or a path
        return False
    if " " not in t and re.fullmatch(r"[a-zA-Z0-9_.]+", t) and re.search(r"[a-z][A-Z0-9]|_", t) and not t[0].isupper():
        return False  # camelCase
    if re.fullmatch(r"[A-Z][A-Za-z]*[_.][A-Za-z_.]+", t):  # Privacy_Accessibility
        return False
    if t.startswith(("http", "/", "~", ".", "x-")) or "://" in t:
        return False
    return True


def check(cat: dict, root: Path) -> list:
    keys = {norm(k) for k in cat.get("strings", {})}
    problems = []
    for path in sorted(root.rglob("*.swift")):
        off = False
        for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if "// i18n:off" in line:
                off = True
            if "// i18n:on" in line:
                off = False
                continue
            code = line.split("//")[0] if "//" in line and '"' not in line.split("//")[0][-1:] else line
            if off or "// i18n:ok" in line or line.lstrip().startswith(("//", "///", "*")):
                continue
            if SKIP_LINE.search(line):
                continue
            for m in LITERAL.finditer(code):
                s = m.group(1)
                if not looks_like_text(s) or s in ALLOWED or only_units(s):
                    continue
                verbatim = re.search(r"verbatim:\s*$", code[: m.start()]) is not None
                if verbatim:
                    problems.append(f"{path}:{n}: Text(verbatim:) with words: {s!r}")
                elif norm(s) not in keys:
                    problems.append(f"{path}:{n}: no catalog key: {s!r}")
    return problems


def main(argv: list) -> int:
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    with open(argv[1], encoding="utf-8") as f:
        cat = json.load(f)
    problems = check(cat, Path(argv[2]))
    for p in problems:
        print("lint-swift-strings: " + p, file=sys.stderr)
    if not problems:
        print("lint-swift-strings: every visible string has a catalog key")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
