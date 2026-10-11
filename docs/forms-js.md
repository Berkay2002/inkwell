# Form JavaScript

AcroForm fields carry JavaScript: keystroke (`/AA /K`), format (`/F`), validate (`/V`), calculate (`/C`), and
document-level scripts (`/Names /JavaScript`). Inkwell runs them in a sandboxed engine (`src-tauri/src/forms/js.rs`,
`js/`, glue in `forms/scripts.rs`). The calls Acrobat writes by itself still take the native code in `af.rs`.

## Why rquickjs (QuickJS-ng)
The engine must be stoppable and bounded. Boa has no interrupt and no heap cap, so a `while(true){}` in a downloaded form
would hang a thread for ever and `"x".repeat(1<<28)` could take the process down. rquickjs has
`set_interrupt_handler` (our deadline), `set_memory_limit`, `set_max_stack_size` and `set_gc_threshold`. The `rust-alloc`,
`allocator` and `parallel` features stay off. On Windows MuPDF's bundled MuJS exports `js_malloc` and friends, which collide
with QuickJS's, so `vendor/mupdf-sys/build.rs` gives those four names a `mujs_` prefix on MSVC.

## Where scripts run
- One runtime per open document, on its own 8 MiB thread (`forms-js`), reached through a channel with a reply timeout. A panic
  is caught and the runtime rebuilt. The handle lives in the open document's entry and is dropped on close.
- Globals persist between events. Document-level scripts run once, in name order, when the runtime starts (the first script
  that needs it), and a failing one is logged and skipped.
- Field scripts are wrapped in a block: `var` and functions reach the global object (as in Acrobat), `let`/`const` stay in
  the script.
- Lock order is edit lane, then the document's scripts.

## Limits
| Limit | Value |
|---|---|
| K / F / V event | 250 ms |
| One C script | 1 s |
| Calculate cascade | 2 s |
| Document JS at open | 2 s |
| Stack | 1 MiB |
| Heap | 64 MiB |
| GC threshold | 8 MiB |
| One script | 256 KiB |
| Document JS together | 1 MiB |
| Strings crossing to the host | cut at 64 KiB |
| Script log | 256 lines |
| Calculate passes | 8 |

On a timeout the event result is unchanged (`rc` true, old value), a line goes in the log, and that field's scripts are off
for the session. After three timeouts every script of the document is off (the recognised calls still run natively). If the
worker does not answer within the deadline plus 2 s it is abandoned and scripts are off until the person turns them back
on, which starts a fresh runtime. A heap, stack or syntax error is an exception: logged, event unchanged.

## The fast path
A script that is exactly one `AF...` call (comments and a `;` allowed) never touches the engine: `af::parse_format`,
`parse_keystroke`, `parse_validate` and `parse_calculate` use `sole_call`. The engine's own `AF...` functions call the same
`af.rs` code, so both give the same output (test: `the_fast_path_and_the_script_engine_agree_...`). There is no fast path
when a document-level script defines or assigns an `AF\w+` function: that document gets its own.

## What runs
With scripts on (the default, per open file, in memory): `/K` on commit, `/V` (`event.rc = false` rejects, with the
`app.alert` text or a default message), `/C` in `/CO` order and again while a pass changed something (at most 8), `/F`
for what the field shows (`FieldDto.shown` and the appearance). Writes a script makes to other fields (`value`,
`readonly`, `required`, `display`, colours) are applied in the same edit step, so the fill is one undo step. A K or V
script does not re-run itself for those writes, as in Acrobat. `app.alert` comes back to the UI; `launchURL` (http, https,
mailto only) and `submitForm` come back as requests and are never acted on by the engine.

## Scripts on or off
Following Acrobat, scripts run, with a banner and a per-file toggle: `forms_scripts(doc)` gives
`{hasScripts, enabled, log:[{level, field?, message}]}` and `forms_scripts_enable(doc, on)` switches the file. Off means
today's behaviour: an unrecognised script is kept untouched in the file and not run, and the field is marked.

## Object model
`event` (value, change, willCommit, rc, name, type, target, targetName, selStart, selEnd, changeEx), the document (`getField`,
`numFields`, `getNthFieldName`, `numPages`, `pageNum`, `info`, `resetForm`, `calculateNow`, `print`, `submitForm`), field
objects (value, valueAsString, defaultValue, type, name, readonly, required, display, numItems, charLimit, exportValues,
textColor/fillColor/strokeColor, `getItemAt`, `isBoxChecked`, `checkThisBox` ...), `app`, `util` (printf, printd, printx,
scand), `console`, `display`, `color`, and the AF functions (`AFNumber_*`, `AFPercent_*`, `AFDate_*`, `AFTime_*`,
`AFSpecial_*`, `AFRange_Validate`, `AFSimple_Calculate`, `AFMakeNumber`, `AFMergeChange`). A keystroke while typing
(`willCommit` false) checks the partial entry; on commit the full rule applies.

## Not supported
Anything else returns `undefined` and the log says "<name> is not supported": timers (`app.setTimeOut`/`setInterval` hand
back a handle and never fire), `app.execMenuItem`, mail, `Net`, `SOAP`, `importDataObject`, page content scripts, `global`
persistence across sessions, and the like. Also not run: button mouse-up scripts, `OpenAction`/page/document-event
scripts, and the K/V scripts of check boxes and list boxes (only text and combo fields have a keystroke and validate step).
Authoring (`forms_edit`, `forms_add`) redraws a field with the native formatter only; the `/F` script of a field being
authored is shown after the next fill or listing.
