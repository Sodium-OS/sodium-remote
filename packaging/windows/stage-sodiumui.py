#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Stages a cross-built Sodium UI program for Windows: what windeployqt does, by hand, plus Sodium UI's
own parts. Called by build-sodiumui.sh; usable on its own:

    stage-sodiumui.py --build DIR --out DIR --qt QTROOT --deps WIN_DEPS --repo REPO [--exe NAME]...

Layout written to --out (docs: remote/packaging/windows/README.md):

    sodiumui-gallery.exe   + every DLL it needs (Qt, ICU, GCC's runtime), found by reading the PE import tables
    platforms/ imageformats/ iconengines/ styles?  Qt plugins the program needs
    qml/                   only the Qt QML modules the Sodium UI and gallery QML import (qmlimportscanner)
    SodiumUI/              the module: qmldir, QML, sodiumuinative.dll, theme.default.json
    gallery/               the gallery pages
    fonts/                 Inter, JetBrains Mono, Noto Sans CJK (OFL), loaded by the plugin at start
    data/icons/SodiumOS/   the icon theme (Qt's GenericDataLocation on Windows includes <app>/data)
    translations/          sodiumui_<lang>.qm
    licenses/              the fonts', ICU's and Qt's license texts: Qt/ (every text in Qt's own LICENSES folder, which
                           the host's qt6-base package carries) and Qt-third-party-attributions.txt (made here from Qt's
                           SBOM files). Missing Qt texts, Qt SBOMs or font licenses stop the staging.
"""
import argparse, json, os, re, shutil, subprocess, sys
from pathlib import Path

TRIPLET = "x86_64-w64-mingw32"
FONT_SOURCES = {
    # destination name: candidate source paths (first that exists)
    "Inter.ttc": ["/usr/share/fonts/inter/Inter.ttc"],
    "JetBrainsMono-Regular.ttf": ["/usr/share/fonts/TTF/JetBrainsMono-Regular.ttf"],
    "JetBrainsMono-Medium.ttf": ["/usr/share/fonts/TTF/JetBrainsMono-Medium.ttf"],
    "JetBrainsMono-SemiBold.ttf": ["/usr/share/fonts/TTF/JetBrainsMono-SemiBold.ttf"],
    "JetBrainsMono-Bold.ttf": ["/usr/share/fonts/TTF/JetBrainsMono-Bold.ttf"],
    # one TTC holds SC, TC, HK, JP and KR; the weights Theme.font asks for (400, 500, 700)
    "NotoSansCJK-Regular.ttc": ["/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc"],
    "NotoSansCJK-Medium.ttc": ["/usr/share/fonts/noto-cjk/NotoSansCJK-Medium.ttc"],
    "NotoSansCJK-Bold.ttc": ["/usr/share/fonts/noto-cjk/NotoSansCJK-Bold.ttc"],
}
FONT_LICENCES = {
    "Inter-OFL.txt": "/usr/share/licenses/inter-font/LICENSE.txt",
    "JetBrainsMono-OFL.txt": "/usr/share/licenses/ttf-jetbrains-mono/LICENSE",
    "NotoSansCJK-OFL.txt": "/usr/share/licenses/noto-fonts-cjk/LICENSE",
}
QT_PLUGINS = ["platforms/qwindows.dll", "imageformats/qsvg.dll", "imageformats/qjpeg.dll", "imageformats/qgif.dll",
              "imageformats/qico.dll", "iconengines/qsvgicon.dll"]
# QML modules the scanner might not follow into (a module's own imports, a sub-folder's plugin), named so the package
# always has them: the app's file choosers (app/qml/ServerFiles.qml) are Qt Quick Dialogs, which load their own
# plugin (qtquickdialogsplugin.dll), the Quick implementation of the dialogs (quickimpl) and the dialogs' utilities.
# Each is copied with its plugin DLLs; the DLL closure below then brings Qt6QuickDialogs2*.dll and the rest.
# Only with --qml-dir (the app): the gallery has no file chooser.
EXTRA_QML_MODULES = ["QtQuick/Dialogs", "QtQuick/Dialogs/quickimpl"]
SYSTEM_DLLS_PREFIXES = ("api-ms-win-",)
# Qt's license texts: Qt's own LICENSES folder, as the Linux qt6-base package installs it (the Windows Qt archives
# carry none). LGPL-3.0 and GPL-3.0 must be there, and the file the attributions list refers to by LicenseRef.
QT_LICENSES_DEFAULT = "/usr/share/licenses/qt6-base"
QT_LICENSES_REQUIRED = ["LGPL-3.0-only.txt", "GPL-3.0-only.txt"]
# The modules whose DLLs are shipped (qtshadertools is a build tool here: Qt6ShaderTools.dll is not in the package).
QT_SBOM_MODULES = ["qtbase", "qtdeclarative", "qtsvg"]
# SBOM packages that are build or test tools, other platforms' code or databases, not in the Windows package.
QT_SBOM_SKIP = ("Bootstrap", "Test", "QSQLite", "DBus_", "Platform_", "CMake", "Compiler", "Linker", "WrapRt", "WrapAtomic",
                "PostgreSQL", "ODBC", "Oracle", "Interbase", "Mimer")


def log(*a):
    print("stage:", *a, file=sys.stderr)


def copy(src, dst):
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)


def imports(path):
    out = subprocess.run([f"{TRIPLET}-objdump", "-p", str(path)], capture_output=True, text=True, check=True).stdout
    return [l.split("DLL Name:")[1].strip() for l in out.splitlines() if "DLL Name:" in l]


def sbom_attributions(spdx):
    """(display name, license, source, copyright) of the third-party parts a Qt module's SPDX file lists."""
    text = spdx.read_text(encoding="utf-8", errors="replace")
    found = []
    for m in re.finditer(r"^PackageName: (.*?)\n(.*?)(?=^PackageName: |\Z)", text, re.S | re.M):
        name, body = m.group(1).strip(), m.group(2)
        if name.startswith(QT_SBOM_SKIP) or "Private" in name or not ("_Attribution_" in name or name.startswith("Bundled")):
            continue
        field = lambda k: (re.search(rf"^{k}: (.*)$", body, re.M) or [None, ""])[1].strip()
        lic = field("PackageLicenseConcluded")
        if lic in ("", "NOASSERTION"):
            continue
        cr = re.search(r"^PackageCopyrightText: (<text>.*?</text>|[^\n]*)", body, re.S | re.M)
        cr = re.sub(r"</?text>", "", cr.group(1)).strip() if cr else ""
        disp = name.split("_Attribution_")[-1] if "_Attribution_" in name else re.sub(r"^Bundled", "", name)
        found.append((disp, lic, field("PackageDownloadLocation"), cr))
    return found


def stage_qt_licenses(out, lic_dir, sbom_dirs):
    """Qt's license texts (all of Qt's LICENSES folder) and the attributions of the Qt parts that carry third-party code."""
    for need in QT_LICENSES_REQUIRED:
        if not (lic_dir / need).is_file():
            sys.exit(f"stage: Qt's license text {need} is not in {lic_dir} (install qt6-base, or give --qt-licenses)")
    for f in sorted(lic_dir.glob("*.txt")):
        copy(f, out / "licenses" / "Qt" / f.name)
    entries, used = {}, []
    for mod in QT_SBOM_MODULES:
        spdx = next((p for d in sbom_dirs if d.is_dir() for p in sorted(d.glob(f"{mod}-*.spdx")) if ".source." not in p.name), None)
        if spdx is None:
            sys.exit(f"stage: no SBOM (*.spdx) of {mod} in {[str(d) for d in sbom_dirs]}: Qt's third-party attributions cannot be made")
        used.append(spdx.name)
        for disp, lic, src, cr in sbom_attributions(spdx):
            entries.setdefault((disp, lic), (src, cr))
    lines = ["Qt 6.12: third-party parts and the licenses of Qt itself", "=" * 56, "",
             "Qt (qtbase, qtdeclarative, qtsvg) is used under the GNU Lesser General Public License version 3 (LGPL-3.0-only,",
             "Qt/LGPL-3.0-only.txt, which refers to Qt/GPL-3.0-only.txt). The Qt libraries are the DLLs beside the program and",
             "can be replaced by your own build of the same Qt version. The exact sources are release assets of the same",
             "release as this package (licenses/README.txt says where). Qt's own files carry the copyright of The Qt Company Ltd. and other",
             "contributors.", "",
             "Qt contains the following third-party code (taken from Qt's own SBOM files: " + ", ".join(used) + ").",
             "The license ids are SPDX ids; each text is in the Qt/ folder next to this file, as <id>.txt",
             "(LicenseRef-X as Qt/LicenseRef-X.txt). Some parts are not linked into every DLL.", ""]
    for (disp, lic), (src, cr) in sorted(entries.items(), key=lambda kv: kv[0][0].lower()):
        lines += [f"* {disp}", f"  License: {lic}"]
        if src and src != "NOASSERTION":
            lines.append(f"  Source:  {src}")
        for c in cr.splitlines():
            if c.strip():
                lines.append("  " + c.strip())
        lines.append("")
    (out / "licenses" / "Qt-third-party-attributions.txt").write_text("\n".join(lines), encoding="utf-8")
    log(f"Qt licenses: {len(list((out / 'licenses' / 'Qt').glob('*.txt')))} texts, {len(entries)} attributions")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--build", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--qt", required=True, type=Path)
    ap.add_argument("--deps", required=True, type=Path)
    ap.add_argument("--repo", required=True, type=Path)
    ap.add_argument("--exe", action="append", default=None)
    ap.add_argument("--host-qt", default="/usr/lib/qt6", type=Path)
    ap.add_argument("--exe-from", type=Path, help="where the programs are (default --build)")
    ap.add_argument("--qml-dir", type=Path, action="append", default=[], help="more QML to scan for Qt modules (an app's)")
    ap.add_argument("--no-gallery", action="store_true", help="the package is an app, not the gallery")
    ap.add_argument("--catalog", action="append", default=[], help="translation catalogs to carry besides sodiumui (e.g. remote)")
    ap.add_argument("--qt-licenses", type=Path, default=Path(QT_LICENSES_DEFAULT), help="Qt's LICENSES folder")
    ap.add_argument("--qt-sbom", type=Path, action="append", default=[], help="folders with Qt's *.spdx files (default <qt>/sbom)")
    ap.add_argument("--icons", type=Path, help="built SodiumOS icon theme (themes/icons/build/SodiumOS)")
    a = ap.parse_args()
    if a.exe is None:
        a.exe = ["sodiumui-gallery.exe"]
    out, qt, repo = a.out, a.qt, a.repo
    ui = repo / "sodium-ui"   # the vendored copy (<repo> is the Sodium Remote tree)

    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)

    # --- programs and the module
    for exe in a.exe:
        copy((a.exe_from or a.build) / exe, out / exe)
    mod = out / "SodiumUI"
    mod.mkdir()
    for f in (ui / "SodiumUI").iterdir():
        if f.suffix in (".qml", ".js") or f.name in ("qmldir", "theme.default.json"):
            copy(f, mod / f.name)
    copy(a.build / "SodiumUI" / "sodiumuinative.dll", mod / "sodiumuinative.dll")
    if not a.no_gallery:
        for f in (ui / "gallery").glob("*.qml"):
            copy(f, out / "gallery" / f.name)

    # --- Qt QML modules the QML imports
    qml_files = [str(p) for p in list((ui / "SodiumUI").glob("*.qml")) + ([] if a.no_gallery else list((ui / "gallery").glob("*.qml")))]
    for d in a.qml_dir:
        qml_files += [str(p) for p in d.glob("*.qml")]
    scan = subprocess.run([str(a.host_qt / "qmlimportscanner"), "-importPath", str(qt / "qml"),
                           "-importPath", str(ui), "-qmlFiles", *qml_files], capture_output=True, text=True, check=True)
    modules = [m for m in json.loads(scan.stdout) if m.get("type") == "module" and m.get("path")]
    qmlroot = (qt / "qml").resolve()
    copied = set()
    for m in modules:
        p = Path(m["path"]).resolve()
        if p in copied or not str(p).startswith(str(qmlroot)):
            continue
        copied.add(p)
        for f in p.iterdir():
            if f.is_file() and f.suffix in (".dll", ".qml", ".js", ".mjs", ".qmltypes", "") and not f.name.endswith(".debug"):
                copy(f, out / "qml" / p.relative_to(qmlroot) / f.name)
    for rel in EXTRA_QML_MODULES if a.qml_dir else []:
        p = (qmlroot / rel).resolve()
        if p in copied:
            continue
        if not p.is_dir():
            sys.exit(f"stage: the Qt QML module {rel.replace('/', '.')} is missing from {qmlroot} (the app's file choosers need it)")
        copied.add(p)
        for f in p.iterdir():
            if f.is_file() and f.suffix in (".dll", ".qml", ".js", ".mjs", ".qmltypes", "") and not f.name.endswith(".debug"):
                copy(f, out / "qml" / p.relative_to(qmlroot) / f.name)
    log(f"{len(copied)} Qt QML modules")

    # --- Qt plugins
    for rel in QT_PLUGINS:
        copy(qt / "plugins" / rel, out / rel)

    # --- DLL closure (what windeployqt does with the import tables)
    search = [qt / "bin", a.deps / "bin", Path(f"/usr/{TRIPLET}/bin")]
    index = {}
    for d in search:
        if d.is_dir():
            for f in d.glob("*.dll"):
                index.setdefault(f.name.lower(), f)
    queue = [p for p in out.rglob("*") if p.suffix in (".dll", ".exe")]
    seen = {p.name.lower() for p in queue}
    while queue:
        cur = queue.pop()
        for dll in imports(cur):
            key = dll.lower()
            if key in seen or key.startswith(SYSTEM_DLLS_PREFIXES):
                continue
            src = index.get(key)
            if src is None:
                continue                       # a Windows system DLL (kernel32, d3d11, ...)
            seen.add(key)
            copy(src, out / src.name)
            queue.append(out / src.name)
    # run-time loaded, never imported: the software OpenGL fallback. d3dcompiler_47.dll is not shipped: Windows 10 and 11
    # carry it in System32 (Microsoft's redistribution terms then do not apply) and Qt loads it from there.
    if (qt / "opengl32sw.dll").exists():
        copy(qt / "opengl32sw.dll", out / "opengl32sw.dll")

    # --- fonts, icons, translations, licenses
    for name, cands in FONT_SOURCES.items():
        src = next((Path(c) for c in cands if Path(c).exists()), None)
        if src is None:
            sys.exit(f"stage: font not found: {name} (looked in {cands})")
        copy(src, out / "fonts" / name)
    for name, src in FONT_LICENCES.items():
        if not Path(src).exists():
            sys.exit(f"stage: the license of a shipped font is missing: {src} (for {name})")
        copy(Path(src), out / "licenses" / name)
    stage_qt_licenses(out, a.qt_licenses, a.qt_sbom or [qt / "sbom"])
    icons = a.icons or (repo / "app/data/icons/sodium-remote")   # the icons the app uses, carried in the repository
    if not icons.is_dir():
        sys.exit(f"stage: the icon theme folder is missing ({icons})")
    dest = out / "data/icons/SodiumOS"
    for sub in ("symbolic", "scalable"):
        if not (icons / sub).is_dir():
            continue
        shutil.copytree(icons / sub, dest / sub, symlinks=False, ignore_dangling_symlinks=True)
    copy(icons / "index.theme", dest / "index.theme")
    qm = repo / "build/i18n/qm"   # i18n/i18n.sh release
    for cat in ["sodiumui"] + a.catalog:
        for f in sorted(qm.glob(f"{cat}_*.qm")):
            copy(f, out / "translations" / f.name)
    here = Path(__file__).resolve().parent
    for f in (here / "licenses").glob("*"):
        if f.name.startswith("ICU"):
            copy(f, out / "licenses" / f.name)

    # --- strip our own binaries (Qt's and ICU's are shipped as built)
    for p in [out / e for e in a.exe] + [mod / "sodiumuinative.dll"]:
        subprocess.run([f"{TRIPLET}-strip", "--strip-unneeded", str(p)], check=True)

    total = sum(f.stat().st_size for f in out.rglob("*") if f.is_file())
    log(f"staged {out} ({total / 1e6:.0f} MB)")


if __name__ == "__main__":
    main()
