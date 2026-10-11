// Captures the Compress and Recognize-text rail (design/approved/tools-*.png) through WebView2's DevTools
// endpoint, light and dark, on COPIES of docs/study samples (never the user's files). Steps are DOM clicks and
// store calls plus CDP mouse events for the context menu: no OS-level input.
//   copy compress-source.pdf and scan-source.pdf to <dir>; start the app with both:
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9333'; inkwell.exe <dir>/compress-source.pdf <dir>/scan-source.pdf
//   node scripts/tools-shots.mjs docs/screens/tools 9333
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/tools";
const port = process.argv[3] ?? "9222";
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
const shot = async (name, wait = 500) => {
  await sleep(wait);
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  console.log("screenshot", name);
};
const S = "window.__inkwell.useStore";
const T = "window.__inkwell.useTools";
const sheet = () => ev(`(() => { const s = ${T}.getState().sheet; return s && { tool: s.tool, phase: s.phase, progress: s.progress, status: s.status, error: s.error, done: s.donePages.length, compressed: s.compressed && { before: s.compressed.sizeBefore, after: s.compressed.sizeAfter, pct: s.compressed.savedPercent, kept: s.compressed.keptOriginal, visual: s.compressed.visual }, rec: s.recognized && s.recognized.report && { rec: s.recognized.report.pages_recognized, skipped: s.recognized.report.pages_skipped_text, failed: s.recognized.report.pages_failed, words: s.recognized.report.words, undo: s.recognized.state && s.recognized.state.undo } } })()`);
const click = (sel, text) =>
  ev(`(() => { const b = [...document.querySelectorAll(${JSON.stringify(sel)})].find((e) => !${JSON.stringify(text ?? "")} || e.textContent.trim() === ${JSON.stringify(text ?? "")}); if (!b) return false; b.click(); return true })()`);
const theme = (t) => ev(`${S}.getState().setTheme("${t}")`);
/** Polls until `phase` is reached; takes `onRunning` shots on the way (once). */
async function until(phase, onRunning) {
  let shotRunning = false;
  for (let i = 0; i < 1200; i++) {
    const s = await sheet();
    if (s?.phase === phase || s?.phase === "error") return s;
    if (s?.phase === "running" && onRunning && !shotRunning && s.progress > 0.05) {
      shotRunning = true;
      await onRunning();
    }
    await sleep(40);
  }
  throw new Error("timeout waiting for " + phase);
}
const report = {};

await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await theme("light");
await sleep(800);

// ---- compress: Tools menu -> rail
const names = await ev(`(() => { const s = ${S}.getState(); return s.order.map((id) => ({ id, name: s.docs[id].name })) })()`);
report.docs = names;
const compressDoc = names.find((d) => /compress/i.test(d.name)).id;
const scanDoc = names.find((d) => new RegExp(process.env.SCAN_DOC ?? "scan", "i").test(d.name)).id;
await ev(`${S}.getState().activate(${compressDoc})`);
await sleep(900);
await click('button[aria-label="Tools"]');
await sleep(200);
report.menuItems = await ev(`[...document.querySelectorAll('.tools-panel .tool-name')].map((e) => e.textContent.trim())`);
await click(".tools-panel .tool-name", "Compress file");
await sleep(1500);
report.compressOptions = await sheet();
await shot("compress-options-light.png", 300);
await theme("dark");
await shot("compress-options-dark.png", 400);
await theme("light");
await click(".tl-preset", undefined);
await ev(`document.querySelector('.tl-foot .ax-btn.primary').click()`);
report.compressDone = await until("done", async () => {
  await shot("compress-progress-light.png", 0);
});
await shot("compress-done-light.png", 500);
await theme("dark");
await shot("compress-done-dark.png", 400);
await theme("light");
await click('.tl [aria-label^="Close"]');
await sleep(300);

// ---- OCR: page context menu -> rail
await ev(`${S}.getState().activate(${scanDoc})`);
await ev(`${S}.getState().setSidebar("thumbs")`);
await sleep(1200);
const th = await ev(`(() => { const r = document.querySelector(".thumb").getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 } })()`);
await send("Input.dispatchMouseEvent", { type: "mousePressed", x: th.x, y: th.y, button: "right", buttons: 2, clickCount: 1 });
await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: th.x, y: th.y, button: "right", buttons: 0, clickCount: 1 });
await shot("page-menu-light.png", 400);
await click(".ink-menu [role=menuitem], .ink-menu [role=option]", "Recognize text on this page…");
await sleep(800);
// the whole document, not just the page the menu was opened on
await click(".tl .tl-seg button", "All pages");
await sleep(300);
report.ocrOptions = await sheet();
await shot("ocr-options-light.png", 300);
await theme("dark");
await shot("ocr-options-dark.png", 400);
await theme("light");
await ev(`document.querySelector('.tl-foot .ax-btn.primary').click()`);
report.ocrDone = await until("done", async () => {
  await shot("ocr-progress-light.png", 0);
});
await shot("ocr-done-light.png", 500);
await theme("dark");
await shot("ocr-done-dark.png", 400);
await theme("light");

// Undo is one step: the rail closes and the document goes back.
const before = await ev(`(() => { const d = ${S}.getState().docs[${scanDoc}]; return { undo: d.undoLabel, dirty: d.dirty } })()`);
await click(".tl-foot .ax-btn", "Undo");
await sleep(800);
const after = await ev(`(() => { const d = ${S}.getState().docs[${scanDoc}]; return { undo: d.undoLabel, redo: d.redoLabel, dirty: d.dirty } })()`);
report.undo = { before, after, railClosed: await ev(`${T}.getState().sheet === null`) };

// One right-side panel at a time.
await ev(`${T}.getState().open("ocr")`);
await sleep(500);
await ev(`window.__inkwell.useAi.getState().setOpen(true)`);
await sleep(300);
report.exclusive = await ev(`({ rail: ${T}.getState().sheet !== null, assistant: window.__inkwell.useAi.getState().open })`);
await ev(`window.__inkwell.useAi.getState().setOpen(false)`);

await send("Emulation.setFocusEmulationEnabled", { enabled: false });
console.log(JSON.stringify(report, null, 2));
ws.close();
