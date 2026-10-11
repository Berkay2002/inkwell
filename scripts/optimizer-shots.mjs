// Drives a running Inkwell through WebView2's DevTools endpoint: Tools > Compress > Advanced on a copy of a
// docs/study sample (never the original): the space-usage table, the settings sections, the live estimate, a
// saved preset, and the result with the before/after table. Screens (light and dark) go to docs/screens/optimizer.
//   $d = "$env:TEMP\inkwell-optimizer"; mkdir $d; copy docs\study\samples\compress-source.pdf $d
//   npx vite --port 1422 --strictPort                       (the debug exe was built with devUrl 1422)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:INKWELL_HISTORY_ROOT = "$d\store"
//   $env:WEBVIEW2_USER_DATA_FOLDER = "$d\wv2"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9227'
//   src-tauri\target\debug\inkwell.exe "$d\compress-source.pdf"
//   node scripts/optimizer-shots.mjs docs/screens/optimizer 9227       (OPT_REPORT=<file> also writes the JSON report)
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/optimizer";
const port = process.argv[3] ?? "9227";
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
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }).catch((e) => {
    throw new Error(`${e.message} in: ${expression.slice(0, 120)}`);
  });
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
const text = (sel) => ev(`document.querySelector(${JSON.stringify(sel)})?.innerText.replace(/\\s+/g, " ") ?? null`);
/** Click the checkbox whose label text contains `label` (only when its state differs from `on`). */
const check = (label, on = true) =>
  ev(`(() => { const l = [...document.querySelectorAll(".tl-check")].find((e) => e.textContent.includes(${JSON.stringify(label)})); const i = l?.querySelector("input"); if (!i) return null; if (i.checked !== ${on}) i.click(); return i.checked })()`);
const section = (title, open = true) =>
  ev(`(() => { const s = [...document.querySelectorAll("details.tl-sec")].find((d) => d.querySelector(".tl-sec-title")?.textContent === ${JSON.stringify(title)}); if (!s) return false; s.open = ${open}; return true })()`);
const report = {};

await ev(`(() => { window.__S = window.__inkwell.useStore; window.__T = window.__inkwell.useTools; return true })()`);
await until(`Object.keys(window.__S.getState().docs).length >= 1`, "the sample document", 60000);
await theme("light");
const id = await ev(`window.__S.getState().order[0]`);
await ev(`window.__S.getState().activate(${id})`);

// ---- Presets tab first, then Advanced ----
await ev(`window.__T.getState().setSettings({ compressMode: "presets" })`);
await ev(`window.__T.getState().open("compress", ${id})`);
await until(`!!window.__T.getState().sheet?.audit && !!window.__T.getState().sheet?.advanced`, "the audit and the engine defaults", 60000);
await until(`!!window.__T.getState().sheet?.estimates`, "the preset estimates", 60000);
report.presetsTab = await text(".tl-presets");
await shot("presets");

await click(".tl-seg button", "Advanced");
await until(`!!document.querySelector(".tl-usage")`, "the space usage table");
await until(`!!window.__T.getState().sheet?.customEstimate`, "the first estimate", 60000);
report.usage = await ev(`[...document.querySelectorAll(".tl-usage tbody tr")].map((r) => [...r.children].slice(0, 3).map((c) => c.innerText.trim()))`);
report.estimate = await text(".tl-est");
await section("Images");
await shot("advanced-images");

// ---- change settings: the estimate follows ----
const est0 = await ev(`window.__T.getState().sheet.customEstimate.bytes`);
await section("Fonts");
await section("Discard objects");
await check("Document information and metadata");
await check("Embedded page thumbnails");
await check("Bookmarks");
await check("Private data of other applications");
await section("Clean up");
await until(`window.__T.getState().sheet.customEstimate === null`, "the estimate to be recomputed", 5000).catch(() => {});
await until(`!!window.__T.getState().sheet?.customEstimate`, "the new estimate", 60000);
report.estimateChange = { before: est0, after: await ev(`window.__T.getState().sheet.customEstimate.bytes`), line: await text(".tl-est") };
await ev(`document.querySelector(".tl-usage")?.scrollIntoView({ block: "start" })`);
await shot("advanced-usage");
const reveal = (title) =>
  ev(`(() => { const s = [...document.querySelectorAll("details.tl-sec")].find((d) => d.querySelector(".tl-sec-title")?.textContent === ${JSON.stringify(title)}); s?.scrollIntoView({ block: "start" }); return !!s })()`);
await reveal("Fonts");
await shot("advanced-fonts");
await reveal("Discard objects");
await shot("advanced-discard");

// ---- save under a name, switch to a preset and back ----
await click(".tl-saved .ax-btn", "Save");
await until(`!!document.querySelector(".tl-saved input[type=text], .tl-saved input:not([type])")`, "the name field", 5000);
await ev(`(() => { const i = document.querySelector(".tl-saved input[type=text], .tl-saved input:not([type])"); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set; set.call(i, "Web, small"); i.dispatchEvent(new Event("input", { bubbles: true })); return true })()`);
await click(".tl-saved .ax-btn.primary");
await until(`window.__T.getState().sheet.saved.some((p) => p.name === "Web, small")`, "the preset to be saved", 10000);
report.saved = await ev(`window.__T.getState().sheet.saved.map((p) => p.name)`);
await ev(`window.__T.getState().startFrom("smallest")`);
await sleep(300);
await ev(`window.__T.getState().loadPreset("Web, small")`);
await sleep(300);
report.loaded = await ev(`(() => { const o = window.__T.getState().sheet.advanced; return { metadata: o.strip.metadata, thumbnails: o.strip.thumbnails, bookmarks: o.strip.bookmarks, privateData: o.strip.privateData } })()`);
await ev(`document.querySelector(".tl-saved")?.scrollIntoView({ block: "start" })`);
await shot("advanced-saved");

// ---- run: the result has the before/after table ----
await until(`!!window.__T.getState().sheet?.customEstimate`, "the estimate", 60000);
await click(".tl-foot .ax-btn.primary", "Compress");
await until(`window.__T.getState().sheet?.phase === "done" || window.__T.getState().sheet?.phase === "error"`, "the result", 180000);
const done = await ev(`(() => { const s = window.__T.getState().sheet; return { phase: s.phase, error: s.error, r: s.compressed && { before: s.compressed.sizeBefore, after: s.compressed.sizeAfter, kept: s.compressed.keptOriginal, stripped: s.compressed.stripped, visual: s.compressed.visual && { psnr: s.compressed.visual.psnr, pages: s.compressed.visual.pagesChecked }, warnings: s.compressed.warnings } } })()`);
report.done = done;
await shot("advanced-done");

if (process.env.OPT_REPORT) fs.writeFileSync(process.env.OPT_REPORT, JSON.stringify(report, null, 2));
console.log(JSON.stringify(report, null, 2));
ws.close();
