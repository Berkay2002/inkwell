# Inkwell

Local, private PDF app for Windows. Phase 1: the core viewer.

Tauri 2 (WebView2) shell, Rust engine on MuPDF, React + TypeScript + Vite UI. No accounts, no network.

## Layout

| Path | What |
|---|---|
| `src-tauri/src/engine/` | Thread-safe document service: worker pool, display-list cache, tiled renderer, tile LRU, text, search, outline |
| `src-tauri/src/lib.rs` | Tauri commands and the `inkwell://` tile protocol (raw RGBA, no JSON) |
| `src-tauri/src/bin/bench.rs` | Engine benchmark on a generated 1000-page fixture |
| `src/` | React UI: virtualized page list, tile canvases, text layer, tabs, sidebar, command palette |
| `scripts/perf.mjs` | Frame-timing, transport and memory harness driven over WebView2 DevTools; writes `docs/screens/phase1/` |
| `src-tauri/src/recents.rs`, `src/recents.ts`, `src/components/Home.tsx` | Start screen: recent list stored in `<app data>/recents.json`, file stats, first-page thumbnails |
| `scripts/landing-shots.mjs` | Start-screen screenshots (light/dark, empty/filled/drag-over) in an isolated profile; writes `docs/screens/landing/` |
| `vendor/mupdf/` | mupdf 0.8.0 with a one-line MSVC fix (`max_align_t` assertion) and `PdfDocument::permissions` reading the P value of encrypted files (`from_bits_truncate`) |
| `vendor/mupdf-sys/` | mupdf-sys 0.8.0 with JPEG 2000 decoding made thread-parallel (`load-jpx.c`: stock MuPDF decodes JPX under its global FreeType lock, one image at a time) |

## Build and run

Prerequisites: Rust (MSVC), Node + pnpm, LLVM (`LIBCLANG_PATH` pointing at its `bin`), VS Build Tools 2022, WebView2 runtime.
The binary links the dynamic CRT, so the machine needs the VC++ 2015-2022 redistributable.

```
pnpm install
pnpm tauri dev                # dev
pnpm tauri build --no-bundle  # src-tauri\target\release\inkwell.exe
pnpm test                     # layout math (vitest)
cd src-tauri && cargo test --lib
cargo run --release --bin bench -- 1000
```

`inkwell.exe a.pdf b.pdf` opens the files in tabs. `INKWELL_DATA_DIR` moves the recent-files list (and keeps the old localStorage list from being imported): set it whenever a test or benchmark launches the app so it never writes the real recents. Files under an `inkwell-tests` or `inkwell-bench` folder are never listed anyway. `INKWELL_CACHE_MB` sets the tile cache budget (default 64; the app idles near 550 MB in total with WebView2 in low-end-device mode).

## Keys

Ctrl+O open, Ctrl+W close, Ctrl+Tab next tab, Ctrl+K command palette, Ctrl+F find (Enter / Shift+Enter, F3),
Ctrl+G go to page, Ctrl+Up/Down page, Ctrl+= / Ctrl+- / Ctrl+wheel / pinch zoom, Ctrl+0 fit page, Ctrl+1 100%,
Ctrl+2 fit width, Ctrl+R rotate, Ctrl+B sidebar. Arrow keys, PageUp/Down, Home/End scroll natively.

## Design notes

- Pages are cut into 512 px tiles. A tile is requested as `http://inkwell.localhost/t/{doc}/{page}/{rot}/{scale}/{tx}/{ty}?e=&r=&s=`
  and comes back as raw RGBA with `x-tile-w/h` headers; the page's whole-page 320 px preview tile doubles as its thumbnail.
- Every wanted-set change bumps a per-stream epoch. The engine answers queued jobs older than the newest epoch with 410 and the
  client re-requests only what is still wanted, one request per tile at a time.
- Render scale is quantised to quarter-octaves so tiles survive zoom steps; zooming in restretches the old tiles until the
  settled scale arrives, zooming out re-renders immediately.
