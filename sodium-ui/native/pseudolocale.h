// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// The pseudo-locales (docs/decisions/i18n.md, decision 19) as text transforms, and the English
// plural forms of the source. Pure functions: I18n's translator calls them, the tests call them.
//
//   qps-ploc   "Save" → "[Šåvé····]": Latin letters accented, padded with '·' by the English length
//              (≤ 10 characters +100 % and at least 4, 11–20 +80 %, 21–30 +60 %, 31–70 +40 %, > 70 +30 %),
//              bracketed. %1, %n, %L1, & mnemonics, HTML tags and entities stay as they are.
//   qps-ploca  every source word becomes full-width CJK filler (Han and katakana, about 0.7 characters per
//              source character, no spaces, so a line can break anywhere), in full-width brackets.
//
// Plurals first, as a real catalog would: "%n item(s)" is "%n item" for n = 1 and "%n items" otherwise.
#pragma once

#include <QString>

namespace pseudo {

/// "%n item(s)" → "%n item" (n = 1) / "%n items"; "(es)" likewise. n < 0: unchanged.
QString resolvePlural(const QString &source, int n);
/// The accented, expanded form (qps-ploc).
QString accented(const QString &source);
/// The CJK filler form (qps-ploca).
QString cjk(const QString &source);
/// How many pad characters qps-ploc adds for a source of `length` visible characters.
int padFor(int length);

} // namespace pseudo
