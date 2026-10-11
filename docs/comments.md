# Comments and annotations

Engine: `src-tauri/src/annot/` (every action is one undo step, kept by Save, recorded in history). UI: Comments rail
(Ctrl+Alt+M), tool strip over the page, selection frame with handles and a property popover.

Keys while Comments is open: V select, H highlight, U underline, S strikethrough, N note, T text box, P pen,
R rectangle, O ellipse, L line, A arrow, M stamp, Delete removes the selection, Esc returns to select.

Replies are hidden `/Text` annotations with `/IRT`; resolved state is a `/StateModel /Review` reply (latest wins).

## Later

- XFDF import and export is not built. The comment list (`annot_list`) already carries everything XFDF needs (type,
  page, rect, quads, colour, contents, author, dates, parent, state), so it can be a reader/writer over that DTO.
- Screenshots (light and dark) in `docs/screens/comments/` still to be taken on copies of sample files.
