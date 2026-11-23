# Style guide: 한국어 (ko_KR)

Glossary: `i18n/glossary/ko_KR.tsv`.

## Register and address (decision 8)
- 합니다체 (하십시오체): "~합니다", "~할 수 없습니다", "~하시겠습니까?". Not 해요체, not 반말.
- Impersonal where possible, no pronoun (never 당신, 사용자님 only as a noun when the source says "user").
- Buttons and menu items are nouns or the noun form of the verb: "저장", "닫기", "이름 바꾸기", "복사". No 합니다 on buttons.
- Questions to the user: "~하시겠습니까?" (honorific -시-, because the dialog speaks to the user).
  Errors and states: plain statements, "비밀번호가 올바르지 않습니다."
- Success toasts: past, "~했습니다": "휴지통으로 이동했습니다".
- Avoid passive overload ("~되었습니다") where an active form is natural.

## Writing system
- Hangul with Sino-Korean words in Hangul. Hanja never. Loanwords as spelled in the national standard
  (디스플레이, 다운로드, 스크린샷, 스토어, 브라우저 for -er words ending in 어).
- Latin names stay Latin: Wi-Fi, Bluetooth, VPN, USB, Dock, GPU.

## Capitalisation
- None. No uppercase transform, no tracking, no italics (decision 21). Emphasis by weight only.

## Punctuation
- Western punctuation: , . ? ! : ; ( ) with ASCII width. Korean text does not use full-width CJK punctuation.
- Quotes: ‘…’ for names and terms, “…” for quoted sentences, straight or typographic as the font supports (typographic).
- Ellipsis: single … (U+2026), no space before: "이름 바꾸기…", "연결하는 중…".
- Full stop (.) on full sentences only; none after buttons, labels, menu items, one-clause tooltips.
- Commas separate list items; the last item joins with 및 in titles ("언어 및 시간") and with 와/과 or 및 in prose.
- Ranges: "9–17시" with an en dash. Colon after a form label has a no-space: "이름:" only when the source has one.

## Spacing (띄어쓰기)
- Korean separates words with spaces. Counters take a space after the number in the unit form used by Windows and
  macOS: "항목 %n개" (counter attached to the number), "%n개 항목" also valid. We choose "항목 %n개" for lists and
  "%n개의 항목" in sentences; keep one shape per context.
- Particles attach to the noun with no space: "%1을(를)", "앱에서", "폴더로".
- Latin or digits followed by a Korean noun: a space ("Wi-Fi 설정", "스페이스 %1"). Particles attach directly.
- Dependent nouns (수, 것, 때) take a space before them: "~할 수 없습니다".

## Particles with placeholders
- 을/를, 이/가, 은/는 depend on the last sound of the word inserted at %1, which is unknown at translation time.
- First choice: rephrase so a fixed noun sits between %1 and the particle ("%1 항목을") or the particle is invariant
  Invariant particles (safe after any word): 에, 에서, 의, 도, 만, 부터, 까지. Variable ones (을/를, 이/가, 은/는,
  와/과, (으)로) need the slash form below, or use 및 instead of 와/과.
- Second choice: the slash form 을(를), 이(가), 은(는), (으)로: this is Microsoft and Apple practice for unknown words.
  Use it, do not guess one. Example: "%1을(를) 휴지통으로 이동했습니다".
- Never choose a single form based on the likely value.

## Numbers, percent, sizes, durations (decision 34)
- Western digits always. Separators from Region.
- Percent: "38%" with no space.
- Sizes: "4.2 MB" with a no-break space; units stay Latin.
- Durations translated, spaced: "2시간 5분", "30초", "3일".
- Counter 개 for items ("%n개"), 건 for events and results, 명 for people, 대 for devices, 장 for pictures.

## Key caps (decision 32)
- Caps as printed: Ctrl, Shift, Alt, Super, Enter, Delete, Esc. Only Space is translated: 스페이스. Tab, Backspace as printed.
- Separator " + ". Names enter sentences through `%1`: "%1을(를) 눌러 런처 열기" (rephrase to "%1: 런처 열기" if possible).
- Shortcut letters are never localised.

## Plurals and placeholders
- Qt numerus forms for ko: 1 (other). One `<numerusform>`.
- Word order is SOV with the verb last; `%1` moves toward the front, before its particle.
- Never build a sentence from fragments.

## Names
- SodiumOS, Sodium Shell, Sodium UI stay Latin. App names are translated (decision 9): 설정, 파일, 계산기, 캘린더, 메모.
- Inside a sentence an app name is plain with its particle: "파일에서 보기". Use ‘ ’ only to prevent misreading.
- Third-party apps keep their vendor name.

## Vocabulary choices where Windows and macOS differ
- 설정 (both). 휴지통 (both). 창 (Windows) over 윈도우 (macOS). 폴더; 파일; 앱 (both).
- 접근성 (Windows) over 손쉬운 사용 (macOS). 비밀번호 (macOS) over 암호 (Windows).
- 어둡게 / 밝게 (Windows) over 다크 / 라이트. 방해 금지 모드 (macOS). 야간 모드 (Windows). 볼륨 over 음량.
- Restore (window) is 이전 크기로 복원; Put back is 되돌려 놓기 (macOS); Undo is 실행 취소.
- Bluetooth stays Latin as Windows prints it; macOS Korean says 블루투스. Native check wanted.
- 시스템 종료 for Shut down (both); 로그아웃 for Sign out; 다시 시작 for Restart; 절전 for Sleep.

## Length
- Never abbreviate. Korean is shorter in characters than English but syllable blocks are wide; check the CJK
  pseudo-locale for clipping and wrapping (decision 19). Hangul wraps at spaces: keep phrases short.

## Never translated
- Product names, command names, file extensions, key letters, URLs, paths, Wi-Fi, Bluetooth, VPN, USB, GPU, CPU,
  kitty, starship, Rime.

## Model strings
| Source (en_US) | ko_KR |
|---|---|
| Move %1 to the Trash? | %1을(를) 휴지통으로 이동하시겠습니까? |
| Moved %1 to the Trash | %1을(를) 휴지통으로 이동했습니다 |
| %n items | 항목 %n개 |
| Wrong password. | 비밀번호가 올바르지 않습니다. |
| This folder is empty | 이 폴더는 비어 있습니다 |
| Formats, holidays and the currency follow it | 형식, 공휴일, 통화는 이 설정을 따릅니다 |
| Apps that start when you sign in | 로그인할 때 시작되는 앱 |
| Connect automatically | 자동으로 연결 |
| Extract to… | 압축 풀 위치… |
| Super + V clipboard · Super + . emoji | Super + V 클립보드 · Super + . 이모지 |
| Keep both | 둘 다 유지 |
| Search settings | 설정 검색 |
| Open in %1 | %1에서 열기 |
| Applies when the app reopens | 앱을 다시 열면 적용됩니다 |
| Display language | 표시 언어 |
