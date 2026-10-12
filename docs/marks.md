# Page marks: header & footer, watermark, background, Bates numbering

Code: `src-tauri/src/marks/` (`mod.rs` the types and the `<<…>>` tokens, `apply.rs` the writer, reader and Bates
files, `geometry.rs` display space, `fit.rs` the shrink and other apps' marks, `text.rs` fonts and widths, `app.rs` the commands, `tests.rs`), the edit-core
hooks in `src-tauri/src/engine/edit.rs`, UI in `src/components/MarksPanel.tsx`, `src/components/MarkPreview.tsx`,
`src/lib/marksStore.ts` and `src/components/marks.css`. Screens: `docs/screens/marks/` (`node
scripts/marks-shots.mjs`). Study of Acrobat's marks: `docs/study/15-page-marks.md` (not observed). Design round:
`design/rounds/marks-r1/` (variant a-tabs, with the "Has:" state line from b-list).

## What it does
- **Header & footer**: six slots (top and bottom, left, centre, right), font family (Helvetica, Times,
  Courier) with bold, italic and underline, size, colour, margins (points: top, bottom, left, right), a start
  number, the pages (all, odd, even, a typed range). Tokens in the text: `<<1>>`, `<<n>>`, `<<1 of n>>`,
  `<<1/n>>`, `<<Page 1>>`, `<<Page 1 of n>>` and the dates `<<m/d>>`, `<<m/d/yy>>`, `<<m/d/yyyy>>`,
  `<<mm/dd/yy>>`, `<<mm/dd/yyyy>>`, `<<d/m/yy>>`, `<<d/m/yyyy>>`, `<<dd/mm/yyyy>>`, `<<yyyy-mm-dd>>`,
  `<<mmmm d, yyyy>>`. The page number counts from the start number over the *selected* pages; the date is taken
  by the UI when you press the button, so the preview and the file agree.
- **Watermark**: text (fitted to the page diagonal by "Size, relative to the page") or a picture (an image file or
  a page of a PDF), opacity, rotation (none, 45°, −45°, custom), an offset from the centre, behind or in front of the
  page content, "Show on screen" and "Show when printing".
- **Background**: a colour or a picture, opacity, scale. Always behind the content.
- **Bates numbering**: prefix, suffix, 3 to 15 digits, start number, one of the six places. With files in
  the list ("Add files…"), copies are written to a folder (`pick_folder` token) as `<name> - Bates.pdf` (`(2)`…
  never overwriting) and the numbers run on from file to file; this document is not changed. Without files it
  numbers this document, as one undo step.
- **Update / Remove**: marks are found by a tag, not by looking at the page. The rail starts with a "Has:" list of
  what the document has and on which pages, each with Update (opens the tab with the settings the file was made
  with) and Remove. Add on pages that already have the kind is an Update (it replaces them). Each Add, Update
  and Remove is one undo step ("Add header and footer to pp. 1-12"), written by Save or Save as; an update or
  remove makes the save a full rewrite so the replaced content is gone from the file. Remove also takes out the
  `IWH…` fonts, `IWGS…` graphics states, `IWPic…` pictures and `IWOC…` layer names that no content uses any
  more (the group goes with the last watermark).
- **Shrink pages to fit the margins** (header & footer and Bates): the page's content is scaled down so the
  marks do not sit on top of it, see "Shrink" below.
- **Other apps' marks**: a header, footer, watermark or background that another app wrote is listed in "Has:" as
  "Header/footer from another app" and so on, with Remove only (no Update: there are no settings to read back).
- **Preview**: `MarkPreview` draws the open tab over every affected page as SVG inside the page's own space (zoom
  and rotation apply), with the same margins, baselines and fitted size as the engine. Text slots are outlined
  with a dashed accent so it reads as a preview. A picture is shown as a labelled box (its pixels are not read
  until the step runs). Nothing is drawn over a page whose file mark already equals the sheet.
- **Outcome sentence** above the button: "Adds 'Lease agreement' to the top right and 'Page 1 of 12' to the bottom
  right of 12 pages." (`outcome()` in `marksStore.ts`).
- Entry: the All tools panel (Header & footer, Watermark, Background, Bates numbering) and the command palette
  ("Add header and footer…", "Add page numbers…", "Add watermark…", "Add background…", "Add Bates numbering…").

## How a mark is stored
- One content stream per mark and page, appended to the page's `/Contents` (prepended for a mark behind the page).
  A picture is remembered in `/IWSpec` by its file name only (no folder, no password), so Update asks for the
  picture again.
  The stream dictionary has `/PCMark /HeaderFooter|/Watermark|/Background|/Bates` (pdfcraft's tags, so files made by
  either app are read by the other) and `/IWSpec`, the settings as ASCII JSON (at most 16 KB), which Update reads.
