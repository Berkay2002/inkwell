# PDF Portfolios

A PDF Portfolio is a PDF whose catalog has a `/Collection` dictionary over its embedded files. Inkwell opens one as
a file list, can extract its files, and can make a new one from picked files.

Code: `src-tauri/src/organize/portfolio/` (`mod.rs` reads the collection and keeps it in step with edits, `make.rs`
creates and extracts, `tests.rs`), commands in `organize/app.rs`. UI: `src/components/PortfolioView.tsx`
(+ `portfolio.css`), state in `src/lib/portfolioStore.ts`, order and rows in `src/lib/portfolioLogic.ts`. The files
themselves are read, listed, described and deleted by `organize/attach.rs`; portfolios add what the collection says
about them. There is one embedded-file reader.

## What the file says (read)

`portfolio::read(doc)` is `None` for a PDF without `/Collection`. Otherwise:

- **Schema** (`/Schema`): one field per key with `/Subtype` (S text, D date, N number, F file name, Desc, Size,
  ModDate, CreationDate, CompressedSize), `/N` name, `/O` column order, `/V` visible. At most 64 fields.
- **Values** of a file, as ISO 32000 and Acrobat have it: the fields of subtype F, Desc, Size, ModDate,
  CreationDate and CompressedSize show the file's own facts (the file specification's name and `/Desc`, the stream's
  `/Params` Size, ModDate, CreationDate, and `/Length` for CompressedSize), and `/CI` only when the file gives none.
  Fields of subtype S, D and N come from `/CI` (a plain value, or a `/CollectionSubitem` with `/D` and a `/P` prefix
  shown in front).
- **Sort** (`/Sort`): `/S` a key or an array of keys, `/A` a bool or an array (a missing one is ascending). Strings
  compare folded like a search folds them (case-insensitive, accents kept) and with digit runs in number order
  ("file2" before "file10"; any run length, nothing is parsed to an integer); numbers as numbers; dates as dates
  (zones respected). A missing value sorts last. Ties keep the name tree's order.
- **View** (`/View` D, T, H, C) and **initial document** (`/D`, the key in the name tree).
- **Folders** (`/Folders`, PDF 1.7 extension level 3): a root whose `/Child` is the first of a `/Next` chain of
  `/Folder` dictionaries (`/ID`, `/Name`, `/Child`, `/Next`). A file is in a folder through the `<id>` prefix of its
  name-tree key (`<3>report.pdf`), or an integer `/Folder` in its file specification. Depth is capped at 32, folders
  at 10 000, files at 50 000 (as attachments). A loop through `/Child` or `/Next` (or one folder reached twice) is
  refused: `portfolio_info` returns an error, the UI says so once and shows the document as a plain PDF.
- Every string shown (field and folder names, `/CI` text, file names) loses control, bidi and zero-width characters.
- **Hostile structure is bounded.** The name tree is read with a visited set and a 100 000 node budget (a `/Kids`
  that loops or repeats is read once). A string is cut to 2 KB when read and remembered by object number, so one
  huge string that many fields point at is read once; a file specification reached again by another name reuses its
  cells; cell text and descriptions are kept up to 32 MB in all (the rest are blank); equal values are ranked once.
- Two name-tree entries with one key get two ids (the second carries its index), so each row is one file.

