# Version history store (`src-tauri/src/history/`)

Every change to a PDF (a save, an assistant turn, an autosave, a change made outside Inkwell) becomes an
immutable, encrypted version. Restore is byte-exact (signatures survive) and never destructive.

## Layout

```
<root>/                         default %LOCALAPPDATA%\Inkwell\history, movable (Store::relocate)
    lock                        held open exclusively by the one Store of one process
    store.json                  format version + retention policy (plaintext, no user data)
    index.key                   32-byte "index key", DPAPI-wrapped (current user)
    never.json                  "never keep history" tombstones: index-keyed hashes only
    docs/<doc_id>/
        key.dpapi               32-byte random doc key, DPAPI-wrapped
        identity.json           NTFS file id + volume serial; index-keyed hashes of path, /ID, head content
        versions/<version>.v    encrypted manifest (immutable)
        chunks/<aa>/<chunk_id>  encrypted chunk (immutable)
        names                   encrypted map version -> name (renames; manifests are never rewritten)
        wal                     write-ahead record of the commit in progress
        session                 present while the doc is open in a live Inkwell
```

- Ids: doc 128-bit random; version = 48-bit ms + 80-bit random, hex, so names sort by time. Ids are monotonic per
  doc (a clock step backwards can't reorder the head).
- Manifest (JSON, zstd, sealed): id, parents, created, author, kind, label, summary, name (commit-time), pages
  touched, size, content hash, `[(chunk_id, len)]`.
- Names: the commit-time name lives in the manifest; later renames/unnames go into `names` (one small encrypted
  record per doc). Effective name = `names[v]` if present, else the manifest's. A named version is kept forever.

## Crypto and the dedup trade-off

- Each object is sealed with XChaCha20-Poly1305 under a key derived from the doc key (`blake3::derive_key`),
  random 24-byte nonce, AAD = doc id + object kind + object id, so objects can't be swapped between slots or docs.
  The payload is a flag byte + zstd(level 3), or raw when zstd doesn't shrink it.
- **Chunk id = keyed BLAKE3 of the plaintext** (key derived from the doc key). A chunk file name doesn't let anyone
  confirm a guessed plaintext. Content hash (verified on every read) is keyed the same way.
- Consequence: **dedup is per document** (across all of its versions), never across documents. Cross-document dedup
  would need a store-wide chunk key, and then crypto-shredding one document could not make its content unrecoverable
  while another document still references the same chunk. Copies of one file therefore cost their full size once each.
  The benchmark below shows where dedup pays: within one document's save sequence.
- Doc and index keys are wrapped with `CryptProtectData` (current user, no UI, fixed entropy). DPAPI keys are
  user-bound, not path-bound, so a relocated store keeps working; another Windows user (or another machine) cannot
  open it.
- identity.json and never.json hold only the raw NTFS file id (not sensitive) and hashes keyed with the index key:
  no path, file name or content in plaintext. The no-plaintext test scans every byte of the store for strings from
  the fixture, the version name, the directory and the file name.

### Crypto-shredding (delete history, redaction purge, never keep)

`shred(doc)`: overwrite and delete `key.dpapi`, sync the directory, then delete manifests, chunks and the rest of
the doc's directory (the identity is forgotten). If the process dies in between, fsck sees a doc without a key and
finishes the deletion. Without the doc key the remaining ciphertext is noise, even where an SSD keeps old cells.

Ceiling (documented, not solved): the wrapped key itself may also survive in old flash cells, and DPAPI can still
unwrap it for the same user. Closing that needs keys that never touch disk unwrapped-able (e.g. a TPM-held key
encrypting a key file that is rotated on shred). Add if the threat model includes forensic recovery from the
user's own logged-in account.

`redactions_applied(doc, keep_history, redacted_bytes, meta) -> VersionInfo`, called after the redacted file is
written, in place of the Save commit. One operation under the doc's mutex, so a `check_external` (window focus)
can't record anything between the purge and the redacted version:
- `false` (default): write a fresh key beside the old one (temp file), forget the old key and versions in memory,
  destroy the old `key.dpapi`, rename the new key in, delete manifests, chunks, names and WAL (errors are returned,
  not ignored; a retry finishes the job), keep the doc's identity (minus the head content hash, a hash of unredacted
  bytes), then commit `redacted_bytes` as the first version. A crash between the destroy and the rename leaves a
  keyless doc, which fsck shreds.
