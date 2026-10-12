# Redact r2: the same three ideas, legible

What r1 got wrong (the user, 2026-10-10): "The images are really bad, cant give my verdict." Each r1 render packed two
app windows onto one board, so each screen was about 1000 px wide and its UI text was mushy and unreadable. r2 keeps
r1's three ideas (a-panel, b-margin, c-sweep) and everything decided in redact-r1/spec.md (idea, palette, marks,
copy, motion, must-nots). It changes only the framing: one app window per image filling 1536x1024, two images per
variant: `-1-review` (marks being reviewed before applying) and `-2-result` (after applying). The sheet is `--cols 2`,
so each variant is one row.

Copy, palette, motion and must-not: as redact-r1/spec.md. Renders may still garble text; the build sets it.

Render notes (2026-10-10): the result screens of a and b are edits of their review screens. Their first renders
drifted to Organize's page sidebar and a bright blue. b-margin-1-review was recovered from ~/.codex/generated_images.
Known drift, not worth another render: a-panel's result puts the panel on the left (the review has it on the right;
the build keeps one side). b-margin's result chrome differs a little (tab row, toolbar). The review has 4 marks but
the counts say 7 (the copy is set by the build).

Verdict (2026-10-11): c-sweep, plus a-panel's manual area and page tools, docked on the right. The user, after seeing
Acrobat's Redact panel, Comments-pane marks, Search window and _Redacted output: "Ok do your recommendation". The
recommendation (inkwell's, adopted by the user): "c-sweep, plus a-panel's manual area and page tools, docked on the
right like b-rail". Build: one right rail like the assistant and Tools rails, with the manual tools at its top, then find
and pattern pills with counts, hits as page snippets grouped by page, the marks list, sanitize inline (no modal), and the
result card. The app's colour tokens, not the render's.

## Shared

Render: one flat screenshot of one Windows 11 desktop application window, seen straight on, filling the whole
1536 x 1024 frame edge to edge. No board, no desktop background, no second window, no device frame, no perspective.
Crisp at 100 % scale like a real screenshot: large legible UI text (13-15 px UI labels, 16-18 px document body
text), sharp 1 px hairlines, sharp icon strokes, no blur, no soft focus, no compression smudge. Every word must be
real, correctly spelled English or Swedish; never lorem ipsum, never squiggles standing in for text.
The app is Inkwell, a PDF app, in its "Redact" mode, matching the attached reference image's window, title bar,
palette, type and rounded shapes exactly.
Window: slim custom title bar with the ink-drop mark and "Inkwell", a "Home" tab, an active rounded tab
"Lease agreement.pdf", a "+" button, and the window controls. Under it a slim toolbar with a rounded segmented pill
"Read | Organize | Redact" with Redact selected, undo and redo icons.
Document: a real residential lease agreement on white paper with a soft shadow, page 2, heading "2. PARTIES",
readable contract sentences: "This Residential Lease Agreement is made on 1 October 2026 between Maple Property
Management AB (the Landlord) and the Tenant named below." Then a "Tenant" block: "Name: Alex Johnson",
"Personnummer: 811218-9876", "E-mail: alex.johnson@example.com", "Phone: +46 70 123 45 67", then "2.3 Rent" with a
sentence about rent of 12 400 SEK per month due on the 25th.
Marks before they are applied: a dark ink hatch over the text, a translucent charcoal fill with a thin dashed charcoal
outline and rounded corners, the text still faintly readable under it. Applied redactions: solid charcoal boxes with
slightly rounded corners, the text gone.
Shape language: everything rounded and smooth (8-12 px radii, pill fields, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; sidebar #ECE9E2; text #1A1C20; secondary #6B675F; borders #E3DED4; one
accent ink blue #2E4B8F only for the selected mode, selection, checkboxes and the primary button. No other hues, no red.
Type: Segoe UI Variable style humanist sans, crisp like a polished native app.
Must not appear: square corners, boxy tables, red, bright blue, purple, gradients, glow, glass, sparkles, emoji,
shields, locks, people, red PDF icons, browser chrome, device frame, watermark, lorem ipsum.

## Shared a-panel
Idea: like organize's panel, made for redaction. A left panel about 300 px wide on the sidebar colour with rounded
groups: three round icon buttons with labels "Mark text", "Mark area", "Mark pages"; a pill search field "Find text";
under it a short list of patterns, each a quiet row with an ink-blue checkbox, a name and a count ("Personnummer 1",
"E-mail 2", "Phone 3"); a "Mark 6" button; at the bottom "7 marks" and a rounded ink-blue "Apply 7 marks" button.
The page is large in the main area, right of the panel.

## a-panel-1-review
ref: ../../approved/organize.png
The personnummer, e-mail and phone on the page are under hatched marks; the mouse pointer is dragging a new dashed box
over the name "Alex Johnson". In the panel, below the patterns, a "Marks" list of quiet rows, each with the marked
text and a small round "x": "811218-9876", "alex.johnson@example.com", "+46 70 123 45 67", "Area on page 4", "Page 9".

## a-panel-2-result
edit: a-panel-1-review.png
Keep this screenshot exactly: the same window, title bar, tabs, toolbar, page, type, sizes and the same right-hand
panel position and width, crisp at 100 % scale. Change only this, to show the state after applying: on the page, the
name, personnummer, e-mail and phone become solid charcoal (#1A1C20) boxes with slightly rounded corners, the text
gone, no hatching, no dashed outlines, no mouse pointer. In the panel, keep the three tool buttons at the top and
replace everything below them with a calm report on the same panel colour: "7 marks redacted" in a larger weight; a
line with a small ink-blue (#2E4B8F) check "Checked: none of the marked text is left in the file."; a heading "Also
removed" with six quiet rows and right-aligned counts "Metadata 2", "Attachments 1", "Hidden layers 1", "Comments 3",
"Bookmarks 4", "Links and scripts 2"; secondary text "Saved as Lease agreement_Redacted.pdf"; at the bottom two
rounded buttons "Show in folder" (white) and "Done" (ink blue #2E4B8F). No other colours.

## Shared b-margin
Idea: no panel; the marks are reviewed where they are, like a proofreader's margin. The page is large and slightly
left of centre; the window's right side, beside the page, is a wide quiet margin on the window colour. A small rounded
floating toolbar at the top centre holds "Mark text", "Mark area", "Mark pages" icons and a pill field "Find or pick a
pattern".

## b-margin-1-review
ref: ../../approved/organize.png
Each hatched mark (name, personnummer, e-mail, phone) has a thin charcoal leader line to a note in the right margin:
"Alex Johnson · Name  x", "811218-9876 · Personnummer  x", "alex.johnson@example.com · E-mail  x",
"+46 70 123 45 67 · Phone  x". At the bottom right a rounded ink-blue pill "Apply 7 marks".

## b-margin-2-result
edit: b-margin-1-review.png
Keep this screenshot exactly: the same window, title bar, tabs, toolbar, floating toolbar, page, type, sizes and
leader lines, crisp at 100 % scale. Change only this, to show the state after applying: the four hatched marks
become solid charcoal (#1A1C20) boxes with slightly rounded corners, the text gone, no hatching. The four margin notes
lose their pill outline and "x" and become quiet secondary grey text: "Name · removed", "Personnummer · removed",
"E-mail · removed", "Phone · removed". The "Apply 7 marks" button is gone. Below the notes in the right margin, a short
report written as plain lines, no box, no card: "7 marks redacted" in a larger weight; "Checked: none of the marked
text is left in the file."; "Also removed: metadata, attachments, hidden layers, comments, bookmarks, links"; "Saved
as Lease agreement_Redacted.pdf"; and an ink-blue text button "Show in folder".

## Shared c-sweep
Idea: find leads. A slim left sidebar on the sidebar colour with three quiet entries "Find and redact" (selected),
"Mark by hand", "Report".

## c-sweep-1-review
ref: ../../approved/organize.png
The content area: a wide pill search field across the top "Find text or a pattern", a row of quiet pill buttons under
it "Personnummer", "SSN", "Card number", "IBAN", "E-mail", "Phone", with "E-mail" selected in ink blue. Below,
"6 found on 3 pages" and a rounded ink-blue "Mark 6" button. Then a column of three wide cut-outs from the pages,
each a horizontal strip of real page text with the e-mail address hatched in place, a round ink-blue checkbox (all
checked) and "p. 2", "p. 5", "p. 9" in small secondary text: "Tenant e-mail: alex.johnson@example.com", "Notices
shall be sent by e-mail to alex.johnson@example.com or by post.", "Contact the tenant at alex.johnson@example.com".

## c-sweep-2-result
ref: ../../approved/organize.png
After applying: the document dimmed behind a calm rounded sheet in the centre. Left in the sheet, a page thumbnail of
page 2 with four solid charcoal boxes. Right: "7 marks redacted" large; a line with a small ink-blue check "Checked:
none of the marked text is left in the file."; "Also removed" with six quiet rows and counts ("Metadata 2",
"Attachments 1", "Hidden layers 1", "Comments 3", "Bookmarks 4", "Links and scripts 2"); "Saved as Lease
agreement_Redacted.pdf"; buttons "Show in folder" and an ink-blue "Done".
