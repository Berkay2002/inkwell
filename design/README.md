# Inkwell design record

System (from home-r2 and app-assistant-r2): a real Windows 11 desktop app, rounded and smooth (8-12 px corners, soft
shadows), warm palette (window #F4F2ED, surfaces #FFFFFF, panel #FAF8F4, text #1A1C20, secondary #6B675F, borders
#E3DED4), one ink-blue accent #2E4B8F, Segoe UI Variable. No sparkles, purple, gradients or glow.

| Surface | Approved | Gives |
|---|---|---|
| home | approved/home.png | home screen (home-r2 b-gallery) |
| library | approved/library.png, library-2.png | Library view and its selected-file preview (home-r2 a, c) |
| app-assistant | approved/app-assistant.png, -2, -3 | docked (default), floating and command-bar modes |
| organize | approved/organize.png | Organize pages view (organize-r1 a-panel) |
| tools | approved/tools-compress.png, tools-ocr.png | Compress and Recognize-text right rail (tools-r2 b-rail: options, progress) |
| history | approved/history.png | Version history panel (history-r1 a-timeline) |
| history | approved/history-2.png | Compare mode, full window (history-r1 b-compare; modes 2-up / Swipe / Onion skin) |
| compare | approved/compare.png, compare-2.png | Compare files: setup page (old and new file slots, content switches) and result (side by side, summary chips as filters, change list, connectors, linked page strips) (compare-r1 a-stage) |
| redact | approved/redact-review.png, redact-result.png, redact-tools.png | Redact rail: find and hits, after-apply report, manual tools row (redact-r2 c-sweep + a-panel tools) |
| comments | approved/comments.png, comments-2.png | Comments rail (grouped by page, search, filter, replies) with the floating tool strip, and a selected annotation with handles and popover (comments-r1 a-rail) |
| print | approved/print.png | Print sheet: centred modal, preview left, one calm option column right, outcome line beside Print (print-r1 a-sheet) |

2026-10-10, history-r1: the user asked for "the most standard way" of showing versions and diffs (Google Docs / Word
version history, Acrobat Compare Files, GitHub image diff). Diff marks may use green/red/amber (added/removed/changed)
with glyphs, an exception to the one-accent rule that applies only to change marks. Details in rounds/history-r1/spec.md.

2026-10-11, tools-r2: tools-r1 packed four windows per image and could not be judged ("The images are really bad, cant give my
verdict"). r2 re-rendered the same three ideas one window per image; b-rail approved after the user saw how Acrobat does it
(left tool list, PDF Optimizer modal). The rail takes the assistant dock width and chrome; one right-side panel at a time.
Details in rounds/tools-r2/spec.md.
2026-10-11, redact-r1/r2: r1 put two windows on one board and was unreadable; r2 rendered one window per screen. The
user picked, after seeing Acrobat's Redact panel, Comments-pane marks, Search window and _Redacted output: "Ok do your
recommendation" (inkwell's: "c-sweep, plus a-panel's manual area and page tools, docked on the right like b-rail").
Approved: approved/redact-review.png (c-sweep find and hit list), redact-result.png (after-apply report),
redact-tools.png (a-panel, only its manual tools row: mark text/area, mark pages). Built as a right rail like the
assistant and Tools rails; sanitize is inline in the rail, not a modal. Details in rounds/redact-r2/spec.md.

2026-10-11, comments-r1: three ideas, one window per image (a-rail floating tool strip + grouped rail, b-bar toolbar row + chips,
c-margin leader lines). The user delegated the pick ("choose what is both standard and what pdfcraft and acrobat has, but also some
hint of uniqueness"); inkwell chose a-rail. Approved: approved/comments.png, comments-2.png. Same dock as the other rails; one
right-side panel at a time. Details in rounds/comments-r1/spec.md.

2026-10-11, print-r1: three ideas, one window per image (a-sheet centred modal, b-rail right rail with the page area as the
preview, c-ticket compact panel with the outcome first). The user delegated design picks to the orchestrator; berka-43
(orchestrator) chose a-sheet "as built": Acrobat's standard modal with the preview on the left. One thing taken from
c-ticket: the outcome line ("6 sheets of A4, both sides, 2 copies") is the prominent summary beside Print and updates live.
Approved: approved/print.png (a-sheet-1-options). a-sheet-2-booklet skipped (never rendered). Details in
rounds/print-r1/spec.md; build notes in docs/print.md.

2026-10-11, edit-r1: three ideas (a-rail Acrobat's Add content / Format text in our right rail, b-bar floating strip and
format bar, c-proof rail with an outcome card and block list). The image tool was at its usage limit, so no renders were made;
inkwell chose a-rail from the spec, with c-proof's one-line summary and block list. Built as the Edit segment of the
Read | Edit | Organize pill plus the right rail (one rail at a time). Screens: docs/screens/edit/. Details in
rounds/edit-r1/spec.md; build notes in docs/text-edit.md.


2026-10-11, compare-r1: three ideas (a-stage Acrobat-like setup page then result, b-rail pdfcraft-like dialog and one shaded
document, c-tape one sheet with a change tape); only a-stage rendered (image tool limit) and inkwell picked it: "Acrobat's
standard layout: side by side, with the change list on the right". Our touches: dashed connectors between matching changes,
chips that filter, linked page strips. The main navbar stays; the render's own top bar does not. The Compare modes are now
Side by side / Slider / Overlay (history-2.png still shows the old labels). Details in rounds/compare-r1/spec.md.
