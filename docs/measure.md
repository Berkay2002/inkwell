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
  used ("sq " + unit for an area). A later element's `/C` is how many of it make one of the previous (feet, then inches).
- Number formats keep `/U /C /F /D /FD /RT /RD /PS /SS /O`: decimal (D 100 is two decimals), fraction, round, truncate;
  our own are two decimals with trailing zeros, "," thousands, "." decimal. Absent `/PS` and `/SS` read as a single
  space (a trailing one is not shown); `/D` is clamped to 1..1e9. A malformed `/Measure` (not a dictionary, `/C` 0, no
  `/X`, junk elements) is left out of `measure_list`, never an error for the document.
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
form `/BBox` are the box of all that. An `annot_edit` colour change makes MuPDF redraw the shape without the label.
`measure_list` lists every line, polyline or polygon with a readable `/Measure` and recomputes its value from its points
and its own `/Measure`, so a file from Acrobat shows Acrobat's scale.

## Snapping (`snap.rs`)
`SnapIndex::build(doc, page)` runs the page's contents (not its annotations) through a MuPDF device and keeps the line
segments (curves as 12 pieces) with their end points and midpoints (a curve's t = 0.5 point). `snap(x, y, radius)`:
an end point or midpoint within the radius wins, the nearer (an end point on a tie); with none, the nearest point on any
segment within the radius; else `None`. Result `{kind: endpoint|midpoint|line, x, y, distance}` in page space.
`SnapCache` (managed state) keeps the last four indexes by (doc, `rev`, page), so a mouse move reads the page once per
revision; the first read runs on the edit lane, and requests that arrived together share it (they look again once they
hold the lane). A page is read with limits: 100,000 segments (a curve is 12) and 1.5 s. The time limit is a watchdog
thread that aborts the MuPDF run (a cookie `AbortHandle`, added to `vendor/mupdf`) whatever it is busy with, not a check
in the path callbacks; the index keeps what it has read (`truncated`).

## Commands (all `doc` = doc id, `page` 0-based)
- `measure_scale_get(doc, page) -> {scale, defined, label}`; `measure_scale_set(doc, page, scale) -> EditState`.
- `measure_add(doc, page, kind, points, scale?, author) -> {state, xref, quantity}`; one undo step
  ("Add distance measurement on p. 2"). `scale` absent: the page's.
- `measure_value(doc, page, kind, points, scale?) -> quantity` (live readout, changes nothing).
- `measure_list(doc) -> [{page, xref, kind, points, contents, author, quantity, scale}]`.
- `measure_snap(doc, page, x, y, radius) -> hit | null` (needs `Arc<SnapCache>` in managed state: done in `lib.rs`).
- `measure_rescale(doc, page, xref, scale) -> {state, xref, quantity}`: the annotation is replaced by one with the same
  kind, points and author at `scale`, as one undo step ("Change the scale of a measurement on p. 2 to 1 in = 1 m"); the
  xref changes. A gone xref or a scale with a zero side is refused and leaves no step.
- `measure_export_csv(doc, to) -> path`: every measurement as CSV to a `pick_save` token (never over an open file).
  `Page,Type,Value,Unit,Label,Scale,Author`; value and unit are apart, UTF-8 with a byte order mark, CRLF lines, and
  text a spreadsheet would run as a formula gets a leading apostrophe.
`quantity` is `{value, unit, text}`. Creating a measurement and setting a scale are `apply_custom` steps (undoable, saved
by Save, in history). The assistant can call `add_one`, `scale_set` and the rest in `measure/mod.rs` with its turn.

## Tests (`cargo test --lib measure -- --test-threads=1`)
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
  is 1 in = 3.048 m); snap toggles Ends / Mid / Path; Done.
- **Drawing:** distance is click, click. Perimeter and area take a click per corner and finish with a double-click or
  Enter; Backspace takes the last corner back and Esc drops the whole thing. A click on the first corner of an area
  (shown as a filled marker) closes it. Shift holds the line to 45 degrees from the last corner and turns snapping off.
  A label next to the pointer shows the running value and the snap that is hovered ("End point", "Midpoint", "On line").
- **Result:** one `measure_add` (one undo step, saved by Save, in history). The page then draws the engine's own
  appearance with its label. With Select, a click on a measurement selects it: the rail shows its value, units and
  scale. Its scale (or unit) can be changed and Apply re-measures it (`measure_rescale`); Delete (key or button) removes
  it (`annot_delete`, one undo step). Undo, redo and a restored version re-read the list and the page scales.
- **Rail:** the selected measurement, or the last value made; the cumulative value per kind and unit (never summed across
  units); the list by page (click to jump to it); Export to CSV.
- **Calibrate:** pick it from the scale chip, draw over a dimension printed on the drawing, type its real length and
  unit: the page scale becomes "(line length in inches) in = (length) unit".
- **Shortcuts taken:** the live label is worked out in the UI from the page's scale (it does not apply a file's own `/D`
  or `/A` multipliers or a detail viewport under the pointer: the label written by the engine is the exact one). The
  snap kinds are the engine's (end points, midpoints, nearest point on a line); there is no intersection snap.
  Editing a measurement means changing its scale or unit; its points are not dragged (delete and draw again).

## Not done
- Not opened in real Acrobat (see the `/D`, `/A` `/C` reading above, the one assumption to confirm).
- `/O` (origin) is kept but unused; `/Y` and `/CYX` are read and applied but never written.
- The polyline is "perimeter" as the brief says (the open path); closing it to a polygon's perimeter is a UI choice
  (add the first point again).
- The UI was built and its unit tests run on Linux; the Windows exe and WebView2 were not run (see the PR for the
  screens still to take).
