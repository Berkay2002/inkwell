# Edit existing text in place

Acrobat's "Edit text & images" for text (study: [study/02-edit.md](study/02-edit.md)). Engine and UI (see "The UI" below).
Code: `src-tauri/src/textedit/`, adapted from pdfcraft `crates/edit/src/text.rs`, `crates/content` and
`crates/fonts` (b8ddf71, MIT OR Apache-2.0; see `third_party/pdfcraft/`). MuPDF stays the only engine that owns the
document: pages, resources, fonts and streams are read and written as MuPDF objects, pdfcraft's own COS layer is not used.

## Commands

| Command | Arguments | Returns |
|---|---|---|
| `text_blocks` | `doc, page` | `{ rev, blocks: TextBlock[], areas: Area[] }` |
| `text_edit` | `doc, rev, page, index, change` | `{ state: EditState, fallback: Fallback \| null, fallbacks: Fallback[] }` |
| `text_add` | `doc, rev, page, new` | `EditState` |
| `text_fonts` | `doc` | `FontInfo[]` |
| `text_replace` | `doc, rev, req` | `{ state, replaced, skipped, skippedCount, stopped, fallbacks, inserted }` |

- Coordinates are page space as the viewer shows it (points, origin top left, `/Rotate` applied): the space of
  `page_text`. Offsets and widths in `change` are in that space too.
- `TextBlock = { index, text, rect, lines: rect[], font, size, bold, italic, color, align, lineSpacing, runs, editable, issue, reason }`.
  `runs: { index, text, font, fontId, size, color, bold, italic, underline, strike, rise }[]` is the text in runs of one
  font, size, colour and decoration (line breaks read as spaces); the block-level font, size, colour, bold and italic
  are those of the first run that is not raised (a block that starts with a superscript is not sized by it).
  `rise` is `"super" | "sub" | null`, read from the sign of `Ts` in any file; `underline` and `strike` are true only
  where one of Inkwell's own tagged rectangles lies under the run (see Decorations).
- `text_blocks` also returns `locked` (the reason string when the document is certified, null otherwise) and `signed`
  (true when it carries signatures: edits stay incremental and the UI warns). A certified document refuses
  `text_edit` / `add_text` with "This document is certified...".
  `lineSpacing` is the distance between lines over the font size (null for one line).
  `font` is the `/BaseFont` (subset prefix kept), `size` in points, `align` one of `left|center|right|justify`.
- `change = { text?, runs?, size?, color?, align?, offset?: [dx, dy], width?, lineSpacing?, bold?, italic? }`; unset fields keep
  the block's own. `text` (one run, in the first run's style) may contain `\n` for hard line breaks. A block of
  two or more lines keeps its lines (see "Lines are kept" below); a block of one line, and any change with a `width`,
  `size`, `lineSpacing` or `align`, reflows everything to the block's width. `underline?`, `strike?` and `rise?: "super" | "sub" | null` apply to every run (`null`
  clears; a missing field keeps). `runs: { text?, source?, font?, size?, color?, bold?, italic?, underline?, strike?, rise? }[]`
  is the new content run by run: unset fields come from run `source` (or the first run); `font` is a `text_fonts` id. A run with a `source`,
  no new text and no font or style change keeps its glyphs exactly, kerning included. Block-level `size` scales every
  run (the first run gets that size); `color`, `bold` and `italic` apply to every run without its own.
