# Assistant actions

The assistant (Codex and Claude, one shared registry, no provider code) can change the open document, not only
read it. Every change waits for the user's approval on a card in the panel, goes through the document's own edit
lane, and is exactly one undo step; a whole turn undoes as one unit. Source: `src-tauri/src/ai/actions.rs`
(tools), `ai/tools.rs` (registry, classes), `ai/mod.rs` (`ActingHost`), `src/lib/aiStore.ts` (UI glue).

Acrobat's AI Assistant cannot act on the file; asking Inkwell's assistant to "redact my name" marks every
match, shows them in the Redact rail for review, and applies on the user's word.

## How a change happens

1. The model calls a tool (`redact_find_and_mark`, `add_comment`, ...). The turn's gate checks the tool's
   **class** against the user's permission mode and shows an Apply / Reject card whose headline is the tool's
   `label` ("Permanently redact 3 marks on pp. 1-3"). The permission check is never in the tool.
2. On Apply the tool runs through `DocHost::lane()` (`Lane { eng, doc, source, edited }`): the same
   `Engine::apply_custom` / `apply_from` lane the UI uses, under `EditSource::Assistant { turn }`. Nothing here
   writes the file; Save is still the user's.
3. The tool returns compact text for the model plus `UiAction::Edited { state, redacted }`. The chat event
   reaches `aiStore.adopt`, which hands the new generation to `applyState` (pages, undo label, redaction marks,
   comments, search hits refresh like after any edit) and refreshes the Forms rail's list when it is loaded.
4. After a change, page images the model asks for are rendered from the document as it is now.

### Classes and modes

| Class | Asks in Ask | Auto | Full | Used for |
|---|---|---|---|---|
| `read` | no | no | no | listing marks, comments, form fields |
| `edit` | yes | no | no | marking, comments, form fill, rotate, move, insert blank |
| `destructive` | yes | yes | no | delete pages |
| `permanent` | yes | yes | **yes** | `redact_apply`: cannot be taken back from the saved file |

## The tools (phases 1 and 2)

| Tool | Class | What it does |
|---|---|---|
| `redact_find_and_mark` | edit | `text` / `pattern` (email, phone, personnummer, ssn, iban, card) / `regex`, optional `pages`, `case_sensitive`, `whole_word`. One mark per match (`redact::app::mark_hits`). Skips matches already marked. Pattern matches are masked in the reply (a regex is at most 200 characters, 1 MB compiled). Over 500 matches is an `Err`: narrow it. |
| `redact_mark_area` | edit | a rectangle of a page as fractions 0-1 of its size (for pictures, stamps, signatures) |
| `redact_list_marks` | read | ids (`p3#41`), page, marked text (as it is in the document: the user needs it to review, and it is what the card shows), and `marks_digest` |
| `redact_unmark` | edit | `marks` ids or `all` (not both) |
| `redact_apply` | permanent | applies every mark with the Sanitize options (default all on; unknown option names are an `Err`), in memory: the file on disk is untouched until the user saves. Takes `expected_marks`, `pages` and `marks_digest` exactly as `redact_list_marks` gave them; the digest is checked before the card is shown and again inside the edit lane's lock, so the user approves exactly the marks that get applied. The card lists the first marked texts, every sanitize option that is on ("delete ALL comments and markup", "strip metadata and all hidden data") and the ones kept. Verification report goes to the Redact rail. An engine refusal (recoverable content) is an `Err` that leaves the marks. |
| `list_comments` | read | ids, kind, author, text, reply parent |
| `add_comment` | edit | `kind`: highlight / underline / strikeout / squiggly on every match of `find` (one step), `note` at text or a page point, `reply` to a comment id |
| `list_form_fields` | read | ids (`p2#17`), name, type, value, choices, read-only, required |
| `fill_form_fields` | edit | `{ field: name or id, value }[]` in one all-or-nothing step; a no-op is an `Err` |
| `organize_pages` | edit | `rotate`, `move`, `insert_blank` |
| `delete_pages` | destructive | keeps at least one page |

The system prompt (`ai/text.rs::BASE_INSTRUCTIONS`) tells the model it can do these with approval, to mark first
and let the user review, and to call `redact_apply` only when asked.

## Phase 3: the rest of the app

The assistant covers most of the All tools list with coarse tools (34 in all, kept under 40): one tool per job, not
one per button. `src/lib/aiCoverage.json` is the contract: every tool id of `toolItems.ts` and `ToolId` is mapped to
the assistant tool that does it, or excluded with a reason; `aiCoverage.test.ts` (ids) and a Rust test
(`registry` == `ai::tools::builtin()`) fail when a tool is added to the app and not to the file.
Source: `ai/toolbox.rs` (changes and checks that stay in the document), `ai/jobs.rs` (tools that take time or make
files).

