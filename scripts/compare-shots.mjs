// Drives a running Inkwell through WebView2's DevTools endpoint: compares two files from the store (formatting, text,
// image and page changes), filters by kind, swaps the sides, then runs a 1200-page comparison to capture progress and
// cancel and to measure how responsive the page stays. Works on generated samples, never on real files.
//   $d = "$env:TEMP\inkwell-cmp"; mkdir $d
//   $env:INKWELL_SEED_DIR = $d; cargo test --lib seed_compare_screens -- --ignored     (from src-tauri)
//   npx vite --port 1420                                  (the debug exe loads the dev server)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:INKWELL_HISTORY_ROOT = "$d\store"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9226'
//   src-tauri\target\debug\inkwell.exe "$d\Lease v1.pdf" "$d\Lease v2.pdf" "$d\Big v1.pdf" "$d\Big v2.pdf"
//   node scripts/compare-shots.mjs docs/screens/compare 9226
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/compare";
const port = process.argv[3] ?? "9226";
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
const until = async (expr, what, limit = 30000, every = 100) => {
  const t0 = Date.now();
  for (;;) {
    const v = await ev(expr);
    if (v) return v;
    if (Date.now() - t0 > limit) throw new Error(`timed out waiting for ${what}`);
    await sleep(every);
  }
};
const theme = (t) => ev(`window.__S.getState().setTheme(${JSON.stringify(t)})`);
const shot = async (name, themes = ["light", "dark"]) => {
  await sleep(700);
  for (const t of themes) {
    await theme(t);
    await sleep(350);
    const r = await send("Page.captureScreenshot", { format: "png" });
    fs.writeFileSync(path.join(out, `${name}-${t}.png`), Buffer.from(r.data, "base64"));
    console.log("screenshot", `${name}-${t}.png`);
  }
  await theme("light");
};
const click = (sel, text) =>
  ev(
    `(() => { const els = [...document.querySelectorAll(${JSON.stringify(sel)})]; const b = ${
      text ? `els.find((e) => e.textContent.includes(${JSON.stringify(text)}))` : "els[0]"
    }; b?.click(); return !!b })()`,
  );

await ev(`(() => { window.__S = window.__inkwell.useStore; window.__H = window.__inkwell.useHistory; return true })()`);
await until(`Object.keys(window.__S.getState().docs).length >= 4`, "four documents", 60000);
const docByName = (n) => ev(`(() => { const d = Object.values(window.__S.getState().docs).find((d) => d.name === ${JSON.stringify(n)}); return d ? { id: d.id, path: d.path } : null })()`);
const known = (p) => ev(`window.__TAURI_INTERNALS__.invoke("known_file", { path: ${JSON.stringify(p)} })`);
const report = {};

await theme("light");
const l1 = await docByName("Lease v1.pdf");
const l2 = await docByName("Lease v2.pdf");
const b1 = await docByName("Big v1.pdf");
const b2 = await docByName("Big v2.pdf");
const f = (r) => known(r.path);

// ---- a small comparison: text, formatting, image, added page ----
await ev(`window.__S.getState().activate(${l2.id})`);
const t1 = await f(l1);
await ev(
  `window.__H.getState().openCompare({ doc: ${l2.id}, a: { kind: "picked", token: ${JSON.stringify(t1.token)} }, b: { kind: "current" }, names: ["Lease v1.pdf", "Lease v2.pdf"], stage: "setup" })`,
);
await until(`!!document.querySelector(".hx-setup") && document.querySelectorAll(".hx-slot .hx-thumb img").length === 2`, "the setup page");
report.setup = await ev(`document.querySelector(".hx-setup").innerText.replace(/\\s+/g, " ")`);
await shot("compare-setup");
if (!(await click(".hx-go"))) throw new Error("no Compare button on the setup page");
await until(`document.querySelectorAll(".hx-rev").length > 0 && !!document.querySelector(".hx-csum")`, "the comparison");
// the pages load when they scroll into view: take the first text change, which scrolls both panes to it
await click(".hx-rev[data-cat=text] .hx-rev-main");
await until(`document.querySelectorAll(".hx-links line").length > 0`, "the connectors", 10000);
report.connectors = await ev(`document.querySelectorAll(".hx-links line").length`);
await until(`document.querySelectorAll(".hx-pthumb .hx-thumb img").length > 0`, "the page strips", 10000);
report.strips = await ev(`[...document.querySelectorAll(".hx-pstrip")].map((s) => s.querySelector(".hx-pstrip-head").innerText.replace(/\\s+/g, " ") + " / " + s.querySelectorAll(".hx-pthumb").length + " thumbs, active " + (s.querySelector(".hx-pthumb.on")?.dataset.p ?? "none"))`);
report.summary = await ev(`document.querySelector(".hx-csum").innerText.replace(/\\s+/g, " ")`);
report.rows = await ev(`[...document.querySelectorAll(".hx-rev")].map((e) => e.innerText.replace(/\\s+/g, " "))`);
report.formatRows = await ev(`[...document.querySelectorAll(".hx-rev[data-cat=formatting]")].map((e) => e.innerText.replace(/\\s+/g, " "))`);
await shot("compare-result");