- `FontInfo = { id, name, family, bold, italic, kind: document|standard|bundled, embedded, subset, usable, pages }`.
  Ids: `doc:<object>` (a document font, one entry for all its pages, named on the page when first used there),
  `res:<name>` (a font written into a page's resources directly: one entry per page, valid on that page only),
  `std:<name>` (standard 14), `bundled:<id>` (Noto faces). `usable` is false for Type 3, predefined-CMap, vertical and
  unreadable fonts; `subset` warns that typed text may fall back. The list reads only the font dicts (`/BaseFont`,
  `/Subtype`, `/Encoding`, the descriptor's flags and font file keys), never a font program or CMap.
- `index` belongs to the `rev` of the list it came from; a `text_edit` against an older `rev` is `Stale` (list again).
- Each `text_edit` is one edit-core step (`Engine::apply_removing`): one undo step labelled "Edit text on p. N",
  "Format text …", "Move text …" or "Resize text box …", kept by Save, recorded in history. A document with text
  edits is saved as a full, garbage-collected rewrite (not an incremental update), so the old text is not left in the
  file's bytes. A signed document is the exception:
  its save stays incremental (the signed bytes are not rewritten, the old text stays in the file) and the UI says so.
- `fallbacks: { font, embedded, reason }[]` when a run's font could not show its text (the UI's yellow `!`), one per
  font used instead; `reason` names the original font and the first character it lacks. `fallback` is the first.

## Design

1. **Reading** (`text.rs: read_page`). The page's content streams are joined (a page's streams are one stream split
   between any two tokens) and parsed into operators that keep their byte spans (`content.rs`). An interpreter tracks
   the graphics and text state (`q/Q`, `cm`, `BT`, `Tf`, `Tc/Tw/Tz/TL/Ts/Tr`, `Td/TD/Tm/T*`, fill colour) and turns
   every `Tj/TJ/'/"` into a run: text (ToUnicode, else the encoding with `/Differences`), box (font ascent/descent),
   baseline start/end, user-space size. Widths come from `/Widths`, `/W`/`/DW`, or for standard 14 fonts without
   widths from MuPDF's built-in faces (`font.rs`).
2. **Lines, runs and blocks.** Show operators in one text object, same render mode and on one baseline (measured in
   the run's own text direction, so rotated text works) form a line; a line holds runs of one font, size and colour,
   each with its glyphs (code, meaning, advance and the TJ adjustment before it). Consecutive lines of similar size
   (largest run within 1.3×), a shared left edge, centre or right edge and a regular gap (0.8-2.5 × size, ±20 %) form
   a block, whatever their fonts and colours (as in Acrobat). Alignment is detected from the lines' edges (`justify`
   when every line but the last fills the width).
