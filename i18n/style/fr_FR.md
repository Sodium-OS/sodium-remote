# Style guide: français (fr_FR)

France only (decision 4); Canadian, Belgian and Swiss formats come from Region. Glossary: `i18n/glossary/fr_FR.tsv`.

## Register and address (decision 8)
- Vous, and impersonal wherever possible. Never tu.
- Buttons and menu items: infinitive or noun: "Enregistrer", "Fermer", "Renommer". Not the imperative "Enregistrez".
- Instructions to the user in sentences use the vous imperative sparingly: "Choisissez une langue". Prefer
  impersonal: "Mot de passe incorrect." not "Vous avez saisi un mauvais mot de passe."
- Questions: "Mettre %1 à la corbeille ?" (infinitive question). Success toasts: "%1 a été mis à la corbeille"
  (check agreement: see below).
- Inclusive writing: no point médian. Use epicene forms ("Utilisateur" generic, "Compte").

## Capitalisation
- Sentence case: only the first word and proper nouns. Do not capitalise months, days, languages or nationalities.
  "Langue d’affichage", "Mettre à la corbeille", "Mode Concentration" (a feature name, as macOS).
- Capitalise accented capitals: "À propos", "État", "Écran", "Échap" (required, not optional).
- App names as the glossary writes them are capitalised when they name the app (Fichiers, Calculatrice).
- Uppercase and tracking come from the UI (decision 21). Keep accents in uppercase.

## Punctuation (checked automatically, decision 4)
- U+202F narrow no-break space before `;` `!` `?` and before `:`. Decision 4 asks the narrow one before `: ; ! ?`
  (the full no-break space U+00A0 before `:` is tolerated by the check, but write U+202F).
  Example: "Mettre %1 à la corbeille ?" with U+202F before "?".
- Guillemets « » with a U+202F narrow no-break space inside each: « texte ». Nested quotes “ ” then ‘ ’.
- Ellipsis: single … (U+2026), no space before: "Renommer…", "Connexion…". No space before it, but a narrow
  space before a following `?` or `!` still applies.
- Apostrophe is typographic ’ (U+2019): "Écran d’accueil". Never the straight '.
- Percent: "38 %" with U+202F (decision 34). Same for other symbols that French separates (° C has a space too: "20 °C").
- Full stop on full sentences only; none after buttons, menu items, labels or short tooltips.
- Comma: no comma before "et" or "ou" in a list: "A, B et C". The comma has no space before it.
- Dash: spaced en dash " – " in running text, avoided in UI strings. Range: "9–17 h".

## Numbers, sizes, durations (decision 34)
- Western digits. Separators from Region (France: comma decimal, U+202F grouping).
- Sizes: "4,2 Mo", "1,5 Go", "320 Ko", with a no-break space before the unit. French uses octet units: ko, Mo, Go, To
  (decision 34). Bit rates keep b: "10 Mb/s" in networking.
- Durations are translated: "2 h 5 min", long form "2 heures 5 minutes". Zero and one are singular in French: "0 minute".
- 24-hour time and date order come from Region.

## Key caps (decision 32)
- French caps: Ctrl, Maj, Alt, Suppr, Entrée, Échap, Espace. Super is the fixed key name (decision 37). Tab; Retour arrière.
- Separator " + ". Names enter sentences through `%1`: "Appuyez sur %1 pour ouvrir le lanceur" (never type Ctrl in the sentence).
- Shortcut letters and mnemonics are never localised.

## Plurals and placeholders
- Qt numerus forms for fr: lupdate lists the CLDR forms (one, many, other in Qt 6.5+; older Qt: 2). Zero and one use
  the singular. Write "one" for 0 and 1 ("%n élément"); "many" (millions) copies "other" with "d’" where needed
  ("%n millions d’éléments"); "other" is "%n éléments".
- Agreement: a past participle agrees with its subject, and %1 may be masculine or feminine. The code therefore passes a
  noun phrase that is always masculine ("3 éléments", or a quoted file name after the noun "Fichier « %1 »"); write the
  sentence for that shape ("%1 a été mis à la corbeille"). If a string cannot be written that way, use a label form
  that needs no agreement: "Corbeille : %1".
- Contractions with placeholders: "de" + vowel becomes "d’". The code cannot elide, so rephrase to avoid "de %1".
- Never build a sentence from fragments.

## Names
- SodiumOS, Sodium Shell, Sodium UI stay Latin, no article change. App names are translated (decision 9): Paramètres, Fichiers,
  Calculatrice, Calendrier, Notes.
- With an app name in a sentence: capitalised, no quotes, with the preposition: "Afficher dans Fichiers", "Ouvrir dans %1".
- Third-party apps keep their vendor name.

## Vocabulary choices where Windows and macOS differ
- Paramètres (Windows, GNOME, Android; the glossary) over Réglages (macOS). Same length; business users know Paramètres.
- Corbeille (both). Fenêtre. Bureau. Fichier. Dossier. Téléchargements. Aperçu (macOS name for Preview).
- Se déconnecter (Windows) over Fermer la session (macOS). Redémarrer. Éteindre. Mettre en veille.
- Afficher (Windows) over Présentation (macOS) for the View menu. Annuler (Undo and Cancel use the same word; context separates them).
- Put back is "Remettre à sa place" (macOS), not Restaurer, which is the window Restore.
- Éclairage nocturne (Windows) over Night Shift. Point d’accès (both) for Hotspot. Mode Concentration (macOS).
- "Appareils" over "Périphériques" in titles ("Appareils" is shorter and current); "périphérique" for storage.

## Length
- Never abbreviate: not "Param.", "Appli", "Ctrl.". French runs 20–35 % longer than English: rephrase with a shorter word
  or redesign the layout (decision 19). The pseudo-locale is the contract.

## Never translated
- Product names, command names, file extensions, key letters, URLs, paths, Wi-Fi, Bluetooth, VPN, USB, GPU, CPU, kitty,
  starship, Rime.

## Model strings
| Source (en_US) | fr_FR (U+202F before ? : ; !) |
|---|---|
| Move %1 to the Trash? | Mettre %1 à la corbeille ? |
| Moved %1 to the Trash | %1 a été mis à la corbeille |
| %n items | %n éléments (one: %n élément) |
| Wrong password. | Mot de passe incorrect. |
| This folder is empty | Ce dossier est vide |
| Formats, holidays and the currency follow it | Les formats, les jours fériés et la devise en dépendent |
| Apps that start when you sign in | Applications lancées à la connexion |
| Connect automatically | Se connecter automatiquement |
| Extract to… | Extraire vers… |
| Super + V clipboard · Super + . emoji | Super + V presse-papiers · Super + . émojis |
| Keep both | Conserver les deux |
| Search settings | Rechercher dans les paramètres |
| Open in %1 | Ouvrir dans %1 |
| Applies when the app reopens | S’applique à la réouverture de l’application |
| Display language | Langue d’affichage |
