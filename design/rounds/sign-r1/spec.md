# Sign r1: Fill & Sign and digital signatures (phase 5)

First round of the surface; there is no verdict to start from. The design follows `design/README.md` (Inkwell system:
rounded, warm, one ink-blue accent, Segoe UI Variable) and the approved comments rail (`approved/comments.png`: floating
vertical tool strip at the left of the page, one right rail at a time, same width and chrome as the assistant rail).
One app window per image at 1536x1024, two images per variant: `-1-fill` (Fill & Sign: marks and a placed signature on a
form) and `-2-sign` (a digital signature being made, then the Signatures panel with its validity). Sheet `--cols 2`.

## Verdict

**a-strip**, with two pieces borrowed (picked by Berkay, 2026-10-11):
- Fill & Sign tools on the floating strip at the left of the page, as in Comment mode; saved signature and initials
  as tiles at the strip's foot. The right rail stays closed while filling.
- Digital signing: draw a rectangle on the page (or click an empty signature field); the rail opens the "Digital
  signature" sheet (ID, reason, lock, outcome line, Sign). **From c-ribbon:** the rectangle shows the live signature
  appearance while the sheet is edited.
- **From b-panel:** a slim banner above the page whenever the document has signatures ("Signed. All signatures are
  valid." / "At least one signature has problems." / "Signature validity is unknown."), with Details opening the
  Signatures rail.
- Skipped: c-ribbon's pill over the page and anchored card; b-panel's always-open tools rail.

## How Acrobat Pro does it (study 06, docs/study/screens/06-certificates/, 07-forms/07-esign-panel.png)
- **Fill & Sign** is the E-Sign tab's left panel: a row of tools **Aa** (text), **X**, **check**, **dot**, **rounded
  rectangle**, **line**, then two dashed tiles *Add signature* and *Add initials*, then "Save a certified copy".
  A signature is typed, drawn or an image, saved for reuse.
- **Digitally sign** is All tools > Use a certificate: a modal says to drag a rectangle; then a web dialog lists the
  Digital IDs (Windows store and file IDs), *Configure New Digital ID* (file, store, self-signed), then **Sign as
  "<name>"**: appearance dropdown, a large preview (name left, "Digitally signed by <name> Date: ..." right),
  *Lock document after signing*, *View Certificate Details*, a red "constructs which could affect its appearance"
  notice with Review, the password, Back / Sign. Then a native Save As.
- After signing, a blue banner "Signed and all signatures are valid." with **Signature Panel**; the panel lists each
  signature with a status icon and expandable details (validity, modified after signing, certificate).
- The capture is dark; the flow is four dialogs deep.

## How pdfcraft does it (crates/ui-egui/src/fill_sign.rs, sign_ui.rs)
- Fill & Sign: text, check, cross, dot, line marks and today's date, plus a drawn signature; every item is an annotation
  (move, delete, undo like a comment). Saved signature/initials have Place / Change / Remove buttons.
- Use a certificate: drag a rectangle or click an empty signature field, then Acrobat's three dialogs (Sign with a
  Digital ID, Configure New Digital ID, Sign as), a Signatures panel, and a signature message bar.

## Where the variants go
- **a-strip** follows pdfcraft and our approved comments layout: the Fill & Sign tools on the same floating vertical
  strip at the left of the page, with the saved signature and initials as two small tiles at the strip's foot. The
  digital signature is one sheet in the right rail (Acrobat's three dialogs folded into one: ID, appearance preview,
  lock, the outcome line, Sign), and the same rail becomes the Signatures panel after.
- **b-panel** follows Acrobat's E-Sign panel most closely, moved to the right rail: one "Fill & Sign" rail with the
  tool row, the two signature tiles, and a "Digital signature" section below; signing turns the rail into the
  Signatures list. Departs from Acrobat by being one rail instead of four dialogs and a light, rounded look.
- **c-ribbon** departs from both: a small horizontal floating pill over the top of the page for the marks, the
  signature in the pill as a chip; the digital signature is drawn as a rectangle on the page and a compact card opens
  beside it, anchored to the rectangle, with the preview inside the rectangle itself (what you see is what you sign).
  The Signatures panel is the right rail.

