#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Checks on the translation catalogs (docs/translating.md).

    check.py [--strict] [--ts-dir DIR] [--glossary-dir DIR] [--catalogs a,b,c] [--langs xx_YY,...]

With --catalogs, every selected catalog/language pair is required (--langs defaults to the ten
UI languages). The input catalogs need en_US only when their synchronized English sources have
plural messages. Without --catalogs, check only the files present (partial scratch catalogs).

For every i18n/ts/<catalog>_<lang>.ts (en_US is plural-only and is only checked for its forms):
  placeholders  %1 %2 %L1 %n %Ln of the source are in every translation (and no others)
  numerus       a plural message has the language's number of forms, none empty once finished
  unfinished    (--strict) any message still type="unfinished"
  french        U+202F before ; : ! ? (a sentence colon) and inside « »
  cjk           zh/ja: full-width , ; : ! ? . after CJK text; zh_TW and ja use 「」, zh_CN “”; no space next to a
                quotation mark or a quoted %1 (zh_CN: one space where a curly quote meets Latin text)
  spaces        no doubled space (U+0020, U+00A0, U+202F) the source doesn't have; U+200B only in zh and ja;
                U+00AD only in de, fr, es, en, between three letters on each side
  glossary      app names (desktop-entries Name) and page and group names (settings-pages titles) are
                the glossary's term for the language (i18n/glossary/<lang>.tsv); no app name equals a folder's
                or another app's name; no default folder of the app gallery is named like an app
Exits 1 on any error (and, with --strict, on unfinished messages); 0 otherwise.
"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import tslib  # noqa: E402

PLACEHOLDER = re.compile(r"%(?:L?\d+|Ln|n)")
EXPECTED_FORMS = {"zh_CN": {1}, "zh_TW": {1}, "ja_JP": {1}, "ko_KR": {1}, "de_DE": {2}, "en_GB": {2}, "en_AU": {2},
                  "es_ES": {2, 3}, "fr_FR": {2, 3}, "en_US": {2}}
CJK = r"぀-ヿ㐀-䶿一-鿿豈-﫿"
NNBSP = " "

# The tabs of the app's Settings (SettingsSheet.qml).
SETTINGS_TABS = ["Display", "Quality", "Input", "Server", "Advanced", "About"]
PAGE_GLOSSARY = {"terminal": "terminal_shell", "keyboard": "keyboard_shortcuts", "printers": "printers_scanners",
                 "users": "users_accounts", "privacy": "privacy_security"}
GROUP_GLOSSARY = {"personalise": "personalize"}


def read_glossary(path):
    terms = {}
    if not os.path.exists(path):
        return terms
    with open(path, encoding="utf-8") as f:
        next(f, None)
        for line in f:
            cols = line.rstrip("\n").split("\t")
            if len(cols) >= 2 and cols[1]:
                terms[cols[0]] = cols[1]
    return terms


def placeholders(s):
    return sorted(PLACEHOLDER.findall(s))


def check_message(m, lang, errors):
    where = f"{m.ctx}: “{m.source[:40]}”"
    forms = [f for f in m.forms]
    if m.state == "unfinished" and not any(f.strip() for f in forms):
        return
    if m.numerus and lang in EXPECTED_FORMS and len(forms) not in EXPECTED_FORMS[lang]:
        errors.append(f"{where}: {len(forms)} plural forms, {lang} has {sorted(EXPECTED_FORMS[lang])}")
    want = placeholders(m.source)
    for i, f in enumerate(forms):
        if not f.strip():
            if m.state == "finished":
                errors.append(f"{where}: finished but plural form {i + 1} is empty" if m.numerus else f"{where}: finished but empty")
            continue
        have = placeholders(f)
        if m.numerus and i == 0 and len(forms) > 1 and have != want:
            # the "one" form may say "one"/"a" instead of %n
            have_cmp = sorted(x for x in have if x not in ("%n", "%Ln"))
            want_cmp = sorted(x for x in want if x not in ("%n", "%Ln"))
        else:
            have_cmp, want_cmp = have, want
        if have_cmp != want_cmp:
            errors.append(f"{where}: placeholders {have} in the translation, {want} in the source")
        if m.disamb == "keywords" and m.ctx.startswith("Desktop."):
            continue    # a .desktop Keywords list: ASCII ";" is the format's separator, in every language
        errors.extend(f"{where}: {e}" for e in punctuation(f, lang))
        errors.extend(f"{where}: {e}" for e in spacing(f, m, lang))


