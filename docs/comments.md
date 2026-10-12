# Comments and annotations

Engine: `src-tauri/src/annot/` (every action is one undo step, kept by Save, recorded in history). UI: Comments rail
(Ctrl+Alt+M), tool strip over the page, selection frame with handles and a property popover.

Keys while Comments is open: V select, H highlight, U underline, S strikethrough, N note, T text box, P pen,
R rectangle, O ellipse, L line, A arrow, M stamp, Delete removes the selection, Esc returns to select.

Replies are hidden `/Text` annotations with `/IRT`; resolved state is a `/StateModel /Review` reply (latest wins).

## Big and shared annotation lists

The list (`annot_list`, `{ items, incomplete }`) reads each page's `/Annots` from the page dictionary and loads only pages
that bring annotations no earlier page had, so pages that share one array cost nothing extra. An object goes to the first
page that lists it, or to the page its `/P` names when that page lists it too. Known limit: a dictionary two pages list
through different arrays, with no `/P` naming the later one, shows on the first page only; the later page still draws it
but it cannot be selected there. Past 50 000 loaded annotations (links and popups not counted) or 2 000 000 repeated array entries
(an object an earlier array listed, or any inline entry; the first sight of an indirect object is free; a page object listed twice in `/Kids` is walked once) the remaining pages are left out and `incomplete` is true; the panel says so.

## Custom stamps

Engine: `src-tauri/src/annot/stamps.rs` (commands in `annot/app.rs`: `stamp_list`, `stamp_create`, `stamp_update`,
`stamp_delete`, `stamp_thumb`, `stamp_place`, `stamp_gate`). UI: the stamp picker in the tool strip (Standard, Dynamic,
then one group per category) and the "Custom stamps" sheet (button beside the picker, or the command palette).

- **Library**: `<data dir>/stamps/` (`INKWELL_DATA_DIR` moves it, like the recent list), not inside any document. It holds
  `stamps.json` (id, name, category, pixel size), one `<id>.png` and one `<id>.t.png` thumbnail per stamp. Ids are 16 random
  lowercase hex characters and are checked before they reach a path. Writes are temp file plus rename under one lock.
- **Sources**: PNG, JPEG, TIFF, BMP, GIF, or one page of a PDF. PNG and JPEG go through the `image` crate (a JPEG's EXIF
  orientation is applied); the rest through MuPDF. A PDF page is drawn at 300 dpi with an alpha channel, so what the page does not
  paint is transparent, then cropped to the painted area. Shortcut: a page with its own white background keeps it.
  An encrypted PDF is refused. The stored PNG is always our own re-encoding of the pixels: nothing of the picked file
  (metadata, extra chunks) is kept, and the picked file is never changed.
- **Limits**: 200 stamps, 20 MB per source file, 50 megapixels per source, longest side shrunk to 4096 px, name 128
  characters (longer is refused, not cut), category 64, default category "Custom". A damaged `stamps.json` is moved
  aside to `stamps.json.bad` and the library starts empty with a warning; unusable entries are dropped one by one. An
  index that cannot be opened or read (permissions, a disk fault) shows as empty with a warning but stops every write,
  so it is never stored over. Only a regular file is read as a source. Drawing a PDF page stops after 20 seconds.
- **Placing**: the click centres a stamp 150 pt wide with the picture's proportions, moved inside the page. The
  appearance is one Form XObject whose only content is the picture (with its alpha as a soft mask), so any PDF
  reader draws it, and MuPDF treats it as a standard stamp: move and resize redraw it in the new `/Rect`. Resize keeps
  the proportions (`fit_aspect`, mirrored in `src/lib/stampsLogic.ts` for the frame while dragging); the corner or edge
  that did not move stays put. One placement is one undo step, kept by Save. On a page with `/Rotate` the picture's
  pixels are stored turned back by the rotation (MuPDF rebuilds the appearance with an identity matrix on every move,
  so a `/Matrix` would not last), and resize fits the proportions as the page shows them.
- **XFDF and FDF**: a placed stamp is exported with its name and its appearance (the picture), so imported into another
  document it shows the same picture (`picture` is true there, so resize keeps the proportions). Into the same document
  it updates the stamp in place. See "Data files" below for the limits.