Inkwell's touch in all three: the outcome is said before you sign ("Saves a new revision. The earlier signature stays
valid." / "Locks the document: no more changes."), the sign step is undoable until the file is shared, and the
validity line uses words, not just icons: Valid, Unknown signer, Invalid.

Idea: a signature is a promise about these exact bytes; show what is promised, then keep it.
For: a person signing a lease, a form or a contract on their own PC, sometimes with a company certificate.
Feel: calm, exact, trustworthy, paper-like.
Palette: as the system (window #F4F2ED, surfaces #FFFFFF, panel #FAF8F4, text #1A1C20, secondary #6B675F, borders
#E3DED4, accent #2E4B8F). Validity states: valid #2F7D4F (green, with a check glyph), unknown signer #B7791F (amber,
with a question glyph), invalid #B3261E (red, with a cross glyph): an exception to the one-accent rule that applies
only to the three signature states, as history's diff marks. Ink on the page (placed text, marks, signature) is a dark
blue-black #1F2A44, a document mark, not UI.
Type: Segoe UI Variable; typed signatures in a script face.
Motion (build): a placed mark lands with a 100 ms settle; the signature preview updates live as you type; after signing
the rail's new card slides in at the top. Reduced motion: no motion.
Must not appear: four stacked dialogs, padlock clip-art, shields, ribbons, seals, glow, purple, sparkles, emoji,
people photos, real people's signatures.

## Shared

Render: one flat screenshot of one Windows 11 desktop application window, seen straight on, filling the whole
1536 x 1024 frame edge to edge. No board, no desktop background, no second window, no device frame, no perspective.
Crisp at 100 % scale like a real screenshot: large legible UI text (13-15 px UI labels, 16-18 px document body
text), sharp 1 px hairlines, sharp icon strokes, no blur, no compression smudge. Every word real, correctly spelled
English; never lorem ipsum, never squiggles standing in for text.
The app is Inkwell, a PDF app, matching the attached reference image's window, title bar, palette, type and rounded
shapes exactly. Window: slim custom title bar with the ink-drop mark and "Inkwell", a "Home" tab, an active rounded
tab "Lease agreement.pdf", a "+" button, and the window controls. Under it a slim toolbar with a rounded segmented
pill "Read | Organize | Comment | Sign" with Sign selected, undo and redo icons.
Document: page 4 of a residential lease agreement on white paper with a soft shadow, heading "7. SIGNATURES", the
sentence "By signing, the parties accept every term of this agreement." Then a small form: "Tenant name:" with a
line, "Date:" with a line, a checkbox row "[ ] I have received the keys", and two signature lines side by side,
labelled under the line "Landlord" and "Tenant".
Shape language: rounded and smooth (8-12 px radii, pill fields, soft diffuse shadows), no sharp corners.
Palette: window #F4F2ED; surfaces #FFFFFF; panel #FAF8F4; text #1A1C20; secondary #6B675F; borders #E3DED4; one UI
accent ink blue #2E4B8F only for the selected mode, the active tool and the primary button. Signature states only:
green #2F7D4F with a check, amber #B7791F with a question mark, red #B3261E with a cross; nowhere else.
Type: Segoe UI Variable style humanist sans.
Must not appear: square corners, boxy grids of cards, bright blue, purple, gradients, glow, glass, sparkles, emoji,
people photos, padlocks, shields, seals, ribbons, dark theme, red PDF icons, browser chrome, device frame, watermark,
lorem ipsum.

## Shared a-strip
Idea: the approved comments layout, for signing. A small floating vertical rounded strip at the left of the page with
seven tools: select, "Aa" text, cross, check, dot, line, date; a thin divider; then two small rounded tiles stacked,
the first showing a tiny handwritten "Alex Johnson" signature, the second the initials "AJ" in script.

## a-strip-1-fill
ref: ../../approved/comments.png
The form on the page is filled in with dark blue-black ink: "Alex Johnson" typed on the Tenant name line, "11 Oct
2026" on the Date line, a check mark in the keys checkbox, and a handwritten script signature "Alex Johnson" on the
Tenant signature line, selected with a thin ink-blue frame, four small round handles and a tiny floating rounded
toolbar above it with "Flatten", a size slider and a trash icon. The check tool is active in the strip. No right
rail is open; the page is centred.