SPACES = " \u00a0\u202f"
OPEN_Q, CLOSE_Q = "“‘「『", "”’」』"
QUOTED = re.compile(r"(%\d+) is a quoted name")


def spacing(t, m, lang):
    out = []
    dbl = re.compile(f"[{SPACES}]{{2}}")
    if dbl.search(t) and not dbl.search(m.source):
        out.append("doubled space")
    if "\u200b" in t and lang not in ("zh_CN", "zh_TW", "ja_JP"):
        out.append("U+200B (a phrase mark) is only for Chinese and Japanese")
    if "\u00ad" in t:
        if lang not in ("de_DE", "fr_FR", "es_ES", "en_US", "en_GB", "en_AU"):
            out.append("U+00AD (a soft hyphen) is only for German, French, Spanish and English")
        else:
            for i, c in enumerate(t):
                if c == "\u00ad" and not (i >= 3 and t[i - 3:i].isalpha() and t[i + 1:i + 4].isalpha() and len(t[i + 1:i + 4]) == 3):
                    out.append("a soft hyphen goes at a compound joint, with three letters on each side")
                    break
    if lang not in ("zh_CN", "zh_TW", "ja_JP"):
        return out
    latin = lambda c: c.isascii() and c.isalnum()

    def outside_ok(mark, neighbour):
        # a separator keeps its spaces (" · "); zh_CN keeps one half-width space where a curly quote meets a Latin
        # letter or digit outside it
        return neighbour in "·—–/|→" or (lang == "zh_CN" and mark in "“”‘’" and latin(neighbour))

    for i, c in enumerate(t):
        if c in OPEN_Q:
            if i + 1 < len(t) and t[i + 1] in SPACES:
                out.append(f"space inside “{c}”")
                break
            if i >= 1 and t[i - 1] in SPACES and not (i >= 2 and outside_ok(c, t[i - 2])):
                out.append(f"space before “{c}”")
                break
        elif c in CLOSE_Q:
            if c == "’" and i >= 1 and latin(t[i - 1]):
                continue    # an apostrophe in a Latin word
            if i >= 1 and t[i - 1] in SPACES:
                out.append(f"space inside “{c}”")
                break
            if i + 1 < len(t) and t[i + 1] in SPACES and not (i + 2 < len(t) and outside_ok(c, t[i + 2])):
                out.append(f"space after “{c}”")
                break
    for q in QUOTED.findall(m.extracomment or ""):
        for mm in re.finditer(re.escape(q) + r"(?!\d)", t):
            a, b = mm.start(), mm.end()
            if a >= 1 and t[a - 1] in SPACES and not (a >= 2 and outside_ok("“", t[a - 2])):
                out.append(f"space before the quoted {q}")
            elif b < len(t) and t[b] in SPACES and not (b + 1 < len(t) and outside_ok("”", t[b + 1])):
                out.append(f"space after the quoted {q}")
    return out


