# Print production

Engine: `src-tauri/src/prepress/`. It does what Acrobat Pro's "Print production" tools do: Output Preview
(separations, overprint preview), ink coverage and total area coverage (TAC), the object inspector, Convert
Colors, and printer's marks with trim and bleed boxes. pdfcraft has none of it. The UI is described under
"The UI" below; the commands and the tile route are what it calls. The PDF/X checks stay in `src/preflight/`.

## How a page is painted

`render::paint_planes` runs the page's display list (the one the viewer uses, `Engine::page_list`) into a
DeviceCMYK pixmap that carries one extra plane per spot colour (MuPDF `fz_separations`, every spot set to
"spot"). MuPDF simulates overprint while it paints into such a pixmap: a Separation or DeviceN colour goes to
its own plane, an object with overprint on leaves the planes it does not paint alone (overprint mode 1 also
leaves the zero components of a CMYK colour), an object without it knocks the planes out. Every view is composed
from those planes:

- **Plates** (`Mode::Plates`): paper white, each plane that is switched on darkens it. Process planes the way
  MuPDF shows DeviceCMYK, spots towards their own colour (the alternate of the Separation at full tint).
- **Coverage** (`Mode::Coverage`): the sum of the planes that are on, as a heat map: paper white at no ink,
  pale blue through green to orange as coverage nears the limit, red above it (default 300%).

`View { mode, process: [C, M, Y, K], hiddenSpots, overprint, tacLimit }`. With `overprint: false` the page is
painted knocked out, without separations: the spot colours are folded into CMYK through their alternates, so
only the four process plates can be switched. MuPDF applies overprint only in a separations pixmap and the
simulation cannot be switched off there.

Spot colours are the page's Separation and DeviceN colorants (`page_separations`, read from the authoritative
document, MuPDF's `fz_page_separations`). The process names and `All` / `None` are not spots. `All` (the
registration colour) paints every plane. A pixmap holds 32 components and a transparency group's alpha takes one,
so at most `MAX_SPOT_PLANES` = 27 spots get a plane; the others (`folded` in the listing) are painted into CMYK
through their alternates and cannot be switched on their own.

## Commands

| Command | Does |
|---|---|
| `prepress_separations(doc, page)` | `{ process, spots: [{ name, rgb, cmyk }], overprint }`: the inks of a page; `overprint` says the page paints with it |
| `prepress_view(doc, view) -> id` | registers a view; the same view of a document gets the same id |
| `prepress_ink(job, doc, pages?, limit?, trimOnly?, onProgress)` | one `InkReport` per page |
| `prepress_inspect(doc, page, x, y)` | what is painted at a point, topmost first |
| `prepress_convert(job, doc, spec, profile?, saveTo?, onProgress)` | Convert Colors, one undo step. With `saveTo` (a save token) the converted document is also written there as Save as, in the same command; the document's own file (by file identity: a link or another case of the name is the same file) is refused before converting. `Converted { state, report, saved, saveError }`: a failed write leaves the conversion in the open document and says so |
| `prepress_save_other(doc, to)` | Save as after a conversion whose write failed: the same own-file check, no second conversion |
| `prepress_protection(doc)` | `{ certified, signed }` for the sheets |
| `prepress_boxes(doc, spec)` | sets TrimBox, BleedBox, ArtBox or CropBox, one undo step |
| `prepress_marks(doc, spec)` | adds printer's marks, one undo step |
| `prepress_page_boxes(doc, page)` | the page's media, crop, trim, bleed and art boxes, `/Rotate`, and which of them the page states itself |
| `prepress_marks_preview(doc, page, spec)` | the marks `prepress_marks` would add to one page (drawing operators, new media, trim and bleed box, notes) without touching the document |
| `prepress_ink_at(doc, page, x, y)` | the ink per plate and the total at one point (page points from the top-left as shown), painted with overprint |
| `prepress_pick_profile()` | opens a file dialog for a CMYK ICC profile and returns an open token for `prepress_convert` |

Tiles use the existing `inkwell://` protocol with a second route, `p`:
`/p/{doc}/{page}/{rot}/{scale}/{tx}/{ty}?v={view}&g={rev}&e={epoch}&r={rank}`. It is `/t/...` plus the view id
from `prepress_view`; tiles are the same 512-pixel RGBA tiles with `x-tile-w` / `x-tile-h`, 410 when the
generation `g` is stale, 500 with a message on failure. `s` (stream) and `e` (epoch) work as on `/t/`: a tile whose
every requester has moved on is answered 410 without being painted, and requests for one tile join one render.
They are scheduled on the engine's pool with the page tiles (`Engine::spawn`) and cached per view (48 MB,
`Prepress::tiles`). A document keeps at most 64 views (the one registered or used longest ago goes); `close_document` forgets them all.

