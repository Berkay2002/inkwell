# Forms r1: fill a form, then prepare one (phase 3b)

First round of the surface; there is no verdict to start from. Keeps design/README.md (Inkwell system: rounded, warm,
one ink-blue accent, Segoe UI Variable, one right rail at a time in the assistant/Tools dock) and reopens nothing.
One app window per image at 1536x1024, two images per variant: `-1-fill` (filling a form on the page) and `-2-author`
(Prepare form: a field selected, its properties visible). The sheet is `--cols 2`: one row per variant.

## Verdict

2026-10-11: a-rail, chosen as what is both standard and what pdfcraft and Acrobat have, with a hint of uniqueness.
Reason: Acrobat puts Prepare Form's field list and properties in a right pane, and a-rail
keeps that placement, with pdfcraft's floating field strip and properties in the same rail instead of a modal. b-bar and
c-page are dropped.
Approved: `design/approved/forms.png` (a-rail-1-fill), `forms-2.png` (a-rail-2-author).
Built as the Forms rail in the shared dock (Ctrl+Alt+F), the strip floating at the left of the page area. Screens of the
build: `docs/screens/forms/`. Follow-up from the review: a blank labelled Signature, Sign or Signed is detected as a
signature field, as in Acrobat.

## How Acrobat Pro does it (study 07-forms; docs/study/screens/07-forms/04-autodetected-fields.png, 05-textfield-props-*.png)
- Filling: open a form, fields are highlighted in pale blue ("Highlight Existing Fields"), you click and type on the page.
- Prepare a form: All tools > Prepare a form. A launcher auto-detects fields, then the left panel lists *Add form
  components* (Text field, Image field, Checkbox, Radio button, Drop-down list, List box, Button, Date field, Digital
  signature, Barcode) and, with a selection, Align / Center / Match size / Distribute. The right *Fields* pane is a tree
  of Page > fields with type icons, a filter and A-Z sort. Preview and Send at the bottom.
- Properties: double-click a field opens a modal "Text Field Properties" with tabs General, Appearance, Position,
  Options, Actions, Format, Validate, Calculate.

## How pdfcraft does it (github.com/storytold/pdfcraft @ b8ddf71: docs/images/pdfcraft-forms.png, crates/ui-egui/src/prepare.rs)
- A blue notice bar above the page: "This document contains 13 interactive form fields." with a "Hide field
  highlights" button. A floating vertical tool strip at the left. A right "Fields 13" rail grouped by page with a type
  icon, name and current value per row. Prepare form adds the same nine tools, name tags on the page, Field Properties.

## Where the variants go
- **a-rail** follows pdfcraft: floating field strip and a Fields rail with values; departs from both by putting the
  properties of the selected field inside the same rail, below the list, instead of a modal.
- **b-bar** follows Acrobat: a slim second toolbar row of labelled field tools plus "Auto-detect", the Fields tree on
  the right; departs by showing the property tabs (General, Appearance, Options, Format) in the rail, not a dialog.
- **c-page** departs from both: no strip and no property rail. One "Add field" pill floats over the page; the selected
  field carries its properties in a popover on the page; the rail is only the Fields list and a one-line prediction of
  what the next action will do ("Detect fields would add 6 text fields and 2 checkboxes").