- The drawing is wrapped as an artifact: `/Artifact <</Type /Pagination /Subtype /Header|/Footer|/Watermark
  [/Attached [/Top]|[/Bottom]] /PCMark /…>> BDC`, then `cm` (display space to the page's user space), the body,
  `EMC`. Background uses `/Type /Background`. Text extraction and screen readers skip artifacts; MuPDF's
  structured text still returns the words, but they come after the body: the page's own streams are first,
  marks that sit in front come after them. Only a mark behind the page (watermark behind, background) precedes the
  body, and those are the ones a reader does not need first.
- Where something is drawn on top, the page's original content is wrapped in two tiny `q %Inkwell` / `Q %Inkwell`
  streams so its graphics state cannot leak into the mark; the wrappers go when the last mark on top goes. The
  wrapper balances what the content leaves open: it counts the content's `q`/`Q` and adds the extra `q` (for a
  stray `Q`) and `Q` (for an unclosed `q`) it needs.
- Resources are never edited in place: a page's `/Resources` (and the `/Font`, `/ExtGState`, `/XObject`,
  `/Properties` category being added to) is copied onto the page first, because it may be inherited from the page
  tree or shared with other pages. A page with no resources gets its own. The layer's name on a page is
  `IWOC<object number of its group>`, so two watermarks never share a name.
