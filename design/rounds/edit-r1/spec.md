# Edit r1: editing text in place (phase 4)

First round of the surface; there is no verdict to start from. It keeps the Inkwell system from `design/README.md`
(rounded, warm, one ink-blue accent, Segoe UI Variable) and the recorded dock: tools sit in one right-side panel at
a time (assistant, Tools, Redact, Comments). The engine for this (text_blocks, text_edit) is on main from PR #1; this
round is only how a person reaches it. One app window per image at 1536x1024, one image per variant.

## Verdict

2026-10-11: a-rail, chosen from this spec alone (no renders exist): Acrobat's standard flow in our right rail. Taken from c-proof: the one-line summary of what is being
edited at the top of the format section ("Editing "Sans-serif line…" · CMSS12 · 1 line"), and the "On page N" block list
(cheap: it is the page's block list, with locked blocks tagged). b-bar was not built. Built screens (light and dark):
docs/screens/edit/. Build notes: docs/text-edit.md "The UI".

## How Acrobat Pro does it (study 02-edit: docs/study/02-edit.md, screens in docs/study/screens/02-edit/)
- A top tab **Edit** opens a left panel; the page's text blocks and images get dotted outlines at once
  (`05-edit-mode-text-blocks.png`). All tools, "Edit a PDF", opens the same panel.
- The panel has **Modify page**, **Add content** (Text, Image, Header and footer, More) and, only while a text block is
  selected, **Format text**: font, size, colour, lists, alignment, bold/italic/underline, super/subscript, line spacing
  (1.2), paragraph spacing, horizontal scale, character spacing (`01-edit-panel.png`).
- Click into text: a frame with 8 resize handles and a rotate handle, caret at the click, typing re-flows the paragraph
  inside the frame width (`07-paragraph-reflow.png`). A yellow `!` appears on the block when the original font cannot
  show a typed character, and that character falls back to a system font (`03-edit-typed-ZZ.png`).
- Add content, Text: the cursor becomes a text cursor, click the page and type; the box gets the default font
  (Minion Pro 12 pt) (`06-add-text-typed.png`). Esc or clicking away commits. Ctrl+S saves in place.
- It never says what a rewrite will do to the file until it is done.

## How pdfcraft does it (github.com/storytold/pdfcraft @ b8ddf71: crates/ui-egui/src/edit_text_ui.rs)
- "Edit text & images": boxes around the paragraphs and images already on the page. Click a paragraph to edit it in
  place; Ctrl+Enter or clicking away applies and rewraps it to the box, Esc cancels. Drag the paragraph to move it,
  drag the handle on its left or right edge to rewrap it to a new width.
- A **Format text** panel in the side panel while a paragraph is open: family (Helvetica, Times, Courier), bold,
  italic, size, colour, alignment, underline, line spacing, character spacing, horizontal scale.
- Images: click to select, drag to move, drag a corner to resize, right-click for rotate, flip, replace, save, delete.

## Where the variants go
- **a-rail** follows Acrobat (Edit as a mode, dotted outlines, a Format text section that appears with a selection,
  Add content with Text and Image) and departs by living in Inkwell's own right rail, calmer and with the engine's
  honest font line ("Arial, kept" or "Helvetica, because...").
- **b-bar** follows pdfcraft's in-place feel: no panel at all, a slim floating tool strip like the comment tools and a
  format bar that hangs over the selected block (size, colour, alignment). The page stays as wide as it can be.
- **c-proof** departs from both: the rail leads with what the edit will do before it is done (the paragraph, its
  font kept or replaced and why, whether it reflows), and lists every block on the page with the ones that cannot be
  edited named and explained. Idea: you know what will happen to the file before you click away.

Image editing is not built yet (its engine is still being written). Every variant leaves a visible, quiet slot for it
("Image" next to "Text", dimmed, "coming soon"), so it drops in without moving anything.

Idea: type over the page and have it stay a page. For: someone fixing a date or a figure in a contract without
rebuilding it. Feel: calm, exact, no surprises.
Palette: as the system (window #F4F2ED, surfaces #FFFFFF, panel #FAF8F4, text #1A1C20, secondary #6B675F, borders
#E3DED4, accent #2E4B8F). Block outlines are 1 px dotted ink blue at about 45 % opacity; the selected block is a solid
1.5 px ink-blue frame with 8 small white square handles.
Type: Segoe UI Variable. The document's own text is Helvetica-like in document colour.
Motion (build): none beyond hover outlines fading in over 80 ms. Reduced motion: no fade.
Must not appear: dense Acrobat-style small type, a ribbon, grids of cards, glow, purple, red, emoji, sparkles, glass,
a dark theme. The only warm hue allowed is a small amber "!" for a font that was replaced.

## Shared

Render: one flat screenshot of one Windows 11 desktop application window, seen straight on, filling the whole
1536 x 1024 frame edge to edge. No board, no desktop background, no second window, no device frame, no perspective.
Crisp at 100 % scale like a real screenshot: large legible UI text (13-15 px UI labels), sharp 1 px hairlines, sharp
icon strokes, no blur, no compression smudge. Every word real, correctly spelled English; never lorem ipsum, never
squiggles standing in for text.
The app is Inkwell, a PDF app, matching the attached reference image's window, title bar, palette, type and rounded
shapes exactly. Window: slim custom title bar with the ink-drop mark and "Inkwell", a "Home" tab, an active rounded
tab "Lease agreement.pdf", a "+" button, and the window controls. Under it a slim toolbar with a rounded segmented
pill "Read | Edit | Organize" with "Edit" selected in ink blue, undo and redo icons, page box "2 / 12", zoom "Fit width".
The thumbnail sidebar stays at the left.
Document: a residential lease agreement, 12 pages, on white paper, page 2 shown at fit width: heading "2. PARTIES",
the sentence "This Residential Lease Agreement is made on 1 October 2026 between Maple Property Management AB (the
Landlord) and the Tenant named below.", a short "Tenant" block with Name, E-mail and Phone lines, then heading
"2.3 Rent" and the paragraph "The rent is 12 400 SEK per month, due on the 25th of each month. Late payment bears
interest as set out in clause 7." Edit mode is on: every block of text on the page has a faint dotted ink-blue outline.
The paragraph under "2.3 Rent" is selected: a solid 1.5 px ink-blue frame with 8 small white square handles and a text
caret after "month," inside it.
Shape language: rounded and smooth (8-12 px radii, pill fields, soft diffuse shadows), no sharp corners except the
page's own outlines.
Palette: window #F4F2ED; surfaces #FFFFFF; panel #FAF8F4; text #1A1C20; secondary #6B675F; borders #E3DED4; one UI
accent ink blue #2E4B8F only for the selected mode, the selection frame and the primary button. No other UI hues
except one small amber "!" where stated; no red.
Type: Segoe UI Variable style humanist sans.
Must not appear: square corners on UI, red, bright blue, purple, gradients, glow, glass, sparkles, emoji, people
photos, dark theme, red PDF icons, browser chrome, device frame, watermark, lorem ipsum.

## a-rail-1-edit
ref: ../../approved/comments.png
The right rail, about 320 px wide on the panel colour, same dock as the Comments rail, titled "Edit" with a close icon.
Top section "Add content": two rounded buttons side by side, "Text" (with a text icon) and "Image" (dimmed, with a
small secondary "soon"). Below, a section "Format text" that is open because a block is selected: a read-only line
"Font" with "Arial" and a small secondary "kept from the document"; "Size" with a small stepper showing "11 pt";
"Colour" with a dark swatch; "Align" as a four-part segmented control (left selected, centre, right, justify). Under
it a quiet line "Changes apply when you click away. Undo with Ctrl+Z." The page shows the dotted outlines and the
selected block as in Shared.

## b-bar-1-edit
ref: ../../approved/comments.png
No right rail: the page fills the width. A slim floating tool strip, rounded, over the left of the page area like the
comment tool strip, with three icon buttons and labels on hover: select text (active, ink blue), add text, add image
(dimmed). Directly above the selected paragraph hangs a small rounded floating format bar with a soft shadow:
"Arial" (a read-only chip), "- 11 +", a colour swatch, and four alignment icons with the left one on, then a thin
divider and a small "Done" text button. The page shows the dotted outlines and the selected block as in Shared.

## c-proof-1-edit
ref: ../../approved/comments.png
The right rail, about 320 px wide on the panel colour, titled "Edit text" with a close icon. First a rounded card with
the outcome stated in words: "Editing 1 paragraph on page 2", under it three small secondary lines with check
marks: "Arial is kept", "Reflows within the box width", "One undo step". Then the format controls in one calm column:
"Size 11 pt" stepper, "Colour" swatch, "Align" four-part segmented control. Under that a list headed "On this page,
9 blocks": rows with the first words of each block ("2. PARTIES", "This Residential Lease Agreement is made...",
"Tenant", "2.3 Rent", "The rent is 12 400 SEK per month..." selected), and one row at the bottom in secondary text
"Stamp text: part of a template" with a small lock icon and the words "can't be edited here". At the very top of the
list a dimmed "Add text" and "Add image (soon)". The page shows the dotted outlines and the selected block as in Shared.
