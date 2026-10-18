# Tools r1: Compress and OCR

First round for this surface. Continues the approved look of home, organize and the assistant:
a real Windows 11 desktop app, rounded and smooth, warm palette, one ink-blue accent.
Function from the Acrobat study plus the engines on main:
- Compress: presets Smallest / Balanced / High quality, "Customize" for ppi/codec/strip options, an audit (images / fonts /
  content / overhead bytes) and a PREDICTED size per preset before running, progress with Cancel, result with before/after size,
  "saved 62%", a quality check (max diff / PSNR in plain words), warnings, "Open compressed file". Never overwrites; the new file is
  "<name> (compressed).pdf" next to the original.
- OCR: pages (All / This page / range `1-3, 5`), languages (installed Windows packs, Tesseract extras), engine (Windows, Tesseract),
  "Redo pages that already have text" toggle, progress with Cancel, result: pages recognized / skipped (already text) / failed,
  "Undo" because it is one undo step, "Save as new file" alternative.
Inkwell does better than Acrobat: one calm sheet, no modal stack, predicted size before running, undo for OCR.

Idea: a tool is a calm sheet over the document that changes state in place (options, progress, result) and never stacks dialogs.
For: someone shrinking a 40 MB scanned contract before emailing it, or making a scan searchable. Feel: calm, direct, native.
Palette/type/shape: as organize (window #F4F2ED, surfaces #FFFFFF, sidebar #ECE9E2, text #1A1C20, secondary #6B675F,
borders #E3DED4, accent #2E4B8F). Document: "Lease agreement.pdf", 12 pages, 8.4 MB (OCR flow: "Scanned contract.pdf", 12 pages).
Each variant is two boards (compress, ocr); each board shows four small states of the same window in a 2x2 grid:
1 where it starts, 2 options, 3 progress, 4 result.
Motion (for the build): the sheet changes state with a 160 ms cross-fade and height ease; progress bar eases; result numbers count up
once. Reduced motion: no movement.
Must not appear: sharp corners, red PDF icons, purple/indigo/violet, gradients, glow, sparkles, emoji, people, browser chrome, stacked modals.

## Shared

Render: a presentation board 1536 x 1024 on a plain #E9E6DF ground containing a 2 x 2 grid of four cropped screenshots of one
Windows 11 desktop app, a PDF app called Inkwell, each with a small mono caption above it ("1 START", "2 OPTIONS", "3 PROGRESS",
"4 RESULT"). The four panels show the same window (slim custom title bar with the ink-drop mark, a tab "Lease agreement.pdf",
a slim toolbar with rounded icon buttons, the page of a lease agreement on a warm window) in four successive states.
Match the attached reference image's window, palette, type and rounded shapes exactly.
Shape language: everything rounded and smooth (8-12 px radii, pill fields, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; sidebar #ECE9E2; text #1A1C20; secondary #6B675F; borders #E3DED4; one accent
ink blue #2E4B8F only for the selected option and the primary button. No other hues.
Type: Segoe UI Variable style humanist sans, small and crisp like a polished native app; numbers in a tabular mono.
Must not appear: square corners, boxy tables, purple, gradients, glow, glass, sparkles, emoji, people, red PDF icons, browser
chrome, device frame, watermark, stacked dialogs, extra text.

## Shared a-popover
Idea of this variant: a "Tools" button in the toolbar opens a menu (Compress file, Recognize text, Redact, ...). Choosing one
opens one white rounded sheet anchored under the toolbar, centred over the document, about 460 px wide, which stays the same
sheet through all states and never becomes a second dialog.
Panel 1 START: the Tools menu open under a "Tools" button, the task highlighted, small mono shortcut hints.
Panel 2 OPTIONS: the sheet titled with its task and its options (below).
Panel 3 PROGRESS: the same sheet, options replaced by a thin ink-blue progress bar, a status line, a quiet "Cancel" button.
Panel 4 RESULT: the same sheet showing the outcome (below).

## Shared b-rail
Idea of this variant: the tool lives in a right-hand rail docked beside the document, like the assistant panel, 340 px wide, on
the sidebar colour, so the document stays visible and the page thumbnails show which pages the tool will touch. Starting
points: a page's right-click context menu and the command palette (panel 1 shows both).
Panel 2 OPTIONS: the document on the left, the right rail titled with its task and its options (below), a primary button at the
rail's bottom.
Panel 3 PROGRESS: the rail shows a progress bar and a live line; the page thumbnails in the document gain a small ink-blue tick as
pages complete; "Cancel" below.
Panel 4 RESULT: the rail shows the outcome (below).

## Shared c-bar
Idea of this variant: no panel at all; a floating rounded action bar hovers at the bottom centre of the document like a native
selection toolbar and expands upwards into a small popover only when options are needed. Starting points: a "Tools" button
in the toolbar and the command palette.
Panel 1 START: a Tools menu dropdown open under the toolbar listing "Compress file" and "Recognize text", one highlighted.
Panel 2 OPTIONS: the floating bar with a one-line summary and a primary button, its option chip opened as a small popover above it.
Panel 3 PROGRESS: the same floating bar with a thin ink-blue progress line along its bottom edge, a status, "Cancel".
Panel 4 RESULT: the bar shows the outcome (below) with a button and a small close.

## a-popover-compress
ref: ../../approved/organize.png
Compress flow. Panel 1: "Compress file" highlighted. Panel 2: three rounded preset rows, each with a name, a one-line description
and the predicted size on the right in mono ("Smallest  ~0.9 MB", "Balanced  ~2.1 MB", "High quality  ~4.0 MB"), Balanced selected
with an ink-blue outline; a line "Now 8.4 MB: images 7.2 MB, fonts 0.9 MB"; a "Customize" text link; primary "Compress".
Panel 3: bar at 62%, "Optimizing images, page 8 of 12". Panel 4: "8.4 MB to 2.0 MB, saved 76%" large, "Looks the same: largest
difference 4 of 255", the new file name "Lease agreement (compressed).pdf", buttons "Open compressed file" (primary) and "Done".

## a-popover-ocr
ref: ../../approved/organize.png
Recognize-text flow on "Scanned contract.pdf" (greyish scanned pages). Panel 1: "Recognize text" highlighted. Panel 2: sheet
titled "Recognize text": segmented pill "All pages | This page | Range" with a field "1-12"; "Language" pill dropdown "English
(Windows)" with an extra chip "+ Swedish"; "Engine" segmented "Windows | Tesseract"; a switch "Redo pages that already have text";
primary "Recognize text". Panel 3: bar at 58%, "Reading page 7 of 12", "Cancel". Panel 4: "12 pages recognized", a small summary
"10 recognized, 2 already had text, 0 failed", "Undo" and "Save as new file" buttons and a quiet "Done".

## b-rail-compress
ref: ../../approved/organize.png
Compress flow. Panel 1: the command palette open at the top centre with "compress" typed and "Compress file" highlighted, and
beside it a page's context menu. Panel 2: rail with three preset rows with the predicted size in mono, "Balanced" selected in ink
blue; below, a "Page range" pill field "All pages". Panel 3: bar at 62%, "Page 8 of 12". Panel 4: before and after as two bars
(8.4 MB long, 2.0 MB short), "Saved 76%", primary "Open compressed file".

## b-rail-ocr
ref: ../../approved/organize.png
Recognize-text flow on "Scanned contract.pdf" in the right rail. Panel 1: page context menu with "Recognize text on this page"
highlighted and the command palette with "recognize" typed. Panel 2: rail with page range field, language dropdown "English
(Windows)", engine segmented "Windows | Tesseract", switch "Redo pages that already have text", primary "Recognize text".
Panel 3: progress with ticks on thumbnails. Panel 4: "10 recognized, 2 already had text, 0 failed", "Undo", "Save as new file".

## c-bar-compress
ref: ../../approved/organize.png
Compress flow. Panel 2: the bar reads "Compress  Balanced  ~2.1 MB" with a primary "Compress"; the popover above it lists
Smallest ~0.9 MB, Balanced ~2.1 MB, High quality ~4.0 MB in mono. Panel 3: "Optimizing 8 of 12", "Cancel". Panel 4:
"8.4 MB to 2.0 MB, saved 76%" with "Open compressed file" and a second line "Looks the same as the original".

## c-bar-ocr
ref: ../../approved/organize.png
Recognize-text flow on "Scanned contract.pdf". Panel 2: the bar reads "Recognize text  All pages  English" with primary
"Recognize"; a popover above it holds pages, language and engine. Panel 3: "Reading 7 of 12", "Cancel". Panel 4:
"12 pages recognized" with "Undo" and a quiet "Save as new file".
