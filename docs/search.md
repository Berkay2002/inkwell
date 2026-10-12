# Search: find options, a results rail, search across files

Engine: `src-tauri/src/search/` (`mod.rs` the matcher, `files.rs` search across files; both adapt the matching idea of
pdfcraft's `find_opts`, see `third_party/pdfcraft/INKWELL-NOTICE.txt`). The per-document search job lives in
`src-tauri/src/engine/mod.rs` (`search_with`). UI: the find box (Ctrl+F) in `Chrome.tsx`, the Search rail (Ctrl+Shift+F,
`components/search/`), state in `lib/findStore.ts` (options, history, field text), `lib/searchStore.ts` (rail,
across files) and `store.ts` (`runSearch`, `stepHit`, `selectHit`, `stopSearch`).

## Find box and options
- While Edit text is on, the box has a second row with **Replace with**, **Replace** and **Replace all**: the matches are
  these (same query, case and whole words), and the text blocks that hold them are edited. See
  [text-edit.md](text-edit.md) ("Find & Replace").
- Ctrl+F opens the floating find box. Live search as you type (220 ms), Enter / Shift+Enter or F3 / Shift+F3 step.
- Two chips sit in the box: **Aa** match case, **ab** whole words. The "…" menu holds **Ignore accents**, **Include
  comments**, **Include bookmarks**, **Include form fields** and **Advanced search…**. Options are kept for the next run
  of the app (local storage) and apply to the rail and to search across files too. A dot on "…" says one of its options
  is on.
- Matching: case folding (Unicode), runs of white space count as one, a phrase may run over a line break (one
  highlight per line), a hyphen that ends a line and is followed by a word joins the two halves ("co-" "operate" finds
  "cooperate"; "well-" "known" is found as "well-known" too, one result either way). A soft hyphen is never part of
  the text. Letters are compared in decomposed form on both sides, so "é" typed as one character or as "e" plus an
  accent is the same, and "ß" is "ss", a final "ς" is "σ" (case folding). Whole words: the characters on both sides
  must not be letters or digits. Ignore accents: letters are compared without their marks (é = e), the query's too;
  without it "cafe" does not find "café".
- Comments: the text of every markup annotation (not links, widgets, popups or redaction marks); the highlight is the
  annotation's rectangle. Form fields: the field's value. Bookmarks: the title of an outline entry that goes to a page.
- A query is at most 256 characters; a page lists at most 2000 hits in all (text, comments and form fields share
  that budget). One in-document search lists at most 5000 hits, then stops and the rail says "Showing the first 5,000
  results".

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
- Bounds: 100 listed hits per file, 5000 listed in one search (counting goes on), 20 000 files, 200 000 directory
  entries looked at while listing, one document open per thread at a time and dropped after it is read.
- Runs on two threads of its own per search (below normal priority), not on the engine's pool, so it cannot starve
  in-document search, page-size scanning or the AI reads. Cancel also reaches the listing of the folder.
- Cloud-only files (OneDrive placeholders: offline / recall-on-open / recall-on-data-access) are not read, so a search
  never starts a download; the rail says "N cloud-only files skipped".
- History: the last ten queries (Enter, or clicking a result, or running a folder search records one), newest first,
  from the clock in the rail's field; "Clear recent searches" empties it. Stored only in this app's local storage.

## Catalog
A catalog is a saved full-text index over a folder: build it once, search it in milliseconds, refresh it when files
change. Own index, not tantivy, for three reasons: (1) the index only narrows which pages to look at, and the same
matcher as the folder search decides every hit, so results are identical for every option combination (a test
compares 16 option sets over 23 queries); (2) tantivy brings about 60 crates and a long build for ranking and
analysers a verbatim finder does not use; (3) the index must also fit inside a PDF (below), which needs a small
format we control and version.

- Storage: `<app data>/catalogs/<id>/`: `catalog.json` (list entry, written last by rename, so it is the commit),
  `table-<gen>.json` (files: size, mtime, head/tail hash, pages), `index-<gen>.bin` (magic, version, blake3, varint
  postings of space-delimited words in the widest fold), `text/<fid>.zst` (the per-page records the matcher reads).
  A damaged or foreign-version index (or `catalog.json`) is never trusted: the catalog is listed as not built,
  `check` reports it `unusable` and a build recreates it. A file whose worker died counts as damaged.
- Candidate rule: one word must be a substring of a term; several words are first=suffix, middle=exact, last=prefix;
  a query of marks only verifies every page. Hits carry the line's box, not exact character rectangles.
- Refresh: a file with the same size and mtime is kept without being opened; a moved file is kept by size, mtime and
  head/tail hash; a changed mtime is re-indexed. Damaged and password-protected files are skipped and counted, and
  tried again at every refresh (only an indexed row is kept). A build reads each PDF into memory (up to 128 MB; a
  larger one is opened by path) so no handle stays on it, and `catalog.json`, table and index are fsynced before the
  rename. Cancel leaves the previous generation live.
- A search checks every candidate file against the disk (size, mtime). A file that is not the one that was indexed is
  left out and counted (`done.stale`, a note under the results), so a catalog never answers from text that was
  saved over. A save that includes a redaction (`save_document`, `protect_and_save`) makes every catalog forget that
  file: row, stored text and postings are rewritten as a new generation (`forget`), and the file is "new" until the
  next refresh. A catalog that is being built forgets it when the build ends (the search check hides it meanwhile): the
  path waits in `forget-pending.json` in the catalog's folder, so a quit does not lose it, and is replayed at startup,
  when the catalogs are listed, and at the next hold. If the postings cannot be rewritten, or the entry or table is
  unreadable, the stored text and the live index are deleted: the catalog reads "needs a rebuild" and keeps no words
  of the file. The same is asked for (`forget-wipe` marker in the catalog's folder, memory if it cannot be written)
  when the waiting list is unreadable (a missing file is an empty list), cannot be written, or would pass 1000 paths:
  a path is never dropped. The wipe is not done by whoever finds this out, because the catalog is held by a build that
  would write its words back: it runs under the hold of whoever frees the catalog, after the build, and the marker
  and list stay while it fails. The next build reads every file again. A Save as, and Redact's own save, that replace a listed file do the same. So does a batch run with a Redact or Sanitize step, for
  each PDF it writes, as soon as that file is written (its result can take a name a catalog still lists). A forget that
  finds the catalog held is looked at again once it is written down, and the holder looks once more after it lets go,
  so a request that lands between the holder reading the list and letting go is not left for a later hold.
  Listing catalogs replays a list or a wipe (a marker, or the in-memory one) without taking the hold of a catalog that has nothing waiting.
- Embedded index: `/Root/PieceInfo/Inkwell` (`/Private` stream) holds the records of that file. A build reads it only
  if the trailer `/ID`, the page count, the version and a fingerprint of page content and annotations all match,
  so a file changed by another tool is read normally. Embedding and removing are one undo step each
  (`index_embed`, `index_unembed`; `index_embedded` reports present/current and `blocked`). A certified or signed
  document gets no index written, updated or removed (refused with the reason; the buttons are disabled): an update
  or removal rewrites the file, which voids signatures, and a certification allows none of it. This is stricter
  than the common rule for edit features (signed: incremental save plus a warning) on purpose: the first Embed on a
  signed file would be incremental, but Update and Remove are not, and one rule is easier to explain than three.
- An embedded index is untrusted input, like hidden text in the PDF itself: it can claim words that are not
  visible, and bookmarks on pages the file does not have are dropped. A build checks the dict (`/DocID`, `/Pages`)
  before decoding the blob, inflates it under a bound, and compares a fingerprint of the page content (page
  streams, Form XObjects and patterns under the resources to depth 8, fonts (`/ToUnicode` and `/Encoding`; the whole
  font with its CMaps, descendant fonts and font programs when there is no `/ToUnicode`), marked-content property
  lists, the layers' defaults, comment and field values as the records read them, bookmarks) read under a
  per-stream and a per-file budget; every entry walked costs budget, and a `Resources` shared by pages is walked once.
  The second `/ID` string is not part of the match: MuPDF writes a new one on every save, Inkwell's own included.
  The fingerprint detects honest edits, not a forger: a crafted file can always carry an index that is "current"
  but says other words, the same as invisible text in the PDF itself.
  A save that merges duplicate objects, or a layer `/Order` nested deeper than about six levels, makes the index "not
  current": a build reads the pages instead (slower, never wrong).
- What is removed with the text: redaction and sanitize always drop the embedded index (it is a compressed copy of
  the page text, which the byte check cannot see inside); deleting pages, editing text, removing attachments or
  annotations, PDF/A conversion to a new file, Bates numbering with replace, and a Compress or batch run (dropped
  extras, flattened pages, unembedded fonts) drop it unless it still matches the file; the metadata and private-data
  options remove it with the rest of `/PieceInfo`. Extract, split and combine build new files and carry none.
- Bounds: 20 MB of stored page records per file, measured as the JSON that is written (a page that does not fit is
  left out, the file is marked cut and counted), bookmark titles 5 MB on top, 100 000 pages, 50 000 bookmarks, 96 MB
  of inflated index (a record that would inflate past it is not written).
- Estimate (before the first build, from a listing): about 25 MB/s on two threads plus 20 ms per file, and an index of
  about 3 % of the PDF bytes plus 2 KB per file. A guess to set expectations, not a promise.
- Commands: `catalog_estimate`, `catalog_create`, `catalog_list`, `catalog_delete`, `catalog_stale`,
  `catalog_build(id, full, onEvent)` (`started`, `progress`, `done`), `catalog_cancel(job)`,
  `catalog_search(id, query, opts, onEvent)` (the folder search's events; results open through `search_file_token`).
  Not exposed to the AI assistant.
- Rail: Folder scope lists "Catalog: <name>" with Refresh and Delete and a badge ("2 new, 1 changed"); "New
  catalog…" picks a folder, names it and shows the estimate ("~1 800 files, about 2 minutes, ~40 MB") before
  building with progress and Cancel. "Embed search index" / "Remove embedded index" sit above the file results.

## Commands (Tauri)
`search(doc, query, opts, onEvent)` streams `hits` (per page, each with rects and snippet), `progress` and `done`;
`cancel_search(doc)`; `pick_search_folder()` returns an open token for a folder (paths.rs: the webview never names a
path); `search_files(scope, query, opts, onEvent)` with `scope` a folder token or file tokens streams `started` (the id,
before the folder is listed), `file`, `progress`, `done`; `cancel_search_files(search)`;
`search_file_token(search, index)` issues the open token of a result when it is opened (tokens are not issued for
every file in the list; only the latest search's files can be opened).

## Checked
`cargo test --lib search::` (matcher options, snippets, comments/fields/bookmarks, engine events, folder listing,
caps, skip, cancel, save and reopen; `search::catalog`: parity with the folder search, refresh reads only changed
files, damaged/encrypted skipped and retried, cancel keeps the old index, corrupt index rebuilt, embedded index and
its undo, signed/certified refused, a saved redaction gone from index and results, changed files left out), `src/lib/searchLogic.test.ts`, `src/lib/search.test.ts`, and
`scripts/search-shots.mjs` (CDP on generated copies; screens in `docs/screens/search/`).
