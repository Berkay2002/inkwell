# Protect r1: passwords, permissions, remove security (phase 5)

First round of the surface; there is no verdict to start from. The design follows `design/README.md` (Inkwell system:
rounded, warm, one ink-blue accent, Segoe UI Variable) and the Tools and Redact rails (one right rail at a time, same
width and chrome as the assistant rail). One app window per image at 1536x1024, two images per variant: `-1-protect`
(an unprotected lease with the Protect rail filled in) and `-2-security` (someone else's restricted file: the notice
above the page and the rail in its Security view, with Remove security). The sheet is `--cols 2`: one row per variant.

Brief (task-protect.md): open password and permissions password; printing none/low/high; changes none, pages, forms,
comments, all except extracting; copying; accessibility; AES-256 (R6) by default with an AES-128 option for old
readers; remove security needs the permissions password when one is set; a strength meter, a confirm field and a
clear "a lost open password can't be recovered" warning; output is Save as, or the normal Save after an explicit
confirm; show what a file restricts and never silently strip owner restrictions.

## Verdict

(pending: what is both standard and what pdfcraft and Acrobat have, with a hint of uniqueness.)

## How Acrobat Pro does it
- All tools > Protect a PDF opens the Protect panel; "Protect using password" asks "Viewing" or "Editing", then the
  classic **Password Security - Settings** dialog: a Compatibility combo (Acrobat 3.0 up to Acrobat X and later,
  128-bit RC4 ... 256-bit AES), what to encrypt (all contents, all except metadata, attachments only), then
  "Require a password to open the document" with a Document Open Password and a strength meter, and "Restrict editing
  and printing of the document" with a Permissions Password, Printing Allowed (None, Low Resolution 150 dpi, High
  Resolution), Changes Allowed (None; Inserting, deleting, and rotating pages; Filling in form fields and signing
  existing signature fields; Commenting, filling in form fields, and signing existing signature fields; Any except
  extracting pages), "Enable copying of text, images, and other content", and "Enable text access for screen reader
  devices for the visually impaired". OK asks to type the passwords again, then the file is changed in place and
  needs Save. File > Properties > Security shows the method and a summary of what is allowed; "Security Method: No
  Security" removes it, asking for the permissions password when one is set.
- It is a modal in front of the document, and the options are many and unordered.

## How pdfcraft does it (github.com/storytold/pdfcraft @ b8ddf71: crates/ui-egui/src/protect.rs)
- A small "Protect Using Password" dialog: "Requires user to enter a password for: Viewing / Editing", a password
  field with a Weak / Medium / Strong label, a confirm field, "Advanced Options" folding out Compatibility (256-bit
  AES ... 40-bit RC4), what to encrypt, and, for Editing, Printing allowed, Changes allowed, copying and
  accessibility. A line says when the password applies ("It's applied when you save").
- A "Security properties..." entry shows the method and the restrictions summary, and "Remove security" is a
  separate command that is only offered for a protected document.

## Where the variants go
- **a-sheet** follows Acrobat's classic settings dialog, as one calm rail sheet: two sections that can each be
  switched on (open password; restrictions with their own password), every Acrobat choice present, the encryption
  level as a two-way switch. It departs by being one sheet with the outcome sentence written out above the button.
- **b-ask** follows pdfcraft: ask one thing first (a password for Viewing or for Editing), one password with its
  confirm and meter, and the rest behind "More options". It departs from both by saying plainly what the file will
  do before the button, and by two honest buttons (save a copy; protect this file).