- `true`: keep it all, set `keep_pre_redaction` in identity.json (the UI shows "pre-redaction history kept";
  `Store::keeps_pre_redaction`), and commit `redacted_bytes`.

Keys in memory (`Keys`, the index key, DPAPI outputs) are wiped on drop (`zeroize`), including a key replaced by a
redaction purge.

`set_never_keep(Target::Doc | Target::Path, true)`: records a tombstone (index-keyed hashes of path, file id and
/ID; a file matching any of them is excluded), then shreds the doc. `doc_for_file` then returns `Excluded` and
`attach` fails with `NeverKeep`. `false` (by path) lifts every tombstone that shares a hash with the file. A path
that resolves `Ambiguous` fails with `HistoryError::Ambiguous(candidates)` and writes nothing: the UI asks which
history, then calls again with `Target::Doc`.

## Identity (`doc_for_file`)

1. Tombstone match -> `Excluded`.
2. NTFS file id + volume serial (`GetFileInformationByHandleEx(FileIdInfo)`, fallback `GetFileInformationByHandle`)
   -> `Known`. Survives rename and move within a volume, and in-place overwrites. Only used when the volume is NTFS
   or ReFS (`GetVolumeInformationByHandleW`) and the id is not 0: FAT/exFAT, SMB shares and cloud drives can return
   0 or reuse ids, so there no file id is recorded (nor hashed into tombstones) and lookup goes on to step 3.
3. Otherwise read the file: content hash equal to the head of exactly one doc -> `Known` (a copy, or a move across
   volumes). Several -> `Ambiguous`.
4. Else the trailer /ID (first element, parsed from the last trailer or xref-stream dict, no MuPDF): one doc with
   that /ID **and** the same path hash -> `Known` (an app rewrote the file in place: new file id, same /ID). Any
   other /ID match -> `Ambiguous { candidates }` ("same document?": `confirm_same` or `attach`).
5. Else `New`.
On a `Known` found by content or /ID, the doc's identity is refreshed (file id, path) so the next lookup is the fast
path.

## Crash safety

Commit order (under the doc's mutex): WAL (version id + chunk ids, atomic write) -> missing chunks (temp in the same
directory, fsync, rename) -> manifest (temp, fsync, rename) -> directory sync (best effort: NTFS journals renames;
the flush is attempted with `FILE_FLAG_BACKUP_SEMANTICS`) -> delete WAL -> refresh identity.

fsck runs on each doc's first use (and on all docs with `Store::fsck()`): delete `*.tmp-*` files; a WAL whose
manifest exists, decrypts and whose chunks all verify is kept, otherwise its manifest is removed (its chunks are left
to GC); manifests that don't decrypt are renamed to `*.bad` (reported, never deleted, and the doc is excluded from
prune and GC); manifests with missing chunks are listed with `broken: true`, and `read` fails on them. An unreadable
`names` record also marks the doc damaged (so named versions can't be mistaken for prunable ones).

The per-doc state is cached after fsck. Any operation on a doc that fails drops that cache, so the next use reloads
from disk (a commit whose manifest landed but whose WAL delete failed is then listed, not lost). Prune and GC reload
each doc from its manifests on disk before planning or sweeping, never trusting the cache. Store-wide passes
(`fsck`, `prune`, `gc`, `usage`) skip a doc that can't be opened (e.g. its key no longer unwraps under DPAPI) and list
it in `skipped`, instead of failing for every doc; nothing of a skipped doc is deleted.

Concurrency: one `Store` per process (every method takes `&self`); a `parking_lot::Mutex` per doc; a `RwLock` over
the root so `relocate` waits for in-flight operations; `<root>/lock` opened with share mode 0, so a second process gets
`HistoryError::Locked` ("the history store is in use by another Inkwell window").

GC safety: the brief suggested "only chunks older than the oldest in-progress WAL". Changed: GC sweeps a doc while
holding that doc's mutex, so no commit of that doc can be in flight, and it skips any doc that still has a WAL (a crash
not yet recovered) or a damaged manifest. Simpler and strictly safer than an mtime cut-off.

