// Drives a running Inkwell through WebView2's DevTools endpoint and lets the real, logged-in assistant ACT on a
// document: make copies (compress, export), highlight, compare with the saved file, mark and apply a redaction,
// and open a tool. Approvals are the app's own Apply buttons (store events), nothing is OS-level input.
// Works on COPIES of docs/study/samples, never on real files.
//   $d = "$env:TEMP\inkwell-ai-actions"; mkdir $d; copy docs\study\samples\redact-source.pdf $d\report.pdf
//   (debug exe built with --features tauri/custom-protocol after `pnpm build`, so it needs no dev server)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:INKWELL_HISTORY_ROOT = "$d\store"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9246'
//   src-tauri\target\debug\inkwell.exe "$d\report.pdf"
//   node scripts/ai-actions-shots.mjs docs/screens/ai-actions 9246
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/ai-actions";
const port = process.argv[3] ?? "9246";
fs.mkdirSync(out, { recursive: true });

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const ws = new WebSocket(targets.find((t) => t.type === "page").webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let seq = 0;
const waiting = new Map();
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  waiting.get(d.id)?.(d);
};
const send = (method, params = {}) =>
  new Promise((res, rej) => {
    const id = ++seq;
    waiting.set(id, (d) => (d.error ? rej(new Error(JSON.stringify(d.error))) : res(d.result)));
    ws.send(JSON.stringify({ id, method, params }));
  });
const ev = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? "eval failed");
  return r.result.value;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const shot = async (name) => {
  await sleep(700); // tiles + transitions settle
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  console.log("screenshot", name);
};

await ev(`(() => {
  window.__ai = window.__inkwell.useAi;
  window.__S = window.__inkwell.useStore;
  window.__R = window.__inkwell.useRedact;
  window.__doc = () => window.__S.getState().active;
  window.__last = () => { const m = window.__ai.getState().chats[window.__doc()]?.messages ?? []; return m[m.length - 1]; };
  return true;
})()`);

const theme = (t) => ev(`window.__S.getState().setTheme(${JSON.stringify(t)})`);
const click = (sel, text) =>
  ev(`(() => { const els = [...document.querySelectorAll(${JSON.stringify(sel)})]; const b = ${text ? `els.find((e) => e.textContent.trim().startsWith(${JSON.stringify(text)}))` : "els[els.length - 1]"}; b?.click(); return !!b })()`);

const snapshot = () =>
  ev(`(() => {
    const m = window.__last();
    if (!m || m.role !== "assistant") return null;
    return {
      state: m.state, text: m.text, error: m.error ?? null, files: m.files ?? [],
      waiting: m.activity.filter((a) => a.state === "waiting").map((a) => ({ id: a.id, tool: a.tool, label: a.label, detail: a.detail, class: a.class })),
      tools: m.activity.map((a) => ({ tool: a.tool, class: a.class, state: a.state, label: a.label, detail: a.detail })),
    };
  })()`);

/** Asks, presses Apply on every card that waits (a card is photographed light and dark first), and resolves with
 *  the finished message. `decide` false presses Reject instead. */
const turn = async (name, question, { decide = true, limitMs = 240000 } = {}) => {
  await ev(`window.__ai.getState().ask(window.__doc(), { kind: "ask", question: ${JSON.stringify(question)} })`);
  const t0 = Date.now();
  const cards = [];
  for (;;) {
    const m = await snapshot();
    for (const w of m?.waiting ?? []) {
      cards.push(w);
      const n = cards.length;
      await shot(`${name}-card${n}-light.png`);
      await theme("dark");
      await shot(`${name}-card${n}-dark.png`);
      await theme("light");
      await ev(`window.__ai.getState().decide(window.__doc(), ${JSON.stringify(w.id)}, ${decide})`);
    }
    if (m && m.state !== "streaming") return { ...m, cards };
    if (Date.now() - t0 > limitMs) throw new Error(`${name}: timed out`);
    await sleep(300);
  }
};

const report = {};
const keep = (name, m) => {
  report[name] = { state: m.state, error: m.error, answer: m.text, cards: m.cards, tools: m.tools, files: m.files };
  console.log(name, m.state, JSON.stringify(m.tools.map((t) => `${t.tool}:${t.state}`)));
};

