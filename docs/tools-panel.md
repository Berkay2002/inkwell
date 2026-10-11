# All tools panel

The "Tools" button keeps its place in the toolbar, right after Read | Edit | Organize. It toggles an "All tools" panel
in the left sidebar slot, replacing Pages / Outline / Attachments while it is open (the way Acrobat's All tools view
does). Esc or the button closes it and the sidebar comes back. Open or closed is per session.

Replaces the Tools dropdown (`ToolsMenu.tsx`, `toolsMenu.ts` and their tests, removed).

## What it shows

- **Find a tool** box, focused on open. Matches the name, the one-line description, the group title and synonyms, so
  "smaller" and "shrink" find Compress file, "OCR" and "scan" find Recognize text, "stamp" and "draft" find Watermark,
  "page numbers" finds Header & footer and Bates numbering. Every word must match; a name match ranks above a synonym,
  a synonym above a description. Enter runs the first match. A search shows one ranked "Results" list.
- **Recent**: the last four tools used, as icon chips (not shown while searching).
- **Groups**: Optimize, Page marks, Protect, Convert & create, Check & compare (which also holds Advanced search).
  Each row has a coloured 28px icon chip
  (hue per group), the name and a one-line plain description.
- **Pin** star on each row: puts the tool in the toolbar as an icon button after Tools. At most three; a fourth is
  refused (the other stars are disabled and say why) rather than silently dropping an older pin.
- **Hover card**: after 600 ms of hover or keyboard focus on a row, a card beside the panel with the name, a fuller
  sentence and the shortcut if the tool has one.

Running a tool from the panel closes the panel (the tool's own rail takes the space) and records it as recent.

The panel belongs to the reader. Home, a comparison and Organize leave it closed (the Tools button in Organize switches
to the reader and opens it), so coming back never shows it unasked or moves focus into it. Any `setSidebar` or
`toggleSidebar` call closes it, even when the choice does not change; with the panel open the sidebar toggle is not
shown as pressed and shows the sidebar underneath instead of hiding it.

Pins and recents are checked against the catalog: an id that is not a tool any more (renamed, removed) is dropped on
load, and the pin limit counts what the toolbar shows, so a leftover id cannot fill a slot.

Below 1240px the toolbar's pinned buttons are hidden (three pins add about 100px to a toolbar that is already about
1130px wide, and the window can be 720px): every tool is still in the panel.

## Where else the same list shows up

- **Home**: an "All tools" grid under the recent files, three or four cards wide. With no document open a click asks
  for a file first, then runs the tool on the document that file became (looked up by its path, not "whatever is
  active"); a locked file waits for the password and then runs, a failed open shows its error. Create PDF needs no file.
- **Command palette**: entries that map to a tool show its description as a second line and also match on its synonyms.
- **Toolbar**: the pinned tools, with the description as their tooltip.

## Code

| File | Role |
|---|---|
| `src/lib/toolItems.ts` | The single catalog: one entry per tool (`id`, `group`, `icon`, `label`, `description`, `detail`, `keywords`, `command`, `onSelect`). `runTool` records recents. `launchToolFromHome` asks for a file first. |
| `src/lib/toolsCatalog.ts` | Pure logic: `TOOL_GROUPS`, `searchTools` (tokens, stem, scoring), `groupTools`, `filterTools`, `pushRecent`, `togglePin`, `cleanIds`. Covered by `toolsCatalog.test.ts`. |
| `src/lib/toolPrefs.ts` | zustand store: `recent` and `pinned` persist in localStorage `inkwell.toolPrefs`; `panelOpen` is session-only. |
| `src/components/AllTools.tsx` | `ToolsButton`, `PinnedTools`, `AllToolsPanel`. |
| `src/components/allTools.css` | Panel, rows, chips, card, Home grid, palette subtitle styles, group hues. |
| `src/components/Sidebar.tsx` | Renders the panel in the slot while `panelOpen`. |

### Adding a tool

Add one object to `toolItems()` in `src/lib/toolItems.ts` (other branches such as PDF/A, forms and convert-plus add
theirs there): `group` is a `TOOL_GROUPS` id (an unknown id gets a header of its own at the end), `description` one
plain line, `keywords` the words people would search for, `command` the command id so the palette entry gets the
subtitle and the card shows the shortcut. The panel, Home grid, pins and palette pick it up.

## Accessibility

- The panel is `nav` with `aria-label="All tools"`; the Tools button has `aria-expanded` and `aria-controls`.
- Each row is a plain button whose `aria-describedby` points at its description; the pin is a separate toggle button
  (`aria-pressed`, labelled "Pin X to the toolbar"). The hover card is `aria-hidden`: the same text is on the row.
- Keyboard: focus starts in the search box; Up/Down move through the box and the rows, Home/End jump to the first/last
  (the box keeps Home/End for the caret), Enter runs, Esc closes and focus returns to the Tools button. A sidebar
  toggled elsewhere (Ctrl+B, the toolbar's Pages and Outline buttons) hands the slot back and closes the panel.
- `.ax-toggle` stays on the Tools button, so existing automation that clicks it still works.

## Checks

`scripts/tools-panel-shots.mjs` drives the panel through DevTools on a copy of a study sample: landmark and ARIA, synonym
search, keyboard, hover card delay, Recent, pin limit and persistence, palette subtitle, Home grid, light and dark.
Screens are in `docs/screens/tools-panel/`.
