# Compress / optimize engine

`src-tauri/src/compress/` (Rust, on the vendored MuPDF). Standalone: input path + `Options` -> new PDF + `Report`.
It never touches the engine's open documents and never writes to the input.

## API (what the UI calls)

```rust
compress::Options::preset(Preset::Balanced)      // Smallest | Balanced (default) | HighQuality | Custom
compress::audit_file(&path, password)             // "Audit space usage": bytes per category, largest items
compress::spawn(input, options, |p: Progress| …)  // -> Job { cancel(), is_finished(), join() -> Report }
compress::compress(&input, &opts, &cancel, &mut progress)   // same, on the calling thread
compress::default_output(&input)                  // "<name> (compressed).pdf" next to the input
```

Errors serialise to codes: `cancelled`, `needs-password`, `bad-password`, `output-exists`, `invalid-options`
(ppi outside 36..=2400 or target above threshold, JPEG/JPX quality outside 1..=100, garbage above 4); the rest are messages.
`Options`, `Report`, `Audit`, `Progress` are serde (camelCase) and ready for the IPC layer.

## Presets

| | colour/gray: above -> to | codec, quality | mono: above -> to | fonts | other |
|---|---|---|---|---|---|
| Smallest | 108 -> 72 ppi | JPEG q18 | 300 -> 200 ppi, CCITT G4 | subset | thumbnails dropped |
| Balanced | 225 -> 150 ppi | JPEG 2000 q75 (JPEG under 512 px) | 450 -> 300 ppi, CCITT G4 | keep | |
| High quality | 300 -> 225 ppi | JPEG 2000 q85 | 600 -> 450 ppi, CCITT G4 | keep | |

All presets: skip images that would grow, object streams, garbage level 4 (dedupe), Flate everywhere.
Metadata, embedded files, JavaScript, bookmarks are only removed when `strip.*` is set (off by default).

## Inkwell vs Acrobat (same input `opt-in-01.pdf`, 2,657,232 B, 2 pages; same render diff at 96 ppi)

| Acrobat run | bytes | max | mean | PSNR | Inkwell | bytes | max | mean | PSNR | vs Acrobat |
|---|---|---|---|---|---|---|---|---|---|---|
| 04: 72 ppi above 108, JPEG minimum | 31,839 | 41 | 1.925 | 36.2 | Smallest | 30,929 | 34 | 1.877 | 36.4 | -3% |
| 01: Single file (JPEG 2000 medium) | 448,574 | 17 | 0.775 | 44.0 | Balanced | 246,447 | 14 | 0.720 | 44.6 | -45% |
| 06: JPEG 2000 maximum, 150 ppi | 866,368 | 15 | 0.693 | 44.9 | High quality | 612,113 | 13 | 0.537 | 47.0 | -29% |

Reproduce: `cargo test --lib compress::tests::presets -- --nocapture --test-threads=1`.

## Behaviour guarantees (all covered by tests)

- Never worse: when the result is not smaller the original bytes are copied to the output and `keptOriginal` is true.
  Exception: if a `strip.*` option actually removed something, the stripped copy is written even when bigger
  (warning "larger than the original because extras were removed").
- Never overwrites: existing output -> `output-exists`; output == input -> error; the output name is claimed
  atomically (hard link, or exclusive create + copy on file systems without links), temp names are unique per job,
  so concurrent jobs on one input cannot corrupt each other; temp files are removed on any failure.
- Cancel is polled per page and between stages; a cancelled job leaves no file behind.
- The output is reopened, its page count checked, re-audited and render-compared (up to 8 pages, 96 ppi):
  `visual.maxDiff`, `meanDiff`, `psnr`, per page.
- Text-only PDFs do not grow; text of every study sample is identical after every preset.
- Encrypted input needs the password (`needs-password`) and the output stays encrypted.

## MuPDF patch

`vendor/mupdf-sys/wrapper/compress.c` is a modified copy of MuPDF's image rewriter (`pdf_rewrite_images`) with
progress/cancel callbacks, per-image failure tolerance, statistics, "re-encode only what was scaled" policy, a
JPEG fallback for JPEG 2000 under 512 px (MuPDF's encoder goes lossless there), and a fix for ICC-gray images
(upstream silently converts them to RGB while declaring them gray, which garbles the result).
Rust side: `vendor/mupdf/src/pdf/document/optimize.rs`.

## Gaps against Acrobat

- No linearisation ("Fast Web View"): MuPDF removed it.
- No JBIG2 encoder: monochrome images use CCITT G4 (or Flate).
- No font unembedding; subsetting only with `subsetFonts`.
- Soft masks and images with colour-key masks are left as they are; alpha images are not recompressed
  (1-bit stencil masks are, as CCITT G4 image masks).
- Signed documents are rewritten, so signatures become invalid (a warning is reported).
