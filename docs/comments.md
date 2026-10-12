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

## Data files: XFDF and FDF

Comments panel menu (the three dots) and the command palette: **Export comments…** and **Import comments…**. Code:
`src-tauri/src/annot/xfdf.rs` (comments) and `src-tauri/src/exchange.rs` (caps, XML reader, FDF reader shared with form
data). The format of an export follows the file name: `.fdf` is FDF, anything else XFDF 3.0 (ISO 19444-1).

- **Exported:** text notes, highlight, underline, squiggly and strikeout (with quad points), ink, square, circle, line,
  polygon, polyline, free text and stamps (by name), with name (`/NM`), page, rect, colour, opacity, border width, author,
  subject, dates, contents, review state and replies (`inreplyto`). A comment with no `/NM` is written under a name
  made for the export (`inkwell-<page>-<object>`, nothing is added to the document); importing it back adds it again
  instead of updating it, because that name is not unique across files. Comments the tools add from now on carry a
  real `/NM`. Carets, file attachments, sound and redaction marks are left out and counted.
  Appearance streams are not written. Page numbers and points are the file's (PDF user space, y up, 0-based page).
- **Imported** as one undo step. A comment whose name matches one in the document is updated in place; the others are
  added, and a reply keeps its parent. Each comment is drawn with Inkwell's own appearance (the file's `/AP` and rich text
  are not used). Skipped and counted by reason, never a failure of the whole import: a page that is not in the document,
  a type these files cannot carry here, a comment of another type that has the same name, a reply to a comment that is in
  neither the file nor the document, no position, and anything past 10 000 comments in one file.
- **Never done with a file:** run `/JavaScript` or `/AA`, follow `/F` or `<f href>` file references, load a DTD or
  entities (a `DOCTYPE` is refused), read more than 32 MB, 100 000 elements, 64 levels or 1 MB in one string.
- **Certified documents** refuse the import (the menu item is off, with the reason); export is always allowed. **Signed**
  documents take the import as an incremental update after the signature, and the summary says so.
- No file written by Acrobat was available while this was built: the shapes follow ISO 19444-1 and ISO 32000-2
  section 12.7.8, and are tested against our own exports and hand-written hostile files. A check against Acrobat's own
  XFDF and FDF is still to do.

## Later

- Screenshots (light and dark) in `docs/screens/comments/` still to be taken on copies of sample files.
