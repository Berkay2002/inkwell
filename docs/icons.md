# Icons

One icon system for the whole app, in two folders of plain SVG files.

| | Where | What |
|---|---|---|
| Fluent UI System Icons | `src/assets/icons/fluent/` | Microsoft's icon family (MIT), vendored. Filled outlines on a 16, 20 or 24 px grid, a Regular and a Filled drawing each. |
| Inkwell icons | `src/assets/icons/inkwell/` | Our own, for PDF-specific tools Fluent has no glyph for. Generated in the Fluent style from Fluent references with Codex's image tool and traced to one filled path on the 20 px grid; rules and pipeline in that folder's README. |
| Manifest | `src/assets/icons/manifest.json` | `fluent`: our icon name -> Fluent glyph. `inkwell`: our own names, each with a line saying what it is. |

Why: the old icons were one hand-drawn Lucide-style set, "really generic". Microsoft (Office, Windows 11) uses one
family with Regular for chrome and Filled for the selected state; Adobe (Acrobat 2023+) colour-codes every tool so it
is recognised by colour and shape. Inkwell does both: Fluent for chrome, coloured tiles for tools, our own glyphs
where Fluent has nothing good (made to look like Fluent, not drawn by hand).

## Where the icons show up

- **Chrome** (toolbar, rails, menus, panels): `<Icon name size />`, Regular. `filled` is the pressed / selected
  state (toolbar toggles, the current Read | Edit | Organize item, the current sidebar tab, the active comment tool,
  a pinned tool's pin). Sizes: 16 in dense menus and pills, 20 on the toolbar and nav, 24 for tile glyphs.
- **Tools** (All tools panel rows and chips, the Home grid and quick tools, pins on the toolbar, the palette's tool
  rows, the hover card): `<ToolTile icon group size />`, a rounded square in the group colour with the Filled glyph.
  Tiles: 36 px with a 24 px glyph (panel, hover card), 40 px on Home cards, 28 px for Home quick tools and palette
  rows, 24 px (16 px glyph) for chips and toolbar pins.

The grid follows the rendered size (`gridFor`): 16 up to 17 px, 20 up to 22 px, 24 above, so strokes land on whole
pixels at 100, 125 and 150 % scale. A glyph Fluent does not draw on a grid falls back to the nearest one.

## Tile colours

One pair of CSS tokens per group in `src/components/allTools.css` (`[data-group="..."]`: `--tile-bg`, `--tile-fg`,
light and dark values). Light tiles are a deep hue with a white glyph; dark tiles are a deep shade of the hue with a
tinted glyph; Comment is yellow with a dark glyph, as in Acrobat. Every pair is 4.5:1 or better; the vitest
`tool tile colours` test computes the ratio from the CSS.

| Group id | Colour | Used by |
|---|---|---|
| optimize | teal | Compress, Recognize text, Preflight |
| marks | amber | Header & footer, Watermark, Background, Bates |
| protect | violet | Protect, Security, Redact |
| convert | blue | Export, Create PDF |
| check | green | Compare, accessibility, Advanced search |
| edit | rose | Edit |
| comment | yellow | Annotate |
| organize | emerald | Organize, Combine |
| sign | indigo | Fill & sign |
| print | fuchsia | Output Preview, Convert Colors, Printer's marks, Page boxes |
| scripts | slate | Document JavaScript |
| automate | sky | Actions, batch |

A tool only picks a glyph: `icon` in `toolItems.ts` names an entry of the manifest and the tile colour comes from its
`group`. Groups beyond the five in `TOOL_GROUPS` are listed in `TOOL_HUES` (`lib/toolsCatalog.ts`).

## Adding an icon

1. Fluent has it: find the name in the Fluent set (github.com/microsoft/fluentui-system-icons, or the file names in
   `@fluentui/svg-icons` without size and style), add `"myName": "fluent_name"` to `fluent` in `manifest.json`, run
   `node scripts/vendor-fluent-icons.mjs` (it runs `npm pack` for the pinned version, or takes `--from <icons dir>`)
   and commit the new SVGs.
2. Fluent does not: add the concept to `scripts/icons-gen/generate.mjs`, run it and `trace.cjs` (steps in
   `src/assets/icons/inkwell/README.md`), and add `"myName": "what it is"` to `inkwell` in the manifest. The images
   come from `codex exec` with the Fluent reference sheet attached; the traced SVGs are what is committed.
3. Use it: `<Icon name="myName" />`, or `icon: "myName"` on a tool. `IconName` comes from the manifest, so a typo is
   a type error. `src/lib/icons.test.ts` checks that every name has both drawings, every vendored file is asked for
   and every tool names a glyph that exists.
4. A new tool group needs a colour: a `[data-group="id"]` block with `--tile-bg` and `--tile-fg` (light, dark) in
   `allTools.css`, and its id in `TOOL_HUES` (`lib/toolsCatalog.ts`). The test fails without both.

Rules the tests enforce:
- Manifest keys are alphanumeric only (`[A-Za-z0-9]+`, no hyphen, underscore or space), one per line, sorted
  alphabetically within `fluent` and within `inkwell`, and no name is in both. The loader reads file names with the
  same rule and ignores anything else (a hyphenated file name used to register under its last segment).
- A custom glyph needs both `<name>.svg` and `<name>-filled.svg` in `src/assets/icons/inkwell/`, and a key in
  `inkwell`; a stray file or a key without files fails.
- No component carries path data: no `const X = "M6 12h12"` and no literal `<path d="…">`, except the two brand marks
  (`Chrome.tsx`, `AssistantPanel.tsx`). Use `<Icon name="…" />`; the assistant's own names are mapped in
  `assistant/parts.tsx` (`GLYPHS`).

`node scripts/icons-sheet.mjs <outDir> <cdpPort> <vitePort> [custom|all]` draws a contact sheet of the set inside the
running app (Regular and Filled at 16, 20 and 24, and a tile in three colours, light and dark) for a look before
committing. `scripts/icons-shots.mjs` takes the toolbar, panel, Home, palette and hover card shots; `docs/screens/icons/panel-full-light.png` and `panel-full-dark.png` show every tool and its tile in one tall panel.

## Outside the set

The ink-drop app mark (`Chrome.tsx`, the tab bar) and the assistant provider's mark (`AssistantPanel.tsx`) are brand
marks. Page drawings (`MarkPreview`, the annotation draft layer, the Compare connectors) are content, not icons.

## Licence

Fluent UI System Icons, Copyright (c) 2020 Microsoft Corporation, MIT: `third_party/fluent/LICENSE-MIT`, listed in
`third_party/THIRD-PARTY-NOTICES.txt` and shipped in the app's `licenses/` folder. Pinned at `@fluentui/svg-icons`
1.1.344 (`fluentVersion` in the manifest). Our own icons are Inkwell's.

## Toolbar controls

Read | Edit | Organize, Tools and the assistant group share one outer height, `--tb-control-h` (28 px, `styles.css`):
the 24 px assistant button plus its 2 px padding, as measured in the DOM. The segmented container is a border-box of
that height with 1 px of border and 2 px of padding, so its items are 22 px and the active one never stands taller
than the right-hand buttons. The toolbar's own height (42 px) and the right-hand buttons are unchanged. Before and
after: `docs/screens/icons/toolbar-before-*.png` and `toolbar-after-*.png` (light and dark, 100 / 125 / 150 %, with
`toolbar-*-heights.json`).
