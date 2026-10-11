// A stand-in for the `claude` CLI (Claude Code) speaking the stream-json protocol seen on 2.1.296,
// for the Rust tests. usage: node fake_claude.mjs <mode> [claude's own arguments...]
//   ok            normal answers, JSON for cited tasks, markdown otherwise
//   slow          long answer, 40 ms between deltas; a control `interrupt` ends the turn (cancel tests)
//   deaf          like slow, but ignores `interrupt` (the provider must kill it)
//   die-once:<f>  exits during the first turn if file <f> does not exist yet (creates it)
//   hang          streams two deltas then goes silent
//   loggedout     `auth status` says so, and a turn fails with authentication_failed
//   apikey        the init event says the key source is an API key
//   limit         the turn fails with a rate limit
//   tools=<json>  before answering, calls Inkwell's MCP tools listed as
//                 [{"tool": "test_edit", "arguments": {...}}] through the server in --mcp-config
//                 (a step {"noauth": true} sends a request without the bearer token instead)
// FAKE_CLAUDE_LOG=<file>: appends one JSON line per start (arguments, the environment's names);
// <file>.trace gets one line per message in/out and every crash, for failing tests.
import fs from "node:fs";
import readline from "node:readline";

const mode = process.argv[2] ?? "ok";
const args = process.argv.slice(3);
const script = mode.startsWith("tools=") ? JSON.parse(mode.slice(6)) : null;
const trace = (s) => {
  if (process.env.FAKE_CLAUDE_LOG) fs.appendFileSync(process.env.FAKE_CLAUDE_LOG + ".trace", `${process.pid} ${s}\n`);
};
const out = (o) => {
  if (o.type !== "stream_event") trace(`out ${o.type}/${o.subtype ?? ""}`);
  process.stdout.write(JSON.stringify(o) + "\n");
};
const log = (s) => {
  trace(`log ${s}`);
  process.stderr.write(s + "\n");
};
// a crash says why on stderr's last line (the provider shows that line) and in the trace
for (const ev of ["uncaughtException", "unhandledRejection"]) {
  process.on(ev, (e) => {
    log(`fake claude crashed: ${String(e?.stack ?? e).replace(/\s*\n\s*/g, " | ")}`);
    process.exit(1);
  });
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const arg = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};

if (process.env.FAKE_CLAUDE_LOG) {
  fs.appendFileSync(
    process.env.FAKE_CLAUDE_LOG,
    JSON.stringify({ args, env: Object.keys(process.env), cwd: process.cwd() }) + "\n",
  );
}

// one-shot answers are written synchronously: an exit right after an async write to a pipe can lose it
const say = (text, code) => {
  fs.writeSync(1, text + "\n");
  process.exit(code);
};
if (args[0] === "--version") say("9.9.9 (Fake Claude Code)", 0);
if (args[0] === "auth" && args[1] === "status") {
  if (mode === "loggedout") say(JSON.stringify({ loggedIn: false, authMethod: "none", apiProvider: "firstParty" }), 1);
  say(
    JSON.stringify({ loggedIn: true, authMethod: "claude.ai", apiProvider: "firstParty", email: "tester@example.com", subscriptionType: "max" }),
    0,
  );
}

const sessionId = arg("--resume") ?? "fake-session-1";
const model = arg("--model") ?? "default";
const effort = arg("--effort") ?? "-";
let interrupted = false;
let turns = 0;
let toolIds = 0;
let mcpReady = false;

