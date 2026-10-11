# OCR engine (`src-tauri/src/ocr/`)

Standalone module: a scan goes in, the same file comes out with a text layer: invisible (default),
invisible on a cleaned-up picture, or real editable text. Only the cleaned and editable styles touch the
page image (see "Output styles"); the MediaBox and CropBox are never touched.

## Backend choice

Measured on 9 generated cases (`scripts/ocr-bench/make_cases.py`, ground truth in points) on a 7800X3D,
one worker, ms/page = open + render + recognise, median of 3 after warm-up.

| case | Windows en-US @200 | Tesseract 5.4 `--oem 1` @200 | MuPDF OCR (Tesseract inside) @300 |
|---|---|---|---|
| clean 300 / 150 dpi scan | 100% / 100%, 91 / 72 ms | 100% / 100%, 0.8 s | 99.3% / 100%, 224 / 275 ms |
| 2 deg skew | 100%, 133 ms (boxes need the reported angle) | 90% (137 of 152 words; 49% at 300 dpi) | 96%, boxes off |
| noise | 100%, 180 ms | 100% | 100% |
| two columns | 100%, 93 ms, correct order | 100%, 0.9 s | 96.5% |
| table (57 words) | bag 94.7%, column order 43.9%, 92 ms | 100% at 200 dpi, **0% at 300** (1.4 s) | 0% |
| Swedish + English (å ä ö) | 100% (en-US engine), 98 ms | 55% with `eng`, 100% with `swe+eng` | 54% |
| numbers (`$1,234.56`, ids, Courier) | 95.6% | 97.8%, 2.1 s | 100%, 3.8 s/page |
| 7.5 pt text | 100%, 78 ms | 100% | 82% |

Pick: **Windows.Media.Ocr is the default** (5-15x faster, nothing to install, equal or better on 7 of 9
cases). **Tesseract stays behind the same `Recognizer` trait** for hosts without the language pack or
when the user asks for a language Windows lacks (needs `tesseract.exe`; see "Finding Tesseract").
**MuPDF's own OCR is not used as the writer**: its pdfocr device rasterises the page and re-encodes the
image, which breaks the "never touch the image" rule. As a *recognizer* it is in-process Tesseract
(`mupdf/tesseract`, built on the vendored MSBuild path; exe 9.0 MB vs 7.5 MB, the vendored build
already compiles libtesseract/leptonica): 0.22-0.47 s/page on the clean cases, several times faster
than the CLI and no `tesseract.exe` to install, at the cost of shipping tessdata and flipping a cargo
feature. Its accuracy (measured at 300 dpi, same layout-analysis failures as Tesseract at 300 on the
table and small text) is not better than the CLI's, and it was not benchmarked at 200 dpi or with
`swe+eng`. Left as an option for replacing the CLI backend; not wired in.

Default render resolution is 200 dpi: as accurate as 300 on every case, faster, and Tesseract's
layout analysis was better at 200 on the table.

Known limits: Windows OCR returns an em dash for hyphens in monospace text (normalised back to `-` in
tokens that contain a digit), splits `$1` / `,234.56` (rejoined when the boxes nearly touch) and reads
one language at a time (see "Languages").

## Finding Tesseract

Only three places, in this order, and never `PATH` or the working directory (a planted `tesseract.exe`
in a folder the user opened must not run):

1. the `ocr.tesseractPath` setting (passed to `TesseractCli::discover(Some(path))`),
2. the `INKWELL_TESSERACT` environment variable,
3. `%ProgramFiles%\Tesseract-OCR\tesseract.exe`, then `%ProgramFiles(x86)%\Tesseract-OCR\tesseract.exe`.

The first candidate that is a file with a `tessdata` folder beside it wins; one that does not exist is skipped, not an error. The
resolved path is logged (`ocr: using Tesseract at ...`) before the first run. Tesseract runs with a
180 s timeout per page and is killed when it passes or when the job is cancelled.

## Orientation

