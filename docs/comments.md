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
  aside to `stamps.json.bad` and the library starts empty with a warning; unusable entries are dropped one by one.
- **Placing**: the click centres a stamp 150 pt wide with the picture's proportions, moved inside the page. The
  appearance is one Form XObject whose only content is the picture (with its alpha as a soft mask), so any PDF
  reader draws it, and MuPDF treats it as a standard stamp: move and resize redraw it in the new `/Rect`. Resize keeps
  the proportions (`fit_aspect`, mirrored in `src/lib/stampsLogic.ts` for the frame while dragging); the corner or edge
  that did not move stays put. One placement is one undo step, kept by Save.
- **Certified and signed**: placing uses `outline::refuse_unless_annotating_allowed` inside the edit, so a document
  certified with P1 or P2 refuses it with no history step and the picker is off with the reason; P3 allows it. A
  signed document takes the stamp as an incremental update and the picker says so. The library sheet never touches the
  open document, so it works with no document and on a certified one.
- **Not an assistant tool**: the `ai/` module does not reference `annot::stamps`; the library is the user's.
- Tests: `annot::stamps::tests` (Rust, with pixel and saved-bytes checks), the "place a custom stamp" row in
  `engine::edit::tests_certified`, `src/lib/stamps.test.ts`.

## Later

- XFDF import and export is not built (custom stamps would export as a stamp with its appearance stream). The comment list (`annot_list`) already carries everything XFDF needs (type,
  page, rect, quads, colour, contents, author, dates, parent, state), so it can be a reader/writer over that DTO.
- Screenshots (light and dark) in `docs/screens/comments/` still to be taken on copies of sample files.
