# Organize-plus: combine files, attachments, page labels, page transitions

PDF Portfolios (the collection view over the attachments) are in docs/portfolios.md; article threads in docs/threads.md.

Code: `src-tauri/src/organize/` (`combine.rs`, `attach.rs`, `labels.rs`, `trans.rs`, `app.rs` Tauri commands, `mod.rs`
edit-lane entry points, `tests.rs`). UI: `src/components/CombineSheet.tsx` (+ `src/lib/combineStore.ts`,
`combineLogic.ts`), `AttachmentsPanel.tsx`, `OrganizePlus.tsx` (labels and transitions sheets), `Presentation.tsx`
(full screen, `src/lib/present.ts`). Design round: `design/rounds/organize-plus-r1/spec.md` (verdict a-sheet).

Every document change goes through the edit core: one undo step each, kept by Save, recorded in history. Files are
named by token only (`paths.rs`); the UI never sees a path it did not get from a dialog or a drop.

## Combine files

Entry: Tools > Merge/Combine, Organize > "Combine files…", command palette "Combine files…".

- **Plan and run are one code path** (`combine::prepare`). The sheet asks `combine_inspect` after every change and
  shows the outcome sentence beside the Combine button ("Combines 4 files, 37 pages, 4 bookmarks. 1 file is left
  out"); `combine_run` writes exactly that.
- **Per file**: range text (`1-3, 6, 9-7`, a backwards range reads backwards, `5-` runs to the end, empty is every
  page), bookmark title (default the file name), reorder by dragging the grip, remove. A first-page thumbnail per row;
  "pages" expands page thumbnails that toggle on click and rewrite the range text. The text stays the source of truth.
- **Options**: a bookmark per file, each file's own outline kept under its bookmark, a contents page in front
  (one line per file with its first page number and a link), and "Keep the source page labels" (on by default).
- **Page labels** (PDF 32000 12.4.2): each source's `/PageLabels` go into the result, shifted to the page's place in
  it and limited to the pages picked, in the order picked (`1-3, 6, 9-7` labels as the source labelled 1-3, 6, 9-7).
  A source without labels gets plain decimal numbers that carry on the combined page numbers (a file of 3 pages
  after 5 labelled ones shows 6, 7, 8), as Acrobat does. A contents page in front is plain decimal and counted. A
  result whose pages are all plain decimal has no `/PageLabels` at all, and a source that only spells out plain
  decimal numbering from page 1 (many producers write that) counts as unlabelled. A source whose labels cannot be read counts
  as unlabelled; a hostile number tree (a node that is its own kid, a million entries) is read under a cap on the
  total work, so it costs time but never hangs the sheet. Unticked: the result has no labels, as before.
- **Form fields** are kept; a field whose name clashes with one already in the result is renamed the way Acrobat does
  ("Name", "Name (2)"). Root field names are the ones compared, as in Insert pages from file.
- **Locked files**: a row that needs a password says "Password needed" and has an Unlock button. It asks with the same
  prompt as opening a file ("Password required", "x.pdf is protected."); a wrong password says so and changes nothing.
  The password lives in the sheet and in the requests for the plan and the run only: it is not written to the
  plan, the history or any log (`Secret` in Rust: zeroed on drop, never serialised or printed; the sheet drops it
  when it closes and a failed run takes it away, so the row asks again). The combined file has no password, and the
  sheet says so. Unlocking opens the file's own handle without starting a version history for it.
- **Left out, with the reason shown on the row**: files that need a password and have not been given it ("Password
  needed"), files whose owner allows no copying or page changes (even when the password opens them: the owner's
  restriction still applies), files that are not PDFs. A range outside a file refuses the
  run and says which file and page ("Page 9 is not in a.pdf"). An open document's own file is never the output.
- **Output**: a save token from the Save dialog; the result opens in a new tab and the sheet closes.
- Dropping PDFs from Explorer on the app while the sheet is open adds them (same token route as drop-to-open).

Page `/Trans` and `/Dur` travel with the pages (also on Insert pages from file).

## Attachments

The sidebar's third tab (paperclip; also a toolbar button). Lists the document's `/EmbeddedFiles` and the page
`FileAttachment` annotations (with their page; click goes there). Per row: Save to a folder, Open in a new tab (PDFs
only, as an unsaved scratch document), Edit description, Delete. "Attach files" takes any files (up to 512 MB each).

- **An attachment is never run.** Save writes bytes to the file the person picked; a PDF can be opened as a document
  of its own; nothing else is offered. A `FileAttachment` that links to a file outside the document has no bytes and
  says so.
- One undo step per change (several files attached together are one step). Delete removes the stream, so after a save
  the bytes are not in the file (tested with 64 KB of incompressible data).
- A second file of the same name is stored as "name (2).ext", never replaced.
- **Names** are cleaned wherever they are shown or used (the list, the Save dialog's default, the scratch file of
  "Open in a new tab"): no folder, no control or bidi/zero-width characters (U+202E and the like), no `< > : " | ? *`,
  device names such as `CON` or `NUL.txt` get a `_` in front.
- **Size**: reading an attachment out stops at 512 MB of decoded data whatever the stream declares (a small compressed
  stream cannot inflate past it; `mupdf_pdf_read_stream_capped`).
- **Save** never writes over a file that is open in a tab, and marks the written file as from the Internet zone
  (`Zone.Identifier`, ZoneId=3) on Windows so the shell warns before it is run.

## Page labels

Organize > "Page labels…", the page context menu, command palette. Pages from/to, numbering (1, i, I, a, A, none with
prefix only), prefix, start at, or "carry on the numbering of the pages before". Shown in the page box (type a label
to go there: `iii`), under every thumbnail and on the Organize grid; the page box reads "iii (3 of 12)".

- Sections are anchored to pages. Deleting pages moves a section start to its first surviving page and keeps its
  start number; inserted pages join the section before them; moving pages is positional. MuPDF adjusts labels on
  delete and insert by itself, so the engine snapshots the sections before the edit and writes the result it wants
  after it.
- One undo step; redundant sections are dropped so a plain document stays plain.

## Page transitions

Organize > "Transitions…", the page context menu. Pages (range text), effect (Split, Blinds, Box, Wipe, Dissolve,
Glitter, Fly, Push, Cover, Uncover, Fade, or none), the controls the effect uses (horizontal/vertical, inward/outward,
direction), seconds it takes, and "Go on by itself after n seconds" (`/Dur`). One undo step; setting what is already
there adds no step.

**Full screen (Ctrl+L, toolbar button, command palette)**: the page fills the screen; Space, Enter, → ↓ PageDown or a
click go on, ← ↑ PageUp Backspace go back, Home/End, Esc leaves (the reader goes to the page you were on). The next
page is drawn ahead. The page being entered supplies the effect, going forward only; pages with `/Dur` advance by
themselves. With "reduce motion" on, effects are skipped.
Effects are CSS approximations of the PDF's: wipe, push, cover, uncover, split and box by clipping or sliding,
blinds by a strip mask, fade/dissolve/glitter as a cross-fade.

## Known limits

- Combine opens each file once for thumbnails (a second parse). The plan is asked for one at a time, 220 ms after the
  last change, and what it learned about a file (pages, bookmarks, first page size) is kept while the file's size and
  time are the same, so editing a range does not parse the files again. A run reads each file's facts one at a time
  (each is closed before the next), then opens a file only when its turn comes, grafts it and closes it, so the
  files not reached yet are never held. A file with no pages is left out ("has no pages"), and so is any file that
  fails at any step of reading (open, page count, outline, first page): only that file is marked Unreadable, never
  the whole look. An open or read error is not remembered between looks (it may be a lock that passes); a
  password, a missing copy right or an empty file is. The plan records each file's path, length and time before
  reading it; when its turn comes the run compares them right after opening, and a file that differs (replaced,
  touched, deleted) stops the run with "X has changed since it was added" and asks to add the files again. The page
  count is a second check, and the bookmarks the plan promised are checked against what is written (an error, not a
  panic).
- A capped read (an attachment unpacking past its limit) raises the wrapper's own code
  `mupdf_error_stream_cap` (not `FZ_ERROR_LIMIT`, which MuPDF also uses for unrelated limits); callers match
  `Error::is_stream_cap()`, not the message text.
- The form fields of every source bring their `/DR` resources (fonts) along, merged key by key; where two files name
  a font the same, the first one's is kept.
- A file opened with a password is not remembered between looks (the cache key holds no password), so such a
  file is read again on every look. Combining never writes a source, so signed or certified sources need no
  special handling. A signature field of a source arrives empty (its value and appearance are dropped, the
  field stays), so the result is never a signed document that fails to verify. The row says "x.pdf's signature
  is not carried over". Insert pages from file does the same.
- Labels are carried page by page: a source's section that is cut by a page range or reordered becomes several
  sections in the result, each with the number the page had in its source.
- The attachments list does not show portfolio (collection) structure; a PDF Portfolio opens in its own file list (docs/portfolios.md).
- Transitions are not previewed in the sheet itself, only in full screen.
- Full screen uses the HTML Fullscreen API; if the platform refuses it, the page still fills the window.
