# Style guide: español (es_ES)

Spain only at launch (decision 3). Choose neutral words where it costs nothing so an es-419 catalog can later be a
small override. Glossary: `i18n/glossary/es_ES.tsv`.

## Register and address (decision 8)
- Informal tú, and impersonal wherever possible. Avoid vosotros forms entirely (Spain-only) so es-419 does not break.
- Buttons and menu items: infinitive or noun: "Guardar", "Cerrar", "Cambiar nombre". Never imperative "Guarda".
- Instructions that need a verb to the user use tú imperative sparingly: "Elige un idioma". Errors are impersonal:
  "Contraseña incorrecta." not "Has escrito mal la contraseña."
- Questions: "¿Mover %1 a la papelera?" Success toasts: "Se ha movido %1 a la papelera" (pretérito perfecto, as in Spain;
  es-419 would use pretérito simple).
- Gender: use the grammatical gender of the noun. Where %1 is unknown, prefer a sentence that does not need agreement
  ("Se ha movido %1 …" rather than "%1 ha sido movido").

## Neutral vocabulary (decision 3)
- equipo (not ordenador/computadora); archivo (not fichero); carpeta; aplicación; papelera; ratón (Spain; es-419 override: mouse);
  contraseña; correo; descargar; ajustes/configuración see below.
- Prefer "Vídeos" with accent (RAE old form, Windows es-ES) as in the glossary; es-419 uses Videos.

## Capitalisation
- Sentence case: only the first word and proper nouns. Months, days, languages, and nationalities lower case.
  "Idioma de visualización", "Mover a la papelera", "Modo de concentración".
- Not English Title Case. App names as the glossary writes them are capitalised (Archivos, Calculadora, Ajustes) when they
  name the app; the noun lowercase when it is generic ("archivo").
- Uppercase and tracking come from the UI (decision 21); ß-type issues do not arise. Accents are kept in uppercase
  (RAE): "ACTUALIZACIONES".

## Punctuation
- Opening ¿ and ¡ are required: "¿Mover %1 a la papelera?" "¡Listo!" (use "Listo" without exclamation in buttons).
- Quotes: angle quotes « » (RAE), nested “ ” then ‘ ’. File names in running text: «%1» only when the source quotes them.
- Ellipsis: single … (U+2026), no space before: "Cambiar nombre…", "Conectando…".
- Dash: spaced em dash — only in running text, avoided in UI strings. Range: en dash "9–17 h".
- Full stop on full sentences only; none after buttons, menu items, labels or short tooltips.
- Comma: before "pero", never before "y" / "o" in a list. "A, B y C".
- Colon: no space before. Lower case after a colon unless a full sentence follows.

## Numbers, percent, sizes, durations (decision 34)
- Western digits. Separators from Region (Spain: decimal comma, thin grouping, four-digit numbers ungrouped).
- Percent: "38 %" with U+202F narrow no-break space before the sign (RAE rule, decision 34).
- Sizes: "4,2 MB" with a no-break space; units are not translated. Space before units, not before ° and %.
- Durations are translated: "2 h 5 min" (symbols h, min), long form "2 horas y 5 minutos".
- 24-hour time and date order come from Region.

## Key caps (decision 32)
- Spanish caps: Ctrl, Mayús, Alt, Supr, Intro, Esc, Espacio. Super is the fixed key name (decision 37). Tab, Retroceso (Backspace).
- Separator " + ". Names enter sentences through `%1`: "Pulsa %1 para abrir el lanzador" (never type "Ctrl" in the sentence).
- Shortcut letters and mnemonics are never localised.

## Plurals and placeholders
- Qt numerus forms for es: lupdate lists the CLDR forms (one, many, other in Qt 6.5+; older Qt: 2). Write "one" for 1;
  "many" (multiples of a million) and "other" are the plural: "%n elementos". Use "de" only in the many form where it
  changes ("%n millones de elementos"); otherwise copy "other".
- Gender and number agree with `%n`/`%1`: write sentences that work for both ("%n elementos" has no adjective).
- Word order: Spanish puts adjectives after nouns ("Aplicaciones predeterminadas"); keep `%1`, `%2` numbering.
- Never build a sentence from fragments.

## Names
- SodiumOS, Sodium Shell, Sodium UI stay Latin, no article change. App names are translated (decision 9): Ajustes, Archivos,
  Calculadora, Calendario, Notas, Archivador.
- With an app name in a sentence: capitalised, no quotes: "Mostrar en Archivos"; add the article only when natural
  ("Abrir en la aplicación %1" where %1 is a name).
- Third-party apps keep their vendor name.

## Vocabulary choices where Windows and macOS differ
- Ajustes (macOS Spain, Android; 8 characters) over Configuración (Windows; 13). An es-419 override can use Configuración.
- Papelera (both). Archivo (both). Ventana. Escritorio. Descargas. Vista previa (macOS) / Vista previa (Windows).
- Suspender (Windows menu) for Sleep; macOS Spain says Reposo. Cerrar sesión; Reiniciar; Apagar.
- Fuentes de entrada (macOS) for Input sources. Luz nocturna (Windows) over Night Shift.
- Punto de acceso (macOS, Spain) over Zona con cobertura inalámbrica (Windows).
- Put back is "Volver a colocar" (macOS), not Restaurar, which is the window Restore.

## Length
- Never abbreviate: not "Config.", "Apps", "Ctrl." Spanish runs 20–30 % longer than English: rephrase with a shorter word
  ("Quitar" not "Eliminar de la lista") or redesign the layout (decision 19).

## Never translated
- Product names, command names, file extensions, key letters, URLs, paths, Wi-Fi, Bluetooth, VPN, USB, GPU, CPU, kitty,
  starship, Rime.

## Model strings
| Source (en_US) | es_ES |
|---|---|
| Move %1 to the Trash? | ¿Mover %1 a la papelera? |
| Moved %1 to the Trash | Se ha movido %1 a la papelera |
| %n items | %n elementos (one: %n elemento) |
| Wrong password. | Contraseña incorrecta. |
| This folder is empty | Esta carpeta está vacía |
| Formats, holidays and the currency follow it | Los formatos, los festivos y la moneda dependen de ella |
| Apps that start when you sign in | Aplicaciones que se abren al iniciar sesión |
| Connect automatically | Conectar automáticamente |
| Extract to… | Extraer en… |
| Super + V clipboard · Super + . emoji | Super + V portapapeles · Super + . emojis |
| Keep both | Conservar ambos |
| Search settings | Buscar en Ajustes |
| Open in %1 | Abrir en %1 |
| Applies when the app reopens | Se aplica al volver a abrir la aplicación |
| Display language | Idioma de visualización |