3. **Rewriting** (`rewrite_block`). The block's show operators are removed, and so is any other text drawn over the
   same area (fake bold, a duplicate copy), except invisible text. Where the block started, one new text object is
   inserted in its own graphics state: `q [cm] /TouchUp_TextEdit MP BT <state> Tm … ET Q` (Acrobat's marker). If the
   same `BT` showed text before the block, the object is split there (`ET … BT Tm`) to keep reading order. Lines are
   positioned with `Td` from the block's left edge in its text space; `center/right` offset each line; `justify`
   spreads the free width over the line's spaces with `TJ` adjustments (any font, one- or two-byte). The content is
   laid out as glyph items (`layout.rs`): an unchanged run's own codes and kerning, new text encoded per character.
   The new object sets `Tw 0`: a kept space has its glyph's own width (the source's word spacing is not added to it),
   and a `TJ` gap that read as a space stays a gap of the same total width (¼ em plus the rest as kerning).
   Words wrap at spaces, CJK text (Han, kana, Hangul) between any two characters, `\n` breaks a paragraph; a line
   with a larger run sits lower in proportion, and a line under one with a larger run is set lower by half the
   difference. Each style change writes `Tf` and the fill colour; each stretch of one style is one `TJ`. Each removed show operator is replaced by a stand-in with
   the same moves and no painting: a numeric-only `TJ` advancing the text matrix by the removed text's width, after
   `T*` for a `'` (and `Tw Tc T*` for a `"`), so text after it in the same object stays put. When the object is split,
   it is reopened at the removed operator's line and position. Everything else in the stream is copied byte for
   byte. An earlier edit's own object (`q [cm] /TouchUp_TextEdit MP BT … ET Q`) whose text all goes is removed,
   its `Tf` and colour included; a wrapper with a `cm` (a move, an added box's placement) stays and the new block is
   set inside it, so the block keeps its place and `q` nesting does not grow with each edit. Moving such a block
   again changes that wrapper's `cm` (the move folded into it) instead of adding another `q cm`. Changed streams become new Flate streams (a content stream may be shared with another page); the page's
   `/Contents` points at them.
4. **Fonts.** A run's font is kept for every character it has a code (reverse ToUnicode/encoding) and a glyph for:
   a code with a zero width is missing, and for a subset simple font the embedded program (loaded by MuPDF) must have
   the glyph. A font without a space code (TeX) places words apart with `TJ` gaps of ¼ em. The characters a run's font
   lacks go to one fallback for the run: the matching standard 14 font (Courier/Times/Helvetica by the base name, with
   bold/italic) as a non-embedded WinAnsi Type1 when WinAnsi can show them all, else the bundled Noto Sans (or Noto
   Sans Mono for monospaced runs, in the run's style) if it has any of them, and, for what that lacks, Noto Sans CJK
   (CJK-only text goes straight to the CJK face, and only the faces something was set in are embedded and reported).
   Two characters drawn with one bundled glyph (U+301C and U+FF5E, Ω and the ohm sign) go to two subsets of the
   face, since a code has one ToUnicode text. Bold or italic without a
   font asks first for a font of the same family and style already on the page, then the standard face. Bundled faces
   are embedded per rewrite with only the glyphs used (`bundled.rs`: a runtime subset with the `subsetter` crate,
   Type0/Identity-H, CIDFontType2 + `/FontFile2` for TrueType, CIDFontType0 + `/FontFile3 /CIDFontType0C` for the
   CFF CJK face, `/W` widths and a ToUnicode map); bold CJK text is drawn by also stroking the outline (`2 Tr`), since
   the CJK face has one weight. New font resources are named `IwEd<n>` (standard), `IwNoto<n>` (bundled), `IwDoc<n>`
   (a document font picked for a page that did not name it). A standard face already added to the page (`IwEd<n>`
   with the same `/BaseFont`) is reused. After each rewrite, `Iw*` names that no `Tf` in the page's content uses any
   more are removed from the page's `/Font` dict (resources and font dict copied onto the page first, shallowly, so
   pages sharing them keep theirs); the next full save drops the replaced subsets.
   The bundled faces (`src-tauri/fonts`, 3.5 MB in the executable) are subset at build time by a developer
   (`scripts/fonts/subset_fonts.py`): Latin, Greek and Cyrillic for Noto Sans/Mono, GB 2312 + JIS X 0208 + KS X 1001
   for the CJK face, keeping the fonts' `name` tables (copyright and licence records); see
   `third_party/fonts/README.md` (OFL 1.1). The OFL texts ship with the app as bundle resources under `licenses/`
   (`tauri.conf.json`), with `third_party/THIRD-PARTY-NOTICES.txt`.
5. **Size, colour, move, resize.** Size scales the text-space font size (and the line spacing with it); colour
   replaces the fill with `rg`; a move is a `cm` translation inside the block's `q … Q`, taken back through the CTM
   into the space the block is drawn in; a width change rewraps. Moving or resizing an edited block works the same way
   (the block is read back from the content like any other).
6. **What can't be edited** is still listed, with `editable: false`, `issue` and a sentence in `reason`:
   `type3` (glyphs are drawings), `noUnicode` (Identity-H and similar without ToUnicode: the text can't be read),
   `vertical`, `encoding` (a predefined CMap other than Identity, e.g. `90ms-RKSJ-H`: its code lengths and CIDs
   are not read), `clip` (render modes 4-7), `invisible` (render mode 3, OCR layers), `hidden` (inside `/OC … BDC` marked content whose optional content group
   is off in the default configuration, including membership dictionaries and their policy). `areas` lists what has no block:
   `inForm` (text inside a form XObject: stamps, templates) and `outlines` (no text but ≥ 100 glyph-sized filled curves:
   text converted to paths).

## Decorations: underline, strikeout, super/subscript

- **Super/subscript** is text rise: `Ts` in the run's state, with the size scaled to 0.65 of the base size and the rise
  at +0.33 (super) or -0.15 (sub) of the base size. Read back from any file by the sign of `Ts` (positive super,
  negative sub); line grouping and block measurement ignore the rise, so a raised run stays in its line. Toggling off
  restores the size by 1/0.65 only when `Ts`/size looks like ours (within 0.05); a foreign raised run keeps its size.
  Each run writes its own `Ts` (and `Ts 0` in the header), so unchanged runs keep what they had.
- **Underline and strikeout** are filled rectangles in the run's colour (thickness 0.05 of the size; underline centre
  0.12 below the baseline, strike 0.28 above), one per line and stretch (justified and kerned lines are measured the
  same way), written after the text object as `/Span <</IWDeco /U>> BDC q ... re f Q EMC` (`/S` for strike).
- **Read back** only for our own groups: a `BDC` with `IWDeco` followed by exactly what `deco_ops` writes, in this
  order: `q`, `cm`, the fill (`g`, `rg`, `k`, or `cs` with `sc`/`scn`), `re`, `f`, `Q`, `EMC` (checked by `deco_group`;
  a lone `Q`, a bare `cm` or any other shape is not ours and is never read, skipped or removed). The rectangle is
  matched by geometry onto the shown text above it. Other producers' lines are not guessed at (Acrobat does). Every
  rewrite removes the block's groups and draws them again, so a moved or reflowed block keeps exactly one per line; a
  stray tagged group with no text under it is left alone. A page is read for at most 10,000 of them.
- **A group that lies on text of more than one block** (two blocks drawn over each other, a fake-bold copy) goes when
  any of them is edited, and is drawn again only for the block that was edited: the other block loses that line.
- **A rise past 0.6 of the size** is a position, not a script (some producers move text with `Ts`): it stays in the
  run's start and box, reads as no rise, and is written back as it was unless the rise is cleared. A `Ts` that is not
  a finite number up to 100,000 reads as no rise.
- The block's undo label is "Format text" when only styles changed.

## What works (tests: `cargo test --lib textedit -- --test-threads=1`)

- Listing blocks with font, size, colour, alignment and line boxes; headings and paragraphs separate.
- Replace → save → reopen: the extracted text equals the new text, and the old string is gone from the content bytes.
- Subset font: letters the subset has stay in it; a missing letter falls back to Helvetica (reported); Greek and
  Cyrillic fall back to a Noto Sans subset and survive save/reopen.
- Runs: a block of three runs (regular, bold, regular) keeps its runs; changing one run keeps the others' glyphs and
  their TJ kerning (checked in the saved bytes after reopening). Runs with their own size and colour, a document font
  picked by id, a bundled mono face and a restyled italic run read back with those fonts after save/reopen.
- CJK (Chinese, Korean, Japanese) and Greek text in a block or a new text box: set in embedded subsets that stay small
  (Noto Sans subset < 12 KB, CJK subset < 24 KB for a handful of characters), extracted unchanged after reopening,
  editable again; bold CJK uses synthetic bold.
- The font list offers the document's fonts (with their pages), the 12 standard faces and the 7 bundled faces; a
  picked face is accepted by a rewrite, an unknown id is refused.
- A justified paragraph re-wraps into its width with both edges aligned (all lines but the last) and stays justified
  after another text change and after save/reopen.
- Font without a space glyph: words placed apart, text extracts with spaces.
- A `Tw`-justified source re-set without `Tw` spreads its lines to the full width again, and right-aligned lines end
  at the box's edge; a `TJ` gap read as a space keeps its exact width (the saved `TJ` numbers add up to the source's).
