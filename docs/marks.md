# Page marks: header & footer, watermark, background, Bates numbering

Code: `src-tauri/src/marks/` (`mod.rs` the types and the `<<…>>` tokens, `apply.rs` the writer, reader and Bates
files, `geometry.rs` display space, `text.rs` fonts and widths, `app.rs` the commands, `tests.rs`), the edit-core
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

## Not done
- **Shrink document to avoid overlap**: not built (needs the content's extents; margins are the control instead).
- **`/PieceInfo`** in Acrobat's own format: unknown, see the study. Marks made by Acrobat are not recognised by
  Update/Remove here (only streams tagged `/PCMark`); the other way round Acrobat sees ours as page content.
- Picture sources are PNG, JPEG (and what MuPDF decodes) or a PDF page; no clipboard source. Update of a picture
  mark asks for the picture again (the file only remembers its name).
- Dates are taken when the button is pressed; a file is not re-dated later.
- The watermark preview of a picture is a box, not the pixels.
