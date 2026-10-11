# Rich media: list and preview, never execute

Code: `src-tauri/src/media/` (`mod.rs` listing, classifying and the decoded-asset cache; `route.rs` the media route's
paths and byte ranges; `app.rs` the two commands; `tests.rs`). UI: `src/lib/media.ts` (store, `media.test.ts`),
`src/components/MediaLayer.tsx` (badges and the player over the page), `MediaPanel.tsx` (sidebar tab "Media"),
command palette "Media in this document". Sample: `docs/study/samples/media-source.pdf`, built by
`scripts/media-sample.py` (needs ffmpeg with libx264 and libmp3lame).

Nothing here edits the document: no undo step, no history entry, nothing for Save to write. It is not an assistant
tool; a click is what starts anything playing.

## What is listed

Per page, every annotation of these kinds (the page's raw `/Annots` is screened first, so only pages that have one are
loaded):

| Kind | Where the assets are |
|---|---|
| RichMedia | `/RichMediaContent /Assets` (name tree) and the configurations' instances (`/Instances /Asset`), one row per file spec |
| Screen | the rendition in `/A` or any `/AA` action (`/S /Rendition`): `/R` media rendition `/C` clip `/D`, selector renditions' list, `/MH` and `/BE`, `/Next` chains; a clip section follows its `/D` |
| Movie | `/Movie /F` |
| Sound | the `/Sound` stream (or its `/F` file) |
| 3D | `/3DD` (U3D or PRC) |

`/D` of a clip may be a file specification, a plain path or URL, or the media bytes as a stream.

Each asset: name (no folder, nothing invisible), type, size, embedded or outside the file. The title is `/T`, else
`/Contents`, else the first asset's name. The poster is the annotation's own appearance, drawn by the page tile like
any other annotation (a 3D annotation without one shows nothing but its badge).

## What plays

Only an **embedded** asset whose **bytes** are one of: MP4/M4V (H.264) and MP4 audio (M4A), WebM, MP3, AAC (ADTS),
WAV, Ogg. The type is read from the first 64 bytes; `/CT`, the stream's `/Subtype` and the file name are only claims,
and a claim that disagrees with the bytes makes the asset not playable ("declared as MP4 video but the data is MP3").
The `Content-Type` served comes from a fixed list chosen by the bytes, never from the file, with `nosniff`.

Not playable, with the reason shown in the panel and on the badge:

- Flash (SWF, FLV): end of life, never run.
- Scripts: `.js` files in the asset tree, `/JavaScript` actions, `/JS` in an action. The item is marked "Scripts in this
  item are never run"; a playable asset next to a script still plays.
- 3D (U3D, PRC, any `/3D` annotation): "3D content is not supported".
- Executables (PE, ELF, Mach-O), anything not recognised, QuickTime (.mov) and Matroska.
- Raw sound samples of a Sound annotation (PCM, mu-law, A-law): "sound with raw samples, not playable here".
- Outside files and links ("external media, not loaded"): never fetched, never opened, never even stat-ed.
- Over 256 MB decoded: "too large to play here".

An item with several assets is playable when any is; the first playable one in file order plays (instances before the
rest of the asset tree). The rendition's own play parameters (autoplay, repeat, window or floating, URL) are ignored.

## The route

