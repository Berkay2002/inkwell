# Dark pages

Shows the pages dark (white paper becomes black) without touching the file. It is a display setting: print, export,
Save, the assistant's page images, OCR, compare and the redaction result always use the real colours.

## Using it

- Toolbar: a page with a half-filled circle, at the end of the view group after rotate (page box | − Fit width + ⟳ |
  dark pages), as it is a page-view setting like zoom and rotate. Tooltip "Dark pages" / "Normal pages",
  `aria-pressed` while dark. Same as the command "Toggle dark pages" (Ctrl+Alt+D, left to a text or form field when one
  has focus). The small chevron beside it, or a right-click on the button, chooses Follow app theme / Always dark /
  Always normal.
- Setting, stored with the app preferences (`localStorage` key `inkwell.pageDark`): **Follow app theme** (default),
  **Always dark**, **Always normal**. Pick it from the Home settings menu ("Pages: ...") or the command palette
  ("Pages: follow app theme / always dark / always normal").
- The toolbar button flips what you see: under Follow in a light app it chooses "Always dark"; pressing it again goes
  back to Follow, because that is what Follow already shows (`togglePageDark` in `src/lib/pageDark.ts`).

## How it works

- `src/lib/pageDark.ts`: the pure rules (parse, resolve the setting against the app theme and the system colour scheme,
  toggle, and which images keep their colours). Covered by `pageDark.test.ts`.
- `src/lib/usePagesDark.ts`: the setting resolved for React; `App.tsx` mirrors it as `data-pages="dark"` on `<html>`.
- `src/styles.css`, "Dark pages": one CSS filter, `invert(1) hue-rotate(180deg) contrast(0.78)`, on every `.tile`
  canvas (the reader, the sidebar thumbnails and the Organize grid use the same `TileCanvas`), on the Organize drag
  ghost and on the page-mark preview. The chrome, overlays and comments are not filtered. The hue turn keeps hues
  roughly where they were (blue stays blue-ish) while black and white swap; the contrast term lifts paper from pure
  black to a dark grey (`#1c1c1c`, just above the viewer's canvas `#0e0f11`) and ink to an off-white (`#e3e3e3`).
  Page, thumbnail and grid backgrounds are that same grey so an undrawn tile does not flash.
- Overlays that assumed white paper are adjusted under `data-pages="dark"`: search hits and the assistant's citation mark
  stop using `multiply` (invisible on black), redaction marks and the accessibility finding switch to light ink, the
  selection is a brighter blue, and `.page` gets `color-scheme: dark` so form fields and comment popovers use their dark
  tokens. The page gets a hairline edge so it still reads as a sheet on the dark canvas.
- The header and footer / watermark / background preview is drawn as page ink, so it takes the same filter and shows what
  the page will look like.

## Keep photos natural

`PageView` asks `image_blocks` (the image-edit command) for the page's pictures while the pages are dark and lays a
`NaturalImage` canvas (`.nat-img`) over each one. The CSS filter only changes how the tile canvases are shown, not
their pixels, so the box copies its region straight out of the tiles (preview, previous scale, current tiles, in that
order) and covers the filtered ones: the picture is the rendered one, pixel for pixel, with no second filter run over
an already clamped 8-bit page (an earlier version inverted the filter with a `backdrop-filter`, which turned pure red
into about 67/24/24 and banded). Tiles tell the boxes to repaint with a bubbling `tiledrawn` event; `displayRect`
turns a page-space box into the rotated page's space. Cached per document revision
(`cached("${doc}:${rev}:${page}:img")`, cleared with the document).

Rules in `naturalRects` (`pageDark.ts`, tested):

- Boxes never overlap: a spot covered twice would be restored twice and come back dark. A box inside another vanishes,
  a partial overlap is cut off the later box (up to four disjoint pieces).
- Skipped: stencil masks (ink), images with a soft or stencil mask (a transparent logo is not a box), pictures
  narrower than 18 pt, and any picture that alone covers 60 % or more of the page.
- When the pictures together cover 60 % or more of the page it is a scan made of strips or tiles: nothing is restored
  and it goes dark with the page. Past 24 boxes the largest 24 are kept (each box is one backdrop filter).

Limits, on purpose:

- `image_blocks` finds pictures inside form XObjects only up to its limits (4 forms deep, 64 form drawings per
  page); a picture past them, in an annotation appearance or in a Type3 glyph is not found, so it is inverted with
  the page.
- Thumbnails and the Organize grid (and its drag ghost) are inverted whole: no photo restoration at that size.
- Plain colour fills (a red swatch) are filtered with the page: only pictures are restored.

## Checked

`scripts/page-dark-shots.mjs` drives the real app over CDP on a COPY of `samples/edit-source.pdf` with an
added blue link line and colour swatches, plus a copy of `form-source.pdf`. Screens:

- `pages-normal-light-app.png`, `pages-dark-light-app.png`, `pages-dark-dark-app.png`, `pages-normal-dark-app.png`
- `search-dark-pages.png`, `selection-dark-pages.png`, `comments-dark-pages.png` (+ `-light-app`),
  `form-dark-pages.png`, `thumbnails-dark-pages.png`, `organize-dark-pages.png`

`photo-natural-dark-pages.png` is a picture of six flat colours on a dark page (`scripts/make-rgb-photo.mjs`); the
script samples each block on normal and dark pages and asserts they match within 3 levels, also on a page turned a
quarter. `pagebox-*-light/dark.png` show the page-number box at 1 / 1, 12 / 91 and 1234 / 1500
(`scripts/make-blank-pages.mjs`); the box is `pageBoxCh(pageCount)` wide in `ch` (digits + 1, 3 to 6).

The script asserts: the toggle works in the light and the dark app, Follow tracks the theme, the setting survives a
reload, the page margin is white / dark grey and above the app's canvas, the app chrome pixels are unchanged, the photo's mean colour is the same with
normal and dark pages, the document's revision and dirty flag do not change, and only `.tile` / `.mk-preview` elements
carry a filter.