A page whose text lies at 90, 180 or 270 degrees is found and read upright; the layer is written in the
page's own frame and **the page is never rotated**. After the first read a health check (word count, share
of common words, boxes taller than wide) decides whether the text is plausible. If not, Tesseract's OSD
(`--psm 0`, confidence >= 2) gives the turn; without OSD (Windows OCR) the page is read at half size at
90/180/270 and a turn must beat the upright read by 1.5x + 24 characters. Cost: nothing on healthy pages,
one OSD call or three half-size reads on odd ones. Both backends place the words correctly on all 90 /
180 / 270 cases in `scripts/ocr-bench/make_cases.py`. `Options.detect_orientation = false` turns it off.
A page in a language outside the stopword list stays at 0 unless a turn is a clear win.

## Reading order

Engines hand back lines in their own order; Windows OCR returns a ruled table column by column. An
XY-cut over line fragments (`order.rs`) separates columns of running text (a gutter no line crosses,
three or more words per fragment on both sides) from tables (cells of 1-3 words are read row by row).
Table case, words in reading order: 43.9% before, **94.7%** after; the two-column and single-column
cases keep their order (tests). Right-to-left scripts: a line whose letters are mostly Hebrew/Arabic is
ordered by descending x and, on a mostly RTL page, columns go right to left; digits and Latin words
inside an RTL line stay in position order (no full bidi split).

## Languages

