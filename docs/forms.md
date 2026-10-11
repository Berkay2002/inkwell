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

## Preparing a form
Forms rail, *Prepare form*: the strip over the page picks a field tool (text, check box, radio, drop-down, list box,
button, signature); click for the default size or drag a rectangle. Select a field to move it, resize it from the eight
handles, or change its properties in the rail: name, tooltip, required, read-only, hidden, default value, character
limit, comb, multiple lines, password, choices, export value, button label and action, text size and colour, alignment,
border and fill, number/percentage/date/time/special/pattern format, range validation, calculation. A radio button drawn
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
