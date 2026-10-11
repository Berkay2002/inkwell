# Claude as the assistant's second provider

The assistant (`src-tauri/src/ai/`) answers through the user's own subscription, with the CLI they
already have installed and signed in. Codex (ChatGPT) was the only provider; Claude (Claude Code)
is the second. The user picks one per document; Codex stays the default.

Tested against **Claude Code 2.1.296** on Windows 11. The protocol below is what that version
writes; Inkwell never relies on anything it did not see there, and treats an event it does not know
as noise.

## What Inkwell does and does not do

Hard line, enforced in `ai/claude/exe.rs` and covered by tests:

* never reads, copies, refreshes, stores or prints Claude credentials or tokens (the login stays
  where `claude login` put it; only the CLI touches it);
* never sets `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL` or
  `CLAUDE_CODE_OAUTH_TOKEN` for the child;
* the child's environment is an **allowlist**, not a filter: only the variables a Windows program
  needs (PATH, PATHEXT, SystemRoot, TEMP/TMP, USERPROFILE/HOME, APPDATA/LOCALAPPDATA, ProgramData,
  the ProgramFiles family, LANG/LC_ALL, the proxy and CA-bundle variables; on Linux and macOS also
  USER, LOGNAME, TMPDIR, SHELL, TZ and `LC_*` / `XDG_*`) plus `CLAUDE_CONFIG_DIR` and
  `CLAUDE_CODE_GIT_BASH_PATH` reach it. Every other variable of Inkwell's process, including any
  `CLAUDE*` / `ANTHROPIC*` one (an Inkwell started from a Claude Code terminal would pass on that
  session's ids and message socket) and every unrelated secret, stays out;
* when the user's own environment sets one of the variables above (or switches to Bedrock / Vertex /
  Foundry), Claude is **refused** with a message naming the variable (never its value): the answer
  would not be billed to their Claude plan. Codex is unaffected;
* never uses `--bare`, never modifies or bundles the binary. The executable is found, not shipped:
  the file the user picked, then `INKWELL_CLAUDE`, `PATH`, `%USERPROFILE%\.local\bin`, the npm global
  folder. An npm `claude.cmd` shim is resolved to the real `claude.exe` it starts; batch files are
  never run.
* the picked file never comes from the webview. "Choose claude.exe…" opens the native file dialog
  from Rust (`ai_pick_claude_path`); Rust checks that it is an absolute local path (no `\\server\share`
  or device path), named `claude.exe` / `claude` (or the npm `claude.cmd` shim), and that
  `--version` answers `… (Claude Code)`, then keeps it in `claude.json` in the app config folder and
  restores it, vetted again, at startup. `ai_claude_path` reads it back, `ai_clear_claude_path` forgets
  it. Nothing about it is in `localStorage` (a path saved there by an earlier build is ignored);
* Claude Code writes its usual local session transcripts for a chat (under `~/.claude/projects`),
  as it does for any `claude -p` run, and Inkwell resumes a conversation from that id. Inkwell itself
  persists nothing of the chat; only the path of a picked `claude` is stored;
* every value on the child's command line is attached to its flag (`--model=<id>`, `--effort=<level>`,
  `--resume=<uuid>`), the model id has to be one of the CLI's own list or plain letters, digits and
  `. _ - [ ]` that does not begin with a dash, and a conversation id has to be a UUID: the webview
  cannot turn a value into another option (`--model --bare`).

## The child

One long-lived `claude` per open document chat, in a private temp directory (its working directory,
holding an `mcp.json`), started with `env_clear` plus the filtered environment:

```
claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages
       --tools "" --strict-mcp-config --mcp-config <dir>\mcp.json --allowedTools mcp__inkwell__*
       --setting-sources "" --disable-slash-commands --permission-prompts none
       --system-prompt <BASE_INSTRUCTIONS> [--model=<id>] [--effort=<level>] [--resume=<session_id>]
       [--autocompact 100k]
```

* `--tools ""`: none of Claude Code's own tools (no shell, no file access). It can only call what
  Inkwell's MCP server lists.
* `--setting-sources ""` and `--disable-slash-commands`: none of the user's settings, hooks, plugins,
  skills or CLAUDE.md files reach the child.