- CJK-only text is reported and embedded as the CJK face alone (no empty Noto Sans subset). Characters sharing a
  glyph (〜 and ～, Ω and Ω) read back as typed after save/reopen. Korean without spaces wraps into the width.
- A block moved 40 times and an added box moved 30 times end where the moves add up to, with `q` nesting of at
  most 3 in the saved content.
- A moved block and an added text box keep their position when edited again (also after save/reopen); 12 pt text
  under a 48 pt run clears it.
- Editing a block again and again leaves only the fonts its content uses: one `IwEd`, one `IwNoto`, one CJK subset in
  the saved file after five edits.
- Multi-line reflow (a width change, or a paragraph typed over) keeps the block's left edge, width and line spacing; hard breaks with `\n`. Otherwise the lines are kept: an edit in line 2 of three moves no other line.
- Size, colour, move, resize and right/centre alignment; a centred block stays centred after a text change.
- Undo/redo of every change (each is one step); a stale `rev` is refused.
- Issues reported for Type 3, missing ToUnicode, invisible text and text converted to outlines.
- Decorations: super/subscript round trip (`Ts` and size written, read back, off again restores the size); `Ts` read
  from a foreign fixture; underline and strikeout drawn and read back; a moved or reflowed decorated block keeps one
  rectangle per line; line grouping unchanged by rise; a tagged group that is not ours is left alone; certified refused.

## Limits

