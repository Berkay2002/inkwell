# Links r1: links and bookmarks inside Edit mode (phase 5)

First round of the surface; there is no verdict to start from. It keeps the Inkwell system from `design/README.md`
(rounded, warm, one ink-blue accent, Segoe UI Variable) and the recorded decision that tools live in a right rail, one
at a time. Edit mode already has a rail ("Edit": text blocks, `design/rounds/edit-r1`); links and bookmarks join it.
One app window per image at 1536x1024. Each variant has one image (`-1-links`); a-tabs and b-canvas have a second
(`-2-bookmarks`). The sheet is `--cols 2`.

## Verdict

2026-10-11: **a-tabs, with two touches.** The user delegated design picks to the orchestrator; the pick below is the
orchestrator's ("inkwell [7da248]"), adopted by the user through that delegation, not the user's own. In its words:
"a-tabs. Keep building it, with two touches. 1. From b-canvas: Acrobat keeps bookmarks in the left navigation pane, so
make the left sidebar outline editable as well. Rename in place, drag to reorder or nest, and a right-click menu for
Add, Delete and Set destination. Both views share the nav store and each change is one undo step. 2. From c-ledger:
in the Links tab, give each link a state dot: ok, broken destination (missing page or named destination), or external
URL. Do the same for bookmarks with a dead destination, and add a "Show problems" filter."
b-canvas and c-ledger are dropped. Built as the Links and Bookmarks tabs of the Edit rail (`src/components/edit/`),
the editable outline in the left sidebar while Edit is open (`BookmarkTree`, one component for both), state dots
(a muted dot: goes somewhere; a ring: web address; a diamond: broken; a dashed ring: goes nowhere), a "Problems"
filter on the bookmark tree and a document-wide "Show problems" check in the Links tab (`links_audit`). Build notes in
docs/bookmarks-links.md.

Render notes: written 2026-10-11 by the worker building the surface. Nothing was rendered: the image tool reported its
usage limit on all three first images (2026-10-11), so there is no sheet. The spec stands as text and the most
standard variant (a-tabs) is built; b-canvas and c-ledger are described only. Text in renders is not authoritative
(the build sets the copy).

## How Acrobat Pro does it (study 16-bookmarks-links: docs/study/16-bookmarks-links.md, screens in docs/study/screens/16-bookmarks-links/)
Only part of it could be observed (Acrobat's editing controls are not reachable without input); the rest is read from
its resources and marked as such in the study.
- **Bookmarks** live in the left navigation pane ("Bookmarks": a tree with chevrons, a New-bookmark icon, a
  Find-current icon and an options "..." menu). A closed group is a right chevron and hides its members; the state is
  the sign of `/Count`. Bold, italic and colour from `/F` and `/C` are drawn in the row. Rows are tall (46 px) and long
  titles wrap to three lines.
- New bookmark, Rename, Delete, Set Destination, Properties (Appearance: text, colour, style; Actions: any action
  type), Show Location, Expand/Collapse top-level, Paste as child or next. Drag reorders and nests; invalid targets
  are refused. **New Bookmarks from Structure** needs a tagged PDF; there is no "from headings" command.
- **Links**: the Edit tab's Link tool. Drag a rectangle, then the **Create Link** dialog: Link Type = Visible or
  Invisible Rectangle, Color, Line Thickness, Line Style, Highlight Style (None, Invert, Outline, Inset); Link
  Action = Go to a page view, Open a file, Open a web page, Custom link. Existing links are selected on the page and
  edited through **Link Properties**. "Create Links from URLs..." and "Remove All Links..." work on the whole
  document and say nothing about how many beforehand.
- A visible link is drawn as a black 1 pt rectangle by default; width 0, `/C []` or a hidden flag draws nothing.
- Nothing was found that asks before a web address is opened.
- Every setting sits in modal dialogs with many tabs; no single place shows the document's bookmarks and the page's
  links together.

## How pdfcraft does it (E:\Dev\pdfcraft: bookmark and link code only; ideas, no code copied)
- Bookmarks: add under a parent, rename, delete, move (refuses a move into itself), set open/closed, set page "Set to
  current page (n)", indent and outdent, and build from the structure tree.
- Links: model Page, Uri or Other (named destinations and actions are kept as "Other"); minimum size 2 pt; links
  drawn by dragging; a small action dialog; the Delete key removes the selected link.
- An address is checked before use: https, http and mailto only, at most 2048 characters. A **confirmation dialog**
  is shown before a link is opened (pending_link).
- Appearance is a highlight, not a border.

## Where the variants go
- **a-tabs** follows Acrobat's pieces (a link tool with a visible/invisible rectangle, a bookmark tree with
  rename/delete/reorder) and departs by putting them in the *same* Edit rail as three tabs (Text | Links | Bookmarks)
  and by stating the outcome before any bulk change: "Bookmarks from headings" and "Links from web addresses" show
  how many they would make, with the first few, then make them as **one** undo step.
- **b-canvas** follows pdfcraft's direct manipulation: no tabs. A link is drawn on the page and a small popover hangs
  from the rectangle (where it goes; border); bookmarks are edited in the left sidebar's Outline tab, which gains an
  edit toolbar and drag handles. The right rail stays the text rail. Departs by having no panel for links at all.
- **c-ledger** departs from both: a "Links & bookmarks" board in the right rail that lists every link and bookmark of
  the document as rows in two groups ("Bookmarks 14", "Links 31"), each row showing where it goes with a state dot
  (opens a page / opens a web address / broken), a filter, and bulk actions at the top ("Make from headings",
  "Make from web addresses"). The page shows link outlines; clicking a row scrolls to it. Best for auditing a long
  document; slowest for adding one link.

