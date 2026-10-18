# Inkwell and Adobe Acrobat Pro

Inkwell covers every tool area of Adobe Acrobat Pro, runs locally with no account, and is not a clone of it. This page lists what is there and the known gaps, honestly. Every gap was checked against the code on main on 2026-10-19.

| Acrobat Pro tool area | Inkwell | Known gaps |
|---|---|---|
| Viewer | Yes | None known |
| [Find](search.md) | Yes | Replace works on page text only, while Edit text is on (not comments, form fields or bookmarks); 5,000 hits per search |
| [Full-text index (catalogues)](search.md) | Yes | A certified or signed file gets no embedded index |
| Organize pages | Yes | A source's signature field arrives empty when pages are extracted, split, merged or inserted |
| [Combine files](organize-plus.md) | Yes | A source's password and restrictions are not carried over; a source that forbids copying is left out; a source's signature field arrives empty |
| [Page labels, page transitions](organize-plus.md) | Yes | Transitions play in full screen only, not in the sheet, and approximate the PDF's effects |
| [Attachments](organize-plus.md) | Yes | None known |
| [PDF Portfolios](portfolios.md) | Yes | Details and Tiles layouts only (a custom layout shows as Details); Extract all writes a flat folder; Create makes no folders |
| [Bookmarks and links](bookmarks-links.md) | Yes | A named destination is not renamed or removed when a script may use its name; JavaScript and named actions are kept but not editable |
| [Comments](comments.md) | Yes | XFDF and FDF import and export leave out sound, redaction marks and measurements |
| [Forms: fill](forms.md) | Yes | Some form JavaScript does not run (timers, `Net`, page and document event scripts, button mouse-up); XFA (XDP) data files are not read |
| [Forms: author](forms.md) | Yes | Scripts that call a field by an old name do not follow a rename or copy |
| [Document JavaScript](forms.md) | Yes | An edited document script takes effect when the file is reopened |
| [Fill and sign](sign.md) | Yes | Saved signatures stay on the machine that made them |
| [Digital signatures](sign.md) | Yes | No RFC 3161 timestamps; no CRL or OCSP revocation check; no P-521, Brainpool or Ed25519 keys |
| [Edit text](text-edit.md) | Yes | No complex shaping: right-to-left text (Arabic, Hebrew), ligatures and Indic or Thai scripts are refused; text inside form XObjects is not editable |
| [Edit images](image-edit.md) | Yes | Pictures in annotation appearances, Type3 glyphs, patterns and soft masks are not listed; inline pictures can be moved to another page but not copied |
| [Page marks (header and footer, watermark, background, Bates)](marks.md) | Yes | Marks written by other apps can be removed but not updated; no clipboard picture source |
| [OCR](ocr-engine.md) | Yes | Editable-text output uses a sans-serif face only; text over a photo or gradient stays as image; right-to-left and CJK are experimental, vertical text is not handled; Tesseract needs to be installed separately |
| [Compress and optimize](compress-engine.md) | Yes | No linearisation (Fast Web View), no JBIG2 encoder; compressing a signed file breaks the signature; a certified file is refused |
| Redact and sanitize | Yes | Text in a partly covered pattern or soft mask is refused, not cut; terms under 5 characters are not searched inside streams; a signed file is redacted to a new file and cannot be sanitized in place |
| [Protect and encrypt](protect.md) | Yes | No certificate security, no encrypt-attachments-only, no encrypt-all-except-metadata; RC4 files are read, not written |
| [Export](convert.md) | Yes | Word and RTF are a flowing reconstruction, not layout-exact (no underline, alignment or lists); no Excel or PowerPoint export |
| [Create PDF](convert.md) | Yes | Only images, text files, blank pages and the clipboard; no HTML, Markdown, scanner, web page or Office source |
| [Compare](compare.md) | Yes | No compare of image-only scans (run OCR first); metadata and attachments are compared for the whole file |
| [Version history](history.md) | Yes | None known |
| [Print](print.md) | Yes | Output to a printer is raster; no sheet range, driver Properties or ink saving |
| [Print production](prepress.md) | Yes | No per-plate output, trapping or ink-limit conversion; overprint preview is a preview, not a proof; Output Preview shows one page at a time |
| [Preflight, PDF/A, PDF/UA, PDF/X](pdfa.md) | Yes | Not a veraPDF validation; profiles are PDF/A-1b, 2b, 3b, 2u, PDF/UA-1 and a basic PDF/X-4 (no PDF/A "a" levels, PDF/A-4 or custom profiles) |
| [Accessibility](a11y.md) | Yes | Autotag handles regular tables, flat contents pages and figure captions only; orphan content cannot be attached to a region; "Failed manually" and the flicker, script and timing checks are not offered |
| [Measure](measure.md) | Yes | A file certified against changes cannot be measured; scale entries not yet tested against files made by Acrobat |
| [Action Wizard and batch](batch.md) | Yes | No scheduling or watch folders; text watermarks only; at most 500 files per folder; password-protected sources are skipped |
| [Article threads](threads.md) | Yes | Extract, split and combine drop article threads; a bead cannot be dragged to another page |
| [Rich media and 3D](media.md) | Yes | Embedded video and sound play (MP4, WebM, MP3, AAC, WAV, Ogg) but are never edited; no Flash, 3D or linked external media |
| [AI assistant](ai-actions.md) | Yes, with your own subscription | Runs the Codex or Claude Code CLI you already have installed and signed in; no API key, no local models; cannot sign, set passwords or print |
| Cloud features (send for signature, shared review, Document Cloud) | Not offered: Inkwell is local only | Not applicable |

Inkwell runs on Windows only for now.

## The assistant uses your own AI subscription

Acrobat's AI Assistant is a paid add-on that runs in Adobe's cloud. Inkwell's assistant instead drives the Codex
(ChatGPT) or Claude Code CLI already installed and signed in on your PC, so it runs on the ChatGPT or Claude plan you
already pay for. No API key, and Inkwell never reads, copies or stores your login. You pick the provider per document,
and nothing is sent until you agree to it for that provider.

It reads, searches, cites and summarises the document, and it can use Inkwell's own tools: redact, comment, fill forms,
organise pages, add page marks, edit text, run accessibility and preflight checks, compress, OCR, export and compare.
Every edit appears as a card you apply or reject, and each is one undo step. Three modes set how much needs your
approval: ask for every edit, ask only for destructive or permanent changes, or ask only for permanent ones. The assistant never
saves the file; you do. Details: [ai-actions](ai-actions.md), [ai-claude](ai-claude.md).

Adobe and Acrobat are trademarks of Adobe Inc. Inkwell is not affiliated with or endorsed by Adobe.
