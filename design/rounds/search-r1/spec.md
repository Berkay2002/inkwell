# Search r1: find options, a results panel, and search across files

First round of the surface; there is no verdict to start from. Keeps design/README.md (Inkwell system: rounded, warm,
one ink-blue accent #2E4B8F, Segoe UI Variable, one right-side panel at a time in the assistant/Tools dock) and reopens
nothing. One app window per image at 1536x1024, two images per variant: `-1-find` (Ctrl+F on one document: the bar,
its options and the results) and `-2-files` (search across a folder: results grouped by file). The sheet is
`--cols 2`: one row per variant.

## Verdict

2026-10-11: pending. The image tool was at its usage limit (render.mjs: "Image generation failed because the usage
limit was reached", all six renders), so there are no renders and no sheet. Per the brief the worker builds the most
standard variant meanwhile: **a-rail** (Acrobat's floating find box with its "..." options, Advanced search as the
right rail in the shared dock, pdfcraft's context list). The rail and the find bar are separate components, so
b-side (results as a left-sidebar tab) or c-bar (a popover under the bar) stay cheap to switch to if inkwell picks
one of them from this text.

## How Acrobat Pro does it (study 09-viewer; docs/study/screens/09-viewer/11-find-ctrl-f.png, 12-find-results.png)
- Ctrl+F opens a small floating find box on the page: the field, previous and next arrows, a "..." menu for options
  (Whole words only, Case-sensitive, Include bookmarks, Include comments), and a close button. Matches are
  highlighted on the page and stepped with Enter. It lists no results.
- Edit > Advanced search (Shift+Ctrl+F) opens a separate window: scope (this document, or all PDFs in a folder), the
  word or phrase, more options (whole words, case, proximity, stemming, bookmarks, comments), and a results list
  grouped by file with a count and context snippets; double-click opens the file at the hit.

## How pdfcraft does it (E:\Dev\pdfcraft\crates\ui-egui\src\search_ui.rs, canvas.rs)
- A find bar with a "Find options" button (Whole words only, Case-sensitive) and, from Edit > Advanced search, a side
  panel: "What word or phrase would you like to search for?", the two check boxes, a count "12 instances", then
  every match as a button with its context ("...text before **match** text after..."), grouped under "Page 3"
  headings; click goes to the match. One document only, no folder search.

## Where the variants go
- **a-rail** follows Acrobat's floating find box and pdfcraft's context list: the box stays small and on the page;
  Advanced search is the right rail "Search" in the shared dock, one panel at a time. Departs from both by putting
  the scope (This file | Folder) and the history in the same rail, and showing progress and Cancel while it runs.
- **b-side** follows pdfcraft's sidebar idea more literally and VS Code's search: results live in the left sidebar
  next to Pages and Outline as a "Search" tab, so the page keeps its full width. Departs from Acrobat (no separate
  window) and keeps the find bar as one row in the toolbar area.
- **c-bar** departs from both: no panel. The find bar grows into a results popover anchored under it (like a command
  palette), options are small chips in the bar, and "Search in folder" opens one centred sheet with results grouped
  by file, as the Print sheet does.

Idea: finding is one gesture that grows only as far as you need: a word on the page, the list of every place it
occurs, then every file in a folder, with the count and progress always saying what is still running, and Esc
cancelling it. For: someone looking for a clause in one contract, or for an invoice number across a year of files.
Feel: calm, quick, paper-like.
Palette: as the system (window #F4F2ED, surfaces #FFFFFF, panel #FAF8F4, text #1A1C20, secondary #6B675F, borders
#E3DED4, accent #2E4B8F). The hit in a snippet is bold with a very pale amber wash (#FBEFC8); the current hit on the
page has a stronger amber wash (#F6D77A) and the others the pale one. Amber is a document overlay, not a UI hue.
Type: Segoe UI Variable.
Motion (build): the current hit scrolls into view with a 120 ms ease; the results list fills live while searching;
a thin ink-blue progress line under the bar. Reduced motion: no easing.
Must not appear: grids of cards, glow, purple, red, emoji, people avatars, sparkles, glass, a dark theme.

## Shared

Render: one flat screenshot of one Windows 11 desktop application window, seen straight on, filling the whole
1536 x 1024 frame edge to edge. No board, no desktop background, no second window, no device frame, no perspective.
Crisp at 100 % scale like a real screenshot: large legible UI text (13-15 px UI labels, 16-18 px document body
text), sharp 1 px hairlines, sharp icon strokes, no blur, no compression smudge. Every word real, correctly spelled
English; never lorem ipsum, never squiggles standing in for text.
The app is Inkwell, a PDF app, matching the attached reference image's window, title bar, palette, type and rounded
shapes exactly. Window: slim custom title bar with the ink-drop mark and "Inkwell", a "Home" tab, an active rounded
tab "Lease agreement.pdf", a "+" button, and the window controls. Under it a slim toolbar with a rounded segmented
pill "Read | Organize" with Read selected, page box "3 / 12", zoom "110%", and at the right a magnifier icon active.
Document: page 3 of a residential lease on white paper with a soft shadow. Heading "4. Termination and notice" and
three short paragraphs of real lease text in which the word "notice" appears five times, for example "Either party
may end this lease by giving written notice of at least thirty days before the end of a rental month." Every
"notice" on the page has a pale amber highlight; one is the current hit with a stronger amber wash.
Shape language: rounded and smooth (8-12 px radii, pill controls, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; panel #FAF8F4; text #1A1C20; secondary #6B675F; borders #E3DED4; one UI
accent ink blue #2E4B8F only for the selected mode, selection and primary button. No other UI hues, no red.
Type: Segoe UI Variable style humanist sans.
Must not appear: square corners, boxy grids of cards, red, bright blue, purple, gradients, glow, glass, sparkles,
emoji, people photos, dark theme, red PDF icons, browser chrome, device frame, watermark, lorem ipsum.

## Shared a-rail
Idea: Acrobat's floating find box on the page, with the full search as a right rail. The find box floats at the top
right of the page: a field with "notice", a count "3 of 12", up and down arrows, a "..." button and a close
button. The right rail, about 360 px wide on the panel colour, titled "Search" with a close button.

## a-rail-1-find
ref: ../../approved/comments.png
The "..." menu of the find box is open as a rounded popover with check rows: "Match case", "Whole words" (checked),
"Ignore accents", a divider, "Include comments" (checked), "Include bookmarks", "Include form fields", a divider
and "Advanced search…". The right rail shows: a search field with "notice" and a small clock icon (history), a
segmented control "This file | Folder" with This file selected, a line "12 results on 5 pages", then groups "Page 2"
(1), "Page 3" (5), "Page 7" (4), "Page 9" (2), each result a row of two lines of secondary text with "notice" in
bold on a pale amber wash. The row of the current hit has an ink-blue 2 px left edge and a very light blue tint.

## a-rail-2-files
edit: a-rail-1-find.png
Keep this screenshot exactly: same window, tabs, toolbar, page and rail frame. Change only this: the "..." popover
is closed. In the rail "Folder" is selected and under it a row "Documents\Housing" with a "Change" text button and a
small switch "Include sub-folders" on. A thin ink-blue progress line and the text "Searching 38 of 64 files" with a
quiet "Cancel" button. Below, results grouped by file: "Lease agreement.pdf" (12), "Insurance policy.pdf" (4),
"Move-out checklist.pdf" (2), each with a small file icon, its folder in secondary text and two result rows with
"Page 3", the snippet with "notice" in bold on pale amber, and a "Show 10 more" text link under the first group.

## Shared b-side
Idea: results in the left sidebar. The left sidebar is 320 px wide on the panel colour with a row of three tabs
"Pages", "Outline" and "Search" (Search selected, underlined in ink blue). A find bar is one slim row under the
toolbar spanning the page area: a field "notice", "3 of 12", up and down arrows, small chips "Aa", "ab", "é" for the
options (the "ab" chip, whole words, is on), and a close button. No right rail.

## b-side-1-find
ref: ../../approved/comments.png
The sidebar Search tab shows "12 results on 5 pages" and result groups "Page 2" (1), "Page 3" (5), "Page 7" (4),
"Page 9" (2) with context snippets, "notice" bold on pale amber; the current result has an ink-blue left edge. The
page area is wide with the same highlighted hits.

## b-side-2-files
edit: b-side-1-find.png
Keep this screenshot exactly: same window, toolbar, page and find bar. Change only this: the sidebar Search tab
starts with a small segmented control "This file | Folder" with Folder selected, the folder line
"Documents\Housing" with a "Change" button, a thin ink-blue progress line, "Searching 38 of 64 files" and "Cancel";
then a tree of files "Lease agreement.pdf" (12), "Insurance policy.pdf" (4), "Move-out checklist.pdf" (2),
each expandable, the first expanded with three snippet rows and "Page 3" labels.

## Shared c-bar
Idea: the bar grows. No panel anywhere: only the page. The find bar floats at the top centre of the page, wide
(560 px): a field "notice", chips "Aa", "ab", "é", "3 of 12", up and down arrows, a funnel-less "..." button and a
close button.

## c-bar-1-find
ref: ../../approved/comments.png
Directly under the bar a rounded white results popover (560 px wide, about 360 px tall, soft shadow) lists the
results grouped by page, "Page 2", "Page 3", "Page 7" with context snippets, "notice" bold on pale amber; the
current result has an ink-blue left edge. At its foot a quiet line "12 results on 5 pages" and a text button
"Search in folder…". The page behind is slightly dimmed only below the popover.

## c-bar-2-files
edit: c-bar-1-find.png
Keep this screenshot exactly: same window, page and bar. Change only this: the popover is replaced by a centred
rounded sheet (760 px wide) titled "Search in folder": a field "notice", a folder row "Documents\Housing" with
"Change", a switch "Include sub-folders", and under it results grouped by file ("Lease agreement.pdf" 12, "Insurance
policy.pdf" 4, "Move-out checklist.pdf" 2) with snippets and page labels; a progress line "Searching 38 of 64 files"
with "Cancel"; the page behind is dimmed.