* `--permission-prompts none`: a call that would still prompt is denied, never a wait nobody answers.
  (Present in 2.1.296's `--help`; `--permission-mode` is left at its default, because the value
  `default` is not among that version's documented choices.)
* `--autocompact 100k`: the CLI's own switch for the window after which it compacts the conversation
  itself (`claude --help`: "auto, or 100k–1M tokens"; 2.1.296 rejects anything else at parse time with
  "It must be 'auto', or between 100k and 1M"). It is a flag on the child, so no settings file of the
  user's is touched, and no environment variable is involved. It is only passed when the CLI's own
  `--help` lists it (asked once per executable): an older CLI would refuse to start with an unknown
  option. 100k is the smallest value the CLI accepts.
* The child lives in a Job object with kill-on-close: it dies with Inkwell however Inkwell dies.
* A turn is one `user` message on stdin; the `system/init` event at the start of every turn carries
  the `session_id` (kept for `--resume` if the child has to be restarted) and `apiKeySource`. If
  that says anything but `none` / OAuth, the turn is refused as above.
* Cancel: a `control_request` `interrupt`; the child stays alive and the turn ends as `Cancelled`.
  An interrupt not answered within 5 s kills the child (the next turn resumes with `--resume`).

### Events mapped to `ChatEvent`

| Claude Code (stream-json) | ChatEvent |
|---|---|
| start of turn | `Status(reading)` while the document is prepared, `Status(connecting)` while the child starts, `Status(thinking)` once the question is sent |
| `stream_event` `content_block_delta` `text_delta` | `Delta` |
| text streamed before a tool call in the same turn | `Reset` (new event: the lead-in is not the answer) |
| `assistant` `tool_use` + the MCP `tools/call` | `Activity` (waiting / running / done / failed / rejected) |
| `result` `success` | `Done` with the answer, citations, follow-ups, timings and `usage` |
| `result` `error_during_execution` after an interrupt | `Cancelled` |
| auth error, `rate_limit`, hung child, env refusal | `Error { hint: login / limit / dead / env }` |

The answer contract is the same as Codex's: cited tasks answer with the JSON object of
`text::CITE_RULES`, parsed tolerantly (code fences allowed), partial text streamed from the
`answer` field.

## Tools (MCP)

`ai/tools.rs` is the one registry; both providers serve all of it. The MCP server
(`ai/claude/mcp.rs`) is streamable HTTP with JSON responses:

* bound to `127.0.0.1` on an OS-picked port, only for the life of that child;
* every request needs the bearer token made for that child (256 random bits; it is in the private
  `mcp.json`, never on a command line), a `Host` naming that exact address, and no `Origin` header;
* a call runs through `ai/gate.rs`, the same gate Codex's dynamic tools use: permission mode
  (`ask` / `auto` / `full`), the Apply / Reject card, the activity trail, the tool's own class
  (Read / Edit / Destructive). A new tool added to the registry needs no provider code;
* the approval wait outlives Claude Code's MCP tool timeout by design: `MCP_TOOL_TIMEOUT` is set
  above the approval limit (4 min) so a slow user is not turned into a failure by the CLI.

## Edits keep the conversation

An edit to the document used to start a new Codex thread. Now, for **both** providers, the thread
and the model's memory of the chat stay; the excerpts it was sent are forgotten (their ids may mean
something else) and the next question is prefixed with `text::CHANGED_NOTE`:
"The document changed since your earlier reads; re-read before relying on them."

## Status and sign-in

`status` runs `claude --version` and `claude auth status` (JSON on stdout), and asks for the model
list with the `initialize` control request of a short-lived, tool-less child (cached for 10 min).
Three states, all in the panel:

* not installed: an install link (copyable) and "Choose claude.exe…" (the native file dialog; the
  chosen file is shown with a "Forget" button);
* not signed in: "Run `claude login` in a terminal" with a copy button; Inkwell never opens a login
  flow, never sees the password or token;
* signed in: the plan type (`max`, `pro`, …) next to the provider name in the picker.

## UI

* Provider picker (Codex | Claude) in the panel header of all three layouts and in the "…" menu,
  per document; the last choice is remembered for new documents (`inkwell.ai` in localStorage).
  Switching starts a new chat (a thread cannot move between CLIs) and is refused while an answer
  streams.
