# Edit images in place

Acrobat's "Edit text & images" for images (study: [study/02-edit.md](study/02-edit.md)). Code:
`src-tauri/src/imageedit/` (engine), `src/lib/imageEdit.ts` and `src/components/edit/Image*.tsx` (UI), adapted from
pdfcraft `crates/edit/src/images.rs` and `crates/content` (b8ddf71, MIT OR Apache-2.0; see `third_party/pdfcraft/`).
It shares the content-stream parser, splice and stream writer of [text editing](text-edit.md). MuPDF stays the only
engine that owns the document.

## In the app

Edit mode (Ctrl+E) lists the pictures of every page near the viewport as outlines, next to the text outlines
(a text block wins a click where they overlap; selecting one lets go of the other).

- **Select.** Click a picture (or its row under "Images on page N" in the rail). The frame has eight handles; the rail
  shows "Format image": pixels, codec, colour space and effective ppi. A picture the engine can't edit (reason from
  `ImageBlock.reason`) is outlined dashed and says why when clicked.
- **Move / resize.** Drag the frame, or a handle: corners keep the proportions (hold Shift to stretch), sides move
  one edge, the opposite side stays put, no side goes under 8 pt. Arrow keys nudge by 1 pt (10 with Shift), turned
  along the screen when the view is rotated. The rail has typed width and height in points ("Keep proportions"; the top
  left stays).
- **Turn / flip.** Quarter turns left and right, flip left-right and top-bottom, about the centre.
- **Crop.** "Crop" dims what is outside the rectangle you drag; Enter or "Apply crop" sends it, Esc leaves.
  By default it is a clip (the picture stays whole in the file and the cut-off part is marked on the frame
  afterwards); "Delete the cut-off pixels" makes a smaller image instead.
- **Replace.** "Replace…" opens the picture chooser (`pick_source`, images); the new picture is fitted into the old
  box. Cancelling the chooser sends nothing.
- **Delete.** The Delete key or the rail's button.
- **Add.** "Image" in Add content chooses a file first, then a click puts it on the page (a 180 pt box at the point,
  kept inside the page) or a drag draws its box; the picture is fitted and centred keeping its proportions and
  selected. Esc or Cancel puts the file back; picking the text tool does too.
- Every change is one undo step ("Move image on p. 1" …), Save writes it, the history records it. A change the page
  moved under (`stale`) is not applied: the selection is found again by position and the rail says so.
- Checked by `scripts/image-edit-shots.mjs` against generated fixtures (`scripts/make-image-edit-sample.mjs`);
  screens in `docs/screens/image-edit/`. Unit tests: `src/lib/imageEdit.test.ts`.

## Commands

| Command | Arguments | Returns |
|---|---|---|
| `image_blocks` | `doc, page` | `{ rev, images: ImageBlock[] }` |
| `image_edit` | `doc, rev, page, index?, change` | `{ state: EditState }` |

- Coordinates are page space as the viewer shows it (points, origin top left, `/Rotate` applied), as in `text_blocks`.
- `ImageBlock = { index, kind: "xObject" | "inline", rect, fullRect, matrix, width, height, colorSpace,
  bitsPerComponent, filter[], masked, name?, xref?, editable, reason? }`. `rect` is what shows (cut by a rectangular
  clip), `fullRect` the whole image; `matrix` is the placement (unit square → user space, the CTM at the `Do`);
  `width`/`height` in pixels; `filter` outermost first; `masked` = has `/SMask` or `/Mask`.
- `change` (tagged by `op`):

| `op` | Fields | Effect |
|---|---|---|
| `move` | `dx, dy` | translate |
| `resize` | `rect, keepAspect` | fit the image's box to `rect`; with `keepAspect` as large as fits from its top left |
| `rotate` | `quarterTurns` | ±1, 2, 3 quarter turns clockwise (as seen) about the centre |
| `flip` | `horizontal` | mirror left-right (`true`) or upside down, about the centre |
| `delete` | | remove it |
| `replace` | `file` | draw that image instead, fitted into the old box keeping its own proportions |
| `crop` | `rect, pixels` | show only `rect`: a clip, or with `pixels` a new image of just those pixels |
| `add` | `file, rect, keepAspect` (default true) | new image in `rect` (no `index`), fitted and centred with `keepAspect` |

- `file` is an open token from `pick_source` (kind `images`); the command swaps it for the path (paths.rs).
- `index` belongs to the `rev` of the list; an older `rev` is `Stale`.
- Each change is one edit-core step (`Engine::apply_removing`): undoable, labelled "Move image on p. N" etc., kept by
  Save, recorded in history. A document with such edits is saved as a full, garbage-collected rewrite.

## Design

1. **Reading.** The page's content streams are parsed joined (one stream in pieces). The interpreter tracks the CTM
   (`q/Q/cm`), clips made of straight edges (`re … re W n`, or `m l … h W n`: the box of the path, intersected with the clip in effect, in user space; a path with curves is not tracked) and text objects, and records every `Do` of
   an image XObject and every inline image (`BI … EI`, its abbreviated keys expanded). Form XObjects are not
   entered: images inside them are not listed.
