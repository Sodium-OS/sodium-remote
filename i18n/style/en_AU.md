# Style guide: English (Australia), en_AU

`en_AU.ts` holds only overrides and loads on top of en_GB (decisions 1 and 10). A string missing here is shown from
en_GB, then from the en_US source. Write a string in en_AU only when Australian usage differs from British usage.
Glossary: `i18n/glossary/en_AU.tsv` (all rows "= en_GB" today).

## Rule of thumb
- Start from en_GB. Do not copy en_GB strings into en_AU just to have them there: an empty override is a bug the
  CI gate treats as noise.
- Add a message only for a real difference. At launch there are none in the core glossary.

## Where Australian usage differs (add overrides here when a string needs them)
- Spelling: -ise/-our/-re as en_GB. Software is a "program" in both.
  "Licence" noun and "license" verb as en_GB. "Tyre", "kerb", "cheque" are not UI words; ignore.
- Vocabulary: "Wi-Fi", "hotspot", "sign out" are shared. For storage and removal Australian OS UIs say "Bin" in macOS
  and "Recycle Bin" in Windows; we keep Trash as in en_GB (decision 1 note).
- Date and time order, 12/24 h and first weekday come from Region (en_AU region: day/month/year), not from this file.
- Quotation marks: single ‘…’ first, as en_GB. Apostrophe ’.
- Dash, ellipsis, sentence case, percent ("38%"), sizes, durations, key caps, placeholders, plurals (one, other):
  exactly as en_GB. See `i18n/style/en_GB.md`.

## Register and length
- Same register as en_GB: plain and impersonal, no slang ("arvo", "servo"), no contractions beyond ones in the source.
- Never abbreviate. Pseudo-locale bands apply unchanged.

## Process
- The translator writes `en_AU.ts` by diffing against en_GB: if after reading the string a native Australian would
  change nothing, leave it out. The reviewer checks the overrides file only.
- Tag every override in the translator comment with the reason ("AU vocabulary" or "AU spelling").

## Never translated
- Product names (SodiumOS, Sodium Shell, Sodium UI), command names, file extensions, key letters, URLs, paths.

## Model strings
| en_GB (inherited) | en_AU |
|---|---|
| Colour and glass | = en_GB |
| Favourites | = en_GB |
| Move %1 to the Trash? | = en_GB |
| %n items | = en_GB |
| Minimise | = en_GB |
