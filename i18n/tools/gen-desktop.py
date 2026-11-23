#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Desktop entries and the catalogs: the English Name/GenericName/Comment/Keywords and
[Desktop Action] Names are the source; every translation comes from the `desktop-entries` catalog.

    gen-desktop.py extract OUT.cpp [--seed SEED.tsv] [FILE.desktop ...]
    gen-desktop.py generate OUTDIR [--ts-dir DIR] [FILE.desktop ...]

extract    a C++ source of QT_TRANSLATE_NOOP calls for lupdate, context `Desktop.<desktop-file-id>`
           (the file name without ".desktop"); Keywords go in as one message (disambiguation
           "keywords": a semicolon list that translators write freely). --seed saves the existing
           [zh_CN] values for the zh_CN translators: msg context, key, English, zh_CN.
generate   translated copies of the files in OUTDIR with Name[xx]= etc. from i18n/ts/
           remote_<lang>.ts (finished messages only). A catalog translation replaces the
           file's own [xx] line for the nine languages; until the catalog has one, the file's line
           stays (the hand-written [zh_CN] values). The English lines are untouched.

With no FILE, the app's (app/*.desktop) is used. The strings live in the app's own catalog, `remote`.
"""
import glob
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import tslib  # noqa: E402

KEYS = ("Name", "GenericName", "Comment", "Keywords")
_KEY = re.compile(r"^(Name|GenericName|Comment|Keywords)(?:\[([^\]]+)\])?\s*=(.*)$")
_GROUP = re.compile(r"^\[(.+)\]\s*$")



# Decision 101: the office apps' names avoid the folder's word and the common noun, so their keywords carry them.
OFFICE_KEYWORDS = {"Desktop.org.sodiumos.Docs", "Desktop.org.sodiumos.Sheets", "Desktop.org.sodiumos.Slides"}
OFFICE_KEYWORDS_NOTE = ("include the Documents folder's word and the common noun the app's name avoided, the category word "
                        "(word processor, spreadsheet, presentation), how people in your language write Word, Excel and "
                        "PowerPoint, and for Japanese the kana readings of the app's name")

def default_files(repo=tslib.REPO):
    files = []
    for pattern in ("app/*.desktop",):
        for f in sorted(glob.glob(os.path.join(repo, pattern), recursive=True)):
            if "build" not in os.path.relpath(f, repo).split(os.sep):
                files.append(f)
    return files


def entry_id(path):
    return re.sub(r"\.desktop$", "", os.path.basename(path))


def is_english_keyword(word):
    return word.isascii()


def parse_entries(text):
    """[(group, key, locale or None, value)] for the translatable keys in the entry and action groups."""
    out, group = [], None
    for line in text.splitlines():
        g = _GROUP.match(line)
        if g:
            group = g.group(1)
            continue
        m = _KEY.match(line)
        if m and group and (group == "Desktop Entry" or group.startswith("Desktop Action ")):
            out.append((group, m.group(1), m.group(2), m.group(3).strip()))
    return out


def group_note(group, key):
    if group == "Desktop Entry":
        return f"{key} of the app" + ("; app names are translated, product names are not" if key == "Name" else "")
    return f'{key} of the action "{group[len("Desktop Action "):]}"'


def source_messages(path):
    """[(context, text, disambiguation, note, group, key)] from one file's unlocalised lines."""
    with open(path, encoding="utf-8") as f:
        entries = parse_entries(f.read())
    ctx = f"Desktop.{entry_id(path)}"
    out = []
    for group, key, loc, value in entries:
        if loc is not None or not value:
            continue
        if key == "Keywords":
            words = [w for w in value.split(";") if w and is_english_keyword(w)]
            if words:
                note = "semicolon-separated search words; write your own list, ending in a semicolon"
                if ctx in OFFICE_KEYWORDS:
                    note += "; " + OFFICE_KEYWORDS_NOTE
                out.append((ctx, ";".join(words) + ";", "keywords", note, group, key))
        else:
            out.append((ctx, value, "", group_note(group, key), group, key))
    return out