// ---- filter by kind from the summary chips ----
const all = report.rows.length;
if (!(await click(".hx-csum .hx-chip", "Formatting"))) throw new Error("no Formatting chip in the summary");
await sleep(300);
report.filtered = await ev(`document.querySelectorAll(".hx-rev").length`);
await shot("compare-filtered", ["light"]);
await click(".hx-csum .hx-chip", "Formatting"); // off again
await sleep(200);

// ---- swap the sides: the same pages with the kinds reversed, counted again ----
await click(".hx-swap");
await until(`!!document.querySelector(".hx-csum") && document.querySelectorAll(".hx-rev").length > 0`, "the swapped comparison");
report.swapped = await ev(`document.querySelector(".hx-csum").innerText.replace(/\\s+/g, " ")`);
await shot("compare-swapped", ["light"]);
await click(".hx-swap");
await until(`!!document.querySelector(".hx-csum") && document.querySelectorAll(".hx-rev").length === ${all}`, "swapped back");

// ---- 1200 pages: progress, cancel, and the page keeps painting ----
await ev(`(() => { window.__gap = 0; let last = performance.now(); const tick = (t) => { window.__gap = Math.max(window.__gap, t - last); last = t; requestAnimationFrame(tick) }; requestAnimationFrame(tick); return true })()`);
await ev(`window.__S.getState().activate(${b2.id})`);
const tb1 = await f(b1);
const openBig = () =>
  ev(
    `window.__H.getState().openCompare({ doc: ${b2.id}, a: { kind: "picked", token: ${JSON.stringify(tb1.token)} }, b: { kind: "current" }, names: ["Big v1.pdf", "Big v2.pdf"] })`,
  );
await ev(`window.__gap = 0`);
let t0 = Date.now();
await openBig();
await until(`!!document.querySelector(".hx-cprog")`, "the progress view", 20000, 10);
await sleep(250);
report.progressText = await ev(`document.querySelector(".hx-cprog")?.innerText.replace(/\\s+/g, " ")`);
{
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, "compare-progress-light.png"), Buffer.from(r.data, "base64"));
  console.log("screenshot compare-progress-light.png");
}
await click(".hx-cprog .hx-btn", "Cancel");
await until(`!!document.querySelector(".hx-cmsg")`, "the cancelled message");
report.cancelled = await ev(`document.querySelector(".hx-cmsg").innerText.replace(/\\s+/g, " ")`);
report.cancelMs = Date.now() - t0;
await shot("compare-cancelled", ["light"]);
await ev(`window.__gap = 0`);
t0 = Date.now();
await click(".hx-cmsg .hx-btn", "Compare again");
await until(`!!document.querySelector(".hx-csum") && document.querySelectorAll(".hx-rev").length > 0`, "the 1200-page comparison", 180000, 200);
report.bigMs = Date.now() - t0;
report.bigGapMs = Math.round(await ev(`window.__gap`));
report.bigSummary = await ev(`document.querySelector(".hx-csum").innerText.replace(/\\s+/g, " ")`);
await shot("compare-1200-pages");

await click(".hx-cbar .hx-btn", "Done");
console.log(JSON.stringify(report, null, 2));
ws.close();
