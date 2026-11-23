# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""Reading and writing Qt Linguist .ts (version 2.1) files the way lupdate writes them, keeping what
lupdate keeps: translations, their state, <translatorcomment> and every <extra-*> element
(the review state <extra-sodium-review> machine|reviewed|native among them)."""
import os
import xml.etree.ElementTree as ET

REVIEW = "extra-sodium-review"


class Msg:
    def __init__(self, ctx, source, disamb=""):
        self.ctx, self.source, self.disamb = ctx, source, disamb
        self.extracomment = ""
        self.translatorcomment = ""
        self.locations = []        # [(filename, line)]
        self.numerus = False
        self.forms = [""]          # the translation (one string, or the numerus forms)
        self.state = "unfinished"  # finished | unfinished | vanished | obsolete
        self.extras = []           # [(tag, text)] tag starts with "extra-"

    @property
    def key(self):
        return (self.ctx, self.source, self.disamb)

    def extra(self, tag):
        for t, v in self.extras:
            if t == tag:
                return v
        return None

    def has_translation(self):
        return any(f.strip() for f in self.forms)


class Ts:
    def __init__(self, language, sourcelanguage="en_US"):
        self.language, self.sourcelanguage = language, sourcelanguage
        self.messages = []   # Msg, in file order


def _text(el):
    """Text of an element, with <byte value="x9"/> turned back into its character."""
    out = [el.text or ""]
    for c in el:
        if c.tag == "byte":
            v = c.get("value", "")
            try:
                out.append(chr(int(v[1:], 16) if v[:1] == "x" else int(v[1:], 8) if v[:1] == "o" else int(v)))
            except ValueError:
                pass
        out.append(c.tail or "")
    return "".join(out)


def read(path):
    root = ET.parse(path).getroot()
    ts = Ts(root.get("language", ""), root.get("sourcelanguage", "en_US"))
    for c in root.findall("context"):
        name = _text(c.find("name")) if c.find("name") is not None else ""
        for m in c.findall("message"):
            src = m.find("source")
            msg = Msg(name, _text(src) if src is not None else "", _text(m.find("comment")) if m.find("comment") is not None else "")
            if m.find("extracomment") is not None:
                msg.extracomment = _text(m.find("extracomment"))
            if m.find("translatorcomment") is not None:
                msg.translatorcomment = _text(m.find("translatorcomment"))
            msg.locations = [(loc.get("filename", ""), loc.get("line", "")) for loc in m.findall("location")]
            msg.numerus = m.get("numerus") == "yes"
            tr = m.find("translation")
            msg.state = "finished"
            if tr is not None:
                msg.state = tr.get("type") or "finished"
                msg.forms = [_text(f) for f in tr.findall("numerusform")] if msg.numerus else [_text(tr)]
                if msg.numerus and not msg.forms:
                    msg.forms = [""]
            msg.extras = [(e.tag, _text(e)) for e in m if e.tag.startswith("extra-")]
            ts.messages.append(msg)
    return ts


def esc(s):
    out = []
    for ch in s:
        if ch == "&":
            out.append("&amp;")
        elif ch == "<":
            out.append("&lt;")
        elif ch == ">":
            out.append("&gt;")
        elif ch == '"':
            out.append("&quot;")
        elif ch == "'":
            out.append("&apos;")
        elif ch in "\n":
            out.append(ch)
        elif ord(ch) < 0x20 or ord(ch) == 0x7F:
            out.append(f'<byte value="x{ord(ch):x}"/>')
        else:
            out.append(ch)
    return "".join(out)


def render(ts):
    """The .ts text: contexts in order of first appearance, messages in file order."""
    ctxs = {}
    for m in ts.messages:
        ctxs.setdefault(m.ctx, []).append(m)
    o = ['<?xml version="1.0" encoding="utf-8"?>', "<!DOCTYPE TS>",
         f'<TS version="2.1" language="{ts.language}" sourcelanguage="{ts.sourcelanguage}">']
    for name, msgs in ctxs.items():
        o.append("<context>")
        o.append(f"    <name>{esc(name)}</name>")
        for m in msgs:
            o.append('    <message numerus="yes">' if m.numerus else "    <message>")
            for fn, ln in m.locations:
                o.append(f'        <location filename="{esc(fn)}" line="{ln}"/>' if ln != "" else f'        <location filename="{esc(fn)}"/>')
            o.append(f"        <source>{esc(m.source)}</source>")
            if m.disamb:
                o.append(f"        <comment>{esc(m.disamb)}</comment>")
            if m.extracomment:
                o.append(f"        <extracomment>{esc(m.extracomment)}</extracomment>")
            if m.translatorcomment:
                o.append(f"        <translatorcomment>{esc(m.translatorcomment)}</translatorcomment>")
            attr = "" if m.state == "finished" else f' type="{m.state}"'
            if m.numerus:
                forms = "".join(f"<numerusform>{esc(f)}</numerusform>" for f in m.forms)
                o.append(f"        <translation{attr}>{forms}</translation>")
            else:
                o.append(f"        <translation{attr}>{esc(m.forms[0] if m.forms else '')}</translation>")
            for tag, val in m.extras:
                o.append(f"        <{tag}>{esc(val)}</{tag}>")
            o.append("    </message>")
        o.append("</context>")
    o.append("</TS>")
    return "\n".join(o) + "\n"


def write(path, ts):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    text = render(ts)
    if os.path.exists(path):
        with open(path, encoding="utf-8") as f:
            if f.read() == text:
                return False
    with open(path + ".tmp", "w", encoding="utf-8") as f:
        f.write(text)
    os.replace(path + ".tmp", path)
    return True