`lang.rs` builds a **catalog** from what is installed: the Windows OCR packs (`WinRtOcr.languages()`) and
the `.traineddata` files in Tesseract's `tessdata` (`tesseract --list-langs`, read once, never installed
or downloaded). Each entry says which engines can read it, and the sheet shows one searchable multi-select
over the union, each row labelled with its engine. Tags are normalised to the primary subtag (`sv-SE` ->
`sv`; Tesseract's `swe`, `chi_sim` ... map back and forth in `tessdata_name`).

* **Engine pick** (`pick_engine`): Windows OCR reads one language per call, so a single language that
  Windows has goes to Windows (5-15x faster); two or more languages, or one Windows lacks, go to
  Tesseract (all packs loaded at once, `swe+eng`). If neither engine has all of them the plan carries a
  `Missing` list with the reason and a hint: `ms-settings:regionlanguage` for a Windows pack, the
  tessdata file name for Tesseract. The sheet shows the missing-language help with a button that opens
  that Settings page; nothing is downloaded.
* **Auto-detect** (`detect.rs`, the default selection): the document's own text if it has some (the
  stopword rate against each language's list, `lang::guess`), else a 100 dpi sample read of the first page
  that needs OCR with at most 5 installed candidates (the reading that looks most like its own language
  wins), else the user's Windows languages. It never reads more than `MAX_CANDIDATES` times and says where
  its answer came from ("from the text on page 2", "from a sample read", "from your Windows languages").
* **Experimental**: right-to-left (Arabic, Hebrew, Persian ...) and CJK (Chinese, Japanese, Korean) are
  marked "experimental" in the list. They are read, laid out right to left where the line is mostly RTL,
  and written with the CJK fallback face; vertical text and ruby are not handled.
* Windows OCR still runs one language per call, so `ocr_plan` never mixes the two engines in a run.

## Parallelism

40-page real scan (`many40.pdf`, 200 dpi, Windows OCR, 7800X3D): 

| workers | 1 | 2 | 4 | 8 | 12 | 16 |
|---|---|---|---|---|---|---|
| ms / page | 92 | 50 | 26 | 15 | 13 | 12 |

Default is half the logical cores, capped at 8 (the knee of the curve; 16 threads buy 3 ms more).
A byte input is written once to a temp file that all workers open, not copied per worker.

## Output styles

`Options.style` is one of `Style::{Searchable, Cleaned, Editable}`; the sheet's radio group maps to it.

| style | what happens to the page | image touched |
|---|---|---|
| Searchable image (default) | the invisible layer above | never |
| Searchable image, cleaned up | straighten (deskew) and despeckle the scan, then the same layer on the upright picture | yes: replaced by a new image object, same family |
| Editable text | words are written as real text in a matching font and the recognised areas are removed from the picture | yes, only under recognised words |

Common to all: remembered settings (languages, style, dpi 150/200/300, skip-pages-with-text, font), one
undo step, cancel with progress, and a **signed document is left alone** (`SigFlags` & 1, explained in the
sheet; the file-level `ocr_file` still copies it with a warning).

### Cleaned up (`clean.rs`, `raster.rs`)

Only a page whose picture is one unmasked, unclipped image that covers at least 60% of the page is
touched (`find_scan`; a fully opaque `/SMask`, which some scanners write on every page, counts as no
mask and is dropped from the new picture, any other mask leaves the page alone); a page with a redaction annotation is left to the redaction engine. The deskew angle
is the engine's reported skew when it has one, else the angle whose row projection is sharpest
(0.25..15 degrees, `raster::row_sharpness`); a tilt below 0.25 degrees is not worth the softening. Specks
are connected components below a size and ink-density limit that are not near text (`raster::despeckle`);
the fill is the page's paper colour (mode, not mean). The result is re-encoded **in the family of the
original**: JPEG stays JPEG (quality 90), a 1-bit image becomes CCITT G4 again (vendored MuPDF
`fz_compress_ccitt_fax_g4`), anything else Flate. The new image is a *new object* that only this page
points at (`rebind` copies the page's Resources/XObject), so an image another page shares stays
byte-identical. A page that was already straight and clean keeps its bytes (`pages_clean`). The sheet
warns that the file is re-encoded (JPEG is lossy).

### Editable text (`editable.rs`)

For every word of every recognised page, a verdict:

* **unsure** (confidence below 60, or the `suspect` heuristic: stray symbols, mixed script, a lone
  consonant run ...): *not* removed from the image and written invisibly (`Tr 3`, still searchable), so
  nothing is ever lost to a bad reading. Counted as "N words kept as image (unsure)"; the result card
  has a **Review unsure words** list that jumps to each one.
* **no glyph**: the bundled faces have no glyph for it (`no-font`): kept as image.
* **picture**: the pixels around the word's box are not flat paper (ring median +-28, at least 75% of the
  ring flat), so it sits on a photo or a shaded area: kept as image.
* otherwise **converted**: the box is filled with the local paper colour in a *copy* of the picture and a
  real word is written.

Layout, per line: baseline = the steepest ink-density drop in the line's row profile; font size =
(baseline - ink top) / 0.74 for lines with ascenders, capitals or digits (else the page median);
monospace when the width per character is nearly constant (CV < 0.07, at least 10 words of 4+ chars);
colour = mean of the darkest quarter of the ink pixels; each word anchored at its own start with a
`TJ` gap and `Tz` (clamped 60..160) to fit its width, so a wrong advance never accumulates along the line.
Fonts are the bundled Noto Sans / Sans Mono / CJK subsets, embedded once per document as Type0/Identity-H
with a ToUnicode map, so text extracts and the Edit text tool edits it (it understands `Tz` and `TJ`).
The invisible layer stream comes first (it starts `/InkOcr BMC`, so a rerun strips it without
dropping the real text); the real text is a separate stream. The picture is replaced by a new image
object (the same copy-on-write as above); the original image is no longer referenced, and
`apply_removing` writes the file with unreferenced objects dropped, so the replaced pixels really leave the
file. **Undo and version history restore the original**: the history snapshot before the run keeps the
untouched image (tested).

A page that does not qualify (several pictures, a redaction annotation, a mask, no usable verdicts) falls
back to the plain invisible layer and is counted in `pages_kept`. Editable text always runs with
skip-pages-with-text on, so a page that already has text is never doubled.

### Preview

`ocr::preview` runs the same pipeline on **one page** in memory (no history entry, no file) and returns
the page before and after as PNG, so the sheet can show a before/after and the user decides before
anything is applied.

### Limits

* No serif face is bundled: serif scans become sans text of the right size and position (a stated
  limitation, shown in the sheet).
* Size and baseline are measured per line with heuristics; a skewed scan is measured in the engine's
  frame and mapped back, so error grows with skew and with unusual type.
* Text over a photo or gradient is left as image; mixed-script lines use the CJK face for the whole word.
* Calibration (tests): positions within 2 pt on a clean and on a 2 degree skewed synthetic page.

## Writer

* One new content stream appended to `/Contents` (`/InkOcr BMC q BT 3 Tr ... ET Q EMC`); the old contents
  are wrapped in `q`/`Q` so a scanner's leftover CTM cannot scale the layer. A forced rerun removes our
  layer **and** its `q`/`Q` pair first, so wrappers do not stack.
* One shared glyphless Type0 / Identity-H font per document (Tesseract's 573-byte glyphless TTF,
  Apache-2.0). Characters above U+FFFF (emoji, mathematical letters) cannot be one 2-byte code, so each
  gets a Private Use Area code and the ToUnicode CMap maps it back to the surrogate pair; the CIDToGIDMap
  reaches the largest code used. Text extracts exactly as recognised.
* The font resource is `InkOcrF` unless the page already has something else under that name, then
  `InkOcrF1`, `InkOcrF2`...
* Words are fitted with `Tz` to their box; a real space follows each word; `$1,234.56` is one token
  (Acrobat wrote `$1 ,234.56`).
* Word boxes go through the inverse page matrix, so `/Rotate` and CropBox offsets are handled; a skewed
  scan gets the layer rotated by the engine's reported angle (detect only, the image is not deskewed).
* Pages that already have >= 12 non-space characters of text are skipped unless `force`; `force` also
  replaces an earlier layer of ours.
* Saved incrementally when the file allows it (every original byte stays where it was). A signed
  document (AcroForm `SigFlags` & 1) is still copied, but `Report.warnings` says the signature no longer
  validates; the caller decides whether to offer it.

## Files and failure

`ocr_file` never replaces anything: the output must not exist (`OutputExists`) and must not be the
input by any spelling (canonicalised: `.`/`..`, case on Windows, links). Work happens on a uniquely
named `create_new` scratch file beside the output (and one more for a full save), and the result is
published with a no-clobber hard link / exclusive copy; scratch files remove themselves on every
path, including a panic in the writer (`catch_unwind`). Cancel returns `Err(Cancelled)` and writes
nothing. If no page could be read and some failed, the result is an error (`Backend`), not an
empty "success". A page that fails in a run where others succeed is reported in `Report.pages`.

## API

```rust
use inkwell_lib::ocr::{self, backend::WinRtOcr, Input, Options};

// Whole file, off the UI thread. `progress` runs on worker threads; set `cancel` to stop.
let report = ocr::ocr_file(&in_path, &out_path, &WinRtOcr, &opts, &|p| emit(p), &cancel)?;

// Edit-core lane: recognise in parallel without writing, then apply on the serial lane.
let rec = ocr::recognize(&Input::Path(&path), &WinRtOcr, &opts, &|_| {}, &cancel)?;
let applied = ocr::apply_styled(&mut pdf_document, &rec.pages, &opts)?; // then ocr::save(..) or the edit core's own save
```

`Options { pages, languages (BCP-47), dpi (200), force, detect_orientation (true), workers, style, font }`; `ocr::parse_page_range("1-3,5,8-", n)`;
`Report`, `Progress`, `PageStatus` and `PageResult` are `Serialize` for the UI. Errors are `OcrError`
(`Cancelled`, `LanguageUnavailable`, `BadPage`, `Pdf`, `Backend`, `Io`, `OutputExists`).

## Reproducing the benchmark

```
python scripts/ocr-bench/make_cases.py <cases_dir>
cargo build --release --bin ocrbench          # in src-tauri, LIBCLANG_PATH set
bash scripts/ocr-bench/run_all.sh <cases_dir> <out_dir>
```
