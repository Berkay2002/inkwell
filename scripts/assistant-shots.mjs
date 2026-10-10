// Drives a running Inkwell through WebView2's DevTools endpoint and captures the AI assistant panel
// (light/dark, streaming answer, citation jump, selection actions, setup states). Real questions go to
// the logged-in Codex; every UI step is an in-app store event or a DOM click, never an OS-level input.
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9222'; inkwell.exe report.pdf
//   node scripts/assistant-shots.mjs docs/screens/assistant
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/assistant";
fs.mkdirSync(out, { recursive: true });

const targets = await (await fetch("http://127.0.0.1:9222/json")).json();
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
  await sleep(600); // tiles + transitions settle
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  console.log("screenshot", name);
};

await ev(`(() => {
  window.__ai = window.__inkwell.useAi;
  window.__S = window.__inkwell.useStore;
  window.__doc = () => window.__S.getState().active;
  window.__last = () => { const m = window.__ai.getState().chats[window.__doc()]?.messages ?? []; return m[m.length - 1]; };
  return true;
})()`);

/** Resolves with the finished assistant message (timing included). */
const answered = async (limitMs = 120000) => {
  const t0 = Date.now();
  for (;;) {
    const m = await ev(`(() => { const m = window.__last(); return m && m.role === "assistant" ? { state: m.state, text: m.text, timing: m.timing, citations: m.citations.length, error: m.error } : null })()`);
    if (m && m.state !== "streaming") return m;
    if (Date.now() - t0 > limitMs) throw new Error("timed out waiting for the answer");
    await sleep(250);
  }
};
const theme = (t) => ev(`window.__S.getState().setTheme(${JSON.stringify(t)})`);
const ask = (task) => ev(`window.__ai.getState().ask(window.__doc(), ${JSON.stringify(task)})`);
const report = {};

// reset to a known state
await ev(`(() => { const a = window.__ai.getState(); a.setSettings({ consent: false }); a.setOpen(true); a.clear(window.__doc()); return true })()`);
await theme("light");
await sleep(1500);
await shot("01-empty-light.png");

// consent card (first question)
await ask({ kind: "summarizeDocument" });
await sleep(300);
await shot("02-consent-light.png");
await ev(`window.__ai.getState().grantConsent()`);
await sleep(1500);
await shot("03-streaming-light.png");
report.summarize = await answered();
await shot("04-summary-light.png");

await ask({ kind: "ask", question: "Why did gross margin improve, and how long can the company run on its cash?" });
report.question = await answered();
await shot("05-answer-light.png");

// citation jump: the first source row of the last answer
await ev(`document.querySelector('.msg.bot:last-of-type .source')?.click()`);
await sleep(1200);
await shot("06-citation-jump-light.png");

// selection actions (a store event stands in for dragging a selection across the page)
await ev(`window.__ai.getState().setSelection({ text: "A supplier shortage of lidar modules delayed 140 units in the third quarter, and we ended the year with a backlog of 310 orders.", page: 0 })`);
await sleep(300);
await shot("07-selection-light.png");
await ev(`[...document.querySelectorAll('.selstrip .chip')].find((b) => b.textContent === 'Explain')?.click()`);
report.explain = await answered();
await shot("08-explain-light.png");

// dark theme
await theme("dark");
await sleep(500);
await shot("09-explain-dark.png");
await ev(`document.querySelector('.msg.bot:last-of-type .source')?.click()`);
await ask({ kind: "ask", question: "What are the main risks and what is the outlook for fiscal 2026?" });
report.risks = await answered();
await ev(`document.querySelector('.msg.bot:last-of-type .source')?.click()`);
await sleep(1200);
await shot("10-citation-jump-dark.png");

// narrow window
await send("Emulation.setDeviceMetricsOverride", { width: 720, height: 760, deviceScaleFactor: 1, mobile: false });
await sleep(800);
await shot("11-narrow-dark.png");
await send("Emulation.clearDeviceMetricsOverride");

// setup states (store injection; the real status is restored afterwards)
await theme("light");
for (const [name, status] of [
  ["12-not-installed-light.png", { installed: false, loggedIn: false, account: null, plan: null, models: [], error: null }],
  ["13-not-logged-in-light.png", { installed: true, loggedIn: false, account: null, plan: null, models: [], error: null }],
]) {
  await ev(`window.__ai.setState({ status: ${JSON.stringify(status)}, chats: {} })`);
  await sleep(300);
  await shot(name);
}
await ev(`window.__ai.getState().refreshStatus()`);
await sleep(1500);

fs.writeFileSync(path.join(out, "latency-report.json"), JSON.stringify(report, null, 1));
console.log(JSON.stringify(Object.fromEntries(Object.entries(report).map(([k, v]) => [k, { ...v.timing, citations: v.citations, chars: v.text.length }])), null, 1));
ws.close();
