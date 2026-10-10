// Drives a running Inkwell through WebView2's DevTools endpoint and captures the version history panel, a selected
// version with its changes marked, Compare mode (2-up, Swipe, Onion skin) and the recovery banner, light and dark.
// The scenario is built from copies of docs/study/samples in a temp dir, with its own history store: never the
// user's real store or files.
//   $d = "$env:TEMP\inkwell-hx"
//   $env:INKWELL_SEED_DIR = $d; cargo test --lib seed_screens -- --ignored      (from src-tauri)
//   $env:INKWELL_HISTORY_ROOT = "$d\store"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9222'
//   inkwell.exe "$d\lease.pdf" "$d\notes.pdf"
//   node scripts/history-shots.mjs docs/screens/history
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/history";
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
const until = async (expr, what, limit = 20000) => {
  const t0 = Date.now();
  for (;;) {
    const v = await ev(expr);
    if (v) return v;
    if (Date.now() - t0 > limit) throw new Error(`timed out waiting for ${what}`);
    await sleep(150);
  }
};
const shot = async (name) => {
  await sleep(900); // page images and transitions settle
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  console.log("screenshot", name);
};
const click = (sel, text) =>
  ev(`(() => { const els = [...document.querySelectorAll(${JSON.stringify(sel)})]; const b = ${text ? `els.find((e) => e.textContent.trim() === ${JSON.stringify(text)})` : "els[0]"}; b?.click(); return !!b })()`);
const rect = (sel) => ev(`(() => { const r = document.querySelector(${JSON.stringify(sel)})?.getBoundingClientRect(); return r ? { x: r.x, y: r.y, w: r.width, h: r.height } : null })()`);
const mouse = (type, x, y) => send("Input.dispatchMouseEvent", { type, x, y, button: "none", buttons: 0 });

await ev(`(() => {
  window.__S = window.__inkwell.useStore;
  window.__H = window.__inkwell.useHistory;
  return true;
})()`);
if (!(await ev("!!window.__H"))) throw new Error("window.__inkwell.useHistory is missing: rebuild the app");

const theme = (t) => ev(`window.__S.getState().setTheme(${JSON.stringify(t)})`);
const docId = (name) => ev(`(() => { const d = Object.values(window.__S.getState().docs).find((d) => d.name === ${JSON.stringify(name)}); return d ? d.id : null })()`);
const activate = (id) => ev(`window.__S.getState().activate(${id})`);
const report = {};

await until(`Object.keys(window.__S.getState().docs).length >= 2`, "both documents");
const lease = await until(`(() => { const d = Object.values(window.__S.getState().docs).find((d) => d.name === "lease.pdf"); return d ? d.id : null })()`, "lease.pdf");
const notes = await docId("notes.pdf");
await ev(`window.__H.getState().refresh(${lease})`);
await ev(`window.__H.getState().refresh(${notes})`);

// ---- recovery banner (a crash left an autosave newer than the last save) ----
await theme("light");
await activate(notes);
await until(`window.__H.getState().docs[${notes}]?.state?.recovery`, "the recovery offer");
await shot("recovery-banner-light.png");
report.recovery = await ev(`window.__H.getState().docs[${notes}].state.recovery.label`);

// ---- the panel ----
await activate(lease);
await until(`window.__H.getState().docs[${lease}]?.state?.status === "ready"`, "history ready");
await ev(`window.__H.getState().setOpen(true)`);
await until(`document.querySelectorAll(".hx-item").length >= 3`, "version rows");
report.rows = await ev(`[...document.querySelectorAll(".hx-item .hx-title")].map((e) => e.textContent)`);
await shot("panel-light.png");
await theme("dark");
await shot("panel-dark.png");
await theme("light");

// ---- a selected version: its change list in the row, its changes marked on the pages ----
const versions = await ev(`window.__H.getState().docs[${lease}].versions.map((v) => ({ id: v.id, label: v.label, name: v.name, kind: v.kind.type }))`);
const asst = versions.find((v) => v.label === "Filled tenant details");
const saved = versions.find((v) => v.label === "Saved");
const first = versions.find((v) => v.kind === "import");
await click(".hx-row .hx-title", "Filled tenant details");
await until(`(() => { const c = window.__H.getState().changes["${lease}:${asst.id}"]; return c && c !== "loading" })()`, "its changes");
await until(`document.querySelectorAll(".hx-page img").length > 0 && document.querySelectorAll(".hx-mark").length > 0`, "marks on the page");
report.changes = await ev(`window.__H.getState().changes["${lease}:${asst.id}"].changes.map((c) => c.label)`);
report.marks = await ev(`document.querySelectorAll(".hx-mark").length`);
await shot("version-light.png");
await theme("dark");
await shot("version-dark.png");
await theme("light");
await click(".hx-vbar .hx-btn", "Back to current");
await sleep(300);
report.backToCurrent = await ev(`!document.querySelector(".hx-version") && !!document.querySelector(".viewer")`);

// ---- Compare: the original against the file as it is now ----
await ev(`window.__H.getState().openCompare({ doc: ${lease}, a: { kind: "version", id: ${JSON.stringify(first.id)} }, b: { kind: "current" } })`);
await until(`document.querySelectorAll(".hx-card").length > 0 && document.querySelectorAll(".hx-crow .hx-page img").length >= 2`, "the comparison");
report.compare = await ev(`({ cards: document.querySelectorAll(".hx-card").length, summary: [...document.querySelectorAll(".hx-summary li")].map((e) => e.textContent), cats: [...document.querySelectorAll(".hx-cats button")].map((e) => e.textContent) })`);
await shot("compare-2up-light.png");
await click(".hx-card", undefined);
await shot("compare-2up-selected-light.png");
await theme("dark");
await shot("compare-2up-dark.png");
await theme("light");

// Swipe: the divider follows the pointer 1:1 (real mouse events over CDP)
await click(".hx-seg.inline button", "Swipe");
await until(`document.querySelectorAll(".hx-swipe").length > 0`, "swipe rows");
await sleep(600);
const sw = await rect(".hx-swipe");
await mouse("mouseMoved", sw.x + sw.w * 0.2, sw.y + 120);
await mouse("mouseMoved", sw.x + sw.w * 0.62, sw.y + 120);
await sleep(200);
report.swipeVar = await ev(`getComputedStyle(document.querySelector(".hx-cscroll")).getPropertyValue("--swipe")`);
await shot("compare-swipe-light.png");

// Onion skin
await click(".hx-seg.inline button", "Onion skin");
await until(`document.querySelectorAll(".hx-onion").length > 0`, "onion rows");
await ev(`(() => { const i = document.querySelector(".hx-slider input"); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set; set.call(i, "0.5"); i.dispatchEvent(new Event("input", { bubbles: true })); return true })()`);
await shot("compare-onion-light.png");
await theme("dark");
await shot("compare-onion-dark.png");
await theme("light");

// the saved version against the assistant's: text changes as inline word diffs
await click(".hx-seg.inline button", "2-up");
await ev(`window.__H.getState().setCompare({ a: { kind: "version", id: ${JSON.stringify(first.id)} }, b: { kind: "version", id: ${JSON.stringify(asst.id)} } })`);
await sleep(1500);
await shot("compare-versions-light.png");
await ev(`window.__H.getState().closeCompare()`);

console.log(JSON.stringify(report, null, 2));
ws.close();
