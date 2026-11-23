# Style guide: 繁體中文 (zh_TW)

Taiwan usage and Traditional characters. One catalog also serves Hong Kong and Macao (decision 2); Region HK gets HK
glyph forms and HK formats, not a vocabulary change. Glossary: `i18n/glossary/zh_TW.tsv`.
Never mix in mainland words (檔案 not 文件 for file, 視窗 not 窗口, 軟體 not 軟件, 設定 not 設置, 預設 not 默認,
滑鼠 not 鼠標, 螢幕 not 屏幕, 資料夾 not 文件夾, 影片 not 視頻, 品質 not 質量).

## Register and address (decision 8)
- No pronoun. Imperative or noun phrases: "儲存", "選擇檔案", "無法連線到網路".
- If a pronoun cannot be avoided: 您 (never 你). "您的帳號" only where the source has "Your account".
- Buttons and menu items: verbs or nouns, no 請. Questions end in 嗎: "要將 %1 移至垃圾桶嗎？"
- Success toasts: "已將 %1 移至垃圾桶".

## Capitalisation
- None in Chinese. Latin names keep their own case. No uppercase transform, no tracking, no italics (decision 21).

## Punctuation
- Full-width ，。、：；？！（）. Quotation marks are 「 」, nested 『 』 (Taiwan standard), never “ ”.
- Ellipsis: single … (U+2026): "重新命名…", "正在連線…". (U+22EF, ⋯, is a native-review option; we do not use it
  until a reviewer asks, because fonts without it fall back to another face.)
- Full stop 。 on full sentences only; none after buttons, menu items, labels or short tooltips.
- Brackets （）full-width around Chinese; half-width () only when the whole bracket is Latin or numeric.
- Enumeration comma 、 between list items in running text; "和" or "與" before the last item.
- Dash: avoid. Range: "9–17 時" with an en dash. Colon ： full-width, half-width only in times.
- Title conjunction: 與 in group and page titles (語言與時間); 和 in running text.

## Spacing between Chinese and Latin or digits
- One half-width space between Chinese and adjacent Latin letters or digits: "終端機與 Shell", "%n 個項目".
  This is Windows and macOS practice for Traditional Chinese UI.
- No space next to full-width punctuation, and none between a digit and %, °.

## Numbers, percent, sizes, durations (decision 34)
- Western digits always; separators from Region.
- Percent: "38%". Sizes: "4.2 MB" with a no-break space; units stay Latin.
- Durations are translated and spaced: "2 小時 5 分鐘", "30 秒".
- Measure words: 個 for items, 張 for pictures, 項 for formal items. "%n 個項目".

## Key caps (decision 32)
- Caps as printed: Ctrl, Shift, Alt, Super, Enter, Delete, Esc. Only Space is translated: 空白鍵.
- Separator " + ". Key names enter sentences through `%1` only: "按 %1 開啟啟動器".
- Shortcut letters are never localised. Taiwan says 快速鍵 for shortcut (not 快捷鍵).

## Plurals and placeholders
- Qt numerus forms for zh_TW: 1 (other). One `<numerusform>`; the sentence must read for any n.
- Add a space between `%1` and an adjacent Chinese character: "項目 %1", "%1 已移至垃圾桶".
- Except a `%1` that arrives already quoted (its comment says "%1 is a quoted name"): no space, and none inside or outside 「」: "無法拷貝%1" renders 無法拷貝「照片.jpg」 (decision 86).
- Word order: place %1, %2 where Chinese needs them, never copy English order.

## Names
- SodiumOS, Sodium Shell, Sodium UI stay Latin. App names are translated (decision 9): 設定, 檔案, 計算機, 日曆, 筆記.
- Quote an app name with 「 」 inside a sentence: 在「檔案」中顯示.
- Third-party apps keep their vendor name.

## Vocabulary choices where Windows and macOS differ
- 垃圾桶 (macOS, 3 characters) over 資源回收筒 (Windows, 5). 設定 (both). 帳號 (usual Taiwan usage) over 帳戶.
- 複製 over 拷貝 for Copy (Windows); Duplicate is 建立副本. 確定 (Windows) over 好 (macOS) for OK.
- 夜覽 (macOS) for Night light; 輔助使用 (macOS) over 協助工具 (Windows); 隱私權 as Windows and macOS both.
- 檢視 for View; 尋找 for Find; 搜尋 for Search; 取代 for Replace; 還原 for Restore; 套用 for Apply.
- 應用程式 for app (Taiwan does not shorten to 應用). 影片 for videos, 圖片 for pictures, 文件 for documents.
- Hong Kong users read 螢幕 / 檔案 / 軟體 without trouble; do not add HK words to this catalog.

## Length
- Never abbreviate. 2–4 character terms preferred. 應用程式 is long: where the source says "app" in a short label use
  應用程式 anyway; widen the layout, do not shorten the word.
- Check the CJK pseudo-locale for vertical clipping and line height (decision 19).

## Never translated
- Product names, command names, file extensions, environment variables, key letters, URLs, paths, Wi-Fi, Bluetooth,
  VPN, USB, GPU, CPU, kitty, starship, Rime.

## Model strings
| Source (en_US) | zh_TW |
|---|---|
| Move %1 to the Trash? | 要將 %1 移至垃圾桶嗎？ |
| Moved %1 to the Trash | 已將 %1 移至垃圾桶 |
| %n items | %n 個項目 |
| Wrong password. | 密碼錯誤。 |
| This folder is empty | 此資料夾是空的 |
| Formats, holidays and the currency follow it | 格式、假日與貨幣會隨之變更 |
| Apps that start when you sign in | 登入時啟動的應用程式 |
| Connect automatically | 自動連線 |
| Extract to… | 解壓縮到… |
| Super + V clipboard · Super + . emoji | Super + V 剪貼簿 · Super + . 表情符號 |
| Keep both | 兩者都保留 |
| Search settings | 搜尋設定 |
| Open in %1 | 在「%1」中開啟 |
| Applies when the app reopens | 應用程式重新開啟後生效 |
| Display language | 顯示語言 |