2. **Transforms** (move, resize, rotate, flip) are page-space matrices `T`, taken to user space
   (`T_user = P·T·P⁻¹` with `P` user → page) and applied after the placement. The image moves as a unit with
   the wrappers around it: each enclosing `q … Q` that holds only `cm`s and a rectangular clip (a clip crop, an
   earlier transform, the producer's own placement). A new `cm <C·T_user·C⁻¹>` goes right after the outermost
   wrapper's `q` (`C` the CTM there), so a crop moves with its image; a bare operator becomes
   `q <D·T_user·D⁻¹> cm <op> Q` (`D` the CTM at the operator). The image's operator is always written back
   byte for byte from its source, so an inline image keeps its parameters and data. Inline images work the same way.
3. **Delete** removes the operator. **Replace** loads the file with MuPDF and adds it with `pdf_add_image` (JPEG data
   is stored as it is, `DCTDecode`; a PNG's alpha becomes an `/SMask`), names it `IwImg<n>` and draws it with
   `q <fit> cm /IwImg<n> Do Q` in place of the old operator. **Crop** with a clip draws
   `q u0 v0 du dv re W n <op> Q` (the rectangle in the image's unit square, the space its `Do` paints in); for an
   image turned or sheared by other than quarter turns the page-space rectangle is a parallelogram there, drawn
   as `m l l l h W n`, so exactly the requested rectangle shows. A pixel
   crop decodes the samples (`pdf_load_stream`: Flate, DCT, LZW, CCITT …; any bit depth 1-16), copies the rows and
   columns inside the crop (rounded out to whole pixels) into a new Flate image with the same colour space, decode
   and mask settings (an `/SMask` or stencil `/Mask` is cropped the same way, one level deep: a mask's own masks
   are dropped, so a mask that refers back to its image can't recurse), and draws it where those pixels were.
   Before decoding, the declared size must stay under 100 megapixels and 512 MB of samples (sizes computed with checked arithmetic,
   colour spaces with 1-32 components); plain Flate data is inflated here with that size as the limit.
4. **Add** appends `q <placement> cm /IwImg<n> Do Q` as a new content stream. The page's content is always
   wrapped first, so whatever state it leaves (a `cm`, a clip, a colour, an unclosed `q`) can't reach the new
   image: one `q` before it, plus one for each `Q` it has without a `q` (underflow), and after it as many `Q` as
   that leaves open (two new pieces; the old streams stay as they are). Image files over 256 MB or 100
   megapixels are refused before their pixels are decoded.
5. **No orphans.** After a delete, replace or pixel crop, a name the page's content no longer draws is removed from
   the page's `/XObject` dict. The page's resources and `/XObject` dict are copied onto the page first (shallowly)
   when they are inherited or shared, so other pages keep theirs. The next save is a full rewrite without
   unreferenced objects, so the old image (and its soft mask) is gone unless something else still uses it.

## What works (tests: `cargo test --lib imageedit -- --test-threads=1`)

- Listing XObject and inline images with box, matrix, pixel size, colour space, bits and filters.
- Move (XObject and inline), round trip through save and reopen, undo.
- Resize with and without kept proportions; rotate by quarter turns and flip, checked by where the image's top left
  corner lands.
- Delete: the saved file holds no image object and the page no longer names it; undo brings it back. An image that
  another page also draws stays in the file.
- Replace with a JPEG: the stored stream is byte-identical to the file. Replace with a PNG with alpha: an `/SMask`;
  deleting it afterwards leaves neither image nor mask.
- Crop with a clip (XObject and inline) and with pixels (exact samples checked; a JPEG is decoded and stored as
  Flate); the uncropped image is gone from the saved file.
- Add an image fitted into a rectangle, upright, unaffected by a `cm`, a clip or a stray `Q` the page's content
  leaves behind.
- Every operation on an inline image keeps its bytes (`BI … ID … EI`) and the rest of the page, checked in the saved
  content after reopening.
- A clip-cropped image moves, and turns, together with its crop (nested crops too).
- A soft mask that refers to its own image is cropped once, without recursion; images with absurd sizes or
  component counts are refused with a message.
- Stale `rev`, a missing index and an index out of range are refused and change nothing.

## Limits

- Images inside form XObjects (and annotation appearances) are not listed; an image drawn inside a text object can
  only be deleted.
- Only straight-edged clips are tracked for `rect`, by their box; curved clip paths are ignored (the box may show more
  than is visible).
- A pixel crop of a turned or sheared image keeps the pixels of the crop's box in the image's own grid (a pixel crop is
  always a rectangle of pixels); use a clip crop for an exact page-space rectangle.
- A pixel crop of a JPEG stores the cropped pixels losslessly (Flate), so it can be larger than the JPEG; JPEG 2000
  and inline images can only be cropped with a clip. A cropped image keeps its `/Decode`, `/Intent` and other keys;
  a colour-key `/Mask` array stays as it is.
- Rotation is in quarter turns; replace fits the new image into the old box rather than keeping its pixel size.
- A name is kept in the page's resources if a form XObject without its own `/Resources` could draw it (PDF 1.1).
- Delete and Replace leave the page's tagged structure as it was: a `/Figure` structure element (and its `/Alt`
  text) that pointed at the old image now points at nothing, or describes the wrong picture. Fix it in the
  accessibility checker after replacing or deleting tagged images.
- The decompression guard is complete for unfiltered and plain Flate images; other filters (DCT, LZW, Flate with a
  predictor, CCITT …) are decoded by MuPDF after the declared-size check, and the result is refused if it is larger
  than declared.
- New images are added through MuPDF's image loader: JPEG, PNG, GIF, BMP, TIFF (first page), JPEG 2000.
- UI: one picture at a time (no multi-select, no copy and paste, no dragging a picture to another page), no turning
  by a free angle, and the chooser offers only what `pick_source` allows (jpg, jpeg, png, tif, tiff, bmp, gif).
  Resizing a clip-cropped picture scales the whole box, the visible part with it.
