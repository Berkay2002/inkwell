// Drives a running Inkwell through WebView2's DevTools endpoint: the accessibility checker on copies of
// docs/study samples (never the originals): untagged -> Autotag -> check -> fix -> undo -> save -> reopen, Show on
// a finding, the figure's alt-text field, Fix all as one step, and a 1200-page document for progress, cancel and
// responsiveness. Screens (light and dark) go to docs/screens/a11y.
//   $d = "$env:TEMP\inkwell-a11y"; mkdir $d; copy docs\study\samples\{compress-source,form-source,redact-source}.pdf $d
//   $env:INKWELL_SEED_DIR = $d; cargo test --lib seed_compare_screens -- --ignored      (from src-tauri; writes "Big v1.pdf")
//   npx vite --port 1420                                   (the debug exe loads the dev server)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:INKWELL_HISTORY_ROOT = "$d\store"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9226'
//   src-tauri\target\debug\inkwell.exe "$d\compress-source.pdf" "$d\form-source.pdf" "$d\redact-source.pdf" "$d\Big v1.pdf"
//   node scripts/a11y-shots.mjs docs/screens/a11y 9226
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/a11y";
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
const report = {};

await ev(`(() => { window.__S = window.__inkwell.useStore; window.__A = window.__inkwell.useA11y; return true })()`);
await until(`Object.keys(window.__S.getState().docs).length >= 3`, "the sample documents", 60000);
// the fourth has a space in its name: open it from here rather than from the command line
const docByNameQuick = (n) => ev(`Object.values(window.__S.getState().docs).some((d) => d.name === ${JSON.stringify(n)})`);
const dir = await ev(`window.__S.getState().docs[window.__S.getState().order[0]].path.replace(/[\\/][^\\/]*$/, "")`);
if (!(await docByNameQuick("Big v1.pdf"))) await ev(`window.__S.getState().openPath(${JSON.stringify(dir + "/Big v1.pdf")})`);
await until(`Object.keys(window.__S.getState().docs).length >= 4`, "four documents", 60000);
const docByName = (n) => ev(`(() => { const d = Object.values(window.__S.getState().docs).find((d) => d.name === ${JSON.stringify(n)}); return d ? { id: d.id, path: d.path } : null })()`);
const activate = (id) => ev(`window.__S.getState().activate(${id})`);
const a11y = (id) => ev(`(() => { const d = window.__A.getState().docs[${id}]; return d ? { phase: d.phase, error: d.error, rev: d.checkedRev, results: d.report?.results.map((r) => [r.rule, r.status, r.findings.length]) ?? null, tagged: d.report?.tagged ?? null } : null })()`);
const status = async (id, rule) => (await a11y(id))?.results?.find((r) => r[0] === rule)?.[1];
const waitChecked = (id, what = "the report") => until(`(() => { const d = window.__A.getState().docs[${id}]; const s = window.__S.getState().docs[${id}]; return d && d.report && d.phase === "idle" && d.checkedRev === s.rev })()`, what, 60000, 100);
const undoLabel = (id) => ev(`window.__S.getState().docs[${id}].undoLabel`);
const rev = (id) => ev(`window.__S.getState().docs[${id}].rev`);
// Ctrl+Z reaches the app's key handler the way a keypress does; the toolbar's Undo button only shows in Organize
const clickUndo = () => ev(`(document.activeElement?.blur?.(), document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "z", ctrlKey: true, bubbles: true })), true)`);

await theme("light");
const comp = await docByName("compress-source.pdf");
const form = await docByName("form-source.pdf");
const big = await docByName("Big v1.pdf");

// ---- open from the Tools menu, on an untagged document ----
await activate(comp.id);
await until(`!!document.querySelector(".ax-toggle")`, "the Tools button");
await click(".ax-toggle", "Tools");
await until(`[...document.querySelectorAll("[role=menuitem], .menu button, button")].some((b) => b.textContent.includes("Check accessibility"))`, "the menu item");
report.menuItem = await click("button", "Check accessibility");
await until(`!!document.querySelector(".a11y-panel")`, "the rail");
await waitChecked(comp.id, "the first report");
await until(`!!document.querySelector(".a11y-card")`, "the Autotag offer");
report.untaggedCard = await text(".a11y-card");
report.untaggedRules = (await a11y(comp.id)).results;
await shot("a11y-untagged");

