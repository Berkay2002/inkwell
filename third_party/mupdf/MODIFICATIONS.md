# Inkwell's modifications to MuPDF and its Rust bindings

Inkwell builds on MuPDF 1.27.2 (Copyright (C) 2004-2025 Artifex Software, Inc., AGPL-3.0-or-later) through the Rust
crates mupdf 0.8.0 and mupdf-sys 0.8.0 (https://github.com/messense/mupdf-rs, AGPL-3.0). Both crates are vendored in
`vendor/mupdf` and `vendor/mupdf-sys`, and Inkwell modified the files below between 2026-10-10 and 2026-10-19.
This list is the notice of modification that AGPL-3.0 section 5(a) asks for. Every file not listed is unchanged from
the published crates, and git history (`git log -- vendor/`) has the exact change and date of each edit.

## mupdf-sys 0.8.0 (`vendor/mupdf-sys`), including MuPDF's C sources

| File | Change |
|---|---|
| `build.rs` | Reruns when `wrapper/` or the MuPDF sources change. On MSVC, MuJS's `js_malloc`, `js_realloc`, `js_free` and `js_strdup` get a `mujs_` prefix so they do not collide with QuickJS |
| `mupdf/source/fitz/load-jpx.c` | The OpenJPEG allocator context is thread-local, and JPEG 2000 decoding no longer takes the FreeType lock, so pages decode in parallel |
| `mupdf/source/fitz/time.c`, `mupdf/source/fitz/stream-open.c` | On Windows, files are opened with a non-inheritable handle, so a process Inkwell starts does not keep an open PDF locked |
| `mupdf/source/pdf/pdf-appearance.c` | `pdf_update_appearance` never synthesises an appearance for a signature widget, signed or not |
| `mupdf/source/pdf/pdf-shade-recolor.c` | `pdf_recolor_shade` returns a new reference when it gives back the unchanged shading |
| `wrapper/internal.h` | Adds the `mupdf_error_stream_cap` error code |
| `wrapper/buffer.c` | Adds `mupdf_buffer_ccitt_g4` (CCITT Group 4 encoding of 1-bit rows) |
| `wrapper/pdf_annot.c` | Adds `mupdf_pdf_set_annot_stamp_image_from_buffer` |
| `wrapper/pdf_object.c` | Adds `mupdf_pdf_read_stream_capped` and `mupdf_pdf_read_stream_head` |
| `wrapper/pdf_page.c` | Adds `mupdf_pdf_page_bounds` (page size from the page dictionary alone) |
| `wrapper/compress.c` | New file: image rewriting, font subsetting and object streams for the optimiser |
| `wrapper/prepress.c` | New file: separations, separation-aware pixmaps, ICC colour spaces and colour conversion |

`mupdf/platform/win32/*.sln` are upstream files that Inkwell's git tracks so the release build can find them. They are
unchanged.

## mupdf 0.8.0 (`vendor/mupdf`)

| File | Change |
|---|---|
| `src/buffer.rs` | Adds `Buffer::ccitt_g4` |
| `src/colorspace.rs` | Adds `is_icc`, `is_separation` and `colorant` |
| `src/cookie.rs` | Adds `Cookie::abort_handle`, `abort_shared` and `AbortHandle` (abort a page run from another thread) |
| `src/device/native.rs` | The `max_align_t` assertion is a literal 16 (bindgen does not emit `max_align_t` on MSVC) |
| `src/document.rs` | Adds `Document::from_shared_bytes` and `pdf_page_bounds` |
| `src/error.rs` | Adds `Error::is_stream_cap` |
| `src/font.rs` | Adds `Font::base14_program` |
| `src/lib.rs` | Exports the new `prepress` module and the separation types |
| `src/page.rs` | Adds `Page::uses_overprint` |
| `src/pdf/annotation.rs` | Adds `set_stamp_image` |
| `src/pdf/document.rs` | `permissions` reads the P value of encrypted files; adds `wipe_passwords` and `authenticate_access` |
| `src/pdf/document/optimize.rs` | New file: image rewriting with progress and cancel, font subsetting and object streams |
| `src/pdf/object.rs` | Adds `share`, `addr`, `string_len`, `read_stream_capped` and `read_stream_head` |
| `src/pdf/widget.rs` | Adds `PdfWidget::request_resynthesis` |
| `src/prepress.rs` | New file: the Rust side of `wrapper/prepress.c` |
| `src/separations.rs` | Adds `SeparationBehavior`, `Separations::new` and `Separations::add` |
| `PATCHES.md` | New file: a shorter developer list of the same patches |
