// Checks that the reader refits when its stage changes width (docked assistant open/close, floating,
// command bar, sidebar toggle) in Fit width, keeping the page in view; screenshots each step.
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9223'; inkwell.exe some.pdf
//   node scripts/refit-shots.mjs docs/screens/refit 9223
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/refit";
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
  new Promise((res) => {
    const id = ++seq;
    waiting.set(id, (d) => res(d.result));
    ws.send(JSON.stringify({ id, method, params }));
  });
const ev = async (expression) => (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result.value;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const S = "window.__inkwell.useStore";
const A = "window.__inkwell.useAi";
const measure = () =>
  ev(`(() => {
    const v = document.querySelector(".viewer");
    const s = ${S}.getState(); const d = s.docs[s.active];
    const pages = [...document.querySelectorAll(".page")].map((p) => p.getBoundingClientRect());
    const r = v.getBoundingClientRect();
    const first = pages.find((p) => p.bottom > r.top + 1);
    return { vw: v.clientWidth, sw: v.scrollWidth, left: v.scrollLeft, page: d.currentPage, zoom: +d.zoom.toFixed(4),
             yPt: +((r.top - first.top) / d.zoom).toFixed(1), pageL: Math.round(first.left - r.left), pageR: Math.round(r.left + v.clientWidth - first.right) };
  })()`);
const results = [];
const step = async (name, js) => {
  if (js) await ev(js);
  await sleep(900);
  const m = await measure();
  // The top page is fully in view and centred (a wider page elsewhere may still overflow).
  m.ok = m.pageL >= 0 && Math.abs(m.pageL - m.pageR) <= 2;
  results.push({ name, ...m });
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, `${name}.png`), Buffer.from(r.data, "base64"));
};

await ev(`(() => { const s = ${S}.getState(); s.setZoomMode(s.active, "fitWidth"); s.goTo(s.active, 3); ${A}.getState().setOpen(false); return 1 })()`);
await step("0-start");
await step("1-docked-open", `${A}.getState().setMode("docked"); ${A}.getState().setOpen(true)`);
await step("2-docked-closed", `${A}.getState().setOpen(false)`);
await step("3-floating", `${A}.getState().setMode("floating"); ${A}.getState().setOpen(true)`);
await step("4-bar", `${A}.getState().setMode("bar")`);
await step("5-docked-again", `${A}.getState().setMode("docked")`);
await step("6-sidebar-off", `${S}.getState().toggleSidebar("thumbs")`);
await step("7-sidebar-on", `${S}.getState().toggleSidebar("thumbs")`);
await step("8-fit-page", `(() => { const s = ${S}.getState(); s.setZoomMode(s.active, "fitPage"); return 1 })()`);
await step("9-fit-page-closed", `${A}.getState().setOpen(false)`);
await ev(`(() => { const s = ${S}.getState(); s.setZoomMode(s.active, "fitWidth"); return 1 })()`);
console.table(results);
const pagesSeen = new Set(results.map((r) => r.page));
// The reading position (offset into the top page, in points) holds within 2 pt in Fit width.
const drift = Math.max(...results.filter((r) => !r.name.includes("fit-page")).map((r) => Math.abs(r.yPt - results[0].yPt)));
console.log("max drift (pt):", drift);
console.log(results.every((r) => r.ok) && pagesSeen.size === 1 && drift <= 2 ? "PASS" : "FAIL", "pages seen:", [...pagesSeen]);
ws.close();