- Styles are per run: a change of style inside a word makes two runs. Character spacing and horizontal scale per run
  are not offered (the block's `Tc` and `Tz` are kept for all runs; `Ts` is per run, see Decorations).
- Formatting part of a run (a selection inside it) re-encodes that piece in the run's font: in a subset font a
  letter the subset lacks falls back to another font for that piece (reported as for typed text).
- Underline and strikeout read back only as Inkwell's own tagged rectangles: a line drawn by another producer shows
  as no decoration, stays in the file and is not removed or redrawn with the text.
- Bold or italic in a font the document lacks uses a standard face (or Noto Sans for other scripts); there is no
  synthetic italic, and synthetic bold is used only for CJK.
- A run whose text changed loses the original kerning (it uses the font's advances); unchanged runs keep theirs. Word
  spacing (`Tw`) is not kept: a left-aligned line that used it to space words comes out narrower (justified lines
  are spread again).
- Text inside form XObjects is reported, not edited (it may be shared by several pages); Find & Replace skips it.
- Find & Replace changes page text only; a run a match is in loses its original kerning, as in any text edit.
- Fallback fonts are the standard 14 and the bundled faces (no system fonts). Characters outside the bundled sets
  (rare Han ideographs, other scripts such as Arabic, Devanagari or Thai) are refused with a message naming the first
  one. CJK text is set in its Simplified Chinese glyph forms; there is no complex shaping (no ligatures, no
  right-to-left or contextual forms).
- Line breaking is greedy, at spaces and between CJK characters; no hyphenation, no kinsoku rules.
- Line/paragraph grouping is heuristic (pdfcraft's rules): tables and tightly packed columns may group differently
  from Acrobat. Lines in one text object across columns can join if they share a baseline within 3 em.
- Type0 fonts with predefined non-Identity CMaps are listed but not editable (embedded CMaps, including mixed
  one-/two-byte code spaces, are read and written with each code's own length).
- Glyph coverage of composite (Type0) subset fonts is judged from `/W` and ToUnicode, not the font program.
- The "outlines" check is a heuristic for pages with no text at all.
- Rewritten streams are Flate-compressed new objects; untouched streams and the rest of the page are byte-identical.

## Adding text

`text_add(doc, rev, page, { text, size, color, at: [x, y], width? })` puts a new text box with its top left at `at`
(page space; 12 pt black by default). It is set in Helvetica (a standard face; other scripts take the bundled
Noto subsets, CJK included), wrapped to `width` when given, upright on rotated pages, in a new Flate stream appended to the
page's contents with its own `q ... Q` and a `/TouchUp_TextEdit` marked-content tag. One undo step, "Add text on
p. N"; unlike a rewrite it saves incrementally. The new box is listed as a block straight away and edited like any
other.

## Lines are kept (Edit text, move, style)

A block of two or more lines is not re-wrapped when it is committed. The UI's draft is one paragraph (the lines
run together with a space between them); `rewrite_block` compares it with the block's words (`replace::keep_edit`,
a word diff) and sets only the stretches of words that changed, through the same builder as Replace
(`replace::keep_change`, `KeepLines { ends, changed, wrap }`, `layout::keep_lines`):

- A line the edit does not touch keeps its origin, baseline and width, so a ragged-right block stays ragged, a
  first-line indent and uneven leading stay, and justified lines stay stretched.
- A changed line is set at the start (middle, end) of the line it was. An edit across a line end (the space between
  two lines deleted or typed over) joins those two lines into one, nothing else. A line emptied leaves a gap.
- **A line that grows past the block's width** (its widest line) is wrapped at that width: the overflow goes on new
  lines below it, at the block's left edge and the block's line distance. The lines under it move down by the
  lines added (its own lines minus the source lines it covers; a line that got shorter or was emptied leaves a gap,
  nothing is pulled up), each keeping its x and its place relative to the others. Lines above it do not move. The
  block can end up taller than its box, and over the text below it: undo is the way back, as for any Edit text.
  Acrobat does the closest thing (it reflows the paragraph and the block grows downwards); we don't pull the
  overflow into the next line because that would join lines the user didn't touch.
- A typed line break starts a new line below the line it was typed in, the lines under it move down by it. A line
  break typed where two words were is a change even though no word is: what stands between words is compared too.
- An edit that is only a style (underline, colour, bold, italic, rise, font) or a move keeps the lines the same
  way: the lines without a restyled run are exactly as they were. A bolder line that no longer fits wraps as above.
- **Whole-block reflow stays** where it is asked for or where there are no lines to keep: a change with a `width`
  (dragging the box's edge), `size`, `lineSpacing` or `align`; a block of one line (it grows to the margin); most of
  the words typed over (a paragraph written anew); a block with a line-end hyphen the edit doesn't cover (the rewrite
  drops it, so the word is joined and the block reflows); a block whose lines have their own character spacing or
  scale, whose runs text differs from the text the user was shown, that holds a private-use mark character or is over
  20,000 characters; and a change of size, which scales the leading too.
- Styles: typed text takes the style of the run the changed word starts in, as a Replace does; a word that spans two
  runs of different styles takes the first's.

## Find & Replace

Acrobat's Edit > Find & Replace, on top of the block edit (`textedit/replace.rs`, command `text_replace`).
`req = { find, with, opts: { caseSensitive, wholeWords, ignoreAccents }, at?: { page, rect }, from?: { page, upto } }`. There is no new
content-stream writer: every changed block goes through `rewrite_block`, so fonts, fallback and the
`fallbacks` report are those of Edit text.

- **Matching** is Find's own (`search::Query`, so case, whole words and accents mean the same), run over the runs of
  each text block as Edit text reads them (`search::find_spans`). Comments, form-field values and bookmarks are never
  replaced, whatever Find's options say. An empty `find` and a replacement with a line break are refused.
- **Replace all** (`at` unset) plans each page from one reading of it and applies it from the last block to the first;
  one `rewrite_block` per block, however many matches it holds. Runs a match does not touch keep their glyphs and
  kerning; a match across runs puts the replacement in the style of the run it starts in. The block's lines stay
  where they were. A line that holds no replacement is left exactly where and as wide as it was (it starts where it
  did and its spaces take up its width, which is what its word spacing did); a line that holds one is set at
  the start (or middle, or end, by the block's alignment) of the line it was, with that line's own baseline, so
  nothing moves with a first-line indent or uneven leading; in a justified block it is stretched to the end that
  line had, unless it is the last (a line's own leading and trailing spaces don't count in its width). The text
  of a run with a match is kept as it is (double and no-break spaces); only a space the replacement would double
  at its edge goes. A line, a single-line block (heading, table cell) included, never grows past the width its
  block had: nothing tells what is beside it, so a replacement that does not fit is not wrapped or pushed down
  but skipped with "the replacement does not fit on its line". The same goes for a block whose line ends can't
  be placed in its new text, for a block whose lines have their own character spacing or scale, and for a match
  that stops inside a word a dropped hyphen split. A match across a line end
  drops
  that one break. Edit text keeps the lines the same way (next section). Nothing is searched again
  after a replacement, so a replacement that contains the find text is set once and never loops. The whole run is
  one undo step ("Replace "cat" with "dog""), saved like any text edit (a full rewrite; incremental when signed).
- **Replace one** (`at` = the first rectangle of the current Find hit): the match is the nth hit of its block in
  Find's list for the page, taken as the nth match in the block's text and checked against the words around it (a
  shortcut: if it can't be placed the match is skipped with "use Replace all"). One undo step, "Replace text on p. N".
  `inserted` is how many times the find text occurs in the replacement: the UI lands that many hits further on.
- **Skipped, never forced**: a match Edit text refuses (Type 3, no ToUnicode, invisible text, vertical, encoding,
  clipping text), a match across two blocks, text in a form XObject or converted to outlines, a block the replacement
  would leave empty, and a block whose rewrite is refused (its text or a character no font can show) are left as they
  are and listed as `{ page, text, before, after, reason: "skipped: …" }` (`before` and `after` are a few words
  around the match, so rows for the same word can be told apart); a block is rewritten whole or not at all. Two more
  reasons are about the block text, which drops a hyphen at the end of a line ("well-" "known" reads "wellknown"):
  a block holding one is left alone unless a match covers it (a rewrite would lose the hyphen), and a block whose
  matches differ in number from Find's hits in it (a match across the hyphen, a whole word cut by the block's edge)
  is left alone because the matches can't be placed. The first 200
  are listed, `skippedCount` counts all. Text on a hidden layer is not offered by Find and is not counted.
- **Locked**: a certified document is refused ("This document is certified…", nothing recorded); a signed one is saved
  incrementally as Edit text is. A stale `rev` is `Stale`.
- **Bounds**: at most 2,000 replacements and 20 seconds per run; past either it stops between two blocks (the time
  is only checked once something was replaced, so every run gets somewhere), keeps what it did (still one step) and
  says so in `stopped`. Nothing is written when nothing was replaced. `resume` is where it stopped (`{ page, upto }`:
  that page, and on it the blocks before `upto`, as blocks are replaced from the last to the first); a Replace all
  sent with that as `from` goes on there, so pages already done are not replaced again (a replacement that holds the
  find text would grow: "cat" to "cats" to "catss"). The Find bar does this by itself when Replace all is pressed
  again for the same text and options on the document the run left. A block the cap would cut is left whole for
  the next run (with `resume` at it); only a single block with more matches than the cap is part replaced, and
  the rest of it is listed as skipped.
- **UI**: while Edit text is on, the Find bar has a second row: "Replace with", Replace, Replace all and the result
  ("5 replaced, 2 skipped", which opens the list of what was skipped, with its page and reason). Replace goes on to
  the next match. The controls say why they are off (certified document, no match, a comment or form hit selected);
  the certified reason is also shown as text under the row. With the signature banner showing, the Find bar and its
  rows sit below it (`barTops` in `src/lib/replaceLogic.ts`).
  Typed text in a block is sent first (`src/lib/replace.ts`).
- **Tests**: `textedit::tests::replace_*` and its neighbours (replace all across pages as one undo, case and whole
  word, a replacement holding the find text, replace one, runs, skips with reasons, certified and signed, a rotated
  page, save and reopen), `search::tests::spans_*`, `src/lib/replace.test.ts`.

## The UI

Entry: the **Edit** segment of the `Read | Edit | Organize` pill (also Ctrl+E, "Edit text & images" in the palette,
and "Add text box"). Edit is a mode of the viewer, not a tab: Read and Organize stay in the pill. The rail is the
dock the other tools use; opening it closes Comments, Redact, History, a Tools sheet and a docked assistant, and
opening any of those closes it (`src/lib/commentRail.ts`).

- **Pages**: `TextEditLayer` (per page, inside `PageView`) draws dotted outlines for blocks. Click a line to edit it in
  place; the overlay editor is a textarea in the block's font family, size, colour, alignment and line height.
  Locked blocks (Type 3, no ToUnicode, vertical, clipped, text in forms, outlines) are dashed grey; clicking one says
  why. Hidden-layer text is not offered at all.
- **Commit**: typed text is one `text_edit` when the block is left (click away, Esc, Ctrl+Enter, another block, Save
  or leaving Edit), so one undo step per block session. A format control sends the typed text together with its change.
  While nothing new was typed, Ctrl+Z / Ctrl+Y in the editor are the document's undo and redo; with typed text they
  are the field's own.
- **Typed text is never dropped silently**: closing a tab or the window, switching tabs, opening Organize and
  leaving Edit send it first (`setLeaveGuard` in `edit.ts`), so the document is dirty and the usual Save prompt
  follows. A change the engine refuses (a character no font can show) keeps its draft and block with a notice; leaving
  the block again drops it, and closing asks first. A refused new box stays the same way.
- **Selection follows its block** across undo, redo and restored versions by position, or is dropped with its draft.
- **Move and rewrap**: the grip above the frame moves the block, the side handles rewrap it to a new width.
- **Rail**: Add content (Text tool; Image is a dimmed "soon" slot for the image engine in `cloud-image-edit`);
  Format text: a one-line summary of what is selected, the font shown honestly (read-only) with B / I (standard face),
  size stepper, colour (presets and the system picker), alignment, line spacing; a quiet notice when a font was
  replaced; an **Effects** row with U, S, x² and x₂ toggles (pressed state from the selection's runs, else the whole
  block; super and sub replace one another; on a certified document every Format control is disabled with the reason, and the store refuses a change without asking the engine; a hint under Format says
  when the file is signed and the edit is saved incrementally); the page's block list. Ctrl/Cmd+U toggles underline.
  With text selected in the editor the toggle applies to that selection (runs cut at its ends, one undo step), else to
  the whole block; typed text is sent first, and a selection made in text the engine then tidied is ignored. The
  editor previews underline and strikeout for the whole block only. Footer: "Changes apply when you click away. Undo with Ctrl+Z."
- **Cost**: one `text_blocks` call per page that is near the viewport, cached per page and document generation
  (`src/lib/textEdit.ts`); edits list the one page again.
- **Tests**: `src/lib/textEdit.test.ts` (helpers and store flows), `scripts/edit-shots.mjs` (runtime: edit a line,
  size, colour, add a text box, undo/redo, save, reopen, on a copy of `docs/study/samples/edit-source.pdf`; screens in
  `docs/screens/edit/`).
