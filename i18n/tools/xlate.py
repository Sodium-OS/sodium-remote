#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Moving translations in and out of i18n/ts/*.ts in batches, for translators who work on text.

    xlate.py stats  [LANG...]                              counts per catalog: total, unfinished, machine, reviewed, native
    xlate.py export LANG [-c CATALOG] [--state unfinished|machine|reviewed|all] [--limit N] [--blind] [-o FILE]
    xlate.py compare LANG BACK.tsv [-o FILE]                for the second review: pair a back-translation with the source
    xlate.py import LANG FILE [--state machine|reviewed|native]
    xlate.py mark   LANG FILE --state reviewed|native      set the review state of the ids in FILE (first column)

The exchange format is TSV, one message per line:
    id  catalog  context  disambiguation  comment  numerus  source  translation
- id is stable: the first 12 hex digits of sha1(catalog, context, source, disambiguation);
- comment holds the developer and translator comments (and the plural rule hint for numerus messages);
- numerus is "yes" or "no"; a numerus translation gives its forms separated by " || " in plural-form order
  (one form for zh/ja/ko; singular || plural for en/de/es; n<=1 || n>1 for fr);
- tabs, newlines and backslashes inside fields are written as \\t, \\n and \\\\.
import fills the translation, marks the message finished and sets its review state (default machine);
an empty translation is skipped. Unknown ids are reported and skipped. Files are rewritten in place.
"""
import argparse
import glob
import hashlib
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import tsio  # noqa: E402

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
TS_DIR = os.path.join(REPO, "i18n", "ts")
LANGS = ["en_GB", "en_AU", "zh_CN", "zh_TW", "ja_JP", "ko_KR", "es_ES", "fr_FR", "de_DE"]
FORMS = {"zh_CN": 1, "zh_TW": 1, "ja_JP": 1, "ko_KR": 1}
PLURAL_HINT = {"fr_FR": "forms: n<=1 || n>1", "zh_CN": "one form", "zh_TW": "one form", "ja_JP": "one form", "ko_KR": "one form"}


def msg_id(catalog, m):
    return hashlib.sha1("\x1f".join((catalog, m.ctx, m.source, m.disamb)).encode()).hexdigest()[:12]


def esc(s):
    return s.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n")


def unesc(s):
    out, i = [], 0
    while i < len(s):
        if s[i] == "\\" and i + 1 < len(s):
            out.append({"t": "\t", "n": "\n", "\\": "\\"}.get(s[i + 1], s[i + 1]))
            i += 2
        else:
            out.append(s[i])
            i += 1
    return "".join(out)


def files(lang, catalog=None):
    pat = "%s_%s.ts" % (catalog or "*", lang)
    for path in sorted(glob.glob(os.path.join(TS_DIR, pat))):
        cat = os.path.basename(path)[:-len("_%s.ts" % lang)]
        yield cat, path


def live(m):
    return m.state not in ("vanished", "obsolete")


def review_state(m):
    return m.extra(tsio.REVIEW) or ("machine" if m.has_translation() and m.state == "finished" else "")


def set_review(m, state):
    m.extras = [(t, v) for t, v in m.extras if t != tsio.REVIEW] + [(tsio.REVIEW, state)]


def cmd_stats(args):
    langs = args.langs or LANGS
    print("%-8s %-16s %6s %10s %8s %9s %7s" % ("lang", "catalog", "total", "unfinished", "machine", "reviewed", "native"))
    for lang in langs:
        tot = [0, 0, 0, 0, 0]
        for cat, path in files(lang):
            ts = tsio.read(path)
            c = [0, 0, 0, 0, 0]
            for m in ts.messages:
                if not live(m):
                    continue
                c[0] += 1
                if m.state == "unfinished" or not m.has_translation():
                    c[1] += 1
                    continue
                r = review_state(m)
                c[{"machine": 2, "reviewed": 3, "native": 4}.get(r, 2)] += 1
            tot = [a + b for a, b in zip(tot, c)]
            if args.verbose:
                print("%-8s %-16s %6d %10d %8d %9d %7d" % (lang, cat, *c))
        print("%-8s %-16s %6d %10d %8d %9d %7d" % (lang, "ALL", *tot))


def cmd_export(args):
    out = open(args.output, "w", encoding="utf-8", newline="") if args.output else sys.stdout
    # plain tab-separated lines: fields are escaped by esc() (\t \n \\), so no CSV quoting (a " in a source broke it)
    class _W:
        @staticmethod
        def writerow(cols):
            out.write("\t".join(cols) + "\n")
    w = _W()
    n = 0
    for cat, path in files(args.lang, args.catalog):
        for m in tsio.read(path).messages:
            if not live(m):
                continue
            unfinished = m.state == "unfinished" or not m.has_translation()
            r = "" if unfinished else review_state(m)
            if args.state == "unfinished" and not unfinished:
                continue
            if args.state in ("machine", "reviewed") and r != args.state:
                continue
            comment = " | ".join(x for x in (m.extracomment, m.translatorcomment) if x)
            if m.numerus:
                comment = (comment + " | " if comment else "") + PLURAL_HINT.get(args.lang, "forms: n==1 || n!=1")
            tr = " || ".join(m.forms) if m.numerus else m.forms[0]
            # --blind (the blind back-translation): the source and the comments stay out
            w.writerow([msg_id(cat, m), cat, esc(m.ctx), "" if args.blind else esc(m.disamb), "" if args.blind else esc(comment),
                        "yes" if m.numerus else "no", "" if args.blind else esc(m.source), "" if unfinished else esc(tr)])
            n += 1
            if args.limit and n >= args.limit:
                break
        if args.limit and n >= args.limit:
            break
    if out is not sys.stdout:
        out.close()
    print("exported %d messages" % n, file=sys.stderr)


def read_rows(path):
    rows = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if not line.strip():
                continue
            cols = line.split("\t")
            if len(cols) < 8:
                rows[cols[0]] = None
                continue
            rows[cols[0]] = unesc(cols[7])
    return rows


def apply(lang, rows, state, set_text):
    seen, changed = set(), 0
    for cat, path in files(lang):
        ts = tsio.read(path)
        touched = False
        for m in ts.messages:
            if not live(m):
                continue
            i = msg_id(cat, m)
            if i not in rows:
                continue
            seen.add(i)
            if set_text:
                text = rows[i]
                if not text:
                    continue
                if m.numerus:
                    forms = [f.strip() for f in text.split(" || ")]
                    want = FORMS.get(lang, 2)
                    if len(forms) < want:
                        forms += [forms[-1]] * (want - len(forms))
                    m.forms = forms[:max(want, 1)]
                else:
                    m.forms = [text]
                m.state = "finished"
            elif not m.has_translation():
                continue
            set_review(m, state)
            touched = True
            changed += 1
        if touched:
            tsio.write(path, ts)
    missing = [i for i in rows if i not in seen]
    print("%s: %d messages updated; %d unknown ids%s" % (lang, changed, len(missing), (": " + " ".join(missing[:10])) if missing else ""),
          file=sys.stderr)


def cmd_compare(args):
    """BACK.tsv: an export --blind file whose column 7 (the source column) the reviewer filled with an English
    back-translation. Writes id, catalog, context, source, translation, back-translation for reading side by side."""
    back = {}
    with open(args.file, encoding="utf-8") as f:
        for line in f:
            cols = line.rstrip("\n").split("\t")
            if len(cols) >= 8 and cols[6]:
                back[cols[0]] = (unesc(cols[6]), unesc(cols[7]))
    out = open(args.output, "w", encoding="utf-8") if args.output else sys.stdout
    n = 0
    for cat, path in files(args.lang):
        for m in tsio.read(path).messages:
            i = msg_id(cat, m)
            if live(m) and i in back:
                bt, tr = back[i]
                out.write("\t".join([i, cat, esc(m.ctx), esc(m.source), esc(tr), esc(bt)]) + "\n")
                n += 1
    if out is not sys.stdout:
        out.close()
    print("paired %d messages" % n, file=sys.stderr)


def cmd_import(args):
    apply(args.lang, read_rows(args.file), args.state, True)


def cmd_mark(args):
    apply(args.lang, read_rows(args.file), args.state, False)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("stats")
    s.add_argument("langs", nargs="*")
    s.add_argument("-v", "--verbose", action="store_true")
    s.set_defaults(func=cmd_stats)
    e = sub.add_parser("export")
    e.add_argument("lang")
    e.add_argument("-c", "--catalog")
    e.add_argument("--state", default="unfinished", choices=["unfinished", "machine", "reviewed", "all"])
    e.add_argument("--limit", type=int, default=0)
    e.add_argument("--blind", action="store_true", help="leave the source, disambiguation and comments out")
    e.add_argument("-o", "--output")
    e.set_defaults(func=cmd_export)
    c = sub.add_parser("compare")
    c.add_argument("lang")
    c.add_argument("file")
    c.add_argument("-o", "--output")
    c.set_defaults(func=cmd_compare)
    i = sub.add_parser("import")
    i.add_argument("lang")
    i.add_argument("file")
    i.add_argument("--state", default="machine", choices=["machine", "reviewed", "native"])
    i.set_defaults(func=cmd_import)
    m = sub.add_parser("mark")
    m.add_argument("lang")
    m.add_argument("file")
    m.add_argument("--state", required=True, choices=["reviewed", "native"])
    m.set_defaults(func=cmd_mark)
    args = p.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
