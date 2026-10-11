# Inkwell vs pdfcraft: Acrobat parity (2026-10-11)

Sources: pdfcraft ROADMAP.md (area breadth/ready %), acrobat-features.toml ids, README crate table; Inkwell files cited inline. Read-only: nothing cloned or run.

| Area | pdfcraft (ROADMAP breadth/ready; TOML) | Inkwell (file evidence) | Gap |
|---|---|---|---|
| Viewer | B 62.5/55; pages drawn by vendored hayro | Viewer.tsx, TileCanvas.tsx; search, outline, page_text commands (lib.rs); dark pages: toolbar toggle, Off/On/Follow app theme, photos stay natural (docs/page-dark.md) | Renderer decision |
| Find | view.find shipped; advanced search partial | search, cancel_search commands; no options UI found | Find options, results panel |
| Organize | D 61.2/70; organize.* shipped | Organize.tsx; extract_pages, split_document; graft keeps links and widgets (docs/insert-links-widgets.md); page labels, page transitions and full screen (docs/organize-plus.md) | Portfolios |
| Combine/split | organize.combine-files, split-* shipped; combine-options partial | Combine sheet (per-file ranges, reorder, bookmark per file, outlines kept, contents page, form fields renamed on clash, locked files left out); attachments panel; split_document command (docs/organize-plus.md) | Per-file password prompt |
| Comments | E 77.7/70; every markup type, replies, stamps | No annotation creation found; annotations read-only in diff/snap.rs | Whole area |
| Forms fill | F 78.5/65; every AcroForm type, JavaScript (boa) | Widgets copied on graft (engine/edit.rs:621); no set-value or appearance code found | Fill, JS, appearances |
| Forms author | F; form.add-signature-field shipped | None found | Whole area |
| Fill & sign | F includes Fill & Sign | None found | Whole area |
| Digital signatures | H 62.2/45; PAdES B-B, certify, cert store | None found (grep pades/pkcs/pdf_sign: no hits) | Whole area |
| Edit text/images | C 53.3/30; edit paragraphs, add text and images | edit_apply, edit_undo (engine/edit.rs); no text-layout editing (study README row 2) | Reflow, fonts, CJK |
| OCR | I 23.1/15; searchable image, Latin only | ocr/ module; ocr_start, ocr_languages, ocr_plan, ocr_preview; invisible layer, cleaned-up image, editable text; any installed Windows/Tesseract language, auto-detect | Done: languages, output styles (see docs/ocr-engine.md) |
| Compress | K 34.5/20; Optimizer shipped | compress/; compress_audit, compress_start; Smallest/Balanced/High presets | Preflight, PDF/A |
| Redact/sanitize | G 58.5/55; glyphs, pixels, vectors, sanitize shipped | redact/, sanitize.rs; set_redaction_hook; redact icon only, no UI label or command found | UI exposure |
| Protect/encrypt | G; password-to-open, encryption level shipped | authenticate command opens protected PDFs; history/crypto.rs encrypts version store only | Output password and permissions |
| Export/convert | J 39.7/25; PNG, JPEG, TIFF, text, RTF shipped | Save and Save as only; no export code found | Whole area, Office, PDF/A |
| Create PDF | J; from images, text, blank shipped | insert-blank command only | From image, text, clipboard |
| Compare | N; compare text, visual, report shipped | diff/ module; history/Compare.tsx compares versions, not two files | Document-to-document UI |
| Print | L 39.7/35; no Windows printing (#756) | No print code found | Whole area |
| Accessibility | M 63.1/30; checker, alt text, title | None; sanitize.rs only strips alt text | Whole area |
| Command palette | view.command-palette, misc.command-registry shipped | Overlays.tsx:15 CommandPalette; commands.ts registry | Command set parity |
| AI/agent | N 30.0/35; MCP, 141 tools; AI providers planned | ai/ (Codex app-server, chat, tools.rs); assistant panel | Local providers |

## Crate fit (deps from Cargo.toml, scope from lib.rs headers)

| Crate | What it does | Deps that matter | Verdict |
|---|---|---|---|
| sign | PAdES B-B sign and validate signature fields, incremental save | cos, annot, fonts; RustCrypto P-256/384/521, Brainpool, rsa 0.10 rc, aws-lc-rs | needs cos (heavier) |
| forms | AcroForm field tree, fill, regenerated appearances | cos, fonts, content, model | needs cos (heavier) |
| annot | Comment builders, appearance streams, reply/move/delete | cos, fonts, model | needs cos (heavier) |
| crypt | Standard security handler R2-R6: authenticate, encrypt, decrypt | aes, md-5, sha2, stringprep only; lib.rs says standalone (L0) | standalone on bytes |
| redact | Removes marked glyphs, images, paths, annots, form fields | cos, content, fonts, model, forms, edit, annot | not separable |
| automation | Headless tool table, CLI run, MCP server | engine, render, organize, platform, serde_json, png | not separable |
| ocr | ocrs/rten recognition on caller pixels; text_layer emits invisible text | fonts, ocrs 0.13.1, rten 0.26; no cos | standalone on bytes (pixels in); you splice text into the page |

Caveats: "None found" rows are from grep over src/ and src-tauri/src; Chrome.tsx and Home.tsx matched a generic comment grep and were not read.
pdfcraft figures are its own self-assessment (ROADMAP: about 49% ready for full Acrobat Pro, stage alpha; 141 of 423 shipped features cite one test).
Redact and OCR overlap Inkwell most; sign and forms would need the cos object layer, which is heavier than MuPDF for this app.
Crate verdicts come from Cargo.toml and lib.rs headers only; no build or test was run.
