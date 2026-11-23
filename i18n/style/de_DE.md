# Style guide: Deutsch (de_DE)

Germany only (decision 4); Austrian and Swiss formats come from Region. Glossary: `i18n/glossary/de_DE.tsv`.

## Register and address (decision 8)
- Sie, capitalised (Sie, Ihr, Ihnen). Impersonal wherever possible: "Falsches Passwort." not "Sie haben ein falsches
  Passwort eingegeben."
- Buttons and menu items: infinitive or noun: "Speichern", "Schließen", "Umbenennen", "Beide behalten".
  Not the imperative "Speichere", and not "Bitte speichern".
- Instructions in sentences use the Sie imperative sparingly: "Wählen Sie eine Sprache." Prefer the impersonal
  "Sprache auswählen" in titles.
- Questions: "%1 in den Papierkorb legen?" (infinitive question). Success toasts: perfect/preterite passive,
  "%1 wurde in den Papierkorb gelegt".
- Gender-inclusive language: no asterisk, colon or Binnen-I. Use neutral forms where they exist ("Konto", "Person",
  "Team", "Mitglieder"); the generic masculine "Benutzer" is the established UI term and stays.

## Capitalisation
- All nouns are capitalised (Datei, Ordner, Papierkorb); verbs, adjectives, prepositions are not, except the first word.
- Sentence case otherwise: "Anzeigesprache", "In den Papierkorb legen", "Möglicherweise nicht übersetzt".
- Nominalised verbs and adjectives are capitalised ("Das Öffnen", "Alles auswählen" has lower-case "auswählen" as a verb).
- Uppercase and tracking come from the UI (decision 21). ß becomes SS in uppercase (accepted).

## Compounds
- German compounds are written as one word: "Dateiauswahl", "Task-Manager". Join with a hyphen only when a
  word is Latin or an acronym ("App-Sprachen", "WLAN-Hotspot", "VPN-Verbindung", "USB-Stick"), or for readability
  (three or more elements). Never "Datei Auswahl" (the Deppenleerzeichen).
- Very long compounds may carry a soft hyphen (U+00AD) at a morpheme boundary so Qt can wrap them:
  "Barriere­freiheit". Use this only if the length audit flags a clip.

## Punctuation
- Quotes: „…“ (U+201E opening, U+201C closing); nested ‚…‘. File names in running text: „%1“ only when the source quotes them.
- Ellipsis: single … (U+2026), no space before: "Umbenennen…", "Verbindung wird hergestellt…".
- Apostrophe: ’ only for omitted letters, rare in German UI.
- Dash: spaced en dash " – " in running text, avoided in UI strings. Range: "9–17 Uhr". Hyphen in compounds.
- Full stop on full sentences only; none after buttons, menu items, labels or short tooltips.
- Comma before subordinate clauses ("dass", "wenn", "weil") and relative clauses is mandatory; no comma before "und"/"oder"
  in a list: "A, B und C".
- Colon and semicolon: no space before.

## Numbers, percent, sizes, durations (decision 34)
- Western digits. Separators from Region (Germany: decimal comma, dot grouping).
- Percent: "38 %" with U+202F narrow no-break space (DIN 5008, decision 34).
- Sizes: "4,2 MB" with a no-break space before the unit; units are not translated (KB, MB, GB, TB).
- Durations are translated: "2 h 5 min" (symbols per DIN), long form "2 Stunden 5 Minuten".
- Temperatures "20 °C" with a no-break space. 24-hour time ("14:05 Uhr") and date order come from Region.

## Key caps (decision 32)
- German caps: Strg, Umschalt, Alt, Entf, Eingabe, Esc, Leertaste. Super is the fixed key name (decision 37). Tab; Rücktaste (Backspace).
- Separator " + ". Names enter sentences through `%1`: "%1 drücken, um den Launcher zu öffnen" (never type Strg in the sentence).
- Shortcut letters and mnemonics are never localised.

## Plurals and placeholders
- Qt numerus forms for de: 2 (one, other). "%n Element" / "%n Elemente". Always `%n`, never `n === 1 ? …`.
- Grammatical cases: the placeholder inserts a name (nominative). Sentences in which `%1` would need a case ending
  must use a fixed noun in front: "Datei „%1“ löschen?" not "%1 löschen?" when %1 could be "3 Dateien".
- Articles: avoid an article before `%1` (gender unknown): "In %1 öffnen", "%1 wurde in den Papierkorb gelegt".
- Verb at the end of subordinate clauses: move `%1` and verbs to fit; keep %1, %2 numbering.
- Never build a sentence from fragments.

## Names
- SodiumOS, Sodium Shell, Sodium UI stay Latin, no inflection. App names are translated (decision 9): Einstellungen, Dateien,
  Rechner, Kalender, Notizen. "Store" and "Task-Manager" follow Windows.
- With an app name in a sentence: capitalised, no quotes: "In Dateien anzeigen".
- Third-party apps keep their vendor name.

## Vocabulary choices where Windows and macOS differ
- Desktop (Windows, GNOME) over Schreibtisch (macOS). Papierkorb (both). Einstellungen (both).
- Barrierefreiheit (Windows, Android) over Bedienungshilfen (macOS). Bildschirme over Displays.
- Mitteilungen (macOS, 12 characters) over Benachrichtigungen (Windows, 17); native review wanted. Passwort (macOS)
  over Kennwort (Windows). Sound (Windows) over Ton. Updates over Softwareupdate. Info over Über.
- Rückgängig / Wiederholen (Undo / Redo, Windows and macOS). Zurücklegen (macOS) for Put back; Wiederherstellen is
  the window Restore. Herunterfahren; Neu starten; Abmelden; Energiesparmodus.
- Arbeitsbereich for a space (not "Desktop", which is taken).
- Ablehnen for Deny; Nicht erlauben is the macOS wording.

## Length
- Never abbreviate: not "Einst.", "Ben.", "Ctrl.". German runs 30–40 % longer than English: rephrase with a shorter word
  (a shorter compound) or redesign the layout (decision 19, 20). The pseudo-locale is the contract.

## Never translated
- Product names, command names, file extensions, key letters, URLs, paths, Bluetooth, VPN, USB, GPU, CPU, kitty,
  starship, Rime. (Wi-Fi is the one name German writes differently: WLAN, as Windows and macOS do.)

## Model strings
| Source (en_US) | de_DE |
|---|---|
| Move %1 to the Trash? | %1 in den Papierkorb legen? |
| Moved %1 to the Trash | %1 wurde in den Papierkorb gelegt |
| %n items | %n Elemente (one: %n Element) |
| Wrong password. | Falsches Passwort. |
| This folder is empty | Dieser Ordner ist leer |
| Formats, holidays and the currency follow it | Formate, Feiertage und Währung richten sich danach |
| Apps that start when you sign in | Apps, die bei der Anmeldung starten |
| Connect automatically | Automatisch verbinden |
| Extract to… | Entpacken nach… |
| Super + V clipboard · Super + . emoji | Super + V Zwischenablage · Super + . Emoji |
| Keep both | Beide behalten |
| Search settings | Einstellungen durchsuchen |
| Open in %1 | In %1 öffnen |
| Applies when the app reopens | Wird beim erneuten Öffnen der App übernommen |
| Display language | Anzeigesprache |
