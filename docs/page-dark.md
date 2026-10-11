# Dark pages

Shows the pages dark (white paper becomes black) without touching the file. It is a display setting: print, export,
Save, the assistant's page images, OCR, compare and the redaction result always use the real colours.

## Using it

- Toolbar: the half-filled circle next to the page number. Tooltip "Dark pages" / "Normal pages". Same as the command
  "Toggle dark pages" (Ctrl+Alt+D).
- Setting, stored with the app preferences (`localStorage` key `inkwell.pageDark`): **Follow app theme** (default),
  **Always dark**, **Always normal**. Pick it from the Home settings menu ("Pages: ...") or the command palette
  ("Pages: follow app theme / always dark / always normal").
- The toolbar button flips what you see: under Follow in a light app it chooses "Always dark"; pressing it again goes
  back to Follow, because that is what Follow already shows (`togglePageDark` in `src/lib/pageDark.ts`).

## How it works

- `src/lib/pageDark.ts`: the pure rules (parse, resolve the setting against the app theme and the system colour scheme,
  toggle, and which images keep their colours). Covered by `pageDark.test.ts`.
- `src/lib/usePagesDark.ts`: the setting resolved for React; `App.tsx` mirrors it as `data-pages="dark"` on `<html>`.
- `src/styles.css`, "Dark pages": one CSS filter, `invert(1) hue-rotate(180deg)`, on every `.tile` canvas (the reader,
  the sidebar thumbnails and the Organize grid use the same `TileCanvas`). The chrome, overlays and comments are not
  filtered. The hue turn keeps hues roughly where they were (blue stays blue-ish) while black and white swap.
- Overlays that assumed white paper are adjusted under `data-pages="dark"`: search hits and the assistant's citation mark
  stop using `multiply` (invisible on black), redaction marks and the accessibility finding switch to light ink, the
  selection is a brighter blue, and `.page` gets `color-scheme: dark` so form fields and comment popovers use their dark
  tokens. The page gets a hairline edge so it still reads as a sheet on the dark canvas.
- The header and footer / watermark / background preview is drawn as page ink, so it takes the same filter and shows what
  the page will look like.

## Keep photos natural

`PageView` asks `image_blocks` (the image-edit command) for the page's pictures while the pages are dark and lays an
empty `.nat-img` box over each one. The box has `backdrop-filter: invert(1) hue-rotate(180deg)`: it inverts the already
inverted page behind it once more, so the picture comes back in its own colours. Cached per document revision
(`cached("${doc}:${rev}:${page}:img")`, cleared with the document).

Limits, on purpose:

- Skipped: image masks (they are ink, not photographs), pictures narrower than 18 pt, and pictures that cover 60 % or
  more of the page (a scan: it is the page, so it goes dark with the rest).
- `image_blocks` reads the page's own content only: a picture drawn inside a form XObject is not found and is inverted.
- Thumbnails and the Organize grid are inverted whole (no photo restoration at that size).
- Inversion is not an exact round trip for very saturated colours (the filter clamps), so a photo can differ by a few
  levels. Plain colour fills (a red swatch) are inverted with the page: only pictures are restored.

## Checked

`scripts/page-dark-shots.mjs` drives the real app over CDP on a COPY of `docs/study/samples/edit-source.pdf` with an
added blue link line and colour swatches, plus a copy of `form-source.pdf`. Screens are in `docs/screens/page-dark/`:

- `pages-normal-light-app.png`, `pages-dark-light-app.png`, `pages-dark-dark-app.png`, `pages-normal-dark-app.png`
- `search-dark-pages.png`, `selection-dark-pages.png`, `comments-dark-pages.png` (+ `-light-app`),
  `form-dark-pages.png`, `thumbnails-dark-pages.png`, `organize-dark-pages.png`

The script asserts: the toggle works in the light and the dark app, Follow tracks the theme, the setting survives a
reload, the page margin is white/black, the app chrome pixels are unchanged, the photo's mean colour is the same with
normal and dark pages, the document's revision and dirty flag do not change, and only `.tile` / `.mk-preview` elements
carry a filter.
