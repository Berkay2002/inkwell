# Marks r1: header and footer, watermark, background, Bates numbering (phase 5)

First round of the surface; there is no verdict to start from. The design follows `design/README.md` (Inkwell system:
rounded, warm, one ink-blue accent, Segoe UI Variable) and the Tools, Redact and Protect rails (one right rail at a
time, same width and chrome as the assistant rail). One app window per image at 1536x1024, two images per variant:
`-1-header` (Header & footer filled in, the live preview on the page) and `-2-watermark` (a text watermark, the live
preview on the page). The sheet is `--cols 2`: one row per variant.

Brief (task-page-marks.md): header and footer with six slots (left, centre, right by top and bottom), font, size,
colour, margins, page-number and date macros, start number, page range with odd and even; watermark from text or an
image, rotation, opacity, scale relative to the page, position, behind or in front, shown on screen and in print;
background from a colour or an image with opacity; Bates numbering with prefix, suffix, digits, start number and a
slot, across several files; Add, Update and Remove for each, one undo step each; a live preview before applying and
the outcome said in a sentence.

## Verdict

**a-tabs** (2026-10-11), as what is both standard and what pdfcraft and Acrobat have, with a hint of uniqueness. The
images were not rendered, so the pick is made on this spec text. From b-list, one thing: a small state line at the top of the rail
showing which marks the document already has ("Has: header & footer · watermark"), each with Update and Remove, so
existing marks are found without opening each tab.

## How Acrobat Pro does it (study 15-page-marks: not observed; from the PDF spec and pdfcraft)
- Edit a PDF, then the "Header & footer" menu: Add, Update, Remove, and the same for Watermark, Background and Bates
  numbering. Add opens a modal dialog with a live preview thumbnail of the page on the right.
- Header & Footer dialog: three header fields (left, centre, right) and three footer fields, font, size and colour,
  "Insert page number" and "Insert date" with a format list, margins (top, bottom, left, right), page range options
  and "Shrink document to avoid overlapping the document's text and graphics".
- Watermark dialog: Text or File, font, size, colour, an opacity slider, rotation (none, clockwise 45, counter-clockwise
  45, custom), scale relative to the page, vertical and horizontal distance from a position, "Appearance options"
  (show when printing, show when displaying on screen), "Page range options", and the preview. Background is the same
  with a colour or a file.
- Bates numbering is a dialog with an "Add files" list, then the same six slots, with "Insert Bates number" opening
  "Number of digits", "Start number", "Prefix" and "Suffix".

## How pdfcraft does it (github.com/storytold/pdfcraft @ b8ddf71: crates/ui-egui/src/marks_ui.rs)
- One window "Page marks" with a row of four kinds; the settings of the picked kind below: for header and footer six
  text fields in a two-row grid, a font family, size, colour; for watermark text, size, opacity, rotation and a
  behind or in front switch; a page selection field; "Apply" and "Remove" buttons. The state of the document is a
  line: "This document has a watermark on pages 1-12".
- Nothing is previewed on the page; the outcome is a line of text.

## Where the variants go
- **a-tabs** follows Acrobat's dialog as one calm rail sheet: four tabs (Header & footer, Watermark, Background,
  Bates), the six slot fields in a 2 x 3 grid, every Acrobat choice present, a page range, and a live preview on the
  page itself. It departs by writing the outcome in a sentence above the button.
- **b-list** follows pdfcraft: the rail opens on a short list of the four kinds with what the file has now ("None",
  "Pages 1-12"), each a row that opens its own compact sheet with Add, Update and Remove at the bottom. One thing
  at a time.
- **c-onpage** departs from both: the six slots are on the page itself, as soft outlined fields at the top and bottom
  edges of the page that you type into; the rail holds only the look (font, size, colour, margins), the pages, and
  the outcome sentence.

