# Preflight: PDF/A, PDF/UA and PDF/X

Engine: `src-tauri/src/preflight/`. It does what Acrobat's "PDF Standards" and Preflight tools do for the
common profiles: check a document against a standard, say which rules fail with the page and object, fix what is
safe, and convert (to a new file, or as one undo step in place). Tests are in `preflight/tests.rs` (generated PDFs,
a pass and a fail case per rule family, every fix survives save and reopen).

## What this is not

**This is not a veraPDF validation, and a file it passes is not certified.** Inkwell reads the object graph with
MuPDF and applies the rules in the tables below; it does not parse every clause of ISO 19005, 14289 and 15930-7, and
it does not decode font programs, validate colour profiles beyond their header, or check every content-stream
operator. A document that passes here can still fail a strict validator, and one that fails here may pass.

- The clause numbers in the report are **best effort and unverified**: they were written from memory of the
  standards, not checked against the ISO texts (which are not freely available). Where a number is not known the
  report says "ISO 19005" instead of inventing one. Treat the rule name and hint as the information, the clause as a
  pointer.
- veraPDF is not shipped and not a dependency. It may be used in tests only if it is installed locally
  (`E:\Tools`), never downloaded into the app.
- "Conforms" in the UI means *no rule that Inkwell checks fails*. The report states this under the verdict, and the
  HTML and PDF reports repeat it.

## Profiles

| id | Name | Family | Notes |
| --- | --- | --- | --- |
| `pdfa-1b` | PDF/A-1b | A1 (ISO 19005-1) | PDF 1.4: no transparency, no layers, no object streams, no embedded files |
| `pdfa-2b` | PDF/A-2b | A23 (ISO 19005-2) | PDF 1.7: transparency and layers allowed; embedded files only if PDF/A |
| `pdfa-3b` | PDF/A-3b | A23 (ISO 19005-3) | like 2b; any embedded file, with an `AFRelationship` |
| `pdfa-2u` | PDF/A-2u | A23 | like 2b, plus every font maps to Unicode |
| `pdfua-1` | PDF/UA-1 | UA (ISO 14289-1) | the accessibility checker's rules plus identification, XMP title, fonts |
| `pdfx-4` | PDF/X-4 | X (ISO 15930-7) | **basic**: see below |

PDF/A-1a, 2a, 3a, 3u and PDF/A-4 are not offered. PDF/A "a" levels need a tagged structure tree and reading order,
which the accessibility checker covers as PDF/UA, not as a PDF/A level.

## Commands

All in `preflight/app.rs`; the assistant calls the functions in `preflight/mod.rs` with its own `EditSource`.

- `preflight_check(job, doc, profile, onProgress) -> Report` runs off the UI thread. `onProgress` is
  `{done, pages, stage}` with `stage` one of `check`, `survey`, `objects`. `tool_cancel(job)` stops it between pages
  (`cancelled`). It reads the document as it is now, unsaved edits included, from its own copy of the current
  generation.
  `Report = {profile, pages, claimed, conforms, results, plan}`. A result is
  `{rule, name, group, clause, status, hint, fix, a11yFix, count, findings}`; a finding is
  `{page, object, xref, rect, message, fixable, newFile, note}`. `status` is `passed`, `failed`, `manual` (a person
  must look: PDF/UA) or `info` (does not fail the profile: PDF/X-4 RGB content). A rule keeps its first findings and
  `count` is the total. `claimed` is what the file's XMP says it conforms to (`PDF/A-2b`), whether it does or not.
- `preflight_claim(doc) -> {label, profile} | null` reads only the XMP identification; the badge in Document
  properties uses it.
- `preflight_fix(job, doc, profile, convert, onProgress) -> EditState` converts the open document as **one undo
  step** (Save keeps it, history records it). `convert = {groups, lang}`: the fix groups to run and the language the
  accessibility group sets when there is none. A group with nothing to do is skipped; the step is refused when
  nothing applied. In place is refused for an encrypted document (a PDF/A file has no encryption and an in-place
  edit would keep it): convert to a new file instead.
- `preflight_convert(job, doc, profile, convert, to, onProgress) -> {path, report, fixed}` writes a new file to the
  save token `to`. The open document is untouched; the command refuses a target that is an open document
  (`Engine::check_not_open`). The result is checked against the same profile and returned, with what was done as
  sentences.
