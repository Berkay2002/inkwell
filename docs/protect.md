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
- **Remove security**: one undo step. A file with an owner (permissions) password needs it, even when its rights
  are all allowed (P = everything): "may change security" is decided by whether the session authenticated as
  *owner* (MuPDF's access flag 4), never by the rights it received. The guess is checked against a fresh copy of
  the document, so a wrong one changes nothing. A file opened with the owner password, or one with no owner
  password, doesn't ask.
- **Owner password**: with only an open password, the file's owner password is set to the open password (what
  Acrobat does, see the study). MuPDF would otherwise hash "" into /O, and qpdf, pikepdf and pypdf accept "" as
  the owner password, so anyone could strip the restrictions.
  The panel (design round protect-r1, variant A) keeps Acrobat's two independent toggles with the full
  permission controls, a predicted-outcome sentence above the button and, when the action is the copy, the line
  "Saves a copy named <name> (protected).pdf. The original is not changed."
- **Other people's files**: a notice above the page says what the file restricts and that the owner's
  restrictions are kept on save; the tab shows a lock; the Security view lists the rights. Saving a protected file
  keeps its encryption and rights (MuPDF `KEEP`), never silently dropping them.
- **Output**: "Protect and save a copy…" asks for a file first (a `pick_save` token), then one command
  (`protect_and_save`) applies the step and saves to it; the original is not changed. "Protect this file
  instead" asks again inline, then does the same with an in-place save. Step and save run under one hold of
  the document's edit lane (and in its UI edit queue), so no edit, save or assistant turn can get between
  them. If the save fails for any reason the command takes exactly that step back out (what Redo had comes
  back too), so nothing stays pending and a later Ctrl+S cannot encrypt the file by surprise; a retry is one
  clean call, and the UI asks `edit_state` for the labels again. Success is one undo step, listed in the version history ("Protect with a
  password (AES-256)"), and the history store keeps its own XChaCha20 crypto. The in-place confirm only says
  the earlier version stays in version history when history is enabled and ready for that file.
- **Restricted files**: extract pages, split, merge and the export family (PDF to Word, images, …) check the
  rights first. A file that forbids copying (extract) or assembling (merge/split) and was not opened with the
  owner password fails with `needs-permissions-password`, and the UI opens the Security view to ask for it,
  rather than writing an unencrypted copy of what the owner restricted.
  Pages copied this way arrive without the signature of a signed source (its signature field stays, empty).

## How it is built
- Security is a property of the *written file*: a `Step.security` (`Set(spec)` or `Remove`) makes the next save a
  full rewrite with those write options. The in-memory snapshots stay as opened and decrypt with the password the
  tab was unlocked with, so rendering never changes.
- `ProtectSpec` (camelCase JSON, unknown fields refused) wipes its passwords on drop and hides them from `Debug`;
  `validate()` refuses: nothing to set, equal passwords, more than 127 bytes (MuPDF's buffer), control characters
  (NUL included: MuPDF takes the password as a C string, so a NUL would silently cut it), restrictions without
  a permissions password, non-keyboard characters with AES-128. Before that, `normalize()` puts both passwords
  in Unicode NFKC, the normalisation SASLprep starts from (it is not full SASLprep: no prohibited-character or
  bidi checks; MuPDF has none either). It is applied when the passwords are *written*. Where a password is
  *checked* (opening the file, and the permissions password of Remove/Change security) the password is tried
  as typed first and then in its SASLprep mapping plus NFKC (`protect::nfkc_form`), because MuPDF itself hashes
  the bytes it is given: "é" typed decomposed, full-width IME letters or a no-break space open a file that was
  protected with the NFKC form, and a file made elsewhere with the raw form still opens as typed. The mapping
  step (RFC 4013) drops the characters RFC 3454 B.1 maps to nothing (soft hyphen, ZWJ, ZWNJ, U+200B, word joiner,
  Mongolian and variation selectors, BOM) and turns the non-ASCII spaces of C.1.2 (U+1680, U+2000..200A, U+202F,
  U+205F, U+3000, no-break space) into U+0020, so a password typed with those opens a file written with the plain
  one. The form that worked is kept for the session.
- After every save the engine reads the file back (`verify`): page count, `/Encrypt` present after a Set and
  absent after a Remove, and that it opens with the password the step just wrote.
- Permission bits follow the PDF spec: print 3, modify 4, copy 5, annotate 6, forms 9, accessibility 10, assemble
  11, high-quality print 12. `Rights::from_p` / `permission()` map both ways; "Pages" = assemble, "Forms" =
  forms, "Comments" = annotate + forms, "All except extracting" = modify + annotate + forms + assemble.
  Checked against Acrobat's own output (`docs/study/11-protect.md`): identical except that Acrobat leaves bit 11
  clear for "All except extracting" (and still reports assembly allowed); we keep it set, which is the more
  permissive and spec-correct choice for strict readers.
- `SecurityInfo` (what the UI reads): method, revision, open password, the rights, `restricted`, `unlocked` (the
  session may change it without asking), `origin` (file / set here / removed here) and `unsaved`.
- The vendored mupdf wrapper's `PdfDocument::permissions()` used to report *everything allowed* for every
  encrypted file (`from_bits(..).unwrap_or(all)`); it now truncates to the known bits.
- Tools that read the file on disk (Compress's audit, estimate and run) use `Engine::file_password`, which is
  the new open password once Save wrote one.

## Commands
`edit_state(doc)` (read-only, for the refresh after a failed command), `security_info(doc)`, `protect_apply(doc, spec, permissionsPassword?)` (applies only, used by tests and the
open-ended flow), `protect_and_save(doc, spec, permissionsPassword?, saveToken?)` (apply + save + rollback, what
the panel calls), `remove_security(doc, permissionsPassword?)`.
Errors: `needs-permissions-password`, `bad-permissions-password` (from the engine); the spec codes in
`SpecError::code()`.

## Gaps
- Inkwell enforces a file's restrictions only where it would write a new file out of the restricted content
  (extract, split, merge, export). Other tools (compress, OCR, redact, print through the OS, copying text) are not
  gated; the notice says what the owner asked for.
- A file whose owner password is "" but which has a user password (written by older Inkwell builds or by qpdf with
  an empty owner password) can't be unlocked as owner here: MuPDF refuses an empty owner password when a user
  password exists, the same as Acrobat does.
- The in-place save's last-resort path (a folder that takes no new files) copies over the target and restores a
  backup copy when the write does not read back; if that restore fails too the error says so, names both
  causes and where the original is kept.
- RC4 files are read and shown but not written (AES only).
- A file opened with only the user password of a document that has restrictions stays restricted; "unlocked" is
  decided from the rights the session received, not from the password's name.
- Restoring an old encrypted version (`reloadInPlace`) can't ask for the password again.
- No batch protect, no certificate (public-key) security, no "encrypt attachments only" / "all except metadata"
  choices (Acrobat's classic dialog has them; it writes `/EncryptMetadata false` for the latter).
- The strength meter is Weak / Fair / Strong over five segments; Acrobat's is Weak / Medium / Strong / Best over
  four (thresholds in the study note). Left as designed.
