# Comments r1: annotate and review (phase 3a)

First round of the surface; there is no verdict to start from. The design below follows `design/README.md`
(Inkwell system: rounded, warm, one ink-blue accent, Segoe UI Variable) and the redact and tools rails (one right rail
at a time, same width and chrome as the assistant rail). One app window per image at 1536x1024, two images per
variant: `-1-review` (a document with comments, the Comments panel) and `-2-select` (one annotation selected, with
handles and the small property popover). The sheet is `--cols 2`: one row per variant.

## Verdict

2026-10-11: a-rail. The user did not pick between the variants; they handed the design call to the orchestrator
(inkwell) in these words: "choose what is both standard and what pdfcraft and acrobat has, but also some hint of
uniqueness". The pick, a-rail, is **inkwell's, adopted by the user** by that delegation, not the user's own. It takes
Acrobat's rail (search, filter, composer, grouped by page, replies) and pdfcraft's floating vertical tool strip, with
the pale-blue selection frame and small property popover as its own touch. b-bar and c-margin are dropped.
Approved: `design/approved/comments.png` (a-rail-1-review), `comments-2.png` (a-rail-2-select).
Built as the Comments rail in the same dock as assistant, Tools, Redact and History; the strip floats at the left of
the page area. Screens of the build: `docs/screens/comments/`.
## How Acrobat Pro does it (study 08-comments, docs/study/screens/08-comments/01-comments-panel.png)
- Right rail, speech-bubble icon, opens a **Comments** panel titled with the count ("Comment 1").
- Panel header: close, search (magnifier), filter (funnel), "..." menu. Under it a composer ("Add a comment",
  cycling with "Add a suggestion", "Add a question"), then a tree grouped per page ("Page 1 ..... 1", collapsible).
- One card per annotation: round type avatar, author, contents, a Reply on hover. Replies are /IRT annotations.
- The tool strip is reached from All tools > Add comments; a small floating quick strip beside the page offers
  "Add a comment". Markup tools are also on the text-selection context menu. The capture is dark.
- The sticky note shows as the small yellow note icon at its rect.

## How pdfcraft does it (E:\Dev\pdfcraft\docs\images\pdfcraft-viewer.png)
- A **floating vertical tool strip** at the left of the page: select, hand, note, highlight, pen, eraser.
- A right-rail **Comments** panel (count in the title), grouped by page ("Page 1 .... 2"): each card a round type
  icon, author, "Type . date", contents, and replies indented under a thin left rule.
- Page-level tools stay in the main toolbar; the rail is toggled from a narrow icon column at the far right.

## Where the variants go
- **a-rail** follows pdfcraft (floating vertical strip, grouped rail) and Acrobat (search, filter, composer) and
  departs from both by using the Inkwell rail and a property popover attached to the selection.
- **b-bar** follows Acrobat's toolbar-strip idea: a comment toolbar as a slim second row under the main toolbar,
  with the rail on the right. It departs from Acrobat by being light and rounded, by opening the thread on the page
  and by putting the selection's properties in a popover on the page, not in a side dialog.
