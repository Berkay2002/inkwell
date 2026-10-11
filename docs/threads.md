# Article threads

An article thread (PDF 32000-1, 12.4.3) is the reading order of a story that runs through columns and pages: a
circular, doubly linked list of beads. Inkwell reads them, follows them, and lets a person author, edit, reorder and
delete them. Every change is an edit-core step: one undo step, kept by Save, recorded in history.

Code: `src-tauri/src/organize/threads.rs` (+ `threads/tests.rs`), wrappers in `organize/mod.rs`, commands in
`organize/app.rs`, the hook for deleted pages in `engine/edit.rs`. UI: `src/lib/threads.ts` (state and every decision
about where reading goes next, no DOM), `src/components/threads/` (`ThreadLayer.tsx` the page overlay,
`Threads.tsx` the host: pill, keys, Properties sheet; `ArticlesPanel.tsx`; `threads.css`). Tests: `cargo test --lib
organize::threads::` and `src/lib/threads.test.ts`.

## The file

- `/Threads` in the catalog lists the threads; a thread has `/F` (first bead) and `/I` (title, subject, author,
  keywords). A bead has `/T` (thread), `/N` and `/V` (next, previous), `/P` (page), `/R` (rectangle in the page's
  own space). Each page lists its beads in `/B`.
- **Ids are object numbers.** A thread or bead id is valid for the generation it was listed at; every step looks its
  ids up again and refuses one that is no longer there (the UI reads the list again after every generation).
- **Coordinates** on the wire are the page as shown (points, y down, `/Rotate` and CropBox applied), the same space
  as form fields and comments; the engine converts to and from the page's user space (tested on a rotated page with
  an offset CropBox). A rectangle is clamped to the page, kept at least 3 pt a side and finite.
- **Every step rewrites its thread** from a list of beads (`commit`): the ring is closed both ways, `/F` is valid,
  `/T` is on every bead, and the touched pages' `/B` arrays are rebuilt (other threads' entries kept, this thread's
  beads in ring order, the key dropped when empty). The invariants hold by construction. `/Threads` goes when the
  last thread goes. A missing `/T` is tolerated on read and always written.
- Deleting the last bead of an article deletes the article. Setting what is already there (same rectangle, same
  properties) adds no undo step.

## Hostile files

The walk is iterative with a visited set. Caps: 1000 articles, 10 000 beads per article, and 100 000 beads visited
in all by one read of the document (a thousand articles may share one ring; the ones past the budget are listed
damaged with no beads). The walk of an article **stops at the first bead of another article** (a /T that is not
its own): that bead and what follows are not its beads. A thread is listed
`damaged` (with the beads that could be read before the flaw and the reason) when: a bead names another thread, a
bead's page is not in the document, a rectangle is empty or not finite, the ring does not close or loops without
coming back to `/F`, a `/V` link is wrong, or the cap is hit. **A damaged article accepts only Properties and
Delete**; every other step returns an error and changes nothing. Deleting one removes only its own beads: a bead
that another article's ring runs through (even without a /T) stays on its page. The UI draws its beads dashed in the danger colour
and shows the reason in the Articles panel.

## Pages and other edits

- **Delete pages** unlinks the beads on them (the article closes up; one that loses all its beads goes). This runs
  inside the same undo step as the delete, also when Extract deletes after extracting. Only an article that was
  whole apart from those beads is mended; a damaged one is left exactly as it was.
- **Pages** that are not indirect objects (a direct dictionary in /Kids) cannot be named by /P and refuse beads.
- Deleting or moving an article edits /Threads in place: entries Inkwell does not list (past the cap, not
  indirect) are kept.
- **Move pages / insert blank** keep the beads (they are on the page object).
- **Extract, split, graft, combine** do not carry beads: the new file has no `/B` and no `/Threads` from the
  source. Insert pages from file does not bring them either (tested for extract, graft and combine).

## Reading (follow mode)

- A click on a bead's number tag ("Show beads" on), a click on a bead while an article is being read, a double
  click on an article in the panel, or the arrow button in its row starts reading. The cursor over them is an
  arrow in a circle.
- The view zooms so the bead's width fills it (24 px each side), scrolls to the bead's top and left edge. A turned
  view (`rot` not 0) uses the bead's height for the zoom and only jumps to the page's top.
- **Enter or Down** scrolls on through the bead (85 % of the view at a time, never past its bottom); at the bottom
  it goes to the next bead. **Shift+Enter or Up** goes back the same way (landing on the top of the previous bead).
  Past the last bead reading ends; at the first it stays. **Esc** or the pill's close button leaves; the zoom
  the person had comes back.
- The pill at the bottom shows "Title · 3 / 12" with previous and next. The bead in view is faintly filled, the next
  one is outlined dimly.
- The keys are left alone when a text field has focus, a menu or sheet is open, or Ctrl/Alt/Meta is down. Enter and
  the arrows also stay with a focused button or list (the pill's buttons, the sidebar); starting to read moves
  focus to the page. Esc always leaves.

## Articles panel and Properties

The sidebar's fourth tab. Rows: title, bead count (or the damage). Click goes to the first bead, double click reads
from it; right click offers Read, Properties, Move up/down, Delete. "New article" arms the Article tool. "Show beads"
draws every bead with its number: "1-3" is the third bead of the first article. Properties edits title, subject,
author and keywords (one undo step).

## Article tool (authoring)

Tools > Article threads, or the palette's "Article threads", opens the panel and arms the tool. It is exclusive with
Mark text or area, the comment tools, the form tools, the text tool and the link tool (arming one puts the other
down).

- Drag over a column: a new article ("Article n", the first title not taken) with that bead; the bead is selected.
- With a bead selected, a drag on any page adds the next bead after it (also on another page; scroll, then drag).
  The "+" at the selected bead's bottom-right starts the same drag from that corner. A click on bare page lets go
  of the selection, so the next drag starts a new article.
- Drag a selected bead to move it, its handles to resize it. The engine is asked once, when the pointer is let go.
  A bead stays on its page; to put it on another page, delete it and draw it there.
- **Delete** removes the selected bead. **Esc or End** finishes the article and opens its Properties; Esc again puts
  the tool down. Double click on a bead reads from it.

## Limits

- Not done: moving a bead to another page by drag; reading backwards lands on the top of a bead, not its end;
  article properties beyond the four `/I` entries; threads in PDF Portfolios' embedded files.
- A view turned with Rotate is read at the bead's width, but the scroll goes to the page, not the bead.
- **The pointer**: bead outlines never take it, so text selection, links and the comment, form, text and link
  tools work under "Show beads". Only the small number tag does (a click reads from that bead). While an article
  is being read the whole bead does, unless another page tool is in the hand (then not even the tag). Arming the
  Article tool puts the other tools down, and arming one of them puts the Article tool down.
- pdfcraft is not used.
- Windows is the reference platform; this was written and tested (cargo, vitest, tsc) on macOS and the UI has not
  been run on screen yet.