- Streams that were rewritten since (autotagging, text editing merge a page's streams into one) lose the stream
  dictionary but keep the `/Artifact <</PCMark …>> BDC` marked content: Update and Remove read that too and cut
  the section (with the `q`…`Q` around it) out of the merged stream.
- Opacity is the page's ExtGState `IWGS<percent>` (`/CA` and `/ca`). Fonts are the standard 14 as shared indirect
  Type1 objects with WinAnsiEncoding, registered per page as `IWH`, `IWHB`, `IWHI`, `IWHBI`, `IWT…`, `IWC…`.
  Pictures are `IWPic<objnum>` XObjects: an image through `add_image`, a PDF page as a Form XObject with its
  resources grafted, its `/Matrix` the display transform and `/BBox` the crop box.
- "Show on screen / when printing": an optional content group "Watermark" with `/Usage` `/View /ViewState` and
  `/Print /PrintState`, listed in the catalog's `/OCProperties /OCGs` and its default configuration's `/AS` array;
  the page names it `IWOC` in `/Properties`. The group and the name go with the last watermark.

## Layout rules (display space)
All marks are laid out in the page as shown (y up, `/Rotate` and the crop box applied); a `cm` of `View::matrix`
maps that to user space, so the same numbers hold on rotated and cropped pages (test:
`text_lands_where_asked_on_every_rotation_and_box`).
- x: left margin; `(w − text width) / 2`; `w − right margin − text width`.
- Header baseline `h − top − 0.8·size − line·1.2·size`; footer baseline `bottom + 0.2·size + (lines−1−line)·1.2·size`.
- Widths come from MuPDF's Helvetica, Times and Courier (Base-14) metrics; the preview measures with the browser's
  Arial, Times New Roman and Courier New, which are the same advances.
- Watermark text: size = `diagonal × scale ÷ widest line at 1 pt`, clamped to 6 to 400 pt, centred at the page
  centre plus the offset, rotated counter-clockwise, line height 1.15 × size.

## Safety
- Requests are validated before anything is drawn (empty text, characters outside WinAnsi, sizes, margins, pages,
  digits and start number that would run out); one page failing leaves no page half-marked (all in one step).
- Opening a picture checks it is an image or a PDF; a password-protected PDF source takes `password`.
- Bates files: the input's rights are checked (`may_extract`), the digits are checked for the whole run before the
  first file is written, every output name is fixed and checked with `Engine::check_not_open` (never a document
  that is open) before the first is written, outputs go through `write_new` (never overwrite) and the originals
  are not touched. An input that is open in Inkwell is refused (its unsaved changes would be left out of the
  copy): save and close it first. A failure after the first copy names the copies already written.

## Shrink (`fit.rs`)
- The box is the page less the bands the marks use: top = top margin + the height of the lines in the three top
  slots (0 when they are empty), bottom likewise, left and right = the left and right margins as set. The scale is
  `s = min(1, (h − top − bottom) / h, (w − left − right) / w)`, never up, and the content is centred in the box.
  The left and right margins are read literally, so the default 72 pt on a Letter page gives about 0.76 (the
  page shrinks in both directions, not only down). `s < 0.1` is refused ("The margins leave too little room…").
- Written as one section in the opening `q %Inkwell` wrapper stream, before the balancing `q`s so a stray `Q` in
  the page's content cannot undo it: `/Artifact <</PCMark /Shrink /HeaderFooter [t b l r] /Bates [t b l r]>> BDC`,
  the `cm` (display space scaled and moved, mapped through `View::matrix`, so `/Rotate` and crop boxes work), a
  clip `x y w h re W n` to the visible area (what a crop box hid stays hidden), `EMC`. The dictionary lists, per
  kind of mark, the bands it asked for; the `cm` is rebuilt from the largest of each band whenever a mark is added,
  updated or removed. `Update` keeps it (the checkbox comes back from `/IWSpec`), unticking it gives the page back,
  and removing the last mark that asked for it takes the section out, and the wrapper with it when no mark on top
  is left (the content stream is then the original bytes). The entries are also read from a merged stream (a text
  edit or autotagging rewrote the page's streams).
- Comments, links and form fields are **not** moved (their rectangles are page-level): the sheet says "Comments
  and links keep their place." when a selected page has annotations (`info().annotated`). The preview does not
  show the shrink.
- Tests: `shrink_puts_the_content_between_the_bands_on_every_rotation_and_box`,
  `shrink_with_nothing_to_clear_does_not_scale_up`, `shrink_clips_to_the_visible_area`,
  `shrink_survives_unbalanced_content_and_a_stray_q`, `update_keeps_the_shrink_and_unticking_it_gives_the_page_back`,
  `removing_the_mark_takes_the_cm_and_the_wrapper_out`, `two_kinds_share_the_page_and_each_removal_recomputes_it`,
  `a_shrink_is_found_again_after_the_streams_were_merged`, `margins_that_leave_no_room_are_refused`.

## Other apps' marks (`fit.rs`)
- Found by the marked content alone (no `/PCMark`): `/Artifact <</Type /Pagination /Subtype …>> BDC … EMC` with
  Header, Footer or PageNum (header/footer), Watermark (watermark), Bates (Bates numbering) or no subtype
  (header/footer); and `/Artifact <</Type /Background …>>` (background). `LineNum`, `Redaction` and unknown
  subtypes, `/Layout`, `/Page` artifacts and untagged content are never touched. Marked content that carries our
  `/PCMark` is ours and is not listed twice.
- Remove cuts exactly `BDC … EMC`, balanced (nested `BDC`/`BMC`/`EMC` inside are part of it), when the section is
  clean: `q`/`Q` and `BT`/`ET` balance inside it and it has no `cm` at depth 0 (its drawing cannot change the
  state the page's own content continues in). A section that is not clean, or whose `BDC` and `EMC` are in
  different streams, is left and counted as "stuck" (the row says so); when every page is stuck the step is
  refused with a sentence and nothing changes. Certified documents are refused, signed ones stay incremental.
- Known limits: colour, `gs` and `Tf` set at depth 0 inside a cut section would have leaked into the following
  content in the original too, so they are left as they were written (not tracked); property lists given by
  name (`/Artifact /MC0 BDC`) are not read; sections inside Form XObjects are not handled; marks only in
  Acrobat's `/PieceInfo` are not seen.
- Tests: `another_apps_header_is_found_and_cut_and_the_body_stays`,
  `another_apps_watermark_and_background_are_their_own_rows`,
  `a_section_that_cannot_be_cut_cleanly_is_left_and_reported`, `our_marks_and_another_apps_do_not_mix`.

## Certified and signed
Both new paths follow the rule of the other marks: a certified document refuses Add, Update and Remove
("This document is certified…", the controls are disabled with that reason, `info().certified`), a signed one
keeps the save incremental (`apply_custom(…, false)`) and the panel says the signature will show the change
(`info().signed`). Tests: `a_certified_document_refuses_the_shrink_and_the_removal_of_another_apps_marks`,
`a_signed_document_keeps_its_signed_bytes_through_a_shrink_and_a_removal`.

## Not done
- **`/PieceInfo`** in Acrobat's own format: unknown, see the study. Marks made by other apps are found by their marked
  content (Remove only, see above); Acrobat's own files were not checked, the other way round Acrobat sees ours as page content.
- Picture sources are PNG, JPEG (and what MuPDF decodes) or a PDF page; no clipboard source. Update of a picture
  mark asks for the picture again (the file only remembers its name).
- Dates are taken when the button is pressed; a file is not re-dated later.
- The watermark preview of a picture is a box, not the pixels.
