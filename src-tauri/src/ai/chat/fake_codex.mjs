// A stand-in for `codex app-server` speaking the same JSONL JSON-RPC, for the Rust tests.
// usage: node fake_codex.mjs <mode> [flagfile]
//   ok        normal answers, JSON for cited tasks, plain text otherwise
//   slow      long answer, 40 ms between deltas (cancel tests)
//   die-once  exits during the first turn if flagfile does not exist yet (creates it), normal afterwards
//   hang      streams two deltas then goes silent
//   fail      the turn fails with an `unauthorized` error
//   approval  sends an approval request mid-turn and only finishes once it was refused
//   slow-start    thread/start answers after 1.5 s
//   stall-start   thread/start is never answered
//   grandchild    starts a child process of its own at initialize and logs its pid
//   tools=<json>  before answering, calls the dynamic tools listed as
//                 [{"tool": "get_text", "arguments": {...}, "delay": ms}] one by one (a server request
//                 `item/tool/call`), logs each reply, and cites a passage found in the replies
import { spawn } from "node:child_process";
import fs from "node:fs";
import readline from "node:readline";

const mode = process.argv[2] ?? "ok";
const flag = process.argv[3];
const script = mode.startsWith("tools=") ? JSON.parse(mode.slice(6)) : null;
let experimental = false;
let srvIds = -1; // server request ids start at 0, like codex's
let threads = 0;
let turns = 0;
const interrupted = new Set();
const replies = new Map();
const out = (o) => process.stdout.write(JSON.stringify(o) + "\n");
const log = (s) => process.stderr.write(s + "\n");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

readline.createInterface({ input: process.stdin }).on("line", (line) => {
  const m = JSON.parse(line);
  if (m.method === undefined) {
    replies.get(m.id)?.(m);
    return;
  }
  const p = m.params ?? {};
  switch (m.method) {
    case "initialize":
      experimental = p.capabilities?.experimentalApi === true;
      log(`initialize experimentalApi=${experimental}`);
      if (mode === "grandchild") {
        const gc = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], { stdio: "ignore" });
        log(`grandchild ${gc.pid}`);
      }
      out({ id: m.id, result: { userAgent: "fake-codex/1", codexHome: "x" } });
      break;
    case "account/read":
      out({
        id: m.id,
        result: {
          account: mode === "loggedout" ? null : { type: "chatgpt", email: "tester@example.com", planType: "plus" },
          requiresOpenaiAuth: true,
        },
      });
      break;
    case "model/list":
      out({
        id: m.id,
        result: {
          data: [
            {
              id: "fake-1",
              model: "fake-1",
              displayName: "Fake One",
              hidden: false,
              isDefault: true,
              defaultReasoningEffort: "low",
              supportedReasoningEfforts: ["low", "medium", "high"].map((e) => ({ reasoningEffort: e, description: e })),
            },
            { id: "fake-hidden", model: "fake-hidden", displayName: "Hidden", hidden: true, isDefault: false, supportedReasoningEfforts: [] },
          ],
          nextCursor: null,
        },
      });
      break;
    case "thread/start": {
      const id = `thr-${++threads}`;
      const names = (p.dynamicTools ?? []).map((t) => t.name).join(",");
      log(`thread/start ${id} sandbox=${p.sandbox} approval=${p.approvalPolicy} ephemeral=${p.ephemeral} envs=${JSON.stringify(p.environments)} model=${p.model ?? "-"} tools=${names}`);
      if (p.dynamicTools && !experimental) {
        out({ id: m.id, error: { code: -32600, message: "thread/start.dynamicTools requires experimentalApi capability" } });
        break;
      }
      if (mode === "stall-start") break; // never answered
      const answer = () => out({ id: m.id, result: { thread: { id }, model: p.model ?? "fake-1" } });
      if (mode === "slow-start") setTimeout(answer, 1500);
      else answer();
      break;
    }
    case "thread/unsubscribe":
      log(`unsubscribed ${p.threadId}`);
      out({ id: m.id, result: { status: "unsubscribed" } });
      break;
    case "turn/interrupt":
      interrupted.add(p.threadId);
      out({ id: m.id, result: {} });
      break;
    case "turn/start":
      void runTurn(m.id, p);
      break;
    default:
      if (m.id !== undefined) out({ id: m.id, error: { code: -32601, message: `unknown ${m.method}` } });
  }
});