readline.createInterface({ input: process.stdin }).on("line", (line) => {
  if (!line.trim()) return;
  const m = JSON.parse(line);
  trace(`in ${m.type}/${m.request?.subtype ?? ""}`);
  if (m.type === "control_request") {
    const sub = m.request?.subtype;
    if (sub === "initialize") {
      out({
        type: "control_response",
        response: {
          subtype: "success",
          request_id: m.request_id,
          response: {
            models: [
              { value: "default", displayName: "Default (recommended)", description: "x", supportsEffort: true, supportedEffortLevels: ["low", "medium", "high", "xhigh", "max"] },
              { value: "sonnet", displayName: "Sonnet 5.5", description: "x", supportsEffort: true, supportedEffortLevels: ["low", "medium", "high"] },
              { value: "claude-haiku-4-5-20251001", displayName: "Haiku 4.5", description: "x" },
            ],
            account: { email: "tester@example.com", subscriptionType: "max", apiProvider: "firstParty" },
            claude_code_version: "9.9.9",
          },
        },
      });
    } else if (sub === "interrupt") {
      log("interrupt received");
      if (mode !== "deaf") interrupted = true;
      out({ type: "control_response", response: { subtype: "success", request_id: m.request_id, response: { still_queued: [] } } });
    }
    return;
  }
  if (m.type === "user") {
    const text = (m.message?.content ?? []).filter((c) => c.type === "text").map((c) => c.text).join("\n");
    void runTurn(text);
  }
});

function init(keySource) {
  out({
    type: "system",
    subtype: "init",
    session_id: sessionId,
    apiKeySource: keySource,
    model,
    tools: [],
    mcp_servers: [{ name: "inkwell", status: "connected" }],
  });
}

function stream(textChunks, stop) {
  out({ type: "stream_event", event: { type: "message_start", message: { id: `msg-${turns}`, role: "assistant" } } });
  out({ type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } } });
  for (const t of textChunks) out({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: t } } });
  out({ type: "stream_event", event: { type: "content_block_stop", index: 0 } });
  out({ type: "stream_event", event: { type: "message_delta", delta: { stop_reason: stop }, usage: { output_tokens: 5 } } });
  out({ type: "stream_event", event: { type: "message_stop" } });
}

async function mcp(method, params, auth = true) {
  const cfg = JSON.parse(fs.readFileSync(arg("--mcp-config"), "utf8")).mcpServers.inkwell;
  const headers = { "Content-Type": "application/json", Accept: "application/json, text/event-stream" };
  if (auth) headers.Authorization = cfg.headers.Authorization;
  const id = ++toolIds;
  const r = await fetch(cfg.url, { method: "POST", headers, body: JSON.stringify({ jsonrpc: "2.0", id, method, params }) });
  if (!r.ok) return { status: r.status };
  return { status: r.status, body: await r.json() };
}