Idea: you fill the form where it is, and when you prepare one, every change says what it will do before you run it
and is one undo step. For: a person filling a tenancy application, or turning a flat form into a fillable one.
Feel: calm, precise, paper-like.
Palette: as the system (window #F4F2ED, surfaces #FFFFFF, panel #FAF8F4, text #1A1C20, secondary #6B675F, borders
#E3DED4, accent #2E4B8F). Field highlight on the page is a very pale ink-blue wash (#E4EAF6) with a 1 px #B9C6E4
edge; it is a document overlay, not a UI hue.
Type: Segoe UI Variable.
Motion (build): selection handles ease in 100 ms; the rail row of the selected field scrolls into view with a 2 px
accent left edge. Reduced motion: no easing.
Must not appear: grids of cards, glow, purple, red, emoji, people avatars, sparkles, glass, a dark theme.

## Shared

Render: one flat screenshot of one Windows 11 desktop application window, seen straight on, filling the whole
1536 x 1024 frame edge to edge. No board, no desktop background, no second window, no device frame, no perspective.
Crisp at 100 % scale like a real screenshot: large legible UI text (13-15 px UI labels, 16-18 px document body
text), sharp 1 px hairlines, sharp icon strokes, no blur, no compression smudge. Every word real, correctly spelled
English; never lorem ipsum, never squiggles standing in for text.
The app is Inkwell, a PDF app, matching the attached reference image's window, title bar, palette, type and rounded
shapes exactly. Window: slim custom title bar with the ink-drop mark and "Inkwell", a "Home" tab, an active rounded
tab "Tenant application.pdf", a "+" button, and the window controls. Under it a slim toolbar with a rounded
segmented pill "Read | Organize | Comment | Forms" with Forms selected, undo and redo icons.
Document: a one-page tenant application on white paper with a soft shadow. Heading "TENANT APPLICATION", a line
"Maple Property Management AB · Storgatan 12, Malmö". Fields with printed labels above them, in two columns:
"Full name" (text field, value "Alex Johnson"), "Email" (text field, value "alex@example.se"), "Date of birth" (text
field, value "1991-04-18"), "Monthly income (SEK)" (text field, right aligned, value "42 500,00"), "Country" (drop-down,
value "Sweden"), "Preferred contact" (two radio buttons "Email" selected and "Phone"), a checkbox "I agree to the
terms" checked, a checkbox "Send me updates" unchecked, a multi-line field "Notes" empty, and a "Signature" field
box at the bottom, empty, with a small "x" at its left. Every field has the very pale ink-blue wash and 1 px edge.
Shape language: rounded and smooth (8-12 px radii, pill fields, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; panel #FAF8F4; text #1A1C20; secondary #6B675F; borders #E3DED4; one UI
accent ink blue #2E4B8F only for the selected mode, selection and primary button. No other UI hues, no red.
Type: Segoe UI Variable style humanist sans.
Must not appear: square corners, boxy grids of cards, red, bright blue, purple, gradients, glow, glass, sparkles,
emoji, people photos, dark theme, red PDF icons, browser chrome, device frame, watermark, lorem ipsum.

## Shared a-rail
Idea: pdfcraft's layout in Inkwell's look. A small floating vertical rounded strip at the left of the page with nine
field tools: select (active, ink blue), text field, checkbox, radio button, drop-down, list box, button, date,
signature. The right rail, about 360 px wide on the panel colour, titled "Fields" with "9" beside it, a magnifier
and a funnel icon, then a group "Page 1" with one row per field: type icon, name, current value in secondary text.

## a-rail-1-fill
ref: ../../approved/comments.png
The form is filled as described. A slim rounded notice above the page: "This form has 9 fields" with a quiet "Clear
form" text button and a toggle "Highlight fields" on. The strip is hidden (filling needs no tools): only the page and
the rail. The rail rows show the values, the "Signature" row says "Empty" in secondary text.

## a-rail-2-author
edit: a-rail-1-fill.png
Keep this screenshot exactly: same window, tabs, toolbar, page and rail frame. Change only this: the toolbar pill has
a small "Prepare form" label chip next to it; the floating field strip is visible at the left of the page; the "Email"
field is selected with eight small round ink-blue handles and its name "Email" as a small tag above it; the values on
the page are cleared to empty fields. In the rail, under the list, a section "Email" with its properties as plain
rows: Name "Email", Required (switch on), Read only (switch off), Default value (empty), Format "None" with a
chevron. The matching list row has an ink-blue 2 px left edge and a very light blue tint.

## Shared b-bar
Idea: Acrobat's toolbar strip, light and rounded. In Forms mode a slim second toolbar row under the main toolbar
carries, left to right, quiet icon + label buttons: "Select", "Text", "Checkbox", "Radio", "Drop-down", "List",
"Button", "Date", "Signature", a divider, "Auto-detect fields" (with a small wand icon), and on the right "Preview".
The right rail is "Fields" (360 px), a pill search, then a tree: "Page 1" with its field rows (icon and name).

## b-bar-1-fill
ref: ../../approved/comments.png
The same filled form. The second toolbar row is replaced by one slim row: "Highlight fields" toggle on, "Clear
form" and, at the right, an ink-blue "Prepare form" button. The rail lists the field names without values.

## b-bar-2-author
edit: b-bar-1-fill.png
Keep this screenshot exactly: same window, page and type. Change only this: the second toolbar row is the field tools
row described above, "Select" active; the "Email" field is selected with eight round handles; the rail below the
tree shows four small tabs "General", "Appearance", "Options", "Format" with General open: Name "Email", Tooltip
"Email", Visibility "Visible" (drop-down), Required switch on, Read only switch off. The matching tree row has an
ink-blue left edge.

## Shared c-page
Idea: nothing but the page. No tool strip, no properties rail. A small floating rounded pill at the top centre of the
page "Add field" with a chevron and, beside it, "Detect fields". The right rail is only the Fields list (names and
values) at 360 px, and under it a quiet one-line note.

## c-page-1-fill
ref: ../../approved/comments.png
The filled form with the pale wash on fields; the pill is not shown. The rail lists the field names with values, and
at the bottom a progress line "8 of 9 filled" with a thin ink-blue bar, and "Clear form" as a text link.

## c-page-2-author
edit: c-page-1-fill.png
Keep this screenshot exactly: same window, page, rail and type. Change only this: the values on the page are cleared;
the pill "Add field" and "Detect fields" appear at the top centre of the page; the "Email" field is selected with
eight round handles and a rounded white property popover just below it with: a name field "Email", two small
switches "Required" (on) and "Read only", a small drop-down "Format: None", and a round trash icon. At the bottom of
the rail a note: "Detect fields would add 6 text fields and 2 checkboxes on this page." with a text link "Add them".
