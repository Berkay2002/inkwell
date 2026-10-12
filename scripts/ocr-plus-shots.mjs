// Captures the Recognize text sheet's languages, missing-language help, result styles and before/after preview
// (light and dark) through WebView2's DevTools endpoint, on a COPY of a study sample.
//   copy docs/study/samples/ocr-in-01.pdf to <dir>/Scan sample.pdf; start the debug exe on it with
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9238'
//   node scripts/ocr-plus-shots.mjs docs/screens/ocr-plus 9238
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/ocr-plus";
const port = process.argv[3] ?? "9238";
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
const S = "window.__inkwell.useStore";
const T = "window.__inkwell.useTools";
const sheet = () => ev(`(() => { const s = ${T}.getState().sheet; return s && { phase: s.phase, languages: s.languages, style: s.style, plan: s.plan && { engine: s.plan.engine, missing: s.plan.missing.map((m) => m.tag) }, avail: !!s.available, detecting: s.detecting, preview: s.preview && { phase: s.preview.phase, converted: s.preview.converted, kept: s.preview.kept, message: s.preview.message }, rec: s.recognized && s.recognized.report && { words: s.recognized.report.words, editable: s.recognized.report.editable && { converted: s.recognized.report.editable.words_converted, kept: s.recognized.report.editable.words_kept } } } })()`);
const until = async (pred, what, limit = 60000) => {
  const t0 = Date.now();
  for (;;) {
    const s = await sheet();
    if (s && pred(s)) return s;
    if (Date.now() - t0 > limit) throw new Error(`timed out waiting for ${what}: ${JSON.stringify(s)}`);
    await sleep(100);
  }
};
const theme = (t) => ev(`${S}.getState().setTheme(${JSON.stringify(t)})`);
const shot = async (name) => {
  await sleep(400);
  for (const t of ["light", "dark"]) {
    await theme(t);
    await sleep(350);
    const r = await send("Page.captureScreenshot", { format: "png" });
    fs.writeFileSync(path.join(out, `${name}-${t}.png`), Buffer.from(r.data, "base64"));
    console.log("screenshot", `${name}-${t}.png`);
  }
  await theme("light");
};
const click = (sel, text) =>
  ev(`(() => { const b = [...document.querySelectorAll(${JSON.stringify(sel)})].find((e) => !${JSON.stringify(text ?? "")} || e.textContent.trim() === ${JSON.stringify(text ?? "")}); if (!b) return false; b.click(); return true })()`);
const pickStyle = (label) =>
  ev(`(() => { const b = [...document.querySelectorAll('[aria-label="Result"] .tl-preset')].find((e) => e.querySelector('.tl-preset-name')?.textContent.trim() === ${JSON.stringify(label)}); if (!b) return false; b.click(); return true })()`);
const type = async (sel, text) => {
  await ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); e.focus(); e.select(); return true })()`);
  await send("Input.insertText", { text });
  await sleep(250);
};
const results = [];
const check = (what, ok, detail = "") => {
  results.push({ what, ok: !!ok, detail });
  console.log(ok ? "ok  " : "FAIL", what, detail);
};

await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await theme("light");
const names = await ev(`(() => { const s = ${S}.getState(); return s.order.map((id) => ({ id, name: s.docs[id].name })) })()`);
const doc = names.find((d) => /scan/i.test(d.name));
if (!doc) throw new Error("no scan document is open: " + JSON.stringify(names));
await ev(`${S}.getState().activate(${doc.id}); ${S}.getState().setSidebar("none")`);
await sleep(1200);
await ev(`${T}.getState().open("ocr", ${doc.id})`);
const first = await until((s) => s.avail && s.plan && !s.detecting, "the language list and the detection");
check("the sheet lists installed languages", await ev(`document.querySelectorAll(".tl-lang").length`) > 0, `${await ev(`document.querySelectorAll(".tl-lang").length`)} rows`);
check("a language is chosen and an engine picked", first.languages.length > 0 && !!first.plan.engine, JSON.stringify(first.plan));
check("the detection line says where it came from", (await ev(`document.querySelector(".tl-detected")?.textContent ?? ""`)).length > 0);
await shot("ocr-languages");

// A name no pack or tessdata file matches: how to add it, with the Settings button (nothing is downloaded).
let helpFor = null;
for (const name of ["Finnish", "Thai", "Greek", "Hebrew", "Hindi", "Vietnamese"]) {
  await type('input[aria-label="Search languages"]', name);
  if (await ev(`!!document.querySelector(".tl-help")`)) {
    helpFor = name;
    break;
  }
}
check("a missing language shows help with a Settings button", helpFor && (await ev(`!!document.querySelector(".tl-help .tl-help-btn")`)), helpFor ?? "no candidate was missing");
await shot("ocr-missing-language");
await type('input[aria-label="Search languages"]', "");

// The result styles.
await ev(`document.querySelector('[aria-label="Result"]').scrollIntoView({ block: "center" })`);
check("three result styles", (await ev(`document.querySelectorAll('[aria-label="Result"] .tl-preset').length`)) === 3);
await pickStyle("Searchable image, cleaned up");
await sleep(300);
check("cleaned up warns that the image is re-encoded", await ev(`!!document.querySelector(".tl-body .tl-warn[role=note]")`));
await shot("ocr-style-cleaned");
await pickStyle("Editable text");
await sleep(300);
check("editable text asks for a preview first", (await ev(`document.querySelector(".tl-foot .ax-btn.primary")?.textContent ?? ""`)).includes("Preview page"));
await ev(`document.querySelector('[aria-label="Result"]').scrollIntoView({ block: "start" })`);
await shot("ocr-style-editable");

// Before / after of one page, before anything is applied.
await click(".tl-foot .ax-btn.primary");
const pv = await until((s) => s.preview && (s.preview.phase === "ready" || s.preview.phase === "error"), "the preview", 120000);
check("the preview is ready", pv.preview.phase === "ready", JSON.stringify(pv.preview));
await ev(`document.querySelector(".tl-preview")?.scrollIntoView({ block: "start" })`);
await click('[aria-label="Show"] button', "Before");
await shot("ocr-preview-before");
await click('[aria-label="Show"] button', "After");
await shot("ocr-preview-after");

// Applied: the result card, then Undo is one step.
const applyBtn = await ev(`document.querySelector(".tl-foot .ax-btn.primary")?.textContent ?? ""`);
check("the preview unlocks the run", applyBtn.includes("Recognize text"), applyBtn);
await click(".tl-foot .ax-btn.primary");
const done = await until((s) => s.phase === "done" || s.phase === "error", "the run", 180000);
check("the run finishes", done.phase === "done", JSON.stringify(done.rec));
await shot("ocr-editable-result");
const before = await ev(`(() => { const d = ${S}.getState().docs[${doc.id}]; return { undo: d.undoLabel } })()`);
await click(".tl-foot .ax-btn", "Undo");
await sleep(800);
const after = await ev(`(() => { const d = ${S}.getState().docs[${doc.id}]; return { undo: d.undoLabel, dirty: d.dirty } })()`);
check("undo is one step", before.undo?.includes("Recognize") && !after.undo?.includes("Recognize"), JSON.stringify({ before, after }));

await send("Emulation.setFocusEmulationEnabled", { enabled: false });
const bad = results.filter((r) => !r.ok);
console.log(`${results.length - bad.length}/${results.length} checks passed`);
ws.close();
process.exit(bad.length ? 1 : 0);
