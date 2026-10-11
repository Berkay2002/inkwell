# Compare r1: compare two documents (phase 5, compare-files)

First round of the surface; there is no verdict to start from. This reopens nothing: it builds on **history-r1 b-compare**
(approved/history-2.png: Side by side / Slider / Overlay, "Show differences on", Previous / Next, a Changes rail, the
version strip) and keeps the Inkwell system (design/README.md): rounded, warm, one ink-blue accent, Segoe UI Variable, change
marks may use green / red / amber with glyphs (+ − ~) as the history round allowed. One app window per image at 1536x1024,
two images per variant: `-1-setup` (choose what to compare) and `-2-result` (the comparison). The sheet is `--cols 2`: one
row per variant.

What the surface must carry (task brief): pick two files, or the open document against another file or one of its past
versions; a summary with counts by kind; synced side-by-side and overlay views; a change list with jump-to and filter by
kind; text, images, pages added / removed / replaced, moved pages, formatting (font, size, colour); a way to export the
report as a PDF; progress and cancel while it runs (a 500-page document must stay responsive, so the result view never
waits for everything); it opens from the Tools menu and the command palette.

## Verdict

(pending: inkwell picks, then the pick is recorded here and in design/README.md)

## How Acrobat Pro does it (study 13-compare, docs/study/screens/13-compare/)

- Tools → **Compare a file**. The landing is a full-page two-slot form: "Old file" and "New file", each a big drop target
  with "Select file", a small page-range per file, and a swap arrow; under it a **Settings** link ("Text", "Images",
  "Annotations", "Headers and footers", "Background"... options) and a **Compare** button.
- The result opens a **summary** first (total number of differences, one line per kind: Replaced, Inserted, Deleted,
  Moved, Style, Image), then two panes side by side with synced scroll, a **changes list** at the left of the panes
  filtered by kind, and "Share / Save report" at the top right. Differences are marked on the pages with coloured boxes
  (replaced blue, inserted green, deleted red).
- (Details above are from Acrobat's documentation and the earlier study; the screenshots and exact wording are filled in
  from `docs/study/13-compare.md` when the study lands.)

## How pdfcraft does it (E:\Dev\pdfcraft: crates/compare, crates/ui-egui/src/compare_ui.rs)

- "Compare files…" opens a small **dialog** with one dropdown: the older document, chosen among the documents already
  open. The comparison is word-by-word (Myers).
- The result is a **Compare panel** in the right rail: a list of Inserted / Deleted / Replaced runs with their text; the
  differences are **shaded on the new document's pages** (one view, not two panes); there are actions to write the
  report as a new PDF and to put the differences on the document as annotations.

## Where the variants go

- **a-stage** follows Acrobat: a deliberate setup page (two file slots, swap, options, Compare), then summary + two panes
  + change list. It departs from Acrobat by being the Inkwell Compare view (flat rows, the version strip, light and
  rounded), by offering "this document" and "a past version" as first-class choices next to "Another file…", and by
  keeping the summary as a slim strip of counts above the panes rather than a page of its own.
- **b-rail** follows pdfcraft: a small dialog, then **one document view with the differences shaded on it** and a right
  rail. It departs from pdfcraft by using Inkwell's rail (the same dock as Comments, Redact, History), a three-way
  "Changes | Old | New" peek instead of a second document, and a summary of counts as the rail header.
- **c-tape** departs from both: setup and result on **one sheet**, no dialog, no page of its own. Two slim file lines at the
  top (Old, New) with the same pick menu, an overlay of the two documents in the middle, and a **change tape**, a thin
  vertical strip beside the page where every page is a tick and every difference a small mark, so a 500-page comparison is
  read at a glance and jumped through; the list is a quiet text index beside it, not cards.

