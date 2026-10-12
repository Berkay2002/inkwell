# Inkwell patches to vendor/mupdf (mupdf 0.8.0)

Everything else is the upstream crate.

| Where | What |
|---|---|
| `src/pdf/document.rs` | `PdfDocument::permissions` reads the P value of encrypted files (`from_bits_truncate`) |
| `src/pdf/document/optimize.rs` | Image rewriting with progress and cancel, font subsetting and object streams (backed by `vendor/mupdf-sys/wrapper/compress.c`) |
| `src/cookie.rs` | `Cookie::abort_handle` and `AbortHandle`: abort a page run from another thread (one plain write of the cookie's abort flag) |
| `src/pdf/object.rs` | `PdfObject::addr`: the object's address, equal for two references that resolve to one object (the links audit keys a shared `/Annots` on it) |
| `src/pdf/object.rs` | `PdfObject::read_stream_head`: the first N bytes of a decoded stream, the rest never decoded |
| `src/pdf/object.rs` | `PdfObject::string_len`: a string object's length in bytes, read without copying it (over `mupdf_pdf_to_bytes`) |
| `src/pdf/widget.rs` | `PdfWidget::request_resynthesis`: marks a field's appearance stale after its dictionary was edited directly |
| `src/font.rs` | `Font::base14_program`: the CFF program of a built-in base-14 font by its standard name |
| `src/buffer.rs` | `Buffer::ccitt_g4`: 1-bit rows as CCITT Group 4 data |
| `src/device/native.rs` | The `max_align_t` assertion is a literal 16 (bindgen does not emit `max_align_t` on MSVC) |
| `../mupdf-sys/mupdf/source/pdf/pdf-appearance.c` | `pdf_update_appearance` returns early for every signature widget (it did for signed ones only): an unsigned one shows its own /AP or nothing, never the synthesised red "SIGN" arrow, and nothing is written back |