// ---- Autotag: one undo step ----
const revBefore = await rev(comp.id);
await click(".a11y-card .a11y-btn", "Autotag");
await until(`window.__S.getState().docs[${comp.id}].rev > ${revBefore}`, "Autotag to finish", 60000);
await waitChecked(comp.id, "the report after Autotag");
report.afterAutotag = { undo: await undoLabel(comp.id), tagged: (await a11y(comp.id)).tagged, results: (await a11y(comp.id)).results };
await sleep(800);
await shot("a11y-report");

// ---- Show: jump to the first figure and outline it ----
const figs = await until(`document.querySelectorAll(".a11y-finding.alt").length`, "figure findings in the rail", 10000);
report.figureFindings = figs;
report.thumbs = await ev(`document.querySelectorAll(".a11y-thumb").length`);
await until(`document.querySelectorAll(".a11y-thumb img").length > 0`, "figure thumbnails", 10000);
await click(".a11y-finding-main:not([disabled])");
await until(`!!document.querySelector(".a11y-mark")`, "the highlight on the page", 8000);
report.showMark = await ev(`(() => { const m = document.querySelector(".a11y-mark"); const r = m.getBoundingClientRect(); return { w: Math.round(r.width), h: Math.round(r.height), page: m.closest(".page")?.dataset.page } })()`);
await shot("a11y-show");

// ---- alt text on a figure: one fix, one undo step, the finding goes ----
const figuresBefore = (await a11y(comp.id)).results.find((r) => r[0] === "figure-alt-text")[2];
const revAlt = await rev(comp.id);
await ev(`(() => { const i = document.querySelector(".a11y-alt-field input"); const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set; set.call(i, "A bar chart of revenue by quarter"); i.dispatchEvent(new Event("input", { bubbles: true })); return true })()`);
await sleep(150);
await click(".a11y-alt-field .a11y-btn", "Save");
await until(`window.__S.getState().docs[${comp.id}].rev > ${revAlt}`, "the alt-text fix");
await waitChecked(comp.id, "the report after the alt text");
const figuresAfter = (await a11y(comp.id)).results.find((r) => r[0] === "figure-alt-text")[2];
report.altText = { before: figuresBefore, after: figuresAfter, undo: await undoLabel(comp.id) };
const revBeforeUndo = await rev(comp.id);
await clickUndo();
await until(`window.__S.getState().docs[${comp.id}].rev !== ${revBeforeUndo}`, "undo");
await waitChecked(comp.id, "the report after undo");
report.altUndo = (await a11y(comp.id)).results.find((r) => r[0] === "figure-alt-text")[2];

// ---- Fix all safe issues: one undo step, outcome predicted on the button ----
report.fixAllButton = await text(".a11y-foot .a11y-btn.primary");
const stepsBefore = await undoLabel(comp.id);
const revAll = await rev(comp.id);
await click(".a11y-foot .a11y-btn.primary");
await until(`window.__S.getState().docs[${comp.id}].rev > ${revAll}`, "Fix all safe issues");
await waitChecked(comp.id, "the report after Fix all");
report.afterFixAll = { undo: await undoLabel(comp.id), results: (await a11y(comp.id)).results.map((r) => `${r[0]}:${r[1]}`), button: await text(".a11y-foot .a11y-btn.primary") };
await shot("a11y-fixed");
// one undo takes all of it back
const revBeforeUndoAll = await rev(comp.id);
await clickUndo();
await until(`window.__S.getState().docs[${comp.id}].rev !== ${revBeforeUndoAll}`, "undo of Fix all");
await waitChecked(comp.id, "the report after undoing Fix all");
report.undoFixAll = { label: await undoLabel(comp.id), expectedBack: stepsBefore, title: await status(comp.id, "title"), language: await status(comp.id, "primary-language") };
// do it again, then save and reopen
await click(".a11y-foot .a11y-btn.primary");
await waitChecked(comp.id, "the report after Fix all again");
const afterAll = (await a11y(comp.id)).results.map((r) => `${r[0]}:${r[1]}`);
await ev(`window.__TAURI_INTERNALS__.invoke("save_document", { doc: ${comp.id}, to: null })`);
const savedPath = comp.path;
await ev(`window.__S.getState().closeDoc(${comp.id})`);
await sleep(400);
await ev(`window.__S.getState().openPath(${JSON.stringify(savedPath)})`);
const reopened = await until(`(() => { const d = Object.values(window.__S.getState().docs).find((d) => d.name === ${JSON.stringify("compress-source.pdf")}); return d ? d.id : null })()`, "the reopened file", 20000);
await ev(`window.__A.getState().check(${reopened})`);
await waitChecked(reopened, "the report of the reopened file");
report.reopened = { tagged: (await a11y(reopened)).tagged, same: JSON.stringify((await a11y(reopened)).results.map((r) => `${r[0]}:${r[1]}`)) === JSON.stringify(afterAll), results: (await a11y(reopened)).results.map((r) => `${r[0]}:${r[1]}`) };