- `preflight_export_report(job, doc, profile, format, date, to, onProgress) -> path` checks again and writes an
  HTML or PDF report (the PDF with the comparison report's writer), refusing an open document as the target.

## Fix groups

A conversion is a set of groups, applied in this order: structure, actions, annotations, forms, images, files,
colour, fonts, appearance, accessibility, metadata (metadata last so it mirrors what the others set). Each group is
idempotent: converting a converted document finds nothing to do.

| Group | Does | Only in a new file |
| --- | --- | --- |
| structure | writes the `/ID`; re-encodes LZW streams without LZW; removes the automatic layer states (`/AS`) in PDF/A-2 and -3; sets a TrimBox from the crop or media box (PDF/X) | the file header (`%PDF-1.x` and the binary comment line), no encryption, no object or xref streams |
| actions | removes Launch, Sound, Movie, ResetForm, ImportData, Hide, SetOCGState, Rendition, Trans, GoTo3DView, JavaScript and Named actions other than the four navigation ones; the document JavaScript tree; `AA` entries | |
| annotations | removes sound, movie, screen, 3D and rich media annotations (file attachments in A-1); sets Print, clears Hidden, Invisible and NoView | |
| forms | draws the field appearances into the file when `NeedAppearances` is set (which also gives those widgets the appearance they lacked) and removes XFA. A missing appearance on any other annotation is reported, not created | |
| images | `Interpolate false`; removes `Alternates` and `OPI` | |
| files | A-1: removes embedded files; A-2: removes the embedded files that are not PDFs (a PDF is kept, by its media type or its first kilobyte; whether it is itself PDF/A is left to a person); A-3: gives each an `AFRelationship`, a media type (`application/octet-stream` when it has none) and a place in the catalog `/AF` array | |
| colour | adds a `GTS_PDFA1` output intent with a small sRGB ICC v2 profile (written in `icc.rs`) when the document has none and uses no DeviceCMYK. CMYK needs a print condition you choose, so a CMYK document is left alone and the log says so. PDF/A allows one output profile in the file: when an output intent (a PDF/X one, say) already has a 3-component profile the new intent shares that very profile object, and when it has another component count nothing is added and the log says why. Device colour is never converted | |
| fonts | embeds the fonts that are missing (below); adds `/CIDToGIDMap /Identity` where absent | |
| appearance | PDF/A-1: sets opacity to 1, removes soft masks and non-Normal blend modes, drops transparency groups. All PDF/A: removes transfer functions (`TR`, `TR2`). Both change how pages look | |
| accessibility | PDF/UA only: the accessibility checker's safe fixes (title, language, tab order, display title, field tooltips) | |
| metadata | writes or updates the XMP packet: the identification schema (`pdfaid`, `pdfuaid` or `pdfxid`) and title, author, subject, keywords, producer, creator and dates mirrored from the document information (properties of other schemas in an existing packet are kept); the stream is stored unfiltered. PDF/X also gets `CreationDate`, `ModDate` and `Trapped` in Info. **The identification is written only when the file passes the profile's other rules** (the step checks the document as the other groups left it; for a new file the header, encryption and object-stream rules count as settled because the write fixes them). In the PDF/A profiles a rule that waits for a person's check (an embedded PDF's own conformance, an attachment without a modification date) holds the identification back as well. Otherwise the XMP is only synced with Info, an identification the file already carried is taken out (the file was changed here and the check does not back it), and the log says "Did not identify the file as ...: N rules still fail or need a check". For a new file the written bytes are checked once more: if the file was identified and that check still finds a failure or a manual rule, nothing is written and the conversion reports which rules. The new file's suggested name carries "(PDF-A-2b)" only when the conversion is expected to identify it, otherwise "(converted)". The per-rule Fix of "Standard identification" therefore runs the ticked groups, not Metadata alone | |

**Appearance is not ticked by default.** It is the one group that changes how pages look, so the outcome card lists
it unticked with its count and the person switches it on (the studio round pdfa-r1 verdict made every other group
ticked by default). A-1 needs it for transparency; without it the file still fails 6.4 and says so.

### Fonts: never substituted silently

A font that is not embedded is embedded from, in this order:

1. the built-in clone of the standard 14 font it is named after (Helvetica, Times, Courier and their styles);
2. the installed TrueType file of the same name (`engine/fonts.rs::installed_file`), unless its `fsType` licence
   flags forbid embedding.

Anything else is **blocked**, left as it is and named in the report and in the outcome line ("2 need fonts that
aren't installed"): a subset whose font file is missing, a CID font with no file, Symbol and ZapfDingbats, a
font that is not installed, one whose licence forbids embedding. The widths are rebuilt from the program, and the
text keeps extracting. The built-in clones are embedded whole; installed fonts are subset to the glyphs the pages
use afterwards (if subsetting fails they stay whole and the log says so). MuPDF subsets every font of the document
at once, so the subsetting runs only when nothing else would be cut down with them: it is skipped, and the fonts
stay whole with a log line, when a font's `fsType` forbids subsetting (0x0100), when the document has embedded fonts
of its own, or when default resources (`AcroForm /DR`, or a widget's or its parent field's own `/DR`) use a font the conversion embedded. A TrueType font
embedded from an installed file gets the plain name `WinAnsiEncoding` in PDF/A-1 (no Differences array, so the
quote marks use WinAnsi; when it replaces a Differences array the log says that characters it remapped may draw as other glyphs) and WinAnsi with the few Differences StandardEncoding needs in the later parts. A blocked font does not stop the other
groups: the file is converted as far as it can be and the report shows the rest.

## Rules

36 rules in report order. `A1`, `A2/3`, `UA`, `X` say which family a rule belongs to; the clause is the one shown in
the report (unverified, see above). "Fix" is the group that repairs it; "none" means it is reported only.

| id | Checks | A1 | A2/3 | UA | X | Fix |
| --- | --- | --- | --- | --- | --- | --- |
| `file-header` | `%PDF-1.n` and four high bytes in the comment line | 6.1.2 | 6.1.2 | | | structure (new file) |
| `trailer-id` | trailer `/ID` of two strings | 6.1.3 | 6.1.3 | | ID | structure |
| `encryption` | no `/Encrypt` | 6.1.3 | 6.1.4 | | | structure (new file) |
| `lzw` | no LZWDecode stream | 6.1.10 | 6.1.10 | | | structure |
| `external-streams` | no stream with an `/F` key | 6.1.7 | 6.1.7 | | | none |
| `object-streams` | no object or xref streams | 6.1.4 | | | | structure (new file) |
| `optional-content` | A-1: no layers; A-2/3: no `/AS` | 6.1.13 | 6.9 | | | structure (A-2/3 only; A-1 layers are reported, not removed) |
| `page-boxes` | TrimBox or ArtBox inside MediaBox | | | | boxes | structure |
| `xmp-present` | catalog `/Metadata` XMP stream | 6.7.2 | 6.6.2.1 | XMP | XMP | metadata |
| `xmp-id` | `pdfaid:part`+`conformance` (A), `pdfuaid:part` (UA), `pdfxid:GTS_PDFXVersion` (X) matches the profile | 6.7.11 | 6.6.4 | pdfuaid | pdfxid | metadata |
| `xmp-sync` | XMP title, author, subject, keywords, producer, creator, dates equal Info | 6.7.3 | 6.6.2.3 | dc:title | XMP | metadata |
| `xmp-unfiltered` | the XMP stream has no filter | 6.7.2 | 6.6.2.1 | | | metadata |
| `info-dates` | Info `CreationDate`, `ModDate` | | | | Info | metadata |
| `trapped` | Info `Trapped` is `True` or `False` | | | | Trapped | metadata |
| `output-intent` | an output intent when device colour is used | 6.2.2 | 6.2.3 | | GTS_PDFX | colour |
| `output-intent-profile` | the destination profile has the ICC `acsp` signature, a size that fits, a known colour space and an `N` that matches it | 6.2.2 | 6.2.3 | | GTS_PDFX | none |
| `device-colour` | DeviceRGB/CMYK/Gray has a matching intent or a `Default` colour space | 6.2.3.3 | 6.2.4.3 | | | none |
| `rgb-content` | RGB is used (information, does not fail) | | | | info | none |
| `fonts-embedded` | every font has a font file (Type 3 excepted) | 6.3.4 | 6.2.11.4.1 | 7.21.4.1 | fonts | fonts |
| `fonts-cidtogid` | embedded CIDFontType2 has `CIDToGIDMap` | 6.3.? | 6.2.11.3.2 | | | fonts |
| `fonts-unicode` | every font maps codes to Unicode (PDF/A-2u only) | | 6.2.11.7 | | | none |
| `image-interpolate` | no `Interpolate true` | 6.2.4 | 6.2.8 | | | images |
| `image-alternates` | no `Alternates`, no `OPI` | 6.2.4 | 6.2.8 | | OPI | images |
| `image-jpx` | no JPXDecode (A-1) | 6.2.4 | | | | none |
| `ps-xobject` | no PostScript XObject, no `PS` | 6.2.7 | 6.2.7 | | | none |
| `transparency` | A-1: no `SMask`, `CA`/`ca` < 1, blend mode, transparency group | 6.4 | | | | appearance |
| `transfer-functions` | no `TR`/`TR2` other than identity or `Default` | 6.2.9 | 6.2.5 | | | appearance |
| `actions-forbidden` | no forbidden action in links, outlines, fields, OpenAction | 6.6.1 | 6.5.1 | | | actions |
| `javascript` | no document JavaScript name tree | 6.6.1 | 6.5.1 | | | actions |
| `actions-additional` | no `AA` on catalog, pages, annotations, fields | 6.6.2 | 6.5.2 | | | actions |
| `annotation-types` | no sound, movie, screen, 3D, rich media (A-1: no file attachment) | 6.5.2 | 6.3.1 | | | annotations |
| `annotation-flags` | Print set, Hidden/Invisible/NoView clear | 6.5.3 | 6.3.2 | | | annotations |
| `annotation-appearance` | every annotation but link and popup has an appearance stream | 6.5.3 | 6.3.3 | | | forms (widgets when `NeedAppearances` is set; others none) |
| `need-appearances` | `NeedAppearances` is not true | 6.9 | 6.4.1 | | | forms |
| `xfa` | no `XFA` | 6.9 | 6.4.2 | | | forms |
| `embedded-files` | A-1: none; A-2: only PDF/A; A-3: with `AFRelationship` | 6.1.11 | 6.8 | | | files |

PDF/UA-1 adds the accessibility checker's own rules (`crate::a11y::check`: tagged PDF, title, language, figure alt
text, tables, lists, headings, tab order, links, field tooltips, fonts to Unicode, bookmarks, colour contrast), shown
under their checker groups with the clause "ISO 14289-1". Its `skipped` rules are left out; colour contrast and
reading order are `manual`. A failed rule that has an accessibility-checker fix (`a11yFix`) runs that fix: one undo
step, the same one the Accessibility rail would run.

## What is not checked

Honest list. A document can fail any of these and pass here.

**PDF/A (all parts)**
- Syntax beyond what MuPDF repairs on load: a damaged cross-reference table, stream length mismatches, a missing
  `endobj`, generation numbers, name and string length limits, integer and real ranges, nesting depth, the number of
  indirect objects, page size limits (6.1.5, 6.1.6, 6.1.8, 6.1.12 and their A-2/3 equivalents).
- Content stream operators: undefined operators, operand types and counts, `BX/EX` use, inline image keys and
  filters, rendering intent values (6.2.10 / 6.2.9), `ri`, and `Do` of an image with unlisted keys. Content
  streams are scanned for resources used, device colour operators and `gs`, not validated.
- Font programs: the glyph set is not compared with the widths, `Encoding` and `Differences` of symbolic
  TrueType fonts (the 6.3.7 cmap rules), CFF and Type 1 structure, `CharSet` and `CIDSet` correctness, the
  `W`/`Widths` agreement, `Type0` encodings other than a presence check, and the embedding of composite font
  `CMap`s.
- Colour: the ICC profile gets a header check only (signature, size, colour space, `N`): not its version against
  the PDF version, its profile class, its tags or whether it is a usable profile; Separation and DeviceN alternate
  spaces and `Lab` ranges; the blending colour space of transparency groups (A-2/3); the `Indexed` base; rendering
  intent names.
- Images: `JPXDecode` colour specification and bit-depth rules in A-2/3, `SMaskInData`, image masks with
  `Decode`, and the `Intent` entry.
- Transparency in A-2/3 is allowed and not examined at all (no group colour space or knockout checks).
- Annotations: the appearance dictionary's structure (`N` as a state dictionary for widgets with states), colours
  of annotations (`C`, `IC`), `Popup` rules, and the flag rules beyond Print, Hidden, Invisible and NoView.
- Forms: field-level `AA` beyond the action check, and `Lock`/`SV` details.
- Metadata: the XMP is read by a small text reader, not an XML parser or a schema validator: extension schema
  declarations for non-predefined properties (6.7.8/6.7.9), property value types and the `pdfaExtension`
  container are not checked, and only the properties named in `xmp-sync` are compared with Info.
- Logical structure (A "a" levels): not offered. The `Lang` and `MarkInfo` rules of A-1a/2a/3a are checked only as
  part of PDF/UA.
- Layers beyond the `/AS` rule: configuration `Name`, `Order`, `BaseState` and `OCG` usage are not checked.
  Named destinations and outline `Dest` validity are not checked either.
- Embedded PDF files in A-2 are only checked for being a PDF, **not** for being PDF/A themselves (the report says
  "must itself conform", status manual). Whether an attachment is a PDF is read from its media type and its first
  kilobyte.
- PDF/A-3 embedded files: the media type, `/AFRelationship` and the catalog `/AF` listing of files in the
  `EmbeddedFiles` name tree are checked; the `/AF` association of file attachment annotations and the value of the
  media type are not. A missing `/Params /ModDate` is shown as a manual check and never made up.
- TrueType encodings: in PDF/A-1 a non-symbolic TrueType font needs `WinAnsiEncoding` or `MacRomanEncoding` and no
  Differences. The conversion fixes the fonts it embeds itself; fonts that were already embedded with another
  Encoding are left as they are and named in the log, and no rule reports them.
- Digital signatures: not validated, and converting a signed file invalidates its signatures (the file is rewritten).

**PDF/UA-1**: whatever the accessibility checker does not check (see `docs/a11y.md`): reading order is a
heuristic, colour contrast is manual, the clauses on tables, lists and headings are the structural ones only, and
there is no check of `Alt` text quality, role map cycles, artifacts tagged as content, or the 7.x character
encoding rules beyond Unicode mapping.

**PDF/X-4 is basic.** It checks: output intent with `GTS_PDFX` and a valid ICC profile, `GTS_PDFXVersion` in the
XMP, trailer `/ID`, TrimBox or ArtBox, Info dates and `Trapped`, embedded fonts, no `OPI`, XMP present and in sync,
and it **lists RGB use as information** (PDF/X-4 allows it with an output intent). It does not check: the print
condition against a registry, overprint and spot colour rules, total ink, transparency and blend colour spaces,
image resolution, bleed, `OutputCondition` text, page-level `Trapped`, or PDF/X-4p external profiles. Nothing is
converted to CMYK. Use a prepress tool for press files.

## The UI

`src/lib/preflightLogic.ts` (pure decisions, tested), `src/lib/preflight.ts` (store and commands),
`src/components/preflight/PreflightPanel.tsx` and `ClaimBadge.tsx`. Tools > Preflight... and the palette
("Preflight (PDF/A, PDF/UA, PDF/X)...") open a right rail like Accessibility (opening one closes the other and the
rest). It reuses the Accessibility rail's classes, so the two read the same:

- a **profile select** with a line about it; changing it checks again;
- the **verdict** ("Doesn't meet PDF/A-2b", "No rule fails for PDF/A-2b") with a segmented bar and the claim the
  file makes about itself;
- the **outcome card** for the PDF/A profiles (PDF/UA and PDF/X-4 have the per-rule Fix buttons only): "Convert to PDF/A-2b", a checklist of the fix groups each with its
  count and what it does, ticked by default (Appearance is not), the **predicted outcome line** that follows the
  ticks live ("Fixes 14 of 16 issues; 2 need fonts that aren't installed"), a language select when the
  accessibility group needs one, a choice of **New file** or **This document** (one undo step), and the button
  ("Save as new file..." asks for a name, "Convert in this document");
- a card after a conversion to a new file: its verdict against the same profile, what was done, Open and Show in
  folder;
- the **rules under their groups**, failed first, each with its hint, clause chip, findings with page and object,
  **Show** (jumps above the object and outlines it) and **Fix** for the safe ones; a rule only a new file can fix
  says so;
- **Export report** (HTML or PDF) in the footer, like the accessibility report.

Document properties (Tools > Security...) shows **"Says it is PDF/A-2b"** (no tick: it is the file's own claim)
when the XMP says so, with the note "This is what the file says about itself" and a **Verify** link that opens the
rail on that profile; once the rail has checked this revision the note becomes "No failures in Inkwell's checks for
PDF/A-2b (not a full validation)." or "Not verified: 3 rules fail".

The report is read again after any change (a conversion, an undo, an edit), a cancelled or failed check waits for
Check again, and a report that no longer matches the document disables Fix and Convert until it is read again.

## Verified

- `cargo test --lib preflight` (generated PDFs): every rule family pass and fail; in-place convert is one undo step
  that survives save and reopen; convert to a file leaves the source untouched and is refused over an open
  document; a blocked font is left and reported; built-in and installed (Arial) embedding with text extraction;
  encrypted in place refused, new file fine; A-1 Appearance opt-in; PDF/UA convert; PDF/X-4; plan arithmetic;
  HTML and PDF report; cancel; 1000 pages.
- `src/lib/preflight.test.ts` (vitest): outcome line arithmetic and live updates, store flows, claim and Verify.
- End to end on copies of `docs/study` samples with `scripts/pdfa-shots.mjs`; screens in `docs/screens/pdfa/`.

## Attribution

The rule families, the ICC profile writer and the XMP packet shape are adapted from pdfcraft's `preflight` crate
(MIT OR Apache-2.0, `b8ddf71`); each adapted file keeps its header and has an entry in
`third_party/pdfcraft/INKWELL-NOTICE.txt`. veraPDF is not used by the app.
