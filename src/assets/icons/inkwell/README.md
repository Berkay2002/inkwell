# Inkwell icons

Our own glyphs, for the PDF-specific tools Fluent UI System Icons has no good drawing for. Fluent's are in `../fluent/`
(vendored, do not edit); `../manifest.json` lists both. Docs: `docs/icons.md`.

## Grid and stroke rules (Fluent's 20 px grid)

- `viewBox="0 0 20 20"`, 2 px of air at the edge: the drawing sits in 2.5..17.5, text-like bars may run 3..17.
- Regular (`<name>.svg`): 1.5 px stroke, `stroke-linecap="round"`, `stroke-linejoin="round"`, `fill="none"` and
  `stroke="currentColor"` on the root. Solid parts (the redaction bar, a filled half disc) are `fill="currentColor"`
  on that path.
- Filled (`<name>-filled.svg`): the drawing for tiles and the pressed state. Solid shapes (`fill="currentColor"`,
  `stroke="none"`, `fill-rule="evenodd"` with holes for the details) where the glyph has a body, otherwise a heavier
  stroke (1.75 on the root, 2 to 2.25 on a path). It sits on a coloured tile in white or a tint, so keep details at
  1.4 units or more. Holes must not overlap each other (evenodd turns an overlap back into fill): trace a hash or a
  grid as one outline.
- Only `<path>` elements (the Icon component reads `d`, `transform`, `fill`, `stroke`, `stroke-width`, `fill-rule`).
  A path with `stroke="none"` is a pure fill.
- The line keeps its width in screen px at 16, 20 and 24 (1.5 / 1.75): the component divides by the scale.
- Every glyph needs both files, or the vitest `icon set` check fails.

## Glyphs

| name | what it is |
|---|---|
| redact | a solid bar over lines of text |
| bates | a page with a hash number |
| pdfa | a page standing in an archive box |
| ocr | scan brackets round a letter T |
| compress | a page squeezed from both sides |
| measure | a ruler laid at an angle |
| batch | a stack of pages with a play triangle |
| prepress | the four CMYK plates |
| pageDark | a page with a half-filled disc |
| stamp | a hand stamp over its pad |
| dropdown | a combo box with its arrow |
| squiggle | a wavy underline under a letter |

Not here: the app mark (ink drop, `Chrome.tsx`) and the assistant provider's mark (`AssistantPanel.tsx`) are brand
marks and stay where they are.
