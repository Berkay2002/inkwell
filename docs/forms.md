# Forms: fill, then author

Engine: `src-tauri/src/forms/` (`mod.rs` fields, filling, reset; `author.rs` create/move/edit/delete/tab order/detect;
`af.rs` the script subset; `detect.rs` auto-detection; `app.rs` the Tauri commands). Every change is one edit-core step
(one undo step, kept by Save, recorded in history). UI: Forms rail (Ctrl+Alt+F), `FormLayer` over each page, field
tool strip while preparing a form.

## Filling
- Every AcroForm field type: text (single and multi-line, comb, password, character limit), check box, radio groups,
  drop-down (also editable), list box (also multiple), push buttons, signature fields (shown, not signed here).
- The page tile already draws the field; a see-through control sits over it and becomes the editor on focus. Values
  commit on blur or Enter, Esc reverts. A value that fails its format or range is rejected with Acrobat's message and the
  document is left as it was.
- Appearance streams are regenerated (MuPDF `update`, formatted text for fields with a format) so other viewers show
  what was typed. Check boxes and radios keep MuPDF's on and off appearances.
- Tab order is the page's `/Annots` order (honouring `/Tabs`); *Highlight fields* tints the widgets; *Clear form* is one
  undo step and puts every field back to its default value; push buttons: reset works (named fields or the whole form),
  next/previous/first/last page work, alert shows its message, submit and links are shown and do nothing.
- Calculated fields are read-only to the person filling and re-run after every change in `/CO` order.

## Scripts
MuPDF is built without JavaScript, so Inkwell has its own small engine (QuickJS, `forms/js.rs`; see
[forms-js.md](forms-js.md) for the limits and what is unsupported). A field script that is exactly one of the calls Acrobat
writes from its Format, Validate and Calculate tabs takes the native fast path below. Every other script runs in the
engine, unless the person turned scripts off for the file: then it is kept untouched in the file and never run (the field
list marks it, as before). No script can reach the network or a file.

| Where | Calls |
|---|---|
| Format (`/AA /F`, keystroke `/K`) | `AFNumber_Format`, `AFPercent_Format`, `AFDate_Format`, `AFDate_FormatEx`, `AFTime_Format`, `AFTime_FormatEx`, `AFSpecial_Format` (zip, zip + 4, phone, social security), `AFSpecial_KeystrokeEx` (pattern mask) |
| Validate (`/AA /V`) | `AFRange_Validate` |
| Calculate (`/AA /C`, order `/CO`) | `AFSimple_Calculate` (sum, product, average, min, max) and simplified field notation (`+ - * /`, brackets, `\ ` for a space in a name) |

## Document JavaScript
The Document JavaScript sheet (command palette "Document JavaScript…", All tools > Scripts; `forms/doc_js.rs`) lists
what a file carries and edits it. Looking never runs anything: the engine only reads the objects, and the syntax check
compiles the text in a throwaway engine without evaluating it.

| Where in the file | Listed as | Edited here | Runs in Inkwell |
|---|---|---|---|
| `/Names /JavaScript` | Document scripts | add, rename, replace, delete | yes, at open, when scripts are on for the file (the sandbox in [forms-js.md](forms-js.md)) |
| `/OpenAction` | Document opens | replace, clear (a non-JavaScript action is shown by what it does) | no |
| catalog `/AA` WC, WS, DS, WP, DP | Before closing, before/after saving, before/after printing | replace, clear | no |
| page `/AA` O, C | Page actions (page opens, closes) | replace, clear, set on any page | no |
| field, link and bookmark scripts | counts only; "Show in Forms" jumps to the fields | no | fields: yes, as above; links, bookmarks: no |

- Chained actions (`/Next`) are listed in order, to 64 steps. Clearing an action removes the whole chain. A step that is
  not JavaScript is shown by what it does and left alone, except a first step, which saving a script replaces.
- A script over 1 MiB is shown as too large and is not read; over 256 KiB it is stored and listed but not syntax-checked,
  because the engine does not run one that long.
- Replacing the first step of an action makes a new action, so a file that shares one action object between pages or
  links is not changed under the others. A later step of a `/Next` chain is edited where it is: if other actions share
  that chain they change too. The fix, if it matters, is to copy the chain up to the edited step.
- A script with a syntax error can be saved after a confirm (Acrobat allows it); the status line gives line and column.
- "What this file can do" is a word scan for `app.launchURL`, `submitForm`, `mailDoc`, `print`, `exportDataObject`,
  `importDataObject`, `getURL` and alert loops. A call built from pieces of text is not seen, so a clean header is not a
  guarantee. Scripts inside XFA forms are not covered.
- Every edit is one undo step. Replace, delete and clear are removing edits: the old text is not left in the saved bytes.
- An edit changes the file, not the running forms engine, which reads the named document scripts once per open document
  and keeps them (`forms/js.rs`, `doc_scripts`). If the forms rail has not listed the fields since the file was opened,
  the first listing after an edit reads the edited scripts and runs them under the normal scripts-on policy. Once it has
  read them, an edited, added or deleted script takes effect when the file is closed and opened again. Actions
  (open, save, print, page) are never run, so they have no such delay.
- Not an assistant tool: the sheet is something a person opens, so a script can only be changed by someone who looked.

## Preparing a form
Forms rail, *Prepare form*: the strip over the page picks a field tool (text, check box, radio, drop-down, list box,
button, signature); click for the default size or drag a rectangle. Select a field to move it, resize it from the eight
handles, or change its properties in the rail: name, tooltip, required, read-only, hidden, default value, character
limit, comb, multiple lines, password, choices, export value, button label and action, text size and colour, alignment,
border and fill, number/percentage/date/time/special/pattern/custom-script format (with a live sample), range or custom validation, calculation (sum, product, average, smallest, largest, simplified field notation or a custom script) and the calculation order (see [forms-js.md](forms-js.md)). A radio button drawn
while another is selected joins its group. Delete removes the selection (the Delete key too).

*Find fields on this page* runs auto-detection (underscore runs, rules under labels, empty boxes, small squares, boxes
drawn as four lines) and says what it would add before it adds anything; *Add them* is one undo step. Names come from the
label next to the field; a blank labelled Signature, Sign or Signed becomes a signature field. Tab order can be set by row or by column, or one step at a time.

Fields are built as plain AcroForm dictionaries (MuPDF only creates signature widgets): `/FT /Tx /Btn /Ch /Sig`, `/Ff`,
`/MK`, `/DA` with the form's `/DR` Helvetica and ZapfDingbats, `/AP` from MuPDF.

## Later (not built)
- FDF, XFDF and JSON export and import of field values. `forms_list` already carries name, value(s) and kind for every
  field, so each format is a reader/writer over that list (`forms_set_value` takes the values back); XFDF can reuse the
  field naming of `FieldDto::name` (dotted full names).
- Field alignment tools (align, centre, match size, distribute) and the Fields list's rename-in-place.
- Signing a signature field (see the Sign task).
