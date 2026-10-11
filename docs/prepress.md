# Print production

Engine: `src-tauri/src/prepress/`. It does what Acrobat Pro's "Print production" tools do: Output Preview
(separations, overprint preview), ink coverage and total area coverage (TAC), the object inspector, Convert
Colors, and printer's marks with trim and bleed boxes. pdfcraft has none of it. There is no UI yet; the
commands and the tile route below are what the UI will call. The PDF/X checks stay in `src/preflight/`.

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
| `prepress_convert(job, doc, spec, profile?, onProgress)` | Convert Colors, one undo step |
| `prepress_boxes(doc, spec)` | sets TrimBox, BleedBox, ArtBox or CropBox, one undo step |
| `prepress_marks(doc, spec)` | adds printer's marks, one undo step |

Tiles use the existing `inkwell://` protocol with a second route, `p`:
`/p/{doc}/{page}/{rot}/{scale}/{tx}/{ty}?v={view}&g={rev}&e={epoch}&r={rank}`. It is `/t/...` plus the view id
from `prepress_view`; tiles are the same 512-pixel RGBA tiles with `x-tile-w` / `x-tile-h`, 410 when the
generation `g` is stale, 500 with a message on failure. `s` (stream) and `e` (epoch) work as on `/t/`: a tile whose
every requester has moved on is answered 410 without being painted, and requests for one tile join one render.
They are scheduled on the engine's pool with the page tiles (`Engine::spawn`) and cached per view (48 MB,
`Prepress::tiles`). A document keeps at most 64 views (the oldest go); `close_document` forgets them all.

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
- Transparency groups on the page and its forms whose blending space was a converted kind are set to
  DeviceCMYK. Afterwards the report says which device spaces are still in use (`remaining`: gray, RGB, CMYK),
  asked of the PDF/X checker's survey (`preflight::device_colours`, so Default colour spaces count the same
  way). Images come out uncompressed (written Flate); run Compress to make them small again.
- Left as they are: CMYK colours (ICCBased CMYK included), Default colour spaces, and whatever MuPDF's colour
  filter does not reach (colours inside Type 3 glyph procedures).

## Boxes and printer's marks

`BoxSpec { pages?, kind, rect? | from + margin }`: `kind` is trim, bleed, art or crop; `rect` is
`[x0, y0, x1, y1]` in default user space (y up); crop and art boxes must also lie in the media box; `from` + `margin` `[left, bottom, right, top]` moves another
box outward (negative inward), e.g. bleed = trim with 9. The result must keep the trim box inside the bleed box
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

## Not here

- No UI: a plate/coverage panel, the inspector tool, and the dialogs for the three edits are for the UI branch.
- No separate-plate output (one file per plate), no trapping, no ink-limit conversion (TAC is reported, not
  reduced), no rich-black or overprint fixups.
- Overprint preview needs MuPDF's rendering, which follows PDF 1.7 overprint (OPM, `All`, `None`); it does not
  simulate the press' dot gain or the paper white.
- The document's output intent is not used when painting (MuPDF paints DeviceCMYK through its own CMYK
  profile): the CMYK plates are the page's CMYK, not a proof.

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
