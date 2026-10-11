# Export and Create PDF

Code: `src-tauri/src/convert/` (export.rs, create.rs, docx.rs), commands in `src-tauri/src/lib.rs`
(`export_start`, `create_start`, `pick_source`, `pick_folder`, `discard_scratch`), UI in
`src/components/ConvertPanel.tsx` and `src/lib/convertStore.ts`. Screens: `docs/screens/convert/`
(`node scripts/convert-shots.mjs`).

## Export
Page images (PNG, JPEG, TIFF; 36 to 1200 dpi; gray; page range), text (plain or layout), HTML / XHTML,
embedded images, Word (.docx). One file per page goes into a chosen folder; names never overwrite others
(`name (2)-1.png`). Files are staged in a hidden folder and moved into place at the end, so a cancel leaves
nothing.

## Create
From images (JPEG is embedded untouched; PNG/BMP/GIF Flate; every page of a multi-page TIFF), a plain-text
file (UTF-8/UTF-16 BOM, else UTF-8, else cp1254/cp1252), blank pages, or the clipboard image. The result is
written under `<temp>/inkwell-new/` and opens as an unsaved tab: Save asks where it goes, closing asks first,
and the scratch file is deleted on save-as or close. Scratch tabs get no version history until saved.

## Gaps
- Word is a flowing reconstruction (paragraphs, headings, simple tables, images), not layout-exact.
- No .doc / .xlsx / .pptx / PostScript output.
- JPEG EXIF orientation is ignored.
- Image fit only shrinks: small images stay at their size on A4/Letter.
- Images without a stated resolution are sized at 96 dpi (MuPDF's default), so 1 px = 0.75 pt.
- Text pages use Helvetica (base 14): characters outside its range may be missing.
