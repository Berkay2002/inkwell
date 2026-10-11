# Search: find options, a results rail, search across files

Engine: `src-tauri/src/search/` (`mod.rs` the matcher, `files.rs` search across files; both adapt the matching idea of
pdfcraft's `find_opts`, see `third_party/pdfcraft/INKWELL-NOTICE.txt`). The per-document search job lives in
`src-tauri/src/engine/mod.rs` (`search_with`). UI: the find box (Ctrl+F) in `Chrome.tsx`, the Search rail (Ctrl+Shift+F,
`components/search/`), state in `lib/findStore.ts` (options, history, field text), `lib/searchStore.ts` (rail,
across files) and `store.ts` (`runSearch`, `stepHit`, `selectHit`, `stopSearch`).

## Find box and options
- Ctrl+F opens the floating find box. Live search as you type (220 ms), Enter / Shift+Enter or F3 / Shift+F3 step.
- Two chips sit in the box: **Aa** match case, **ab** whole words. The "…" menu holds **Ignore accents**, **Include
  comments**, **Include bookmarks**, **Include form fields** and **Advanced search…**. Options are kept for the next run
  of the app (local storage) and apply to the rail and to search across files too. A dot on "…" says one of its options
  is on.
- Matching: case folding (Unicode), runs of white space count as one, a phrase may run over a line break (one
  highlight per line), a hyphen that ends a line and is followed by a word joins the two halves ("co-" "operate" finds
  "cooperate"). Whole words: the characters on both sides must not be letters or digits. Ignore accents: letters are
  compared without their marks (é = e), the query's too.
- Comments: the text of every markup annotation (not links, widgets, popups or redaction marks); the highlight is the
  annotation's rectangle. Form fields: the field's value. Bookmarks: the title of an outline entry that goes to a page.
- A query is at most 256 characters; a page lists at most 2000 hits (the rest of the page is counted as read).

## Search rail
- This file: the hits grouped under their page with the words around each (bold on a pale amber wash), the count
  ("12 results on 5 pages"), progress and Cancel while it runs. Click a row or use ↑ ↓ Home End in the list; the page
  scrolls to the hit and the hit gets the stronger highlight. The list shows 300 rows and "Show more" adds 300, so a
  long document stays responsive; the number counted is always the full one.
- Folder: a folder (with or without sub-folders) or PDFs picked one by one. Two files are read at a time on the
  engine's background workers; the list fills file by file ("Searching 38 of 64 files"), Cancel ends it between two
  pages. Each file lists its first two hits, "Show 10 more" adds ten; the file's name opens it at the first hit and a
  row opens it at that hit with the query in the find box. Damaged and password-protected files are skipped and
  counted.
- Bounds: 100 listed hits per file, 5000 listed in one search (counting goes on), 20 000 files, one document open per
  worker at a time and dropped after it is read.
- History: the last ten queries (Enter, or clicking a result, or running a folder search records one), newest first,
  from the clock in the rail's field; "Clear recent searches" empties it. Stored only in this app's local storage.

## Commands (Tauri)
`search(doc, query, opts, onEvent)` streams `hits` (per page, each with rects and snippet), `progress` and `done`;
`cancel_search(doc)`; `pick_search_folder()` returns an open token for a folder (paths.rs: the webview never names a
path); `search_files(scope, query, opts, onEvent)` with `scope` a folder token or file tokens streams `file`,
`progress`, `done`; `cancel_search_files(search)`.

## Checked
`cargo test --lib search::` (matcher options, snippets, comments/fields/bookmarks, engine events, folder listing,
caps, skip, cancel, save and reopen), `src/lib/searchLogic.test.ts`, `src/lib/search.test.ts`, and
`scripts/search-shots.mjs` (CDP on generated copies; screens in `docs/screens/search/`).
