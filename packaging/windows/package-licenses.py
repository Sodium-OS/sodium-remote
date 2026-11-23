#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""The license side of the Windows package.

    package-licenses.py check  PKGDIR     every binary in the package has its license text in PKGDIR/licenses, or it fails
    package-licenses.py append PKGDIR     appends the license texts to PKGDIR/THIRD-PARTY-NOTICES.md (made by cargo-about,
                                          the Rust libraries only), so the file holds the full text of every license

The table below is the one in packaging/windows/README.md ("Licenses of the shipped files"): keep them the same.
"""
import fnmatch, sys
from pathlib import Path

# file name pattern (matched against the path inside the package, lower case) -> license files (inside licenses/) it needs
RULES = [
    ("sodiumremote.exe", []),                       # Sodium Remote itself: AGPL-3.0, LICENSE in the package
    ("sodiumui/sodiumuinative.dll", []),
    ("qt6*.dll", ["Qt/LGPL-3.0-only.txt", "Qt/GPL-3.0-only.txt", "Qt-third-party-attributions.txt"]),
    ("platforms/*.dll", ["Qt/LGPL-3.0-only.txt", "Qt-third-party-attributions.txt"]),
    ("imageformats/*.dll", ["Qt/LGPL-3.0-only.txt", "Qt-third-party-attributions.txt"]),
    ("iconengines/*.dll", ["Qt/LGPL-3.0-only.txt", "Qt-third-party-attributions.txt"]),
    ("qml/*.dll", ["Qt/LGPL-3.0-only.txt", "Qt-third-party-attributions.txt"]),
    ("icu*.dll", ["ICU-LICENSE.txt"]),
    ("avcodec-*.dll", ["FFmpeg-LGPL-2.1.txt", "FFmpeg-LICENSE.md"]),
    ("avutil-*.dll", ["FFmpeg-LGPL-2.1.txt", "FFmpeg-LICENSE.md"]),
    ("swscale-*.dll", ["FFmpeg-LGPL-2.1.txt", "FFmpeg-LICENSE.md"]),
    ("libgcc_s_*.dll", ["GCC-Runtime-Library-Exception-3.1.txt", "GCC-runtime-GPL-3.0.txt"]),
    ("libstdc++-*.dll", ["GCC-Runtime-Library-Exception-3.1.txt", "GCC-runtime-GPL-3.0.txt"]),
    ("libwinpthread-*.dll", ["MinGW-w64-runtime-and-winpthreads.txt"]),
    ("opengl32sw.dll", ["Mesa-opengl32sw-NOTICE.txt"]),
]
# Linked into the program itself (static), so there is no DLL to match: their texts must be there too.
STATIC = ["opus-BSD-3-Clause.txt", "libvpl-MIT.txt", "nv-codec-headers-MIT.txt", "AMF-MIT.txt", "openh264-headers-BSD-2-Clause.txt",
          "OpenH264-Cisco-NOTICE.txt", "MinGW-w64-runtime-and-winpthreads.txt",
          "Inter-OFL.txt", "JetBrainsMono-OFL.txt", "NotoSansCJK-OFL.txt"]
# Never shipped: Microsoft's redistributable, which would need Microsoft's terms (Windows has it in System32).
FORBIDDEN = ["d3dcompiler_47.dll"]


def check(pkg: Path) -> int:
    lic, bad = pkg / "licenses", []
    need = {s: "linked into the program" for s in STATIC}
    for f in sorted(pkg.rglob("*")):
        if f.suffix.lower() not in (".dll", ".exe") or not f.is_file():
            continue
        rel = f.relative_to(pkg).as_posix().lower()
        if f.name.lower() in FORBIDDEN:
            bad.append(f"{rel}: must not be shipped (Microsoft's redistribution terms)")
            continue
        # Qt's QML plugins sit in qml/<module>/...: match by the first folder
        keys = [rel, rel.split("/", 1)[0] + "/*.dll" if "/" in rel else rel, f.name.lower()]
        files = next((fl for pat, fl in RULES if any(fnmatch.fnmatch(k, pat) for k in keys)), None)
        if files is None:
            bad.append(f"{rel}: no license rule for this file (add its text to licenses/ and a rule to package-licenses.py)")
            continue
        for t in files:
            need[t] = rel
    for t, why in sorted(need.items()):
        if not (lic / t).is_file():
            bad.append(f"licenses/{t} is missing ({why})")
    if not (pkg / "licenses" / "LICENSE").is_file():
        bad.append("licenses/LICENSE (Sodium Remote's own) is missing")
    for b in bad:
        print("package-licenses:", b, file=sys.stderr)
    if not bad:
        print(f"package-licenses: every shipped binary has its license text ({len(need)} texts needed)", file=sys.stderr)
    return 1 if bad else 0


def append(pkg: Path) -> int:
    notices = pkg / "THIRD-PARTY-NOTICES.md"
    if not notices.is_file():
        print(f"package-licenses: {notices} is missing (cargo-about writes it first)", file=sys.stderr)
        return 1
    base = notices.read_text(encoding="utf-8")
    marker = "\n\n# License texts of the other parts of this package\n"
    base = base.split(marker)[0].rstrip("\n")           # idempotent
    out = [base, marker.strip("\n") + "\n",
           "The Rust libraries are listed above. The licenses of everything else in this package (Qt, FFmpeg, Opus, ICU, the GCC",
           "and MinGW-w64 run-time, Mesa, the fonts and the codec headers) follow in full, one file each, the same files as in",
           "the `licenses` folder. `licenses/README.txt` says which file belongs to which DLL.\n"]
    lic = pkg / "licenses"
    for f in sorted(lic.rglob("*")):
        if not f.is_file() or f.name == "README.txt":
            continue
        out += [f"\n## licenses/{f.relative_to(lic).as_posix()}\n", "~~~~text", f.read_text(encoding="utf-8", errors="replace").rstrip("\n"), "~~~~"]
    notices.write_text("\n".join(out) + "\n", encoding="utf-8")
    print(f"package-licenses: {notices.name} now {notices.stat().st_size // 1024} KB", file=sys.stderr)
    return 0


if __name__ == "__main__" and len(sys.argv) == 3 and sys.argv[1] in ("check", "append"):
    sys.exit({"check": check, "append": append}[sys.argv[1]](Path(sys.argv[2])))
sys.exit(__doc__)
