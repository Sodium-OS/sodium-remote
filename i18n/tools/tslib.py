# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Shared helpers for the i18n scripts: languages, catalogs and reading Qt Linguist .ts files.

The shared contract (docs/translating.md): catalogs live in
i18n/ts/<catalog>_<lang>.ts and compile to build/i18n/qm/<catalog>_<lang>.qm.
"""
import os
import re
import xml.etree.ElementTree as ET

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

LANGS = ["en_GB", "en_AU", "zh_CN", "zh_TW", "ja_JP", "ko_KR", "es_ES", "fr_FR", "de_DE"]
SOURCE_LANG = "en_US"   # the source; en_US.ts holds plural forms only

# The catalog -> source directories table is in i18n/i18n.sh (one place); the app's also has a generated source (its .desktop file).
GENERATED_SOURCES = {"remote": "desktop-entry.cpp"}

# a login locale -> the catalog language it uses. Bare zh is Simplified (glibc's default); the other
# Chinese script is never chosen for it.
_ALIASES = {"zh": "zh_CN", "zh_Hans": "zh_CN", "zh_Hant": "zh_TW", "zh_SG": "zh_CN", "zh_HK": "zh_TW",
            "ja": "ja_JP", "ko": "ko_KR", "es": "es_ES", "fr": "fr_FR", "de": "de_DE",
            "en_GB": "en_GB", "en_AU": "en_AU", "en_IE": "en_GB", "en_NZ": "en_AU"}


def pick_language(spec):
    """The first supported language of a LANGUAGE-style list ("de:en", "ja_JP.UTF-8"), as a
    catalog language, or "en_US" (the source) when the first usable entry is English or none is
    supported. Entries of languages we don't have (pt_BR) are skipped, as gettext does."""
    for item in re.split(r"[:,\s]+", spec or ""):
        item = re.sub(r"[.@].*$", "", item.strip()).replace("-", "_")
        if not item:
            continue
        if item in ("C", "POSIX", "en", "en_US"):
            return SOURCE_LANG
        if item in LANGS:
            return item
        if item in _ALIASES:
            return _ALIASES[item]
        base = item.split("_")[0]
        if base == "en":
            return SOURCE_LANG
        if base in _ALIASES and base != "zh":
            return _ALIASES[base]
    return SOURCE_LANG


class TsMessage:
    def __init__(self, ctx, source, disamb, comment, extracomment, forms, state, numerus, locations):
        self.ctx, self.source, self.disamb = ctx, source, disamb
        self.comment, self.extracomment = comment, extracomment
        self.forms, self.state, self.numerus = forms, state, numerus   # forms: [str]
        self.locations = locations

    @property
    def translation(self):
        return self.forms[0] if self.forms else ""

    @property
    def finished(self):
        """A usable translation: not unfinished/obsolete and not empty."""
        return self.state == "finished" and any(f.strip() for f in self.forms) and all(f.strip() for f in self.forms)


class TsFile:
    def __init__(self, language, messages):
        self.language, self.messages = language, messages

    def lookup(self, ctx, source, disamb=""):
        for m in self.messages:
            if m.ctx == ctx and m.source == source and m.disamb == disamb:
                return m
        return None

    def translations(self):
        """{(ctx, source, disamb): text} of the finished, non-plural messages."""
        return {(m.ctx, m.source, m.disamb): m.translation for m in self.messages if m.finished and not m.numerus}


def parse_ts(path_or_text):
    if "<TS" in path_or_text:
        root = ET.fromstring(path_or_text)
    else:
        root = ET.parse(path_or_text).getroot()
    msgs = []
    for ctx in root.findall("context"):
        name = (ctx.findtext("name") or "")
        for m in ctx.findall("message"):
            tr = m.find("translation")
            ttype = (tr.get("type") if tr is not None else None) or "finished"
            numerus = m.get("numerus") == "yes"
            if tr is not None and numerus:
                forms = [f.text or "" for f in tr.findall("numerusform")]
            else:
                forms = [(tr.text or "") if tr is not None else ""]
            locs = [(l.get("filename", ""), l.get("line", "")) for l in m.findall("location")]
            msgs.append(TsMessage(name, m.findtext("source") or "", m.findtext("comment") or "",
                                  m.findtext("comment") or "", m.findtext("extracomment") or "",
                                  forms, ttype, numerus, locs))
    return TsFile(root.get("language", ""), msgs)


def ts_path(catalog, lang, ts_dir=None):
    return os.path.join(ts_dir or os.path.join(REPO, "i18n", "ts"), f"{catalog}_{lang}.ts")


BREAK_MARKS = dict.fromkeys(map(ord, "\u00ad\u200b\u2060"))


def load_translations(catalog, lang, ts_dir=None):
    """{(ctx, source, disamb): text} for one catalog and language; {} when there is no file. For the
    generated files (.desktop, polkit, the crash notifier's lookup), so the line-break marks translators
    may place (U+00AD soft hyphen, U+200B phrase mark) and word joiners are removed."""
    p = ts_path(catalog, lang, ts_dir)
    if not os.path.exists(p):
        return {}
    return {k: v.translate(BREAK_MARKS) for k, v in parse_ts(p).translations().items()}


def cpp_string(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\t", "\\t") + '"'


def noop_line(ctx, text, disamb=""):
    if disamb:
        return f"QT_TRANSLATE_NOOP3({cpp_string(ctx)}, {cpp_string(text)}, {cpp_string(disamb)});"
    return f"QT_TRANSLATE_NOOP({cpp_string(ctx)}, {cpp_string(text)});"


def write_atomic(path, text):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path + ".tmp", "w", encoding="utf-8") as f:
        f.write(text)
    os.replace(path + ".tmp", path)
