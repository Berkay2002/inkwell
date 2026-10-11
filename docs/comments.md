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
(an object an earlier array listed; first sights and links are free) the remaining pages are left out and `incomplete` is true; the panel says so.

## Later

- XFDF import and export is not built. The comment list (`annot_list`) already carries everything XFDF needs (type,
  page, rect, quads, colour, contents, author, dates, parent, state), so it can be a reader/writer over that DTO.
- Screenshots (light and dark) in `docs/screens/comments/` still to be taken on copies of sample files.
