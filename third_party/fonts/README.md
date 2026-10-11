# Bundled fonts

Fallback faces for editing text (`src-tauri/src/textedit/bundled.rs`), compiled into the app from
`src-tauri/fonts/`. They are used when a document's own font has no glyph for typed text; only the
glyphs a block uses are embedded into the PDF (a runtime subset, Type0/Identity-H with ToUnicode).

| File | Source | Licence |
|---|---|---|
| NotoSans-Regular/Bold/Italic/BoldItalic.ttf | notofonts/notofonts.github.io `fonts/NotoSans/hinted/ttf` @ 75ea688 (Noto Sans 2.015) | OFL 1.1, `OFL-NotoSans.txt` |
| NotoSansMono-Regular/Bold.ttf | same repository, `fonts/NotoSansMono/hinted/ttf` (Noto Sans Mono 2.014) | OFL 1.1, `OFL-NotoSans.txt` |
| NotoSansCJK-Regular.otf | notofonts/noto-cjk `Sans/OTF/SimplifiedChinese/NotoSansCJKsc-Regular.otf` @ f8d1575 (2.004) | OFL 1.1, `OFL-NotoSansCJK.txt` |

The files are subsets made at build time (by a developer, committed) with `scripts/fonts/subset_fonts.py`
(fontTools `pyftsubset`, no hinting, no layout tables; the `name` table is kept whole, so each file carries
its copyright and licence records):

- Noto Sans and Noto Sans Mono: Latin (Basic to Extended-B, IPA, Latin Extended Additional), combining marks,
  Greek, Cyrillic, punctuation, currency, letterlike symbols, number forms, arrows, mathematical operators,
  enclosed alphanumerics, box drawing, geometric shapes, miscellaneous symbols, Latin ligatures, U+FFFD.
  About 135 KB each, 820 KB together.
- Noto Sans CJK (SC glyph forms): every character of GB 2312, JIS X 0208 and KS X 1001 (9,788 Han ideographs,
  2,350 Hangul syllables, kana, CJK punctuation and symbols; the list is `scripts/fonts/cjk-chars.txt`), plus
  the space, CJK symbols, kana, Bopomofo, Hangul compatibility jamo and half/full-width forms. 2.8 MB.

Total: 3.5 MB in the executable. Text outside these sets (rare Han ideographs, other scripts) is refused with a
message naming the first character no bundled face has.

The licence files ship with the app: `src-tauri/tauri.conf.json` bundles them as resources under `licenses/`
(with `third_party/THIRD-PARTY-NOTICES.txt`, which lists them).