## Ink coverage and TAC

`ink_report(doc, page, limit, maxPx, trimOnly)` paints the page with its longer side `maxPx` pixels (1600 by
default, at most twice natural size) and reports, over the planes that are inked:

- `maxTac` per cent (four full inks read 400), `maxAt` in page points from the top-left, `overLimit`,
  `overLimitArea` (share of the measured area) and up to 64 `regions` `[x0, y0, x1, y1]` around the areas above
  the limit (a 32 x 32 grid, runs merged);
- `regionsTruncated` when more than 64 regions were found;
- per plane `max` and `mean`, spots after CMYK. Spots count in the total. The limit must be 0 to 1000.

A hairline thinner than a pixel is averaged with its paper; raise `maxPx` to look closer. With `trimOnly`
(the default in the command) the measure stops at the TrimBox when the page has one, so printer's marks in the
slug (registration colour, 400%) do not count.

## Object inspector

`inspect_at(doc, page, x, y, limit)`: `x`, `y` are page points from the top-left of the page as shown
(`/Rotate` applied). The display list runs through a recording device; an object counts when it covers the
point, which is decided by painting it into a 3 x 3 pixel probe (so curves, strokes and glyphs use MuPDF's own
geometry). Each hit has the kind (fill, stroke, text, stroked text, image, image mask, shading), the colour
space (`DeviceRGB`, `ICCBased (...)`, `Separation / DeviceN`, ...), its model, the colorants of a spot space,
the values as written, the CMYK equivalent and its ink in per cent, `overprint` and `overprintMode`, alpha,
bounds, the font of a text run and the size of an image. Clips are honoured when the clip is a path; a text,
stroke or image-mask clip counts as inside. A shading is reported (model `shading`) wherever no path clip
excludes it, without its colours. Tiling-pattern cells and soft-mask content are not page content and are skipped.

## Convert colours

`ConvertSpec { pages?, gray, rgb, spot, grayToK, preserveBlack, intent, condition? }`. The page streams are
rewritten by MuPDF's colour filter (`vendor/mupdf-sys/wrapper/prepress.c`, shaped like its `pdf_recolor_page`,
which only goes to device spaces): fills, strokes, text, shadings and images of the picked kinds become
DeviceCMYK. Annotation appearances, forms and patterns are filtered too.

- RGB and gray (CalRGB, Lab, ICC RGB/gray) and spot colours (Separation, DeviceN, through their alternate and
  tint transform). An Indexed space counts as the space its indices decode into (Indexed CMYK stays). CMYK is
  left alone, and so is `Separation /None`. A spot space set without a colour starts at full tint, as in the
  file. Form fields' appearances are converted with the page's annotations.
- `grayToK`: gray becomes K alone (K = 1 - gray). `preserveBlack`: RGB 0 0 0 becomes K = 1, however the profile
  would mix it.
- `intent`: `device` (MuPDF's conversion), `document` (the CMYK profile of the output intent already in the
  file) or `profile` (a CMYK ICC file the command reads from an open token; it also becomes the file's
  `GTS_PDFX` output intent, named `condition`). The values are computed through the profile by MuPDF's lcms2
  and written as DeviceCMYK. A profile that is not a CMYK ICC profile is refused.
- Transparency groups on the page and its forms whose blending space was a converted kind (DeviceRGB, CalRGB, Lab, a
  3-component ICC profile; gray likewise) are set to
  DeviceCMYK. Afterwards the report says which device spaces are still in use (`remaining`: gray, RGB, CMYK),
  asked of the PDF/X checker's survey (`preflight::device_colours`, so Default colour spaces count the same
  way). Images come out uncompressed (written Flate); run Compress to make them small again.
- Left as they are: CMYK colours (ICCBased CMYK included), Default colour spaces, and whatever MuPDF's colour
  filter does not reach (colours inside Type 3 glyph procedures).

## Boxes and printer's marks

`BoxSpec { pages?, kind, rect? | from + margin }`: `kind` is trim, bleed, art or crop; `rect` is
`[x0, y0, x1, y1]` in default user space (y up); crop and art boxes must also lie in the media box; `from` + `margin` `[left, bottom, right, top]` moves another
box outward (negative inward), e.g. bleed = trim with 9. With `shown: true` the margin is of the page as it is
shown (left, bottom, right, top) and each page maps it through its own `/Rotate`; the sheet uses this, so one
request over pages turned differently is right on every page. `/Rotate` is read exactly like MuPDF's
`pdf_to_int` + page loader does (a real rounds half up, a negative value wraps, then it snaps to the nearest quarter
turn: -45 is 0, -135 is 270, 45 is 90). The result must keep the trim box inside the bleed box
and the bleed box inside the media box, or nothing changes. The media box is the paper size and is not set here.