def zh_seed(path):
    """[(context, key, English, zh_CN)] from the [zh_CN] lines (Chinese keywords: the non-ASCII words)."""
    with open(path, encoding="utf-8") as f:
        entries = parse_entries(f.read())
    ctx = f"Desktop.{entry_id(path)}"
    plain = {(g, k): v for g, k, loc, v in entries if loc is None}
    rows = []
    for group, key, loc, value in entries:
        if loc != "zh_CN":
            continue
        name = key if group == "Desktop Entry" else f'{key} ({group[len("Desktop Action "):]})'
        rows.append((ctx, name, plain.get((group, key), ""), value))
    for group, key, loc, value in entries:
        if loc is None and key == "Keywords":
            zh = [w for w in value.split(";") if w and not is_english_keyword(w)]
            if zh:
                rows.append((ctx, "Keywords", ";".join(w for w in value.split(";") if w and is_english_keyword(w)) + ";",
                             ";".join(zh) + ";"))
    return rows


def render_cpp(files):
    lines = ["// Generated by i18n/tools/gen-desktop.py extract from the .desktop files. Not compiled:",
             "// it only feeds lupdate (the app's catalog `remote`). Do not edit.", ""]
    seen = {}
    for path in files:
        for ctx, text, disamb, note, _g, _k in source_messages(path):
            k = (ctx, text, disamb)
            if k in seen:
                if note not in seen[k]:
                    seen[k].append(note)
                continue
            seen[k] = [note]
    for (ctx, text, disamb), notes in seen.items():
        for n in notes:
            lines.append("//: " + n)
        lines.append(tslib.noop_line(ctx, text, disamb))
    return "\n".join(lines) + "\n"


def translate_file(path, translations_by_lang):
    """The file's text with the translated lines added. translations_by_lang: {lang: {(ctx, source,
    disamb): text}}."""
    with open(path, encoding="utf-8") as f:
        text = f.read()
    ctx = f"Desktop.{entry_id(path)}"
    out, group = [], None
    lines = text.splitlines()
    # lines already in the file for our languages: kept while the catalog has no translation yet
    # (the transition from hand-written Name[zh_CN]= lines; the catalog wins as soon as it has one)
    existing, g2 = {}, None
    for line in lines:
        g = _GROUP.match(line)
        if g:
            g2 = g.group(1)
        m = _KEY.match(line)
        if m and m.group(2) in tslib.LANGS:
            existing[(g2, m.group(1), m.group(2))] = line
    for line in lines:
        g = _GROUP.match(line)
        if g:
            group = g.group(1)
            out.append(line)
            continue
        m = _KEY.match(line)
        translatable = m and group and (group == "Desktop Entry" or group.startswith("Desktop Action "))
        if not translatable:
            out.append(line)
            continue
        key, loc, value = m.group(1), m.group(2), m.group(3).strip()
        if loc is not None:
            if loc in tslib.LANGS:      # replaced by the catalog's
                continue
            out.append(line)
            continue
        out.append(line)
        if key == "Keywords":
            words = [w for w in value.split(";") if w and is_english_keyword(w)]
            source, disamb = ";".join(words) + ";", "keywords"
        else:
            source, disamb = value, ""
        for lang in tslib.LANGS:
            tr = translations_by_lang.get(lang, {}).get((ctx, source, disamb))
            if tr and tr.strip() and tr != source:
                if key == "Keywords" and not tr.endswith(";"):
                    tr += ";"
                out.append(f"{key}[{lang}]={tr}")
            elif (group, key, lang) in existing:
                out.append(existing[(group, key, lang)])
    return "\n".join(out) + "\n"


def main(argv):
    args = argv[1:]
    if not args or args[0] not in ("extract", "generate"):
        print(__doc__.strip(), file=sys.stderr)
        return 2
    cmd, args = args[0], args[1:]
    opts = {}
    for flag in ("--seed", "--ts-dir"):
        if flag in args:
            i = args.index(flag)
            opts[flag] = args[i + 1]
            del args[i:i + 2]
    if not args:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    target, files = args[0], args[1:] or default_files()
    if cmd == "extract":
        tslib.write_atomic(target, render_cpp(files))
        if "--seed" in opts:
            lines = ["msg context\tkey\tEnglish\tzh_CN"]
            for p in files:
                lines += ["\t".join(r) for r in zh_seed(p)]
            tslib.write_atomic(opts["--seed"], "\n".join(lines) + "\n")
        return 0
    by_lang = {lang: tslib.load_translations("remote", lang, opts.get("--ts-dir")) for lang in tslib.LANGS}
    for p in files:
        tslib.write_atomic(os.path.join(target, os.path.basename(p)), translate_file(p, by_lang))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
