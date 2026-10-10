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
| redact | approved/redact-review.png, redact-result.png, redact-tools.png | Redact rail: find and hits, after-apply report, manual tools row (redact-r2 c-sweep + a-panel tools) |

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