## a-strip-2-sign
edit: a-strip-1-fill.png
Keep this screenshot exactly: same window, tabs, toolbar, strip, page and type. Change only this: on the Landlord
signature line there is now a dashed ink-blue rectangle, and the right side of the window has a 360 px rail on the
panel colour titled "Digital signature". In the rail: a rounded selector "Signing as" showing "Maple Property (test
ID)" with "Windows certificate store · expires 2031" under it; a white preview card showing, left, "Maple Property"
large and, right, small text "Digitally signed by Maple Property, Date: 2026.10.11 14:32 +02'00', Reason: Lease
signed"; a "Reason" pill field with "Lease signed"; a checkbox "Lock the document after signing"; a quiet grey line
"Saves a new revision. The tenant's signature stays valid."; and an ink-blue pill button "Sign".

## Shared b-panel
Idea: Acrobat's E-Sign panel, in Inkwell's rail. The right rail (360 px, panel colour) titled "Fill & Sign" holds:
a row of six round icon buttons "Aa", cross, check, dot, rounded rectangle, line; two dashed rounded tiles "Add
signature" and "Add initials" (the first already holds a small script "Alex Johnson" with a "..." button); then a
section "Digital signature" with a quiet button "Sign with a certificate" and a text link "Certify document".

## b-panel-1-fill
ref: ../../approved/comments.png
The page in the centre, filled in with dark blue-black ink: "Alex Johnson" on the Tenant name line, "11 Oct 2026" on
the Date line, a check in the keys checkbox, and a script signature "Alex Johnson" on the Tenant line. The "Aa" tool
is active (ink-blue), and a small text cursor blinks in a fresh text box after "11 Oct 2026".

## b-panel-2-sign
edit: b-panel-1-fill.png
Keep this screenshot exactly: same window, toolbar, page and type. Change only this: above the page a slim full-width
rounded banner on a very light green tint with a green check: "Signed. Both signatures are valid." and a text button
"Details". The rail is now titled "Signatures 2" and lists two rounded cards: "Maple Property" with a green check and
"Valid", "Certified · no changes allowed", "Signed 11 Oct 2026 14:32", "Windows certificate store"; and "Alex Johnson"
with an amber question mark and "Unknown signer", "Signature intact · signer not in your trusted list", "Signed 11
Oct 2026 14:20". The Landlord line on the page now shows a signature appearance box: "Maple Property" large with
small "Digitally signed by Maple Property, Date: 2026.10.11 14:32 +02'00'".

## Shared c-ribbon
Idea: everything happens on the page. A small horizontal floating rounded pill centred over the top edge of the page:
select, "Aa", cross, check, dot, line, date, a divider, a chip with a tiny script signature "Alex Johnson" and a
chip "AJ", then a divider and a pen-nib icon labelled "Certificate".

## c-ribbon-1-fill
ref: ../../approved/comments.png
The page large in the centre, filled in with dark blue-black ink: "Alex Johnson" on the Tenant name line, "11 Oct
2026" on the Date line, a check in the keys checkbox. The signature chip is active: a translucent script signature
"Alex Johnson" follows the pointer just above the Tenant line, ready to drop, with a thin ink-blue baseline guide.
No right rail.

## c-ribbon-2-sign
edit: c-ribbon-1-fill.png
Keep this screenshot exactly: same window, toolbar, pill, page and type. Change only this: the Tenant line holds the
placed script signature, and on the Landlord line a rectangle has been drawn that already shows the live signature
appearance in grey: "Maple Property" large with small "Digitally signed by Maple Property, Date: 2026.10.11". Anchored
to its right edge, a compact white rounded card with a soft shadow: "Sign as Maple Property (test ID)" with a small
chevron, a "Reason" field "Lease signed", a toggle "Lock after signing", the grey line "Saves a new revision. The
earlier signature stays valid.", and an ink-blue pill "Sign". The right rail is open, titled "Signatures 1", with one
card "Alex Johnson" with an amber question mark, "Unknown signer", "Signed 11 Oct 2026 14:20".
