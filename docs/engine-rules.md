# Engine rules for editing phases (2-5)

From the phase 1 pre-merge review. The viewer keeps up to `DOC_SLOTS=4` read-only replicas per file; edits must not go through them.

1. One authoritative document per file for every mutation (annotations, page organize, form fields, redaction).
   Mutations run on a dedicated serial lane against it, never through `lock_slot`.
2. After each committed edit: snapshot to memory (incremental save to a buffer where possible), then `bump_generation`.
   Replicas reopen from that snapshot, not from `entry.path`. Until a snapshot exists, render from the authoritative doc only.
3. Bump the generation only after the snapshot exists. Every render/list/text result is checked against the current
   generation. The frontend gets the new `rev` in `DocState` and re-requests (today `rev` is always 0).
4. Page organize changes `page_count` and `sizes`: swap them atomically with the generation; page indices belong to one generation.
5. Save = write from the authoritative doc to a temp file, release every replica/display list on the old file (see `close()`), then rename over the original.

## File paths: tokens only (`src-tauri/src/paths.rs`)

The webview never hands Rust a path. Every command that reads or writes a file takes a token; Rust is the only source of paths.

- **Get a token**: `pick_open(multiple, title?) -> FileRef[]` (`title`: one of Rust's fixed titles, `"locate"` = "Locate the moved file", `"compare"` = "Choose two PDFs to compare"; add a variant to `PickTitle` in lib.rs for a new one, never pass text) and `pick_save(name, doc? | near?) -> FileRef | null` run the
  dialogs in Rust (off the UI thread). Drag-drop (`files-dropped` event) and command-line files (`initial_files`) come
  with tokens. `known_file(path) -> FileRef` re-issues one for a path the user chose before (the stored recent list);
  any other path is refused. `FileRef = { token, path }`: `path` is for display and dedupe only.
- **Kinds**: open tokens name a file to read and can be reused; save tokens come only from `pick_save` and work once.
  Both belong to the window that got them. Unknown, spent, wrong-kind or other-window tokens are `Invalid`.
  Asking again for the same file in the same window returns the same open token (the registry doesn't grow).
- **Open documents** are named by doc id. Save over the file is `save_document(doc)`; its path never leaves the engine.
- **In a command**: `reg.open(window.label(), &token)?` for inputs, `reg.take_save(window.label(), &token)?` for
  outputs, before the blocking work; a result file to open comes back as `reg.issue_open(...)` (see `merge_files`).
- Known paths (`stat_files`, `reveal_in_folder`, `recents_save`) are every path a token was issued for plus the stored
  recent list; a save target becomes known only once the write succeeded. `recents_save` drops entries for other paths.
  The webview's `dialog:allow-open/save` permissions are off.
- The pre-recents.json list in localStorage (`inkwell.recent`) is not trusted: its first-run import is filtered to
  nothing by `recents_save`, and the old list is left in place rather than deleted.

New tools (history, compress, OCR, redact) take the same tokens: never add a command parameter that is a path.
`history_save_copy(doc, version, to)` takes a save token from `pick_save` and makes the copy's path known.

## Open follow-ups (fix at the start of phase 2)
- `engine/mod.rs` `run_tile`/`page_list`: a stale in-flight request for rev N can build from content N+1 and cache it as N.
  Return `Stale` when `rev != entry.generation()` and re-check before the cache insert. (Medium; must be fixed before any edit lands.)
- `page_list` `OnceLock` caches `Err` forever: don't cache errors.
- Dedupe keeps the first requester's priority class: raise it when a main-view request joins a thumbnail job.
- `close()` vs `run_tile` race can leave orphan cache entries: re-check `closed` under the `tiles` lock.
- Unverified: CSP under `pnpm tauri dev` (inline React-refresh preamble), `--enable-low-end-device-mode` visuals at high zoom.
- `vendor/mupdf-sys` (65 MB, one patched file `load-jpx.c`) could become a patch + script step.
