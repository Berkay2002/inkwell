# Compare

Compare shows what differs between two PDFs: two files, a file and one of its past versions, or two versions. The
engine is `src-tauri/src/diff/` (bytes in, a typed change list out; it never touches the user's files or the edit
core). The setup page is `src/components/history/CompareSetup.tsx`, the result `Compare.tsx`, the commands
`history_compare_run`, `history_compare_report` and `history_page_count` in `history_ipc.rs`. Compare is read-only:
no document changes, nothing is written except a report the user saves through a save token.

## Page ranges

Each side has a Pages field on the setup page (same syntax as the other page range fields: `1-3, 5`, 1-based; empty
means every page). The two ranges are independent: they may differ in length, one side may have none, and the same
file may be chosen on both sides when the ranges differ (old pages 3-10 against new pages 1-8 of one file).

- Pairing works on the picked pages only, as it does on a whole file: pages are matched by content, the rest are
  added, removed, replaced or moved, and "next to each other" means next in the picked pages.
- Every page number the user sees is the file's own: "Text changed on p. 5", "Page 7 removed", the rows and strips
  of the result, the report. The engine's `PageMap` carries `aIds` and `bIds` (the compared pages, ascending) and
  `bToA` and `removedA` hold file page numbers. Nothing is a position in the range.
- The range is typed per side and travels with the source: changing a side's file clears its range, swapping the
  sides swaps the ranges.
- The setup page asks the engine for each file's page count (`history_page_count`), so a page past the end is
  flagged while typing. The engine checks again before any page is read and refuses with "the first file has 12
  pages, so page 15 is out of range" (or "no pages of the first file were chosen" for an empty list). A page count
  that cannot be read (a password, a damaged file) leaves only the syntax check.
- The existing caps (the per-page diff budget, the picture pages of the report, the render pixel cap) apply to the
  picked pages; progress counts the picked pages, so a short range is quick on a long file.
- The comparison cache is keyed on both files' bytes and both ranges, so a report and the view of the same ranged
  comparison share one result and a different range never gets a cached whole-file one.

What is still file-wide: metadata (Info and XMP) and attachments are compared for the whole file, since they belong to
no page. Bookmarks are compared only where they point into the picked pages of their side (or nowhere). The report
header names the ranged side as "file.pdf (pages 3-10)" followed by the number of pages compared.
