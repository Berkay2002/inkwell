# Forms: fill, then author

Engine: `src-tauri/src/forms/` (`mod.rs` fields, filling, reset; `author.rs` create/move/edit/delete/tab order/detect; `layout.rs` align, centre, distribute, match size,
nudge, duplicate, copies and rename;
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
- The field list walks the pages' `/Annots` like the comment list ([comments.md](comments.md)): shared arrays are read once and
  a widget two pages list through different arrays (no `/P` naming the later one) belongs to the first page only. A file
  with more than 2 000 000 repeated or inline array entries (see [comments.md](comments.md)) is refused ("this file's form is too large to edit safely") rather than shown with
  fields missing.

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
- An edit never changes an object other places share: replacing the first step makes a new action, editing a later step
  copies the chain from the head down to it (a `/Next` array too), and a page or catalog `/AA` that pages share is
  copied before one of its entries is set or cleared. The other pages and links keep what they had.
- A certified file (a `/DocMDP` signature, any `/P`) is refused: any script change would void the certification, and the
  sheet shows its scripts read-only with the reason. A signed file is edited, but its save stays an update after the
  signature (no rewrite, which would void it), the sheet warns that the signature will show the change, and replaced
  or deleted text stays in the signed revision.
- A script with a syntax error can be saved after a confirm (Acrobat allows it); the status line gives line and column.
- "What this file can do" is a word scan for `app.launchURL`, `submitForm`, `mailDoc`, `print`, `exportDataObject`,
  `importDataObject`, `getURL` and alert loops. A call built from pieces of text is not seen, so a clean header is not a
  guarantee. Scripts inside XFA forms are not covered.
- Every edit is one undo step. Replace, delete and clear are removing edits: in an unsigned file the old text is not left
  in the saved bytes (a signed file is the exception above).
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
while another is selected joins its group. Delete removes the selection (the Delete key too; with several fields picked, all of them, as one step).

*Find fields on this page* runs auto-detection (underscore runs, rules under labels, empty boxes, small squares, boxes
drawn as four lines) and says what it would add before it adds anything; *Add them* is one undo step. Names come from the
label next to the field; a blank labelled Signature, Sign or Signed becomes a signature field. Tab order can be set by row or by column, or one step at a time.

Fields are built as plain AcroForm dictionaries (MuPDF only creates signature widgets): `/FT /Tx /Btn /Ch /Sig`, `/Ff`,
`/MK`, `/DA` with the form's `/DR` Helvetica and ZapfDingbats, `/AP` from MuPDF.

## Layout tools
Acrobat Prepare Form's layout tools, in `forms/layout.rs`. Each is one undo step, and a refused one records none.

- **Picking.** Ctrl, Cmd or Shift click adds a field to the picked ones (or takes it out; on a Mac Ctrl-click is the context menu, so Cmd or Shift); dragging a box on empty
  page picks every visible field it touches (with Ctrl, Cmd or Shift it adds). The **anchor** is the field picked last
  (a box picks in reading order, so the last one in the box), and has a heavier outline. A plain click picks one field.
  There is no group drag: a press on a field picks just that one, and it moves alone.
- **Align** (left, right, top, bottom, horizontal centres, vertical centres) and **Same width / height / size** follow the
  anchor, as in Acrobat, and need two or more fields on one page. "Horizontal centres" gives the fields one x centre (a
  column); "vertical centres" one y centre (a row). Same size keeps each field's top-left corner and pulls the field back
  onto the page if it would run off.
- **Centre on the page** (horizontally, vertically, both) moves the picked fields as a group, per page.
- **Distribute** (horizontally, vertically) needs three or more fields: the two outermost stay and the gaps between
  the others become equal (a gap may be negative when the fields are wider than the room).
- **Nudge**: arrow keys move the picked fields 1 pt (Shift: 10 pt), clamped to the page. The presses of a burst are added
  up and sent at key up as one step.
- All of it is worked out in page space, as shown: `/Rotate` and a crop box that does not start at the origin are
  honoured, and the written rectangles are put back into the page's unrotated space.
- **Duplicate on other pages** adds a widget of the same field to each page named, at the same place: same name, same
  field, one value (Acrobat's behaviour), so filling one fills them all. A page where the field does not fit refuses
  the whole step. Radio buttons and signature fields are not duplicated. A field that was one dictionary (field and
  widget merged) becomes a parent with widget kids. At most 2 000 pages.
- **Create multiple copies** (rows × columns, space between across and down in points) lays a grid to the right of and
  below the field. The field itself becomes `name.0` and the copies `name.1`, `name.2` … in row order. The gaps are the
  space *between* fields, not offsets. A copy that would run off the page is left out and the answer says how many.
  Refused for a field with several widgets, a calculated field and a radio or signature field. At most 500 copies.
  Calculations that use the old name through the simplified notation follow the rename; a script that calls
  `getField("name")` does not, and keeps naming the old field.
- **Rename**: F2 or a double click on a row of the Fields list edits the field's **full** dotted name in place (the name
  box in the properties edits the last part only). Groups of a dotted name are created or reused, and a group the
  rename empties is removed. A name that is already a field of the **same kind** offers *Merge* (the widgets join that
  field); a different kind is refused with a message, radio groups and signature fields never merge, and no field can be
  named below another field or over a group.
- **Refused**: a certified document at any `/P` (P 3 allows form fill and comments, not changing fields); a field a
  signature covers (`/Lock`) and a signature field that is signed. A certified document, or a selection that is too
  small, shows the controls disabled with the reason as their tooltip; a signature lock is refused by the engine with a
  message. A signed, uncertified document is changed incrementally: the signed bytes stay a prefix of the file.
- Caps: 1 000 widgets per step, 500 copies, 2 000 pages.
- Not built: dragging a group, and an assistant tool for any of this.

## Later (not built)
- FDF, XFDF and JSON export and import of field values. `forms_list` already carries name, value(s) and kind for every
  field, so each format is a reader/writer over that list (`forms_set_value` takes the values back); XFDF can reuse the
  field naming of `FieldDto::name` (dotted full names).
- Signing a signature field (see the Sign task).
