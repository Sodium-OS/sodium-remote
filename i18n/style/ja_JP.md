# Style guide: 日本語 (ja_JP)

Glossary: `i18n/glossary/ja_JP.tsv`. Kana readings for our app names are in its notes column (decision 26).

## Register and address (decision 8)
- です/ます style (敬体), plain: "〜してください", "〜できません". No heavy keigo (no お/ご overload, no いたします).
- Prefer impersonal phrasing and no pronoun: never あなた. "アカウント", not "あなたのアカウント".
- Buttons and menu items are nouns or the dictionary form: "保存", "閉じる", "名前を変更", "ごみ箱に入れる". No ます on buttons.
- Questions: "〜しますか？" Confirmations and errors: "〜できません。" "〜が見つかりません。"
- Success toasts: past polite, short: "%1をごみ箱に入れました".
- Prefer active verbs. Avoid "〜することができます" when "〜できます" is enough.

## Writing system
- Kanji, hiragana, katakana as the Japanese UI convention: common kanji in 常用漢字, no rare kanji; functional words
  (ください, できる, ほか, すでに) in kana. 「および」「または」 in kana; 「又は」「及び」 not used.
- Katakana for loanwords. Long vowel mark ー is kept for words ending in -er, -or, -ar of three or more morae
  (ユーザー, フォルダー, プリンター, ブラウザー, コンピューター). Exceptions with a dropped mark follow Windows
  (メモリ, ピクチャ). We keep one rule across the UI (the glossary decides); do not mix フォルダ with フォルダー.
- Half-width katakana never. Full-width Latin never; Latin letters and digits are half-width.

## Capitalisation
- None in Japanese. Latin names keep their own case. No uppercase transform, no tracking, no italics (decision 21).
- Do not use bold-for-emphasis in place of italics inside strings.

## Punctuation
- Full-width: 、。（）「」？！ and the middle dot ・. Quotes are 「 」, nested 『 』. Do not use “ ”.
- "？" and "！" full-width, no space after. A question that ends a sentence keeps "？" (Windows and macOS both do).
- Ellipsis: the single … (U+2026) for "needs more input" ("名前を変更…", "接続中…"). Not "。。。" and not "...".
- 。 only on full sentences; none after buttons, menu items, labels or one-clause tooltips.
- Brackets （ ） full-width around Japanese; half-width ( ) around Latin or digits only.
- Wave dash 〜 (U+301C) only for ranges, never in labels. Prefer "から": "9時から17時".
- Colon: drop label colons ("言語", not "言語："). Where a colon is needed, use half-width ":" after Latin text and
  full-width "：" after Japanese text.

## Spacing between Japanese and Latin or digits
- No space (decision 81). macOS and Microsoft's current style guide set "Wi-Fiの設定", "Bluetoothデバイス", "%n個の項目", "スペース%1"; older Windows strings still carry the space.
  Spaces make the line wrap oddly in Japanese typesetting. (Chinese and Korean have different rules.)
- Exception: a space is kept inside a Latin product name ("Sodium Shell"), in the key-cap separator " + ", and before a number that a file name carries as a copy stamp ("新しいフォルダー %1", as Files names a second copy).
- Between two Latin tokens that are separate words, keep the space ("Super + V クリップボード" has spaces around +).

- A quoted name (a `%1` whose comment says "%1 is a quoted name", wrapped in 「」 by the code): no space around it: 「写真.jpg」をコピーできません (decision 86).

## Numbers, percent, sizes, durations (decision 34)
- Half-width Western digits always. Separators from Region.
- Percent: "38%" with the half-width sign, no space.
- Sizes: "4.2 MB" with a no-break space before the Latin unit; units stay Latin.
- Durations are translated: "2時間5分", "30秒", "3日". No spaces. Day counters: 日, 週間, か月 (hiragana か), 年.
- Counters: 個 for items ("%n個の項目"), 件 for results or messages, 台 for devices, 枚 for pictures.

## Key caps (decision 32)
- Caps as printed: Ctrl, Shift, Alt, Super, Enter, Delete, Esc. Only Space is translated: スペース.
- Separator " + ". Names enter sentences through `%1`: "%1でランチャーを開く".
- Shortcut letters are never localised.

## Plurals and placeholders
- Qt numerus forms for ja: 1 (other). One `<numerusform>`.
- Word order is SOV: `%1` often moves to the front. The particle follows `%1` directly with no space: "%1で開く",
  "%1をごみ箱に入れました", "%1を削除". Choose the particle (を, で, に, の) that fits whatever %1 stands for.
- When %1 may be a count ("3個の項目") or a name, write the noun after it only if needed: "%1をごみ箱に入れますか？"
  already works for both.
- Never build a sentence from fragments.

## Names
- SodiumOS, Sodium Shell, Sodium UI stay Latin. App names are translated (decision 9): 設定, ファイル, 電卓, カレンダー,
  メモ. Each carries a kana reading in the glossary for search (hiragana: ふぁいる).
- Inside a sentence an app name is written plain: "ファイルで表示". Use 「 」 only to prevent misreading.
- Third-party apps keep their vendor name.

## Vocabulary choices where Windows and macOS differ
- 設定 (Windows) for Settings (macOS: システム設定). ごみ箱 (both). ウィンドウ; フォルダー (Windows) over フォルダ (macOS).
- Cut: 切り取り / Paste: 貼り付け (Windows) over カット / ペースト (macOS). 検索 for both Find and Search.
- サインアウト (Windows) over ログアウト (macOS). 既定のアプリ (Windows) over デフォルトのアプリケーション.
- 入力方式 for input method, 入力ソース (macOS) for an input source entry. Dock as written in Latin (macOS).
- Restore (window) is 元のサイズに戻す; Undo is 元に戻す; Put back is also 元に戻す (macOS), told apart by context.
- 応答不可 (Windows) for Do not disturb; 夜間モード; 明るさ (Windows) over 輝度.

## Length
- Never abbreviate. Japanese is shorter in characters than English but kanji need height: check the CJK pseudo-locale
  for vertical clipping, line height and wrapping without spaces (decision 19).
- Line breaks: Qt breaks between any two characters in Japanese. Keep words that must not split inside one run
  (use U+2060 word joiner only if the audit finds a bad break).

## Never translated
- Product names, command names, file extensions, key letters, URLs, paths, Wi-Fi, Bluetooth, VPN, USB, GPU, CPU,
  kitty, starship, Rime.

## Model strings
| Source (en_US) | ja_JP |
|---|---|
| Move %1 to the Trash? | %1をごみ箱に入れますか？ |
| Moved %1 to the Trash | %1をごみ箱に入れました |
| %n items | %n個の項目 |
| Wrong password. | パスワードが違います。 |
| This folder is empty | このフォルダーは空です |
| Formats, holidays and the currency follow it | 形式、祝日、通貨はこの設定に従います |
| Apps that start when you sign in | サインイン時に起動するアプリ |
| Connect automatically | 自動的に接続 |
| Extract to… | 展開先… |
| Super + V clipboard · Super + . emoji | Super + V クリップボード · Super + . 絵文字 |
| Keep both | 両方を残す |
| Search settings | 設定を検索 |
| Open in %1 | %1で開く |
| Applies when the app reopens | アプリを開き直すと反映されます |
| Display language | 表示言語 |