`MarksSpec { pages?, marks: { crop, bleed, registration, colorBars }, weight?, offset?, length? }`. Marks are
laid out around the effective TrimBox (a page without one is trimmed at its edge: the TrimBox is written) and
start beyond the BleedBox: crop marks `offset` from the trim edge (never less than the bleed + 2, default
bleed + 6) and `length` long (18), bleed marks 2 points beyond the bleed corners, registration targets on the
middle of each side, a colour bar (C, M, Y, K, overprints, tints) on the bottom edge when it fits. `offset` is
0 to 144. The media box is enlarged only as far as the marks need (a page that already has a slug is not
touched); a CropBox is enlarged with it. A TrimBox, BleedBox or ArtBox the page leaves to its default (its crop
box) is written with that value first, so they do not follow the media box outward and a second run of marks
finds the same boxes and grows nothing. Marks use a Separation `All` colour space (every plate, spots included), 0.25 pt by default;
the colour bar uses process colours. The page's own contents are wrapped in a balanced `q ... Q` first. Notes
come back for what was skipped (no bleed, a page too small for colour bars).

## Signed and certified documents

Convert Colors, Printer's Marks and Page Boxes refuse a certified document (a `/DocMDP` signature, any
level): the command returns "This document is certified ..." before changing anything, and the sheets turn their
controls off and show the reason. A signed document (signature field with a value, `/SigFlags`) is edited
through the same custom edit, which never asks for a removing rewrite, so its save appends and the signed bytes
stay a prefix of the file; the sheets warn that the file shows as changed after signing. The sheets read this
once per document generation (`rev`), so a signature or certification that appears later in the session turns the
controls off or the warning on.

Convert + Save as another file is one engine step (`Engine::apply_custom_and_save`): the conversion and the
write run under one edit guard, so no other edit or undo can land between them, and the document's own file is
refused under the same guard. If the write fails the conversion stands in the open document and the command
reports `saveError`; the sheet retries the save only.

## The UI

Print production tools are the "Print production" group of the tools list and of the command palette. They
are not assistant tools. State is `src/lib/prepressStore.ts` (logic in `prepressLogic.ts`, wire types in
`prepressApi.ts`, rail wiring in `prepressRail.ts`); the screens are `src/components/prepress/`.

- **Output Preview** is a right-hand rail (one rail at a time, like Preflight). It switches between the plates
  view and the ink coverage view, lists the page's plates (the four inks and the spots) with a switch and
  an "Only" button each, shows the ink per plate and the total under the pointer (`prepress_ink_at`, one
  question in flight, the newest point wins), and takes a total-ink limit (300% by default, any 0 to 1000%).
  The areas above the limit are drawn on the page and the sentence under the field says how high the ink goes.
  "Simulate overprint" off folds the spots into CMYK, so only the four inks can be switched. While the rail is
  open the page's tiles come from the `p` route with the view id of the current settings, the "pages dark"
  inversion is off, and the view is registered again whenever a setting changes. A strip of small pictures of
  the current page's plates (one per plate, drawn when scrolled into view and kept per document generation,
  tile streams 2 and 3) shows one plate alone on click.
