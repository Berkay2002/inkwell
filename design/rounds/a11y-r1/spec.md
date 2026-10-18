# A11y r1: the accessibility checker (phase 5, a11y-ui)

First round of this surface. The engine is on main (`src-tauri/src/a11y`, docs/a11y.md): fifteen rules, each Passed / Failed /
Needs manual check / Skipped, findings with page and object, five automatic fixes plus a best-effort Autotag. This round
designs how a person uses it. It reopens nothing: it uses the Inkwell system (design/README.md) and the right-rail
pattern the Tools, Redact, Comments and History rails already share (one right-side panel at a time, same dock, same
head with a close button). One app window per image at 1536x1024; two images per variant (`-1-report`, `-2-autotag`), so
the sheet is `--cols 2`: one row per variant.

What the surface must carry (task brief): opens from the Tools menu ("Check accessibility...") and the command palette;
a report rail with a summary (passed / failed / needs manual check), the rules grouped by category, each with its status,
a short explanation, "Fix" where the engine has an automatic fix, and "Show" which jumps to the page and element and
highlights it; each fix is one undo step and "Fix all safe issues" is one step; the list updates after a fix; Autotag for an
untagged document with progress and cancel, one undo step, and the outcome predicted before it runs ("Adds tags to 12 pages;
reading order from layout"); export the report as HTML or PDF; the check runs off the UI thread with progress and cancel.

What the engine can and cannot do, so the screens do not promise more: automatic fixes exist for Title, Title in title bar,
Primary language, Tab order, Form field tooltips, Figures alternate text (the person writes the text) and Autotag. Reading
order, table headers, heading nesting, list structure, tagged links, character encoding and bookmarks are reported with
page and object but have no automatic fix. Colour contrast is always "Needs manual check". Autotag builds headings,
paragraphs, lists, figures, links and form elements from the layout; no tables.

## Why this round has three ideas

Acrobat's checker is the standard and the user wants the standard first. The three ideas are three ways to lay the same
facts out in a rail, not three palettes:

- **a-rail** is Acrobat's own structure in Inkwell's look: a result summary, then the rules grouped by Acrobat's seven
  categories (Document, Page content, Forms, Alternate text, Tables, Lists, Headings), each rule a row with a status glyph.
- **b-todo** sorts by what the person can do instead of by category: "Fixed in one step", "Needs you", "Check by eye",
  "Passed". The outcome is predicted on the one primary button: "After this, 11 of 15 pass".
- **c-onpage** puts the problems on the document: outlined elements and numbered pins on the page, a slim rail of
  findings by page; Acrobat's Reading Order overlay and pdfcraft's shaded findings are its ancestors.

## How Acrobat Pro does it

Read from the shipping files only (`docs/study/14-a11y.md`; nothing was driven). The check has an options dialog (report
options, pages, one check box per rule in seven groups), progress ("Evaluating page", "Evaluating structure"), and a result
panel that is a tree: the seven groups, each rule as "Rule - Status" (Passed, Failed, Needs manual check, Skipped, plus
"Passed manually" and "Failed manually" a person sets). Per rule: Fix, Pass or Fail, Skip Rule, Explain, Check Again, Show
Report and, for findings with an element, Show in Tags / Content panel. The report is HTML: a Summary of counts, then one
table per group (Rule Name, Status, Description). Fix mostly opens another tool (the Reading Order tool, a dialog per
figure); "Add tags to document" runs with no preview.

## How pdfcraft does it

Same seven groups (headers go red with "(n issues)"), rows "Rule - Failed" with a status icon, a click on a row with findings
expands them and a click on a finding goes to its page. Fix exists for three rules (language and title open Properties, tab
order applies); Skip Rule persists; it re-checks after a fix. The HTML report has a Summary and a table per group, with
findings as bullets and no page links. Autotag and reading order are planned there, not built.

## Verdict

2026-10-11 (no render was judged: the pick is from this spec's text): **a-rail**, plus two things taken from b-todo: (1) the predicted outcome on the
"Fix all safe issues" button: "Fix 3 issues · after this 12 of 15 pass"; (2) under a failed Figures alternate text finding,
an inline alt-text field with the figure's thumbnail, so no dialog is needed. c-onpage is dropped. Nothing was rendered;
no approved image exists for this surface, the build follows the spec text and Inkwell's rails.

## Shared

Render: one flat screenshot of one Windows 11 desktop application window, seen straight on, filling the whole
1536 x 1024 frame edge to edge. No board, no desktop background, no second window, no device frame, no perspective.
Crisp at 100 % scale like a real screenshot: large legible UI text (13-15 px UI labels, 16-18 px document body
text), sharp 1 px hairlines, sharp icon strokes, no blur, no compression smudge. Every word real, correctly spelled
English; never lorem ipsum, never squiggles standing in for text.
The app is Inkwell, a PDF app, matching the attached reference image's window, title bar, toolbar, palette, type and
rounded shapes exactly: a slim custom title bar with the ink-drop mark and "Inkwell", a "Home" tab, an active rounded tab,
a "+" button and the window controls; under it the slim toolbar with a "Tools" menu button and the right-hand icons; the
left sidebar of page thumbnails; the right side is a docked rounded rail panel about 400 px wide. The PDF page area
between them is the warm window colour with a white page on it.
Shape language: rounded and smooth (8-12 px radii, pill buttons and chips, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; panel #FAF8F4; text #1A1C20; secondary #6B675F; borders #E3DED4; one UI
accent ink blue #2E4B8F only for the selected item, the focus and the primary button. Status marks may use green #2F7D4F
for passed, red #B3392F for failed and amber #B7791F for needs manual check, each always with a glyph and a word
(check, cross, exclamation), never colour alone, and only on status marks.
Type: Segoe UI Variable style humanist sans.
Must not appear: square corners, boxy grids of equal cards, bright blue, purple, gradients, glow, glass, sparkles, emoji,
people photos, dark theme, red PDF icons, browser chrome, device frame, watermark, lorem ipsum, a score out of 100, a
traffic-light or confetti reward.

The tagged document for the `-1-report` images: "Annual report 2026.pdf", 12 pages, page 2 on screen: a heading "Results
at a glance", two short paragraphs, a bar chart image with a caption "Revenue by quarter" and a small photo of a harbour.
The report of that document, the same in every `-1-report` image, **15 rules checked: 5 failed, 1 needs a manual check, 9
passed**:
- Document: "Title" FAILED "The document has no title." (fix: Set title to "Annual report 2026"); "Title in title bar"
  PASSED; "Primary language" FAILED "No document language is set." (fix: Set language, suggestion "English (en-US)");
  "Tagged PDF" PASSED; "Bookmarks" PASSED; "Character encoding" PASSED.
- Page content: "Reading order" PASSED; "Tab order" FAILED "Pages 4 and 7 have links that do not follow the structure
  order." (fix: Set tab order); "Color contrast" NEEDS MANUAL CHECK "Check that text contrasts enough with its
  background."; "Tagged links" PASSED.
- Forms: "Form field tooltips" PASSED.
- Alternate text: "Figures alternate text" FAILED "3 figures have no alternate text." findings: "Figure on p. 2 (bar chart)",
  "Figure on p. 5", "Figure on p. 9" (fix: Add alternate text, written by the person; no one-click fix).
- Tables: "Table headers" PASSED. Lists: "List structure" PASSED.
- Headings: "Heading nesting" FAILED "Heading levels skip from H1 to H3 on p. 6." (no automatic fix; Show only).
Safe automatic fixes: Title, Primary language, Tab order = **3**.

The untagged document for the `-2-autotag` images: "Product brochure.pdf", 8 pages, page 1 on screen (a title, a
subtitle, a bulleted list of three features, a photo of a desk lamp). The report says "Tagged PDF: failed. This document is
not tagged." and the structure rules are Skipped because it is untagged.

Shared labels: the Tools menu item "Check accessibility..."; rail title "Accessibility"; buttons "Check again", "Fix",
"Show", "Fix all safe issues", "Autotag (best effort)", "Export report", "Cancel"; status words "Passed", "Failed", "Needs
manual check", "Skipped".

## Shared a-rail
Idea: Acrobat's checker as a calm Inkwell rail. Top of the rail: the head "Accessibility" with a close button; under it a
quiet one-line result "5 failed · 1 needs a manual check · 9 passed" with a slim segmented bar (red, amber, green) and a
text button "Check again" at the right. Then the rules grouped under seven small-caps group titles with counts (Document,
Page content, Forms, Alternate text, Tables, Lists, Headings). A rule is one row: a round status glyph, the rule name in
semibold and, for failed rules, one line of explanation in secondary text and a row of small pill buttons "Fix" and
"Show". Passed rules are single quiet lines. Failed rules in a group are shown first. A footer pinned to the bottom of the
rail: a primary ink-blue pill "Fix all safe issues (3)" and a quiet "Export report" button with a small chevron.

## a-rail-1-report
ref: ../../approved/redact-review.png
The tagged "Annual report 2026.pdf", page 2 in the viewer, the rail as described with the Document group and the
Alternate text group expanded: Title (Failed, "Fix" shown), Primary language (Failed), Figures alternate text (Failed, with
its three findings listed under it as small rows "p. 2 Figure", "p. 5 Figure", "p. 9 Figure", the first one hovered with
the page highlighted behind), Heading nesting (Failed, "Show" only).

## a-rail-2-autotag
ref: ../../approved/redact-review.png
The untagged "Product brochure.pdf" in the viewer. The rail's top, instead of the result line, a rounded card: "This document
is not tagged" and below it the predicted outcome in one calm sentence "Adds tags to 8 pages. Headings, lists and paragraphs
from the layout; 4 images get a Figure tag without alternate text. One undo step." with a primary pill "Autotag (best
effort)" and a quiet text button "Not now". Under the card the rule list with "Tagged PDF" Failed and the structure rules
greyed with "Skipped".

## Shared b-todo
Idea: sorted by what you can do. Top of the rail: the head "Accessibility", then one line "15 rules checked" and
"Check again". Then four rounded sections, each with a title and a count: "Fixed in one step (3)" holding the three safe
fixes as a checklist of one line each ("Set the title to Annual report 2026", "Set the language to English (en-US)", "Tab in
reading order on pages 4 and 7"); "Needs you (2)" holding "Figures alternate text" with three small figure thumbnails each
with a one-line text field "Describe this chart" and "Heading nesting" with "Show"; "Check by eye (1)" holding "Color
contrast"; "Passed (9)" collapsed with a chevron. The one primary ink-blue pill at the bottom of the rail carries the
predicted outcome: "Fix 3 issues" with the line above it in secondary text "After this, 12 of 15 rules pass." Beside it a
quiet "Export report".

## b-todo-1-report
ref: ../../approved/redact-review.png
The tagged "Annual report 2026.pdf" as described, the sections as above, "Needs you" expanded with the figure thumbnails and
fields, the first field focused with the ink-blue ring.

## b-todo-2-autotag
ref: ../../approved/redact-review.png
The untagged "Product brochure.pdf". The rail shows a single large section "Tag this document" with the predicted outcome as
three short lines with small glyphs (headings, lists, paragraphs from the layout; 4 images need alternate text afterwards;
nothing on the page moves), a primary pill "Autotag (best effort)", and below it, while running, a slim ink-blue progress bar
labelled "Tagging page 5 of 8" with a quiet "Cancel" text button. The image shows the running state.

## Shared c-onpage
Idea: the problems sit on the document. The viewer shows the page with the bar chart and the photo each outlined by a
rounded dashed red rectangle with a small numbered round pin at the corner (1 and 2), and the heading "Results at a
glance" outlined in amber with a pin 3; a thin legend strip under the toolbar reads "Showing problems on this page".
The rail is slimmer, about 340 px: head "Accessibility"; a result line; filter chips "All 5", "Failed", "To check",
"Passed"; a list of findings grouped by page ("Page 2", "Page 5", ...), each row with its pin number, rule name and a one
line message and a pill "Fix" or "Show"; the selected row has an ink-blue edge and its pin is drawn larger on the page.
The footer: primary pill "Fix all safe issues (3)" and a quiet "Export report".

## c-onpage-1-report
ref: ../../approved/redact-review.png
The tagged "Annual report 2026.pdf", page 2 with the outlined chart and photo, pin 1 on the chart with a small popover
anchored to it: "Figure has no alternate text" with a text field "Describe this chart" and buttons "Save" and "Skip". The rail
lists the findings as described, the chart row selected.

## c-onpage-2-autotag
ref: ../../approved/redact-review.png
The untagged "Product brochure.pdf", page 1 shown with a soft translucent ink-blue overlay of what Autotag will tag:
numbered reading-order boxes around the title (1), subtitle (2), the three bullet lines (3), and the photo (4, with a small
"no alternate text" marker). The rail holds the predicted outcome "Adds tags to 8 pages. Reading order shown on the page.
One undo step." with the primary pill "Autotag (best effort)" and a quiet "Cancel".
