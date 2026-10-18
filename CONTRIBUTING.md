# Contributing to Inkwell

Thanks for your interest. Inkwell is Windows only; build setup is in the [README](README.md#build-and-run).

## Before you start

- Open an issue before a large feature or a big refactor, so we can agree on the approach first.
- Bugs: use the bug report template. Never attach a confidential PDF.
- Security issues: do not open a public issue, see [SECURITY.md](SECURITY.md).

## Before you open a pull request

These must pass:

```
pnpm test                       # repo root: vitest
pnpm build                      # repo root: tsc + vite build
cd src-tauri; cargo test --lib  # engine tests
```

Also:

- When a feature changes, update its doc in `docs/<feature>.md`.
- `vendor/` holds patched third-party sources. Change them only when needed, and record every change: a MuPDF change gets
  an entry in [vendor/mupdf/PATCHES.md](vendor/mupdf/PATCHES.md), a `vendor/mupdf-sys` change a note next to its patch;
  and add or update the file's row in [third_party/mupdf/MODIFICATIONS.md](third_party/mupdf/MODIFICATIONS.md) (with the date range).
- New third-party code, fonts or assets need their licence and notice in `third_party/` (and an entry in
  `third_party/THIRD-PARTY-NOTICES.txt`). The licence must be compatible with AGPL-3.0.

## Licence of contributions

Inkwell is licensed under AGPL-3.0-or-later. By submitting a contribution you agree that it is licensed under the same
terms (inbound = outbound). There is no CLA.

## Conduct

This project follows the [Code of Conduct](CODE_OF_CONDUCT.md).
