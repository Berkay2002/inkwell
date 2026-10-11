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

## In the window
- **Banner.** A file with scripts (document-level, or a field script that is not exactly one Acrobat call) shows "This form
  runs scripts." over the page with a **Run scripts** switch. The choice is per open file and in memory (default on, as in
  Acrobat). Off: the banner stays, as "Scripts are off for this file.", so it can be turned back on. While on, the banner can be
  dismissed for the session (the switch is also in the Forms rail).
- **Scripts note** in the Forms rail: the switch, and the script log (warnings and errors, with the field), collapsed, with a
  count badge. Read again after every edit.
- **`app.alert`** is an in-app dialog (title "Inkwell" unless the script gives one), one at a time, in order.
- **Requests** (`Stepped.requests`): `reset` runs the existing reset path (a reset that a script asked for does not act on the
  requests of that reset's own scripts, so a script cannot loop); `goToPage` goes to the page; `focus` goes to the field's page
  and focuses it; `print` opens the Print sheet; `beep` does nothing. `launchUrl` opens a confirm dialog with the address
  (http, https and mailto only, no credentials) and **Copy link** / Cancel: Inkwell has no browser opener and never opens one.
  `submit` is a note, "This form wants to send its data to <url>. Inkwell never submits forms." Nothing acts on a link or a
  submit by itself.

## Preparing form scripts
The rail's field properties follow Acrobat's tabs.
- **Format**: None, Number, Percentage, Date, Time, Special, Pattern, **Custom script** (a format script, `/AA /F`, and a
  keystroke script, `/AA /K`; either may be empty). Next to the pickers a **live sample** shows "1234.5 -> $1,234.50" (command
  `forms_format_preview(format, sample)` over `af::format_value`; a custom script is not run for it).
- **Validate**: Not validated, Number in a range, **Custom validation script** (`/AA /V`).
- **Calculate**: Not calculated, Sum / Product / Average / Smallest / Largest of fields, **Simplified field notation**,
  **Custom calculation script** (`/AA /C`; a calculated field is also put in `/CO`). **Calculation order...** (when two or more
  fields calculate) lists them in `/CO` order with up and down; OK is one undo step (`forms_calc_order(doc, nodes)`, which
  writes `/AcroForm /CO`; calculated fields left out follow the listed ones; the same order is not a step).
- Reading a field back: the editor shows the Acrobat setting when the scripts are exactly its calls, otherwise Custom with the
  scripts as found (`af::format_for_editor`, `validate_for_editor`, `calculate_for_editor`). A format call next to a keystroke
  script of the author's own reads as Custom with both. Choosing Custom with empty boxes writes nothing (it clears the old
  setting); emptying every box removes the scripts.
- Each change is one `forms_edit` step; the Custom boxes commit when they lose focus.
- `FormDto.calcOrder` lists the `node` of each calculated field in running order.

## Checked at runtime
`scripts/formsjs-shots.mjs` drives the debug app over CDP on a copy of `docs/study/samples/_gen/formsjs-sample.pdf` (written by
`cargo test --lib write_the_form_javascript_sample -- --ignored`) and saves the screens, light and dark, in
`docs/screens/forms-js/`.