Idea: you see the mark where it will be before it is written, and the sentence says exactly what it will do.
For: someone numbering pages of a contract, stamping a draft, or numbering a bundle of evidence.
Feel: calm, exact, a small stamp rather than a drama.
Palette: as the system (window #F4F2ED, surfaces #FFFFFF, panel #FAF8F4, text #1A1C20, secondary #6B675F, borders
#E3DED4, accent #2E4B8F). The marks drawn on the page in the preview are in ink blue so they read as a preview and
not as part of the document.
Type: Segoe UI Variable; file names, page numbers and the outcome numbers in a quiet monospace.
Motion (build): the preview moves with the field you type in (no easing, it follows the keystroke); the outcome
sentence swaps its changed phrase with a 120 ms cross-fade. Reduced motion: no easing.
Must not appear: red, a warning triangle, glow, glass, gradients, purple, emoji, avatars, dark theme, tables of boxed
cards.

## Shared

Render: one flat screenshot of one Windows 11 desktop application window, seen straight on, filling the whole
1536 x 1024 frame edge to edge. No board, no desktop background, no second window, no device frame, no perspective.
Crisp at 100 % scale like a real screenshot: large legible UI text (13-15 px UI labels, 16-18 px document body
text), sharp 1 px hairlines, sharp icon strokes, no blur, no compression smudge. Every word real, correctly spelled
English; never lorem ipsum, never squiggles standing in for text.
The app is Inkwell, a PDF app, matching the attached reference image's window, title bar, palette, type and rounded
shapes exactly. Window: slim custom title bar with the ink-drop mark and "Inkwell", a "Home" tab, an active rounded
tab "Lease agreement.pdf", a "+" button, and the window controls. Under it a slim toolbar with a rounded segmented
pill "Read | Organize | Comment" with Read selected, undo and redo icons. A left page-thumbnail strip, the page large
in the centre, the right rail about 360 px wide on the panel colour with a close icon, in the same dock as the
reference's Compress rail.
Document: a residential lease agreement on white paper with a soft shadow, page 1 of 12, heading "RESIDENTIAL LEASE
AGREEMENT", then "1. PARTIES": "This Residential Lease Agreement is made on 1 October 2026 between Maple Property
Management AB (the Landlord) and the Tenant named below.", then "2. RENT": "The rent is 12 400 SEK per month, due on
the 25th of each month."
Shape language: rounded and smooth (8-12 px radii, pill fields, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; panel #FAF8F4; text #1A1C20; secondary #6B675F; borders #E3DED4; one UI
accent ink blue #2E4B8F only for the selected choice, the preview marks and the primary button. No other UI hues, no
red, no orange, no green.
Type: Segoe UI Variable style humanist sans; file names and numbers in a quiet monospace.
Must not appear: square corners, boxy grids of cards, red, orange, green, bright blue, purple, gradients, glow, glass,
sparkles, emoji, people photos, dark theme, red PDF icons, browser chrome, device frame, lorem ipsum.

## Shared a-tabs
Idea: Acrobat's dialog as one rail sheet. The rail is titled "Page marks" with a close icon. Under the title a
four-way segmented control: "Header & footer" (selected), "Watermark", "Background", "Bates". Content, top to
bottom on the panel colour, no cards, separated by hairlines:
a label "Top" over three small pill fields side by side, the left one empty, the centre one empty, the right one
holding "Lease agreement"; a label "Bottom" over three pill fields, the left holding "<<m/d/yyyy>>", the centre empty,
the right holding "Page <<1>> of <<n>>"; a small row of two quiet buttons "Insert page number" and "Insert date";
a row with a select "Helvetica", a number field "10 pt", a small colour swatch, and bold and italic toggle buttons;
a row "Margins" with four small number fields "Top 36", "Bottom 36", "Left 72", "Right 72";
a row "Pages" with a segmented control "All | Odd | Even | Range" with All selected;
a row "Start number" with a small field "1".
Then the outcome sentence in the text colour: "Adds 'Lease agreement' to the top right and 'Page 1 of 12' to the
bottom right of 12 pages." Then a full-width primary ink-blue button "Add to the document" and under it a quiet text
link "Remove header and footer".

## a-tabs-1-header
ref: ../../approved/tools-compress.png
The page is large in the centre. On the page, in ink blue, the live preview: "Lease agreement" at the top right
margin and "Page 1 of 12" at the bottom right and "10/10/2026" at the bottom left, in 10 pt. The rail shows the
sheet described above.

## a-tabs-2-watermark
edit: a-tabs-1-header.png
Keep the window, tabs, toolbar, thumbnails and type exactly. Change only this: on the page, a large diagonal text
"DRAFT" in light grey, rotated 45 degrees counter-clockwise, 50 % opacity, across the middle of the page, in front of
the text; the header and footer preview is gone. In the rail the segmented control has "Watermark" selected and the
sheet is: a two-way segmented control "Text | Picture" with Text selected; a pill field holding "DRAFT"; a row with
a select "Helvetica", a bold toggle (on) and a small grey swatch; a row "Opacity" with a slider at 50 % and "50 %" at
its end; a row "Rotation" with a three-way segmented control "None | 45° | Custom" with 45° selected; a row "Size"
with a slider and "50 % of the page"; a row "Position" with a segmented control "Behind the text | In front"
with In front selected; two quiet checkboxes "Show on screen" and "Show when printing", both checked; a row "Pages"
with "All" selected. Then the sentence "Adds 'DRAFT' in front of the text on 12 pages." and the primary button "Add to
the document" with a quiet link "Remove watermark".

## Shared b-list
Idea: pdfcraft's list of kinds. The rail is titled "Page marks" with a close icon.
Four plain rows with hairlines between, each with a name on the left, what the file has now in secondary text under
it, and a chevron on the right: "Header & footer" "None"; "Watermark" "On pages 1-12"; "Background" "None";
"Bates numbering" "None".

## b-list-1-header
ref: ../../approved/tools-compress.png
The page is large in the centre with a small header at the top right "Lease agreement" and a footer "Page 1 of 12"
in ink blue (the preview). The rail has a back arrow and the title "Header & footer" and shows a compact sheet: two
rows of three pill fields (top, bottom) as in a table with the words "Left", "Centre", "Right" above them and "Top"
and "Bottom" at the row starts; a row with a select "Helvetica", a field "10 pt" and a swatch; a row "Pages" with
a select "All pages". Then a sentence "Adds 'Page 1 of 12' to the bottom right of 12 pages." and a primary button
"Add" with a quiet button "Remove".

## b-list-2-watermark
edit: b-list-1-header.png
Keep the window, tabs, toolbar, thumbnails and type exactly. Change only this: on the page a diagonal text "DRAFT"
in light grey at 45 degrees, 50 % opacity; the header and footer preview is gone. The rail has a back arrow and the
title "Watermark" and shows a compact sheet: a pill field "Text" holding "DRAFT"; a row "Opacity" with a slider and
"50 %"; a row "Rotation" with a field "45°"; a switch "Behind the text" (off); a row "Pages" with a select "All
pages". Then the sentence "Replaces the watermark on 12 pages." and a primary button "Update" with a quiet button
"Remove".

## Shared c-onpage
Idea: type the marks where they go. The page is large in the centre with, in ink blue, six soft outlined pill fields
laid on the page: three at the top margin (left, centre, right) and three at the bottom margin. The top right field
holds "Lease agreement" and is the one focused (ink blue outline), the bottom right holds "Page 1 of 12", the bottom
left "10/10/2026"; the three others show a faint "Add text" hint. The rail is titled "Header & footer" with a close
icon.

## c-onpage-1-header
ref: ../../approved/tools-compress.png
The rail, top to bottom: a small segmented control "Header & footer | Watermark | Background | Bates" with the first
selected; a row "Insert" with two quiet buttons "Page number" and "Date"; a row with a select "Helvetica", a field
"10 pt", a swatch; a row "Margins" with a single field "0.5 in" and a link "More"; a row "Pages" with "All | Odd |
Even | Range". Then the outcome sentence "Adds 2 marks to the top right and bottom right of 12 pages." and a primary
button "Add to the document" with a quiet link "Remove".

## c-onpage-2-watermark
edit: c-onpage-1-header.png
Keep the window, tabs, toolbar, thumbnails and type exactly. Change only this: the six slot fields are gone; the page
shows, in the middle, a large diagonal text "DRAFT" in light grey at 45 degrees, 50 % opacity, with eight small round
handles around its box and a rotation handle above, in ink blue, as if it can be dragged. The rail has "Watermark"
selected in the segmented control and shows: a pill field "DRAFT"; a row with a select "Helvetica", a bold toggle and
a swatch; a row "Opacity" slider "50 %"; a row "Position" with a segmented control "Behind | In front"; two quiet
checkboxes "Screen" and "Print"; a row "Pages" with "All". Then "Adds 'DRAFT' in front of the text on 12 pages." and
the primary button "Add to the document".
