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
- **Free angle.** The round knob above the frame turns the picture about its centre (hold Shift for steps of 15°),
  and "Angle" in the rail takes degrees clockwise as seen, typed as the picture's angle (a field commit is one undo
  step, and so is a knob drag). The picture keeps its shape: it is turned by a `cm` matrix about the centre of its
  box, never sheared. The `cm` is written to eight decimals, so a typed angle is stored as typed (30 reads back as
  30.0000, not 30.0007) whatever the picture's proportions. A turned picture shows its own outline next to the frame,
  hides the side handles and is only scaled evenly (the engine refuses an uneven resize of it).
- **Size of a turned picture.** The rail's Size is the picture's own width and height (the sides of its outline, the
  `quad`), which a turn does not change: 150 × 100 stays 150 × 100 at 30°, while its frame, the box that holds it, is
  179.9 × 161.6. Typing a size on a turned picture scales it evenly (the other side follows, whatever "Keep
  proportions" says), keeps its angle, and grows or shrinks its frame by the same factor from the frame's top left
  corner, so the picture's centre moves; the picture's own corner does not stay put. A quarter-turned picture
  takes the typed width for its own width, which is its frame's height.
- **Several pictures.** Shift (or Cmd/Ctrl) and click adds a picture to the picks or takes it out; picks are of one
  page at a time. A marquee picks the pictures it **touches**: any overlap counts, including a box that contains a
  picture or sits inside one (Acrobat's rule). A turned picture is tested by its own outline, not by its bounding
  box. Shift (Cmd) and a drag on the page starts a marquee, adding to the picks; a plain drag on empty page does it
  when a picture is already selected (otherwise a plain drag belongs to the form, annotation or text layers below).
  A click on an empty page with no drag clears the picks. The frames of the picks are drawn, the rail says "N images
  picked", and a drag of any pick moves all of them as one block that stays on the page (the forms' rule). Move,
  delete, turn (by quarter turns, by an angle, each about its own centre), flip and the six aligns (left, centre,
  right, top, middle, bottom edges of the group) are one undo step each ("Move 3 images on p. 1"). At most 100
  pictures can be picked (the engine's batch limit): a marquee that touches more keeps the first 100 and the rail
  says so, and a Shift-click on a 101st is refused with the same note.
- **Copy and paste.** Cmd/Ctrl+C or the rail's Copy puts the picked pictures on the app's picture clipboard; Cmd/Ctrl+V
  or "Paste on page N" puts them on the page in view, in the same document or in another open one. A paste lands
  where the copy was, and 16 pt down and right of it for each paste after the first on that page (and from the first
  one on the page it was copied from), so it is never hidden under its original; it is kept on the page. The pasted
  pictures become the picks. One undo step ("Paste 2 images on p. 3"). Only image XObjects are copied: an inline
  picture can be moved to another page but not copied (the rail says so).
- **Drag to another page.** Dragging picks over another page of the same document and letting go moves them there
  (the target page is outlined, drawn over its canvases, while the pointer is over it; the frames follow the pointer
  across the gap and over that page as a preview in the body, since a page clips what it holds): "Move image to p. 3", one undo step. The group lands
  with the same grip on it as at pick-up, so it ends up where it was dropped, whatever the other page's size or
  `/Rotate` (the picture stays upright as seen). A drop partly outside the page is moved back inside it.
- **Pictures in forms.** A picture a page draws through a form XObject (Word, InDesign and scanner output, stamps and
  overlays) is listed like any other, up to four forms deep, with its place on the page, whatever the forms'
  `/Matrix` and the picture's own `cm`; the rail adds "Inside form Fm0 › Fm1". Move, resize, turn, flip, crop,
  replace, delete, copy and drag to another page work as for a page's own pictures. A form is usually shared (the same
  form drawn again on this page, on other pages, in an annotation's appearance, in a pattern): when anything but this
  one drawing can reach it, the form is **copied first** and only this drawing is pointed at the copy, so no other page
  changes. A picture moved out of its form's `/BBox` is still moved; the rail says it is cut off or hidden. One undo
  step as always ("Move image on p. 1"; the copy is part of it).
- **Locked.** A certified document refuses every one of these (certification check as for any image change); the
  rail's controls are disabled with the reason, and a paste or a move is refused by the engine too. A document whose
  owner withholds copying refuses Copy. Copy and Paste dim to 45% with the not-allowed cursor, like the other controls. Pasting into or changing a signed document keeps the signed-file path: the
  change is saved after the signed bytes and the rail says so.
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
| `image_edit_many` | `doc, rev, page, items: { index, change }[]` | `{ state: EditState }` |
| `image_copy` | `doc, rev, page, indices[]` | `ClipInfo { count, rect, rects[] }` |
| `image_clip_info` | | `ClipInfo \| null` |
| `image_paste` | `doc, rev, page, at: [x, y]` | `{ state: EditState }` |
| `image_move_to_page` | `doc, rev, page, indices[], toPage, at: [x, y]` | `{ state: EditState }` |

- Coordinates are page space as the viewer shows it (points, origin top left, `/Rotate` applied), as in `text_blocks`.
- `ImageBlock = { index, kind: "xObject" | "inline", rect, fullRect, matrix, width, height, colorSpace,
  bitsPerComponent, filter[], masked, name?, xref?, editable, reason? }`. `rect` is what shows (cut by a rectangular
  clip), `fullRect` the whole image; `matrix` is the placement (unit square → user space, the CTM at the `Do`);
  `width`/`height` in pixels; `filter` outermost first; `masked` = has `/SMask` or `/Mask`. `angle` is the turn of
  the picture's bottom edge as seen, degrees clockwise in (-180, 180]; `quad` its four corners in page space (bottom
  left, bottom right, top right, top left of the picture itself), whose box `fullRect` is. `inForm` lists the resource
  names of the forms it is drawn through, outermost first (empty on the page itself); `name` and `xref` are then its
  entry in the innermost form's resources, and `matrix` and the boxes already include the forms' matrices and
  `/BBox`es. `ImageBlocks` also carries `locked` (a certified document: why), `signed` and `copyLocked` (the owner's
  permissions withhold copying).
- `change` (tagged by `op`):

| `op` | Fields | Effect |
|---|---|---|
| `move` | `dx, dy` | translate |
| `resize` | `rect, keepAspect` | fit the image's box to `rect`; with `keepAspect` as large as fits from its top left |
| `rotate` | `quarterTurns` | ±1, 2, 3 quarter turns clockwise (as seen) about the centre |
| `angle` | `degrees` | turn clockwise (as seen) by any angle about the centre; 0 and 360 are refused; the picture keeps its shape |
| `flip` | `horizontal` | mirror left-right (`true`) or upside down, about the centre |
| `delete` | | remove it |
| `replace` | `file` | draw that image instead, fitted into the old box keeping its own proportions |
| `crop` | `rect, pixels` | show only `rect`: a clip, or with `pixels` a new image of just those pixels |
| `add` | `file, rect, keepAspect` (default true) | new image in `rect` (no `index`), fitted and centred with `keepAspect` |

- Each of `image_edit`, `image_edit_many`, `image_paste` and `image_move_to_page` returns `{ state, warning? }`;
  `warning` is a sentence when a picture was moved so that less of it shows inside the `/BBox` of its form.
- `image_edit_many` applies one `move`, `rotate`, `angle`, `flip` or `delete` to each of up to 100 images of one page
  as a single step: all deletes or none, no image twice; a batch that fails changes nothing. Deletes run from the
  last index back, since deleting renumbers.
- `image_copy` reads, it does not change the document (`rev` must be current). It checks the owner's permission,
  refuses inline pictures and pictures drawn inside text, applies the megapixel and sample-byte bounds of crops to the
  declared size (the soft mask's and stencil mask's samples count too), takes at most 100 pictures, and puts the
  image objects into a small PDF of their own. Each image is rebuilt there, not grafted: a dictionary keeps only
  the keys an image, its masks, colour space (ICC profile, Indexed lookup, tint functions) and filter parameters
  use, so a stray key that points elsewhere in the source file (`/Parent`, a catalog reference) is not followed and
  nothing else of the document leaks into the clip; streams are copied as raw bytes with their own filters. The
  result has no `/OC`, `/StructParent`, `/Alternates` or `/OPI`. The clipboard is that file's bytes plus where
  each picture sat; no document is held open. One copy has one set of caps (bytes read, objects visited), and a
  stream several pictures share (a soft mask, a profile) is read and copied once and shared in the clip.
- `image_paste` grafts each picture from the clipboard's file into the target document (one graft map per paste), names it in the page's own `/XObject` dict and draws it after the content in a
  clean graphics state (the same `q … Q` balancing as Add), with the picture's rectangular clip. The objects are the
  document's own: no object is shared with the source document or with another paste.
- `image_move_to_page` moves in the same document. An XObject picture keeps its object (the target page names it, the
  source page forgets the name when it no longer draws it); an inline picture's operator is written byte for byte
  into the target page. The placement is the picture's page-space placement, shifted and taken to the target page's
  user space, so a rotated or differently sized page lands it where it was dropped.
- `file` is an open token from `pick_source` (kind `images`); the command swaps it for the path (paths.rs).
- `index` belongs to the `rev` of the list; an older `rev` is `Stale`.
- Each change is one edit-core step (`Engine::apply_removing`): undoable, labelled "Move image on p. N" etc., kept by
  Save, recorded in history. A document with such edits is saved as a full, garbage-collected rewrite.

## Design

1. **Reading.** The page's content streams are parsed joined (one stream in pieces). The interpreter tracks the CTM
   (`q/Q/cm`), clips made of straight edges (`re … re W n`, or `m l … h W n`: the box of the path, intersected with the clip in effect, in user space; a path with curves is not tracked) and text objects, and records every `Do` of
   an image XObject and every inline image (`BI … EI`, its abbreviated keys expanded). A `Do` of a form XObject is
   entered where it stands, so the pictures come out in drawing order: the form's stream is parsed as a scope of its
   own, its CTM starts from the form's `/Matrix` and the CTM at the `Do`, and its `/BBox` (in user space) joins the
   clip. A form drawn twice is two scopes, each with its own place. Reading stops at 4 forms deep, at 64 form
   drawings and at 16 MB of form streams per page, and a form that is already being read (it draws itself, directly
   or through others) is not entered again. A form without `/Resources` of its own (PDF 1.1) is read with its
   parent's, and its pictures are shown but not changeable.
2. **Transforms** (move, resize, rotate, flip) are page-space matrices `T`, taken to user space
   (`T_user = P·T·P⁻¹` with `P` user → page) and applied after the placement. The image moves as a unit with
   the wrappers around it: each enclosing `q … Q` that holds only `cm`s and a rectangular clip (a clip crop, an
   earlier transform, the producer's own placement). A new `cm <C·T_user·C⁻¹>` goes right after the outermost
   wrapper's `q` (`C` the CTM there), so a crop moves with its image; a bare operator becomes
   `q <D·T_user·D⁻¹> cm <op> Q` (`D` the CTM at the operator). The image's operator is always written back
   byte for byte from its source, so an inline image keeps its parameters and data. Inline images work the same way.
   For a picture in a form the same edit is made in the form's own stream (`unit_of` and the splice work on
   that scope's operators, the CTM before each of which is the full one, so `C · T · C⁻¹` is unchanged), and the
   form is written back as its stream object (Flate, same object number).
3. **Copy before write.** Before a form is written, the file is asked who else can reach it: every object
   reachable from the trailer is walked once and the references to the forms on the way, and to the resource
   dictionaries that name them, are counted (a page's resources inherited from `/Pages` count as shared). A form
   is the picture's alone when exactly one slot names it, the parent's content has exactly one `Do` of that name,
   and the dictionaries on the way are named once. The first form on the way down that is not alone is copied,
   with every form inside it: the copy has the decoded stream written again as Flate, its own `/Resources` and
   `/XObject` dictionaries, and is added to the parent's (itself private) resources as `IwForm<n>`; the one `Do`
   in the parent is rewritten to that name and the old name is dropped from the parent's resources if nothing
   there draws it. Anything the walk can't settle (a file of over two million objects) counts as shared: a copy
   costs a few bytes, a change to another page is a wrong document. In `image_edit_many` each item reads the page
   again, so the first item of a shared form copies it and the others edit the copy in place.
4. **Delete** removes the operator. **Replace** loads the file with MuPDF and adds it with `pdf_add_image` (JPEG data
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
5. **Add** appends `q <placement> cm /IwImg<n> Do Q` as a new content stream. The page's content is always
   wrapped first, so whatever state it leaves (a `cm`, a clip, a colour, an unclosed `q`) can't reach the new
   image: one `q` before it, plus one for each `Q` it has without a `q` (underflow), and after it as many `Q` as
   that leaves open (two new pieces; the old streams stay as they are). Image files over 256 MB or 100
   megapixels are refused before their pixels are decoded.
6. **No orphans.** After a delete, replace or pixel crop, a name the page's (or, for a picture in a form, the form's)
   content no longer draws is removed from that `/XObject` dict. The page's resources and `/XObject` dict are copied onto the page first (shallowly)
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
- Several pictures: moved as one undo step, deleted in any order as one step, a batch that makes no sense changes
  nothing; a group turn leaves each picture's centre in place; a certified document refuses every change and a signed
  one takes it after its signed bytes.
- Free angle: 30° saved and reopened with the right bounding box and corners, turned again by -30° back upright; 0, 360
  and NaN refused; an uneven resize of a turned picture refused; an inline picture turns too.
- Copy and paste: a copy with an `/SMask` pasted in the same document (its own object, own mask, original untouched,
  saved and reopened) and into another document that is then closed (the picture and its mask stay); a declared size
  past the bounds (a mask's samples included), an inline picture, a stale list and a document that withholds copying
  are refused; a copy keeps the colour space (ICC, Indexed lookup), `/Decode` and `/Mask` and drops a stray key that
  points into the source file.
- Move to another page: lands where dropped on a page with `/Rotate 90`, on a smaller page it is kept inside, as one
  undo step and through save and reopen; a group with an inline picture moves together; a colour space of the page
  (or an Indexed one over it) can't go with an inline picture; a certified document refuses paste and move, a signed one keeps its signed bytes.
- Pictures in forms: a picture in a form used once is listed and moved in the form's own stream; the same form on a
  second page is copied for this page and the other page's content and form stay byte-identical (one undo step);
  nested forms with 30° `/Matrix` entries list the box that renders, within 0.5 pt; delete, replace and crop (pixels and clip)
  in a shared form; a form that draws itself and a chain deeper than 4 end the reading; a form drawn twice on a page is two
  pictures and one moves alone; resources inherited from `/Pages`, a `/Resources` dictionary two pages share and a
  form that is an annotation's appearance all make a form shared; a batch in one shared form makes one copy; a move
  out of the form's `/BBox` warns; a form picture goes to another page and is copied with its full place (mixed with
  page pictures too); a form without `/Resources` is listed and not changed; certified refuses, signed saves after
  its signed bytes; the 64 form-drawing cap holds.

## Limits

- Images in annotation appearance streams, Type3 glyph procedures, patterns and soft-mask groups are not listed; an
  image drawn inside a text object can only be deleted.
- Forms: only the first 64 form drawings of a page, at most 4 forms deep and 16 MB of form streams are read; the
  rest is not listed. A form without `/Resources` of its own is listed and not changeable. A form's `/BBox` only
  produces a warning when a move takes a picture out of it (the picture is still moved), and a form turned by its
  `/Matrix` is approximated by the user-space box of its `/BBox`.
- If the file has more than two million objects the sharing check gives up and the form is always copied.
- OCR clean-up still looks at the page's own content only.
- Only straight-edged clips are tracked for `rect`, by their box; curved clip paths are ignored (the box may show more
  than is visible).
- A pixel crop of a turned or sheared image keeps the pixels of the crop's box in the image's own grid (a pixel crop is
  always a rectangle of pixels); use a clip crop for an exact page-space rectangle.
- A pixel crop of a JPEG stores the cropped pixels losslessly (Flate), so it can be larger than the JPEG; JPEG 2000
  and inline images can only be cropped with a clip. A cropped image keeps its `/Decode`, `/Intent` and other keys;
  a colour-key `/Mask` array stays as it is.
- Replace fits the new image into the old box rather than keeping its pixel size.
- A turned picture is only scaled evenly; a clip crop of one is the parallelogram of the requested rectangle.
- A name is kept in the page's resources if a form XObject without its own `/Resources` could draw it (PDF 1.1).
- Copy before write drops the old form's name from the parent's resources only when nothing there draws it; the
  old form object stays for its other users. A form copy is stored Flate compressed, whatever the original filter was.
- Delete and Replace leave the page's tagged structure as it was: a `/Figure` structure element (and its `/Alt`
  text) that pointed at the old image now points at nothing, or describes the wrong picture. Fix it in the
  accessibility checker after replacing or deleting tagged images.
- The decompression guard is complete for unfiltered and plain Flate images; other filters (DCT, LZW, Flate with a
  predictor, CCITT …) are decoded by MuPDF after the declared-size check, and the result is refused if it is larger
  than declared.
- New images are added through MuPDF's image loader: JPEG, PNG, GIF, BMP, TIFF (first page), JPEG 2000.
- UI: the chooser offers only what `pick_source` allows (jpg, jpeg, png, tif, tiff, bmp, gif). Resizing a
  clip-cropped picture scales the whole box, the visible part with it.
- Several pictures: picks are of one page; a marquee from the empty page needs Shift (Cmd) unless a picture is
  already selected, since a plain drag there belongs to the layers below in Edit mode. A group has no resize
  handles, and a crop, a replace or the angle field work on one picture. A group turn rotates each picture about its
  own centre, not the group's.
- Copy and paste: inline pictures can't be copied (only moved), and a picture drawn inside a text object can only be
  deleted. A picture in a form is copied with its full place onto a page's own content. Another window of the app has
  its own picks but shares the clipboard. A picture is not copied if the owner's permissions withhold copying
  (`/P` bit 5), as with extracting pages.
- Drag to another page: the target must be in view (its layer is mounted); only a drop within the same document, a
  paste is how a picture reaches another document. An inline picture whose colour space is defined in its page's
  resources (`/CS /Cs1`) is refused, since the other page does not have that name.
- A move keeps the picture's object as it is. A copy drops the object's `/OC` (optional-content layer),
  `/StructParent`, `/Alternates` and `/OPI`, so a pasted picture is on no layer and has no tagged-structure entry.
- "Locked page" has no meaning in Inkwell beyond certification (a document-wide lock); there is no per-page lock.