Idea: you are looking at two things that should be one; the surface says in one glance how different they are, where, and
of what kind. For: a person comparing a contract draft with the signed version, or a paper with its revision, alone.
Feel: calm, precise, paper-like; the counts are the headline.
Palette: as the system (window #F4F2ED, surfaces #FFFFFF, panel #FAF8F4, text #1A1C20, secondary #6B675F, borders #E3DED4,
accent #2E4B8F for the selected mode, selection and the primary button). Change marks only: added green #2F7D4F with "+",
removed red #B3392F with "−", changed amber #B7791F with "~"; each always with its glyph and a word, never colour alone.
Type: Segoe UI Variable.
Motion (build): the row for the selected change scrolls into view and the pages scroll to it together (150 ms ease);
progress is a thin ink-blue bar under the toolbar with a "Cancel" text button; reduced motion: no easing.
Must not appear: grids of equal cards, glow, purple, gradients, glass, sparkles, emoji, people avatars, red PDF icons,
a dark theme, browser chrome, a second window.

## Shared

Render: one flat screenshot of one Windows 11 desktop application window, seen straight on, filling the whole
1536 x 1024 frame edge to edge. No board, no desktop background, no second window, no device frame, no perspective.
Crisp at 100 % scale like a real screenshot: large legible UI text (13-15 px UI labels, 16-18 px document body
text), sharp 1 px hairlines, sharp icon strokes, no blur, no compression smudge. Every word real, correctly spelled
English; never lorem ipsum, never squiggles standing in for text.
The app is Inkwell, a PDF app, matching the attached reference image's window, title bar, toolbar, palette, type and
rounded shapes exactly. Window: slim custom title bar with the ink-drop mark and "Inkwell", a "Home" tab, an active
rounded tab "Lease agreement.pdf", a "+" button, and the window controls. Under it the slim toolbar of the reference
with the history clock icon active and the label "Compare".
The two documents: "Lease agreement v1" (older, 2 Oct 2026) and "Lease agreement v2" (newer, today 14:32), residential
lease agreements of 4 and 5 pages. Page 2 reads: heading "2. PARTIES"; "This Residential Lease Agreement is made on 1
October 2026 between Maple Property Management AB (the Landlord) and the Tenant named below."; "2.3 Rent": v1 "The rent
is 12 400 SEK per month, due on the 25th of each month." and v2 "The rent is 12 900 SEK per month, due on the 1st of each
month."; "2.4 Notice": v1 "Either party may end this agreement with three months' written notice." and v2 "... with two
months' written notice.".
The summary, the same in every image that shows it: **12 differences**: Text 6, Formatting 2, Images 1, Pages 2 (one
added, one moved), Annotations 1. Marks on the pages: green for added, red for removed, amber for changed, each with the
glyph + − ~ in the change list.
Change list rows (use these, in this order, wherever a list is shown): "~ Rent changed · p. 2" "12 400 SEK → 12 900 SEK";
"~ Payment day changed · p. 2" "25th → 1st"; "~ Notice period changed · p. 2" "three months → two months"; "+ Clause added
· p. 3" "2.5 Deposit"; "~ Heading font changed · p. 2" "Segoe UI 12 pt → Georgia 12 pt"; "+ Page added · p. 5" "Signature
page"; "~ Logo replaced · p. 1"; "~ Page moved · p. 4 → p. 3"; "+ Comment added · p. 2".
Shape language: rounded and smooth (8-12 px radii, pill fields, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; panel #FAF8F4; text #1A1C20; secondary #6B675F; borders #E3DED4; one UI
accent ink blue #2E4B8F only for the selected mode, selection and the primary button; change marks green #2F7D4F, red
#B3392F, amber #B7791F only on differences, always with a glyph.
Type: Segoe UI Variable style humanist sans.
Must not appear: square corners, boxy grids of cards, bright blue, purple, gradients, glow, glass, sparkles, emoji, people
photos, dark theme, red PDF icons, browser chrome, device frame, watermark, lorem ipsum.

## Shared a-stage
Idea: Acrobat's setup page then its result, in the Inkwell Compare look. Setup: the page area is the window colour with
a calm centred composition, about 40 % empty ground. A heading "Compare files". Two large rounded slots side by side with
a round swap button between them: left "Old file" with a small page thumbnail, the file name "Lease agreement v1.pdf",
"4 pages · 2 Oct 2026" and a text link "Change"; right "New file" with its thumbnail, "Lease agreement v2.pdf",
"5 pages · today 14:32" and a link "Change". Under the left slot a segmented pill "This document | Past version |
Another file…" with "Past version" selected and a small menu hint "Signed by tenant · 2 Oct". Below, a row of three quiet
toggles "Text", "Formatting", "Images" on, and a text link "Page ranges". A primary ink-blue pill button "Compare" at the
bottom right of the slots and a quiet text button "Cancel".

## a-stage-1-setup
ref: ../../approved/history-2.png
The setup page as described, inside the Inkwell window (toolbar the same as the reference, the right side empty, no rail).

## a-stage-2-result
ref: ../../approved/history-2.png
Keep the reference's Compare view: segmented "Side by side | Slider | Overlay" with Side by side selected, "Show differences
on", Previous / "1 of 12" / Next. Add, directly under that bar, a slim summary strip: "12 differences" in semibold, then
five quiet count chips with glyph and word: "~ 6 Text", "~ 2 Formatting", "~ 1 Images", "+ 2 Pages", "+ 1 Annotations";
at the far right a rounded button "Export report" and a pill button "Done". Two panes with page 2 of each version, the
changed words marked (amber boxes) with a thin leader line between them; the Changes rail at the right lists the nine
rows from Shared, the first one selected with the ink-blue edge. The version strip at the bottom shows "Lease agreement
v1" and "v2" selected as a connected pair.

## Shared b-rail
Idea: pdfcraft's view in Inkwell's look: one document, differences shaded on it, a right rail. The page is large in the
centre, the new version (v2) page 2, with the changed words shaded amber and the added clause green. The right rail is
about 360 px wide on the panel colour, titled "Compare" with "12" beside it, a pill "Old: Lease agreement v1.pdf" with a
small chevron (the old file is chosen here), a row of five filter chips "All 12", "Text 6", "Formatting 2", "Images 1",
"Pages 2", then the change list grouped by page ("Page 2 · 5"), and at the bottom a quiet "Export report…" button and a
text link "Show old file".

## b-rail-1-setup
ref: ../../approved/history-2.png
The new document open normally (v2, page 2 centred, Read mode toolbar of the reference). In the centre of the window a small
rounded dialog with a soft shadow titled "Compare files": the label "Compare with", a dropdown field open showing three
options "Lease agreement v1.pdf · open document" (selected), "Past version · Signed by tenant · 2 Oct", "Another file…",
a quiet note "Text, formatting and images are compared; the differences are shown on this file.", a primary ink-blue pill
"Compare" and a text button "Cancel".

## b-rail-2-result
ref: ../../approved/history-2.png
The result as described. One row in the rail is selected (ink-blue left edge, very light blue tint): "~ Rent changed · p. 2".
A small floating pill at the top of the page: "Showing differences on the new file", with a toggle "Old | New".

## Shared c-tape
Idea: one sheet, and a tape. No setup page, no dialog. Under the toolbar, two slim lines instead of a form: "Old  Lease
agreement v1.pdf · 2 Oct 2026 · 4 pages" and "New  Lease agreement v2.pdf · today 14:32 · 5 pages", each with a quiet
"Change ▾" menu, and between them a small swap icon. At the right of these lines: the count "12 differences" in large
semibold with, in one quiet line of small text, "6 text · 2 formatting · 1 image · 2 pages · 1 annotation". The page
area shows the overlay of the two versions of page 2: unchanged text in ink grey, removed words in red with a strikethrough,
added words in green, changed formatting in amber, on one page. Left of the page, a **change tape**: a thin vertical strip
about 28 px wide running the full height of the page area, where each of the 5 pages is a small tick row with its number
and the differences are small coloured marks (red, green, amber) at their positions; page 2 is highlighted. Right of the
page, a quiet text index (no cards): the nine rows from Shared as plain lines with the glyph, grouped by page, the active
line in ink blue.

## c-tape-1-setup
ref: ../../approved/history-2.png
The sheet before anything is compared: the two lines at the top, the "New" line filled with Lease agreement v2.pdf and the
"Old" line showing an empty slot "Choose the file to compare against" with a small menu open beneath it: "This document's
past versions", "Another file…", "Recent". Below, the page area shows only page 2 of v2, calm, no marks, no tape; a
primary ink-blue pill "Compare" sits at the right of the two lines.

## c-tape-2-result
ref: ../../approved/history-2.png
The compared sheet as described, with the tape and the text index. A thin ink-blue progress line is not shown (finished).
At the bottom right a rounded button "Export report".
