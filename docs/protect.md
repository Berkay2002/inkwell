# Protect (passwords and permissions) and Remove security

Code: `src-tauri/src/protect/` (`mod.rs` the types and the read of /Encrypt, `app.rs` the commands, `tests.rs`),
the edit-core steps in `src-tauri/src/engine/edit/security.rs`, UI in `src/components/ProtectPanel.tsx`,
`src/lib/protectStore.ts` and `src/components/protect.css`. Screens: `docs/screens/protect/`
(`node scripts/protect-shots.mjs`). Study of Acrobat's Protect: `docs/study/11-protect.md`. Design round:
`design/rounds/protect-r1/`.

## What it does
- **Protect**: an open password and/or a permissions password; printing none / low / high; changes none / pages /
  forms / comments / all except extracting pages; copying; screen readers (copying implies them); AES-256 (V5/R6,
  the default) or AES-128 (V4/R4) for old readers. The engine is MuPDF's own writer (`pdf_write_options`
  encryption); pdfcraft's crypt crate was not needed.
- **Remove security**: one undo step. A file with restrictions needs its permissions password (checked against a
  fresh copy of the document, so a wrong guess changes nothing); a file opened with that password, or one with no
  restrictions, doesn't ask.
- **Other people's files**: a notice above the page says what the file restricts and that the owner's
  restrictions are kept on save; the tab shows a lock; the Security view lists the rights. Saving a protected file
  keeps its encryption and rights (MuPDF `KEEP`), never silently dropping them.
- **Output**: "Protect and save a copy…" asks for a file first (a `pick_save` token), then applies the step and
  saves to it; the original is not changed. "Protect this file instead" asks again inline, then does a normal
  Save. Either way the step is one undo step, listed in the version history ("Protect with a password
  (AES-256)"), and the history store keeps its own XChaCha20 crypto.

## How it is built
- Security is a property of the *written file*: a `Step.security` (`Set(spec)` or `Remove`) makes the next save a
  full rewrite with those write options. The in-memory snapshots stay as opened and decrypt with the password the
  tab was unlocked with, so rendering never changes.
- `ProtectSpec` (camelCase JSON, unknown fields refused) wipes its passwords on drop and hides them from `Debug`;
  `validate()` refuses: nothing to set, equal passwords, more than 127 bytes (MuPDF's buffer), restrictions without
  a permissions password, non-keyboard characters with AES-128.
- Permission bits follow the PDF spec: print 3, modify 4, copy 5, annotate 6, forms 9, accessibility 10, assemble
  11, high-quality print 12. `Rights::from_p` / `permission()` map both ways; "Pages" = assemble, "Forms" =
  forms, "Comments" = annotate + forms, "All except extracting" = modify + annotate + forms + assemble.
- `SecurityInfo` (what the UI reads): method, revision, open password, the rights, `restricted`, `unlocked` (the
  session may change it without asking), `origin` (file / set here / removed here) and `unsaved`.
- The vendored mupdf wrapper's `PdfDocument::permissions()` used to report *everything allowed* for every
  encrypted file (`from_bits(..).unwrap_or(all)`); it now truncates to the known bits.
- Tools that read the file on disk (Compress's audit, estimate and run) use `Engine::file_password`, which is
  the new open password once Save wrote one.

## Commands
`security_info(doc)`, `protect_apply(doc, spec, permissionsPassword?)`, `remove_security(doc, permissionsPassword?)`.
Errors: `needs-permissions-password`, `bad-permissions-password` (from the engine); the spec codes in
`SpecError::code()`.

## Gaps
- Inkwell shows a file's restrictions but does not enforce them in its own UI (it will still print through the
  OS, copy text, etc.). The notice says what the owner asked for.
- RC4 files are read and shown but not written (AES only).
- A file opened with only the user password of a document that has restrictions stays restricted; "unlocked" is
  decided from the rights the session received, not from the password's name.
- Restoring an old encrypted version (`reloadInPlace`) can't ask for the password again.
- No batch protect, no certificate (public-key) security, no "encrypt attachments only" / "all except metadata"
  choices (Acrobat's classic dialog has them).
