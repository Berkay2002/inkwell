# Insert from file: keep links and form widgets (done: f488dbe)

Today `graft_pages` (src-tauri/src/engine/edit.rs) copies each page alone, and `graft_annots` drops
form widgets and every link that is not a web link: their targets (pages, fields) live elsewhere
in the source, and grafting /P, /Parent or /Dest would pull in the source's whole page tree. Insert,
extract, split and merge all go through `graft_pages`, so all of them gain this.

## Change (edit.rs only)

1. **Two passes in `graft_pages`.** Pass 1 makes every new page (PAGE_KEYS as today) and inserts it,
   recording `moved: HashMap<i32, PdfObject>`: source page object number to the new page's indirect
   ref. Pass 2 copies each page's /Annots with `moved` in hand. (Links to a later page need pass 1 done.)
2. **Links** (/Subtype /Link, also any annot whose /A is GoTo):
   - target = /Dest, else /A /D when /A /S is /GoTo. A name or string target is looked up in the
     source (`/Root /Dests` dict, then `/Root /Names /Dests` name tree) and replaced by its
     explicit array, because the destination document has no such name.
   - Explicit array `[page /XYZ …]`: when `page`'s object number is in `moved`, copy the array with
     element 0 set to the new page ref (graft the remaining elements) and write it as /Dest
     (dropping /A). Otherwise drop the link: its target page did not come along.
   - URI links stay as today. Drop GoToR, Launch and JavaScript actions.
3. **Widgets** (/Subtype /Widget):
   - Copy the widget without /P and /Popup, and set /P to the new page.
   - Field tree: walk /Parent up to the root field. Rebuild the chain by hand, bottom up: copy
     each ancestor, keep only the /Kids being copied (widgets on copied pages, plus their
     ancestors), and point /Parent at the new copy. Graft only the non-structural values (/T, /FT,
     /V, /DV, /Ff, /Opt, /DA, /AP, /MK…). Never graft /Parent, /Kids or /P directly.
   - A widget without /Parent and with /T is itself a root (merged field and widget).
   - Destination /AcroForm: create it in /Root if missing. Append each new root to /Fields once
     (dedupe by source object number). Copy the source /AcroForm /DR fonts and /DA only when keys
     are missing, and keep NeedAppearances as is.
   - Same full name: when the destination already has a root field with that /T, rename the
     copy's /T to `"<T> (2)"` (then 3, …). Fields with the same name share one value in viewers.
4. Popups and the back-references (/IRT, /Popup) are still dropped. Only /P is rewritten.

## Tests (tests_edit.rs; the fixture builder needs a page with /Annots and a /Root /AcroForm)
- A 2-page source with page 1 linking to page 2 and a text field "name" on page 1. Insert both
  pages at 1 into a 3-page file, then reopen:
  - the link on page 2 goes to page 3;
  - the widget is on page 2, and AcroForm /Fields has one "name";
  - its value survives.
- Insert only page 1: the link is dropped and the widget is kept.
- Insert twice: the second field is named "name (2)".
- Extract pp. 1-2 of the result: the link and the field survive.
- Undo after the insert: the destination's /AcroForm is as before (that comes free with snapshots).
