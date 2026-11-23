#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""The i18n lint (docs/translating.md): finds UI code that breaks the language rules.

    lint.py [--strict] [--summary] [--component NAME] [--rule RULE] [--allow FILE] [--root DIR] [PATH ...]

Reports per component (shell module, app, SodiumUI, greeter, settings JSON) with file:line. Prints the
counts, then the findings. --strict exits 1 on any finding (warnings included); the default exits 0.
Heuristic rules are warnings (`warn`); the rest are findings (`error`).

Rules:
  unwrapped-qml     a string literal set on a text property (text, title, label, placeholderText,
                    ToolTip.text, Accessible.name, ...) or in its ternary or returned by a function
  unwrapped-cpp     (warn) a sentence-like QStringLiteral/"" next to a text-ish name, outside tr()
  british           a British spelling in a source string (qsTr/tr/QT_TRANSLATE_NOOP/pages.json)
  concat            qsTr(...) + x or x + qsTr(...)
  hand-plural       n === 1 ? "..." : "..." (use %n)
  hardcoded-locale  Qt.locale("xx"), QLocale(QLocale::English), a display date pattern
  font-family       font.family outside ui/SodiumUI
  letter-spacing    letterSpacing outside ui/SodiumUI
  uppercase         toUpperCase() on UI text, Font.AllUppercase outside ui/SodiumUI
  compare-translated  a translated string compared (=== qsTr(...))
  italic            italic on UI text (font.italic: true, { italic: true }; C++ setItalic(true) a warning) and
                    <i>/<em> in a source string: emphasis is weight in every language
  word-wrap         Text.WordWrap on UI text: Text.Wrap breaks at words, then anywhere
  raw-bold          a raw font.weight >= 700 (Font.Bold, ExtraBold, Black), font.bold: true, CSS font-weight >= 700 or
                    <b> not passed through Theme.rich(), C++ setBold(true) / QFont::Bold: Qt fakes bold at 700 on
                    variable fonts (Nunito, Resource Han Rounded); Theme.font asks 699, Theme.rich does the same for <b>
  soft-hyphen       a soft hyphen (U+00AD) or phrase mark (U+200B) in a source file: translators place them in the
                    catalogs, the US English source never has them
  placeholder-label a TextField with a placeholder but no visible label (it isn't inside a FormField or ControlRow)
                    and no Accessible.name: a placeholder may elide, so it never identifies the field

The allowlist (i18n/lint-allow.txt): `path[:line] [@rule] [/regex/]  # reason` (path may be a glob).
"""
import fnmatch
import json
import os
import re
import sys
from collections import Counter, defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import tslib  # noqa: E402

ROOTS = ["app", "sodium-ui"]
SKIP_DIRS = {"data", "build", "tests", "test", "node_modules", "third_party", "gallery", "fixtures", ".git", "__pycache__", "build-test"}
CODE_EXT = {".qml", ".js", ".cpp", ".h", ".hpp", ".cc"}

TEXT_PROPS = (r"text|title|label|placeholderText|placeholder|subtitle|subTitle|sub|description|headline|caption|hint|"
              r"message|actionText|emptyText|errorText|helperText|statusText|summary|pageTitle|windowTitle|detail|"
              r"detailText|secondaryText|tooltip|toolTip|displayText|nameText|helpText|noteText|body")
DOTTED_PROPS = r"ToolTip\.text|Accessible\.name|Accessible\.description|Accessible\.help"
PROP_RE = re.compile(r"(?<![\w.])(" + DOTTED_PROPS + r"|" + TEXT_PROPS + r")\s*:\s*(.*)$")
STR_RE = re.compile(r'"((?:[^"\\]|\\.)*)"' + "|" + r"'((?:[^'\\]|\\.)*)'")
WRAP_CALLS = re.compile(r"\b(?:qsTr|qsTrId|qsTranslate|QT_TR_NOOP|QT_TRANSLATE_NOOP3?|tr|translate|trUtf8)\s*\(")

# British -> US. Word forms are generated from the stems below.
BRITISH = {
    "minimise": "minimize", "maximise": "maximize", "colour": "color", "favourite": "favorite", "centre": "center",
    "licence": "license", "organise": "organize", "personalise": "personalize", "recognise": "recognize",
    "behaviour": "behavior", "grey": "gray", "catalogue": "catalog", "customise": "customize", "optimise": "optimize",
    "synchronise": "synchronize", "authorise": "authorize", "initialise": "initialize", "summarise": "summarize",
    "realise": "realize", "analyse": "analyze", "neighbour": "neighbor", "honour": "honor", "favour": "favor",
    "labour": "labor", "metre": "meter", "litre": "liter", "fibre": "fiber", "theatre": "theater",
    "dialogue": "dialog", "enrol": "enroll", "fulfil": "fulfill", "practise": "practice", "ageing": "aging",
    "judgement": "judgment", "artefact": "artifact", "programme": "program", "defence": "defense", "offence": "offense",
    "apologise": "apologize", "normalise": "normalize", "prioritise": "prioritize", "categorise": "categorize",
    "visualise": "visualize", "utilise": "utilize", "finalise": "finalize", "emphasise": "emphasize",
    "sanitise": "sanitize", "serialise": "serialize", "localise": "localize", "digitise": "digitize",
    "mobilise": "mobilize", "memorise": "memorize", "criticise": "criticize", "capitalise": "capitalize",
    "centralise": "centralize", "stabilise": "stabilize", "specialise": "specialize", "generalise": "generalize",
    "harmonise": "harmonize", "magnetise": "magnetize", "randomise": "randomize", "virtualise": "virtualize",
    "cancelled": "canceled", "cancelling": "canceling", "labelled": "labeled", "labelling": "labeling",
    "modelling": "modeling", "travelled": "traveled", "travelling": "traveling", "whilst": "while",
    "amongst": "among", "learnt": "learned", "tyre": "tire", "cheque": "check", "sceptical": "skeptical",
}
_SUFFIX = r"(?:s|d|r|rs|ing|ed|es|ation|ations|ations?)?"
BRITISH_RE = {}
for brit, us in BRITISH.items():
    if brit in ("cancelled", "cancelling", "labelled", "labelling", "modelling", "travelled", "travelling", "whilst",
                "amongst", "learnt", "tyre", "cheque", "sceptical", "ageing", "judgement"):
        pat = r"\b" + brit + r"\b"
    elif brit.endswith("ise"):
        stem = brit[:-1]            # "minimis" + e|ed|es|ing|ation(s)
        pat = r"\b" + stem + r"(?:e|ed|es|ing|er|ers|ation|ations)\b"
    elif brit.endswith("yse"):
        stem = brit[:-1]
        pat = r"\b" + stem + r"(?:e|ed|es|ing|er|ers)\b"
    elif brit.endswith("our"):
        pat = r"\b" + brit + r"(?:s|ed|ing|ite|ites)?\b"
    elif brit.endswith(("re", "ogue", "ence", "ogue")):
        pat = r"\b" + brit + r"s?\b"
    else:
        pat = r"\b" + brit + r"(?:s|d|ed|ing)?\b"
    BRITISH_RE[brit] = (re.compile(pat, re.I), us)

SPAN = {"unwrapped-qml": "error", "unwrapped-cpp": "warn", "british": "error", "concat": "error", "hand-plural": "error",
        "hardcoded-locale": "error", "font-family": "error", "letter-spacing": "error", "uppercase": "error",
        "compare-translated": "error", "italic": "error", "word-wrap": "error", "raw-bold": "error",
        "soft-hyphen": "error", "placeholder-label": "error"}

# Decision 54: emphasis by weight, never italic in UI text (italics only in user documents).
ITALIC_QML = re.compile(r"\bitalic\s*:\s*true\b")
ITALIC_CPP = re.compile(r"\bsetItalic\s*\(\s*true\s*\)")
ITALIC_MARKUP = re.compile(r"<\s*/?\s*(?:i|em)\s*>", re.I)
# Decision 57: Text.Wrap, never WordWrap, on multi-line UI text.
WORD_WRAP = re.compile(r"\b(?:Text|Label)\.WordWrap\b")
# Decision 72: Qt fakes bold for a request at weight >= 700 on a variable font. Theme.font (I18n::font) asks 699; Theme.rich turns <b> into 699.
RAW_BOLD_QML = re.compile(r"\bfont\.weight\s*:[^/\n]*?(?:\b[7-9]\d\d\b|\b1000\b|Font\.(?:Bold|ExtraBold|Black)\b)|\bfont\.bold\s*:\s*true\b|"
                          r"font-weight\s*:\s*(?:[7-9]\d\d\b|bold)")
BOLD_MARKUP = re.compile(r"<\s*(?:b|strong)\s*>", re.I)
RAW_BOLD_CPP = re.compile(r"\bsetBold\s*\(\s*true\s*\)|\bQFont::(?:Bold|ExtraBold|Black)\b|\bsetFontWeight\s*\(\s*QFont::(?:Bold|ExtraBold|Black)")


COUNTISH = re.compile(r"^(?:[a-zA-Z]|\w{2,3})$|count|length|size|total|num|number|items|files|folders|rows|selected|"
                      r"many|days|hours|minutes|seconds|weeks|months|years|results|matches|entries|\w+s$", re.I)


class Finding:
    def __init__(self, path, line, rule, msg, component, severity=None):
        self.path, self.line, self.rule, self.msg, self.component = path, line, rule, msg, component
        self.severity = severity or SPAN[rule]

    def __str__(self):
        w = " (warning)" if self.severity == "warn" else ""
        return f"{self.path}:{self.line}: [{self.rule}]{w} {self.msg}"


def component_of(rel):
    parts = rel.split("/")
    if parts[0] == "app":
        return "app"
    if parts[0] == "sodium-ui":
        return "SodiumUI"
    return parts[0]


def in_sodiumui(rel):
    return rel.startswith("sodium-ui/SodiumUI/") or rel.startswith("sodium-ui/native/")


def strip_comments(lines):
    """The lines with // and /* */ comments blanked out (strings are kept intact)."""
    out, in_block = [], False
    for line in lines:
        res, i, quote = [], 0, None
        while i < len(line):
            c = line[i]
            if in_block:
                if line.startswith("*/", i):
                    in_block = False
                    i += 2
                else:
                    i += 1
                continue
            if quote:
                res.append(c)
                if c == "\\" and i + 1 < len(line):
                    res.append(line[i + 1])
                    i += 2
                    continue
                if c == quote:
                    quote = None
                i += 1
                continue
            if c in "\"'":
                quote = c
                res.append(c)
            elif line.startswith("//", i):
                break
            elif line.startswith("/*", i):
                in_block = True
                i += 2
                continue
            else:
                res.append(c)
            i += 1
        out.append("".join(res))
    return out


def looks_like_text(s):
    """A literal that is probably shown to the user (and so should be a qsTr message)."""
    s = s.strip()
    if not re.search(r"[A-Za-z]{2}", s):
        return False
    if re.fullmatch(r"(?:(?:Ctrl|Alt|Shift|Super|Meta|Win|Cmd)\s*\+\s*)+\S+|F\d{1,2}|(?:Ctrl|Alt|Shift)", s):   # key caps
        return False
    if re.search(r"://|^[#./~$%]|[\\]", s) and " " not in s:
        return False
    if re.fullmatch(r"[a-z0-9_.:\-/+]+", s):             # an id, a path, a role: lowercase, no spaces
        return False
    if re.fullmatch(r"[A-Za-z]+(?:[_.:\-][A-Za-z0-9]+)+", s) and " " not in s:   # org.x.Y, snake_case
        return False
    if re.fullmatch(r"[a-z]+[A-Z][A-Za-z0-9]*", s):         # camelCase identifier
        return False
    if re.fullmatch(r"[A-Z0-9_]+", s) and len(s) > 3:       # CONSTANT
        return False
    return bool(re.search(r"[A-Z]", s) or " " in s)


def literals(expr):
    for m in STR_RE.finditer(expr):
        yield m.start(), (m.group(1) if m.group(1) is not None else m.group(2))


def wrapped_spans(expr):
    """[(start, end)] of the argument lists of qsTr()/tr()/translate() calls in expr."""
    spans = []
    for m in WRAP_CALLS.finditer(expr):
        depth, i, q = 1, m.end(), None
        while i < len(expr) and depth:
            c = expr[i]
            if q:
                if c == "\\":
                    i += 1
                elif c == q:
                    q = None
            elif c in "\"'":
                q = c
            elif c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
            i += 1
        spans.append((m.start(), i))
    return spans


def check_qml(rel, lines, code, comp, add):
    for n, line in enumerate(code, 1):
        m = PROP_RE.search(line)
        if m:
            expr = m.group(2)
            spans = wrapped_spans(expr)
            for pos, text in literals(expr):
                if any(a <= pos < b for a, b in spans):
                    continue
                before = expr[:pos].rstrip()
                # a comparison or lookup is data, not text
                if re.search(r"(?:[=!]==?|\[|\bindexOf\(|startsWith\(|endsWith\(|includes\(|\bcase)\s*$", before):
                    continue
                # a key chord for KeyCaps (it names the keys in the app's language): data, not text
                if re.search(r"\bKeyCaps\.(?:format|name)\(\s*$", before):
                    continue
                if not looks_like_text(text):
                    continue
                if pos == 0 or before.endswith(("?", ":", "||", "??", "(", "+", "&&")) or before == "":
                    add(rel, n, "unwrapped-qml", f'{m.group(1)}: "{short(text)}" is not wrapped in qsTr()', comp)
        elif re.search(r"\breturn\s+[\"']", line):
            rm = re.search(r"\breturn\s+(?:[^\"'?:]*\?\s*)?[\"']((?:[^\"'\\]|\\.)*)[\"']", line)
            if rm and " " in rm.group(1).strip() and looks_like_text(rm.group(1)):
                add(rel, n, "unwrapped-qml", f'a function returns "{short(rm.group(1))}" without qsTr()', comp)
        if not in_sodiumui(rel):
            if re.search(r"\bfont\.family\s*:|\bfontFamily\s*:|Qt\.font\(\s*\{[^}]*\bfamily\s*:", line):
                add(rel, n, "font-family", "font.family outside Sodium UI: use Theme.font(style)", comp)
            if re.search(r"\bletterSpacing\b", line):
                add(rel, n, "letter-spacing", "letterSpacing outside the type styles", comp)
            if re.search(r"Font\.AllUppercase|capitalization\s*:\s*Font\.AllUppercase", line):
                add(rel, n, "uppercase", "Font.AllUppercase outside the type styles", comp)
        if ITALIC_QML.search(line):
            add(rel, n, "italic", "italic on UI text: emphasis is weight in every language", comp)
        if not rel.endswith("sodium-ui/SodiumUI/Theme.qml"):
            if RAW_BOLD_QML.search(line):
                add(rel, n, "raw-bold", "a raw weight >= 700 fakes bold on Nunito and Resource Han Rounded: use Theme.font(style, { weight: 700 })", comp)
            elif BOLD_MARKUP.search(line) and not any("Theme.rich(" in c for c in code[max(0, n - 4):n]):
                add(rel, n, "raw-bold", "<b> in a Text fakes bold on Nunito and Resource Han Rounded: wrap the string in Theme.rich()", comp)
        if WORD_WRAP.search(line):
            add(rel, n, "word-wrap", "Text.WordWrap: use Text.Wrap, which breaks at words, then anywhere", comp)


# Decision 87: the components whose label is visible beside or above their field.
LABELLING_PARENTS = {"FormField", "ControlRow"}
QML_TOKEN = re.compile(r"([A-Za-z_][\w.]*)\s*\{|([A-Za-z_][\w.]*)\s*:(?!:)|[{}]")


def blank_strings(line):
    """The line with the insides of its string literals blanked (braces and colons in text don't count)."""
    return STR_RE.sub(lambda m: m.group(0)[0] + " " * (len(m.group(0)) - 2) + m.group(0)[-1], line)


def check_placeholder_labels(rel, code, comp, add):
    """A TextField that sets placeholderText needs a visible label (a FormField or ControlRow around it) or an
    Accessible.name (also as `Accessible { name: ... }`)."""
    stack = []   # [type or None for a JavaScript block, first line, property names]
    for n, line in enumerate(code, 1):
        for m in QML_TOKEN.finditer(blank_strings(line)):
            if m.group(1):
                name = m.group(1)
                obj = name.split(".")[-1][:1].isupper() and name not in ("if", "else")
                stack.append([name.split(".")[-1] if obj else None, n, set()])
            elif m.group(2):
                if stack and stack[-1][0] is not None:
                    stack[-1][2].add(m.group(2))
            elif m.group(0) == "{":
                stack.append([None, n, set()])
            elif stack:
                kind, first, props = stack.pop()
                if kind == "Accessible" and "name" in props and stack:
                    stack[-1][2].add("Accessible.name")
                if kind == "TextField" and "placeholderText" in props and "Accessible.name" not in props \
                        and not any(f[0] in LABELLING_PARENTS for f in stack):
                    add(rel, first, "placeholder-label", "a TextField with a placeholder needs a visible label (FormField) "
                        "or an Accessible.name: the placeholder may elide", comp)


def check_common(rel, code, comp, add):
    for n, line in enumerate(code, 1):
        if re.search(r"\b(?:qsTr|qsTranslate|tr)\s*\([^()]*(?:\([^()]*\)[^()]*)*\)\s*\+(?!\+)", line) or \
                re.search(r"(?<!\+)\+\s*(?:qsTr|qsTranslate|tr|QObject::tr|QCoreApplication::translate)\s*\(", line):
            add(rel, n, "concat", "a translated string is concatenated: use one message with %1", comp)
        if re.search(r"\+=\s*(?:qsTr|qsTranslate|tr|QObject::tr|QCoreApplication::translate)\s*\(", line):
            add(rel, n, "concat", "a translated string is appended (+=): use one message with %1", comp)
        if re.search(r"[!=]==?\s*(?:qsTr|qsTranslate|tr)\s*\(", line) or \
                re.search(r"\b(?:qsTr|qsTranslate|tr)\s*\([^()]*\)\s*[!=]==?", line):
            add(rel, n, "compare-translated", "a translated string is compared: compare ids or roles", comp)
        pm = re.search(r"([\w.]+)(?:\(\))?\s*[!=]==?\s*1\s*\?(.*)$", line)
        if pm and COUNTISH.search(pm.group(1).split(".")[-1]) and re.search(r"(?:qsTr|tr)\(|[\"'][^\"']*[A-Za-z]{2}", pm.group(2)):
            add(rel, n, "hand-plural", "a hand-rolled plural: use qsTr(\"%n item(s)\", \"\", n)", comp)
        if re.search(r'Qt\.locale\(\s*["\'][^"\']+["\']\s*\)|QLocale\(\s*QLocale::(?!C\b|system)\w+|QLocale\(\s*QStringLiteral\(\s*"|QLocale::setDefault|QLocale\(\s*"', line):
            add(rel, n, "hardcoded-locale", "a fixed locale: use the app's language and Region (Sodium UI's Format)", comp)
        for _, lit in literals(line):
            if display_pattern(lit):
                add(rel, n, "hardcoded-locale", f'a fixed display date/time pattern "{lit}": use Sodium UI\'s Format', comp)
                break
        if re.search(r"\.(?:toUpperCase|toLocaleUpperCase)\s*\(\)|\.toUpper\s*\(\)", line):
            ui = re.search(r"\b(?:" + TEXT_PROPS + r")\b|qsTr|tr\(", line)
            add(rel, n, "uppercase", "toUpperCase()" + (" on UI text: uppercase comes from a type style" if ui else " (check it is not UI text)"),
                comp, None if ui else "warn")


def display_pattern(lit):
    """A date/time format string meant for display ("d MMM yyyy", "ddd d MMM", "h:mm ap"); machine keys
    (yyyy-MM-dd, yyyyMMdd) are fine."""
    if re.search(r"MMM|ddd", lit) and re.search(r"\b[dM]{1,4}\b|yy", lit) and len(lit) < 40:
        return True
    return bool(re.search(r"\b[hH]{1,2}:mm(?::ss)?\s*(?:AP|ap|a)\b", lit))


def short(s, n=48):
    s = s.replace("\n", " ")
    return s if len(s) <= n else s[:n - 1] + "…"


def check_british_literal(text):
    hits = []
    for brit, (rx, us) in BRITISH_RE.items():
        m = rx.search(text)
        if m:
            w = m.group(0)
            hits.append((w, suggest(w, brit, us)))
    return hits


def suggest(word, brit, us):
    """The US form of the actual word found (keeping its ending)."""
    stem_b = brit[:-1] if brit.endswith("e") and brit[-3:] in ("ise", "yse") else brit
    stem_u = us[:-1] if us.endswith("e") and us[-3:] in ("ize", "yze") else us
    w = word.lower()
    if w.startswith(stem_b):
        out = stem_u + w[len(stem_b):]
    else:
        out = us
    return out.capitalize() if word[:1].isupper() else out


def check_british_source(rel, code, comp, add):
    """qsTr("..."), tr("..."), QT_TRANSLATE_NOOP(ctx, "...") and qsTranslate(ctx, "...") strings."""
    for n, line in enumerate(code, 1):
        for m in re.finditer(r"\b(qsTr|qsTrId|tr|QT_TR_NOOP|qsTranslate|QT_TRANSLATE_NOOP3?|translate)\s*\(\s*", line):
            rest = line[m.end():]
            lits = list(STR_RE.finditer(rest))
            if not lits:
                continue
            idx = 1 if m.group(1) in ("qsTranslate", "QT_TRANSLATE_NOOP", "QT_TRANSLATE_NOOP3", "translate") else 0
            if idx >= len(lits):
                continue
            lm = lits[idx]
            text = lm.group(1) if lm.group(1) is not None else lm.group(2)
            for w, us in check_british_literal(text):
                add(rel, n, "british", f'British spelling "{w}" in a source string: write "{us}" (en_GB is a translation)', comp)
            if ITALIC_MARKUP.search(text):
                add(rel, n, "italic", "<i>/<em> in a source string: emphasize with <b> (weight), never italic", comp)


def check_cpp(rel, lines, code, comp, add):
    sink = re.compile(r"\b(?:text|title|label|sub|subtitle|message|error|status|description|tooltip|summary|body|reason|hint|note|detail)\b",
                      re.I)
    skip = re.compile(r"\bq(?:Debug|Warning|Critical|Info|Fatal)\b|Q_ASSERT|Q_UNUSED|QLoggingCategory|qCDebug|qCWarning|argv|\.arg\(\)|QStringList|"
                      r"\.value\(|\.contains\(|\.startsWith\(|\.endsWith\(|insert\(|toMap|QProcess|setProgram|\.property\(|QDBus|"
                      r"QStringLiteral\(\"(?:org|com|net|/)|case\b")
    for n, line in enumerate(code, 1):
        if ITALIC_CPP.search(line):
            add(rel, n, "italic", "setItalic(true): italic is for user documents only", comp, "warn")
        if RAW_BOLD_CPP.search(line) and not rel.endswith("sodium-ui/native/i18n.cpp"):
            add(rel, n, "raw-bold", "QFont::Bold fakes bold on variable fonts (Nunito, Resource Han Rounded): ask weight - 1", comp, "warn")
        if skip.search(line):
            continue
        spans = wrapped_spans(line)
        for pos, text in literals(line):
            if not line[max(0, pos - 22):pos].rstrip().endswith(("QStringLiteral(", "QString::fromUtf8(", "QLatin1String(", "(", ",", "=", "<<", "return")) and \
                    "QStringLiteral(" not in line[max(0, pos - 20):pos]:
                continue
            if any(a <= pos < b for a, b in spans):
                continue
            if not re.match(r"[A-Z][a-z]+(?:[ ’'][\w%:,.…’'-]+)+[.…!?]?$", text.strip()):
                continue
            if not sink.search(line):
                continue
            add(rel, n, "unwrapped-cpp", f'"{short(text)}" may reach the UI without tr()', comp)
            break
        if re.search(r"\bQLocale\s*\(\s*QLocale::(?!C\b)", line) is None and False:
            pass


def check_json(rel, text, comp, add):
    sys.path.insert(0, HERE)
    import importlib.util
    spec = importlib.util.spec_from_file_location("extract_settings", os.path.join(HERE, "extract-settings.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    doc = json.loads(text)
    lines = text.splitlines()
    if rel.endswith("pages.json"):
        msgs = mod.extract(doc, None)
    else:
        msgs = mod.extract({"groups": [], "pages": {}}, doc)
    seen = set()
    for m in msgs:
        if m.disamb == "keywords":
            continue
        for w, us in check_british_literal(m.text):
            key = (m.text, w)
            if key in seen:
                continue
            seen.add(key)
            enc = json.dumps(m.text, ensure_ascii=False)
            ln = next((i for i, l in enumerate(lines, 1) if enc[1:-1] in l), 1)
            add(rel, ln, "british", f'British spelling "{w}" in "{short(m.text)}": write "{us}"', comp)
        if ITALIC_MARKUP.search(m.text) and (m.text, "<i>") not in seen:
            seen.add((m.text, "<i>"))
            enc = json.dumps(m.text, ensure_ascii=False)
            ln = next((i for i, l in enumerate(lines, 1) if enc[1:-1] in l), 1)
            add(rel, ln, "italic", f'<i>/<em> in "{short(m.text)}": emphasize with weight, never italic', comp)


def parse_allow(path):
    entries = []
    if not os.path.exists(path):
        return entries
    with open(path, encoding="utf-8") as f:
        for n, raw in enumerate(f, 1):
            line = raw.rstrip("\n")
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            if " # " not in line and not line.rstrip().endswith("#"):
                raise SystemExit(f"{path}:{n}: an allowlist entry needs a reason after ' # '")
            body, _, reason = line.partition(" # ")
            m = re.match(r"^\s*(\S+?)(?::(\d+))?(?:\s+@(\S+))?(?:\s+/(.*)/)?\s*$", body)
            if not m:
                raise SystemExit(f"{path}:{n}: can't read the allowlist entry")
            entries.append({"glob": m.group(1), "line": int(m.group(2)) if m.group(2) else None, "rule": m.group(3),
                            "rx": re.compile(m.group(4)) if m.group(4) else None, "reason": reason.strip(), "used": False, "n": n})
    return entries


def allowed(entries, f, src_line):
    for e in entries:
        if not fnmatch.fnmatch(f.path, e["glob"]):
            continue
        if e["line"] is not None and e["line"] != f.line:
            continue
        if e["rule"] and e["rule"] != f.rule:
            continue
        if e["rx"] and not e["rx"].search(src_line):
            continue
        e["used"] = True
        return True
    return False


def git_files(root):
    """The repo's tracked and not-ignored files (relative, /), or None when root isn't a git work tree."""
    import subprocess
    try:
        out = subprocess.run(["git", "-C", root, "ls-files", "-co", "--exclude-standard", "-z"], capture_output=True, check=True)
    except (OSError, subprocess.CalledProcessError):
        return None
    if not os.path.exists(os.path.join(root, ".git")):
        return None
    return set(out.stdout.decode("utf-8", "replace").split("\0"))


def scan(root, paths=None, allow_path=None):
    """(findings, stale allowlist entries). paths: files or directories to limit the scan to."""
    allow = parse_allow(allow_path if allow_path is not None else os.path.join(root, "i18n", "lint-allow.txt"))
    files = []
    targets = paths or [os.path.join(root, r) for r in ROOTS]
    tracked = git_files(root)   # generated and ignored files (the apps build copies SodiumUI.FileChooser into ui/) are not source
    for t in targets:
        t = os.path.abspath(t)
        if os.path.isfile(t):
            files.append(t)
            continue
        for d, dirs, names in os.walk(t):
            dirs[:] = sorted(x for x in dirs if x not in SKIP_DIRS and not x.startswith("build"))
            for nme in sorted(names):
                ext = os.path.splitext(nme)[1]
                rel = os.path.relpath(os.path.join(d, nme), root).replace(os.sep, "/")
                if tracked is not None and rel not in tracked:
                    continue
                if ext in CODE_EXT:
                    files.append(os.path.join(d, nme))
    findings = []
    for fp in files:
        rel = os.path.relpath(fp, root).replace(os.sep, "/")
        comp = component_of(rel)
        try:
            with open(fp, encoding="utf-8") as fh:
                text = fh.read()
        except (OSError, UnicodeDecodeError):
            continue
        lines = text.splitlines()
        raw = []

        def add(path, line, rule, msg, c, severity=None, _raw=raw):
            _raw.append(Finding(path, line, rule, msg, c, severity))

        ext = os.path.splitext(fp)[1]
        for n, line in enumerate(lines, 1):
            if "\u00ad" in line or "\u200b" in line:
                add(rel, n, "soft-hyphen", "a soft hyphen or phrase mark in source: translators place them in the catalogs", comp)
        if ext == ".json":
            check_json(rel, text, comp, add)
        else:
            code = strip_comments(lines)
            if ext in (".qml", ".js"):
                check_qml(rel, lines, code, comp, add)
            if ext == ".qml":
                check_placeholder_labels(rel, code, comp, add)
            else:
                check_cpp(rel, lines, code, comp, add)
            check_common(rel, code, comp, add)
            check_british_source(rel, code, comp, add)
        seen_f = set()
        for f in raw:
            if (f.line, f.rule, f.msg) in seen_f:
                continue
            seen_f.add((f.line, f.rule, f.msg))
            src = lines[f.line - 1] if 0 < f.line <= len(lines) else ""
            if not allowed(allow, f, src):
                findings.append(f)
    return findings, [e for e in allow if not e["used"]]


def main(argv):
    args = argv[1:]
    strict = "--strict" in args
    summary = "--summary" in args
    opts = {}
    rest = []
    it = iter(args)
    for a in it:
        if a in ("--component", "--rule", "--allow", "--root"):
            opts[a] = next(it, "")
        elif a not in ("--strict", "--summary"):
            rest.append(a)
    root = os.path.abspath(opts.get("--root", tslib.REPO))
    findings, stale = scan(root, rest or None, opts.get("--allow"))
    if "--component" in opts:
        findings = [f for f in findings if f.component == opts["--component"]]
    if "--rule" in opts:
        findings = [f for f in findings if f.rule == opts["--rule"]]
    by_comp = defaultdict(Counter)
    for f in findings:
        by_comp[f.component][f.rule] += 1
    rules = sorted({f.rule for f in findings})
    if findings:
        w = max(len(c) for c in by_comp) + 2
        print("component".ljust(w) + "total  " + "  ".join(rules))
        for c in sorted(by_comp):
            print(c.ljust(w) + str(sum(by_comp[c].values())).rjust(5) + "  " + "  ".join(str(by_comp[c][r]).rjust(len(r)) for r in rules))
        print("TOTAL".ljust(w) + str(len(findings)).rjust(5))
        errors = sum(1 for f in findings if f.severity == "error")
        print(f"{errors} findings, {len(findings) - errors} warnings")
    else:
        print("i18n lint: no findings")
    if not summary:
        for f in sorted(findings, key=lambda f: (f.component, f.path, f.line)):
            print(f)
    for e in stale:
        print(f"i18n/lint-allow.txt:{e['n']}: stale entry (nothing matched): {e['glob']}")
    return 1 if strict and (findings or stale) else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