async function runTurn(prompt) {
  const n = ++turns;
  const markers = prompt.match(/\[p\d+\.\d+\]/g) ?? [];
  log(`turn ${n} session=${sessionId} model=${model} effort=${effort} excerpts=${markers.length} first=${prompt.includes("Document: ")} changed=${prompt.includes("document changed since")}`);
  const started = Date.now();
  init(mode === "apikey" ? "ANTHROPIC_API_KEY" : "none");

  if (mode.startsWith("die-once:")) {
    const flag = mode.slice(9);
    if (!fs.existsSync(flag)) {
      fs.writeFileSync(flag, "x");
      await sleep(20);
      log("dying now");
      process.exit(3);
    }
  }
  if (mode === "loggedout") {
    out({ type: "assistant", message: { content: [{ type: "text", text: "Not logged in · Please run /login" }] }, error: "authentication_failed" });
    out({ type: "result", subtype: "success", is_error: true, result: "Not logged in · Please run /login", num_turns: 1, duration_ms: 3 });
    return;
  }
  if (mode === "limit") {
    out({ type: "rate_limit_event", rate_limit_info: { status: "rejected", resetsAt: 1 } });
    out({ type: "assistant", message: { content: [{ type: "text", text: "You've hit your limit" }] }, error: "rate_limit" });
    out({ type: "result", subtype: "success", is_error: true, result: "You've hit your limit · resets 5pm", num_turns: 1, duration_ms: 3 });
    return;
  }

  let toolText = "";
  for (const step of script ?? []) {
    if (step.delay) await sleep(step.delay);
    if (!mcpReady) {
      await mcp("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "claude-code", version: "9.9.9" } });
      const list = await mcp("tools/list", {});
      log(`tools=${(list.body?.result?.tools ?? []).map((t) => t.name).join(",")}`);
      mcpReady = true;
    }
    if (step.noauth) {
      const r = await mcp("tools/list", {}, false);
      log(`noauth status=${r.status}`);
      continue;
    }
    stream(["Let me look. "], "tool_use");
    const useId = `toolu_${n}_${++toolIds}`;
    out({ type: "assistant", message: { content: [{ type: "tool_use", id: useId, name: `mcp__inkwell__${step.tool}`, input: step.arguments ?? {} }] } });
    const r = await mcp("tools/call", { name: step.tool, arguments: step.arguments ?? {}, _meta: { "claudecode/toolUseId": useId } });
    const res = r.body?.result;
    const t = (res?.content ?? []).filter((c) => c.type === "text").map((c) => c.text).join("\n");
    const images = (res?.content ?? []).filter((c) => c.type === "image").length;
    toolText += t + "\n";
    log(`tool-reply ${step.tool} isError=${res?.isError} images=${images} :: ${t.replace(/\s+/g, " ").slice(0, 90)}`);
    out({ type: "user", message: { content: [{ type: "tool_result", tool_use_id: useId, content: t }] } });
  }

  const cited = prompt.includes("Reply with the JSON object only");
  let text;
  if (cited) {
    const first = /\[(p\d+\.\d+)\] ([^\n]{0,80})/.exec(toolText || prompt);
    const quote = first ? first[2].trim().split(" ").slice(0, 6).join(" ") : "";
    text = JSON.stringify({
      answer: `Fake answer about it [1].\nSecond "quoted" line, café 😀 and a \\ backslash [1].`,
      citations: first ? [{ n: 1, chunks: [first[1]], quote }] : [],
      followups: ["What else?", "Why so?", "Who wrote it?"],
    });
  } else {
    text = "Plain **markdown** reply.\n\n- one\n- two";
  }
  if (mode === "slow" || mode === "deaf") text = JSON.stringify({ answer: "word ".repeat(400), citations: [], followups: [] });

  const step = mode === "slow" || mode === "deaf" ? 8 : 7;
  const gap = mode === "slow" || mode === "deaf" ? 40 : 3;
  out({ type: "stream_event", event: { type: "message_start", message: { id: `msg-${n}-final`, role: "assistant" } } });
  out({ type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } } });
  let sent = 0;
  for (let i = 0; i < text.length; i += step) {
    if (interrupted) {
      interrupted = false;
      out({ type: "user", message: { content: [{ type: "text", text: "[Request interrupted by user]" }] } });
      out({ type: "result", subtype: "error_during_execution", is_error: true, result: null, num_turns: 1, duration_ms: Date.now() - started });
      return;
    }
    if (mode === "hang" && sent >= 2) return; // silence
    out({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: text.slice(i, i + step) } } });
    sent++;
    await sleep(gap);
  }
  out({ type: "stream_event", event: { type: "content_block_stop", index: 0 } });
  out({ type: "stream_event", event: { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 20 } } });
  out({ type: "stream_event", event: { type: "message_stop" } });
  out({ type: "assistant", message: { content: [{ type: "text", text }] } });
  out({ type: "rate_limit_event", rate_limit_info: { status: "allowed" } });
  out({
    type: "result",
    subtype: "success",
    is_error: false,
    result: text,
    num_turns: 1 + (script?.length ?? 0),
    duration_ms: Date.now() - started,
    duration_api_ms: 10,
    total_cost_usd: 0.001,
    usage: { input_tokens: 100, output_tokens: 20, cache_read_input_tokens: 7, cache_creation_input_tokens: 3 },
  });
}
