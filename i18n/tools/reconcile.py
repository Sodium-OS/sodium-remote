#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
"""After lupdate6 has rewritten i18n/ts in place (run by `scripts/i18n.sh reconcile`).

    reconcile.py --before DIR [--ts-dir DIR]

DIR holds the .ts files as they were before lupdate. For every file now in the ts dir:
  - a message that is unfinished and empty takes the translation of a message with the same source
    text and disambiguation that is obsolete or vanished (now, or only in the file from before),
    preferring the same context; its review state (<extra-sodium-review>) comes with it;
  - a message whose translation lupdate dropped but that is unchanged gets it back from DIR;
  - <extra-sodium-review> is restored from DIR when lupdate lost it.
Prints the number of obsolete messages left, unfinished messages and messages that moved to another
context (the target is zero unfinished).
"""
import argparse
import glob
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import tsio  # noqa: E402


def usable(m):
    return m.has_translation() and m.state in ("finished", "vanished", "obsolete")


def reattach(new, old):
    """Mutates `new` (a tsio.Ts); `old` is the file from before (or None). -> (reattached, context_changed)."""
    oldmap = {m.key: m for m in old.messages} if old else {}
    newkeys = {m.key for m in new.messages}
    # translations that have lost their message: obsolete/vanished ones now, and the old file's unmatched ones
    pool = [m for m in new.messages if m.state in ("obsolete", "vanished") and usable(m)]
    pool += [m for k, m in oldmap.items() if k not in newkeys and usable(m)]
    used, moved, back = set(), 0, 0
    for m in new.messages:
        o = oldmap.get(m.key)
        if o is not None and m.extra(tsio.REVIEW) is None and o.extra(tsio.REVIEW) is not None:
            m.extras.append((tsio.REVIEW, o.extra(tsio.REVIEW)))
        if m.state != "unfinished" or m.has_translation():
            continue
        if o is not None and o.state == "finished" and o.has_translation() and o.numerus == m.numerus:
            m.forms, m.state = list(o.forms), "finished"   # lupdate dropped it
            back += 1
            continue
        cands = [p for p in pool if id(p) not in used and p.source == m.source and p.disamb == m.disamb and p.numerus == m.numerus]
        if not cands:
            continue
        same = [p for p in cands if p.ctx == m.ctx]
        p = (same or cands)[0]
        used.add(id(p))
        m.forms, m.state = list(p.forms), "finished"
        if p.extra(tsio.REVIEW) is not None and m.extra(tsio.REVIEW) is None:
            m.extras.append((tsio.REVIEW, p.extra(tsio.REVIEW)))
        if p.ctx != m.ctx:
            moved += 1
    new.messages = [m for m in new.messages if id(m) not in used]
    return back + len(used), moved


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--before", required=True)
    ap.add_argument("--ts-dir", default="i18n/ts")
    a = ap.parse_args(argv)
    obsolete = unfinished = moved = attached = 0
    for path in sorted(glob.glob(os.path.join(a.ts_dir, "*.ts"))):
        new = tsio.read(path)
        before = os.path.join(a.before, os.path.basename(path))
        old = tsio.read(before) if os.path.exists(before) else None
        n, mv = reattach(new, old)
        attached, moved = attached + n, moved + mv
        tsio.write(path, new)   # writes only when the text changed (restored review states count)
        obsolete += sum(1 for m in new.messages if m.state in ("obsolete", "vanished"))
        unfinished += sum(1 for m in new.messages if m.state == "unfinished")
    print(f"reconcile: {attached} translations re-attached; {obsolete} obsolete, {unfinished} unfinished, {moved} context-changed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