// ---- setup: docked, Ask mode, light ----
await ev(`(() => { const a = window.__ai.getState(); a.setSettings({ consent: true, mode: "docked", float: null, permissions: "ask" }); a.setOpen(true); a.clear(window.__doc()); return true })()`);
await theme("light");
await sleep(1500);
if (process.env.AI_LIVE) {
  // a real model: use one the signed-in account can run (the user's own Codex config may name one it cannot)
  const st = await ev(`(async () => { window.__ai.getState().refreshStatus(); for (let i = 0; i < 60; i++) { const s = window.__ai.getState().status; if (s && s !== "loading") return s; await new Promise((r) => setTimeout(r, 500)); } return null })()`);
  console.log("status", JSON.stringify(st));
  const models = st?.models ?? [];
  const pick = process.env.AI_MODEL ?? (models.find((m) => m.isDefault ?? m.is_default) ?? models[0])?.id;
  if (pick) await ev(`window.__ai.getState().setSettings({ model: ${JSON.stringify(pick)} })`);
  report.model = pick ?? null;
  console.log("model", pick);
}
await shot("start-light.png");

// ---- a copy: compress (the document is still the saved file) ----
let m = await turn("compress", "Make a smaller copy of this document.");
keep("compress", m);
await shot("compress-done-light.png");

// ---- a new file: export the text ----
m = await turn("export", "Export the text of pages 1 to 2 to a text file.");
keep("export", m);
await shot("export-done-light.png");

// ---- an edit in the document: highlights, one undo step ----
m = await turn("highlight", "Highlight every date in this document.");
keep("highlight", m);
await shot("highlight-done-light.png");

// ---- compare what the turn changed with the saved file ----
m = await turn("compare", "What is different between this document and the saved file?");
keep("compare", m);
await shot("compare-done-light.png");

// ---- redaction: mark first, review in the rail, then apply (permanent, asks in every mode) ----
// AI_LIVE=1 (a real model): the brief's own question, with the name of the person in the sample
m = await turn("redact-mark", process.env.AI_LIVE ? "Redact my name, Jane Q. Tester, everywhere in this document." : "Redact every social security number in this document.");
keep("redact-mark", m);
await shot("redact-marked-light.png");
// the "Review in Redact" link under the answer opens the rail on the marks
const linked = await click(".ax-activity .ax-btn", "Review in Redact");
report.reviewLink = linked;
await sleep(1200);
await shot("redact-marks-rail-light.png");
await theme("dark");
await shot("redact-marks-rail-dark.png");
await theme("light");
// docked, the rail replaced the assistant: bring the assistant back for the next turn
await ev(`window.__R.getState().setOpen(false); window.__ai.getState().setOpen(true)`);
await sleep(600);

m = await turn("redact-apply", "Apply the redactions now.");
keep("redact-apply", m);
report.redactOutcome = await ev(`(() => { const d = window.__R.getState().docs[window.__doc()]; return d ? { phase: d.phase, outcome: d.outcome ? { status: d.outcome.status, marks: d.outcome.marks, leaks: d.outcome.verify?.leaks?.length ?? null } : null } : null })()`);
await sleep(800);
await shot("redact-applied-light.png");
await theme("dark");
await shot("redact-applied-dark.png");
await theme("light");
// the verification of what was applied is one click away
await click(".ax-activity .ax-btn", "Review in Redact");
await sleep(1200);
await shot("redact-verify-rail-light.png");
await theme("dark");
await shot("redact-verify-rail-dark.png");
await theme("light");
await ev(`window.__R.getState().setOpen(false); window.__ai.getState().setOpen(true)`);
await sleep(600);

// ---- a tool for the user ----
m = await turn("open-tool", "Open the password protection tool for me.");
keep("open-tool", m);
await shot("open-tool-light.png");

// ---- the made file opens from the button under the answer ----
const before = await ev(`Object.keys(window.__S.getState().docs).length`);
await ev(`window.__S.getState().setActive?.(Number(Object.keys(window.__S.getState().docs)[0]))`);
report.openedFile = false;
const offered = await ev(`(() => { for (const c of Object.values(window.__ai.getState().chats)) for (const m of c.messages) if (m.files?.length) return m.files[0]; return null })()`);
if (offered) {
  await ev(`window.__S.getState().openPath(${JSON.stringify(offered.path)}, ${JSON.stringify(offered.token)})`);
  await sleep(2000);
  const after = await ev(`Object.keys(window.__S.getState().docs).length`);
  report.openedFile = { name: offered.name, docsBefore: before, docsAfter: after };
  await shot("made-file-opened-light.png");
}

fs.writeFileSync(path.join(out, "report.json"), JSON.stringify(report, null, 1));
console.log(JSON.stringify(report.redactOutcome), JSON.stringify(report.openedFile));
ws.close();
