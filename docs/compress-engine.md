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
| Discard: invalid bookmarks | `strip.invalidBookmarks` | outline items whose `/Dest`, or the `/D` of a GoTo action, does not resolve (see below); off in every preset |
| Discard: form actions | `strip.formActions` | SubmitForm, ResetForm, ImportData in `/A` and `/AA` (fields and values stay) |
| Discard: alternate images | `strip.alternateImages` | `/Alternates` of image streams |
| Discard: private data | `strip.privateData` | `/PieceInfo` of the catalog, pages and form XObjects |
| Discard: all links / invalid links | `strip.links`, `strip.invalidLinks` | every Link annotation / those with no action, a page that does not exist, or an unknown named destination (URIs and other actions count as working) |
| Clean up: object streams, Flate, unused objects | `objectStreams`, `compressStreams`, `garbage` 0..=4 | the writer's options |

Not in MuPDF, so not here: linearization, JBIG2, object-level
transparency flattening (a flattened page becomes a picture: its text is no longer text; the report lists the page
numbers, rotated pages and pages with `/UserUnit` are left alone and listed as skipped).

Invalid bookmarks (Acrobat's "Discard invalid bookmarks"): an item is invalid when its `/Dest`, or the `/D` of a
GoTo `/A`, is an array whose page is not a page of this document (a reference to something that is not in the page
tree, or to an object that does not exist, or no page at all), a name or string the catalog's `/Dests` or `/Names`
`/Dests` tree does not define (or defines as something that is not a destination), or anything that is not a
destination. An integer page (the remote form) is kept while it is below the page count. Items with no destination
(headings) and items with another action (URI, GoToR, Launch, Named, JavaScript) are never judged. An invalid item is
removed and **its working children are lifted into its place**, in order, at any depth (Acrobat does the same; the
other choice, removing the whole subtree, would lose working bookmarks). `/First /Last /Prev /Next /Parent /Count`
are rewritten, an item's open or closed state is kept. The walk has the outline editor's limits (50 000 items, 48
levels, each item once): an outline that is larger, or that reaches an item from two places, is left as it is with a
warning (`stripped.bookmarksUnchecked`), as is one when the named destinations cannot be read; neither stops the run.
`stripped.invalidBookmarks` is the count; the space audit's Bookmarks row shrinks by their bytes. With `strip.bookmarks`
also on, the outline goes whole and the count is 0. A certified file is refused by the optimizer as for every option.
The estimate does not predict it (it is a few bytes).

Caveat: when the image pass runs (any image class that downsamples or re-encodes), MuPDF replaces the images and
form XObjects it rewrites by new objects, which carry no `/Alternates` and no `/PieceInfo`. They are gone from such a
copy whether or not the discard option is on; with every image class left as it is the pass does not run and the
options remove exactly what they say. When the pass replaced anything and the file had image `/Alternates` or form
`/PieceInfo` (counted after the discard step), the report carries the warning "Rewritten images lost their alternates
and private data", and the sheet's hints for the two options say so.

Unembedding keeps the font's description and widths; the subset tag (`ABCDEF+`) is dropped from the name. It is
refused, with the reason in `unembedded.skipped` and a warning, for composite (Type0) fonts, symbolic fonts and
fonts whose flags are missing, because a viewer cannot substitute those faithfully. The encoding has to be one a
stand-in can follow: StandardEncoding, WinAnsiEncoding or MacRomanEncoding, alone or as the base of a `/Differences`
list that names only Adobe Latin glyphs. A list naming `g12`, `glyph34` and the like, any other base encoding, and a
Type 1 font with no `/Encoding` stay embedded (a TrueType font with no `/Encoding` is read as Standard). The text of a
font that goes is drawn in a similar font: the sheet says so.

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
- The output is reopened, its page count checked, re-audited and render-compared (`visualPages` evenly spread pages,
  at most 64, 0 skips the check; 96 ppi). Pages the run flattened and pages set in an unembedded font are always
  added (at most 24 of them, spread over the list): `visual.maxDiff`, `meanDiff`, `psnr`, `p999Diff` (the difference
  99.9 % of the samples stay under), `worstPage`, per page. A flattened page is drawn on both sides at the
  resolution it was flattened at, so resampling alone is not a change. The result screen says "Looks the same" up to a
  largest difference of 8 of 255, "Slightly different" above that, and "Looks different, check page N" (red) when
  `p999Diff` is above 64 or the PSNR is under 30. Stray pixels (an edge after a lossy pass) do not trip it; a changed
  line of text does. `worstPage` is the page furthest past either threshold.
- A named-destination tree that cannot be read skips the invalid-links and invalid-bookmarks passes (warning), it does
  not make every named link or bookmark look broken. A signed document is flagged in the audit reply (`signed`) so the sheet warns before Start.
- The presets file is read up to 1 MB; an out-of-range `visualPages` in it is clamped to 64.
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