async function runTurn(id, p) {
  const n = ++turns;
  const turn = `turn-${n}`;
  const tid = p.threadId;
  const prompt = p.input?.[0]?.text ?? "";
  const markers = prompt.match(/\[p\d+\.\d+\]/g) ?? [];
  log(`turn ${tid} ${turn} effort=${p.effort ?? "-"} model=${p.model ?? "-"} schema=${p.outputSchema ? "yes" : "no"} excerpts=${markers.length}`);
  out({ id, result: { turn: { id: turn, status: "inProgress", items: [] } } });
  out({ method: "turn/started", params: { threadId: tid, turn: { id: turn, status: "inProgress", items: [] } } });

  if (mode === "die-once" && flag && !fs.existsSync(flag)) {
    fs.writeFileSync(flag, "x");
    await sleep(20);
    log("dying now");
    process.exit(3);
  }
  if (mode === "fail") {
    out({
      method: "turn/completed",
      params: { threadId: tid, turn: { id: turn, status: "failed", items: [], error: { message: "Not signed in", codexErrorInfo: "unauthorized" } } },
    });
    return;
  }
  if (mode === "approval") {
    const reply = await new Promise((res) => {
      replies.set("srv-1", res);
      out({ id: "srv-1", method: "item/commandExecution/requestApproval", params: { threadId: tid, turnId: turn, command: "rm -rf /" } });
    });
    log(`approval answered with ${reply.error ? "error" : "result"}`);
  }

  let toolText = "";
  for (const step of script ?? []) {
    if (step.delay) await sleep(step.delay);
    const cid = `c${srvIds + 2}`;
    out({ method: "item/started", params: { threadId: tid, turnId: turn, item: { type: "agentMessage", id: cid, text: "", phase: "commentary" } } });
    out({ method: "item/agentMessage/delta", params: { threadId: tid, turnId: turn, itemId: cid, delta: "Let me look. " } });
    out({ method: "item/completed", params: { threadId: tid, turnId: turn, item: { type: "agentMessage", id: cid, text: "Let me look. ", phase: "commentary" } } });
    const rid = ++srvIds;
    const reply = await new Promise((res) => {
      replies.set(rid, res);
      out({
        id: rid,
        method: "item/tool/call",
        params: { threadId: tid, turnId: turn, callId: `call-${rid}`, namespace: null, tool: step.tool, arguments: step.arguments ?? {} },
      });
    });
    const items = reply.result?.contentItems ?? [];
    const t = items.filter((i) => i.type === "inputText").map((i) => i.text).join("\n");
    const images = items.filter((i) => i.type === "inputImage").length;
    toolText += t + "\n";
    const what = reply.error ? `error:${reply.error.message}` : `success=${reply.result.success} images=${images}`;
    log(`tool-reply ${step.tool} ${what} :: ${t.replace(/\s+/g, " ").slice(0, 90)}`);
  }

  let text;
  if (p.outputSchema) {
    const first = /\[(p\d+\.\d+)\] ([^\n]{0,80})/.exec(toolText || prompt);
    const quote = first ? first[2].trim().split(" ").slice(0, 6).join(" ") : "";
    const answer = `Fake answer about it [1].\nSecond "quoted" line, café 😀 and a \\ backslash [1].`;
    text = JSON.stringify({
      answer,
      citations: first ? [{ n: 1, chunks: [first[1]], quote }] : [],
      followups: ["What else?", "Why so?", "Who wrote it?"],
    });
  } else {
    text = "Plain **markdown** reply.\n\n- one\n- two";
  }
  if (mode === "slow") text = JSON.stringify({ answer: "word ".repeat(400), citations: [], followups: [] });

  out({ method: "item/started", params: { threadId: tid, turnId: turn, item: { type: "agentMessage", id: "i1", text: "", phase: "final_answer" } } });
  const step = mode === "slow" ? 8 : 7;
  const gap = mode === "slow" ? 40 : 3;
  let sent = 0;
  for (let i = 0; i < text.length; i += step) {
    if (interrupted.has(tid)) {
      interrupted.delete(tid);
      out({ method: "turn/completed", params: { threadId: tid, turn: { id: turn, status: "interrupted", items: [] } } });
      return;
    }
    if (mode === "hang" && sent >= 2) return; // silence
    out({ method: "item/agentMessage/delta", params: { threadId: tid, turnId: turn, itemId: "i1", delta: text.slice(i, i + step) } });
    sent++;
    await sleep(gap);
  }
  out({ method: "item/completed", params: { threadId: tid, turnId: turn, completedAtMs: Date.now(), item: { type: "agentMessage", id: "i1", text, phase: "final_answer" } } });
  out({
    method: "thread/tokenUsage/updated",
    params: { threadId: tid, turnId: turn, tokenUsage: { last: { inputTokens: 100, outputTokens: 20, totalTokens: 120 }, total: {} } },
  });
  out({ method: "turn/completed", params: { threadId: tid, turn: { id: turn, status: "completed", items: [], error: null } } });
}
