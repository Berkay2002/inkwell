# Print

Native Windows printing from the Rust core, with a live preview that is the printer's own drawing.
Code: `src-tauri/src/print/` (`layout.rs` pure sheet layout, `render.rs` MuPDF sheet renderer, `pdf.rs` Save as PDF,
`win.rs` GDI job, `app.rs` Tauri commands). UI: `src/components/PrintSheet.tsx`, `src/lib/printStore.ts`. Entry: Ctrl+P and the
command palette ("Print…").

## How it works

1. **Plan.** `print_plan(doc, settings)` picks the pages (range, current, odd/even, reverse), lays them on sheets
   (`layout::layout`) and expands copies, collation and two-sided padding (`layout::sequence`). It returns the
   number of sides per copy, the physical sheets, and each side's size and pages: that is the summary line and the
   pager of the sheet.
2. **Preview.** `print_preview(doc, settings, sheet, maxPx)` draws side N with the *same* function the printer path
   uses (`render::render`), only at screen size. Placement, scale, rotation, clipping, borders and grayscale are
   therefore the same as on paper.
3. **Print.** `print_start(job, doc, settings, saveTo, onProgress)` runs off the UI thread: a private parse of what
   the document shows now (unsaved edits included), then one GDI page per side at the printer's resolution, in
   bands (`BAND_BYTES`), with progress per side and cancel through `tool_cancel(job)`. Printing is not an edit: no
   undo step, no history entry, the open document is not touched.

The layout works in points, y down, top-left origin, on a sheet of the chosen paper (portrait width/height from the
driver, swapped when the sheet is landscape). A fixed 18 pt margin keeps content off the edge; pages per sheet use a
6 pt gap.

4. **Save as PDF.** `print_save_pdf(job, doc, settings, saveTo, onProgress)` writes the same plan as a new PDF
   (`pdf::save`), one page per side. It needs no printer and works on every platform. See "Save as PDF" below.

## Options

| Option | Behaviour |
|---|---|
| Pages | all, current page, range text (`1-3, 5, 8-`), odd or even among the chosen pages, reverse |
| Copies, collate | up to the driver's maximum; copies are sent explicitly (collated or not), so the result does not depend on the driver's own collation |
| Scaling (one per sheet) | fit, shrink oversized pages only, actual size, custom percent |
| Several per sheet | 2, 4, 6, 9, 16 or any grid up to 100; order across/down, optional border, turn pages to fit |
| Booklet | saddle-stitch imposition, both sides / front only / back only, binding left or right; both-sides booklets go out short-edge two-sided, a front-only or back-only booklet goes out one side at a time (simplex) |
| Poster | each chosen page enlarged by the tile scale (percent, default 100) and cut into sheet-sized tiles that overlap by 0 to 50 mm (default 6 mm), sheets in reading order (across, then down). Optional cut marks and labels. |
| Creep (booklet) | 0 to 3 mm (default 0): the pages of the innermost sheet sit that much closer to the spine than those of the outermost, growing evenly between |
| Orientation | automatic (per sheet, by the page), portrait, landscape |
| Two-sided | off, long edge, short edge (only when the printer reports duplex). The DEVMODE flag follows the documented absolute meaning: long edge is `DMDUP_VERTICAL`, short edge `DMDUP_HORIZONTAL`, whatever the orientation (a booklet is landscape and short-edge). |
| Grayscale | drawn as grey in the same pass |
| Comments and forms | document (no markup), document and markups, form fields only. Annotations without the PDF Print flag are not printed. |

## Poster

`Layout::Poster { scale, overlap, cut_marks, labels }`. A page of display size `d` at tile scale `s` is `d * s` points;
a tile shows `printable - overlap` more of it than the one before, where the printable area is the sheet minus the
18 pt margin. Tiles per page = `ceil((d * s - overlap) / (printable - overlap))` each way, at least one. The sheet
is oriented by the page when the orientation is automatic. An overlap of half a tile or more is refused.

- **Cut marks** are short ticks in the margin, at both ends of every edge of the overlap strip that has a
  neighbour: where the neighbour's printed area begins (right and bottom) or ends (left and top). With no overlap
  they sit on the printable edge. (pdfcraft draws corner ticks at the printable area instead.)
- **Labels** are one line of 7 pt Helvetica in the top margin: `Page 3, row 2 of 3, column 1 of 4`.
- **Limit:** 1024 sheets per job, counted over all chosen pages before anything is built; a larger job is refused
  with a message.
- The plan's summary reads the grid from the tiles: "12 sheets of A4, 3 x 4 tiles per page".

## Booklet creep

Thick paper pushes the inner pages outward when folded, so the inner sheets' pages are moved toward the spine. With
`creep` c and p sheets of paper, sheet k (0 outermost) shifts `c * k / (p - 1)`; both pages of a sheet and both
sides of the paper move together. To make room, every page cell keeps `c` clear of the fold (pages are fitted to
slightly narrower cells when creep is on), and the clip moves with the page, so nothing crosses the fold. One sheet
of paper gets no creep. The summary says "creep up to 0.4 mm at the centre".

