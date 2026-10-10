# Redact r1: removing what must not leave

First round for this surface. It continues the approved look of home, the assistant and organize
(approved/organize.png): a real Windows 11 desktop app, rounded and smooth, warm palette, one ink-blue accent.
Function from the Acrobat study (docs/study/01-redact.md): mark text (drag over it), an area (drag a box) or whole
pages; find text or a pattern (SSN, personnummer, card, IBAN, e-mail, phone) and mark the checked hits; review marks
and unmark; Apply with "remove hidden information" on by default, showing counts per category first; after apply
the file is rewritten, saved as `<name>_Redacted.pdf`, and checked. Inkwell does better: the engine re-reads the
written file and proves no marked text is left (the report says so), sanitize covers what Acrobat misses, and the
report shows what was removed per category instead of a silent rewrite.

Idea: marks are a promise you can still take back; Apply keeps it for good and shows the proof.
For: someone sending a lease, a payslip or a court filing out and needing names and numbers gone for real.
Feel: calm, deliberate, trustworthy, native.
Palette/type/shape: as organize (window #F4F2ED, surfaces #FFFFFF, sidebar #ECE9E2, text #1A1C20, secondary #6B675F,
borders #E3DED4, accent #2E4B8F). A mark before apply is a dark ink hatch: #1A1C20 at about 20 % fill with a 1.5 px
dashed #1A1C20 outline; after apply it is a solid #1A1C20 box. No red: the accent stays ink blue, and a mark's state
is told by its shape (dashed and hatched vs solid), never by colour alone.
Document: "Lease agreement.pdf", page 2 (Parties) with the tenant's name, a personnummer, an e-mail and a phone.

Copy (final; renders may garble it, the build sets it):
- Tools: "Mark text", "Mark area", "Mark pages", "Find", patterns "Personnummer", "SSN", "Card number", "IBAN",
  "E-mail", "Phone".
- Find results: "6 found on 3 pages", "Mark 6".
- Marks: "7 marks", per mark the marked text or "Area on page 4" / "Page 9", an unmark (x) button.
- Apply button: "Apply 7 marks".
- Confirm: title "Redact 7 marks for good?"; body "The marked text, images and pages are removed from the file.
  This can't be undone in the saved file." Checkbox "Also remove hidden information" (on) with rows and counts:
  "Metadata 2", "Attachments 1", "Hidden layers 1 page", "Comments 3", "Bookmarks 4", "Links and scripts 2".
  Save line: "Saves as Lease agreement_Redacted.pdf". Buttons "Cancel", "Redact".
- Report: "7 marks redacted"; "Checked: none of the marked text is left in the file."; "Also removed: metadata 2,
  attachments 1, hidden layers 1, comments 3, bookmarks 4, links and scripts 2"; "Saved as Lease
  agreement_Redacted.pdf"; buttons "Show in folder", "Done".
Motion (for the build): a mark draws its dashed outline in 120 ms as the drag ends; unmark fades it 120 ms. On
Apply, each hatched mark fills to solid ink top to bottom in 180 ms, page by page (the engine's result, not a
timer: the boxes fill when the verified file is back). Reduced motion: marks and boxes appear in their settled state.
Must not appear: red, sharp corners, purple/indigo/violet, gradients, glow, sparkles, emoji, shields or lock icons,
people, browser chrome, device frames, tag pills.

Render notes (2026-10-10): a-panel came out on a blue desktop with a bright blue accent; a-panel.2 is a colour-only
edit of it and is what the sheet shows. c-sweep's file write failed in Codex and was recovered from
~/.codex/generated_images; its board is taller (square) than the others. Copy is off in places (the confirm's
category rows in a, c's "Also removed" lists patterns instead of hidden information): the build sets the text.

## Shared

Render: a presentation board, 1536 x 1024, holding two flat screenshots side by side of one Windows 11 desktop
application window each, seen straight on, with a thin margin of plain light desktop around them. Both screenshots
are the "Redact" mode of a PDF app called Inkwell, matching the attached reference image's window, title bar,
palette, type and rounded shapes exactly.
Window: slim custom title bar with the ink-drop mark and "Inkwell", a "Home" tab, an active rounded tab
"Lease agreement.pdf", a "+" button, and the window controls. Under it a slim toolbar with a rounded segmented pill
"Read | Organize | Redact" with Redact selected, undo and redo icons.
Document: a lease contract on white paper with soft shadows; page 2 "PARTIES" shows the tenant "Alex Johnson",
"Personnummer 811218-9876", "alex.johnson@example.com" and "+46 70 123 45 67".
Marks before they are applied: a dark ink hatch over the text, a translucent charcoal fill with a thin dashed charcoal
outline and rounded corners, the text still faintly readable under it. Applied redactions: solid charcoal boxes with
slightly rounded corners, the text gone.
Shape language: everything rounded and smooth (8-12 px radii, pill fields, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; sidebar #ECE9E2; text #1A1C20; secondary #6B675F; borders #E3DED4; one
accent ink blue #2E4B8F only for the selected mode, selection and the primary button. No other hues, no red.
Type: Segoe UI Variable style humanist sans, small and crisp like a polished native app.
Must not appear: square corners, boxy tables, red, purple, gradients, glow, glass, sparkles, emoji, shields, locks,
people, red PDF icons, browser chrome, device frame, watermark, extra text.

## a-panel
ref: ../../approved/organize.png
Like organize's panel, made for redaction. Left screenshot: a left panel about 260 px wide on the sidebar colour with
rounded groups: three round icon buttons with labels "Mark text", "Mark area", "Mark pages"; a pill search field
"Find text" and under it a short list of patterns, each a quiet row with a checkbox and a count ("Personnummer 1",
"E-mail 2", "Phone 3"); a "Mark 6" button; at the bottom "7 marks" and a rounded ink-blue "Apply 7 marks" button.
Main area: page 2 large and centred, the personnummer, e-mail and phone under hatched marks, the mouse dragging a new
dashed box over the tenant's name. Right screenshot: the same window dimmed, with a rounded confirm dialog in the
centre: "Redact 7 marks for good?", a short body line, a checked "Also remove hidden information" with six indented
rows and counts, "Saves as Lease agreement_Redacted.pdf" in secondary text, and "Cancel" and an ink-blue "Redact".

## b-margin
ref: ../../approved/organize.png
No panel: the marks are reviewed where they are. Left screenshot: page 2 large and slightly left of centre; each
hatched mark has a thin leader line to a quiet note in the right margin of the window, like a proofreader's margin:
"811218-9876 · Personnummer  x", "alex.johnson@example.com · E-mail  x", "+46 70 123 45 67 · Phone  x". A small
rounded floating toolbar at the top centre holds "Mark text", "Mark area", "Mark pages" icons and a pill field "Find
or pick a pattern". At the bottom right a rounded ink-blue pill "Apply 7 marks". Right screenshot: after applying:
the same page with solid charcoal boxes where the marks were, the margin notes now quiet secondary text ending
"removed", and below them, in the margin, a short report written as plain lines, no box: "7 marks redacted",
"Checked: none of the marked text is left in the file.", "Also removed: metadata, attachments, hidden layers,
comments, bookmarks, links", "Saved as Lease agreement_Redacted.pdf", with a text button "Show in folder".

## c-sweep
ref: ../../approved/organize.png
Find leads. Left screenshot: a wide pill search field across the top of the content, "Find text or a pattern", with a
row of quiet text buttons under it "Personnummer  SSN  Card number  IBAN  E-mail  Phone", "E-mail" underlined as
selected. Below, the results as a column of cut-outs from the pages, each a short horizontal strip of real page with
the hit hatched in place and "p. 2", "p. 5", "p. 9" in small secondary text beside it and a round checkbox, all
checked; above them "6 found on 3 pages" and a rounded ink-blue "Mark 6" button. Right screenshot: the after-apply
report as a calm sheet over the dimmed document: a page thumbnail with three solid charcoal boxes on the left, and on
the right "7 marks redacted", a line with a small ink-blue check "Checked: none of the marked text is left in the
file.", a short list "Also removed" with six rows and counts, "Saved as Lease agreement_Redacted.pdf", and buttons
"Show in folder" and an ink-blue "Done".

## a-panel.2
edit: a-panel.png
Keep this image exactly: same layout, windows, text, marks and dialog. Change only the colours: the plain desktop
margin around the two windows becomes a light warm grey (#F4F2ED family), not blue. The selected "Redact" mode, the
checkboxes, "Apply 7 marks" and the "Redact" dialog button become a deep muted ink blue (#2E4B8F), not bright blue.
Window chrome stays warm off-white (#F4F2ED), the left panel warm grey (#ECE9E2). No other hues.