def punctuation(t, lang):
    out = []
    if lang == "fr_FR":
        for m in re.finditer(r"(\S)?([   ]?)([;!?:])(?=\s|$)", t):
            before, sp, ch = m.group(1), m.group(2), m.group(3)
            if ch == ":" and before and before.isdigit():
                continue    # a time or a ratio
            if sp != NNBSP:
                out.append(f"French: U+202F narrow no-break space needed before “{ch}”")
                break
        for m in re.finditer(r"«(.?)", t):
            if m.group(1) != NNBSP:
                out.append("French: « needs a U+202F after it")
                break
        for m in re.finditer(r"(.?)»", t):
            if m.group(1) != NNBSP:
                out.append("French: » needs a U+202F before it")
                break
    if lang in ("zh_CN", "zh_TW", "ja_JP"):
        if re.search(rf"[{CJK}][,;!?]", t) or re.search(rf"[{CJK}]:(?=\s|$)", t) or re.search(rf"[{CJK}]\.(?=\s|$)", t):
            out.append("use full-width punctuation (， ； ： ！ ？ 。) after Chinese or Japanese text")
        if lang == "zh_CN" and re.search(r"[「」]", t):
            out.append("Simplified Chinese quotes are “ ” (「」 is Taiwan)")
        if lang in ("zh_TW", "ja_JP") and re.search(r"[“”]", t):
            out.append("quotes are 「 」 in this language, not “ ”")
    return out


def name_collisions(terms):
    """Decision 101: an app's name differs from every folder's and every other app's name in the language."""
    out, seen = [], {}
    for gid in sorted(terms):
        if not (gid.startswith("app_") or gid.startswith("folder_")):
            continue
        key = terms[gid].strip().casefold()
        if key in seen and (gid.startswith("app_") or seen[key].startswith("app_")):
            out.append(f"{seen[key]} and {gid} are both “{terms[gid]}”")
        seen.setdefault(key, gid)
    return out


def glossary_checks(catalog, lang, ts, glossary, errors):
    if catalog in ("desktop-entries", "remote"):
        for m in ts.messages:
            mo = re.match(r"Desktop\.org\.sodiumos\.(\w+)$", m.ctx)
            if not mo or not re.search(r"\bName\b", m.extracomment.split(";")[0]) or "action" in m.extracomment or not m.finished:
                continue
            gid = "prod_sodium_remote" if mo.group(1) == "Remote" else "app_" + re.sub(r"(?<!^)(?=[A-Z])", "_", mo.group(1)).lower()
            term = glossary.get(gid)
            if term and m.translation.replace("\u00ad", "") != term:
                errors.append(f"{m.ctx}: the app name is “{m.translation}”, the glossary ({gid}) says “{term}”")
    if catalog == "remote":
        # A "Settings › Tab" path in a sentence names this app's own Settings tab: the translation says the tab as the tab
        # itself is translated (a path segment left in English is not caught by anything else).
        tabs = {m.source: m.translation for m in ts.messages if m.ctx == "SettingsSheet" and m.finished and m.source in SETTINGS_TABS}
        for m in ts.messages:
            if not m.finished or m.numerus:
                continue
            for tab in re.findall(r"Settings › (%s)\b" % "|".join(SETTINGS_TABS), m.source):
                want = tabs.get(tab)
                if want and want not in m.translation:
                    errors.append(f"{m.ctx}: “{m.source[:40]}”: Settings › {tab} must name the tab as it is translated (“{want}”)")
    if catalog == "settings-pages":
        for m in ts.messages:
            if not m.finished:
                continue
            gid = None
            pm = re.search(r'page "([\w-]+)" \(title\)', m.extracomment)
            gm = re.search(r'group "([\w-]+)" \(title\)', m.extracomment)
            if pm:
                gid = "page_" + PAGE_GLOSSARY.get(pm.group(1), pm.group(1).replace("-", "_"))
            elif gm:
                gid = "group_" + GROUP_GLOSSARY.get(gm.group(1), gm.group(1).replace("-", "_"))
            term = glossary.get(gid) if gid else None
            if term and m.translation.replace("\u00ad", "") != term:
                errors.append(f"{m.ctx}: the title is “{m.translation}”, the glossary ({gid}) says “{term}”")
    if catalog == "shell":
        # app-gallery.md: the default folders (their translator comment says so) aren't named like an app
        apps = {t.strip().casefold(): gid for gid, t in glossary.items() if gid.startswith("app_")}
        for m in ts.messages:
            if m.finished and m.extracomment.startswith("A folder in the app gallery that holds"):
                gid = apps.get(m.translation.replace("\u00ad", "").strip().casefold())
                if gid:
                    errors.append(f"{m.ctx}: the app gallery's folder “{m.source}” is “{m.translation}”, the name of {gid}")


