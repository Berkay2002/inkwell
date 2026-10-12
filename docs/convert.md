# Export and Create PDF

Code: `src-tauri/src/convert/` (export.rs, create.rs, flow.rs, docx.rs, rtf.rs), commands in `src-tauri/src/lib.rs`
(`export_start`, `create_start`, `pick_source`, `pick_folder`, `discard_scratch`), UI in
`src/components/ConvertPanel.tsx` and `src/lib/convertStore.ts`. Screens: `docs/screens/convert/`
(`node scripts/convert-shots.mjs`).

## Export
Page images (PNG, JPEG, TIFF; 36 to 1200 dpi; gray; page range), text (plain or layout), HTML / XHTML,
embedded images, Word (.docx), Rich Text (.rtf). One file per page goes into a chosen folder; names never overwrite others
(`name (2)-1.png`). Files are staged in a hidden folder and moved into place at the end, so a cancel leaves
nothing.

Word and Rich Text come from the same reconstruction (`flow.rs`): paragraphs with size, bold, italic and
colour; Heading 1 and 2 by size; tables found from the text layout; pictures at their size on the page
(at most the text width); a page break between pages; the first page's size and margins. RTF is
`\rtf1\ansi\ansicpg1252` with one font (Helvetica), a colour table and a style sheet. Headings carry the
style (`\s1`, `\s2`) and the same formatting directly, so a reader that ignores the style sheet still shows
them. Every non-ASCII character is `\uN?` per UTF-16 unit (signed 16-bit N), so the file is plain ASCII.
Pictures are `\pngblip` hex; the file holds at most 32 MB of PNG data, and pictures past that are left
out with a warning. Not yet opened in Word or WordPad. macOS `textutil` read the text, the title and
the table (one table, nine cells) but drops pictures even from a minimal hand-made RTF, so the picture
is unconfirmed.

Tables: columns come from the cells' left edges. Each column is at least as wide as its longest word plus
the cell padding (5.4 pt a side; the word is measured from the cell's own width, else 0.6 em a character),
and when the table is wider than the text area the widest columns shrink first, never below that minimum
(`flow::col_twips`, shared by both writers). A table drawn cell by cell along each row comes out of MuPDF as
one block per row (or several rows), one line per cell; lines that sit side by side become their own
paragraphs, so the table is found from where its cells are, not from the order they were drawn in. The
pieces go back into the original block when no table takes them (a single pair of words with a wide gap),
and a list label (a bullet, "1.", "a)") beside its text is never a cell; pieces a table leaves in a block it
took from are one paragraph again, but only lines that stand directly below one another. The row pitch is
a line plus the white space between rows (measured bottom to top, so a cell that wraps over several lines is
not several pitches). Rows with more than 3 pitches of white space between them, or with text between them
under the table's width, are separate tables (a running header and a footer are not one). The leading
all-bold rows (the first with two or more filled cells) of a table with a non-bold body are its header rows,
which repeat on the next page; a bold caption that joined as a single-cell first row is marked with them, as
Word repeats only rows contiguous from the top. A line above or below a table joins it only when it starts
on a column edge, reaches the next column and sits within 1.5 row pitches.
Limits: a cell that wraps in a row-wise drawing becomes an extra row; and MuPDF keeps a column of tight rows
(pitch under about 1.5 times the size) drawn column by column as one paragraph, so that drawing is not found
as a table at that pitch although the same table drawn row by row is.

## Create
From images (JPEG is embedded untouched; PNG/BMP/GIF Flate; every page of a multi-page TIFF), a plain-text
file (UTF-8/UTF-16 BOM, else UTF-8, else cp1254/cp1252), blank pages, or the clipboard image. The result is
written under `<temp>/inkwell-new/` and opens as an unsaved tab: Save asks where it goes, closing asks first,
and the scratch file is deleted on save-as or close. Scratch tabs get no version history until saved.

## Gaps
- Word and RTF are a flowing reconstruction (paragraphs, headings, simple tables, images), not layout-exact.
- Neither carries underline, alignment, lists or the PDF's fonts (the model has no such fields); RTF uses one font.
- No .doc / .xlsx / .pptx / PostScript output.
- JPEG EXIF orientation is ignored.
- Image fit only shrinks: small images stay at their size on A4/Letter.
- Images without a stated resolution are sized at 96 dpi (MuPDF's default), so 1 px = 0.75 pt.
- Text pages use Helvetica (base 14): characters outside its range may be missing.
