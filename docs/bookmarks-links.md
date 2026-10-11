# Edit bookmarks and links

Acrobat's Bookmarks panel and Link tool (study: [study/14-bookmarks-links.md](study/14-bookmarks-links.md), partial:
the editing UI is not UIA-reachable, rendering behaviour was observed). Code: `src-tauri/src/outline/` (bookmarks)
and `src-tauri/src/links/`; UI in `src/components/edit/` (`BookmarksPanel`, `LinksPanel`, `LinkLayer`) and the store
`src/lib/nav.ts`. Design: `design/rounds/links-r1/spec.md` (a-tabs built; not rendered, image tool at its limit).
Ideas from pdfcraft (add/move with a refusal into itself, link model Page/Uri/Other, https/http/mailto gate with a
confirm before opening); no pdfcraft code is copied, the document stays MuPDF objects.

## Where it lives

Edit mode (Read | Edit | Organize) has one rail with three sections: **Text** (existing), **Links**, **Bookmarks**
(`useTextEdit.section`). Only the section in view has a page layer: text blocks or link outlines. The left sidebar's
Outline tab is the same bookmark editor while Edit is open (`BookmarkTree`, one component for both; the same `nav`
store, so a change in one shows in the other); outside Edit it is the read-only outline and refreshes after every
change.

## Commands

