# Pdfa r1: Preflight, PDF/A and PDF/UA (phase 5, pdfa)

First round of this surface. It reuses the accessibility rail's rule / report / fix shape (docs/a11y.md, design/rounds/a11y-r1)
and the right-rail pattern the Tools, Redact, Comments, History and Accessibility rails share (one right-side panel at a
time, same dock and head). One app window per image at 1536x1024; two images per variant (`-1-report`, `-2-convert`).

What the surface must carry (task brief): opens from Tools > "Preflight..." and the palette; a profile picker (PDF/A-1b, 2b,
3b, 2u, PDF/UA-1, PDF/X-4); findings grouped like the accessibility rail, each with page and object, "Show", and a fix where
it is safe; check off the UI thread with progress and cancel; "Convert to PDF/A" with the outcome predicted first ("Fixes 14
of 16 issues; 2 need fonts that aren't installed"), written either to a new file (save token, never over the open
document) or as one undo step in place; report export (HTML/PDF); and a "Conforms to PDF/A-2b" badge in Document
properties when the XMP claims it, with a "Verify" link that runs the profile.

What the engine can and cannot do, so the screens do not promise more: it decides rules from the object graph (clause list in
docs/pdfa.md); it is not veraPDF. Fixups: XMP identification and Info sync, sRGB output intent, trailer ID, forbidden
actions / JavaScript / additional actions, annotation flags and forbidden annotation types, NeedAppearances, image
interpolation / alternates / OPI, transparency (PDF/A-1 only; changes how pages look), embedded files, LZW streams, font
embedding (the built-in base-14 metric clones and installed Windows fonts; never a silent substitute: a font with no file is
reported and blocks the conversion). Not fixable: CMYK without a CMYK output condition, missing annotation appearances,
fonts addressed by CID with no file, JPEG 2000 in PDF/A-1.

## Three ideas

- **a-rail** is Acrobat's PDF Standards / Preflight result tree in Inkwell's accessibility rail: a profile picker on top,
  one verdict line ("Does not conform to PDF/A-2b: 6 of 31 rules failed"), the rules under groups (File, Metadata, Colour,
  Fonts, Images, Actions and annotations, Embedded files), each rule a row with a status glyph, its clause, Fix and Show; a
  card pinned at the top with the predicted outcome and the two ways to convert.
- **b-sheet** is Acrobat's Preflight dialog as a centred sheet: the profile list on the left (Acrobat's folders: PDF/A, PDF/X,
  PDF/UA), the result tree on the right, "Analyze" and "Analyze and fix" at the bottom. One sheet, modal.
- **c-ticket** is one outcome-first sheet: "This file can become PDF/A-2b" with the fix list as a checklist, the two
  destinations as a segmented control, and the findings behind a disclosure.

## How Acrobat Pro does it

Read from the shipping files (docs/study/15-preflight.md; nothing was driven). Acrobat's "PDF Standards" tool offers Save as
PDF/A, PDF/X, PDF/E and Preflight (profiles: Verify compliance with PDF/A-1b, Convert to PDF/A-1b ..., fixups listed per
profile). Converting runs "Analyze and fix" and saves a new file (Save as Other > Archivable PDF). The shipped PDF/A job
options say what a conversion does: embed all fonts, **error when a font cannot be embedded**, sRGB conversion, no
transparency for PDF/A-1b, compatibility 1.4, no object streams. The result is a tree of Errors / Warnings / Info per rule,
with "Show" on a finding and a report you can create.

## How pdfcraft does it

A "Standards" panel under Document properties shows what the file declares (PDF/A part and conformance, PDF/UA, output
intents) with "Verify" and "Convert". Verification covers PDF/A-2b and 3b (XMP, encryption, output intent, fonts,
annotations, actions, images, embedded files); conversion fixes the structural things, keeps the pages as they are and
returns what remains. Fonts that are not embedded are not fixed.

## Our options and where they differ

All three keep the verdict and the fix list from Acrobat. **a-rail** follows Acrobat's structure and our other rails
exactly (one right-side panel at a time, the accessibility rail's rows), so a person who learned one has learned both. The
Inkwell touches: the outcome predicted before anything runs, the choice of destination (new file is recommended; in place is
one undo step), and "never silently substitute" shown as a blocked line with its reason instead of a successful-looking
conversion. **b-sheet** is the nearest to Acrobat's Preflight dialog but a modal with two panes is the opposite of our "one
panel, the document stays visible" decision. **c-ticket** is the calmest for the conversion and weakest for reading
findings.

## Verdict

**a-rail**, confirmed by the orchestrator (the image tool was not available, nothing was rendered). It is the most
standard variant and sits in the accessibility rail's look. One touch is taken from c-ticket: the outcome card carries a
small checklist of the fix groups (Fonts, Colour and output intent, Metadata, Actions and JavaScript, Embedded files,
plus Appearance for what changes how pages look). Each group is ticked by default and shows its own count; unticking
one updates the predicted outcome line ("Fixes 11 of 16 issues; 2 need fonts that aren't installed") at once, and the
conversion runs only the ticked groups. Groups that have nothing to do are not listed; a group that cannot run (fonts
without a file) is shown unticked and disabled with its reason.

## Shared

Same render brief as design/rounds/a11y-r1/spec.md (Shared): flat 1536x1024 Inkwell window, right rail about 400 px, ink-blue
accent only for the primary button, focus and selection, status marks in green/red/amber with a glyph and a word.