Kill test: the test binary re-runs itself as a child that commits (and names and autosaves) in a loop, and is killed
at a random point 50 times; after each kill the store is opened, fsck'd, and every version is read back and compared
byte for byte with the bytes the child committed; no temp file may remain.

## Sessions and crash recovery

`begin_session(doc) -> SessionGuard` writes `session`; dropping the guard (or `end()`) removes it. `Store::open` holds
the lock, so every marker found at open belongs to a dead process: those docs are "stale". `pending_recovery(doc)` =
stale **and** the newest Autosave is newer than the newest Save / External / Import (versions that match the file on
disk). Call it before `begin_session`, which clears the flag.

## Retention

`Policy { all_days: 30, daily_months: 6, per_doc_cap_bytes: 2 GiB, total_cap_bytes: 20 GiB }` in store.json
(`Store::policy` / `set_policy`). `plan_retention(&[VersionInfo], &Policy, now, cost)` is pure: every version for
`all_days`; then the newest per UTC day for `daily_months` x 30 days; then the newest per calendar month; the head
and named versions always. Over `per_doc_cap_bytes` (cost = on-disk bytes of the chunks the kept set references,
shared chunks once), the oldest unnamed non-head versions go first. `Store::prune()` applies it per doc, then the
total cap across docs (oldest unnamed first, globally), then `gc()`. A pruned version's children keep their `parents`
id (dangling by design: the timeline shows a gap, not a lie).

## Benchmark and the chunking choice

Bytes stored: first version (KiB) / mean per later version (KiB). Logical = mean file size.

| sequence | logical KiB | CDC 16 KiB | CDC 64 KiB | object-level | whole-file zstd |
|---|---|---|---|---|---|
| fixture 200p: incremental x10 | 824.6 | 34.6 / 2.5 | 18.3 / 1.5 | 137.2 / 16.4 | 13.3 / 14.1 |
| fixture 200p: full rewrite x10 | 821.0 | 34.6 / 8.5 | 18.3 / 10.0 | 137.2 / 29.2 | 13.3 / 12.7 |
| fixture 200p: rewrite garbage+compress x10 | 190.1 | 34.6 / 15.5 | 18.3 / 26.3 | 137.2 / 30.3 | 13.3 / 26.1 |
| compress-source (images): incremental x10 | 2599.1 | 2605.3 / 21.2 | 2598.1 / 86.9 | 2596.1 / 2.9 | 2593.7 / 2594.6 |
| compress-source (images): full rewrite x10 | 2597.3 | 2605.3 / 37.5 | 2598.1 / 147.7 | 2596.1 / 262.0 | 2593.7 / 2594.2 |
| compress-source (images): rewrite garbage+compress x10 | 2597.3 | 2605.3 / 37.5 | 2598.1 / 147.1 | 2596.1 / 262.1 | 2593.7 / 2594.2 |
| Acrobat edit: edit-source -> edit-out-* | 67.4 | 54.5 / 43.1 | 54.2 / 40.3 | 58.2 / 38.9 | 52.9 / 59.8 |
| Acrobat redact: redact-source -> red-out-* | 53.5 | 49.5 / 51.8 | 49.2 / 50.4 | 56.3 / 54.8 | 48.8 / 50.0 |
| Acrobat optimize: opt-in-01 -> opt-out-* | 1601.7 | 2605.3 / 854.0 | 2598.1 / 902.6 | 2596.1 / 1062.3 | 2593.7 / 1508.6 |
| Acrobat form: form-source -> frm-out-* | 33.3 | 28.5 / 29.8 | 28.5 / 29.0 | 34.4 / 33.4 | 28.1 / 28.5 |
| **total stored, all sequences (KiB)** | | 21549.0 | 24962.9 | 28934.9 | 105795.9 |
| split+hash+compress, ms per MB | | 0.9 | 0.8 | 1.2 | 1.0 |

