// Captures the Organize view (design/approved/organize.png) through WebView2's DevTools endpoint:
// the selection of the design, light and dark, a drag in progress, a rotation, a delete with its
// Undo toast. Steps are store calls, DOM clicks and CDP input events (never OS-level input).
//   python docs/study/samples/_gen/make_lease.py <copy>/"Lease agreement.pdf"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9223'; inkwell.exe <copy>/"Lease agreement.pdf"
//   node scripts/organize-shots.mjs docs/screens/organize 9223
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/organize";
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
const shot = async (name, wait = 700) => {
  await sleep(wait);
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  console.log("screenshot", name);
};
const mouse = (type, x, y, extra = {}) =>
  send("Input.dispatchMouseEvent", { type, x, y, button: "left", buttons: type === "mouseReleased" ? 0 : 1, clickCount: 1, ...extra });
const key = async (k, code, modifiers = 0) => {
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: k, code, windowsVirtualKeyCode: k.length === 1 ? k.toUpperCase().charCodeAt(0) : { Delete: 46, Escape: 27 }[k], modifiers });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, modifiers });
};
const center = (i) =>
  ev(`(() => { const r = document.querySelector('[data-index="${i}"] .org-page').getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 } })()`);
const S = "window.__inkwell.useStore";
const doc = () => ev(`(() => { const s = ${S}.getState(); const d = s.docs[s.active]; return { rev: d.rev, pages: d.pageCount, undo: d.undoLabel, dirty: d.dirty } })()`);
const report = {};

await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await ev(`${S}.getState().setTheme("light")`);
await ev(`${S}.getState().patch(${S}.getState().active, { organize: true })`);
await sleep(800);

// The design's selection: pages 3 and 4, by click and Ctrl+click.
let p = await center(2);
await mouse("mousePressed", p.x, p.y);
await mouse("mouseReleased", p.x, p.y);
p = await center(3);
await mouse("mousePressed", p.x, p.y, { modifiers: 2 });
await mouse("mouseReleased", p.x, p.y, { modifiers: 2 });
report.field = await ev(`document.querySelector(".org-field input").value`);
report.count = await ev(`document.querySelector(".org-count").textContent`);
await shot("organize-light.png", 1200);
await ev(`${S}.getState().setTheme("dark")`);
await shot("organize-dark.png");
await ev(`${S}.getState().setTheme("light")`);

// Typed ranges select pages.
await ev(`(() => { const i = document.querySelector(".org-field input"); i.focus(); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set; set.call(i, "1, 6-8"); i.dispatchEvent(new Event("input", { bubbles: true })); return true })()`);
await sleep(200);
report.typed = await ev(`[...document.querySelectorAll(".org-cell.on")].map((e) => +e.dataset.index + 1)`);
await ev(`document.querySelector(".org-field input").blur()`);

// Drag page 9 to the front: insertion bar and sliding neighbours mid-drag.
p = await center(8);
await mouse("mousePressed", p.x, p.y);
const q = await center(0);
for (let k = 1; k <= 12; k++) await mouse("mouseMoved", p.x + ((q.x - 30 - p.x) * k) / 12, p.y + ((q.y - p.y) * k) / 12);
report.barShown = await ev(`!!document.querySelector(".org-bar")`);
await shot("organize-drag.png", 300);
await mouse("mouseReleased", q.x - 30, q.y);
await sleep(600);
report.afterMove = await doc();

// Rotate the moved page (now page 1, still selected) with Ctrl+R; shot mid-turn.
await ev(`document.querySelector(".org-grid").focus()`);
await key("r", "KeyR", 2);
await shot("organize-rotate.png", 90);
await sleep(600);
report.afterRotate = await doc();

// Delete pages 2-3: the Undo toast.
p = await center(1);
await mouse("mousePressed", p.x, p.y);
await mouse("mouseReleased", p.x, p.y);
p = await center(2);
await mouse("mousePressed", p.x, p.y, { modifiers: 8 });
await mouse("mouseReleased", p.x, p.y, { modifiers: 8 });
await key("Delete", "Delete");
await shot("organize-delete-toast.png", 700);
report.afterDelete = await doc();
await ev(`[...document.querySelectorAll(".org-toast button")].find((b) => b.textContent === "Undo")?.click()`);
await sleep(600);
report.afterUndo = await doc();

// Right click: the page menu.
p = await center(4);
await send("Input.dispatchMouseEvent", { type: "mousePressed", x: p.x, y: p.y, button: "right", buttons: 2, clickCount: 1 });
await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: p.x, y: p.y, button: "right", buttons: 0, clickCount: 1 });
await shot("organize-menu.png", 400);
await key("Escape", "Escape");

// Undo everything; back to Read.
for (let k = 0; k < 4; k++) await ev(`document.querySelector('.org-icon[aria-label="Undo"]')?.click()`), await sleep(300);
report.final = await doc();
await ev(`document.querySelector(".org-done").click()`);
await shot("read-mode-switch.png");
await send("Emulation.setFocusEmulationEnabled", { enabled: false });
console.log(JSON.stringify(report, null, 2));
ws.close();