| Command | Arguments | Returns |
|---|---|---|
| `bookmarks_list` | `doc` | `{ rev, items: Bookmark[], count, truncated }` |
| `bookmarks_apply` | `doc, rev, op` | `{ state: EditState, id, count }` |
| `bookmarks_headings` | `doc` | `{ count, levels, rows (first 40) }`: the prediction, nothing is changed |
| `links_list` | `doc, page` | `{ rev, links: PageLink[] }` |
| `links_apply` | `doc, rev, op` | `{ state, id, count }` |
| `links_urls` | `doc` | `{ count, pages, rows (first 12) }`: the prediction |
| `links_audit` | `doc` | `{ total, external, broken, rows (first 200 broken: page, id, what) }`: every page's links, nothing changed |
| `link_check_url` | `url` | the address as it would be linked, or an error saying why not |
| `link_pick_file` | none | an open token (`FileRef`) for any file |
| `link_open_url` | `url` | opens the address (only after the UI's confirm; the gate runs again) |

- Ids are the PDF object numbers of the outline item or link annotation: stable across undo and redo. A `rev` older
  than the document's is `Stale` (list again), as for text edits.
- `BookmarkOp`: `add{parent,index,title,page,at}`, `rename`, `delete{ids}`, `move{id,parent,index}`,
  `setDest{id,page,at}`, `setOpen{id,open}`, `setStyle{id,bold,italic,color,resetColor}`, `fromHeadings{replace}`.
- `LinkOp`: `add{page,rect,target,border}`, `setTarget`, `setRect`, `setBorder`, `delete{page,ids}`, `fromUrls`.
- Every op is one edit-core step (`Engine::apply_custom`): one undo step, kept by Save, recorded in history. "From
  headings" and "from URLs" make all their items in one step.

## Bookmarks

- Read: `/Outlines` chain by `/First /Next`; title decoded from the PDF text string (UTF-16BE/LE BOM, UTF-8 BOM,
  PDFDocEncoding); destination `/Dest [page …]` or `/A` (GoTo, URI, other kept as "other"); `/C` clamped to 0..1,
  `/F` bit 1 italic, bit 2 bold; open when `/Count` is positive (Acrobat does the same). At most 50 000 items and
  depth 48 are read; more are left as they are (`truncated`).
- Write: items are edited in place and relinked (`/First /Last /Prev /Next /Parent`), `/Count` of every ancestor is
  recomputed (sign = open or closed). A destination is `[page /XYZ x y null]` in user space. Titles are written as a
  plain string when ASCII, else UTF-16BE. Add or move into a closed parent opens it. Moving into itself or a
  descendant is refused. Deleting removes the item and everything under it.
- **From headings**: body size = the size with most characters; a heading is a block of at most 3 lines and 160
  characters set at least 15 % larger than the body, not a running head (same title on at least 3 pages and 30 % of
  pages); level = rank of the distinct sizes (at most 5). The count and the first rows are shown before anything is
  made; "Replace the bookmarks there are" is off by default (the new ones are added after the existing). Not done:
  the structure tree (H1 to H6) as a source (needs the text under marked content ids; headings from a tagged file
  are found by size like any other).

## Links

- Read: `/Annots` of the page; only indirect `/Subtype /Link` objects are listed and edited (direct ones are left
  untouched). Target: `/Dest`, `/A` GoTo (page), URI, GoToR / Launch (file), anything else "other" and kept.
  Border: visible when width is above 0, `/C` is not `[]` and the hidden flag is not set; default black 1 pt when
  `/C` is absent (what Acrobat draws).
- Write: `/Rect` (minimum 2 pt a side), `/Border [0 0 w]`, `/C`, `/F 4`, `/P`; a page link as `/Dest`, a web link
  as `/A << /S /URI /URI (…) >>`, a PDF file as GoToR with `/D [page /Fit]`, another file as Launch. Visible
  borders have no appearance stream: the viewer's tiles do not draw them, the Edit overlay does.
- **Address gate** (`links::gate_url`, also on open): https, http or mailto only, at most 2048 characters, no control,
  space or bidi characters, a host is required, mail attach/insert parameters are refused, non-ASCII is
  percent-encoded.
- **Files** are chosen through the open dialog (token only, `paths.rs`) and stored as a path relative to the
  document's folder when under it, else the bare file name. Inkwell never runs a non-PDF file from a link.
- **Opening** an address: only from the rail's "Open address…", always after a confirm dialog showing the address,
  and through the gate again. Nothing opens on its own, and Read mode does not follow links yet.
- **Links from web addresses**: finds `https://`, `http://`, `www.` and e-mail addresses per text line (trailing
  punctuation trimmed), skips addresses already under a link, shows the count first. One undo step.

## UI

- Links: **Draw a link** tool, drag a rectangle, then "Where does it go?" in the rail: a page (number, "Use current
  view" for the place), a web address (checked live by the gate), a file; "Show a border" off by default. The link
  is made when "Create link" is pressed (one step). A selected link has corner handles (move, resize: one step per
  drag) and Delete removes it. Change target, border width and colour are in the rail.
- Bookmarks: **New** adds after the selected one (or at the top level), named from selected text when there is
  some, else "Untitled" with its name in edit; double-click or F2 renames in place; drag to reorder or nest (the
  top and bottom quarter of a row put it before or after, the middle puts it inside); indent / outdent buttons;
  twisty = expand or collapse (saved; one undo step); style B / I and five colours; "Set to current view".
- Keyboard: Delete removes the selected link or bookmark (not while typing in a field), F2 renames, Ctrl+Z undoes.
- Right-click on a bookmark (rail or sidebar): Add bookmark, Add bookmark inside, Rename, Set destination to current
  view, Delete.
- **State dots** on every bookmark and link row: a muted dot goes somewhere, a ring goes to a web address, a diamond
  is broken (a named destination the document does not have, or a page that is gone; the row's title text says
  which), a dashed ring is a bookmark that goes nowhere and has nothing under it. The dot is never the only signal
  (title and label text). **Problems** in the bookmark bar filters the tree to broken ones with their parents (shared
  by rail and sidebar); **Show problems** in the Links tab runs `links_audit` over the document and lists the broken
  links ("Page 6 � the named destination "x" is not in the document", click goes there and selects it) with the
  counts of all links and web links.

## Not done

- The structure tree as a source for bookmarks; clicking links in Read mode (the open path exists in the store);
  appearance streams for visible link borders; named destinations and JavaScript are listed as "other" and kept as
  they are, not editable; link highlight style (Invert, Outline, Inset) is not offered.

## Tests

`cargo test --lib outline -- --test-threads=1`, `cargo test --lib links -- --test-threads=1`, `src/lib/nav.test.ts`.
