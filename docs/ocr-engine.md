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
when the user asks for a language Windows lacks (needs `tesseract.exe`; found via `INKWELL_TESSERACT`,
`E:\Tools\Tesseract-OCR`, Program Files, PATH). **MuPDF's own OCR is not used as the writer**: its pdfocr device rasterises the page and re-encodes the
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
tokens that contain a digit), splits `$1` / `,234.56` (rejoined when the boxes nearly touch), reads one
language at a time, and orders table cells column-wise.

## Writer

* One new content stream appended to `/Contents` (`/InkOcr BMC q BT 3 Tr ... ET Q EMC`); the old contents
  are wrapped in `q`/`Q` so a scanner's leftover CTM cannot scale the layer.
* One shared glyphless Type0 / Identity-H font per document (Tesseract's 573-byte glyphless TTF,
  Apache-2.0), identity ToUnicode, so text extracts exactly as recognised.
* Words are fitted with `Tz` to their box; a real space follows each word; `$1,234.56` is one token
  (Acrobat wrote `$1 ,234.56`).
* Word boxes go through the inverse page matrix, so `/Rotate` and CropBox offsets are handled; a skewed
  scan gets the layer rotated by the engine's reported angle (detect only, the image is not deskewed).
* Pages that already have >= 12 non-space characters of text are skipped unless `force`; `force` also
  replaces an earlier layer of ours.
* Saved incrementally when the file allows it (every original byte stays where it was).

## API

```rust
use inkwell_lib::ocr::{self, backend::WinRtOcr, Input, Options};

// Whole file, off the UI thread. `progress` runs on worker threads; set `cancel` to stop.
let report = ocr::ocr_file(&in_path, &out_path, &WinRtOcr, &opts, &|p| emit(p), &cancel)?;

// Edit-core lane: recognise in parallel without writing, then apply on the serial lane.
let rec = ocr::recognize(&Input::Path(&path), &WinRtOcr, &opts, &|_| {}, &cancel)?;
let applied = ocr::apply(&mut pdf_document, &rec.pages, opts.force)?; // then ocr::save(..) or the edit core's own save
```

`Options { pages, languages (BCP-47), dpi (200), force, workers }`; `ocr::parse_page_range("1-3,5,8-", n)`;
`Report`, `Progress`, `PageStatus` and `PageResult` are `Serialize` for the UI. Errors are `OcrError`
(`Cancelled`, `LanguageUnavailable`, `BadPage`, `Pdf`, `Backend`, `Io`).

## Reproducing the benchmark

```
python scripts/ocr-bench/make_cases.py <cases_dir>
cargo build --release --bin ocrbench          # in src-tauri, LIBCLANG_PATH set
bash scripts/ocr-bench/run_all.sh <cases_dir> <out_dir>
```