- **Certified and signed**: placing uses `outline::refuse_unless_annotating_allowed` inside the edit, so a document
  certified with P1 or P2 refuses it with no history step and the picker is off with the reason; P3 allows it. A
  signed document takes the stamp as an incremental update and the picker says so. The library sheet never touches the
  open document, so it works with no document and on a certified one.
- **Not an assistant tool**: the `ai/` module does not reference `annot::stamps`; the library is the user's.
- Tests: `annot::stamps::tests` (Rust, with pixel and saved-bytes checks), the "place a custom stamp" row in
  `engine::edit::tests_certified`, `src/lib/stamps.test.ts`.
## Data files: XFDF and FDF

Comments panel menu (the three dots) and the command palette: **Export comments…** and **Import comments…**. Code:
`src-tauri/src/annot/xfdf.rs` (comments) and `src-tauri/src/exchange.rs` (caps, XML reader, FDF reader shared with form
data). The format of an export follows the file name: `.fdf` is FDF, anything else XFDF 3.0 (ISO 19444-1).

- **Exported:** text notes, highlight, underline, squiggly and strikeout (with quad points), ink, square, circle, line,
  polygon, polyline, free text, stamps, carets (`/RD` as `fringe`, `/Sy` as `symbol`) and file attachments, with name
  (`/NM`), page, rect, colour, opacity, border width, author, subject, dates, contents, rich text (`/RC`, as
  `<contents-richtext>`), review state and replies (`inreplyto`). A comment with no `/NM` is written under a name
  made for the export (`inkwell-<page>-<object>`, nothing is added to the document); importing it back adds it again
  instead of updating it, because that name is not unique across files. Comments the tools add from now on carry a
  real `/NM`. Sound and redaction marks are left out and counted.
  So are measurements (a line, polyline or polygon with `/Measure` data, see [measure.md](measure.md)) and their replies:
  these files have no place for the scale, and a plain line would be a measurement that lost its meaning. The export
  summary counts them.
  Page numbers and points are the file's (PDF user space, y up, 0-based page).
- **Appearance (`/AP`)**: written for picture stamps only (the other types are drawn from their data, which any reader
  can do). XFDF `<appearance>` holds the base64 of an XML tree that spells the PDF objects: `<DICT KEY="AP">` with
  `<STREAM KEY="N">`, whose entries are `INT`, `FIXED`, `NAME`, `BOOL`, `ARRAY`, `DICT` and `STREAM` elements (`KEY` is
  the entry, `VAL` a scalar) and whose bytes are a `<DATA MODE ENCODING LENGTH>` element (`HEX` or `ASCII`). **These
  element and attribute names are not confirmed against the standard or an Acrobat file**: the text of ISO 19444-1
  was not available (only its table of contents), and the names come from Apache PDFBox's reader for the appearance
  Acrobat writes. `STRING` elements are skipped and `MODE` is written but not needed to read. Files Inkwell wrote before
  (base64 of a small FDF file with one annotation) are still read. An appearance is never copied as it is: the XML (or
  FDF) is read through a whitelist into plain data (`annot/xfdf/ap.rs`): one form, at most 4 levels of forms, 64
  objects, simple Type 1 and TrueType fonts (their embedded font file included), graphics states with opacity and
  blend mode, images up to 16 megapixels (one Flate or DCT filter, as stored; ICC profiles replaced by their device
  space). Actions, `/AA`, `/OC`, metadata, patterns, shadings, inline images, soft masks of a graphics state, Type 3
  and composite fonts are not carried; an appearance that needs one of them is refused as a whole. Every name its
  content uses must resolve in the form's own `/Resources`.
  Streams (appearances, font programs, rich text and attached files) are decoded by `annot/xfdf/stream.rs`, one filter
  stage at a time: Flate, ASCII hex and ASCII 85, as `[ASCII]? [Flate]{0,2}` (an ASCII filter only first, never after a
  decompressor, and at most 3 filters). Every stage's output is held to the cap (4 MB per appearance or file) and
  charged to a per-file allowance of 256 MB of decoding, so a bomb stops at the cap and a stream shared by thousands of
  comments is decoded once and then refused from what was learned. DCT, JPX, JBIG2, CCITT, LZW, run length, `/Crypt`
  and a Flate predictor are not decoded outside of an image (an LZW attachment is left out as "not understood").
  There is also 64 MB for what appearances hold in memory (a font many comments share is a copy each); past either, the
  rest get Inkwell's own drawing and are counted.
