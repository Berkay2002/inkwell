# Organize-plus r1: the combine sheet and the attachments panel

Second round for Organize (organize-r1 approved the grid). It keeps the Inkwell system from `design/README.md`. No render
was made (spec only, as for edit-r1 and compare-r1); the build is the most standard variant, a-sheet.

## How Acrobat Pro does it (docs/study/03-organize.md, 09-viewer.md)
- **Combine files**: Tools > Combine files opens a full-page workspace (thumbnail tiles per file, Add files, a right-hand
  "Combine files" options column: file size Small / Medium / Large, and a Combine button). Per-file page ranges come from
  expanding a tile. Bookmarks per source file are added automatically; there is no table-of-contents page.
- **Attachments**: a left-rail Attachments panel (paperclip) lists embedded files with name, description and size; its
  toolbar has Add, Open, Save, Delete and Settings. Opening a file asks for confirmation (it may be dangerous).
- **Page labels**: Organize > More > Page labels: modal with Pages (All / Selected / From-To), Begin new section or
  Extend, Style, Prefix, Start, and a sample line.
- **Page transitions**: Organize > More > Page transitions: modal with Transition, Direction, Speed, Auto flip after n s,
  and a range (all / selected / from-to). Ctrl+L plays them full screen.

## How pdfcraft does it
- Combine: one window, file list with drag handle, per-file range text ("1-3, 6"), password per file, bookmarks per
  source with the source's outline nested and collapsed, form fields kept. No table of contents page.
- Attachments: only as comment pins; no list panel, no delete, no description.
- Page labels: one dialog with From/To, Style, Prefix, Start and a live preview. No transitions at all.

## Variants
- **a-sheet** (built): one centred sheet like Print. Left: the file list (name, pages "of 12", range field, drag to
  reorder, remove). Right: options (bookmark per file, keep each file's outline under its bookmark, contents page,
  form fields). The outcome sentence is the prominent line beside Combine: "Combines 4 files, 37 pages, 4 bookmarks".
  Encrypted files stay in the list, greyed, with "Password protected: left out".
  Attachments is a third tab of the left sidebar (Pages, Outline, Attachments): rows with icon, name, description
  and size; a toolbar Add / Save / Open / Delete; the description is edited in place.
- **b-rail**: the combine options live in the right rail and the Organize grid shows the files as sections, so the
  result is visible while choosing. Departs from both by having no dialog.
- **c-ticket**: a compact popover under the toolbar: outcome first, three switches, "Choose files and ranges...".

## Our touch
Outcome before you run it, one sheet, one undo step for every edit (labels, transitions, attachments), and opening an
attachment never runs it: Save out, or open a PDF attachment in a new tab.

## Verdict
2026-10-11: **a-sheet**, picked by inkwell (orchestrator, delegated by the user) with two touches from Acrobat:
1. Each file row has a small first-page thumbnail; expanding the row (or clicking "pages") shows page thumbnails that
   toggle on click and update the range field. The range text stays the source of truth.
2. Files dragged from Explorer onto the sheet are added (same token route as drop-to-open, paths.rs).
Attachments is the third tab of the left sidebar: yes.
