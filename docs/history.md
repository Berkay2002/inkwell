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

`redactions_applied(doc, keep_history)`:
- `false` (default): shred everything (key first), create a fresh key, keep the doc's identity (minus the head
  content hash, which is a hash of unredacted bytes); the caller then commits the redacted file as the first version.
- `true`: keep it all and set `keep_pre_redaction` in identity.json (the UI shows "pre-redaction history kept";
  `Store::keeps_pre_redaction`).

`set_never_keep(Target::Doc | Target::Path, true)`: records a tombstone (index-keyed hashes of path, file id and
/ID; a file matching any of them is excluded), then shreds the doc. `doc_for_file` then returns `Excluded` and
`attach` fails with `NeverKeep`. `false` (by path) lifts every tombstone that shares a hash with the file.

## Identity (`doc_for_file`)

1. Tombstone match -> `Excluded`.
2. NTFS file id + volume serial (`GetFileInformationByHandleEx(FileIdInfo)`, fallback `GetFileInformationByHandle`)
   -> `Known`. Survives rename and move within a volume, and in-place overwrites.
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

BENCH

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
- Redaction apply: `redactions_applied(doc, keep)` **before** committing the redacted file.
- Close: drop the `SessionGuard`.
- Settings: `usage()`, `policy()` / `set_policy()`, `prune()`, `relocate(new_root)` (persist the new root in app
  settings after it returns Ok; an old root holds `moved-to.txt`, and `Store::open` on it returns `Moved(path)`).