- **c-margin** departs from both: no rail frame; comments sit in a quiet margin beside the page on leader lines
  (a proofreader's margin, as redact b-margin), with the tools in a small floating pill. The grouped list (the rail)
  stays available as a toggle, because the brief needs it; the render shows the margin view.

Idea: one pen, one surface: you mark the page where it is, and what you wrote is listed in the order you read.
For: a person reviewing a contract or paper, alone or with a reviewer. Feel: calm, precise, paper-like.
Palette: as the system (window #F4F2ED, surfaces #FFFFFF, panel #FAF8F4, text #1A1C20, secondary #6B675F, borders
#E3DED4, accent #2E4B8F). Annotation colours on the page are real markup colours: highlight yellow #F6D860, green
#9BD3A4, pen in #2E4B8F, note icon amber #E0A93B. These are document marks, not UI.
Type: Segoe UI Variable.
Motion (build): a drawn stroke appears as it is drawn; selection handles ease in 100 ms; the rail card for the
selected annotation scrolls into view and takes a 2 px accent left edge. Reduced motion: no easing.
Must not appear: grids of cards, glow, purple, red, emoji, people avatars, sparkles, glass, a dark theme.

Verdict: (pending)

## Shared

Render: one flat screenshot of one Windows 11 desktop application window, seen straight on, filling the whole
1536 x 1024 frame edge to edge. No board, no desktop background, no second window, no device frame, no perspective.
Crisp at 100 % scale like a real screenshot: large legible UI text (13-15 px UI labels, 16-18 px document body
text), sharp 1 px hairlines, sharp icon strokes, no blur, no compression smudge. Every word real, correctly spelled
English; never lorem ipsum, never squiggles standing in for text.
The app is Inkwell, a PDF app, matching the attached reference image's window, title bar, palette, type and rounded
shapes exactly. Window: slim custom title bar with the ink-drop mark and "Inkwell", a "Home" tab, an active rounded
tab "Lease agreement.pdf", a "+" button, and the window controls. Under it a slim toolbar with a rounded segmented
pill "Read | Organize | Comment" with Comment selected, undo and redo icons.
Document: a residential lease agreement on white paper with a soft shadow, page 2, heading "2. PARTIES", sentences:
"This Residential Lease Agreement is made on 1 October 2026 between Maple Property Management AB (the Landlord) and the
Tenant named below." Then "2.3 Rent": "The rent is 12 400 SEK per month, due on the 25th of each month." and "2.4
Notice": "Either party may end this agreement with three months' written notice."
Annotations on the page: the words "12 400 SEK per month" highlighted in yellow (#F6D860, translucent, text still
readable); "three months' written notice" underlined in green with a wavy line; a small amber sticky-note icon in the
right margin of the page at the rent sentence; a rectangular text box with the words "Check with the landlord"
over empty space; one freehand blue pen circle around "25th".
Comment content (where shown): author "Alex Johnson" on every card, small type label and time like "Highlight · 14:32";
texts: "Is this the final rent?" (note on the rent, with one reply by "Maria Lind": "Yes, confirmed by mail on 2
October."), "Check with the landlord" (text box), "Which date counts?" (pen circle on the 25th), and the highlight and
underline cards with no text. Grouped by page: "Page 2 · 5".
Shape language: rounded and smooth (8-12 px radii, pill fields, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; panel #FAF8F4; text #1A1C20; secondary #6B675F; borders #E3DED4; one UI
accent ink blue #2E4B8F only for the selected mode, selection and primary button. No other UI hues, no red.
Type: Segoe UI Variable style humanist sans.
Must not appear: square corners, boxy grids of cards, red, bright blue, purple, gradients, glow, glass, sparkles,
emoji, people photos, dark theme, red PDF icons, browser chrome, device frame, watermark, lorem ipsum.

## Shared a-rail
Idea: pdfcraft's layout, in Inkwell's look. A small floating vertical rounded strip at the left of the page with
eight tools: select (active, ink blue), highlight, underline, note, text box, pen, shapes, stamp. The right rail,
about 360 px wide on the panel colour, titled "Comments" with "5" beside it, a magnifier and a funnel icon, a pill
composer "Add a comment", then a group "Page 2" with count 5.

## a-rail-1-review
ref: ../../approved/redact-review.png
The page is large in the centre with the annotations. The rail lists the five cards as described, the note card with
its reply indented under a thin rule, and a quiet "Reply" text link under it. Nothing is selected.

## a-rail-2-select
edit: a-rail-1-review.png
Keep this screenshot exactly: same window, tabs, toolbar, strip, page, rail and type. Change only this: the yellow
highlight is selected: thin ink-blue handles at the ends of the highlight and a small rounded white popover just
above it with five colour dots (yellow active, green, blue, orange, grey), a slim opacity slider, and a small round
trash icon. In the rail, the matching card has an ink-blue 2 px left edge and a very light blue tint.

## Shared b-bar
Idea: Acrobat's toolbar strip, light and rounded. In Comment mode a slim second toolbar row under the main toolbar
carries, left to right, quiet icon + label buttons: "Select", "Highlight" with a small chevron, "Underline",
"Note", "Text", "Pen", "Shapes" with chevron, "Stamp" with chevron, a divider, then on the right "Author: Alex
Johnson". The right rail is the Comments panel (360 px) with the header, a pill search, filter chips "All",
"Notes", "Markup", "Resolved", and the page group.

## b-bar-1-review
ref: ../../approved/redact-review.png
The page in the centre with the annotations. The note at the rent sentence is open as a small rounded floating
thread on the page, to the right of the note icon, showing "Alex Johnson" with "Is this the final rent?" and
"Maria Lind" with "Yes, confirmed by mail on 2 October." and a reply field "Reply". The rail lists the cards,
the note card active.

## b-bar-2-select
edit: b-bar-1-review.png
Keep this screenshot exactly: same window, toolbar rows, page, rail and type. Change only this: the thread popover
is closed, and the pen circle around "25th" is selected: a dashed ink-blue rectangle around it with eight small
round handles, and a rounded property popover below with a row of colour dots, a thickness slider with the value
"2 pt", and a trash icon. The rail's "Which date counts?" card has an ink-blue left edge.

## Shared c-margin
Idea: a proofreader's margin. The page is slightly left of centre; the window's right side is a wide quiet margin on
the window colour (no rail frame). A small floating rounded tool pill at the top centre of the page: select,
highlight, note, text, pen, shapes, stamp, and a count pill "5 comments" with a funnel. Thin charcoal leader lines
run from each annotation to a note in the margin.

## c-margin-1-review
ref: ../../approved/redact-review.png
Margin notes as plain text blocks, not cards: "Alex Johnson · 14:32" with "Is this the final rent?" and under it,
indented, "Maria Lind: Yes, confirmed by mail on 2 October." and a small text link "Reply"; "Alex Johnson" with
"Which date counts?"; "Alex Johnson" "Highlight"; "Alex Johnson" "Underline". One note has a tiny ink-blue
"Resolved" check pill.

## c-margin-2-select
edit: c-margin-1-review.png
Keep this screenshot exactly: same window, page, margin and type. Change only this: the text box "Check with the
landlord" is selected: ink-blue dashed rectangle with eight round handles, and a rounded property popover below it
with colour dots, a font size field "12", an opacity slider, and a trash icon. Its margin note has an ink-blue
left edge.