def input_has_plurals(catalog):
    source = os.path.join(tslib.REPO, "i18n", "input", catalog)
    if catalog == "sodiumos-input":
        with open(source + ".pot", encoding="utf-8") as f:
            return any(re.match(r"\s*msgid_plural\s", line) for line in f)
    return any(m.numerus for m in tslib.parse_ts(source + ".ts").messages)


def main(argv):
    args = argv[1:]
    strict = "--strict" in args
    opt = {}
    it = iter(a for a in args if a != "--strict")
    for a in it:
        opt[a] = next(it, "")
    ts_dir = opt.get("--ts-dir", os.path.join(tslib.REPO, "i18n", "ts"))
    gdir = opt.get("--glossary-dir", os.path.join(tslib.REPO, "i18n", "glossary"))
    langs = opt["--langs"].split(",") if "--langs" in opt else tslib.LANGS + [tslib.SOURCE_LANG]
    try:
        files = sorted(f for f in os.listdir(ts_dir) if f.endswith(".ts"))
    except OSError as e:
        print(f"error: {ts_dir}: can't read catalog directory ({e})")
        return 1
    catalogs = set(opt["--catalogs"].split(",")) if "--catalogs" in opt else None
    errors, unfinished, checked = [], 0, 0
    if catalogs is not None:
        for catalog in sorted(catalogs):
            for lang in langs:
                fn = f"{catalog}_{lang}.ts"
                if fn in files:
                    continue
                if lang == tslib.SOURCE_LANG and catalog in ("sodiumos-input", "sodiumos-input-qml"):
                    try:
                        if not input_has_plurals(catalog):
                            continue
                    except Exception as e:
                        errors.append(f"{fn}: can't inspect input source ({e})")
                        continue
                errors.append(f"{os.path.join(ts_dir, fn)}: required catalog is missing")
    glossaries = {}
    for fn in files:
        m = re.match(r"^(.+)_([a-z]{2}_[A-Z]{2})\.ts$", fn)
        if not m or m.group(2) not in langs or (catalogs and m.group(1) not in catalogs):
            continue
        catalog, lang = m.groups()
        checked += 1
        try:
            ts = tslib.parse_ts(os.path.join(ts_dir, fn))
        except Exception as e:  # a broken file is an error, not a crash
            errors.append(f"{fn}: can't read it ({e})")
            continue
        errs = []
        if lang != tslib.SOURCE_LANG:
            for msg in ts.messages:
                if msg.state in ("obsolete", "vanished"):
                    continue
                if msg.state == "unfinished":
                    unfinished += 1
                check_message(msg, lang, errs)
            glossaries.setdefault(lang, read_glossary(os.path.join(gdir, lang + ".tsv")))
            glossary_checks(catalog, lang, ts, glossaries[lang], errs)
        else:
            for msg in ts.messages:
                if msg.numerus and len(msg.forms) not in EXPECTED_FORMS["en_US"]:
                    errs.append(f"{msg.ctx}: {len(msg.forms)} plural forms in en_US")
                elif msg.numerus and msg.state not in ("obsolete", "vanished") \
                        and any(not f.strip() for f in msg.forms):
                    errs.append(f"{msg.ctx}: empty plural form in en_US for {msg.source!r}")
        errors += [f"{fn}: {e}" for e in errs]
    for lang, terms in glossaries.items():
        errors += [f"glossary {lang}: {e}" for e in name_collisions(terms)]
    for e in errors:
        print("error:", e)
    print(f"checked {checked} catalogs: {len(errors)} errors, {unfinished} unfinished messages")
    if strict and unfinished:
        print("--strict: unfinished messages fail the check")
    return 1 if errors or (strict and unfinished) else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
