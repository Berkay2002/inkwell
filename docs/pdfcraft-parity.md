# Inkwell vs pdfcraft: Acrobat parity (2026-10-11)

Sources: pdfcraft ROADMAP.md (area breadth/ready %), parity/acrobat-features.toml ids and status, README crate table, all read at b8ddf71 (read-only, nothing built or run). Inkwell side: main at ca86fc8, from the code paths and docs cited in each row. pdfcraft's figures are its own self-assessment and have not moved since the last version of this file.

Status key for the Inkwell column ("ahead" means pdfcraft has the matching TOML ids planned):
- **Shipped**: engine and UI are on main (a Tauri command is called from `src/`).
- **Engine only**: commands and tests exist, no UI calls them yet.
- **Partial**: some of the area is shipped, the gap column says what is not.
- **Not started**: no code found (grep over `src/` and `src-tauri/src/`).

## In flight (open PRs, not counted as shipped below)

| PR | What | Area it will change |
|---|---|---|
| #15 | Measuring tools UI: bar, drawing, rail, calibrate, CSV | Measure goes from engine only to shipped |
| #16 | Action Wizard UI: actions, editor, run with preview | Action Wizard goes from engine only to shipped |
| #17 | Article threads: read, follow, create and edit | New row (pdfcraft has `view.articles-panel` and `edit.action-thread` planned) |

No open PR yet for catalog, rich media, or the JavaScript events that do not run today (OpenAction, page and document events, button mouse-up); document-level scripts already run at open.

## Parity table

