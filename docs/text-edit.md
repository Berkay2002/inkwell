# Edit existing text in place

Acrobat's "Edit text & images" for text (study: [study/02-edit.md](study/02-edit.md)). Engine only; the UI is not
built yet. Code: `src-tauri/src/textedit/`, adapted from pdfcraft `crates/edit/src/text.rs`, `crates/content` and
`crates/fonts` (b8ddf71, MIT OR Apache-2.0; see `third_party/pdfcraft/`). MuPDF stays the only engine that owns the
document: pages, resources, fonts and streams are read and written as MuPDF objects, pdfcraft's own COS layer is not used.

## Commands

| Command | Arguments | Returns |
|---|---|---|
| `text_blocks` | `doc, page` | `{ rev, blocks: TextBlock[], areas: Area[] }` |
| `text_edit` | `doc, rev, page, index, change` | `{ state: EditState, fallback: Fallback \| null }` |

- Coordinates are page space as the viewer shows it (points, origin top left, `/Rotate` applied): the space of
  `page_text`. Offsets and widths in `change` are in that space too.
- `TextBlock = { index, text, rect, lines: rect[], font, size, bold, italic, color, align, editable, issue, reason }`.
  `font` is the `/BaseFont` (subset prefix kept), `size` in points, `align` one of `left|center|right|justify`.
- `change = { text?, size?, color?, align?, offset?: [dx, dy], width? }`; unset fields keep the block's own. `text`
  may contain `\n` for hard line breaks; everything else reflows to the block's width.
- `index` belongs to the `rev` of the list it came from; a `text_edit` against an older `rev` is `Stale` (list again).
- Each `text_edit` is one edit-core step (`Engine::apply_custom`): one undo step labelled "Edit text on p. N",
  "Format text …", "Move text …" or "Resize text box …", kept by Save (incremental update), recorded in history.
- `fallback = { font, embedded, reason }` when the block's font could not show the new text (the UI's yellow `!`):
  `reason` names the original font and the first character it lacks.

## Design

1. **Reading** (`text.rs: read_page`). The page's content streams are joined (a page's streams are one stream split
   between any two tokens) and parsed into operators that keep their byte spans (`content.rs`). An interpreter tracks
   the graphics and text state (`q/Q`, `cm`, `BT`, `Tf`, `Tc/Tw/Tz/TL/Ts/Tr`, `Td/TD/Tm/T*`, fill colour) and turns
   every `Tj/TJ/'/"` into a run: text (ToUnicode, else the encoding with `/Differences`), box (font ascent/descent),
   baseline start/end, user-space size. Widths come from `/Widths`, `/W`/`/DW`, or for standard 14 fonts without
   widths from MuPDF's built-in faces (`font.rs`).
2. **Lines and blocks.** Runs in one text object, same font/size/colour/render mode and on one baseline (measured in
   the run's own text direction, so rotated text works) form a line. Consecutive lines with the same style, a shared
   left edge, centre or right edge and a regular gap (0.8-2.5 × size, ±20 %) form a block. Alignment is detected from
   the lines' edges (`justify` when every line but the last fills the width).
3. **Rewriting** (`rewrite_block`). The block's show operators are removed, and so is any other text drawn over the
   same area (fake bold, a duplicate copy), except invisible text. Where the block started, one new text object is
   inserted in its own graphics state: `q [cm] /TouchUp_TextEdit MP BT <state> Tm … ET Q` (Acrobat's marker). If the
   same `BT` showed text before the block, the object is split there (`ET … BT Tm`) to keep reading order. Lines are
   positioned with `Td` from the block's left edge in its text space; `center/right` offset each line; `justify`
   spreads the free width with `Tw` (single-byte fonts) or `TJ` gaps. Everything else in the stream is copied byte for
   byte. Changed streams become new Flate streams (a content stream may be shared with another page); the page's
   `/Contents` points at them.
4. **Fonts.** The block's own font is kept when every character has a code (reverse ToUnicode/encoding) and a glyph:
   a code with a zero width is missing, and for a subset simple font the embedded program (loaded by MuPDF) must have
   the glyph. A font without a space code (TeX) keeps its glyphs and places words apart with `TJ` gaps of ¼ em.
   Otherwise the text is set in the matching standard 14 font (Courier/Times/Helvetica by the base name, with
   bold/italic) as a non-embedded WinAnsi Type1, or, when the text needs more than WinAnsi (Greek, Cyrillic …), that
   face embedded as Type0/Identity-H through `pdf_add_font`. New font resources are named `IwEd<n>`.
5. **Size, colour, move, resize.** Size scales the text-space font size (and the line spacing with it); colour
   replaces the fill with `rg`; a move is a `cm` translation inside the block's `q … Q`, taken back through the CTM
   into the space the block is drawn in; a width change rewraps. Moving or resizing an edited block works the same way
   (the block is read back from the content like any other).
6. **What can't be edited** is still listed, with `editable: false`, `issue` and a sentence in `reason`:
   `type3` (glyphs are drawings), `noUnicode` (Identity-H and similar without ToUnicode: the text can't be read),
   `vertical`, `clip` (render modes 4-7), `invisible` (render mode 3, OCR layers). `areas` lists what has no block:
   `inForm` (text inside a form XObject: stamps, templates) and `outlines` (no text but ≥ 100 glyph-sized filled curves:
   text converted to paths).

## What works (tests: `cargo test --lib textedit -- --test-threads=1`)

- Listing blocks with font, size, colour, alignment and line boxes; headings and paragraphs separate.
- Replace → save → reopen: the extracted text equals the new text, and the old string is gone from the content bytes.
- Subset font: letters the subset has stay in it; a missing letter falls back to Helvetica (reported); Greek and
  Cyrillic fall back to an embedded face and survive save/reopen.
- Font without a space glyph: words placed apart, text extracts with spaces.
- Multi-line reflow keeps the block's left edge, width and line spacing; hard breaks with `\n`.
- Size, colour, move, resize and right/centre alignment; a centred block stays centred after a text change.
- Undo/redo of every change (each is one step); a stale `rev` is refused.
- Issues reported for Type 3, missing ToUnicode, invisible text and text converted to outlines.

## Limits

- One font per block: a block is one font, size and colour (mixed-style lines are separate blocks), and a rewrite sets
  the whole block in one style; per-character formatting, bold/italic toggles and font choice are not offered.
- Text inside form XObjects is reported, not edited (it may be shared by several pages).
- Fallback fonts are the standard 14 faces only (no system fonts, no CJK: CJK text that the original font lacks is
  refused with a message). The embedded face is the whole font, not a subset.
- Kerning inside the original runs is not kept for rewritten text (it uses the font's advances); `Tc`, `Tz`, `Ts`
  and render mode are kept.
- Line/paragraph grouping is heuristic (pdfcraft's rules): tables and tightly packed columns may group differently
  from Acrobat. Lines in one text object across columns can join if they share a baseline within 3 em.
- Glyph coverage of composite (Type0) subset fonts is judged from `/W` and ToUnicode, not the font program.
- The "outlines" check is a heuristic for pages with no text at all.
- Rewritten streams are Flate-compressed new objects; untouched streams and the rest of the page are byte-identical.
