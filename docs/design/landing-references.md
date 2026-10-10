# Inkwell start screen: reference research

Scope: start / home screen with brand mark, drop zone + Open, key-cap hints, recent files (thumbnail, name, folder, pages, relative time, pin, remove, missing state).

Verification note: only a few vendor pages could be fetched in full and most did not describe the home screen. Most claims below come from WebSearch result summaries (page excerpts), marked "search". Visual details (colors, exact spacing) were NOT verified for any app; treat them as unknown. No screenshots were seen.

## PDF apps

### PDF Expert (Readdle)
- Fetched https://pdfexpert.com/ : marketing page only, no home screen description. Home layout NOT verified.
- Only verified: tool-first tiles (Edit, Annotate, Convert, Sign, Merge), Day/Night/Sepia reading modes, "designed for Apple" native styling.
- Avoid: leading with a tool grid on the start screen; Inkwell is a viewer first.

### Foxit PDF Editor
- Search: https://resource-cms.foxit.com/foxit-pdf-editor-for-windows/about-the-start-page . Start page lists recent documents, product tutorials and a Tool Wizard; can be turned off in Preferences; reopenable from View > Start Page.
- Works: recents shown on a dedicated Start tab, with a setting to turn it off.
- Avoid: ribbon UI (fetched https://www.foxit.com/pdf-editor/ describes "familiar ribbon-style") and tutorials / wizard clutter on the start tab.

### PDFgear
- Search: https://www.pdfgear.com/windows-user-guide/open-a-pdf.htm . Windows welcome page has an "Open File" button and a Recent area with a Clear button. Mac guide describes drag from Finder into the window. Drag-and-drop on the Windows welcome page: not confirmed.
- Works: one obvious Open File button plus a Recent list; Clear action.
- Avoid: Clear-all as the only management; no per-item pin or remove noted.

### UPDF
- Search: https://updf.com/updf-mac-user-guide-1/toolbar-navigation-updf-mac/ . Home has a left sidebar (Recent, Starred, Cloud, PDF Tools, Batch), an Open File button at top middle, recents in the middle. Recents can be sorted, switched between list and thumbnail views, starred, renamed, cleared, shown in Finder.
- Works: list/thumbnail toggle, star (pin), sort, reveal in folder. This is the closest feature match to Inkwell's recents.
- Avoid: sidebar with cloud / tools / batch entries; Inkwell is local and private.

### SumatraPDF
- Search: https://blog.kowalczyk.info/article/ao9k/sumatrapdf-1.6-released.html (frequently-read thumbnails, "inspired by new tab page in Chrome"), https://www.neowin.net/software/sumatrapdf-36/ (30 thumbnails, default sort by recent). Official docs page for the start page returned 404.
- Works: page-1 thumbnail grid of recents is the proven pattern for PDF readers; recency as default sort.
- Avoid: no visible management (pin/remove) in what I found; dense, unstyled grid.

### Okular
- Search: https://discuss.kde.org/t/okular-22-12-3-is-it-possible-to-disable-the-display-of-recent-documents/20591 and https://mail.kde.org/pipermail/okular-devel/2026-June/049452.html . Welcome screen shows recents when launched without a file, "Forget All" button, capped at 10 entries (open wishlist to raise it).
- Works: recents appear only in the no-document state.
- Avoid: hard cap of 10, no way to hide, only a clear-all.

### macOS Preview
- Search found nothing usable about a start screen (results were unrelated threads). Verified: nothing. Do not cite. Known only from general knowledge: Preview has no start screen, opens a file dialog; Open Recent lives in the File menu.
- Takeaway: Inkwell should also expose recents in a menu / jump list, but this is an assumption, not a finding.

## Modern desktop apps

### Linear
- Search: https://linear.app/blog/how-we-redesigned-the-linear-ui and https://linear.app/blog/behind-the-latest-design-refresh . Reduce visual noise, improve hierarchy and density; "not every element should carry equal visual weight"; LCH-based theme generation from few variables.
- Works: restrained chrome, one accent, theme tokens generated from base + accent + contrast.
- Avoid: density that is right for issue lists but too tight for thumbnail cards.

### Arc
- Search (How-To Geek, TidBITS, ghacks; no first-party page read). Sidebar-first, pinned items persist, unpinned items auto-archive, per-space color. TidBITS reports the company later judged parts a "novelty tax".
- Works: pinned vs unpinned separation; color as identity.
- Avoid: novel navigation; the start screen must be obvious in 1 second.

### Raycast
- Search: https://manual.raycast.com/keyboard-shortcuts.md , https://www.raycast.com/blog/a-fresh-look-and-feel . Keyboard-first; action bar shows available actions with their shortcut in context; Cmd+K opens actions.
- Works: shortcuts shown inline next to the action, in key caps, where they apply.
- Avoid: relying on a command palette as the only path; third-party design-system extractions (not official) were ignored.

### Notion
- Search: https://www.notion.com/en-gb/help/navigate-with-the-sidebar . Home lists Recents and Favorites as sections; sections can be limited in count, reordered, hidden; Cmd/Ctrl+K search jumps to recent pages.
- Works: Favorites separate from Recents; user-controlled count.
- Avoid: many sections; Inkwell has exactly two (Pinned, Recent).

### Figma file browser
- Fetched help article did not cover it. Search (forum.figma.com, firmbee.com): grid by default with grid/list toggle; recents ordered by recent view/edit; file-type icon only on hover; list view removed in a March 2025 update then restored after feedback.
- Works: grid default plus list toggle (users demanded list back).
- Avoid: hiding meta behind hover only.

### Zed welcome
- Fetched https://zed.dev/docs/getting-started : welcome shows in the empty center pane when no folder is open; quick actions (open folder, clone, docs); disappears once something opens; reopen via command palette. Docs do not say the page displays shortcuts.
- Works: start screen exists only in the empty state; very few actions.
- Avoid: no recents on the page itself (they live in a separate Open Recent picker).

### VS Code welcome
- Fetched https://code.visualstudio.com/docs/getstarted/userinterface : no welcome-page content. Search (third-party/Microsoft Learn): sections Start (new file, open folder), Recent, Walkthroughs.
- Works: Start actions on the left, Recent below them, short list.
- Avoid: walkthroughs; GitHub issue shows they annoy experienced users.

### Things
- Search (MacStories, thesweetsetup; no empty-state source found): generous white space, bold type, few splashes of color. Empty-state behavior NOT verified.
- Works: whitespace and typography carry the design instead of borders.
- Avoid: nothing specific verified.

## What Inkwell takes and why

Base on: UPDF home (Open button on top, recents in the middle, list/thumbnail toggle, star, sort, reveal-in-folder), PDFgear welcome (Open File + Recent + Clear), Foxit Start tab (recents on a start tab, can be switched off), SumatraPDF (page-1 thumbnail grid, default sort by recent), with the VS Code / Zed welcome rule (start screen only when no document is open) and Raycast inline key caps.

Layout (single column, centered, max ~960 px):
1. Header strip: brand mark + wordmark left; Settings and theme toggle right. No toolbar, no ribbon.
2. Hero row: large drop zone (dashed 1 px border, radius 16) with the Open button inside it as the primary action. Drag-over: border goes accent, background tint, label "Drop to open".
3. Key caps under the drop zone, one line: Ctrl+O Open, Ctrl+K Command search, Ctrl+Shift+T Reopen last (only list shortcuts the app really binds).
4. Recents section: heading "Recent" with count, right side: sort (Recent / Name), grid-list toggle, "Clear" (confirm). Pinned group above Recent, only if any are pinned.
5. Empty state (no recents): the drop zone grows to fill the page, one sentence "Nothing here yet. Open a PDF to see it listed here. Files stay on this PC."; recents section hidden, not shown empty.

Recents card anatomy (grid): page-1 thumbnail (3:4, fixed ratio, skeleton while rendering) on top; below: file name (1 line, middle ellipsis keeps extension), folder path muted (1 line, tail ellipsis, full path in tooltip), meta row "24 pages  |  2 h ago". Hover reveals pin and remove icon buttons top-right and a "Show in folder" action; pinned items show a persistent pin glyph. Keyboard: arrows move focus, Enter opens, P pins, Del removes from list (never deletes the file). List mode: same fields in one row, thumbnail 40 px.
Missing file: card dimmed, thumbnail replaced by a broken-file glyph, "File not found" in place of time, actions become "Locate..." and "Remove"; do not auto-remove.

Modern touches Inkwell adds on top of the above:
- Brand mark: a small ink-drop / nib glyph, single color, also used as the drop-zone center glyph.
- Theme tokens: Linear-style small token set (base, accent, contrast) with generated surfaces; dark default, light and system follow; one accent color; WCAG AA text contrast.
- Motion: 120-180 ms ease-out for hover, drop-zone state and card reorder on pin; thumbnails fade in; honor prefers-reduced-motion by removing movement.
- Polish: key caps as small raised chips (inline next to the action they trigger, per Raycast); skeletons instead of spinners; focus rings on every card; relative time updates without reload.

Open questions (not verified): exact visuals of PDF Expert, Foxit, PDFgear, UPDF home (no screenshots read); Preview and Things empty states. Check with real screenshots before pixel decisions.

## As built (phase "landing")

Based on UPDF home (Open button on top, recents as thumbnails, grid/list toggle, pin, remove), PDFgear welcome (Open + Recent + Clear), SumatraPDF (page-1 thumbnail grid) and the VS Code / Zed rule that the start screen exists only while nothing is open. Key caps inline next to the actions they trigger (Raycast). Built: drop zone with drag-over state, Open + Commands key caps, pinned-first recents grid/list with thumbnail, folder, pages, size, relative time, pin, remove, Clear (two-step), missing-file state with Locate/Remove, arrow-key navigation (P pins, Del removes). Not built: sort toggle (always pinned, then newest), "Show in folder" (needs the opener plugin), Settings entry.
