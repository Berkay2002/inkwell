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
while another is selected joins its group. Delete removes the selection (the Delete key too).

*Find fields on this page* runs auto-detection (underscore runs, rules under labels, empty boxes, small squares, boxes
drawn as four lines) and says what it would add before it adds anything; *Add them* is one undo step. Names come from the
label next to the field; a blank labelled Signature, Sign or Signed becomes a signature field. Tab order can be set by row or by column, or one step at a time.

Fields are built as plain AcroForm dictionaries (MuPDF only creates signature widgets): `/FT /Tx /Btn /Ch /Sig`, `/Ff`,
`/MK`, `/DA` with the form's `/DR` Helvetica and ZapfDingbats, `/AP` from MuPDF.

## Form data files: FDF, XFDF and JSON

Forms panel menu (the three dots) and the command palette: **Export form data…** and **Import form data…**. Code:
`src-tauri/src/forms/data.rs`, with the caps and the XML and FDF readers in `src-tauri/src/exchange.rs`. The format of an
export follows the file name (`.fdf`, `.json`, anything else XFDF); an import is told by what is inside, not the name.

- **Exported:** every text, check box, radio, drop-down and list field by its full name (`Address.Street`), as stored:
  a check box or radio button as its export value or `Off`, a list box that takes several entries as a list, rich text as
  its plain value. Push buttons and signature fields have no value and are not written. JSON is
  `{ "full.name": "value" | ["values"] }`; XFDF nests `<field name>` by the dots; FDF is `/Fields` with `/T` and `/V`.
  Export runs no script.
- **Imported** as one undo step, through the same set-value path as typing: formats, validation, calculations and
  signature locks apply as they would to a person. Reported and skipped (the summary lists the first few with the
  reason, never a failure of the whole import): a name that is not a field, a value the field cannot take (a choice that
  is not in the list, an on/off for a text field, a list for a text field), a read-only field, a field a signature
  locks, and anything past 10 000 fields in one file. A script's alerts, links, submits and print requests during the
  import are dropped, not acted on.
- **Never done with a file:** run `/JavaScript` or `/AA`, follow `/F` file references, load a DTD or entities, read
  more than 32 MB, 100 000 values or objects, 64 levels or 1 MB in one string.
- **Certified documents** refuse the import (the menu item is off, with the reason); export is always allowed.
  **Signed** documents take it as an incremental update after the signature, and the summary says so.
- Known limit: the fields are looked up once at the start of an import, so a field a calculation rewrites later in the
  same import is compared with its earlier value when the file also has a value for it.
- No file written by Acrobat was available while this was built: the shapes follow ISO 19444-1 and ISO 32000-2
  section 12.7.8. XML data files (Acrobat's XDP and its plain XML/TXT forms) are not read.

## Later (not built)
- Field alignment tools (align, centre, match size, distribute) and the Fields list's rename-in-place.
- Signing a signature field (see the Sign task).