| Area | pdfcraft (ROADMAP breadth/ready; TOML) | Inkwell status | Inkwell evidence | Gap |
|---|---|---|---|---|
| Viewer | B 62.5/55; pages drawn by vendored hayro | Shipped | `src/components/Viewer.tsx`, `TileCanvas.tsx` (512 px MuPDF tiles over the `inkwell://` protocol), `Presentation.tsx` (full screen), `Home.tsx`; dark pages with natural photos (`docs/page-dark.md`) | Renderer differs (MuPDF here, hayro there); nothing else known |
| Find | `view.find`, `find-options`, `find-results-panel` shipped; `find-in-comments-bookmarks` planned | Shipped | `src-tauri/src/search/` (mod.rs, files.rs), `src/components/search/`, `docs/search.md`: case, whole word, ignore accents, comments, bookmarks, form fields, results rail, search across files | No replace (pdfcraft `edit.find-replace` is planned too); query 256 characters, 5,000 hits per search |
| Organize pages | D 61.2/70; `organize.*` mostly shipped | Shipped | `src/components/Organize.tsx`, `PageMenu.tsx`; `extract_pages`, `split_document`, `merge_files` (`src-tauri/src/lib.rs`); links and widgets kept on insert (`docs/insert-links-widgets.md`) | None known |
| Combine files | `organize.combine-files` shipped, `combine-options` partial | Shipped | `src-tauri/src/organize/combine.rs`, `src/components/CombineSheet.tsx`, `docs/organize-plus.md`: per-file ranges, reorder, bookmark per file, contents page, form fields renamed on clash | No per-file password prompt (a locked file is left out); source page labels are not carried over |
| Page labels, page transitions | `organize.page-labels-edit` shipped; `organize.page-transitions` planned | Shipped | `src-tauri/src/organize/labels.rs`, `organize/trans.rs`, `src/components/OrganizePlus.tsx`; transitions play in `Presentation.tsx`, `src/lib/present.ts` | None known; page labels are at parity, transitions are ahead of pdfcraft |
| Attachments | `view.attachments-panel` shipped; `organize.attachments-add`, `-delete`, `-description` planned | Shipped (ahead) | `src-tauri/src/organize/attach.rs`, `src/components/AttachmentsPanel.tsx`: list, add, save, open, remove, describe | None known; encrypt-attachments-only is not built (see Protect and encrypt) |
| PDF Portfolios | `organize.portfolio-view`, `-extract`, `-create` planned | Shipped (ahead) | `src-tauri/src/organize/portfolio/` (mod.rs, make.rs), `src/components/PortfolioView.tsx`, `docs/portfolios.md`: schema columns, sort, folders, Extract, Extract all, Create PDF Portfolio | Details layout only (no tiles, no preview pane); Extract all writes flat; folder prefix not checked against an Acrobat file |
| Bookmarks and links | `organize.bookmark-*`, `edit.link-create`, `edit.link-properties`, `edit.links-from-urls` shipped; `edit.remove-all-links` planned | Shipped | `src-tauri/src/outline/`, `src-tauri/src/links/`, `src/components/edit/` (BookmarksPanel, LinksPanel, LinkLayer), `ReadLinks.tsx`, `docs/bookmarks-links.md`; links open only after a confirm, through the UI's gate | Named destinations and JavaScript actions are listed as "other", not editable; no link highlight style; direct-object link annotations are not listed |
| Comments | E 77.7/70; every markup type, replies, stamps | Shipped | `src-tauri/src/annot/`, `src/components/comments/`, `src/lib/annot.ts`, `docs/comments.md`: highlight, underline, strikeout, squiggly, note, text box, pen, rectangle, ellipse, line, arrow, polygon, stamp, replies, resolved state | XFDF import and export not built (pdfcraft `comment.export-fdf-xfdf` shipped); no stamp management found |
| Forms fill | F 78.5/65; every AcroForm type, JavaScript (boa) | Shipped | `src-tauri/src/forms/` (mod.rs, af.rs, scripts.rs, js.rs, js/), `src/components/forms/`, `docs/forms.md`, `docs/forms-js.md`: every field type, regenerated appearances, calculation order, scripts in a sandboxed QuickJS runtime with time, heap and stack caps, signature locks honoured | FDF, XFDF and JSON field data not built (pdfcraft `form.export-data` shipped); not run: timers, `app.execMenuItem`, `Net`, button mouse-up, page and document event scripts |
| Forms author | F; `form.add-signature-field` shipped | Shipped | `src-tauri/src/forms/author.rs`, `detect.rs`, `src/components/forms/FormsPanel.tsx`: create, move, resize, properties, tab order, auto-detect fields on flat forms | No align, centre, match size or distribute tools; no in-place rename in the field list |
| Fill and sign | `form.fill-sign-text`, `-marks`, `-date`, `-signature`, `-flatten` shipped | Not started | No Fill and Sign tool. Text boxes, pen and stamps exist in Comments (`src/components/comments/`), and `src/components/forms/FormLayer.tsx` shows signature fields with "signing is not available here" | Whole tool: place text, marks and date, signature image, flatten |
| Digital signatures | H 62.2/45; PAdES B-B, certify, cert store | Not started | No signing or validation code. Signature fields can be authored (`forms/author.rs`) and are shown; typing into a field a signature locks is refused (`forms/mod.rs`); a restored version is byte-exact so signatures survive (`docs/history.md`) | Whole area: sign, validate, certify, certificate store, timestamps |
| Edit text | C 53.3/30; `edit.text-edit` partial, `edit.text-reflow`, `-move-resize`, `-format-font` shipped | Shipped | `src-tauri/src/textedit/`, `src/components/edit/TextEditLayer.tsx`, `EditPanel.tsx`, `docs/text-edit.md`: edit in place, add text, per-run formatting, reflow, bundled Noto fallback fonts | No complex shaping (right-to-left, ligatures); Type0 fonts with predefined non-Identity CMaps and text in form XObjects not editable; no underline, strikeout or super/subscript per run |
| Edit images | `edit.image-add`, `-replace`, `-rotate-flip`, `-move-resize`, `-extract` shipped; `-crop` partial | Engine only | `src-tauri/src/imageedit/`, `docs/image-edit.md`; `src/components/edit/EditPanel.tsx` shows a disabled "Image ... soon" button. `image_blocks` is called only for dark-page photos (`src/lib/api.ts`) | The UI; images inside form XObjects are not listed |
| Page marks | `edit.header-footer-*`, `edit.watermark-*`, `edit.background-*` shipped; `edit.bates-numbering` partial, `-multi-file` planned | Shipped | `src-tauri/src/marks/`, `src/components/MarksPanel.tsx`, `MarkPreview.tsx`, `docs/marks.md`: header and footer with tokens, watermark, background, Bates numbering (continues across files in Action Wizard steps) | No shrink-to-avoid-overlap; marks Acrobat made are not recognised by Update and Remove; no clipboard picture source |
| OCR | I 23.1/15; searchable image, Latin only | Shipped (Windows OCR default) | `src-tauri/src/ocr/` (backend.rs, layer.rs, glyphless.rs), `ocr_start`, the Recognize text sheet in `src/components/ToolsPanel.tsx`, `docs/ocr-engine.md`; Tesseract behind the same trait | Windows OCR reads one language per call; no editable-text output (pdfcraft `ocr.editable-text-output` planned too); Windows.Media.Ocr is not available on macOS |
| Compress | K 34.5/20; Optimizer shipped | Shipped | `src-tauri/src/compress/`, `compress_audit`, `compress_estimate`, `compress_start`, the Compress sheet in `src/components/ToolsPanel.tsx`, `docs/compress-engine.md`: Smallest, Balanced, High quality, Custom; audit of space usage | No linearisation, no JBIG2 encoder, no font unembedding; signed files are rewritten and the signature breaks (warned) |
| Redact and sanitize | G 58.5/55; glyphs, pixels, vectors, sanitize shipped | Shipped | `src-tauri/src/redact/` (mod.rs, patterns.rs, sanitize.rs), `src/components/redact/`, `src/lib/redact.ts`: find, mark, apply, sanitize, verified full rewrite | Text in a partly covered tiling pattern or soft mask is refused, not cut; needles under 5 characters are not searched inside streams |
| Protect and encrypt | G; password-to-open, encryption level shipped | Shipped | `src-tauri/src/protect/`, `src/components/ProtectPanel.tsx`, `docs/protect.md`: open and permissions passwords, AES-256 and AES-128, remove security, restrictions kept on save | No certificate security, no encrypt-attachments-only or all-except-metadata (pdfcraft `protect.encrypt-except-metadata` shipped); RC4 read, not written; restrictions are enforced only by tools that write new content out (extract, split, merge, export) |
| Export | J 39.7/25; PNG, JPEG, TIFF, text, RTF shipped, DOCX partial | Shipped | `src-tauri/src/convert/export.rs`, `docx.rs`, `export_start`, `src/components/ConvertPanel.tsx`, `docs/convert.md`: PNG, JPEG, TIFF, text, HTML, embedded images, Word | No RTF writer; Word is a flowing reconstruction, not layout-exact; no .xlsx, .pptx |
| Create PDF | J; from images, text, blank, clipboard shipped | Shipped | `src-tauri/src/convert/create.rs`, `create_start`, `ConvertPanel.tsx`, `docs/convert.md`: images (multi-page TIFF too), text file, blank pages, clipboard image; result is an unsaved tab | No HTML, Markdown, screenshot or Office source (planned in pdfcraft too) |
| Compare | `misc.compare-text`, `-visual`, `-report` shipped; `-side-by-side`, `-filters`, `-page-ranges`, `-scanned` planned | Shipped | `src-tauri/src/diff/`, `src/components/history/CompareSetup.tsx`, `src/components/history/Compare.tsx`, `history_ipc.rs`: two files or two versions, text, formatting and image changes, filter by type, visual page diff, report export | No page-range or scanned-document compare found |
| Version history | `core.history-panel` planned; `view.view-history` shipped | Shipped (Windows only) | `src-tauri/src/history/` (encrypted, deduplicated store), `src-tauri/src/history_ipc.rs`, `src/components/history/`, `docs/history.md`: every save, assistant turn and outside change is a version; restore is byte-exact | Needs DPAPI and NTFS file ids; the history tests fail on macOS by design |
| Print | L 39.7/35; no Windows printing (#756) | Shipped (Windows only for printers) | `src-tauri/src/print/` (layout.rs, render.rs, win.rs GDI job, pdf.rs), `src/components/PrintSheet.tsx`, `docs/print.md`: pages, copies, collate, scaling, several per sheet, booklet with creep, poster, two-sided, preview drawn by the printer's own renderer | GDI output is raster; duplex flags not tried on hardware; no sheets-from-to, no driver Properties, no Save ink. Save as vector PDF runs on every platform |
| Print production | `print.separations`, `print.overprint-preview`, `print.output-preview`, `print.convert-colors`, `print.print-marks-bleeds` planned | Engine only (ahead) | `src-tauri/src/prepress/`, `docs/prepress.md`: separations and overprint preview, ink coverage and TAC, object inspector, Convert Colors, trim and bleed boxes, printer's marks | The UI; no per-plate output, trapping or ink-limit conversion |
| Preflight, PDF/A, PDF/UA, PDF/X | `optimize.pdfa-validate`, `-convert` partial; `pdfx-*`, `pdfua-validate`, `preflight*` planned | Shipped (ahead) | `src-tauri/src/preflight/`, `src/components/preflight/PreflightPanel.tsx`, `docs/pdfa.md`: PDF/A-1b, 2b, 3b, 2u, PDF/UA-1, PDF/X-4 (basic): check, fix, convert | Not a veraPDF validation; clause numbers unverified; no PDF/A "a" levels or PDF/A-4; no custom profiles found |
| Accessibility | M 63.1/30; checker, alt text, title shipped; `a11y.autotag`, `a11y.reading-order-tool`, `a11y.tags-panel` planned | Partial | Checker, fixes, alt text and Autotag are shipped: `src-tauri/src/a11y/`, `src/components/a11y/A11yPanel.tsx`, `docs/a11y.md`. Reading order and tag editing (`a11y_reading_order`, `a11y_tags`, `a11y_reorder`, `a11y_edit_tag` and others) are engine only: nothing in `src/` calls them | UI for the Reading order tool and Tags panel; Autotag has no tables, TOC or captions; some manual checks not offered |
| Measure | `comment.measure-perimeter`, `-area`, `-snap`, `-info-panel`, `-export` shipped; `-distance`, `-scale` partial | Engine only | `src-tauri/src/measure/`, `docs/measure.md`; UI is #15 | The UI; `/Measure` reading is not checked against a file from Acrobat |
| Action Wizard and batch | `misc.action-wizard-run`, `-create` shipped; `misc.batch-processing` partial | Engine only | `src-tauri/src/batch/`, `docs/batch.md`: compress, OCR, header and footer, watermark, Bates, protect, sanitize, redact, flatten forms, export, set properties; UI is #16 | The UI; no scheduling or watch folders; protected sources are skipped |
| Command palette | `view.command-palette`, `misc.command-registry` shipped | Shipped | `src/components/Overlays.tsx` (CommandPalette), `src/lib/commands.ts` | Command set parity not measured |
| All tools | Acrobat's All tools view: search, groups, favourites | Shipped | `src/components/ToolsPanel.tsx`, `AllTools.tsx`, `src/lib/toolsCatalog.ts`, `docs/tools-panel.md`: synonym search, recent chips, three pinned toolbar buttons, hover cards, Home grid | Per-document recommendations |
| AI assistant | N 30.0/35; MCP, 141 tools; AI providers planned | Shipped | `src-tauri/src/ai/` (codex.rs, chat, tools.rs), `src/components/assistant/`; a permission check in each turn's tool handler (header of `ai/tools.rs`); edits are undo steps with their own `EditSource` | Local providers; the Codex app-server is the only provider found |
| Core model | A 76.8/70 | Shipped | `src-tauri/src/engine/` (edit.rs, edit/security.rs, cache.rs, pool.rs), `docs/engine-rules.md`: one authoritative document per file, every edit one undo step, tokens-only paths | Not compared item by item with `core.*` ids |

## Windows-only code

Code present but not run on macOS: history (DPAPI, NTFS ids), printing to a printer (GDI, `print/win.rs`), Windows.Media.Ocr (Tesseract needs `tesseract.exe`), and the WebView2 shell. These cannot be checked on this machine.

## Crate fit (deps from Cargo.toml, scope from lib.rs headers)

| Crate | What it does | Deps that matter | Verdict | On main |
|---|---|---|---|---|
| sign | PAdES B-B sign and validate signature fields, incremental save | cos, annot, fonts; RustCrypto P-256/384/521, Brainpool, rsa 0.10 rc, aws-lc-rs | needs cos (heavier) | Not used; no signing on main |
| forms | AcroForm field tree, fill, regenerated appearances | cos, fonts, content, model | needs cos (heavier) | Partly adapted: `forms/af.rs`, `forms/detect.rs`, `parse_da` in `forms/mod.rs`; the field tree and appearances stay on MuPDF |
| annot | Comment builders, appearance streams, reply/move/delete | cos, fonts, model | needs cos (heavier) | Not used; `annot/` writes MuPDF annotations |
| crypt | Standard security handler R2-R6: authenticate, encrypt, decrypt | aes, md-5, sha2, stringprep only; lib.rs says standalone (L0) | standalone on bytes | Not needed; `protect/` uses MuPDF's writer (`docs/protect.md`) |
| redact | Removes marked glyphs, images, paths, annots, form fields | cos, content, fonts, model, forms, edit, annot | not separable | Not used; `redact/` is Inkwell's own on MuPDF |
| automation | Headless tool table, CLI run, MCP server | engine, render, organize, platform, serde_json, png | not separable | Not used; `batch/` is Inkwell's own |
| ocr | ocrs/rten recognition on caller pixels; text_layer emits invisible text | fonts, ocrs 0.13.1, rten 0.26; no cos | standalone on bytes (pixels in); you splice text into the page | Not used; backends are Windows.Media.Ocr and Tesseract, text layer in `ocr/glyphless.rs`, `ocr/layer.rs` |

Crates main already adapts (from `third_party/pdfcraft/INKWELL-NOTICE.txt`):

| pdfcraft crate | Inkwell files |
|---|---|
| content, fonts, edit | `textedit/` (content.rs, font.rs, encodings.rs, text.rs, layout.rs), `imageedit/mod.rs`, `marks/` (apply.rs, mod.rs, text.rs) |
| a11y | `a11y/mod.rs`, `a11y/tree.rs` |
| preflight | `preflight/` (mod.rs, check.rs, fix.rs, report.rs, xmp.rs, icc.rs) |
| print | `print/layout.rs`, `print/pdf.rs` |
| export, create | `convert/docx.rs`, `convert/create.rs` |
| render (find_opts) | `search/mod.rs` |

## Caveats

- "Not started" rows come from grep over `src/` and `src-tauri/src/`; "Shipped" means a command is called from `src/`, not that it was exercised in a build.
- Behaviour was read from code and docs, not run; the UI could not be screenshotted on macOS.
- pdfcraft figures are its own self-assessment (ROADMAP: about 49% ready for full Acrobat Pro, stage alpha; 141 of 423 shipped features cite one test). TOML ids and statuses are quoted by name; per-area counts are not republished.
- Crate verdicts come from Cargo.toml and lib.rs headers only; no build or test was run.
