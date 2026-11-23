# Style guide: English (UK), en_GB

Source language is en_US (decision 1). `en_GB.ts` is complete: every source string is present, even when identical.
Glossary: `i18n/glossary/en_GB.tsv`. The en_AU catalog loads on top of this one.

## What changes from en_US
- Spelling: British -ise/-isation (Minimise, Personalise, Customise, Organise), -our (Colour, Favourite, Behaviour),
  -re (Centre), Licence (noun) and License (verb), Grey, Catalogue, Programme only for TV or events (software is
  "program"), Cancelled, Labelled, Travelling, Modelling, Analyse, Defence, Enquiry for questions of fact.
- Vocabulary: keep Trash (not "Bin"), Wi-Fi, Hotspot, Sign out, Shut down. Do not switch to Bin, Mobile data, Lift, Torch.
  Apart from the spelling pairs, nearly every string is identical to the source; copy it, do not rephrase.
- Do not translate between the two: if the source reads well in British English with only a spelling change, change only that.

## Register and address
- Neutral, plain, impersonal. Prefer no pronoun: "Wrong password." not "You entered the wrong password."
- "you" and "your" only where the source has them ("Your account"). Never "we".
- Buttons: verb in the imperative or a noun, no full stop: "Save", "Cancel", "Keep both".

## Capitalisation
- Sentence case everywhere: only the first word and proper nouns. "Show in Files", "Move to Trash", "Do not disturb".
- Fixed capitalised nouns: Trash, Home, Desktop, Files and every app name when it names the app, Wi-Fi, Bluetooth.
- Never capitalise for emphasis. Uppercase and tracking come from the UI (decision 21), not from the text.

## Punctuation
- Apostrophe is the typographic ’ (U+2019): "Don’t save", "When it’s idle". Never the straight '.
- Quotation marks: single ‘…’ for quotes, double “…” inside them. For file names use ‘%1’ only when the source does.
- Ellipsis is the single character … (U+2026), no space before it: "Rename…". It marks an action that asks for more input.
- Dashes: spaced en dash " – " for a break in a sentence; unspaced en dash for ranges (9–5). No em dash.
- One space after a full stop. No full stop on buttons, menu items, labels, tooltips of one clause. Full stop on full sentences.
- Serial comma: not used ("red, green and blue").
- Lists in running text: "A, B and C".

## Numbers, units, durations (decision 34)
- Formats come from Region, never from the language: do not write digit grouping or decimal marks yourself.
- Percent: "38%", no space. Sizes: "4.2 MB" with a no-break space before the unit; units are not translated.
- Durations: "2 h 5 min"; long form "2 hours 5 minutes".
- Western digits. Time-of-day and dates are produced by `Format`; never type "3 pm" in a string.

## Key caps (decision 32)
- Key names come from the `KeyCaps` context and enter sentences only through `%1`.
- Caps as printed: Ctrl, Shift, Alt, Super, Enter, Delete, Esc, Space, Tab, Backspace. Separator " + ".
- Shortcut letters and mnemonics are never changed.

## Plurals, placeholders
- Qt numerus forms for en: 2 (one, other). `%n item` / `%n items`; always use `%n` with `tr(…, n)`, never `n === 1 ? …`.
- en_US.ts holds only the plural forms; en_GB must supply both forms for every `%n` string.
- `%1`, `%2` keep their numbers. Do not reorder placeholders unless the sentence needs it, then keep the numbers.
- Never build a sentence from fragments (" and ", "Opens " + name).

## Names
- SodiumOS, Sodium Shell, Sodium UI: always as written, no article change. Say "the Sodium Shell" only if the source does.
- App names follow the glossary and are capitalised: Files, Calculator, Task Manager.
- Third-party names as the vendor writes them (Wi-Fi, Bluetooth, kitty, starship, Rime).

## Length
- Never abbreviate to fit. If a string is too long, rephrase with a shorter word, not "Cfg", "Msg" or a trailing dot.
- The pseudo-locale is the contract (decision 19). British spellings are the same length or one character longer.

## Never translated
- Product names, command names (`ls`, `systemctl`), file extensions (.tar.gz), environment variables, keyboard letters,
  URLs, paths, key IDs, the words "kitty", "starship", "Rime".

## Model strings
| Source (en_US) | en_GB |
|---|---|
| Move %1 to the Trash? | Move %1 to the Trash? |
| Moved %1 to the Trash | Moved %1 to the Trash |
| %n items | %n item / %n items |
| Wrong password. | Wrong password. |
| This folder is empty | This folder is empty |
| Color and glass | Colour and glass |
| Accent color | Accent colour |
| Minimize | Minimise |
| Favorites | Favourites |
| Personalize | Personalise |
| Apps that start when you sign in | Apps that start when you sign in |
| Keep both | Keep both |
| Applies when the app reopens | Applies when the app reopens |
| Control Center | Control Centre |
