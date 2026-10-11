# Batch actions (Action Wizard)

Code: `src-tauri/src/batch/` (`action.rs` the Action and its steps, `steps.rs` the step trait and one adapter per
step, `run.rs` the runner, report and events, `store.rs` files on disk and the built-ins, `app.rs` the Tauri
commands, `tests.rs`). Engine only: there is no UI yet. Parity row: Acrobat Pro "Action Wizard".

## The Action
A name, a description, ordered steps, an output rule and "on error". JSON, camelCase, every field defaulted.

- **Output rule**: `nextToSource {suffix}` writes `<name><suffix>.pdf` beside the source; `folder {suffix}` writes into a
  folder picked when the run starts (a `pick_folder` token goes to `batch_run`; a token is good for one use, so the
  Action cannot store it). A result never replaces a file: if the name is taken it becomes `name (2).pdf`, `(3)`...
  `nextToSource` needs a non-empty suffix.
- **On error**: `skip` reports the file and goes on; `stop` leaves the files after it untouched (they are reported as
  skipped). A skipped file (see below) is not an error and never stops a run.
- **Steps** (`kind`): `compress {preset}`, `ocr {languages, force, backend}`, `headerFooter` (the marks settings),
  `watermark` (text only), `bates {bates, continueAcrossFiles}`, `protect`, `sanitize` (the redact sanitize switches),
  `redact {queries, overlayText, sanitize}`, `flattenForms`, `exportImages {format, dpi, gray, quality}`,
  `exportText {layout}`, `setProperties {title, author, subject, keywords, creator, producer}`.
  There is no remove-security step. A file that names one, or any kind this build does not know, still loads (as an
  invalid step) so validation can say what is wrong; it cannot be saved or run.

Each step reuses an engine as it is: `compress::compress`, `ocr::ocr_file_with`, `marks::add` (always as an update, so
running an action over its own output does not stamp twice), `redact::find/mark_hits/apply` (the verified bytes are what
is written), `redact::sanitize`, `protect::write_options`, MuPDF's `bake` for form fields, `convert::export_document`.
Steps are chained on staged files: each reads the file the one before wrote. A step that finds nothing to do (no
form fields, nothing to redact, no page needing OCR, a file that cannot shrink) leaves the file as it is and says so
in the report's notes. Exports read the file as the steps before them left it.

