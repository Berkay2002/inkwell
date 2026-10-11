# Print

Native Windows printing from the Rust core, with a live preview that is the printer's own drawing.
Code: `src-tauri/src/print/` (`layout.rs` pure sheet layout, `render.rs` MuPDF sheet renderer, `win.rs` GDI job,
`app.rs` Tauri commands). UI: `src/components/PrintSheet.tsx`, `src/lib/printStore.ts`. Entry: Ctrl+P and the
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

## Options

| Option | Behaviour |
|---|---|
| Pages | all, current page, range text (`1-3, 5, 8-`), odd or even among the chosen pages, reverse |
| Copies, collate | up to the driver's maximum; copies are sent explicitly (collated or not), so the result does not depend on the driver's own collation |
| Scaling (one per sheet) | fit, shrink oversized pages only, actual size, custom percent |
| Several per sheet | 2, 4, 6, 9, 16 or any grid up to 100; order across/down, optional border, turn pages to fit |
| Booklet | saddle-stitch imposition, both sides / front only / back only, binding left or right; forces short-edge two-sided |
| Orientation | automatic (per sheet, by the page), portrait, landscape |
| Two-sided | off, long edge, short edge (only when the printer reports duplex) |
| Grayscale | drawn as grey in the same pass |
| Comments and forms | document (no markup), document and markups, form fields only. Annotations without the PDF Print flag are not printed. |

## Printers and files

Printers come from `EnumPrinters`; paper sizes, trays, duplex, colour and copies from `DeviceCapabilitiesW`.
Print-to-file printers (Microsoft Print to PDF, the XPS writer) ask where to write through the save dialog (a
path token, `paths.rs`); `INKWELL_PRINT_DIR` writes them into that folder without asking, for automation.
A document that forbids printing (`Permission::PRINT`) is refused with `not-permitted`.

## Tests

- `cargo test --lib print:: -- --test-threads=1`: layout, imposition, sequence, preview, content modes, and the
  round trip: unsaved markup prints, survives save and reopen, and prints again the same.
- Printer tests (`mod printer`, real "Microsoft Print to PDF" to a temp file, then read back with MuPDF to check
  page count and sheet sizes) are opt-in: `INKWELL_PRINT_TESTS=1`. They never touch a physical printer.
- vitest: `src/lib/printStore.test.ts` (settings mapping, paper choice, remembered options, the sheet's
  open/plan/preview/start/cancel flow).
- Runtime: `scripts/print-shots.mjs` drives the sheet through CDP and prints a copy of a generated sample to
  Microsoft Print to PDF into `INKWELL_PRINT_DIR`; screens in `docs/screens/print/`.

## Not done, and known limits

- The margin is a fixed 18 pt, not the printer's hardware margins (the preview shows the same margin, so it still
  matches what the sheet does, but a driver that clips more will clip more).
- Output through GDI is raster at the printer's resolution (capped at `win::MAX_DPI`), not vector.
- Two-sided flags were checked against the driver's capability call and by the plan, not on duplex hardware.
- Not built: Poster (tiling), page labels in range text, "Document and stamps", the driver's own Properties
  dialog, Save ink/toner.
