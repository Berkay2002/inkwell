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

## Open follow-ups (fix at the start of phase 2)
- `engine/mod.rs` `run_tile`/`page_list`: a stale in-flight request for rev N can build from content N+1 and cache it as N.
  Return `Stale` when `rev != entry.generation()` and re-check before the cache insert. (Medium; must be fixed before any edit lands.)
- `page_list` `OnceLock` caches `Err` forever: don't cache errors.
- Dedupe keeps the first requester's priority class: raise it when a main-view request joins a thumbnail job.
- `close()` vs `run_tile` race can leave orphan cache entries: re-check `closed` under the `tiles` lock.
- Unverified: CSP under `pnpm tauri dev` (inline React-refresh preamble), `--enable-low-end-device-mode` visuals at high zoom.
- `vendor/mupdf-sys` (65 MB, one patched file `load-jpx.c`) could become a patch + script step.