## Validation
`batch_validate(action)` returns issues `{step, code, message}`: `name`, `no-steps`, `output`, `order`, `unknown-step`,
`forbidden-step`, `invalid-step`, `unavailable`. `batch_run` validates first and refuses to start with the same list, so
nothing fails halfway for a reason known up front. Header/footer, watermark and Bates settings are checked by trying
them on a one-page scratch document (the marks engine's own messages). OCR is `unavailable` when its engine is not on
the machine (Tesseract not found, Windows OCR off Windows). Rules: Protect is the last step and used once (what follows
could not read the file); one header/footer, one watermark, one Bates step.

## What a run does to a file
The source is only ever read. In order, a file is **skipped with a reason** when:
- it is open in a tab (its unsaved edits are not on disk);
- it needs a password (a batch has none for sources);
- its owner restricted copying and page changes (a batch cannot lift that, and exports would write it out);
- it has an owner password a batch does not know and the action would replace or ignore what that password guards:
  a Protect step is refused for it, and so is any writing step when the owner denied changes (`modify`). Other steps
  keep the file's encryption and rights in the result (tested for every writing step);
- it is signed (`/SigFlags` bit 1, a certification `/Perms /DocMDP`, or a signature field with a value) and the action
  has a step that rewrites the file (everything but the two exports).
  The check is on the whole action before any step runs, so no half-processed copy exists;
- the same file was listed twice, the run was cancelled, or an earlier file stopped the run.
It is an **error** when the file is not a PDF or a step fails (the reason names the step). Nothing of an errored or
cancelled file is left: steps write into a hidden staging folder in the destination folder (`Staging` from convert), the
final names are claimed with `create_new` and the staged files renamed over the claims, and the staging folder is
dropped on any failure. A cancel between steps or inside compress/OCR discards that file's work; files finished before it
are whole and stay. The publish step itself is not cancellable (it is a few renames).
Redact with no hit on the pages still applies its sanitize switches (a value can sit in the properties, a form field or a
comment), and a file with pages that have no text layer gets a `warnings` entry in its report: a scan has to go through
"Make searchable" first. OCR languages are checked at validation: each tag must have the shape `en`, `en-US` and be
installed.
Exports go next to the result: `<name><suffix>.txt` and a folder `<name><suffix> (images)` holding `<name>-1.png`...; an
action with only exports makes no PDF. Bates numbers continue from file to file (a file that fails does not use numbers)
unless `continueAcrossFiles` is off. Header/footer dates are today in UTC.

## Passwords
A Protect step's passwords are never serialised: not in the store, not in an exported file, not in `Debug` output, and
they are zeroed on drop. A stored Protect step therefore fails validation ("type the password") until the person types it
into the Action they pass to `batch_run`. Output of Protect is AES; sources are never decrypted.

## Storage and files
`<app data>/batch-actions/<id>.json` (`INKWELL_DATA_DIR` applies), one file per Action, written through a temp file
(random name, `create_new`) and a rename. Id: `[a-z0-9-]`, at most 64 characters, so no path can come out of it. The file and the export format is the
same envelope: `{"format": "inkwell-batch-action", "version": 1, "action": {...}}`. A newer `version` is refused
(`TooNew`), a missing or zero one is invalid, an older one goes through `migrate` (nothing to migrate yet). Import reads an
open token, always stores under a new id (never replaces) and renames a duplicate name "(imported)"; export writes to a
`pick_save` token. The four built-ins live in code (read-only; save/delete on them is refused): *Prepare for email*
(sanitize metadata and scripts, compress balanced), *Make searchable* (OCR), *Publish with watermark* (flatten forms,
CONFIDENTIAL watermark, compress high quality), *Redact emails and phone numbers* (patterns email and phone, sanitize
metadata/layers/scripts/attachments only so comments and bookmarks stay).

## Commands
`batch_list`, `batch_save(action)`, `batch_delete(id)`, `batch_import(file token)`, `batch_export(action, saveTo token)`,
`batch_validate(action)`, `batch_run(job, action, files[], folder?, onEvent)`, `batch_cancel(job)`. Files are open tokens
and the result files come back as open tokens (`outputs[].token`), never as paths to use. Events (`event` tag):
`started`, `fileStarted`, `step`, `fileDone {report}`, `finished`. The report has, per file, `status` (`ok`, `skipped`,
`error`), `reason`, `sizeBefore`, `sizeAfter`, `outputs`, `notes`, `stepsDone`, `ms`, and the run's counts, `cancelled`
and `stopped`. `batch_cancel` uses the shared job ids (`tools::Jobs`).

## After a crash
Each run adds `<folder>\t<ending>` to `batch-actions/run-dirs.txt` (under a lock file; the sweep takes the same lock), and
every name claimed for a result is written into the file's staging folder (`claims.txt`: `+path` when claimed, `=path`
once it was moved into place). At startup `batch::app::sweep_at_startup` removes staging folders (`.inkwell-convert-<pid>-<n>-<hex>`)
older than a day in the journaled folders, and the claims that are still empty. A claims list is only trusted for
claims that are absolute, directly inside the journaled folder, and named after a PDF in that folder plus a journaled
ending (`<name><ending>.pdf`, `.txt`, ` (images)`, with an optional ` (n)`); published claims and everything else are left
alone. `batch_run` refuses a `job` id that is already running (`Jobs::try_begin`).

## Gaps
- No UI, no scheduling, no watch folders.
- Watermark pictures are refused (their file token does not outlast a session). No page ranges on stamps; they go on all pages.
- Set properties writes the Info dictionary only; an existing XMP packet keeps its old values.
- A password-protected source is skipped; there is no per-file password.
- Steps do not run in parallel across files, and only compress/OCR/export stop in the middle of a step on cancel.
- Redaction: marks already in a source (placed by hand) are applied too. Glyph boxes are taller than the font size, so a
  mark on a line also takes the glyphs of a line closer than about 1.4 em with it (an engine behaviour; it errs towards
  removing too much, and `redact::apply` still verifies the result).
- Tests run on Linux through a scratch crate that includes the engine modules without `history`/`print`/`ai` (the crate
  as a whole only builds on Windows); the OCR tests use a stub recogniser.
