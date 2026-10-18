# Tools r2: Compress and OCR, legible

Round 1 (tools-r1) could not be judged (2026-10-10). Cause:
each render packed four app windows into one 1536x1024 board, so each screen was about 700 px wide and the UI text was
unreadable. Same three ideas, nothing else changed; only the render changes:
- ONE app window per image, filling the whole 1536x1024 frame. No board, no grid, no captions, no desk ground.
- Two images per variant: the compress OPTIONS state (presets with predicted sizes) and the OCR PROGRESS state (pages ticking).
  The start and result states are described in the notes below for the build and are not rendered.
- Large crisp UI text, sharp hairlines, realistic copy.

Verdict (2026-10-11): b-rail approved, after comparing with Acrobat's left-panel tool list and the PDF Optimizer modal.
Picks: b-rail-1-compress-options and b-rail-2-ocr-progress.
Build notes: the rail takes the assistant dock's width and chrome and the app's colour tokens (not the render's brighter blue);
only one right-side panel is open at a time (opening Tools closes the assistant and the other way round; a running tool keeps
the side). Starts: toolbar "Tools" menu, command palette, page context menu. Left out of the first build: the "Page range" field
on Compress (the engine has no page range) and "Customize" (the engine takes custom options; the rail has no editor for them yet).

Ideas (unchanged from r1):
- a-popover: a "Tools" toolbar menu opens one anchored white sheet that changes state in place (options, progress, result).
- b-rail: the tool is a 340 px right-hand rail beside the document; page thumbnails tick as pages finish. Start from a page's
  context menu or the command palette.
- c-bar: a floating bottom action bar that expands upward into a small popover; result inline.
Recommendation: b-rail.

For: someone shrinking a 40 MB scanned contract before emailing it, or making a scan searchable. Feel: calm, direct, native.
Palette/type/shape: as approved organize (window #F4F2ED, surfaces #FFFFFF, sidebar #ECE9E2, text #1A1C20, secondary #6B675F,
borders #E3DED4, accent #2E4B8F). Document: "Lease agreement.pdf", 12 pages, 8.4 MB (OCR: "Scanned contract.pdf", 12 pages).
Result copy for the build: compress "8.4 MB to 2.0 MB, saved 76%", "Looks the same: largest difference 4 of 255",
"Lease agreement (compressed).pdf", "Open compressed file" / "Done"; OCR "12 pages recognized", "10 recognized, 2 already had
text, 0 failed", "Undo", "Save as new file".
Motion (for the build): the sheet changes state with a 160 ms cross-fade and height ease; progress eases; result numbers count
up once. Reduced motion: no movement.
Must not appear: sharp corners, red PDF icons, purple/indigo/violet, gradients, glow, sparkles, emoji, people, browser chrome.

## Shared

Render: one single screenshot of one Windows 11 desktop app, a PDF app called Inkwell, at 1440 px wide scale, filling the
whole 1536 x 1024 image edge to edge. NOT a board, NOT a grid, NOT several windows, no captions, no ground around the window,
no device frame. The window has a slim custom title bar with a small ink-drop mark and one tab, a slim toolbar with rounded
icon buttons, a left page-thumbnail strip and the document page in the centre on a warm window. Match the attached reference
image's window, palette, type and rounded shapes exactly.
Legibility is the point of this image: all UI text is rendered large and crisp at 100% scale, 14 to 16 px body text with
sharp edges, hairline borders one pixel sharp, every word fully readable, nothing blurred, nothing tiny. The tool panel is
big enough to read from arm's length. Use exactly the quoted text and keep it short.
Shape language: everything rounded and smooth (8-12 px radii, pill fields, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; sidebar #ECE9E2; text #1A1C20; secondary #6B675F; borders #E3DED4; one accent
ink blue #2E4B8F only for the selected option, the progress bar and the primary button. No other hues.
Type: Segoe UI Variable style humanist sans, crisp like a polished native app; numbers in a tabular mono.
Must not appear: square corners, boxy tables, purple, gradients, glow, glass, sparkles, emoji, people, red PDF icons, browser
chrome, device frame, watermark, several windows, extra text.

## Shared a-popover
Idea of this variant: the window's toolbar has a "Tools" button; its tool sheet is one white rounded sheet anchored under
the toolbar, centred over the document, about 560 px wide (large in the frame), titled with its task, and it stays one sheet
through every state. Nothing else is open; the document is dimmed slightly behind it. The window touches all four edges of
the image: no desktop wallpaper, no blue swirl, no margin. The sheet has no icon or illustration, only its title and controls.
The document text behind the sheet is sharp, real, readable lease or contract wording.

## Shared b-rail
Idea of this variant: the tool lives in a right-hand rail docked beside the document, 460 px wide in this frame, on the
sidebar colour #ECE9E2, like an assistant panel. The document and its thumbnails stay visible to its left.

## Shared c-bar
Idea of this variant: no panel. A floating rounded white action bar hovers at the bottom centre of the document like a native
selection toolbar, about 760 px wide in this frame, and expands upward into a small popover only when options are needed.

## a-popover-1-compress-options
ref: ../../approved/organize.png
Document "Lease agreement.pdf" open. The sheet is titled "Compress file". Three tall rounded preset rows stacked, each with
the name on the left in medium weight, a one-line description below it in secondary colour, and the predicted size on the right
in mono: "Smallest" "Smaller files, softer images" "~0.9 MB"; "Balanced" "Good for email and sharing" "~2.1 MB"; "High quality"
"Keeps images sharp" "~4.0 MB". "Balanced" is selected with an ink-blue outline and a small ink-blue radio dot. Below the rows
one line in secondary colour: "Now 8.4 MB: images 7.2 MB, fonts 0.9 MB". Then a "Customize" text link on the left and a primary
ink-blue pill button "Compress" on the right.

## a-popover-2-ocr-progress
ref: ../../approved/organize.png
Document "Scanned contract.pdf" open, greyish scanned pages. The same sheet, titled "Recognize text", now showing progress:
a wide thin ink-blue progress bar at 58% on a pale track, the status line "Reading page 7 of 12" above it and "58%" in mono on
the right, a second line in secondary colour "Windows OCR, English", and a quiet outlined "Cancel" button at the bottom right.
Nothing else in the sheet. The tab's icon is a plain grey document glyph, not red.

## b-rail-1-compress-options
ref: ../../approved/organize.png
Document "Lease agreement.pdf" open with its page and the thumbnail strip on the left. The right rail is titled "Compress
file" with a small close button. Three tall rounded preset rows stacked, each with the name, a one-line description in secondary
colour and the predicted size on the right in mono: "Smallest" "Smaller files, softer images" "~0.9 MB"; "Balanced" "Good for
email and sharing" "~2.1 MB"; "High quality" "Keeps images sharp" "~4.0 MB". "Balanced" selected with an ink-blue outline.
Below them "Now 8.4 MB: images 7.2 MB, fonts 0.9 MB" in secondary colour and a "Page range" pill field showing "All pages".
At the rail's bottom a full-width primary ink-blue pill button "Compress".

## b-rail-2-ocr-progress
ref: ../../approved/organize.png
Document "Scanned contract.pdf" open. In the left thumbnail strip the first seven page thumbnails each show a small ink-blue
tick in the corner and the eighth has a thin spinner ring; the rest are plain. The right rail is titled "Recognize text":
a wide thin ink-blue progress bar at 58%, "Reading page 7 of 12" above it, "58%" in mono, "Windows OCR, English" in secondary
colour, and a quiet outlined "Cancel" button at the rail's bottom.

## c-bar-1-compress-options
ref: ../../approved/organize.png
Document "Lease agreement.pdf" fills the window, no side panel. At the bottom centre a floating rounded white action bar reads
"Compress   Balanced   ~2.1 MB" with a primary ink-blue pill "Compress" at its right end and a small close at its left. Expanded
above the bar, attached to it, a small rounded popover lists three rows in mono sizes: "Smallest" "~0.9 MB"; "Balanced" "~2.1 MB"
with an ink-blue check and outline; "High quality" "~4.0 MB". Text large and crisp.

## c-bar-2-ocr-progress
ref: renders/c-bar-1-compress-options.png
The attached image shows the window: make the same window (same title bar, same slim toolbar, no side panel, no page strip,
no Read/Organize switch, no Done button) with the same warm background, now with the document "Scanned contract.pdf" (a
greyish scanned page) filling the window. At the bottom centre the same floating rounded white action bar
now in progress: "Reading page 7 of 12" with "58%" in mono, a thin ink-blue progress line along the bottom edge of the bar at
58%, and a quiet outlined "Cancel" at the right. No popover open.
