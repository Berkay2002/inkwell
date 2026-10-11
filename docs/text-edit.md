# Edit existing text in place

Acrobat's "Edit text & images" for text (study: [study/02-edit.md](study/02-edit.md)). Engine and UI (see "The UI" below).
Code: `src-tauri/src/textedit/`, adapted from pdfcraft `crates/edit/src/text.rs`, `crates/content` and
`crates/fonts` (b8ddf71, MIT OR Apache-2.0; see `third_party/pdfcraft/`). MuPDF stays the only engine that owns the
document: pages, resources, fonts and streams are read and written as MuPDF objects, pdfcraft's own COS layer is not used.

## Commands

| Command | Arguments | Returns |
|---|---|---|
| `text_blocks` | `doc, page` | `{ rev, blocks: TextBlock[], areas: Area[] }` |
| `text_edit` | `doc, rev, page, index, change` | `{ state: EditState, fallback: Fallback \| null }` |
| `text_add` | `doc, rev, page, new` | `EditState` |

- Coordinates are page space as the viewer shows it (points, origin top left, `/Rotate` applied): the space of
  `page_text`. Offsets and widths in `change` are in that space too.
- `TextBlock = { index, text, rect, lines: rect[], font, size, bold, italic, color, align, lineSpacing, editable, issue, reason }`.
  `lineSpacing` is the distance between lines over the font size (null for one line).
  `font` is the `/BaseFont` (subset prefix kept), `size` in points, `align` one of `left|center|right|justify`.
- `change = { text?, size?, color?, align?, offset?: [dx, dy], width?, lineSpacing?, bold?, italic? }`; unset fields keep the block's own. `text`
  may contain `\n` for hard line breaks; everything else reflows to the block's width.
- `index` belongs to the `rev` of the list it came from; a `text_edit` against an older `rev` is `Stale` (list again).
- Each `text_edit` is one edit-core step (`Engine::apply_removing`): one undo step labelled "Edit text on p. N",
  "Format text …", "Move text …" or "Resize text box …", kept by Save, recorded in history. A document with text
  edits is saved as a full, garbage-collected rewrite (not an incremental update), so the old text is not left in the
  file's bytes.
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
   spreads the free width with `Tw` (single-byte fonts) or `TJ` gaps. Each removed show operator is replaced by a stand-in with
   the same moves and no painting: a numeric-only `TJ` advancing the text matrix by the removed text's width, after
   `T*` for a `'` (and `Tw Tc T*` for a `"`), so text after it in the same object stays put. When the object is split,
   it is reopened at the removed operator's line and position. Everything else in the stream is copied byte for
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
   `vertical`, `encoding` (a predefined CMap other than Identity, e.g. `90ms-RKSJ-H`: its code lengths and CIDs
   are not read), `clip` (render modes 4-7), `invisible` (render mode 3, OCR layers), `hidden` (inside `/OC … BDC` marked content whose optional content group
   is off in the default configuration, including membership dictionaries and their policy). `areas` lists what has no block:
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
  the whole block in one style; per-character formatting and font choice are not offered. Bold and italic set the
  whole block in the matching standard face (reported as a fallback), because an embedded subset has no bold.
- Text inside form XObjects is reported, not edited (it may be shared by several pages).
- Fallback fonts are the standard 14 faces only (no system fonts, no CJK: CJK text that the original font lacks is
  refused with a message). The embedded face is the whole font, not a subset.
- Kerning inside the original runs is not kept for rewritten text (it uses the font's advances); `Tc`, `Tz`, `Ts`
  and render mode are kept.
- Line/paragraph grouping is heuristic (pdfcraft's rules): tables and tightly packed columns may group differently
  from Acrobat. Lines in one text object across columns can join if they share a baseline within 3 em.
- Type0 fonts with predefined non-Identity CMaps are listed but not editable (embedded CMaps, including mixed
  one-/two-byte code spaces, are read and written with each code's own length).
- Glyph coverage of composite (Type0) subset fonts is judged from `/W` and ToUnicode, not the font program.
- The "outlines" check is a heuristic for pages with no text at all.
- Rewritten streams are Flate-compressed new objects; untouched streams and the rest of the page are byte-identical.

## Adding text

`text_add(doc, rev, page, { text, size, color, at: [x, y], width? })` puts a new text box with its top left at `at`
(page space; 12 pt black by default). It is set in Helvetica (a standard face; other scripts take the embedded
fallback face), wrapped to `width` when given, upright on rotated pages, in a new Flate stream appended to the
page's contents with its own `q ... Q` and a `/TouchUp_TextEdit` marked-content tag. One undo step, "Add text on
p. N"; unlike a rewrite it saves incrementally. The new box is listed as a block straight away and edited like any
other.

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
  replaced; the page's block list. Footer: "Changes apply when you click away. Undo with Ctrl+Z."
- **Cost**: one `text_blocks` call per page that is near the viewport, cached per page and document generation
  (`src/lib/textEdit.ts`); edits list the one page again.
- **Tests**: `src/lib/textEdit.test.ts` (helpers and store flows), `scripts/edit-shots.mjs` (runtime: edit a line,
  size, colour, add a text box, undo/redo, save, reopen, on a copy of `docs/study/samples/edit-source.pdf`; screens in
  `docs/screens/edit/`).