- **c-sentence** departs from both: the page of the rail is a sentence that rewrites itself ("Opens with a
  password. Prints at low resolution. Can't be copied or edited.") over compact controls, so the outcome is read
  first and the options only change it.

Idea: you never apply a lock you can't read back: the outcome is stated in plain words before it is written, and
removing it is as plain as setting it.
For: someone sending a contract or an invoice and wanting to be sure who can open it and what they can do.
Feel: calm, exact, reassuring; a lock is a small thing, not a drama.
Palette: as the system (window #F4F2ED, surfaces #FFFFFF, panel #FAF8F4, text #1A1C20, secondary #6B675F, borders
#E3DED4, accent #2E4B8F). Strength meter segments use the accent for filled and #E3DED4 for empty: the label says
the strength, colour never carries it alone.
Type: Segoe UI Variable; the file name and the outcome numbers in a quiet monospace.
Motion (build): the strength meter fills segment by segment as you type (80 ms); the outcome sentence swaps its
changed phrase with a 120 ms cross-fade; the result card slides in. Reduced motion: no easing.
Must not appear: red, a warning triangle, a padlock hero graphic, glow, glass, gradients, purple, emoji, avatars,
dark theme, tables of boxed cards.

## Shared

Render: one flat screenshot of one Windows 11 desktop application window, seen straight on, filling the whole
1536 x 1024 frame edge to edge. No board, no desktop background, no second window, no device frame, no perspective.
Crisp at 100 % scale like a real screenshot: large legible UI text (13-15 px UI labels, 16-18 px document body
text), sharp 1 px hairlines, sharp icon strokes, no blur, no compression smudge. Every word real, correctly spelled
English; never lorem ipsum, never squiggles standing in for text.
The app is Inkwell, a PDF app, matching the attached reference image's window, title bar, palette, type and rounded
shapes exactly. Window: slim custom title bar with the ink-drop mark and "Inkwell", a "Home" tab, an active rounded
tab "Lease agreement.pdf", a "+" button, and the window controls. Under it a slim toolbar with a rounded segmented
pill "Read | Organize | Comment" with Read selected, undo and redo icons. A left page-thumbnail strip, the page large
in the centre, the right rail about 360 px wide on the panel colour with a close icon, in the same dock as the
reference's Compress rail.
Document: a residential lease agreement on white paper with a soft shadow, page 1, heading "RESIDENTIAL LEASE
AGREEMENT", then "1. PARTIES": "This Residential Lease Agreement is made on 1 October 2026 between Maple Property
Management AB (the Landlord) and the Tenant named below.", then "2. RENT": "The rent is 12 400 SEK per month, due on
the 25th of each month."
Shape language: rounded and smooth (8-12 px radii, pill fields, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; panel #FAF8F4; text #1A1C20; secondary #6B675F; borders #E3DED4; one UI
accent ink blue #2E4B8F only for the selected choice, filled meter segments, and the primary button. No other UI
hues, no red, no orange, no green.
Type: Segoe UI Variable style humanist sans; file names and numbers in a quiet monospace.
Must not appear: square corners, boxy grids of cards, red, orange, green, bright blue, purple, gradients, glow, glass,
sparkles, emoji, people photos, dark theme, red PDF icons, a big padlock illustration, browser chrome, device frame,
watermark, lorem ipsum.

## Shared a-sheet
Idea: Acrobat's settings dialog as one rail sheet. The rail is titled "Protect" with a close icon. Content, top to
bottom on the panel colour, no cards, separated by hairlines:
a switch row "Require a password to open" (on) with a pill password field showing six dots, a pill field "Confirm
password" with six dots, and a strength meter of five thin segments, three filled in ink blue, with the label "Fair"
beside it;
a switch row "Restrict editing and printing" (on) with a pill field "Permissions password" with dots, a select
"Printing" showing "Low resolution (150 dpi)", a select "Changes" showing "Commenting, filling in forms and
signing", and two quiet checkboxes "Allow copying of text and images" (off) and "Allow screen readers" (on);
a two-way segmented control "Encryption": "AES-256" (selected) and "AES-128 (older readers)".
Then a line in secondary text: "If you lose the open password, the file can't be recovered." Then the outcome
sentence in the text colour: "Opens with a password. Prints at low resolution. Can't be copied or edited." Then a
full-width primary ink-blue button "Protect and save a copy..." and under it a quiet text link "Protect this file
instead".

## a-sheet-1-protect
ref: ../../approved/tools-compress.png
The page is large in the centre, unmarked. The rail shows the sheet described above.

## a-sheet-2-security
edit: a-sheet-1-protect.png
Keep the window, tabs, toolbar, thumbnails and type exactly. Change only this: the tab is "Supplier terms.pdf" with a
small lock icon before the name. Above the page, spanning the page area, a slim rounded notice strip in the surface
colour with a lock icon: "This file restricts printing and copying. The owner's restrictions are kept when you
save." with a small text link "Details". The rail is titled "Security" and shows, as plain rows with hairlines:
"Encryption" "AES-256"; "Opens with" "a password"; then "What the owner allows": "Printing" "Low resolution",
"Copying" "Not allowed", "Editing" "Commenting and forms", "Page changes" "Not allowed", "Screen readers"
"Allowed". Then a pill field "Permissions password" with dots and a quiet button "Remove security", with the line
"Needs the permissions password. You can undo this before saving."

## Shared b-ask
Idea: pdfcraft's one question first. The rail is titled "Protect" with a close icon. Content, top to bottom, no
cards: the question "Ask for a password to:" with two radio choices, "View the file" (selected) and "Edit the file";
a pill field "Password" with six dots, a strength meter of five thin segments, two filled, with the label "Weak"; a
pill field "Confirm password" with six dots; a line "Anyone opening the file will need this password. If you lose
it, the file can't be recovered."; a disclosure row "More options" with a chevron (closed); the outcome sentence in
the text colour, in a quiet monospace for the file name: "Saves a copy named Lease agreement (protected).pdf. The
original is not changed."; a full-width primary ink-blue button "Protect and save a copy..." and under it a quiet text
link "Protect this file instead".

## b-ask-1-protect
ref: ../../approved/tools-compress.png
The page is large in the centre, unmarked. The rail shows the sheet described above, with "More options" closed.

## b-ask-2-security
edit: b-ask-1-protect.png
Keep the window, tabs, toolbar, thumbnails and type exactly. Change only this: the tab is "Supplier terms.pdf" with a
small lock icon before the name. Above the page a slim rounded notice strip with a lock icon: "Restricted file:
printing is limited and copying is off." with a small text link "Details". The rail is titled "Security" and shows one
quiet paragraph, "Protected with a password, AES-256. Printing is limited to low resolution, copying and page
changes are off.", a disclosure "What the owner allows" opened to a short list of five rows with a small check or
dash before each ("Print low resolution", "Fill forms and comment", "Screen readers", and in secondary text "No
copying", "No page changes"), then a pill field "Permissions password" with dots and a quiet button "Remove
security".

## Shared c-sentence
Idea: the outcome is the interface. The rail is titled "Protect" with a close icon. At the top, in larger type (20 px,
text colour), a plain sentence on three short lines: "Opens with a password." "Prints at low resolution." "Can't be
copied or edited." Under it a hairline. Then compact controls: a row "Open password" with a pill field of six dots and a
small strength meter of five thin segments, four filled, with the label "Strong"; a row "Permissions password" with
a pill field of dots; a row "Printing" with a three-way segmented control "None | Low | High" with Low selected; a row
"Changes" with a select showing "Comments and forms"; two rows with switches "Copying" (off) and "Screen readers"
(on); a row "Encryption" with a two-way segmented control "AES-256" selected and "AES-128". Under the controls the
line "If you lose the open password, the file can't be recovered." in secondary text, then a full-width primary
ink-blue button "Protect and save a copy..." and under it a quiet text link "Protect this file instead".

## c-sentence-1-protect
ref: ../../approved/tools-compress.png
The page is large in the centre, unmarked. The rail shows the sheet described above.

## c-sentence-2-security
edit: c-sentence-1-protect.png
Keep the window, tabs, toolbar, thumbnails and type exactly. Change only this: the tab is "Supplier terms.pdf" with a
small lock icon before the name. Above the page a slim rounded notice strip with a lock icon: "This file is
protected. Its owner restricts printing and copying." with a small text link "Details". The rail is titled
"Security" and its top, in the larger type, is the sentence on three lines: "Opens with a password." "Prints at low
resolution." "Can't be copied, edited or reordered." Under a hairline, in the compact rows with the controls shown
read-only (greyed values, no switches): "Encryption" "AES-256", "Open password" "set", "Permissions password" "set";
then a pill field "Permissions password" with dots and a quiet button "Remove security".
