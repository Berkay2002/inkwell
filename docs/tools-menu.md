# Tools menu

The "Tools" button sits in the toolbar's mode group, right after Read | Edit | Organize (icon, name, chevron). It was
a text button among the icon buttons on the right.

- **List:** `toolItems(doc)` in `src/components/ToolsMenu.tsx` is a plain array of `{ id, label, group, keywords?,
  onSelect }`. To add a tool, add one entry with its group.
- **Groups:** `TOOL_GROUPS` in `src/lib/toolsMenu.ts` gives the header order and titles: Optimize (Compress, Recognize
  text; Preflight goes here), Page marks (Header & footer, Watermark, Background, Bates), Protect, Convert & create
  (Export, Create PDF), Check & compare (Compare files, Check accessibility). An item whose `group` is not listed gets
  a header of its own after the listed ones, so a worker can add a group without touching the menu code.
- **Filter:** the box at the top has focus when the menu opens. Every word typed must appear in an item's label,
  keywords or group title (`filterTools`, covered by `toolsMenu.test.ts`); empty groups lose their header.
- **Keyboard and ARIA:** the filter box is a combobox (`aria-controls` → a `role="listbox"` of grouped `role="option"`
  rows, `aria-activedescendant` on the highlighted one) and keeps focus. Down/Up move the highlight (wrapping), Enter
  runs it (the first match by default), Escape closes and returns focus to the button, Tab closes and moves on from
  the button, and focus leaving the popup closes it.
- **Hooks:** the button is `button.ax-toggle.tools-btn[aria-label="Tools"]`, the popup is `.ink-menu.tools-menu` and
  every item is a `button[role="option"]` whose text is its label (they were `role="menuitem"`; the shot scripts that
  click Tools → item match both).
- **Organize:** choosing an item leaves Organize first (the tools' panels live in the reader), as the right-hand panel
  toggles do.

Screens: `docs/screens/page-dark/tools-menu-light.png`, `tools-menu-dark.png`, `tools-button-light.png`,
`tools-button-dark.png` (taken by `scripts/page-dark-shots.mjs`).
