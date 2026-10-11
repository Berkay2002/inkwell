// Drives a running Inkwell through WebView2's DevTools endpoint and captures the Redact rail: hits for a pattern,
// marks on the page with the marks list, the apply section, the result card and a refused outcome, light and dark.
// Works on a copy of a generated sample (scripts/redact-sample.mjs), never on real files. The engine is real (find,
// mark, unmark, options); only `redact_apply` is mocked, because its native save dialog cannot be driven over CDP.
//   $d = "$env:TEMP\inkwell-rd"; mkdir $d; node scripts/redact-sample.mjs "$d\Lease agreement.pdf"
//   npx vite --port 1420        (the debug exe loads the dev server)
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9224'
//   src-tauri\target\debug\inkwell.exe "$d\Lease agreement.pdf"
//   node scripts/redact-shots.mjs docs/screens/redact 9224
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/redact";
const port = process.argv[3] ?? "9224";
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
const until = async (expr, what, limit = 20000) => {
  const t0 = Date.now();
  for (;;) {
    const v = await ev(expr);
    if (v) return v;
    if (Date.now() - t0 > limit) throw new Error(`timed out waiting for ${what}`);
    await sleep(150);
  }
};
const theme = (t) => ev(`window.__S.getState().setTheme(${JSON.stringify(t)})`);
const shot = async (name) => {
  await sleep(700); // page images and transitions settle
  for (const t of ["light", "dark"]) {
    await theme(t);
    await sleep(500);
    const r = await send("Page.captureScreenshot", { format: "png" });
    fs.writeFileSync(path.join(out, `${name}-${t}.png`), Buffer.from(r.data, "base64"));
    console.log("screenshot", `${name}-${t}.png`);
  }
  await theme("light");
};
const click = (sel, text) =>
  ev(`(() => { const els = [...document.querySelectorAll(${JSON.stringify(sel)})]; const b = ${text ? `els.find((e) => e.textContent.trim() === ${JSON.stringify(text)} || e.firstChild?.textContent?.trim() === ${JSON.stringify(text)})` : "els[0]"}; b?.click(); return !!b })()`);
const rect = (sel) => ev(`(() => { const r = document.querySelector(${JSON.stringify(sel)})?.getBoundingClientRect(); return r ? { x: r.x, y: r.y, w: r.width, h: r.height } : null })()`);
const mouse = (type, x, y) => send("Input.dispatchMouseEvent", { type, x, y, button: "left", buttons: type === "mouseReleased" ? 0 : 1, clickCount: 1 });
const report = {};

await ev(`(() => { window.__S = window.__inkwell.useStore; window.__R = window.__inkwell.useRedact; return true })()`);
if (!(await ev("!!window.__R"))) throw new Error("window.__inkwell.useRedact is missing: rebuild the app");
const doc = await until(`(() => { const d = Object.values(window.__S.getState().docs).find((d) => d.name === "Lease agreement.pdf"); return d ? d.id : null })()`, "the sample");
await ev(`window.__S.getState().activate(${doc})`);
await ev(`window.__S.getState().patch(${doc}, { organize: false })`);
await ev(`window.__S.getState().setSidebar("none")`);
await until(`document.querySelectorAll(".viewer .page").length > 0 && window.__S.getState().docs[${doc}].sizes.length === 9`, "the pages");
await ev(`window.__S.getState().goTo(${doc}, 1)`);

// ---- the rail with the e-mail hits ----
await ev(`window.__R.getState().setOpen(true)`);
await until(`Object.keys(window.__R.getState().docs[${doc}]?.counts ?? {}).length === 6`, "the pattern counts");
report.counts = await ev(`window.__R.getState().docs[${doc}].counts`);
await click(".rd-pill", "E-mail");
await until(`document.querySelectorAll(".rd-hit").length > 0`, "hits");
report.emailHits = await ev(`document.querySelectorAll(".rd-hit").length`);
await shot("rail-hits");

