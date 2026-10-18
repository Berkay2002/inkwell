# Batch actions (Action Wizard)

Code: `src-tauri/src/batch/` (`action.rs` the Action and its steps, `steps.rs` the step trait and one adapter per
step, `run.rs` the runner, report and events, `store.rs` files on disk and the built-ins, `app.rs` the Tauri
commands, `tests.rs`); the UI is `src/components/ActionWizard.tsx` (see "In the app"). Parity row: Acrobat Pro "Action Wizard".

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
- **Per step, beside its own options** (flat in the step's JSON, left out when default): `pages` and `prompt`, below.

### Page range per step
`pages` is the range syntax of OCR and Convert (`1-3, 5, 8-`, one-based, `,` or `;`, at most 200 characters); empty or
absent means every page. Only the steps that work page by page take one: `headerFooter`, `watermark`, `bates`, `redact`,
`ocr`, `flattenForms` (`Step::takes_pages`). A range on any other step is an `invalid-step` issue ("This step works on
the whole file: clear its page range."). Compress, Protect, Sanitize and Set properties act on the whole file; the two exports have no range (not done, see Gaps).
- A range reaching past a file's last page is **clipped to the pages that exist**, and the report says so: a note
  "the page range 3-9 reaches past the last page (5); used page(s) 3-5". A range wholly past the end does nothing to that
  file: a warning "<step> (pages 8-9): the page range 8-9 is past the last page (the file has 5), so nothing was done."
  (nothing at all: Redact's sanitize switches are not applied either). Neither fails the file: a batch holds files of different lengths.
- Header/footer `<<1>>` counts from the first page of the range (the marks engine's own numbering), Bates numbers go on
  the pages of the range in order, flatten forms leaves the fields of the other pages as fields, redact searches only the
  range, OCR reads only the pages of the range that have no text (all of them with `force`).
- **Redact with a range** (`redact::apply` takes every pending mark of the document, so the range is guarded):
  - A file that already has a redaction mark on a page outside the range is **skipped**, not touched: "redaction marks
    already on page(s) 7, outside the range 1-3". The mark is left where it is; apply it first, or include its page.
    Marks inside the range are applied with the hits, as without a range.
  - The sanitize switches that work page by page (comments and markups, hidden text, links) act on the pages of the
    range only; a link or button on another page keeps its action. The file-level ones (document information, attached
    files, bookmarks, and the script part of "scripts and actions": JavaScript code, additional actions (`/AA`),
    OpenAction, XFA) still act on the whole file, and anything that quotes a redacted string is removed wherever it is. Hidden layers cannot be limited
    to pages (the layer list is one for the file, and dropping it would show the hidden content of the pages left out):
    a Redact step with a range and "Hidden layers" on is an `invalid-step` issue; untick it or clear the range.
- Header/footer, watermark and Bates on a range replace the step's own earlier marks on those pages only; the marks of
  an earlier run on the other pages stay.
- Preview does not open the files for their page counts, so it cannot show clipping.

### Prompt user per step
`prompt: true` makes the run stop before that step **for each file** and show the step's own options sheet, prefilled
with the saved ones (and its page range). The answer applies to that file only: the saved action is not changed.
- **Continue** runs the step with the options as shown, **Skip this step** leaves it out for this file (the file ends as
  it would without the step; no Bates numbers are used), **Stop** cancels the run (the report says `cancelled`).
- Nothing waits for ever: the run polls for Cancel while it waits, and gives up after 30 minutes (`PROMPT_WAIT`), which
  stops the run like Stop. A closed window or a dropped answer channel does the same.
- Every answer is checked before it runs: it must be the same kind of step (a prompt cannot turn Compress into
  Protect), the page-range rules above apply, and the step's own `validate` runs. An answer that fails is not run: the
  prompt comes again with the reason (`problem`) and a new request id. Passwords never travel in a prompt: a Protect
  step keeps the run's own, and the event carries none.
- Each step keeps its own refusal paths: the file gate (signed, certified, restricted, a password) runs on the whole
  action before step one, so prompting cannot reach a file the steps would refuse; a step's per-run checks run after the
  answer.
- Flow: the engine sends the `prompt` event `{request, index, name, step, steps, label, entry, problem}` and waits on
  that `request`; the UI answers with `batch_prompt_answer(job, request, answer)`, `{kind: "continue", step}`,
  `{kind: "skip"}` or `{kind: "stop"}`. The command returns `false` (and does nothing) for a request that is not the
  open one: a second press, an answer after the run moved on, a stale sheet. The UI drops those too.
- A run with a prompting step and no way to ask (`env.ask` unset) is refused at the start (`BatchError::Input`), so a
  prompt never fails open. Preview works, and lists for each file the steps that will ask (`prompts`, positions in
  `steps`).

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
same envelope: `{"format": "inkwell-batch-action", "version": 2, "action": {...}}`. Version 2 is written only when a
step has a page range or `prompt` (otherwise 1, so a build before this change still opens plain actions); a file without
those fields imports as it always did. A newer `version` than the build's is refused (`TooNew`), a missing or zero one is
invalid, an older one goes through `migrate` (nothing to migrate: the new fields are optional). A `pages` or `prompt` of the
wrong type makes the step invalid rather than silently dropping it. Import reads an
open token, always stores under a new id (never replaces) and renames a duplicate name "(imported)"; export writes to a
`pick_save` token. The four built-ins live in code (read-only; save/delete on them is refused): *Prepare for email*
(sanitize metadata and scripts, compress balanced), *Make searchable* (OCR), *Publish with watermark* (flatten forms,
CONFIDENTIAL watermark, compress high quality), *Redact emails and phone numbers* (patterns email and phone, sanitize
metadata/layers/scripts/attachments only so comments and bookmarks stay).

## Commands
`batch_list`, `batch_save(action)`, `batch_delete(id)`, `batch_import(file token)`, `batch_export(action, saveTo token)`,
`batch_validate(action)`, `batch_preview(action, files[])`, `batch_folder_files(folder, recursive)`,
`batch_run(job, action, files[], folder?, onEvent)`, `batch_cancel(job)`, `batch_prompt_answer(job, request, answer)`. Files are open tokens
and the result files come back as open tokens (`outputs[].token`), never as paths to use. Events (`event` tag):
`started`, `fileStarted`, `step`, `prompt`, `fileDone {report}`, `finished`. The report has, per file, `status` (`ok`, `skipped`,
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

## In the app
Tools > Action Wizard, or "Action Wizard" in the command palette. One centred sheet with three views.

- **Actions**: the saved actions, then the four built-ins, as cards with their steps listed. Run, Edit (saved ones; a
  built-in is read-only: Duplicate it), Duplicate, Export, Delete (asks twice), Import. Import goes through the engine
  (`batch_import`), so a file that names an unknown or forbidden step ("remove security") is refused with the engine's own
  sentence, shown above the list. An imported action opens in the editor for a look before anything runs.
- **Editor**: the step library on the left in Acrobat's groups (Protect, Document processing, Pages, Export, Optimize),
  the sequence in the middle, the selected step's options on the right. Click a step to add it; drag the grip or press
  Up/Down on it to reorder. Name, description, where results go, and what a failing file does sit under the columns.
  The draft is validated by the engine a moment after each change and the issue is shown on its step. Save needs a name,
  one step and no step the engine could not read; other issues still let a draft be kept, they only stop it running.
- **Run**: files (Add files, or Add folder with an "include sub-folders" box: the engine lists the PDFs, at most 500),
  what it does, **Preview**, Run. A folder result is chosen when Run is pressed, not before, so its token is spent only
  on a run that starts. Progress is per file with the step it is on; Cancel stops between files and inside compress/OCR.
  The summary lists done (size before and after, notes, warnings), skipped (with the engine's reason, for example
  "its owner restricted copying") and failed, with "Show in folder" for a result (the existing `reveal_in_folder`).
- **Preview** (`batch_preview`) is a dry run. It runs the same checks as a run (the action's validation, repeated files and
  the per-file gate: open in a tab, password, restricted, signed) through one shared function, and lists per file whether it
  would run or be skipped and why, and the file names it would write (the first free ones today). It writes nothing: no
  staging folder, no journal. It cannot know that a step will find nothing to do, that a file will fail partway, or that an
  error stops the run; the summary says so. It does not take the output folder (chosen at Run), so names are only kept
  apart from each other there.
- **Passwords** of a Protect step are typed in the Run view, held in memory, merged into the one `batch_run` call, and cleared
  when the run ends. The editor never holds one, `batch_save` and `batch_export` are given the action with any password
  removed, and the engine drops them anyway. A Protect step with no password only waits (the editor shows "Password is typed
  when it runs"); Preview uses stand-in passwords when none are typed, since it writes nothing.
- **Page range and "Ask before this step, for each file"** are fields under each step's options in the editor (Pages only
  on the steps that take one). The sequence shows "(pages 1-3)" and "Asks before it runs, for each file" on the row; the
  Run view's "What it does" list and each file's preview line ("asks before ...") show them too.
- **Prompt sheet**: during a run, a dialog over the progress ("Step 2 of 4: Watermark", the file's name) with the step's
  options and Pages field, the engine's reason when an answer was refused, and Continue / Skip this step / Stop. It is a
  modal dialog over the whole sheet: Escape does nothing, so a stray key never stops the run, and Stop in the dialog is how
  a run is cancelled while it waits (the footer's Cancel is behind it).

Screens (light and dark) are taken by `scripts/batch-shots.mjs` in headless Chrome against a mocked
invoke layer, so they show the interface, not the engine. The runtime pass against the real exe is still to do on Windows.

## Gaps
- No scheduling, no watch folders. The two exports have no page range (an export of some pages is Convert's job); Compress,
  Protect, Sanitize and Set properties never take one.
- Watermark pictures are refused (their file token does not outlast a session).
- A prompt answer changes one file's step options; there is no "apply to all remaining files" button.
- Set properties writes the Info dictionary only; an existing XMP packet keeps its old values.
- A password-protected source is skipped; there is no per-file password.
- Steps do not run in parallel across files, and only compress/OCR/export stop in the middle of a step on cancel.
- Redaction: marks already in a source (placed by hand) are applied too (with a page range, a file with marks outside
  the range is skipped instead). Glyph boxes are taller than the font size, so a
  mark on a line also takes the glyphs of a line closer than about 1.4 em with it (an engine behaviour; it errs towards
  removing too much, and `redact::apply` still verifies the result).
- Tests run on Linux through a scratch crate that includes the engine modules without `history`/`print`/`ai` (the crate
  as a whole only builds on Windows); the OCR tests use a stub recogniser.
