# Organize r1: the Organize pages view

First round for this surface. It continues the approved look of home (approved/home.png, d-mix) and the assistant
(approved/app-assistant*.png): a real Windows 11 desktop app, rounded and smooth, warm palette, one ink-blue accent.
Function from the Acrobat study (docs/study/03-organize.md): full-window page grid, multi-select with a "Pages
selected" range field (`1, 3-5`), rotate left/right, delete, extract, insert (blank / from file), split, merge;
drag to reorder with an insertion bar; undo/redo. Inkwell does better: no confirm modal on delete (undo exists),
actions appear where the selection is.

Idea: your document laid out on a table as pages you can pick up, turn and rearrange.
For: someone reordering a scanned contract or pulling pages out of a report. Feel: calm, direct, tactile, native.
Palette/type/shape: as home-r2 (window #F4F2ED, surfaces #FFFFFF, sidebar #ECE9E2, text #1A1C20, secondary #6B675F,
borders #E3DED4, accent #2E4B8F). Document: "Lease agreement.pdf", 12 pages (a contract: cover, clauses, a signature
page, two scanned appendix pages that are slightly grey and one rotated sideways).
Motion (for the build): pages lift with a soft shadow when dragged, neighbours slide apart 160 ms to open the gap,
rotate animates 200 ms, deleted pages shrink and fade with an "Undo" toast. Reduced motion: no movement.
Must not appear: sharp corners, red PDF icons, purple/indigo/violet, gradients, glow, sparkles, emoji, people,
browser chrome, device frames.

Verdict (2026-10-10): a-panel. User: "Take a-panel". Build it (approved/organize.png). b-floating render recovered from Codex output (c9 of 11 candidates).

## Shared

Render: a flat screenshot of one Windows 11 desktop application window, 1440 x 960, seen straight on, filling the
image with a thin margin of plain desktop around it. It is the "Organize pages" view of a PDF app called Inkwell,
matching the attached reference image's window, title bar, palette, type and rounded shapes exactly.
Window: slim custom title bar with the ink-drop mark and "Inkwell", a "Home" tab, an active rounded tab
"Lease agreement.pdf", a "+" button, and the window controls. Under it a slim toolbar with rounded icon buttons and,
on the left, a rounded segmented pill "Read | Organize" with Organize selected; undo and redo icons; on the right a
rounded ink-blue "Done" button.
Content: twelve page thumbnails of a lease contract on white paper with rounded corners and soft shadows, each with
its page number in a small rounded badge under it: a cover page, clause pages with dense text, a signature page,
two greyish scanned appendix pages, one of them turned sideways. Pages 3 and 4 are selected: a 2 px ink-blue rounded
outline and a small round ink-blue check badge at their top right corner.
Shape language: everything rounded and smooth (8-12 px radii, pill fields, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; sidebar #ECE9E2; text #1A1C20; secondary #6B675F; borders #E3DED4; one
accent ink blue #2E4B8F only for selection and the primary button. No other hues.
Type: Segoe UI Variable style humanist sans, small and crisp like a polished native app.
Must not appear: square corners, boxy tables, purple, gradients, glow, glass, sparkles, emoji, people, red PDF
icons, browser chrome, device frame, watermark, extra text.

## a-panel
ref: ../../approved/home.png
Like a modernised Acrobat Organize: a left panel about 260 px wide on the sidebar colour with rounded groups:
"Pages selected" with a pill text field showing "3-4"; a row of four round icon buttons (rotate left, rotate right,
delete, extract); rounded list buttons "Insert pages", "Split", "Merge files", "Replace"; small secondary text
"2 of 12 pages selected". Main area: the twelve thumbnails in a calm 6-column grid with generous gaps.

## b-floating
ref: ../../approved/home.png
No side panel: the twelve thumbnails fill the window in a 6-column grid with generous gaps. Because pages 3 and 4 are
selected, a floating rounded action pill hovers at the bottom centre with a soft shadow, like a native selection
toolbar: "2 selected" in secondary text, then icon-and-label buttons "Rotate", "Extract", "Duplicate", "Delete", and
a "..." button. A slim ink-blue rounded vertical insertion bar between pages 8 and 9 shows a page being dragged
there, the dragged page lifted slightly with a bigger shadow, tilted a few degrees.

## c-strip
ref: ../../approved/home.png
A split layout like Preview or PDF Expert's page editing: a left column about 300 px wide with the pages as a
two-column grid of smaller thumbnails (3 and 4 selected); the right side shows selected page 3 large on white paper
with a soft shadow, with a small rounded floating toolbar above it (rotate left, rotate right, extract, delete) and,
under the page, "Page 3 of 12 · 8.5 x 11 in" in secondary text. A rounded "Insert" pill button with a "+" sits
between thumbnails in the left column where the mouse hovers.