// ---- marks: the e-mails, the phone, a dragged area and a page ----
await click(".rd-found .rd-btn.primary");
await until(`window.__R.getState().docs[${doc}].marks.length >= 3`, "the e-mail marks");
await click(".rd-pill", "Phone");
await until(`document.querySelectorAll(".rd-hit").length > 0`, "phone hits");
await click(".rd-found .rd-btn.primary");
await until(`window.__R.getState().docs[${doc}].marks.length >= 4`, "the phone mark");
await click(".rd-tool", "Mark text or area");
await until(`!!document.querySelector(".rd-capture")`, "the drag overlay");
await ev(`window.__S.getState().goTo(${doc}, 1)`);
await sleep(500);
const pg = await rect(`.viewer .page[data-page="1"]`);
const zoom = await ev(`window.__S.getState().docs[${doc}].zoom`);
// the "Name:" row of page 2: page points x 64..300, y (792-600-12)..(792-600+4)
const at = (x, y) => [pg.x + x * zoom, pg.y + y * zoom];
const [x0, y0] = at(60, 176);
const [x1, y1] = at(300, 200);
await mouse("mouseMoved", x0, y0);
await mouse("mousePressed", x0, y0);
await mouse("mouseMoved", (x0 + x1) / 2, (y0 + y1) / 2);
await mouse("mouseMoved", x1, y1);
await mouse("mouseReleased", x1, y1);
await until(`window.__R.getState().docs[${doc}].marks.length >= 5`, "the area mark");
await click(".rd-tool", "Mark text or area");
await click(".rd-tool", "Mark pages");
await ev(`(() => { const i = document.querySelector(".rd-pages input"); i.focus(); return true })()`);
await send("Input.insertText", { text: "9" });
await click(".rd-pages .rd-btn", "Mark");
await until(`window.__R.getState().docs[${doc}].marks.length >= 6`, "the page mark");
report.marks = await ev(`window.__R.getState().docs[${doc}].marks.map((m) => ({ page: m.page, wholePage: m.wholePage, label: m.label }))`);
await click(".rd-tool", "Mark pages"); // closes the page field
await ev(`window.__S.getState().goTo(${doc}, 1)`);
await ev(`document.querySelector(".rd-body").scrollTop = 0`);
await shot("marks");

// ---- Apply: the section opens in the rail ----
await click(".rd-foot .rd-btn.primary");
await until(`!!document.querySelector(".rd-apply .rd-rows")`, "the apply section");
report.applyRows = await ev(`[...document.querySelectorAll(".rd-apply .rd-rows li")].map((l) => l.textContent)`);
await ev(`document.querySelector(".rd-body").scrollTop = 1e6`);
await shot("apply");

// ---- refused, then the result (redact_apply is mocked: its save dialog is native) ----
await ev(`(() => { window.__apply = null; const api = window.__inkwell.redactApi; const real = api.apply; api.apply = (...a) => (window.__apply ? Promise.resolve(window.__apply) : real(...a)); return true })()`);
await ev(`window.__apply = { status: "refused", leaks: [
  { needle: "alex.johnson@example.com", place: "page 4, hidden text", critical: true },
  { needle: "811218-9876", place: "the document's metadata", critical: false },
  { needle: "alex.johnson@example.com", place: "bookmark \\"Contact\\"", critical: false },
  { needle: "+46 70 123 45 67", place: "page 2, a form field", critical: true } ] }`);
await click(".rd-actions .rd-btn.primary", "Redact");
await until(`!!document.querySelector(".rd-card.refused")`, "the refused card");
await shot("refused");
await click(".rd-card.refused .rd-btn", "Back to marks");
await until(`!document.querySelector(".rd-card") && !!document.querySelector(".rd-foot")`, "back to the marks");

await click(".rd-foot .rd-btn.primary");
await until(`!!document.querySelector(".rd-apply .rd-rows")`, "the apply section again");
const saved = await ev(`window.__S.getState().docs[${doc}].path.replace(/\\.pdf$/i, "_Redacted.pdf")`);
await ev(`(() => { const d = window.__S.getState().docs[${doc}]; window.__apply = { status: "applied", marks: 7, savedTo: ${JSON.stringify(saved)},
  state: { rev: d.rev, pageCount: d.pageCount, sizes: d.sizes, path: d.path, dirty: false, undo: null, redo: null },
  sanitize: { metadata: 2, hiddenLayerBlocks: 1, hiddenTextPages: 0, scriptsActions: 2, embeddedFiles: 1, annotations: 3, formFields: 0, bookmarks: 4, quoting: 0 },
  verify: { leaks: [] } }; return true })()`);
await click(".rd-actions .rd-btn.primary", "Redact");
await until(`!!document.querySelector(".rd-card.result")`, "the result card");
await shot("result");
await click(".rd-card.result .rd-btn.primary", "Done");
report.done = await ev(`window.__R.getState().docs[${doc}].phase`);

await ev(`window.__R.getState().setOpen(false)`);
console.log(JSON.stringify(report, null, 2));
ws.close();