- **Attached files**: a FileAttachment comment carries its file in XFDF `<data mode="raw" encoding="hex">` (a
  `filtered` mode is Flate; ascii and base64 are read too) or FDF `/FS` with `/EF`. At most 4 MB each and 10 MB for all
  attachments and appearances of one file; a larger one is left out and counted (a comment whose file cannot be written is
  left out of an export with its replies; one that cannot be read is skipped on import). A file that is a link to
  another file is never followed or opened, and a name is display text only (no folder, no characters a file system
  refuses). Name, size, modification date, description and MIME type are kept. The document's own embedded files
  (`/Names /EmbeddedFiles`) are not touched: the file lives in the comment. An import never replaces the file of a
  comment that is already in the document (matched by name): the other fields update, and the report counts "its own
  file is kept". A file stored with an image codec (JPEG, JPEG 2000, JBIG2, CCITT) is refused, since decoding it would
  give pixels, not the file.
- **Rich text**: read with the XML reader (no DOCTYPE), rebuilt from a whitelist (`p span b i u s br div ul ol li`,
  `style` properties for font, colour, alignment and spacing, `dir`); links lose their address, `script` and `style`
  go with their content. Never executed or fetched. Over 128 KB or 16 levels deep it is dropped and the plain
  `/Contents` stays; when a file has no `<contents>`, the plain text is made from the rich text. An edit by Inkwell
  replaces `/Contents` and drops `/RC` (MuPDF does the same).
- **Imported** as one undo step. A comment whose name matches one in the document is updated in place; the others are
  added, and a reply keeps its parent. A comment is drawn with the file's own appearance when it could be read as above
  (the comment's `/AP` is that one, with the file's `/Rect`; an edit by Inkwell's own code regenerates it); otherwise
  with Inkwell's own, and the report counts it ("made without part of the file", with the reason). Rich text and a
  caret's `fringe` and `symbol` are put back last, after every setter. Skipped and counted by reason, never a failure
  of the whole import: a page that is not in the document,
  a type these files cannot carry here, a comment of another type that has the same name, a reply to a comment that is in
  neither the file nor the document, no position, a measurement in the file (`/Measure` or a dimension intent: it is not made as a plain line),
  a comment whose name is a measurement of the document's (the Measure tool changes those; an update would leave the
  value stale), an attachment whose file is over its limit, is a link or is missing, and anything past 10 000 comments
  in one file.
- **Never done with a file:** run `/JavaScript` or `/AA`, follow `/F` or `<f href>` file references or open an
  attached file, load a DTD or entities (a `DOCTYPE` is refused), read more than 32 MB, 100 000 elements or
  attributes, 64 levels or 1 MB in one string (9 MB for a `<data>` or `<appearance>` element, which holds a file).
- **Certified documents** take the import only when the certification allows annotations (`/P` 3); at `/P` 1 or 2, or
  when the certification cannot be read, the menu item is off with the reason and nothing is changed. Export is always
  allowed. **Signed** documents take the import as an incremental update after the signature, and the summary says so.
- No file written by Acrobat was available while this was built: the shapes follow ISO 19444-1 and ISO 32000-2
  section 12.7.8, and are tested against our own exports and hand-written hostile files. A check against Acrobat's own
  XFDF and FDF is still to do. What is least certain: what Acrobat puts inside `<appearance>` (Inkwell reads and
  writes the XML tree described above there, from PDFBox's reader), the exact attributes of `<data>`, and the caret attributes `fringe` and `symbol`.
- Tests: `annot::xfdf::tests` and `annot::xfdf::tests_full` (round trips of carets, rich text, attachments and picture
  stamps in both formats and between two documents, the appearance drawing and replaced by an edit, a hostile file per
  new element, certified and signed documents), `src/lib/dataExchange.test.ts`.

## Later

- Screenshots (light and dark) in `docs/screens/comments/` still to be taken on copies of sample files.