## Save as PDF

The same sheets as the preview and the printer, written as a vector file:

- One Form XObject per source page used, shared by every tile or cell that shows it, with the page's resources
  grafted once per job (a font used by 50 pages is in the file once). Each placement is `q <clip> re W n <cm> /Fm Do Q`.
- Comments and forms follow the content mode: the job runs on the same private copy the printer path parses
  (unsaved edits included); markups without the Print flag are hidden, then MuPDF bakes annotations and form fields
  into the page content (it synthesises missing appearances). "Form fields only" drops the page content first.
- Pages not chosen lose their annotations and chosen pages get their own Resources before baking, so no comment of
  an unused page can ride along in a shared Resources dict. Layers keep their on/off state (the catalog's
  `/OCProperties` is grafted) and a page's transparency `/Group` stays on its form.
- Borders, cut marks and labels are vector operators on the sheet page.
- Copies and collation are ignored; two-sided padding is kept (a blank back is as large as the sheet before it), so
  a booklet printed double-sided from the file comes out right. Grayscale is ignored: the file keeps colour, and the
  sheet greys the toggle out when there is no printer.
- The result goes to a path from a `pick_save` token, never over a file that is open in Inkwell, through the same temp-file-and-read-back step the other tools use, and is offered
  in a new tab. The open document is not changed.
- A document whose owner forbids copying is refused (`Rights::may_extract`): vector output makes its text copyable,
  which a printer never did. Printing itself still needs the Print permission.
- Cancel through `tool_cancel(job)`; progress counts pages written.
- With no printer installed the sheet opens, plans and previews on A4, Letter, Legal, A3 or A5, and Save as PDF works.

## Printers and files

Printers come from `EnumPrinters`; paper sizes, trays, duplex, colour and copies from `DeviceCapabilitiesW`.
Print-to-file printers (Microsoft Print to PDF, the XPS writer) ask where to write through the save dialog (a
path token, `paths.rs`); `INKWELL_PRINT_DIR` (debug builds only) writes them into that folder without asking, for automation.
A document that forbids printing (`Permission::PRINT`) is refused with `not-permitted`.

## Tests

- `cargo test --lib print:: -- --test-threads=1`: layout, imposition, sequence, preview, content modes, and the
  round trip: unsaved markup prints, survives save and reopen, and prints again the same.
- Printer tests (`mod printer`, real "Microsoft Print to PDF" to a temp file, then read back with MuPDF to check
  page count and sheet sizes) are opt-in: `INKWELL_PRINT_TESTS=1`. They never touch a physical printer.
- `print::tests_layouts`: poster tile count, overlap, cut-mark and label positions, the sheet cap; creep; band
  skipping of margin marks; Save as PDF on generated fixtures (page counts, text order, forms not images, one form
  per page, markups by content mode, the open file untouched, the output equal to the preview page for page,
  no comments of unused pages in shared resources, layers off, `/Group`, a turned page with an offset crop box,
  form fields only, a no-copy document refused).
- vitest: `src/lib/printStore.test.ts` (settings mapping, paper choice, remembered options, the sheet's
  open/plan/preview/start/cancel flow, poster and creep mapping, Save as PDF with and without a printer).
- Runtime: `scripts/print-shots.mjs` drives the sheet through CDP and prints a copy of a generated sample to
  Microsoft Print to PDF into `INKWELL_PRINT_DIR`; screens in `docs/screens/print/`.

## Not done, and known limits

- The margin is a fixed 18 pt, not the printer's hardware margins (the preview shows the same margin, so it still
  matches what the sheet does, but a driver that clips more will clip more).
- Output through GDI is raster at the printer's resolution (capped at `win::MAX_DPI`), not vector.
- Two-sided flags follow the documented absolute DEVMODE semantics (unit-tested as a mapping) and were not
  checked on duplex hardware: only the plan and the capability call were.
- Paper is capped at 14400 pt (the PDF page limit); a larger size is refused. When no paper was chosen and the
  printer's own paper differs from the A4 the preview assumed, the job is laid out again on the printer's paper.
- Progress counts sides sent (copies and padded blank backs included), shown as "Side n of total".
- Not built: "Sheets from ... to ..." for printing a large booklet in signatures (Acrobat has it; not done), page
  labels in range text, "Document and stamps", the driver's own Properties dialog, Save ink/toner.
- Save as PDF: annotations without an appearance stream that MuPDF cannot synthesise are left out; the whole
  private copy is baked, so a very large document pays for pages it does not use. "Vector" means the layout is: images
  in the source stay images.
- A Resources dict shared by many pages still brings every page's fonts and images when grafted (the same
  `marks::display_form` is used by stamps and watermarks), so a file made from two pages of a shared-resources
  document can be larger than two pages' worth.
- Poster labels are Helvetica (WinAnsi), ASCII only; creep is applied to booklets only, not to Multiple.
