# Inkwell icons

Our own glyphs, for the PDF-specific tools Fluent UI System Icons has no good drawing for. Fluent's are in `../fluent/`
(vendored, do not edit); `../manifest.json` lists both. Docs: `docs/icons.md`.

These are not hand-drawn. Each one was generated in the Fluent style with Codex's image tool, using Fluent icons as the
style reference, then traced to a vector path. The pipeline is in `scripts/icons-gen/`:

1. `node scripts/icons-gen/generate.mjs <rawDir> <name>...` asks Codex for a 2-up PNG (Regular outline left, Filled
   solid right) per glyph. `reference.png` next to it is the style reference; the concept line for each glyph is in
   `generate.mjs`.
2. `node scripts/icons-gen/trace.cjs <rawDir> src/assets/icons/inkwell <name>...` splits each PNG and traces both
   halves with potrace (see the header of that file for the one-off install).
3. Look at the result in the app (`node scripts/icons-sheet.mjs`, see `docs/icons.md`), and regenerate a glyph whose
   concept came out wrong by adjusting its line in `generate.mjs`. The SVG files here are the source of truth, not the
   PNGs.

## File rules

- `viewBox="0 0 20 20"`, one `<path fill-rule="evenodd">` per file, no stroke, no transform, no other elements
  (the Icon component reads `d` and `fill-rule` only, and colours it with `currentColor`).
- The glyph is fitted into 16 x 16 units and centred, so there are 2 units of air at the edge, as in Fluent's 20 px set.
- `<name>.svg` is the Regular (outline) drawing and `<name>-filled.svg` the Filled one: a solid shape with its
  details knocked out. Every glyph needs both, or the vitest `icon set` check fails.
- One grid only (20). The Icon component scales it at 16 and 24 px; a hand-tuned 16 or 24 px drawing could be added
  later as `<name>-16.svg`, which the loader does not read yet.

## Glyphs

| name | what it is |
|---|---|
| redact | a page with a text line and a solid redaction bar |
| bates | a page with a number sign |
| pdfa | a page with an archive box |
| ocr | scan brackets round a letter A |
| compress | a page squeezed by two arrows |
| measure | a diagonal ruler |
| batch | two stacked pages with a play triangle |
| prepress | a print registration target |
| pageDark | a page with a half-filled disc |
| stamp | a rubber stamp over a line |
| dropdown | a combo box with its chevron |
| squiggle | a letter A over a zigzag underline (the one hand-drawn glyph: the traced wave read as a plain underline at 16 px) |

Not here: the app mark (ink drop, `Chrome.tsx`) and the assistant provider's mark (`AssistantPanel.tsx`) are brand
marks and stay where they are.
