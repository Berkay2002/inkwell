# Print r1: the print sheet (phase 5)

First round of the surface; there is no verdict to start from. It keeps the Inkwell system from `design/README.md`
(rounded, warm, one ink-blue accent, Segoe UI Variable) and reopens one recorded decision: tools live in a right rail,
one at a time. Printing is a hand-off to the outside world, so it gets a centred sheet; the variants test that against
a rail. One app window per image at 1536x1024. Variant a-sheet has two images (`-1-options`, `-2-booklet`), b-rail and
c-ticket one each. The sheet is `--cols 2`.

## Verdict

2026-10-11: **a-sheet, as built.** "a-sheet, as built. It keeps Acrobat's
standard modal with the preview on the left. Take one thing from c-ticket: make the outcome line ("6 sheets of A4, both
sides, 2 copies") the prominent summary beside the Print button, and update it live. Skip a-sheet-2."
b-rail and c-ticket are dropped; a-sheet-2-booklet was never rendered and is skipped.
Approved: a-sheet-1-options. Built as `src/components/PrintSheet.tsx`; the outcome line is
`outcomeLine` in `src/lib/printStore.ts`.

Render notes: `a-sheet-2-booklet` was not rendered; the
booklet state is described above and is built from the engine's own preview. `c-ticket` came out as a wider panel with
switch rows instead of chips; the idea (outcome first, a few toggles, "More options") is intact. Text in renders is
not authoritative (the build sets the copy).

## How Acrobat Pro does it
- Ctrl+P (toolbar printer icon) opens one **modal Win32 dialog**, about 959 x 697 px at 125 % display scaling, which grows
  downward when More Options or Multiple/Booklet add rows. Controls on the left, preview on the right.
- Top band: Printer combo with **Properties** (the driver's own dialog) and **Advanced** (Advanced Print Setup: output,
  marks and bleeds, PostScript, colour management, Print As Image with a dpi); Copies with **Collate** (only shown above 1),
  **Print in grayscale**, **Save ink/toner**.
- **Pages to Print**: All, Current, Pages (range text), and a "More Options" fold: Current view, Reverse pages, Odd or
  Even Pages. **Page Sizing & Handling** is four tabs: Size (Fit, Shrink oversized pages (default), Actual size, Custom
  scale %, "Choose paper source by PDF page size"), Poster (tile scale, overlap, cut marks, labels), Multiple (2, 4, 6, 9,
  16 or n by m; four page orders; page border; auto-rotate), Booklet (both sides / front / back, binding left or right,
  sheet range). Orientation: Auto, Portrait, Landscape. **Comments & Forms**: Document, Document and Markups (default),
  Document and Stamps, Form fields only, with a **Summarize Comments** button that opens a modal Yes/No with no cancel.
- Right: the preview with "Scale: 97%" or "Document: 8.5 x 11.0in", the paper size ("8.27 x 11.69 Inches"), a pager and
  slider with "Page i of N (document page)". It updates live for every option. **Page Setup...**, **Print**, **Cancel**.
- Two-sided is not in Acrobat's own dialog: it is in the driver's Properties, and shown only for duplex printers.
- Everything is on one screen at once, in small type; the dialog is dense and the options that matter most (how many
  sheets, which side) are never stated in words.

## How pdfcraft does it (github.com/storytold/pdfcraft @ b8ddf71: crates/ui-egui/src/print_ui.rs)
- The same shape as Acrobat in egui: one window, left options, right preview with "‹ sheet ›" paging.
- Options: Printer with **Properties...** (the driver's own settings) and "Save as PDF", Copies, Collate, Print in
  grayscale, Two-sided (Off / long edge / short edge), Paper, Pages to Print (All, Current, Pages, Selected pages,
  odd/even, reverse), Page Sizing & Handling (Size, **Poster** with tile scale, overlap, cut marks; Multiple with
  order incl. "Cut and stack", border, auto-rotate; Booklet with subset and binding), Orientation, Comments & Forms
  (Document, Document and markups, Document and stamps, Form fields only).
- Progress and "Sent to <printer>" are shown in the same window; no Windows printing yet (issue #756).

## Where the variants go
- **a-sheet** follows Acrobat and pdfcraft (one modal with the preview beside the options) and departs by being a calm
  single column of grouped fields at 14 px with the outcome stated in plain words under the preview ("6 sheets of A4,
  both sides, 2 copies"). Booklet is a first-class mode with its own picture of the fold.
- **b-rail** follows Inkwell's own rail (tools, comments, redact): the options in the right rail, and the **page area
  itself** becomes the preview: the viewer shows the sheets as they will come out, so there is no second preview
  panel and no modal. It departs from both by not being a dialog.
- **c-ticket** departs from both: a compact "ticket" popover under the toolbar with the predicted outcome first
  (what, how many sheets, which printer) and one-tap chips for the common changes (Both sides, Grayscale, 2 per
  sheet, Booklet); everything else sits behind "More options", which opens the full sheet. Fast for the 90 % case,
  complete for the rest.

Idea: you see the paper before you spend it. The preview is the printer's own drawing, so what you see is what comes out.
For: someone printing a contract, a paper or a booklet who has to get it right the first time. Feel: calm, exact.
Palette: as the system (window #F4F2ED, surfaces #FFFFFF, panel #FAF8F4, text #1A1C20, secondary #6B675F, borders
#E3DED4, accent #2E4B8F). The paper in the preview is white with a soft shadow; page content is document colour.
Type: Segoe UI Variable.
Motion (build): the preview crossfades 120 ms to the next drawing; the progress bar is a plain filled track.
Reduced motion: no crossfade.
Must not appear: dense Acrobat-style small type, grids of cards, glow, purple, red, emoji, sparkles, glass, a dark theme.

## Shared

Render: one flat screenshot of one Windows 11 desktop application window, seen straight on, filling the whole
1536 x 1024 frame edge to edge. No board, no desktop background, no second window, no device frame, no perspective.
Crisp at 100 % scale like a real screenshot: large legible UI text (13-15 px UI labels), sharp 1 px hairlines, sharp
icon strokes, no blur, no compression smudge. Every word real, correctly spelled English; never lorem ipsum, never
squiggles standing in for text.
The app is Inkwell, a PDF app, matching the attached reference image's window, title bar, palette, type and rounded
shapes exactly. Window: slim custom title bar with the ink-drop mark and "Inkwell", a "Home" tab, an active rounded
tab "Lease agreement.pdf", a "+" button, and the window controls. Under it a slim toolbar with a rounded segmented
pill "Read | Organize | Comment", undo and redo icons.
Document: a residential lease agreement, 12 pages, on white paper: heading "2. PARTIES", sentences "This Residential
Lease Agreement is made on 1 October 2026 between Maple Property Management AB (the Landlord) and the Tenant named
below." then "2.3 Rent": "The rent is 12 400 SEK per month, due on the 25th of each month."
Print settings shown (where shown): printer "Microsoft Print to PDF", copies 2, collate on, pages All 12, paper A4,
two-sided "Both sides, flip on long edge", colour, comments "Document and markups", outcome line "6 sheets of A4,
both sides, 2 copies".
Shape language: rounded and smooth (8-12 px radii, pill fields, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; panel #FAF8F4; text #1A1C20; secondary #6B675F; borders #E3DED4; one UI
accent ink blue #2E4B8F only for the selected mode and the primary button. No other UI hues, no red.
Type: Segoe UI Variable style humanist sans.
Must not appear: square corners, red, bright blue, purple, gradients, glow, glass, sparkles, emoji, people photos,
dark theme, red PDF icons, browser chrome, device frame, watermark, lorem ipsum.

## Shared a-sheet
Idea: the one-modal shape of Acrobat and pdfcraft, calmer. A sheet about 980 x 700 px centred over the dimmed document.
Title row "Print" with the file name "Lease agreement.pdf" in secondary text and a close icon. Left 60 %: the preview
on the panel colour: one white A4 sheet with page 2 of the lease drawn on it, a soft shadow, and under it a pager
"1 of 6" with small up and down chevrons and the words "pages 1, 2". Right 40 %: one column of grouped fields with
small secondary labels: Printer (a drop-down), Copies (2) with Collate, Pages as a three-part segmented control
"All 12 | Page 2 | Range", "Pages per sheet" segmented "One | Several | Booklet", Scaling segmented "Fit | Shrink |
Actual size | Custom", Paper drop-down "A4", Orientation segmented, Colour segmented "Colour | Grayscale", Two-sided
drop-down, Comments and forms drop-down. Footer: secondary text "6 sheets of paper, 2 copies" on the left, a quiet
"Cancel" and a primary ink-blue "Print" on the right.

## a-sheet-1-options
ref: ../../approved/comments.png
The sheet over the dimmed viewer, settings as in Shared, "One" selected, "Fit" selected.

## a-sheet-2-booklet
edit: a-sheet-1-options.png
Keep this screenshot exactly: same window, sheet, fields and type. Change only this: "Booklet" is selected under
Pages per sheet, two small segmented controls appear under it ("Both sides | Front only | Back only" and "Left |
Right" for binding), the preview shows one landscape A4 sheet with two lease pages side by side and a faint centre fold
line, the pager reads "1 of 4", and the footer reads "4 sheets of paper, 2 copies".

## Shared b-rail
Idea: no dialog. The right rail, about 360 px wide on the panel colour, titled "Print" with a close icon; the page
area shows the printed sheets: one white A4 sheet centred with page 2 of the lease on it, under it a small pill pager
"1 of 6" with chevrons. The rail holds the same fields as a quiet single column; at its bottom a pinned footer with
the outcome line "6 sheets of A4, both sides, 2 copies" and a full-width primary ink-blue "Print" button.

## b-rail-1-options
ref: ../../approved/comments.png
The rail open with the settings as in Shared; the viewer's sidebar of thumbnails stays at the left.

## Shared c-ticket
Idea: the 90 % case in one glance. A rounded popover about 420 px wide hanging under the toolbar's print icon on the
right, over the document (not dimmed). Top: a small preview of the first sheet (a white A4 thumbnail with the page
drawn) beside the outcome in large type "6 sheets of A4" and under it in secondary text "12 pages, both sides, 2
copies, Microsoft Print to PDF". Under that a row of toggle chips: "Both sides" (on, ink blue), "Grayscale" (off),
"2 per sheet" (off), "Booklet" (off), "Collate" (on). Then a small stepper "Copies - 2 +", a text link "Pages: All
12" and a text link "More options...". Footer: a full-width primary ink-blue "Print".

## c-ticket-1-options
ref: ../../approved/comments.png
The popover over the lease page, settings as in Shared.