The engine returns the files in the collection's own sort order, the values as display text, and for each file its
**rank per field**. A click on a header sorts by those ranks in the UI, so there is one collation (the engine's)
and a header click needs no round trip.

## The view

When the document is a portfolio the stage shows the file list instead of the page view. `/View /H` (the author
asked for no portfolio view) starts on the cover page instead.

- Columns: the schema's visible fields in `/O` order. A click on a header sorts by it, a second click flips it;
  the collection's sort is the default (its column shows an arrow and "portfolio order"; the toolbar's "Portfolio
  order" returns to it). Numbers are right-aligned.
- Folders are expandable rows (name and file count); files sit under their folder, folders first. The list is
  windowed, so 50 000 files stay light. Arrow keys move, Left/Right fold, Enter or double-click act.
- Toolbar: **Open** (a PDF opens in a new tab as an unsaved scratch copy, the attachments path),
  **Extract…** (one file to a Save dialog), **Extract all…** (a folder dialog), **Show cover page** (the normal page
  view; a note over it has "Show files").
- **A file that is not a PDF is never opened or run.** The primary action for it is "Extract…", which first asks
  with the file's name and type and then shows the Save dialog. No thumbnails or previews of non-PDF files.
- The attachments panel keeps working on the same files. A file added there to a portfolio gets its `/CI` filled from
  the document's own schema (name, description, size, modified, by subtype, whatever the keys are called);
  describing a file also updates the `/CI` description (the file's own `/Desc` is what is read first); deleting the initial
  document drops `/D`. All through the attach edit steps: one undo step each.

## Extract

- One file: the Save dialog and `attachment_save` (written to the picked file, marked from the Internet zone on
  Windows, never over a file that is open in a tab).
- All: `portfolio_extract_all(doc, folderToken)` writes flat into the picked folder. Each name goes through
  `clean_name` (no path or `..`, no control or bidi characters, no `< > : " | ? *`, `CON`/`NUL`/`COMn`/`LPTn` get a
  `_`; so do `CONIN$`, `CONOUT$` and `COM`/`LPT` with superscript digits) and is cut to 200 characters, 250
  UTF-16 units and 250 UTF-8 bytes, keeping the extension. A name that is taken (in the folder, or earlier in the
  same run, on a case-insensitive disk too) becomes `name (2).ext`: files are opened with create-new, nothing is
  replaced. Limits: 512 MB per file as decoded, 4 GB of decoded bytes per run (a file that turns out too big counts
  the work it cost); a file over a limit, one that links to a file outside the document, one that shares its stream
  with an earlier name, or one that cannot be written is left out and listed (200 lines, then a count) and the run
  goes on. Each file is marked as from the Internet (Windows) the moment it is written. One file's bytes are held at
  a time. The document is only read (not dirty).

## Create PDF Portfolio…

Command palette. Pick files (any type), then a Save dialog; the result opens in a tab.

- 1 to 500 files, each up to 512 MB, 2 GB together; sizes are checked before anything is read. A folder, or the
  output being one of the inputs, is refused. The sources are only read.
- A **cover page** (A4, Helvetica, as many pages as the list needs): "PDF Portfolio", the file count and total size,
  a table of names and sizes. Readers that ignore `/Collection` show it.
- Every file is embedded with `/Params` Size and ModDate (the file's own time), `/AFRelationship /Unspecified`,
  a `/CI` and a name made unique ("a (2).pdf"). `/Collection`: schema Name (F), Description (Desc), Size, Modified
  (ModDate), `/View /D`, `/Sort` on Name ascending, `/D` the first PDF (left out when there is none).
- Written like Combine: temp file, read back, then renamed over the target.

## Known limits

- The view always lays out as Details; `/View T` and `C` are read but not drawn as tiles or a custom layout.
- Folders are shown as folders but Extract all writes flat (clashes get " (2)"), not into sub-folders.
- Creating a portfolio makes no folders; a `/CI` field of subtype S, D or N is only read, never filled for new files.
- There is no preview pane (Acrobat's layout has one beside the list); a PDF is looked at by opening it in a tab.
- The page-view tools (zoom, find) act on the cover page, so they do nothing visible while the list shows.
- The `<id>` folder prefix is read as a decimal number; this is the form the brief describes, not verified against
  a file made by Acrobat here.

## pdfcraft parity (for the docs refresh)

Organize row in `docs/pdfcraft-parity.md`: remove "Portfolios" from the gaps and add "PDF Portfolios: Portfolio view
(schema columns, sort, folders), Extract / Extract all, Create PDF Portfolio (docs/portfolios.md)". pdfcraft has no
`/Collection` code (its roadmap lists PDF Portfolios as not yet), so nothing is adapted from it and
`third_party/pdfcraft/INKWELL-NOTICE.txt` has no new entry.