`inkwell://localhost/m/<doc>/<token>/<annotation object>.<index>` (`http://inkwell.localhost/m/...` on Windows). The
path takes only these: a decimal document id, a 32-character lowercase hex token and an id of two decimal numbers.
`media_list` returns the token with the items; it is minted with the document's first listing, kept while the document
is open (an edit that keeps the asset keeps the player's address) and unknown to any other run of the app. Each listing
replaces the set of playable asset ids the route may serve (one made at an older document generation than the
recorded set is dropped, so two listings finishing out of order keep the newer). A click calls `media_play(doc,
asset)`, which arms that one asset (one player at a time); the element is mounted after it returns. A request is
answered only when the token is the document's, the id is in the set and it is the armed asset; the handler then
reads the asset with the same rules as the list and checks the bytes again. A wrong token, an id the listing did not
return (past its caps, or not playable), an asset that is not armed, an unknown document or a closed one answer 404.
`media_release` (the player was removed) disarms it: the late range requests of the removed element are refused and
neither decode nor refill the slot, and neither does a decode that was already waiting or running when it came.

- Byte ranges: `a-b`, `a-` and `-n` answer 206 with `Content-Range`; past the end 416. A ranged answer is cut to
  1000 KB (as Tauri's asset protocol), and the player asks again. With no usable Range (none, several ranges, bad
  syntax) a body over 1000 KB gets the first 1000 KB as a 206 (the responder takes a whole body, so a copy of up to
  256 MB is never made); a smaller one is a 200.
- The decoded asset is kept in one slot (doc, id, document generation) so a seek does not decode again; `media_release`
  empties it when the player is removed. Decoding is capped at 256 MB (a compressed stream cannot inflate past it) and
  happens outside the slot's lock, one at a time, so a release or a cached seek never waits for it.
- A listing's work is bounded twice. Per annotation: 2000 name-tree nodes and entries, selectors and clips (each object
  once, however many actions name them). Per document: 200 000 of those, and 20 000 assets whose bytes are looked at,
  summed over the annotations (they may share one tree). Past a budget the rest of that annotation, and the annotations
  after it, are listed without assets and with "too much media to list in full". A cut keeps a prefix of what no
  budget would give, so an asset's index, and the route's read of it, do not change.
- CSP: `media-src http://inkwell.localhost/m/ inkwell://localhost/m/` in `csp` and `devCsp`, nothing else new.

## The UI

- A round badge (play triangle, or slashed for not supported with the reason underneath) sits in the middle of the
  annotation's area; an item with no asset at all stays in the panel and gets no badge. Hover shows the format and, once the element has read it, the duration (`MP4 · 1:15`).
- Click: a `<video>` or `<audio>` element replaces the badge over the annotation's rect (audio and tiny rects get a
  minimum size), native controls, `preload="none"`, no `loop`, no autoplay attribute (the click is the play). It is
  placed in page CSS px outside the scaled page space, so the controls stay upright and the same size at any zoom or
  rotation. Muted state is remembered for the session.
- One player at a time. Esc (unless full screen is open), the page scrolling out of view, the page leaving the
  rendered range, closing the tab, or an edit that removes the asset removes it. A player started from the panel is
  removed if it was never on screen after 2 s.
- No iframe, object, embed, `srcdoc` or script ever comes from the PDF; the element's `src` is only the route URL.
- Panel: page, kind, title, status, Play (goes to the page, then plays), Go to; per-asset reasons for items with
  several files.

## Caps

1 000 media annotations per page, 10 000 per document, 100 000 `/Annots` entries screened per page and 500 000 per
document, 50 000 annotations built (MuPDF) per document, 500 assets and 2 000 name-tree nodes per annotation (a visited
set stops a node that lists itself), 32 actions, nesting depth 8. The two document budgets matter because many pages can
share one huge indirect `/Annots` array: each page is charged the array's length, so the listing stops after a few pages.

## Acrobat compatibility

Acrobat Pro plays embedded video and sound (RichMedia, Screen with a rendition, legacy Movie and Sound annotations)
and shows 3D. Inkwell follows its placement (media plays in the annotation's rect) and plays the formats Chromium
decodes natively. It does not run Flash, JavaScript or any embedded program, does not show 3D, does not follow
play-parameter requests such as autoplay or opening a URL, and plays nothing outside the document. pdfcraft keeps
these annotations untouched and has nothing to reuse (its parity table lists multimedia as "preserved, not played").

Not covered: a rendition action triggered from a link or a form button (`/AN` naming a Screen annotation) is not
followed; only the Screen annotation's own `/A` and `/AA` are read. Poster images given as `/Poster` or `/MK` without
an appearance stream are not drawn.

## Windows check

WebView2 must play the H.264 mp4 and the mp3 from the route with seeking, over the annotation, light and dark; see the
sample above. WKWebView (macOS) with a custom scheme was not checked.
