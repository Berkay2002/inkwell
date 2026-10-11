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
(ppi outside 36..=2400 or target above threshold, JPEG/JPX quality outside 1..=100, garbage above 4, flatten ppi
outside 36..=600, font names that are too many or too long); the rest are messages.
`Options`, `Report`, `Audit`, `Progress` are serde (camelCase) and ready for the IPC layer.

## Presets

| | colour/gray: above -> to | codec, quality | mono: above -> to | fonts | other |
|---|---|---|---|---|---|
| Smallest | 108 -> 72 ppi | JPEG q18 | 300 -> 200 ppi, CCITT G4 | subset | thumbnails dropped |
| Balanced | 225 -> 150 ppi | JPEG 2000 q75 (JPEG under 512 px) | 450 -> 300 ppi, CCITT G4 | keep | |
| High quality | 300 -> 225 ppi | JPEG 2000 q85 | 600 -> 450 ppi, CCITT G4 | keep | |

All presets: skip images that would grow, object streams, garbage level 4 (dedupe), Flate everywhere.
Metadata, embedded files, JavaScript, bookmarks are only removed when `strip.*` is set (off by default).

## Advanced settings (Acrobat's Optimized PDF)

The three presets are `Options` values; Advanced is the same `Options`, edited by hand. `CompressRequest.custom`
wins over the preset name (the UI sends the whole set; output name and password come from the command, never the
JSON). Defaults of every key are the Balanced preset, so an `Options` saved by an older build still loads.

| Acrobat panel | `Options` | What it does |
|---|---|---|
| Images: downsample, compress, colour / gray / mono | `images.color/gray/mono {abovePpi, toPpi, codec, quality}`, `images.onlyIfSmaller` | the existing image rewriter, per class |
| Fonts: unembed | `unembedFonts[]` (by `/BaseFont`) or `unembedAll` | removes `FontFile*`; simple, non-symbolic fonts only |
| Fonts: subset | `subsetFonts` | glyph subsetting |
| Transparency: flatten | `transparency {flatten, ppi 36..=600}` | pages that use transparency are drawn once and replaced by the picture |
| Discard: JavaScript | `strip.javascript` | JavaScript actions only (a reset action next to a script in `/AA` stays) |
| Discard: thumbnails, bookmarks, attachments, metadata | `strip.thumbnails / bookmarks / embeddedFiles / metadata` | as before; metadata also takes the Info dictionary and `/PieceInfo` of catalog and pages |
| Discard: form actions | `strip.formActions` | SubmitForm, ResetForm, ImportData in `/A` and `/AA` (fields and values stay) |
| Discard: alternate images | `strip.alternateImages` | `/Alternates` of image streams |
| Discard: private data | `strip.privateData` | `/PieceInfo` of the catalog, pages and form XObjects |
| Discard: all links / invalid links | `strip.links`, `strip.invalidLinks` | every Link annotation / those with no action, a page that does not exist, or an unknown named destination (URIs and other actions count as working) |
| Clean up: object streams, Flate, unused objects | `objectStreams`, `compressStreams`, `garbage` 0..=4 | the writer's options |

Not in MuPDF, so not here: linearization, JBIG2, invalid-bookmark removal (bookmarks go as a whole), object-level
transparency flattening (a flattened page becomes a picture: its text is no longer text; the report lists the page
numbers, rotated pages and pages with `/UserUnit` are left alone and listed as skipped).

Unembedding keeps the font's description and widths; the subset tag (`ABCDEF+`) is dropped from the name. It is
refused, with the reason in `unembedded.skipped` and a warning, for composite (Type0) fonts, symbolic fonts and
fonts whose flags are missing, because a viewer cannot substitute those faithfully.

### Space usage

`Audit.categories` (bytes, objects, percent) cover the file exactly: stream bytes are measured, other objects are
scaled to what remains, and the remainder is "Document structure". Categories: images, fonts, page content,
form fields, comments, links, bookmarks, page thumbnails, attachments, metadata, document structure, other.
`Audit.fontList` lists each font with its embedded size, subset flag, and whether it can be unembedded and why not.
`Report.before` / `Report.after` are the audits of the input and of the copy, so the table of what each option
took out is the difference between them.

### Estimate

`estimate_custom` recompresses the same ~2.5 Mpx sample the presets use under the given settings; discarded
categories and unembedded font files are subtracted from the full audit, a flatten adds a 0.15-0.5 bytes per pixel
range per page. The result is a range (`low..high`), not a promise. The never-worse rule and the visual check run
for Advanced settings exactly as for presets.

### Named presets

`compress::presets` keeps them in `<data dir>/compress-presets.json` (`{version, presets: [{name, options}]}`):
names trimmed, at most 60 characters, case-insensitive replace, at most 50, written through a temp file and a
rename. Password, output name and the per-document font names are never stored. A damaged file is an empty list; an
entry that no longer validates is skipped, not fatal. Commands: `compress_preset_options`, `compress_estimate_custom`,
`compress_presets`, `compress_preset_save`, `compress_preset_delete`.

## Inkwell vs Acrobat (same input `opt-in-01.pdf`, 2,657,232 B, 2 pages; same render diff at 96 ppi)

| Acrobat run | bytes | max | mean | PSNR | Inkwell | bytes | max | mean | PSNR | vs Acrobat |
|---|---|---|---|---|---|---|---|---|---|---|
| 04: 72 ppi above 108, JPEG minimum | 31,839 | 41 | 1.925 | 36.2 | Smallest | 30,929 | 34 | 1.877 | 36.4 | -3% |
| 01: Single file (JPEG 2000 medium) | 448,574 | 17 | 0.775 | 44.0 | Balanced | 246,447 | 14 | 0.720 | 44.6 | -45% |
| 06: JPEG 2000 maximum, 150 ppi | 866,368 | 15 | 0.693 | 44.9 | High quality | 612,113 | 13 | 0.537 | 47.0 | -29% |

Reproduce: `cargo test --lib compress::tests::presets -- --nocapture --test-threads=1`.

## Behaviour guarantees (all covered by tests)

- Never worse: when the result is not smaller the original bytes are copied to the output and `keptOriginal` is true.
  Exception: if a `strip.*` option removed something, a font was unembedded or a page flattened, the copy is
  written even when bigger (warning "larger than the original because objects were removed or flattened").
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
- Font unembedding is limited to simple, non-symbolic fonts (see Advanced settings); subsetting only with `subsetFonts`.
- Soft masks and images with colour-key masks are left as they are; alpha images are not recompressed
  (1-bit stencil masks are, as CCITT G4 image masks).
- Signed documents are rewritten, so signatures become invalid (a warning is reported).
