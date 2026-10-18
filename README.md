# Inkwell

A free, open-source alternative to Adobe Acrobat Pro for Windows: every Acrobat Pro tool area, from editing, forms and signatures to redaction, OCR, compare, preflight and print production, running locally with no account and no cloud.

The built-in AI assistant uses your own subscription: it drives the Codex (ChatGPT) or Claude Code CLI you already have
signed in on your PC, with no API key, and by default every edit it proposes waits for your approval
([how it works](docs/acrobat-coverage.md#the-assistant-uses-your-own-ai-subscription)).

Tauri 2 (WebView2) shell, Rust engine on a patched MuPDF, React + TypeScript + Vite UI.

**Status:** early. Windows only, unsigned MSI (see [docs/installer.md](docs/installer.md)), no releases yet: build it from source.

## Privacy

No accounts, no telemetry. The only outside connection is the optional assistant, which runs your own signed-in Codex or
Claude Code CLI.

## Features

Each area has its own doc; [docs/README.md](docs/README.md) lists them all.

How it compares with Acrobat Pro, area by area, including what is still missing: [docs/acrobat-coverage.md](docs/acrobat-coverage.md).

- View and search: tiled renderer, tabs, outline, full-text search ([search](docs/search.md))
- Comments and markup ([comments](docs/comments.md), [threads](docs/threads.md))
- Forms, including form JavaScript in a sandbox ([forms](docs/forms.md), [forms-js](docs/forms-js.md))
- Edit text and images ([text-edit](docs/text-edit.md), [image-edit](docs/image-edit.md))
- Organize pages and portfolios ([organize-plus](docs/organize-plus.md), [portfolios](docs/portfolios.md))
- Sign: fill-and-sign and digital signatures ([sign](docs/sign.md))
- Passwords and permissions ([protect](docs/protect.md)), redaction, compression ([compress-engine](docs/compress-engine.md))
- OCR ([ocr-engine](docs/ocr-engine.md))
- Compare and version history ([compare](docs/compare.md), [history](docs/history.md))
- Convert ([convert](docs/convert.md))
- Print and print production ([print](docs/print.md), [prepress](docs/prepress.md))
- Preflight, PDF/A and accessibility ([pdfa](docs/pdfa.md), [a11y](docs/a11y.md))
- Batch actions ([batch](docs/batch.md))
- Header/footer, watermark, Bates ([marks](docs/marks.md))
- Measure ([measure](docs/measure.md))
- Optional assistant ([ai-actions](docs/ai-actions.md), [ai-claude](docs/ai-claude.md))

## Build and run

Prerequisites:

- Windows 10/11 x64
- Rust stable 1.85+ with the MSVC target (`rustup default stable-x86_64-pc-windows-msvc`); the crate uses edition 2024
- Node 22+ and pnpm 10 (`corepack enable`)
- VS Build Tools 2022 with the C++ workload and a Windows SDK (MuPDF builds with msbuild)
- LLVM/clang for bindgen in `vendor/mupdf-sys`: `winget install LLVM.LLVM`, then `$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'`
- WebView2 runtime (ships with Windows 11)

The binary links the dynamic CRT, so the machine needs the VC++ 2015-2022 redistributable.
The first build compiles MuPDF from source and takes several minutes.

From the repo root:

```
pnpm install
pnpm tauri dev                # dev
pnpm tauri build --no-bundle  # src-tauri\target\release\inkwell.exe
pnpm tauri build              # MSI, see docs/installer.md
pnpm test                     # UI unit tests (vitest)
pnpm build                    # type-check (tsc) and bundle the UI
```

From `src-tauri/`:

```
cargo test --lib
cargo run --release --bin bench -- 1000   # engine benchmark on a generated 1000-page fixture
```

`cargo test --lib` needs `node` on PATH (some tests run small Node fakes of the assistant CLIs).

`inkwell.exe a.pdf b.pdf` opens the files in tabs. `INKWELL_DATA_DIR` moves the recent-files list (and keeps the old localStorage list from being imported): set it whenever a test or benchmark launches the app so it never writes the real recents. Files under an `inkwell-tests` or `inkwell-bench` folder are never listed anyway. `INKWELL_CACHE_MB` sets the tile cache budget (default 64; the app idles near 550 MB in total with WebView2 in low-end-device mode).

## Layout

| Path | What |
|---|---|
| `src-tauri/src/engine/` | Thread-safe document service: worker pool, display-list cache, tiled renderer, tile LRU, text, search, outline |
| `src-tauri/src/lib.rs` | Tauri commands and the `inkwell://` tile protocol (raw RGBA, no JSON) |
| `src-tauri/src/bin/` | Engine benchmarks |
| `src/` | React UI |
| `scripts/` | Performance and screenshot harnesses, icon and installer scripts |
| `docs/` | Feature and engine docs ([index](docs/README.md)) |
| `vendor/mupdf/` | mupdf 0.8.0 with Inkwell patches, listed in [`vendor/mupdf/PATCHES.md`](vendor/mupdf/PATCHES.md) |
| `vendor/mupdf-sys/` | mupdf-sys 0.8.0 with JPEG 2000 decoding made thread-parallel (`load-jpx.c`: stock MuPDF decodes JPX under its global FreeType lock, one image at a time) |
| `third_party/` | Licences and notices for bundled third-party material |

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

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Report security issues privately as described in [SECURITY.md](SECURITY.md).
This project follows a [Code of Conduct](CODE_OF_CONDUCT.md).

## License

Copyright (C) 2026 Berkay.

Inkwell is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public
License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later
version. See [LICENSE](LICENSE).

Inkwell is built on MuPDF (AGPL-3.0, Artifex). The modified vendored sources are in `vendor/`, with their changes listed
in [third_party/mupdf/MODIFICATIONS.md](third_party/mupdf/MODIFICATIONS.md) (summary in [vendor/mupdf/PATCHES.md](vendor/mupdf/PATCHES.md)). Other third-party material:
[third_party/THIRD-PARTY-NOTICES.txt](third_party/THIRD-PARTY-NOTICES.txt).

Adobe and Acrobat are trademarks of Adobe Inc. Inkwell is not affiliated with or endorsed by Adobe.