| Tool | Class | What it does |
|---|---|---|
| `page_marks` | edit | `kind` header_footer (six places, `<<1 of n>>` tokens), watermark, background, bates; `action` add / update / remove; `pages`. Add on a page that has the kind is an `Err` (use update). Picture marks need a file the user picks: not offered. |
| `set_document_properties` | edit | title, author, subject, keywords; a no-op is an `Err` |
| `edit_text` | edit | replace one piece of text (`find` must appear once on the page) keeping font, size and colour; text in a scan or in a font that cannot be edited is an `Err` that says so |
| `sanitize_document` | destructive | metadata, hidden layers, scripts and actions, attached files (on) / hidden text, ALL comments, bookmarks (off); a full-rewrite snapshot, so the removed data is gone from the saved file |
| `check_accessibility` / `fix_accessibility` | read / edit | the a11y checker's list; safe fixes (title, language, tab order, tooltips, figure alt, autotag) as one step |
| `check_preflight` | read | PDF/A, PDF/UA, PDF/X check: failing rules and what a conversion could settle |
| `open_tool` | read | opens protect, security, merge, search-advanced (and the tools it can also run) for the user |
| `list_open_documents` / `compare_documents` | read | ids and names; changes between this document and the saved file or another open one |
| `compress_document` | edit | a smaller copy next to the document; wants the saved file (unsaved changes are an `Err`) |
| `ocr_document` | edit | invisible text layer, one undo step; pages that have text are skipped; Windows or Tesseract engine |
| `export_document` | edit | text, HTML, Word, images, extract_images into a new file or folder next to the document |
| `convert_to_standard` | edit | a converted copy for PDF/A, PDF/UA or PDF/X plus a recheck; the original stays |
| `create_blank_pdf` | edit | a new blank PDF (1-500 pages) offered as a separate document |

Rules for the job tools: output is always a new file next to the document under a free name (`unique_stem`:
`a (compressed).pdf`, `a (compressed) (2).pdf`), never over a file; progress goes to the activity trail line while
it runs; the panel's Stop ends the job and leaves nothing half-written; a made file is offered under the answer as
**Open `<name>`** (`UiAction::File`, opened with its token); `open_tool` runs the tool as the All tools list would
(`UiAction::OpenTool`, `aiOpenTool.ts`). Excluded for good (reasons in `aiCoverage.json`): digital signing and
certifying, passwords and permissions, printing, overwriting the original, opening links, and anything that needs a
file the user must pick (picture marks, insert image, merge).

## Rules every action tool follows

- One call, one undo step; a call that changes nothing is `Err` (and pushes no step).
- Signatures are read once (`src-tauri/src/signed.rs`): a signature field with a value, `/Perms /DocMDP`, or
  `SigFlags`. What a change may do then follows the certification: no certification: content changes
  (`redact_apply`, `organize_pages`, `delete_pages`) are refused with "This document is digitally signed ... save
  a copy of the file and make the change there", markup and form fills are incremental updates and go in;
  DocMDP P=1: nothing; P=2: form fills only, no comments or marks; P=3: forms and comments. A form field a
  signature locked (`/Lock`, FieldMDP: All / Include / Exclude) is not filled. A signed document whose save would
  be a full rewrite (repaired on open, or redacted) refuses every change.
- The refusal comes **before** the approval card (`Tool.check`), so the user is never asked to approve what
  cannot happen. The same check runs again when the tool runs.
- The card must say what happens: `fill_form_fields` shows `name = value` per field, `add_comment` says "every
  match (N)" or "the first match" and the note text, `redact_apply` the marked texts and sanitize options.
- Document text in a result (matched text, comments, field values) is data: inside `<<<DOCUMENT ... DOCUMENT>>>`
  and defanged, like every excerpt.
- Arguments are named in the user's terms (1-based pages, fractions of the page); ids come from the `list_*` tools.

## UI

- Approval card: `ApprovalCard` (class `permanent` is styled like `destructive`).
- "Review in Redact" button under the answer when a redaction tool changed something this turn
  (`touchedRedaction`): opens the rail on the marks, or on the verification of a redaction just applied.
- A redaction the assistant applied opens the Redact rail on its verify view, unless the assistant is docked
  (the rail would replace it; the button is there instead).
- A long job shows its progress in the one-line trail (`Make a compressed copy (smallest) (Compressing: 40%)`).
- A file the assistant made is an **Open `<name>`** button under the answer (`Msg.files`, once per path).

## Known limits

- `get_text` / `search` read the text of the turn's start. After a change in the same turn they show the document
  as it was; action results say so. Page images are current.
- Nothing is saved by the assistant. "Redacted" means redacted in the open document and checked; Save writes it.
- Regex uses Rust syntax and a 1 MB compiled-size limit.
- `compress_document`, `compare_documents` (with the saved file) and `export_document` work on the saved file or
  the live document as stated in their descriptions; the assistant cannot save, so it asks the user to.
- `convert_to_standard` applies only the safe fixes; what still fails is reported, never hidden.

## Verification (2026-10-11)

- `cargo test --lib ai::` 118 passed; `pnpm tsc --noEmit`, `pnpm vitest run` (413) clean.
- Live through the real app (debug exe, scratch data dir, a copy of `docs/study/samples/redact-source.pdf`,
  `scripts/ai-actions-shots.mjs`, screens in `docs/screens/ai-actions/`): compress and export (cards, files made
  next to the copy, "Open report (compressed).pdf" opens it), highlight, compare with the saved file, mark SSNs,
  Review in Redact, apply (permanent card, "5 marks redacted, none of the marked text is left in the file"), and
  `open_tool`. The model was **not** the real Codex: the account was at its usage limit until 2026-10-12 00:06, so
  a scripted stand-in (the tests' `fake_codex.mjs` speaking the same app-server protocol, calling the tools a model
  would call for each question) drove the turns. The tools, cards, engine and UI are the real ones; whether a
  model picks these tools for these questions is still to be checked with Codex.
