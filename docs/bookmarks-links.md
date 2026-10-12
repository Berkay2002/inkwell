# Edit bookmarks and links

Acrobat's Bookmarks panel and Link tool (study: [study/16-bookmarks-links.md](study/16-bookmarks-links.md), partial:
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
| `dests_list` | `doc` | `{ rev, items: DestEntry[], both, truncated, frozen, locked, signed, unchecked }`: every named destination, sorted by name |
| `dests_apply` | `doc, rev, op` | `{ state: EditState, count, kept }` |
| `bookmarks_headings` | `doc` | `{ count, levels, rows (first 40) }`: the prediction, nothing is changed |
| `links_list` | `doc, page` | `{ rev, links: PageLink[] }` |
| `links_apply` | `doc, rev, op` | `{ state, id, count }` |
| `links_urls` | `doc` | `{ count, pages, rows (first 12) }`: the prediction |
| `links_audit` | `doc` | `{ total, external, broken, rows (first 200 broken: page, id, what) }`: every page's links, nothing changed |
| `link_check_url` | `url` | the address as it would be linked, or an error saying why not |
| `link_pick_file` | none | an open token (`FileRef`) for any file |
| `link_file_path` | `doc, name` | the path a file link resolves to, for the confirm to show: no token is issued |
| `link_file_ref` | `doc, name` | the same file as an open token: called only after the person said Open |
| `link_open_url` | `url` | opens the address (only after the UI's confirm; the gate runs again) |

- Ids are the PDF object numbers of the outline item or link annotation: stable across undo and redo. A `rev` older
  than the document's is `Stale` (list again), as for text edits.
- `BookmarkOp`: `add{parent,index,title,page,at}`, `rename`, `delete{ids}`, `move{id,parent,index}`,
  `setDest{id,page,at}`, `setOpen{id,open}`, `setStyle{id,bold,italic,color,resetColor}`, `fromHeadings{replace}`.
- `LinkOp`: `add{page,rect,target,border}`, `setTarget`, `setRect`, `setBorder`, `delete{page,ids}`, `fromUrls`.
- `DestOp`: `rename{name,to}`, `setView{name,page,at,zoom}`, `delete{name}`, `add{name,page,at,zoom}`, `removeUnused`. A
  destination is named by its text as `dests_list` shows it, not by an id.
- Every op is one edit-core step (`Engine::apply_custom`): one undo step, kept by Save, recorded in history. "From
  headings" and "from URLs" make all their items in one step.

## Bookmarks

- Read: `/Outlines` chain by `/First /Next`; title decoded from the PDF text string (UTF-16BE/LE BOM, UTF-8 BOM,
  PDFDocEncoding); destination `/Dest [page …]` or `/A` (GoTo, URI, other kept as "other"); `/C` clamped to 0..1,
  `/F` bit 1 italic, bit 2 bold; open when `/Count` is positive (Acrobat does the same). At most 50 000 items and
  depth 48 are read; more are left as they are (`truncated`), and such an outline is not edited.
- **Signed documents stay incremental.** `outline::signed` is true for a `/DocMDP` or `/UR3` permission, an
  `/AcroForm` with `/SigFlags` bit 1, or a field `/FT /Sig` with a `/V`. Deleting bookmarks (Delete, From headings
  with Replace) and deleting links normally drop their objects so the next Save is a full rewrite without them
  (a deleted title or address does not stay in the file); for a signed document that would invalidate every
  signature, so the step keeps the objects and Save appends an incremental update that leaves the signed bytes
  where they are. The deleted item is gone for readers, its bytes are not.
- The outline is walked once from the root, first in first out, each item met once (`outline::Shape`): a link to a
  deleted object ends that chain, and so does a link to an item already met. When that item was met by the same
  chain (a loop back into itself) nothing is lost and an edit mends it; when another chain has it (two parents
  sharing items, a cycle between chains) the outline is `tangled`: it is listed with `truncated` set and no edit
  is made ("this outline reaches some bookmarks from more than one place"), because relinking one parent would cut
  the items of the other. Parent and children come from that walk and never from an item's own `/Parent`, so a
  wrong `/Parent` can't misplace a move or delete, and the work is bounded by the items, not by how the file
  links them. An item not found by the walk is `Stale`.
- A certified document (a `/DocMDP` signature) is not changed where the certification does not allow it. Bookmarks
  change the document's structure, so they refuse at any `/P` (1, 2 or 3). Links are annotations: they refuse at `/P`
  1 and 2 and are allowed at `/P` 3, which permits annotations. A certification that cannot be read counts as
  certified, and the message says why. An op that changes nothing (same name, same place) makes no undo step.
- Write: items are edited in place and relinked (`/First /Last /Prev /Next /Parent`), `/Count` of every ancestor is
  recomputed (sign = open or closed). A destination is `[page /XYZ x y null]` in user space. Titles are written as a
  plain string when ASCII, else UTF-16BE. Add or move into a closed parent opens it. Moving into itself or a
  descendant is refused. Deleting removes the item and everything under it.
- **From headings**, two sources, the preview says which was used (`source`: `tags` or `size`):
  - *Tags*: when the file is tagged and its structure tree has headings, those are used: `H1` to `H6` give the level,
    a bare `H` takes its depth in `Sect` elements (1 to 6). The text of a heading is the first text block (at most 3
    lines, 160 characters) at the place its marked content starts (`a11y::locate::mcid_text_origins`); noise and
    titles that repeat on most pages are skipped like in the size source.
  - *Size* (an untagged file, or a tagged one without headings): body size = the size with most characters; a
    heading is a block of at most 3 lines and 160 characters set at least 15 % larger than the body, not a running
    head (same title on at least 3 pages and 30 % of pages); level = rank of the distinct sizes (at most 5).
  The count and the first rows are shown before anything is made; "Replace the bookmarks there are" is off by
  default (the new ones are added after the existing).

## Links

- Read: `/Annots` of the page; only indirect `/Subtype /Link` objects are listed and edited (direct ones are left
  untouched). Target: `/Dest`, `/A` GoTo (page), URI, GoToR / Launch (file), anything else "other" and kept.
  Border: visible when width is above 0, `/C` is not `[]` and the hidden flag is not set; default black 1 pt when
  `/C` is absent (what Acrobat draws).
- Write: `/Rect` (minimum 2 pt a side), `/Border [0 0 w]`, `/C`, `/F 4`, `/P`; a page link as `/Dest`, a web link
  as `/A << /S /URI /URI (…) >>`, a PDF file as GoToR with `/D [page /Fit]`, another file as Launch.
- **Appearance stream**: a visible border is also written as `/AP << /N >>`, a form XObject that strokes the inside
  of the rectangle in the border colour and width (`links::sync_ap`), so page tiles and other readers draw it.
  It is rewritten when the rectangle or the border changes, and removed when the border is made invisible. Links
  that were not made here keep the appearance they have until they are edited.
- **Address gate** (`links::gate_url`, also on open): https, http or mailto only, at most 2048 characters, no control,
  space or bidi characters, a host is required, non-ASCII and characters RFC 3986 does not allow are
  percent-encoded, mail attach/insert parameters are refused (names are percent-decoded and compared without case).
  An address is handed to the system with `ShellExecuteW` (Windows), `open` or `xdg-open`, never through a shell.
- **Files** are chosen through the open dialog (token only, `paths.rs`) and stored as a path relative to the
  document's folder, with `/` between the parts and `../` to go up: the paths are compared without regard to case
  and `\\?\` is dropped. A file on another drive or share is refused with that reason: no path of this machine is
  written into a document. The name stored comes back in `LinkEdited.name` and the rail says "This link looks for
  <name> relative to the document". `/F` is a byte string, `/UF` a UTF-16 text string. A link to a PDF is a GoToR;
  any other file is a Launch, allowed only for documents (`OPENABLE`: pdf, txt, text, md, csv, tsv, log, png, jpg,
  jpeg, gif, bmp, tif, tiff, webp, docx, xlsx, pptx, odt, ods, odp). Everything else is refused, not listed as
  dangerous: programs, scripts, shortcuts, `.appref-ms`, `.chm`, disk images, Office files that can hold macros,
  and a file without an extension. The check ignores trailing dots and spaces and splits on `/` and `\`.
- **Link edits** keep a link's other keys: an existing Launch, GoToR, JavaScript or named action survives a change
  of rectangle or border. Deleting a link also takes it out of the structure tree (`/StructParent`, `/K` object
  references), and the deletion is an `apply_removing` step (an ordinary `apply_custom` one in a signed document,
  see above); so is Remove all links.
- **Opening** an address: from the rail's "Open address…" or by clicking a link in Read mode, always after a confirm
  dialog showing the address, and through the gate again. Nothing opens on its own.
- **Read mode** (`ReadLinks.tsx`, hidden while Edit is open): each link of the pages near the view is a hit area
  over its rectangle. Hovering shows where it goes (page number, address, file name). A page link jumps to its
  target at once; a web, mail or file link asks first, showing the whole address or path; an action Inkwell does
  not follow ("other") and a broken link do nothing (the hover says why). The links are also reachable by Tab.
- **File links** (`link_file_path`, then `link_file_ref`): the name is resolved against the folder of the document
  (`..` goes up from it), may not be absolute or hold `:`, and must be an existing `.pdf`; anything else is
  refused with the reason (a Launch link included: Read mode never runs anything). The confirm shows the path
  `link_file_path` resolved, with no token issued; only when the person answers Open is `link_file_ref` called,
  which issues the open token, and the file opens in a new tab (the page of a GoToR is jumped to afterwards).
  Cancel leaves no token behind.
- **Highlight style** (`/H`): None, Invert, Outline, Inset (`N`, `I`, `O`, `P`); a link with no `/H`, or one with a
  value this does not know, is Invert, as PDF 32000 12.5.6.5 says. It is read into the link model, set from the
  rail's Highlight select as one undo step (writing the same style as the link has makes no step), and in Read
  mode drawn while the link is pressed (`:active`, `data-press`): Invert inverts what is behind it
  (`backdrop-filter`, since the layer is a transformed box and a blend mode would not reach the page), Outline
  draws the accent outline, Inset a pressed-in bevel, None nothing. A link that does nothing when clicked (an
  action Inkwell does not follow, a broken one) shows no press whatever its style (`pressFeedback`).
- **Links written inline in `/Annots`** (a `/Link` dictionary in the array, not a reference) are listed, followed
  in Read mode through the same confirm gate and counted by the audit. They have no object number, so their id is
  `-(position in /Annots + 1)`, good for the generation they were listed at like every id here (a change checks
  `rev`). Edits (target, rectangle, border, highlight) are made in place, in the array, by MuPDF's tracking of the
  object that holds a nested dictionary; the id does not change. Delete takes the entry out by position, from the
  end, together with any indirect links named in the same call. An object listed twice in the array is one link.
  `/Annots` is read up to 50,000 entries and 5,000 links per page.
- **Remove all links** (rail, "All links"; `LinkOp::RemoveAll`): one undo step that takes every `/Link` annotation,
  indirect and inline, off every page, and their objects out of the file. Comments, highlights, form widgets and
  the bookmarks are not touched. The rail counts first (`links_audit`) and asks with the count; a document with no
  links says so and asks nothing. Every page is read before the first link goes: a page with more than the read
  caps (above), a document of more than 100,000 pages, or one whose walk would read more than 2,000,000 entries
  (pages and `/Annots` entries, both passes; an `/Annots` array that pages share is read once) refuses the whole
  removal ("... at once") instead of doing part of it; the audit spends the same budget, so it refuses where
  Remove all would, and a Delete names no more ids than a page lists and reads the page once. The count is the one the audit gives (a link two pages
  list is one); its object is deleted once. The structure tree is cleaned once for all the links, not once per
  link: each parent-tree node is read once (4,096 nodes, 256 kids each, 500,000 `/Nums` and `/K` entries in
  all; what lies past that is left as it is). It is a removal like Delete: an unsigned document's next
  Save rewrites the file without the objects (the addresses are not left in it); a signed one stays incremental.
- **Certified and signed**: `/P` 1 or 2 refuses every links change inside the closure (no step), and `links_list`
  returns `locked` (the reason) so the rail turns Draw, Change target, Delete, border, Highlight, Links from web
  addresses and Remove all links off with it as the title. A signed document gets `signed`: the rail shows the
  note and the Remove all confirm repeats it.
- **Reading** loads the `/Dests` name tree once per read and takes an annotation's object number and generation from
  the walk of `/Annots`; "from web addresses" applies the same 2 pt minimum the link gets, so the preview count
  is the number made.
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
  twisty = expand or collapse (saved; one undo step); style B / I and five colours; "Set to current view" (the
  top left of the view in the page's own space, whatever way the view is rotated).
- Keyboard: Delete removes the selected link or bookmark (not while typing in a field, nor from the bookmark list or
  the rail), F2 renames, Ctrl+Z undoes.
- Right-click on a bookmark (rail or sidebar): Add bookmark, Add bookmark inside, Rename, Set destination to current
  view, Delete.
- **State dots** on every bookmark and link row: a muted dot goes somewhere, a ring goes to a web address, a diamond
  is broken (a named destination the document does not have, or a page that is gone; the row's title text says
  which), a dashed ring is a bookmark that goes nowhere and has nothing under it. The dot is never the only signal
  (title and label text). **Problems** in the bookmark bar filters the tree to broken ones with their parents (shared
  by rail and sidebar); **Show problems** in the Links tab runs `links_audit` over the document and lists the broken
  links ("Page 6 · the named destination "x" is not in the document", click goes there and selects it) with the
  counts of all links and web links.

## Named destinations

Code: `src-tauri/src/outline/dests.rs`; UI: `Destinations` in the Bookmarks section of the Edit rail ("Manage named
destinations", listed when opened, since finding what uses a name walks the document).

- **Sources**: the catalog's `/Dests` dictionary (names as keys) and the `/Names /Dests` name tree (strings as keys),
  read by an own bounded walk, not by `Names`, which flattens the tree and does not see a mis-sorted one or wrong
  `/Limits`. A destination is its text: MuPDF's decoding of the key (`mupdf_text` in `compress/strip.rs`, the viewer's rules), else the bytes
  spelled out as `\xNN`, so a name that can't be decoded is still listed. The same text in both sources is one
  destination and every change reaches both places. The list shows the page (or Broken), sorted by name, and says
  where the name lives only when the file has both sources.
- **Which source is read**: MuPDF reads `/Dests` alone when the catalog has one, and the tree only when it does not.
  A name that is only in the tree of such a file is listed and marked "a viewer that reads /Dests alone does not find
  it" (`shadowed`); its bookmarks and links resolve to Broken, and the list says so rather than hiding it.
- **Rename** follows every reference it can see: bookmarks and links (`/Dest`, `/A` GoTo `/D`), `/OpenAction`, form
  fields, pages and the document's actions, and a destination whose value is another name. A reference spelled in another way (a UTF-16
  string for a `/Dests` name that MuPDF's lookup forms differently), JavaScript that mentions the name, or a result
  that would change where any bookmark or link goes **refuses the rename and names the one it can't rewrite**: the
  document is not changed. A name that already exists in either source is refused; so is a non-ASCII name in
  `/Dests` (the viewer never forms such a key), which only the name tree takes.
- **Set to current view**: `/XYZ` at the top left of the view (page space, rotation undone) and the viewer's zoom
  (1 is 100 %); a value written as `<< /D … >>` keeps its dictionary. Setting the same view again changes nothing.
- **Add** (from the current view): a file whose only source is `/Dests` keeps it there (ASCII name); otherwise the
  name goes into the name tree (made when there is none). `/Dests` is never created or deleted: its presence flips
  MuPDF to `/Dests` alone, so adding one would hide every name in the tree.
- **Delete** takes the name out of its source(s); what used it reads as Broken, and the notice says how many.
- **Remove unused** takes out every name nothing uses: no bookmark, link, GoTo in a form field, page or document
  action, `/OpenAction`, or chained destination. Names kept in doubt are reported with why (at most 20): a script
  mentions the name (any JavaScript text that contains it), or its own name can't be read as text. When the document
  has too many objects or too much JavaScript to check, nothing is removed.
- **The name tree comes out valid**: leaves of 32 pairs and inner nodes of 32 kids, keys sorted by their bytes
  with no duplicates, `/Limits` on every node but the root, kids indirect; the root object is reused, so the
  catalog's reference stays. A node nobody else reaches is dropped; an emptied tree is a root with `/Names []`.
- **Caps** (past any of them the list is partial, `truncated`, and nothing is edited): 20,000 entries in each
  source, 4,096 name tree nodes, 12 levels; looking for uses reads at most 1,000,000 objects, 100,000 pages and
  128 KiB of JavaScript. A tree of a shape this does not write (a node reached twice, a cycle, a kid that is not an
  indirect node, `/Kids` with `/Names`, an odd `/Names`, a key that is not a string) is listed and left as it is:
  `frozen` says why.
- **Certified** (`/P` of any value, as for bookmarks) refuses every change; `dests_list` returns `locked` and the
  panel turns each button off with the reason as its title. **Signed** documents are saved incrementally and the
  panel shows the note; Delete and Remove unused in a signed document leave the old objects in the file.
- Each op is one undo step. After a change the panel lists the bookmarks and links again, since their state dots
  depend on it.
- A rename compares where every bookmark, link and action resolved before and after, and refuses (the document is
  restored) when one differs; the tests hold set view, add, delete of another name and remove unused to the same
  rule.

## Not done

- JavaScript and named actions are listed as "other" and kept as they are; a script that mentions a destination keeps it
  from "Remove unused" and refuses its rename rather than being rewritten.

## Tests

Highlight, inline links and Remove all links: `links::tests` (`the_highlight_style_*`, `a_link_written_inline_*`,
`an_inline_link_is_edited_in_place_*`, `deleting_inline_and_indirect_*`, `remove_all_links_*`, `a_certified_document_takes_neither_*`,
`remove_all_links_of_a_signed_document_*`, `a_hostile_annots_array_*`), the two ops in `engine/tests_certified.rs`, and
`nav.test.ts` ("link actions").

Named destinations: `outline::dests_tests` (list from both sources and from a tree in the wrong order; rename in the
tree, in `/Dests` and in both with bookmarks, links, GoTo and `/OpenAction` followed and undone in one step; rename
refused for a script mention, a reference spelled another way and a name that exists; set view with zoom and a
dictionary value; add into the source the file uses; delete; remove unused keeps what is referenced or in doubt;
the name tree walked node by node for byte order, `/Limits` and indirect kids after inserts and deletes, on a
1,100-entry tree and a given two-level tree; certified refuses every op; a signed document saves incrementally;
unreadable shapes and caps refuse; every bookmark and link resolves the same before and after each op),
`Destinations.test.tsx` and `nav.test.ts` ("named destinations").

`cargo test --lib outline -- --test-threads=1`, `cargo test --lib links -- --test-threads=1`, `src/lib/nav.test.ts` (including following links).

CDP pass on a copy of `docs/study/samples/bookmarks-links-source.pdf`: `scripts/bookmarks-shots.mjs` (32 checks: add, rename,
drag reorder and nest, undo and redo, set destination, style, collapse, sidebar menu, draw a link, gate refusal, open
confirm, audit, save, reopen); screens, light and dark, in `docs/screens/bookmarks/`.

Second pass, for the review fixes: `scripts/links-review-shots.mjs`. The app is started with `bm-copy.pdf`, a
`hostile.pdf` (a GoToR to `C:/Windows/x.pdf` and a Launch of `run.bat`), a PDF in a sibling folder, a PDF on another
drive and `run.bat`; the file picker is answered with their tokens at the page's fetch. It checks Read-mode links
(hit areas, hover titles, the web address asked first, the page jump), both hostile links refused with no token,
bookmark add, move and delete (key, undo, redo; the deleted title is not in the saved file), a file link one
folder up made as `../other/sibling.pdf`, a PDF on another drive and a program refused, and a file link that asks
first and issues its token only after Open.
