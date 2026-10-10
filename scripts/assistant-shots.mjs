// Drives a running Inkwell through WebView2's DevTools endpoint and captures the AI assistant in its
// three layouts (docked, floating, command bar), light and dark, plus a citation jump and the setup
// states. Real questions go to the logged-in Codex; UI steps are in-app store events, DOM clicks, or
// CDP mouse events on the floating sheet's header (never OS-level input).
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
  await sleep(700); // tiles + transitions settle
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
const mode = async (m) => {
  await ev(`window.__ai.getState().setMode(${JSON.stringify(m)})`);
  await sleep(900); // the viewer re-fits its width to the new layout before anything jumps
  // Exactly one layout is on screen at a time: the one that was asked for.
  const live = await ev(`({
    docked: document.querySelectorAll(".ax-dock").length,
    floating: document.querySelectorAll(".ax-panel.floating").length,
    bar: document.querySelectorAll(".ax-bar, .ax-bar-sheet").length > 0 ? 1 : 0,
  })`);
  const shown = Object.keys(live).filter((k) => live[k] > 0);
  if (shown.length !== 1 || shown[0] !== m) throw new Error(`layout ${m} requested, on screen: ${JSON.stringify(live)}`);
  console.log("layout", m, "only");
};
const ask = (task) => ev(`window.__ai.getState().ask(window.__doc(), ${JSON.stringify(task)})`);
const click = (sel, text) =>
  ev(`(() => { const els = [...document.querySelectorAll(${JSON.stringify(sel)})]; const b = ${text ? `els.find((e) => e.textContent.trim() === ${JSON.stringify(text)})` : "els[els.length - 1]"}; b?.click(); return !!b })()`);
const rect = (sel) =>
  ev(`(() => { const r = document.querySelector(${JSON.stringify(sel)})?.getBoundingClientRect(); return r ? { x: r.x, y: r.y, w: r.width, h: r.height } : null })()`);
const mouse = (type, x, y) => send("Input.dispatchMouseEvent", { type, x, y, button: "left", buttons: type === "mouseReleased" ? 0 : 1, clickCount: 1 });
const report = {};

// ---- docked (the default) ----
await ev(`(() => { const a = window.__ai.getState(); a.setSettings({ consent: false, mode: "docked", float: null }); a.setOpen(true); a.clear(window.__doc()); return true })()`);
await theme("light");
await sleep(1500);
await shot("docked-empty-light.png");

await ask({ kind: "summarizeDocument" });
await sleep(300);
await shot("docked-consent-light.png");
await ev(`window.__ai.getState().grantConsent()`);
await sleep(1500);
await shot("docked-streaming-light.png");
report.summarize = await answered();

// selection actions (a store event stands in for dragging a selection across the page)
await ev(`window.__ai.getState().setSelection({ text: "A supplier shortage of lidar modules delayed 140 units in the third quarter, and we ended the year with a backlog of 310 orders.", page: 0 })`);
await sleep(300);
await shot("docked-selection-light.png");
await click(".ax-selstrip .ax-chip", "Explain");
report.explain = await answered();

await ask({ kind: "ask", question: "Why did gross margin improve, and how long can the company run on its cash?" });
report.question = await answered();
await shot("docked-answer-light.png");

// citation jump: the first source row of the last answer
await click(".ax-source");
await sleep(1200);
await shot("docked-citation-light.png");

await theme("dark");
await sleep(500);
await shot("docked-citation-dark.png");
await ev(`window.__ai.getState().clearMark()`);
await shot("docked-answer-dark.png");

// ---- floating ----
await mode("floating");
await ev(`window.__ai.getState().clearMark()`);
await click(".ax-source");
await sleep(900);
await shot("floating-dark.png");
await click(".ax-seg button", "Summary");
await shot("floating-summary-dark.png");
await click(".ax-seg button", "Chat");
await theme("light");
await shot("floating-light.png");

// drag the sheet by its header and resize it from the corner, with real mouse events over CDP
const h = await rect(".ax-panel.floating .ax-head");
await mouse("mousePressed", h.x + 60, h.y + 20);
await mouse("mouseMoved", h.x - 140, h.y + 60);
await mouse("mouseReleased", h.x - 140, h.y + 60);
const moved = await ev(`window.__ai.getState().settings.float`);
const rz = await rect(".ax-rz.se");
await mouse("mousePressed", rz.x + 8, rz.y + 8);
await mouse("mouseMoved", rz.x + 8 + 60, rz.y + 8 - 40);
await mouse("mouseReleased", rz.x + 68, rz.y - 32);
const resized = await ev(`window.__ai.getState().settings.float`);
report.floatGeometry = { start: h, moved, resized, stored: await ev(`localStorage.getItem("inkwell.ai")`) };
await shot("floating-moved-light.png");

// ---- command bar ----
await ev(`window.__ai.getState().setSettings({ float: null })`);
await mode("bar");
await ev(`window.__ai.getState().clearMark()`);
await click(".ax-source");
await sleep(900);
await shot("bar-light.png");
await theme("dark");
await shot("bar-dark.png");
await ev(`window.__ai.getState().clearMark(); window.__ai.getState().clear(window.__doc())`);
await sleep(600);
await shot("bar-empty-dark.png");
await theme("light");
await shot("bar-empty-light.png");

// ---- narrow window, docked ----
await mode("docked");
await send("Emulation.setDeviceMetricsOverride", { width: 720, height: 760, deviceScaleFactor: 1, mobile: false });
await sleep(800);
await shot("docked-narrow-light.png");
await mode("bar");
await sleep(500);
await shot("bar-narrow-light.png");
await send("Emulation.clearDeviceMetricsOverride");
await mode("docked");

// ---- setup states (store injection; the real status is restored afterwards) ----
for (const [name, status] of [
  ["docked-not-installed-light.png", { installed: false, loggedIn: false, account: null, plan: null, models: [], error: null }],
  ["docked-not-logged-in-light.png", { installed: true, loggedIn: false, account: null, plan: null, models: [], error: null }],
]) {
  await ev(`window.__ai.setState({ status: ${JSON.stringify(status)}, chats: {} })`);
  await sleep(300);
  await shot(name);
}
await ev(`window.__ai.getState().refreshStatus()`);
await sleep(1500);

fs.writeFileSync(path.join(out, "latency-report.json"), JSON.stringify(report, null, 1));
console.log(
  JSON.stringify(
    Object.fromEntries(Object.entries(report).filter(([, v]) => v.timing).map(([k, v]) => [k, { ...v.timing, citations: v.citations, chars: v.text.length }])),
    null,
    1,
  ),
);
console.log(JSON.stringify(report.floatGeometry));
ws.close();
