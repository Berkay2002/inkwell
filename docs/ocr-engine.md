# OCR engine (`src-tauri/src/ocr/`)

Standalone module: a scan goes in, the same file comes out with an invisible, searchable text layer.
It never re-encodes or touches the page image, MediaBox or CropBox.

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

Windows OCR runs one language per call, so a list of languages is tried in priority order and the first
installed pack is used for the whole page. Per-line best-of-several was not built: Windows OCR reports no
confidence (the API has none), so there is nothing to compare lines by except word counts, which prefers
the wrong language on mixed lines; and it would multiply the cost by the number of languages. For mixed
documents use Tesseract with `swe+eng` (all requested packs are loaded at once). The run checks the
languages once before any page is read and fails with `LanguageUnavailable` if none is installed.
Packs on the development machine: `en-US`, `ja`, `sv-SE` (`WinRtOcr.languages()` lists them).

## Parallelism

40-page real scan (`many40.pdf`, 200 dpi, Windows OCR, 7800X3D): 

| workers | 1 | 2 | 4 | 8 | 12 | 16 |
|---|---|---|---|---|---|---|
| ms / page | 92 | 50 | 26 | 15 | 13 | 12 |

Default is half the logical cores, capped at 8 (the knee of the curve; 16 threads buy 3 ms more).
A byte input is written once to a temp file that all workers open, not copied per worker.

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
let applied = ocr::apply(&mut pdf_document, &rec.pages, opts.force)?; // then ocr::save(..) or the edit core's own save
```

`Options { pages, languages (BCP-47), dpi (200), force, detect_orientation (true), workers }`; `ocr::parse_page_range("1-3,5,8-", n)`;
`Report`, `Progress`, `PageStatus` and `PageResult` are `Serialize` for the UI. Errors are `OcrError`
(`Cancelled`, `LanguageUnavailable`, `BadPage`, `Pdf`, `Backend`, `Io`, `OutputExists`).

## Reproducing the benchmark

```
python scripts/ocr-bench/make_cases.py <cases_dir>
cargo build --release --bin ocrbench          # in src-tauri, LIBCLANG_PATH set
bash scripts/ocr-bench/run_all.sh <cases_dir> <out_dir>
```