* The model list and effort come from the provider (Claude's from the CLI's own `initialize`
  answer, with a fixed fallback); each provider keeps its own choice. A model id or effort from the
  other provider is never sent.
* Claude starts on **Haiku, medium effort** (`claudeModel` "haiku", `claudeEffort` "medium") for new
  installs and for settings saved while nothing had been chosen (stored `null`); a choice made in the
  picker is kept. If the CLI's list does not offer the model, the CLI's own default is used.
* The provider's mark (OpenAI for Codex, Claude for Claude Code; simple-icons, CC0, drawn in the text
  colour, `providerMarks.tsx`, noted in `third_party/THIRD-PARTY-NOTICES.txt`) is on the toolbar toggle,
  the picker, its menu rows and the sign-in / consent cards.
  Screens (`scripts/provider-shots.mjs`, the real status of the installed CLI):
  `docs/screens/assistant/provider-claude-{light,dark}.png` and `provider-picker-claude-{light,dark}.png`.
* Consent is per provider: "Send this document to Anthropic?" for Claude, with the note
  "Uses your Claude Code login; usage counts against your Claude plan."

## Tests

`cargo test --lib ai::` with `ai/claude/fake_claude.mjs` (a node script speaking the protocol
above): event mapping, streaming of the cited answer, a tool call through the MCP server with
approve and deny, the missing bearer token, cancel (interrupt and the kill after an ignored
interrupt), `--resume` with the id, a child that dies once, a hung child, credential variables
stripped (and only the allowlist passed), a forced API key refused (`ANTHROPIC_API_KEY`, and
`apiKeySource` reported by the CLI), not installed, not logged in, usage limit, a model id that
looks like an option refused before a child starts, a non-UUID conversation id not resumed, a file
that is not Claude Code (or not named like it, relative, or a network path) never chosen. The Codex tests are unchanged except the replaced
"edited document gets a new thread" (see above). Frontend: `src/lib/aiStore.test.ts`.

## Deviations from the brief, for the record

* `ChatEvent::Reset` is new (see the table).
* `Status` gained `version`; `ai_status`, `ai_prepare` and `ai_ask` take an optional `provider`
  (default Codex); `ai_pick_claude_path`, `ai_claude_path` and `ai_clear_claude_path` are new (the
  first draft had a webview-supplied `ai_set_claude_path`, removed after the security review); the
  managed state is `ai::Hub`, not `Assistant`.
* Editing keeps the Codex thread too (see above).
* No production Edit or Destructive tool exists on main yet, so the approval path is exercised with
  tools registered by the tests and the live check.

## Live check

Run by hand with `cargo test --lib ai::claude::tests::live -- --ignored --nocapture` against the
installed, signed-in **Claude Code 2.1.296** (plan `max`, `apiKeySource` none), on a short synthetic
contract (the provider only ever sees extracted text, never the file, so no user file was involved),
`add_note` being a test-only Edit tool in `Ask` mode:

| step | result | usage in the `result` event |
|---|---|---|
| question "When must the supplier deliver?" | streamed, cited `[1]`, 3.2 s (first token 2.4 s) | 2 in / 146 out tokens, 2906 cache-creation, 1 turn |
| tool call, Apply | card waited, ran, answer "added", 3.3 s | 4 in / 99 out, 5816 cache-read + 831 cache-creation, 2 turns |
| tool call, Reject | card waited, "no note was added" (the model saw the refusal), 4.0 s | 4 in / 172 out, 7018 cache-read + 412 cache-creation, 2 turns |
| cancel after the first delta | `Cancelled`, the same child still alive for the next question | — |

The CLI's own `initialize` answer listed 13 models (aliases `default`, `opus`, `fable`, `sonnet`,
`haiku`, plus pinned ids) with their effort levels; `claude-haiku-4-5-20251001` has none. The alias
`haiku` is shown by the CLI as **"Haiku 5.5"** (efforts low to max), so the new default reads
"Haiku 5.5 · Medium" in the picker. The child was started with `--autocompact 100k`: the CLI's
`--help` listed the flag, it was accepted, and all four steps above ran with it.