Real store commit (default chunking, XChaCha20-Poly1305, temp+fsync+rename):

| sequence | stored after first (KiB/version) | commit ms per MB |
|---|---|---|
| fixture 200p: incremental x10 | 2.4 | 11.0 |
| fixture 200p: full rewrite x10 | 8.4 | 18.5 |
| fixture 200p: rewrite garbage+compress x10 | 15.4 | 94.0 |
| compress-source (images): incremental x10 | 21.1 | 5.1 |
| compress-source (images): full rewrite x10 | 37.4 | 5.2 |
| compress-source (images): rewrite garbage+compress x10 | 37.4 | 5.5 |

Measured 2026-10-10 with `cargo run --release --bin history_bench` (Ryzen 7 7800X3D, NVMe). Choice: **FastCDC, avg
16 KiB (min 4, max 64)**: the smallest total over all sequences (21.5 MB vs 25.0 MB for 64 KiB, 28.9 MB for
object-level dedup, 105.8 MB for whole-file zstd). Object-level dedup only wins on incremental saves of an image-heavy
file (2.9 vs 21.2 KiB/version) and loses on every full rewrite, so it was not kept. Acrobat's own saves rewrite and
re-compress most streams: those versions cost close to their full compressed size whatever the method (edit, redact,
form rows); that is the floor for per-document encrypted storage. Commit cost is 5-20 ms per MB (94 ms/MB on the
re-compressed fixture, where zstd works hardest).

## Integration points (for edit-core and the panel)

- Open: `doc_for_file(path)`: `Known` -> `pending_recovery(doc)` (show "Recover unsaved changes" if Some) ->
  `check_external(doc, path)` -> `begin_session(doc)`, keep the guard with the open document. `New` -> `attach(path)`.
  `Ambiguous` -> ask, then `confirm_same` or `attach`. `Excluded` -> no history.
- Window focus: `check_external(doc, path)`.
- Save (engine rule 5, after the rename): `commit(doc, bytes, NewVersion { author: User, kind: Save, label, summary,
  pages_touched })`; the bytes are what was written.
- edit-core checkpoint after an assistant turn that changed something: `commit` with `kind: Checkpoint`,
  `author: Assistant { turn_id }`, label = the turn's label, summary = the entries' labels, pages = union of pages
  touched. A labelled checkpoint is recorded even when bytes equal the head.
- Autosave timer (every N minutes of activity, only when dirty): `autosave(doc, snapshot_bytes)` from the in-memory
  snapshot (engine rule 2).
- Redaction apply: write the redacted file, then `redactions_applied(doc, keep, redacted_bytes, meta)` **instead
  of** the Save commit (it records the redacted bytes itself).
- Wired (history_ipc.rs "edit-core hooks"): `save_document` -> `History::saved` (Save as rebinds the document
  to its new file instead); the UI calls `history_checkpoint(doc)` when an assistant turn ends (done, error or
  cancelled) and Rust checks the document ends with that turn's steps (`Engine::assistant_turn`, once per turn);
  `spawn_autosave` every 2 minutes for documents whose unsaved content changed; the engine's redaction hook marks the
  document and the next save that includes the redaction goes through `redactions_applied` (keep = `keeps_pre_redaction`).
- Never keep (by path): on `Err(Ambiguous(candidates))` ask which history, then `set_never_keep(Target::Doc(d), true)`.
- Close: drop the `SessionGuard`.
- Settings: `usage()`, `policy()` / `set_policy()`, `prune()` (`Maintenance { pruned, freed_bytes, skipped }`; show
  skipped docs as unreadable), `relocate(new_root) -> leftovers` (persist the new root in app settings after it
  returns Ok; the old keys are destroyed first, then the old tree removed; non-empty leftovers are old files that
  could not be deleted, e.g. held open, worth telling the user. An old root holds `moved-to.txt`, and `Store::open`
  on it returns `Moved(path)`).
