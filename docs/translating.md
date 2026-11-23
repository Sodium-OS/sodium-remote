# Translating and writing UI for ten languages

Every screen of Sodium Remote supports all ten interface languages, and each app can run in its own language, whatever the system's:

- English: US (the source language), UK and Australia (`en_US`, `en_GB`, `en_AU`);
- Chinese, Simplified (`zh_CN`) and Traditional (`zh_TW`);
- Japanese (`ja_JP`), Korean (`ko_KR`), Spanish (`es_ES`), French (`fr_FR`) and German (`de_DE`).

This page has the rules for anyone who writes or changes interface, and how to fix or add a translation. A change that breaks the rules doesn't merge.

## Where the translations are

| What | Where |
|---|---|
| The Qt app (Linux, Windows) | The `remote` catalog: `i18n/ts/remote_<lang>.ts` |
| Sodium UI, the vendored library | The `sodiumui` catalog: `i18n/ts/sodiumui_<lang>.ts` |
| The Mac app | `client-mac/App/Localizable.xcstrings` |
| Glossary and style guides | `i18n/` (terms per language, and register, punctuation and number rules per language) |

Qt's catalogs are `.ts` files, kept in git; the compiled `.qm` files are made at build time and never committed.

## The tools

The repository includes tools to **extract** strings from the source into the catalogs (keeping existing translations), **check** the catalogs (every language present, placeholders and plurals match the source, nothing unfinished, punctuation, glossary terms) and **release** them (compile `.ts` to `.qm`). The Mac build checks that every key in the string catalog has all ten languages, and fails if one is missing. All of them run through one script, `i18n/i18n.sh`:

| Command | What it does |
|---|---|
| `i18n/i18n.sh update` | Extracts the strings into `i18n/ts/<catalog>_<language>.ts`, keeping translations and review states (needs `lupdate6` from `qt6-tools`) |
| `i18n/i18n.sh check --strict` | The lint for untranslated UI code, then the catalog checks. Needs no Qt tools |
| `i18n/i18n.sh release --strict` | Compiles the catalogs to `.qm` files in `build/i18n/qm` (needs `lrelease6`) |
| `i18n/i18n.sh generate` | Writes the `.desktop` file with every translation, into `build/i18n/desktop` |

Run `i18n/i18n.sh` with no arguments for the other commands.

## Fixing or adding a translation

1. Open the catalog for the language (`i18n/ts/remote_de_DE.ts`, for example) in Qt Linguist or a text editor, and change the translation. Don't change the source text: that is changed in code, then the catalog is updated.
2. Use the glossary: product names (Sodium Remote, SodiumOS, Sodium UI) and OS names are never translated. Follow the language's style guide.
3. Keep every placeholder (`%1`, `%n`) and make plurals right for the language: Japanese, Chinese and Korean have one form; the European languages have two or more.
4. Run `i18n/i18n.sh check --strict`, then open a pull request. Fixes to a translation are welcome and need no code.

A new language needs its catalogs, a glossary and style guide, a native review, and a layout check; open an issue first.

## Rules for writing UI

### 1. Every visible string is translatable

- **QML:** `qsTr("Move to Trash")`. **C++:** `tr("…")` in a `QObject`, `QCoreApplication::translate("Context", "…")` elsewhere. **Swift:** `Text("literal")` is localised by SwiftUI; our own components take `LocalizedStringKey`, not `String`.
- Strings defined away from where they are shown (arrays, models) use `QT_TRANSLATE_NOOP` and are translated when shown.
- Tooltips, accessible names and descriptions, placeholders, notifications, toasts, and empty and error states are all strings.
- **Never translated:** SodiumOS, Sodium Remote, Sodium UI, file extensions, command names, keyboard shortcut letters, and codec names.

### 2. Write the source in US English, so it can be translated

- **US spelling** (Color, License, Center). The British text is a translation (`en_GB`).
- **Sentence case**, short, no "please", curly quotes and ’.
- **Whole sentences with placeholders.** Never concatenate: `qsTr("Opens %1").arg(name)`, not `"Opens " + name`. Lists are a whole message.
- **Plurals use `%n`:** `qsTr("%n attempt(s) left", "", count)`. Never `count === 1 ? … : …`.
- **Give translators context** for short or ambiguous words: a `//:` comment above the line, or the disambiguation argument.
- Don't build a string from parts of other messages. "Shut down" and "Shut down?" are two messages.

### 3. Never use text as data

- Don't compare translated strings. Compare ids, enums or roles. A disconnect reason is a code the UI turns into words, never text the UI matches.
- Don't store translated text (in settings, history, logs). Store an id and its arguments, and translate when showing.
- Language changes must update what is on screen: a QML binding on `qsTr` does it by itself; C++ that caches text must handle `QEvent::LanguageChange`.
- Don't match another program's window or app by its translated title.

### 4. Dates, numbers and sizes

Dates, times, numbers, percentages, sizes, durations and currency go through Sodium UI's `Format` functions (`Format.number`, `Format.size`, `Format.duration`, `Format.relativeTime`, …), never by concatenation or hard-coded patterns. French and German write "38 %", Japanese "2時間5分". Sorting and searching names use Sodium UI's `Sort` and `Match`.

### 5. Layout that survives German and Japanese

German runs about 30% longer than English, and short strings up to twice as long; Chinese and Japanese need more height.

- **Size to the content.** Never give text a fixed width. Fixed widths become minimums.
- **Buttons** never elide or wrap. A row of buttons that doesn't fit stacks full width.
- **Menus** are 200–420 px, **dialogs** grow to 640 px. Segmented controls become a dropdown when they don't fit; tabs scroll; chips wrap.
- **Descriptions and messages wrap.** No fixed heights on text containers.
- **Only sidebars and table headers elide** UI labels, and always with a tooltip showing the full text. User content (names, titles) may elide.
- **No abbreviations to make something fit.** Rephrase, or change the layout.
- **Short wrapped labels** use Sodium UI's `WrappedLabel`, which balances lines so Chinese and Japanese never leave one character alone.
- **Type:** use the theme's font styles, never set a font family. Uppercase comes only from a type style, never from the text. No italics for emphasis in text that may be Chinese, Japanese or Korean; use weight. No letter spacing outside type styles.
- **Right-to-left** isn't supported yet, but write for it: use layouts and anchors rather than x arithmetic, and "leading" and "trailing" rather than left and right.

### 6. Design with the tightest languages first

A new screen is designed with its tightest parts in German and Japanese next to English.

## Checking a screen

- **Pseudo-locales** exercise length and wrapping: set the application's UI language to the accented, expanded pseudo-locale (`qps-ploc`) or the Chinese, Japanese and Korean one (`qps-ploca`), and run your screens in both. In Sodium UI the variable is `SODIUMOS_UI_LANGUAGE`.
- **The overflow audit** (`SODIUMOS_I18N_AUDIT=1`) outlines and logs every label that is elided, clipped or overflowing, with its source string. The goal is zero in all ten languages and both pseudo-locales.
- Tests never assert on translated text. Use object names, ids or roles, and run in the source language.

## Review states

Each translation has a review state: `machine`, `reviewed` or `native`. A machine translation is a starting point; wording can still change while the project is a preview.