- **Convert Colors** is a sheet with options (RGB, gray, spot; gray to black only; keep pure black; standard
  conversion, the file's output intent or a CMYK profile file; pages; result), a confirmation that lists what
  will change in plain sentences, and then runs with progress and Cancel. The result is a new file (asks where
  first; the engine converts the open document and saves it there in the same command, so nothing can land
  between the two, the tab moves to the new file and the original on disk is untouched) or one undo step in the
  open document, which Save writes. If the write fails the conversion stays in the open document and the sheet
  offers only the save again ("Save as...", `prepress_save_other`), never a second conversion.
- **Printer's Marks** is a sheet with a live preview: the page, the marks, the new media box and the trim and
  bleed boxes, drawn from `prepress_marks_preview` (the same geometry `prepress_marks` writes). Which marks,
  line weight, length, distance from the trim, units, and all pages / this page / a range. One undo step.
- **Page Boxes** is a sheet with a live preview of the box that will be written: which box, which box it is
  measured from, margins for each side as the page is shown (turned pages are handled), units, all sides the
  same, a 3 mm bleed preset, and pages. The sheet refuses what the engine would refuse before asking it. The
  engine takes margins from each page's own reference box and maps them by that page's own turn, so a range
  applies the same margins to every page as they are shown. One box kind per apply; one undo step.
- The sheets are not modal: when an undo, redo or an assistant edit changes the document under an open sheet it
  reads the page's boxes and the marks again, and closes if its page is gone. The rail uses the inks of the page
  and generation on screen only, drops a probe answer asked before the pointer left or the document changed,
  and asks for its view again when tiles of it fail (the engine keeps a bounded number of views).

## Not here

- Output Preview works on one page at a time: there is no document-wide list of spot colours, and the object
  inspector has no UI yet.
- Convert Colors to a new file saves the open document there (Save As), because the engine converts in place.
- A certified document is not edited at all; no incremental certified-save path (a change allowed by its
  DocMDP level) is written.
- No separate-plate output (one file per plate), no trapping, no ink-limit conversion (TAC is reported, not
  reduced), no rich-black or overprint fixups.
- Overprint preview needs MuPDF's rendering, which follows PDF 1.7 overprint (OPM, `All`, `None`); it does not
  simulate the press' dot gain or the paper white.
- MuPDF paints DeviceCMYK through the output intent's CMYK profile when the file has one, so a file with an
  output intent is measured and shown as that profile converts its CMYK (a black image converted through a
  profile and then given that intent reads as rich black on screen although its samples are K alone). The
  plates are a preview, not a proof.

## Tests

`cargo test --lib prepress -- --test-threads=1`: generated fixtures only. A spot-colour page lists its spot;
switching a separation changes the pixels it paints and not the others; a black patch with overprint mode 1
leaves the cyan plate under it, knockout does not; a 4 x 100% patch reads 400% TAC (and its position, area and
regions), 4 x 50% reads 200%; the inspector names the space, values and overprint of an RGB fill, a spot fill, a
CMYK fill with overprint and a text run and lists overlapping objects topmost first behind a clip; conversion
leaves no DeviceRGB or DeviceGray (checked in the content, in image and shading colour spaces, in transparency
groups, and by the PDF/X survey), undoes as one step, survives a save, and goes through a CMYK ICC profile
(MuPDF's own `ps_cmyk`, which gives other values than its default) that also becomes the output intent; boxes
are validated; marks fall outside the trim and the bleed box, the media box grows around them, and ink inside
the trim is unchanged (`trimOnly`).

The review round added tests for: an Indexed RGB and an Indexed CMYK image, a form field's appearance, 30 spots
(27 planes, 3 folded), a spot space set without a colour and `Separation /None`, marks run twice, refused
offsets, limits and boxes, and three requests for one tile joining one render.

The UI round added Rust tests for `prepress_page_boxes` (which boxes a page states, `/Rotate` as a number),
`prepress_ink_at` (a spot patch reads on its plane only, overprint keeps both plates, four inks read 400, a
point off the page is clamped) and `prepress_marks_preview` (it changes nothing and matches what the marks
write), and vitest suites for the logic (`prepressLogic.test.ts`: plates, solo, limit, margins for every
rotation, box rules, marks parsing, convert summary and request) and the store (`prepressStore.test.ts`: view
sync, plate toggles and solo, limit, single-flight probe, convert confirmation and the new-file flow, marks and
boxes forms and apply).

The review round added Rust tests for: the three edits refused on a certified document (nothing recorded), the
saved bytes of a signed document starting with the original for all three, margins as shown on an upright and a
turned page in one request (and a real `/Rotate`), the document's own file refused by name, link and case,
and the bounded profile read; vitest for the single convert + save command and the retry of a failed write,
certified and signed documents, resync of a sheet under undo, the probe dropped after leave or rev change,
stale inks, and the view asked for again after failed tiles.

The follow-up round added Rust tests for: `/Rotate` snapping like MuPDF (90, 90.0, 44, 45, -90, -45, -135, 450,
359), a conversion saved as another file as one guarded step (an undo started inside it lands after the
save; the document's own file is refused with nothing converted), and a failed write leaving the conversion in the
open document; vitest for the protection being read again when the document's generation moves.

Known fault, not from this code: `prepress::` run in one process with `--test-threads=1` aborts with SIGTRAP in
about a third of runs, always inside MuPDF's allocator (`fz_calloc` -> libmalloc lock trap) on a test that edits
a two-page document after the colour-conversion tests have run. It moves between tests and reproduces with the
earlier engine code, so it looks like a heap corruption left by a conversion test rather than by the edit it
trips on. Each test passes alone; the margins test is a single edit so it does not depend on the order.
