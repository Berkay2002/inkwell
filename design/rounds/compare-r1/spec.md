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

2026-10-11: **a-stage.** "It is Acrobat's standard layout: side by side, with the change list on the right." b-rail and
c-tape were never rendered  and are dropped. Kept as our touches: the dashed connector lines between
matching changes, the kind chips that work as filters, and the linked page-thumbnail strips. Two rules: (1) Inkwell's real
navbar (Read / Organize and the rest) stays on screen; the render's Acrobat-style top bar (View / Annotate / Edit / Fill &
Sign) is not ours. (2) The setup screen is built from a-stage-1. Approved: approved/compare.png (a-stage-1-setup),
approved/compare-2.png (a-stage-2-result).

Built as: the setup page (Old file / New file slots with first-page thumbnail, "This document | Past version | Another
file..." under each, swap, "Compare content" switches Text / Formatting / Images, Cancel, Compare) and the result (modes
Side by side / Slider / Overlay, summary strip with chips as filters, change list at the right, connectors, linked strips).
Not built: "Page ranges" (the engine compares whole files) and a page count for past versions and picked files (no
engine call for it); the content switches set which kinds the result starts with, the engine still compares everything.

## How Acrobat Pro does it (study 13-compare, docs/study/screens/13-compare/)

Source: `docs/study/13-compare.md`, partly seen (landing screen, `screens/13-compare/01-landing.png`) and partly read from
Acrobat's shipped resources (result screen, report). Its result screen could not be driven, so the second half is wording
and structure, not observed behaviour.

- All tools → **Compare files**. The landing is a full-page form "Select Files to Compare": **Old File** and **New File**
  as two large first-page thumbnails with the file name, **swap** arrows between them, **Change File** and a "…" menu per
  file, a **Compare text only** checkbox, **Settings** and a blue **Compare** button.
- Settings: page range per file, document type (Auto / text flows / page to page / image only) and what to include
  (Text, Images, Headers/footers, Annotations, Formatting, Background) with Reset.
- Result: a **summary** page (total changes, Replacements / Insertions / Deletions, Styling and Annotations, "Go to first
  change") and two layouts: **side by side** with previous / next change, an include-changes filter and a change list
  panel, and an **overlay** with an old / new slider. Colours per kind (insert, delete, replace, move) with an opacity
  choice.
- Wording to borrow: `Font "A" changed to "B"`, size, colour, "Page inserted / deleted", a plain message for identical
  files, and a warning before a long comparison that offers page range or text only.
- Not seen: progress and cancel, the saved report, timings.

## How pdfcraft does it (github.com/storytold/pdfcraft @ b8ddf71: crates/compare, crates/ui-egui/src/compare_ui.rs)

- "Compare files…" opens a small **dialog** with one dropdown: the older document, chosen among the documents already
  open. The comparison is word-by-word (Myers).
- The result is a **Compare panel** in the right rail: a list of Inserted / Deleted / Replaced runs with their text; the
  differences are **shaded on the new document's pages** (one view, not two panes); there are actions to write the
  report as a new PDF and to put the differences on the document as annotations.

## Where the variants went

- **a-stage** (picked) follows Acrobat: a deliberate setup page (two file slots, swap, options, Compare), then summary + two
  panes + change list. It departs from Acrobat by being the Inkwell Compare view (flat rows, light and rounded), by offering
  "this document" and "a past version" as first-class choices next to "Another file...", and by keeping the summary as a
  slim strip of counts above the panes rather than a page of its own.
- b-rail (pdfcraft's one-document view with a right rail) and c-tape (one sheet with a change tape) were dropped unrendered.

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