// ---- a form: tooltips ----
await activate(form.id);
await ev(`window.__A.getState().setOpen(true)`);
await waitChecked(form.id, "the form's report");
report.form = (await a11y(form.id)).results.map((r) => `${r[0]}:${r[1]}`);
await shot("a11y-form", ["light"]);

// ---- 1200 pages: progress, cancel, and the page keeps painting ----
await activate(big.id);
await ev(`(() => { window.__gap = 0; let last = performance.now(); const tick = (t) => { window.__gap = Math.max(window.__gap, t - last); last = t; requestAnimationFrame(tick) }; requestAnimationFrame(tick); return true })()`);
await sleep(300);
await ev(`window.__gap = 0`);
await until(`(() => { const d = window.__A.getState().docs[${big.id}]; return d && (d.phase === "checking" || d.report) })()`, "the big check to start", 20000, 20);
const tBig = Date.now();
await until(`!!document.querySelector(".a11y-run")`, "check progress", 20000, 20).catch(() => {});
report.checkProgress = await text(".a11y-run");
if (report.checkProgress) {
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, "a11y-checking-light.png"), Buffer.from(r.data, "base64"));
  console.log("screenshot a11y-checking-light.png");
}
await waitChecked(big.id, "the 1200-page report");
report.bigCheck = { ms: Date.now() - tBig, maxFrameGapMs: Math.round(await ev(`window.__gap`)), rules: (await a11y(big.id)).results.length };

// Autotag on it, first run to the end (for the progress screen, and one undo step takes it all back) ...
const revBig = await rev(big.id);
const undoBig = await undoLabel(big.id);
await until(`!!document.querySelector(".a11y-card .a11y-btn")`, "the Autotag offer on the big file", 20000);
await ev(`window.__gap = 0`);
await click(".a11y-card .a11y-btn", "Autotag");
await until(`(() => { const p = window.__A.getState().docs[${big.id}].progress; return p && p.stage === "read" && p.done >= 20 })()`, "Autotag progress", 20000, 5);
report.autotagProgress = await text(".a11y-card .a11y-run");
{
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, "a11y-autotagging-light.png"), Buffer.from(r.data, "base64"));
  console.log("screenshot a11y-autotagging-light.png");
}
const tRun = Date.now();
await until(`window.__S.getState().docs[${big.id}].rev > ${revBig}`, "Autotag on 1200 pages", 120000, 50);
report.autotagBig = { msAfterShot: Date.now() - tRun, undo: await undoLabel(big.id), maxFrameGapMs: Math.round(await ev(`window.__gap`)) };
await waitChecked(big.id, "the report of the tagged big file");
const revTagged = await rev(big.id);
await clickUndo();
await until(`window.__S.getState().docs[${big.id}].rev !== ${revTagged}`, "undo of Autotag on the big file", 60000);
await waitChecked(big.id, "the report after undoing Autotag");
report.autotagBigUndo = { label: await undoLabel(big.id), expected: undoBig, tagged: (await a11y(big.id)).tagged };

// ... then stopped part-way: the document is as it was
await until(`!!document.querySelector(".a11y-card .a11y-btn")`, "the Autotag offer again", 20000);
const revBeforeCancel = await rev(big.id);
await click(".a11y-card .a11y-btn", "Autotag");
await until(`(() => { const p = window.__A.getState().docs[${big.id}].progress; return p && p.stage === "read" && p.done >= 10 })()`, "Autotag progress", 20000, 2);
const tCancel = Date.now();
report.autotagCancelClicked = await click(".a11y-card .a11y-run button", "Cancel");
await until(`window.__A.getState().docs[${big.id}].phase === "idle"`, "Autotag to stop", 30000, 20);
report.autotagCancel = {
  ms: Date.now() - tCancel,
  revSame: (await rev(big.id)) === revBeforeCancel,
  undoSame: (await undoLabel(big.id)) === undoBig,
  error: (await a11y(big.id)).error,
};
await shot("a11y-after-cancel", ["light"]);

await ev(`window.__A.getState().setOpen(false)`);
console.log(JSON.stringify(report, null, 2));
ws.close();
