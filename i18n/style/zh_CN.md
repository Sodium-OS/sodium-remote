# Style guide: 简体中文 (zh_CN)

Mainland vocabulary and Simplified characters. Serves China and Singapore (decision 2). Glossary: `i18n/glossary/zh_CN.tsv`.
Never mix in Taiwan words (文件 not 檔案, 窗口 not 視窗, 软件 not 軟體, 设置 not 設定, 默认 not 預設).

## Register and address (decision 8)
- No pronoun. Write impersonal imperatives and noun phrases: "保存", "选择文件", "无法连接到网络".
- If a pronoun cannot be avoided, use 您 (never 你). Write "您的账户" only where the source has "Your account".
- Buttons and menu items are verbs or nouns, no 请 and no sentence particles: "保存", not "请保存".
- Questions end in 吗: "要将 %1 移到回收站吗？" Messages and errors are plain statements: "密码错误。"
- Avoid 了 in success toasts unless needed for natural tense ("已将 %1 移到回收站": use 已 before the verb).

## Capitalisation
- Chinese has none. Latin words keep their own case: Wi-Fi, Bluetooth, Dock is not used (程序坞).
- No uppercase transform, no tracking, no italics (decision 21). Emphasis uses weight only.

## Punctuation
- Inside Chinese text use full-width: ，。、：；？！（）and “ ” for quotes (mainland standard); nested quotes ‘ ’.
- Do not use 「」 (that is Taiwan). Book titles and file names use plain quotes “%1” only if the source quotes them.
- Ellipsis: a single … (U+2026) for "needs more input" items ("重命名…", "解压到…") and for progress ("正在连接…").
  Do not type six dots (……) in UI labels.
- Full stop 。 ends full sentences only; none after buttons, menu items, labels or short tooltips.
- Brackets: （）full-width around Chinese content; half-width () only when the whole bracket is Latin or numeric.
- A sentence that ends in a Latin word or a number still ends with 。（full-width）.
- Dash: use no dash; if the source has a spaced dash, restructure with ，or ：. Range: "9–17 时" with an en dash.
- Colon: full-width ： after a Chinese label; half-width : only in times and ratios.

## Spacing between Chinese and Latin or digits
- One half-width space between Chinese characters and adjacent Latin letters or digits: "终端和 Shell", "%n 个项目",
  "应用 %1". This is Windows and macOS practice for Simplified Chinese UI and keeps mixed runs readable.
- No space between a Chinese character and full-width punctuation, nor between a digit and a unit symbol (%, °).
- Quotation marks are punctuation: no space outside or inside “ ” ‘ ’, Latin names included: 无法复制“照片.jpg” (decision 86). A `%1` that arrives already quoted (its comment says "%1 is a quoted name") takes no space either: "无法复制%1". Only where a curly quote would touch a Latin letter or digit outside it ("Wi-Fi “Home”"), keep one half-width space on that side.
- Product names stay Latin with the same rule: "SodiumOS 设置".

## Numbers, percent, sizes, durations (decision 34)
- Western digits always. Digit grouping and decimal marks come from Region; never type them in a string.
- Percent: "38%" (no space).
- Sizes: "4.2 MB" with a no-break space before the unit; units stay Latin (KB, MB, GB, TB).
- Durations are translated and spaced: "2 小时 5 分钟", "30 秒". Short form in tight places: the same, never "2h".
- "几个" counters: use the measure word that fits the object: 个 for items, 张 for pictures, 条 for lines, 份 for copies.
  If the noun is a placeholder (%1) use 个 with a neutral noun ("%n 个项目").

## Key caps (decision 32)
- Caps as printed: Ctrl, Shift, Alt, Super, Enter, Delete, Esc. Only Space is translated: 空格. Tab and Backspace as printed.
- Separator " + " (spaces on both sides). Key names enter sentences through `%1` only: "按 %1 打开启动器".
- Shortcut letters and mnemonics stay Latin and are never localised.

## Plurals and placeholders
- Qt numerus forms for zh: 1 (other). One `<numerusform>`; write the sentence so it reads for 1 and for 100: "%n 个项目".
- `%1` after a Chinese noun: add a space ("项目 %1"). `%n` before a measure word: "%n 个".
- Word order differs from English: use %1, %2 in the order the sentence needs; never rely on English order.
- No plural marking is needed (no 们 for things). Use 所有 or 多个 where the source implies several.

## Names
- SodiumOS, Sodium Shell, Sodium UI stay Latin. App names are translated (decision 9): 设置, 文件, 计算器, 日历, 笔记.
- Quote an app name with “ ” when it sits inside a sentence and could read as a plain noun: 在“文件”中显示.
- Third-party apps keep their vendor name, Latin: Chrome, Firefox.

## Vocabulary choices where Windows and macOS differ
- 回收站 (Windows) over 废纸篓 (macOS); 设置 (both); 程序坞 (macOS) for Dock; 屏幕 over 荧幕; 鼠标; 窗口;
  复制 over 拷贝; 粘贴; 夜览 (macOS) for Night light; 辅助功能; 隐私和安全 ("和" over "与" in titles, as Windows).
- "应用" for app (Windows 应用; macOS 应用程序 is too long). "文件" is both Files app and "file"; "文档" is Documents.
- 账户 (Windows) over 帐户 (old) and 账号.

## Length
- Never abbreviate. Chinese runs 40–60 % shorter than English in characters but needs equal width per glyph; check the
  CJK pseudo-locale for vertical clipping (decision 19). Prefer the 2–4 character term.
- Two equally good words: pick the shorter (关机, not 关闭计算机).

## Never translated
- Product names, command names (`systemctl`), file extensions (.zip, .tar.gz), environment variables, key letters,
  URLs, paths, Wi-Fi, Bluetooth, VPN, USB, GPU, CPU, kitty, starship, Rime.

## Model strings
| Source (en_US) | zh_CN |
|---|---|
| Move %1 to the Trash? | 要将 %1 移到回收站吗？ |
| Moved %1 to the Trash | 已将 %1 移到回收站 |
| %n items | %n 个项目 |
| Wrong password. | 密码错误。 |
| This folder is empty | 此文件夹为空 |
| Formats, holidays and the currency follow it | 格式、节假日和货币将随之变化 |
| Apps that start when you sign in | 登录时启动的应用 |
| Connect automatically | 自动连接 |
| Extract to… | 解压到… |
| Super + V clipboard · Super + . emoji | Super + V 剪贴板 · Super + . 表情符号 |
| Keep both | 保留两者 |
| Search settings | 搜索设置 |
| Open in %1 | 在“%1”中打开 |
| Applies when the app reopens | 应用重新打开后生效 |
| Display language | 显示语言 |
