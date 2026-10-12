# Measuring tools (engine)

Parity row: Acrobat Pro "Measure" (distance, perimeter, area, with a scale and units). pdfcraft has none, so this is
written from the PDF spec (ISO 32000-1 section 12.9) and from MuPDF's annotation calls; no code is taken from pdfcraft.
Brief: `docs/cloud/measure.md`. The UI is described under "In the app" below.

Code: `src-tauri/src/measure/` (`calc.rs` units, scale, geometry, page transform; `numfmt.rs` number formats; `dict.rs`
`/Measure` and `/VP`; `write.rs` annotations, appearance, listing; `snap.rs`; `mod.rs` the engine calls; `app.rs` the
commands; `tests.rs`). The UI: `src/lib/measure.ts` (store), `src/lib/measureLogic.ts` (pure maths), `src/components/measure/`.

## Spaces and units
- Points the UI and commands pass are in **page space**: points, y down, as the viewer shows the page, rotation applied
  (the space `annot` uses). `PageGeom` (from MuPDF's page transform) maps them to PDF **user space**, where `/L`,
  `/Vertices` and `/Measure` live. Rotation, the crop box origin and `/UserUnit` are all in that one transform.
- `/UserUnit` (user space unit = n points) goes into the `/C` of `/X` (a page with `/UserUnit 2` writes twice the `/C`),
  and reading divides it out, so a value is the same on any page that shows the same paper length. The unit is taken
  from the transform (square root of its determinant), so it is whatever MuPDF used. A page whose transform cannot be
  inverted (`/UserUnit 0`) is skipped by `measure_list` and refused by the other calls.
- Limits: at most 10,000 points per measurement and coordinates up to 1e7 pt (`measure_add`, `measure_value`).
- Units: `pt in mm cm m ft` (`Unit`). A `Scale` is "paper of unit = real of unit", e.g. `1 in = 10 ft`
  (`{"paper":1,"paperUnit":"in","real":10,"realUnit":"ft"}`). A page with no scale measures in 1 in = 1 in.
- Kinds: `distance` (exactly 2 points, straight), `perimeter` (2 or more points, the length of the open path through
  them), `area` (3 or more, shoelace; the polygon closes itself).

## `/Measure` dictionaries (`dict.rs`)
Written as `/Type /Measure /Subtype /RL` with `/R "1 in = 10 ft"`, `/X`, `/D`, `/A` number format arrays and `/O [0 0]`.
Read: any `/RL` measure with a usable `/X`; `/Y`, `/CYX`, `/O` are kept. How it is read (this is the part to check
against a file from Acrobat; I had no Acrobat here, the shape is from memory of its files and the spec):
- `/X` first element's `/C` = real distance (in `/U`) per user space unit; `/Y` (when present) scales the y axis, and
  `/CYX` (default 1) turns its units into the x axis units.
- `/D` and `/A` first element's `/C` multiplies the result again (Acrobat writes 1). No `/D` or `/A`: the `/X` format is
  used ("sq " + unit for an area, from `/X`'s first element only: a later element's `/C` is a linear factor, so a chain
  does not carry over to an area). A later element's `/C` is how many of it make one of the previous (feet, then inches).
- Number formats keep `/U /C /F /D /FD /RT /RD /PS /SS /O`: decimal (D 100 is two decimals), fraction, round, truncate;
  our own are two decimals with trailing zeros, "," thousands, "." decimal. Absent `/PS` and `/SS` read as a single
  space (a trailing one is not shown); `/D` is clamped to 1..1e9. A malformed `/Measure` (not a dictionary, `/C` 0, no
  `/X`, junk elements) is left out of `measure_list`, never an error for the document.
- **Chains** (`/D` or `/A` with several elements, `numfmt::chain_text`): all but the last element show whole units
  (truncated), the last takes the remainder in its own style, and a remainder that rounds up to a whole unit of the one
  before is carried (2.9999 ft with inches in hundredths is "3 ft 0.00 in", never "2 ft 12.00 in"; this runs up the
  whole chain). An array is at most 8 elements (`dict::MAX_CHAIN`); a longer one is not walked.
- **A malformed `/D` or `/A`** (an element that is not a dictionary, a `/C` at or below zero, no or an empty `/U`, more
  than 8 elements, or a value that is not an array such as a reference that loops back) is not used at all, because a chain with a unit missing in the middle would read the wrong unit. The
  value is then written in the `/X` format, as for a measure with no `/D`, the annotation stays listed, and its
  `quantity.note` says which array was left out (the Measure rail shows it). `/X` and `/Y` are lenient as before (a
  bad element is skipped; no usable `/X`, or more than 8, leaves the measure out).
- **`plain`**: whether the UI's own live label (the page scale, two decimals, "," for thousands) reads exactly as the
  engine does. It is true for what this app writes (`/X` of one element, `/D` and `/A` of one element at `/C` 1 in our
  format, no `/Y`, no note) and for a page with at most one such viewport. `MeasureDto.plain` and `PageScale.plain`
  carry it; when false the live label comes from `measure_value`.
- `/R` is shown back as the scale when it parses ("1 in = 10 ft") and agrees with `/C`; otherwise the scale is derived
  from `/C` as "1 in = n unit". A unit we have no name for (km, mi) still gives values, with no `Scale`.
- Page level: a `/VP` viewport array on the page, each `/Viewport` with `/BBox` and `/Measure`. The page's scale
  (`measure_scale_get`) is the last viewport that covers the whole page (its box contains the crop box, so a media-box
  viewport counts), else the widest one; a measurement with no scale of its own uses the last viewport that contains
  its first point (later viewports are on top). `measure_scale_set` replaces the measure of that same viewport,
  or adds one at the *start* of the array so a detail viewport stays on top of it; other viewports stay. `/VP`
  is written as a fresh array (and a changed viewport as a fresh dictionary), so a `/VP` shared with another page is
  never edited in place.

## Annotations (`write.rs`)
`measure_add` makes a `/Line` with `/IT /LineDimension`, a `/PolyLine` with `/PolyLineDimension` or a `/Polygon` with
`/PolygonDimension`: the geometry through MuPDF (so `/L` and `/Vertices` are in user space), then `/Measure` (its own
copy, so the annotation keeps the scale it was made with), `/Subj` ("Distance Measurement"...), `/Contents` = the label
("25.00 ft"), print flag, author, dates. The appearance stream is ours: the shape in red (a polygon with a light wash),
end ticks on a distance (no `/IC`: the wash exists only in the appearance, so a redraw elsewhere gives an outline, not
a solid fill), and the label in Helvetica along the line (never upside down) with a white halo. `/Rect` and the
form `/BBox` are the box of all that. A measurement from another app keeps its own look when a point is moved: `/C`,
`/BS /W` (or `/Border`), `/LE` line endings (the arrows, butt, slash, square, diamond and circle styles at the first
and last point), `/IC` for the area fill, and on a distance `/LL`, `/LLE` and `/LLO`: the dimension line, its label and
its endings are drawn `/LL` from the points (clockwise on screen when positive), with leader lines from `/LLO` next to
each point to `/LLE` past the dimension line. `/Rect` and `/BBox` take in the endings, the leaders and the wider
stroke. A polygon with an `/IC` keeps the fill opacity the file asked for (`/ca` or `/CA` of the annotation, else the
`/ca` of its appearance's graphics states) and is opaque when it asked for none; ours is a 12% wash. A distance with
no `/LE` gets our end ticks. Every move writes a new appearance stream and font and points
`/AP /N` at them; it never edits the stream the annotation pointed at, because a file can share that form with other
annotations or the page. The stream MuPDF's own `update` makes in between is deleted, but the previous appearance (and
its font) stays in the file unused until a full save. An `annot_edit` colour change makes MuPDF redraw the shape without the label.
`measure_list` lists every line, polyline or polygon with a readable `/Measure` and recomputes its value from its points
and its own `/Measure`, so a file from Acrobat shows Acrobat's scale.

## Snapping (`snap.rs`)
`SnapIndex::build(doc, page)` runs the page's contents (not its annotations) through a MuPDF device and keeps the line
segments (curves as 12 pieces) with their end points and midpoints (a curve's t = 0.5 point). `snap(x, y, radius)`:
an end point, midpoint or crossing of two segments within the radius wins, the nearest (end point, then crossing, then
midpoint on a tie); with none, the nearest point on any segment within the radius; else `None`. Result
`{kind: endpoint|midpoint|intersection|line, x, y, distance}` in page space. Kinds can be switched off per request
(`kinds`, all on when absent): an off kind is never the answer, the next one is (midpoints off still snaps to the line).
Crossings are strictly inside both segments (a shared or touching end point is an end point) and are looked for
among the 64 segments nearest the pointer.
`SnapCache` (managed state) keeps the last four indexes by (doc, `rev`, page), so a mouse move reads the page once per
revision; the first read runs on the edit lane, and requests that arrived together share it (they look again once they
hold the lane). A page is read with limits: 100,000 segments (a curve is 12) and 1.5 s. The time limit is a watchdog
thread that aborts the MuPDF run (a cookie `AbortHandle`, added to `vendor/mupdf`) whatever it is busy with, not a check
in the path callbacks; the index keeps what it has read (`truncated`).

## Commands (all `doc` = doc id, `page` 0-based)
- `measure_scale_get(doc, page) -> {scale, defined, label}`; `measure_scale_set(doc, page, scale) -> EditState`.
- `measure_add(doc, page, kind, points, scale?, author) -> {state, xref, quantity}`; one undo step
  ("Add distance measurement on p. 2"). `scale` absent: the page's.
- `measure_value(doc, page, kind, points, scale?, xref?) -> quantity` (live readout, changes nothing). With `xref` the
  points are valued under that annotation's own `/Measure` (its `/D`, `/A`, units; the readout while a point is
  dragged); else under `scale`, else the viewport of the page at the first point.
- `measure_move(doc, page, xref, points) -> {state, xref, quantity}`: the points of one measurement are replaced in
  place, as one undo step ("Move a point of a measurement on p. 2"). The same annotation is kept (same xref, replies,
  author, subject and its own `/Measure` stay); `/L` or `/Vertices` (user space), `/M`, the appearance with its label,
  `/Rect` and `/Contents` are written again, and the value comes from its own `/Measure` (a file's own `/D` and `/A`
  apply). Refused: certified at `/P` 1 or 2 (nothing changes, no step), a gone xref, the wrong number of points,
  non-finite or far points. A signed file is saved after the signature as for any annotation edit.
- `measure_list(doc) -> [{page, xref, kind, points, contents, author, quantity, scale}]`.
- `measure_snap(doc, page, x, y, radius, kinds?) -> hit | null` (needs `Arc<SnapCache>` in managed state: done in `lib.rs`).
- `measure_rescale(doc, page, xref, scale) -> {state, xref, quantity}`: the annotation is replaced by one with the same
  kind, points and author at `scale`, as one undo step ("Change the scale of a measurement on p. 2 to 1 in = 1 m"); the
  xref changes. A gone xref or a scale with a zero side is refused and leaves no step.
- `measure_export_csv(doc, to) -> path`: every measurement as CSV to a `pick_save` token (never over an open file).
  `Page,Type,Value,Unit,Label,Scale,Author`; value and unit are apart, UTF-8 with a byte order mark, CRLF lines, and
  text a spreadsheet would run as a formula gets a leading apostrophe.
`quantity` is `{value, unit, text}`. Creating a measurement and setting a scale are `apply_custom` steps (undoable, saved
by Save, in history). The assistant can call `add_one`, `scale_set` and the rest in `measure/mod.rs` with its turn.

## Tests (`cargo test --lib measure -- --test-threads=1`)
Number formats (a mile, foot and inch chain with eighths, the carry up a chain, separators and styles), a file's own
chained `/D` and `/A` in the label and in a move, a malformed `/D` falling back with a note (zero and negative `/C`,
no `/U`, a bad link, nine elements), a nine-element `/X` left out, `plain`; a drag of each kind (same xref, `/L` or
`/Vertices` in user space, `/Rect` around the points, label, one undo step, undo and redo, save and reopen), a drag
under the scale the measurement was made with, a drag of an Acrobat measurement keeping its `/Measure`, refusals
that leave no step, a signed file saved after the signature, certified at `/P` 1, 2 (refused) and 3 (allowed).
Each kind and unit against hand-worked numbers (3-4-5 of 36 pt = 25 ft, a 2 sq in rectangle = 200 sq ft, ...), a rotated
page (the file holds user-space points; the value is unchanged), a `/UserUnit 2` page (`/C` doubled, same value), an
Acrobat-style `/Measure` and viewports (read, rewritten, read again equal; detail viewport wins inside its box), a unit
we cannot name, snapping (end point, midpoint, line, none, curve, rotated page, ignores the measurement itself, cached
per revision, cut off on a page of 150k segments), refusals that leave no step, undo/redo/save/reopen, a pixel check
that the appearance draws, the page scale as an undo step, all four rotations x media/crop box origin x `/UserUnit` 1
and 2 (user-space points and `/C` worked out by hand), a new scale under an existing detail viewport, a shared `/VP`,
a `/UserUnit 0` page, malformed measures, point and coordinate caps, `/Y` with `/CYX`.

## In the app
Entry: Tools -> Comment & measure -> **Measure**, or the command palette ("Measure"). It takes the right side like
Comments does (opening one closes the other) and shows a floating bar over the page area and a Measure rail.
- **Bar:** Select, Distance, Perimeter, Area; the scale chip ("1 in = 10 ft", the page's `/VP` scale when it has one;
  click to type another, or "Calibrate from a known length"); a units menu (the same ratio in another unit: 1 in = 10 ft
  is 1 in = 3.048 m; this sets the page's scale, so it is an undo step and writes a `/VP`, replacing the `/D` and `/A`
  formats of a page that came from Acrobat with ours); snap toggles Ends / Mid / Cross / Path; Done. The bar steps down under the find bar (Ctrl+F) while that is open.
- **Drawing:** distance is click, click. Perimeter and area take a click per corner and finish with a double-click or
  Enter; Backspace takes the last corner back and Esc drops the whole thing. A click on the first corner of an area
  (shown as a filled marker) closes it. Shift holds the line to 45 degrees from the last corner and turns snapping off.
  A label next to the pointer shows the running value and the snap that is hovered ("End point", "Midpoint", "On line").
- **Drag to edit:** with Select and a measurement selected, each of its points shows a handle. Drag one to move that
  point: it snaps as drawing does (end points, midpoints, crossings, the path), Shift holds it to 45 degrees from the
  point before it (an area's first point to its last) and turns snapping off, and the label next to it shows the new
  value and the snap ("End point"...) while you drag, with the same marker as drawing on the point. Release writes it (`measure_move`, one undo step). Esc during the drag puts it back, and so does
a release off the page (outside the window the pointer is clamped to the edge); a click
  without moving writes nothing. From the keyboard a handle takes focus (Tab), the arrow keys move it by 1 point
  (Shift: 10) along the screen whatever the page rotation, and the move is written after a short pause, on Enter, when
  focus leaves or the tool changes (a run of keys is one undo step); Esc drops it, and so does a pointer capture that is
  lost before the button comes up. Handles have an aria label ("Area point 3 of 4").
  Not shown on a certified file at `/P` 1 or 2 (it says why in the rail); a signed file shows the same note links and
  outline give, and the change is saved after the signature.
- **Result:** one `measure_add` (one undo step, saved by Save, in history). The page then draws the engine's own
  appearance with its label. With Select, a click on a measurement selects it: the rail shows its value, units and
  scale. Its scale (or unit) can be changed and Apply re-measures it (`measure_rescale`); Delete (key or button) removes
  it (`annot_delete`, one undo step). Re-measuring replaces the annotation, so replies to it go too and the new one is last
  in `/Annots` (undo brings them back). Undo, redo and a restored version re-read the list and the page scales.
- **Rail:** the selected measurement (its value follows a drag as the label does), or the last value made; the cumulative value per kind and unit (never summed across
  units); the list by page (click to jump to it); Export to CSV.
- **Calibrate:** pick it from the scale chip, draw over a dimension printed on the drawing, type its real length and
  unit: the page scale becomes "1 in = n unit" (n = real length x 72 / the line's length in points).
- **Certified files:** a document certified with DocMDP `/P` 1 or 2 does not allow annotations, so adding a
  measurement, setting a page scale and re-measuring are refused (`measure_*` write calls check `certified()` first, so
  nothing changes and there is no undo step). `/P` 3 allows annotations (measuring, re-measuring) but not a page scale:
  that is a `/VP` entry of the page dictionary, refused at any certification. A certification that cannot be read is
  refused too. The tools are greyed out with the reason (`measure_lock`); at `/P` 3 only the scale chip, the units and
  Calibrate are (the store reads `a11y_protection` for it). Select, the rail and Export to CSV still work.
- **Live label:** where the page's measure is `plain` (our own scale, or none) the label is worked out in the UI from the
  page scale. Where the file has its own `/D` or `/A` (or several viewports, a `/Y`), the UI asks `measure_value`
  with the same points, one request at a time (the newest points are asked for when it lands), so the label and the
  stored value use the same formats, chains included; nothing is shown until the first answer.
- **Shortcuts taken:** the crossings are found among the 64 segments nearest the pointer, and curves cross as their 12
  pieces. A dragged measurement keeps its own scale (re-measuring it is the scale field of the rail); the engine's
  page still shows the old appearance under the highlight until the drag is released.

## Not done
- Not opened in real Acrobat (see the `/D`, `/A` `/C` reading above, the one assumption to confirm). The same goes for
  two number format entries whose spec text could not be checked here: `/FD` is read as "keep trailing zeros and do
  not reduce a fraction" when true (pdfcraft writes it that way, and Acrobat's files carry `/FD true` with two-decimal
  labels), and `/PS` and `/SS` surround the label after the number; with `/O /P` the label (`/PS` + unit + `/SS`)
  comes before the number instead of after it.
- The spec's own examples were not at hand (no copy of ISO 32000 on this machine); the number format tests use
  hand-worked chains (mile, foot, inch in eighths; hours, minutes, seconds) instead.
- `/O` (origin) is kept but unused; `/Y` and `/CYX` are read and applied but never written.
- The polyline is "perimeter" as the brief says (the open path); closing it to a polygon's perimeter is a UI choice
  (add the first point again).
- The UI was built and its unit tests run on Linux; the Windows exe and WebView2 were not run (see the PR for the
  screens still to take).