Idea: you see what a link or bookmark will do, and how many a bulk action will make, before you do it.
For: someone finishing a report, manual or thesis who has to make its navigation work. Feel: calm, exact.
Palette: as the system (window #F4F2ED, surfaces #FFFFFF, panel #FAF8F4, text #1A1C20, secondary #6B675F, borders
#E3DED4, accent #2E4B8F). Links on the page are drawn as thin outlines in the accent; the drawn one is dashed with a
faint accent fill. Page content is document colour.
Type: Segoe UI Variable.
Motion (build): none beyond a 120 ms fade of the popover/notice. Reduced motion: none.
Must not appear: dense Acrobat-style small type and nested tabbed dialogs, grids of cards, glow, purple, red, emoji,
sparkles, glass, a dark theme.

## Shared

Render: one flat screenshot of one Windows 11 desktop application window, seen straight on, filling the whole
1536 x 1024 frame edge to edge. No board, no desktop background, no second window, no device frame, no perspective.
Crisp at 100 % scale like a real screenshot: large legible UI text (13-15 px UI labels), sharp 1 px hairlines, sharp
icon strokes, no blur, no compression smudge. Every word real, correctly spelled English; never lorem ipsum, never
squiggles standing in for text.
The app is Inkwell, a PDF app, matching the attached reference image's window, title bar, palette, type and rounded
shapes exactly. Window: slim custom title bar with the ink-drop mark and "Inkwell", a "Home" tab, an active rounded
tab "Annual report.pdf", a "+" button, and the window controls. Under it a slim toolbar with a rounded segmented
pill "Read | Edit | Organize" with "Edit" selected, undo and redo icons.
Document: an annual report, 24 pages, on white paper, page 6 open: heading "2. Results" in large type, paragraph
"Revenue grew 12 % to 4.8 million SEK, driven by the new service plans. Details are on page 14.", a line "Read the
full tables at https://example.com/report" with the address underlined in the accent blue, and a heading "2.1 Costs".
Shape language: rounded and smooth (8-12 px radii, pill fields, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; panel #FAF8F4; text #1A1C20; secondary #6B675F; borders #E3DED4; one UI
accent ink blue #2E4B8F only for the selected mode, the selected tab and the primary button. No other UI hues, no red.
Type: Segoe UI Variable style humanist sans.
Must not appear: square corners, red, bright blue, purple, gradients, glow, glass, sparkles, emoji, people photos,
dark theme, red PDF icons, browser chrome, device frame, watermark, lorem ipsum.

## Shared a-tabs
Idea: one Edit rail, three tabs. The right rail about 320 px wide on the panel colour, titled "Edit" with a close icon;
under the title a segmented control "Text | Links | Bookmarks". On the page, the words "Details are on page 14" and the
address line have thin ink-blue dotted outlines (links), one of them selected with four corner handles.

## a-tabs-1-links
ref: ../../approved/comments.png
"Links" tab selected. Rail sections: "Add a link" with a button "Draw a link"; "Selected link" with a summary card
"Page 14 / Goes to a page of this document", buttons "Go there" and "Change target" and a trash icon, a row "Border"
with a checkbox "Visible"; "From the document" with a button "Links from web addresses" expanded to the card "3 links
on 2 pages, for addresses that are text only." with a list of two addresses and a primary button "Make 3 links";
"On page 6 · 2 links" with two rows "Page 14" and "https://example.com/report".

## a-tabs-2-bookmarks
edit: a-tabs-1-links.png
Keep this screenshot exactly: same window, page and rail frame. Change only this: the "Bookmarks" tab is selected and
the rail shows a toolbar (a "New" button with a plus, indent and outdent icons, a trash icon), a bordered tree: "1.
Introduction" with a chevron, "2. Results" selected and bold with children "2.1 Costs" and "2.2 Margins", "3. Outlook"
; under it a card "Selected bookmark": "2. Results / Goes to page 6", buttons "Set to current view" and "Rename", a row
"Style" with B and I toggles and five round colour swatches; at the bottom "From the document" with the button
"Bookmarks from headings".

## Shared b-canvas
Idea: no panel for links. The right rail is the plain Edit text rail. On the page the address line has a drawn dashed
rectangle with a faint accent fill and, hanging from its lower edge, a popover about 300 px wide: title "Link",
segmented "Page | Web address | File", a field "https://example.com/report" with the line "Opens https://example.com/
report" under it in secondary text, a checkbox "Show a border", and "Cancel" and a primary "Create link". The left
sidebar shows the Outline tab as an editable tree with small drag handles and a slim toolbar "New | Rename | Delete".

## b-canvas-1-links
ref: ../../approved/comments.png
The popover over the page as described in Shared b-canvas; the sidebar outline open at the left.

## b-canvas-2-bookmarks
edit: b-canvas-1-links.png
Keep this screenshot exactly. Change only this: no popover and no drawn rectangle; in the left sidebar the outline tree
shows "2. Results" being renamed in place (an input with the text selected) and a thin accent line between "2.1 Costs"
and "2.2 Margins" where a dragged row would land.

## Shared c-ledger
Idea: audit view. The right rail titled "Links & bookmarks", with two collapsible groups. "Bookmarks · 14" lists rows
"1. Introduction · p. 3", "2. Results · p. 6", "2.1 Costs · p. 8"; "Links · 31" lists rows "Page 14 · p. 6", "https://
example.com/report · p. 6", "mailto:info@example.com · p. 24" and one row "Target page missing" in secondary
text with a small warning dot. A filter field "Filter" under the title. At the top two quiet buttons "Make from
headings (9)" and "Make from web addresses (3)".

## c-ledger-1-ledger
ref: ../../approved/comments.png
The rail as in Shared c-ledger; on the page, the link outlines are visible and one is selected.
