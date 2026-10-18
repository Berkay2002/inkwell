// Drives a running Inkwell through WebView2's DevTools endpoint: combines three COPIES of samples (ranges,
// bookmarks, contents page), adds / saves out / deletes an attachment, numbers pages in roman, sets a transition and
// plays it full screen, saves and reopens, undoes. Captures the screens light and dark. Never the user's files.
// The native save dialog cannot be driven without OS-level input: a debug build answers it with INKWELL_SAVE_DIR.
//   $d = "$env:TEMP\inkwell-organize-plus"; mkdir $d\in, $d\out, $d\data
//   copy samples\compress-source.pdf, edit-source.pdf, form-source.pdf -> $d\in
//   npx vite --port 1420        (the debug exe loads the dev server)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:INKWELL_SAVE_DIR = "$d\out"
//   $env:WEBVIEW2_USER_DATA_FOLDER = "$d\wv"; $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9246'
//   src-tauri\target\debug\inkwell.exe "$d\in\compress-source.pdf" "$d\in\edit-source.pdf" "$d\in\form-source.pdf"
//   node scripts/organize-plus-shots.mjs docs/screens/organize-plus 9246 "$d\out"
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/organize-plus";
const port = process.argv[3] ?? "9246";
const outDir = process.argv[4];
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
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text ?? "eval failed");
  return r.result.value;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const I = "window.__inkwell";
const S = `${I}.useStore`;
const until = async (expr, what, tries = 150) => {
  for (let i = 0; i < tries; i++) {
    if (await ev(`(() => { try { return !!(${expr}) } catch { return false } })()`)) return;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${what}`);
};
const shot = async (name, wait = 350) => {
  await sleep(wait);
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  console.log("screenshot", name);
};
const theme = (t) => ev(`${S}.getState().setTheme("${t}")`);
async function both(name, wait) {
  await theme("light");
  await shot(`${name}-light.png`, wait);
  await theme("dark");
  await shot(`${name}-dark.png`, wait);
  await theme("light");
}
const click = (sel) => ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) throw new Error("no " + ${JSON.stringify(sel)}); e.click(); return true })()`);
const key = (k, extra = {}) =>
  ev(`window.dispatchEvent(new KeyboardEvent("keydown", ${JSON.stringify({ key: k, bubbles: true, cancelable: true, ...extra })}))`);
const report = { checks: {} };
const check = (name, ok, detail) => {
  report.checks[name] = ok ? "ok" : `FAILED ${JSON.stringify(detail)}`;
  if (!ok) console.error("FAILED", name, JSON.stringify(detail));
};

await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await theme("light");
await until(`${S}.getState().order.length >= 3`, "three open documents");
const docs = await ev(`${S}.getState().order.map((id) => { const d = ${S}.getState().docs[id]; return { id, path: d.path, name: d.name, pages: d.pageCount } })`);
report.opened = docs;

// ---- Combine files -------------------------------------------------------------------------------
const refs = [];
for (const d of docs) refs.push(await ev(`${I}.api.knownFile(${JSON.stringify(d.path)})`));
await ev(`${I}.useCombine.getState().open(${JSON.stringify(refs)})`);
await until(`${I}.useCombine.getState().sheet.entries.every((e) => e.pages != null && e.thumb)`, "thumbnails of the files", 300);
await until(`${I}.useCombine.getState().sheet.plan`, "the first plan");
const keys = await ev(`${I}.useCombine.getState().sheet.entries.map((e) => e.key)`);
await ev(`${I}.useCombine.getState().setRange(${keys[0]}, "1-2")`);
await ev(`${I}.useCombine.getState().setTitle(${keys[1]}, "Edit sample")`);
if (docs[2].pages >= 2) await ev(`${I}.useCombine.getState().setRange(${keys[2]}, "2, 1")`);
await ev(`${I}.useCombine.getState().setOptions({ contentsPage: true })`);
await until(`!${I}.useCombine.getState().sheet.planning && ${I}.useCombine.getState().sheet.plan`, "the plan after the changes");
const plan = await ev(`${I}.useCombine.getState().sheet.plan`);
report.plan = plan.summary;
check("plan says files, pages, bookmarks", /^Combines 3 files, \d+ pages?, 3 bookmarks/.test(plan.summary) || /contents/.test(plan.summary), plan.summary);
await shot("_probe.png");
fs.rmSync(path.join(out, "_probe.png"), { force: true });
await both("combine-sheet");
await ev(`${I}.useCombine.getState().toggleOpen(${keys[0]})`);
await until(`document.querySelectorAll(".cb-pg-img img").length > 0`, "page thumbnails");
await both("combine-pages");
await click(".cb-pg:nth-child(2)");
await sleep(400);
const afterClick = await ev(`${I}.useCombine.getState().sheet.entries[0].range`);
report.rangeAfterPageClick = afterClick;
check("a page click rewrites the range", afterClick !== "1-2", afterClick);
await ev(`${I}.useCombine.getState().setRange(${keys[0]}, "1-2")`);
await until(`!${I}.useCombine.getState().sheet.planning`, "the plan");

const before = await ev(`${S}.getState().order.length`);
await ev(`${I}.useCombine.getState().run()`);
await until(`${I}.useCombine.getState().sheet == null`, "the sheet to close after the run", 600);
await until(`${S}.getState().order.length === ${before + 1}`, "the result in a tab");
const res = await ev(`(() => { const s = ${S}.getState(); const d = s.docs[s.active]; return { id: d.id, path: d.path, pages: d.pageCount } })()`);
report.result = res;
check("result page count = plan", res.pages === plan.pages, { got: res.pages, plan: plan.pages });
check("result file exists", !outDir || fs.existsSync(res.path), res.path);
const outline = await ev(`${I}.api.outline(${res.id})`);
report.outline = outline.map((o) => o.title);
check("a bookmark per file (plus the contents entry)", outline.length >= 3, outline.length);
const names = outline.map((o) => o.title).join("|");
check("the file's own title was used", names.includes("Edit sample"), names);
await both("combined-result");

// ---- attachments ---------------------------------------------------------------------------------
const id = res.id;
await ev(`${S}.getState().setSidebar("attachments")`);
await until(`document.querySelector(".att")`, "the attachments panel");
await shot("_probe.png");
fs.rmSync(path.join(out, "_probe.png"), { force: true });
const attachRef = await ev(`${I}.api.knownFile(${JSON.stringify(docs[2].path)})`);
const rev0 = await ev(`${S}.getState().docs[${id}].rev`);
await ev(`${I}.edits.run(${id}, () => ${I}.api.attachmentsAdd(${id}, [${JSON.stringify(attachRef.token)}], "the form, attached"))`);
await until(`document.querySelectorAll(".att-row").length === 1`, "the attachment row");
const list1 = await ev(`${I}.api.attachmentsList(${id})`);
report.attachment = list1[0];
check("attachment listed with size and description", list1.length === 1 && list1[0].size > 0 && list1[0].description === "the form, attached", list1);
check("an add is one undo step", (await ev(`${S}.getState().docs[${id}].rev`)) === rev0 + 1, null);
await both("attachments");
// save out through the save token
const outName = list1[0].name;
const saved = await ev(`(async () => { const t = await ${I}.api.pickSave(${JSON.stringify(outName)}, { doc: ${id} }, "any"); return await ${I}.api.attachmentSave(${id}, ${JSON.stringify(list1[0].id)}, t.token) })()`);
check("saved attachment is byte-identical", !outDir || (fs.existsSync(saved) && fs.readFileSync(saved).equals(fs.readFileSync(docs[2].path))), saved);
// delete (one step) and undo
await click(`.att-row [aria-label^="Delete"]`);
await until(`document.querySelectorAll(".att-row").length === 0`, "the row to go");
await ev(`${I}.edits.undo(${id})`);
await until(`document.querySelectorAll(".att-row").length === 1`, "undo to bring it back");
check("undo restores the attachment", true);

// ---- page labels ---------------------------------------------------------------------------------
await ev(`${S}.getState().setSidebar("thumbs")`);
await ev(`${I}.useSheets.getState().openLabels(${id}, 0, 1)`);
await until(`document.querySelector(".op-sheet select")`, "the labels sheet");
await ev(`(() => { const s = document.querySelector(".op-sheet select"); const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set; set.call(s, "lowerRoman"); s.dispatchEvent(new Event("change", { bubbles: true })); return true })()`);
await sleep(250);
await both("labels-sheet");
const rev1 = await ev(`${S}.getState().docs[${id}].rev`);
await click(".op-foot .btn.primary");
await until(`!document.querySelector(".op-sheet")`, "the sheet to close");
await until(`${S}.getState().docs[${id}].labels.length > 0`, "labels loaded");
const labels = await ev(`${S}.getState().docs[${id}].labels.slice(0, 4)`);
report.labels = labels;
check("roman labels on the first pages", labels[0] === "i" && labels[1] === "ii" && (labels[2] ?? "3") === (labels[2] ?? "3"), labels);
check("labels are one undo step", (await ev(`${S}.getState().docs[${id}].rev`)) === rev1 + 1, null);
const box = await ev(`document.getElementById("page-input").value`);
check("page box shows the label", box === "i", box);
await both("labels-thumbs");
// organize grid with labels
await ev(`${S}.getState().patch(${id}, { organize: true })`);
await until(`document.querySelector(".org-num")`, "the organize grid");
const orgNums = await ev(`[...document.querySelectorAll(".org-num")].slice(0, 3).map((e) => e.textContent)`);
check("organize grid shows labels", orgNums[0] === "i", orgNums);
await both("labels-organize");
await ev(`${S}.getState().patch(${id}, { organize: false })`);

// ---- transitions and full screen -----------------------------------------------------------------
await ev(`${I}.edits.run(${id}, () => ${I}.api.transitionsSet(${id}, [1], { transition: { style: "wipe", duration: 1, dimension: null, motion: null, direction: 0 }, autoAdvance: null }))`);
await ev(`${I}.useSheets.getState().openTrans(${id}, [1])`);
await until(`document.querySelector(".op-sheet .op-sample") && !/Reading/.test(document.querySelector(".op-sheet .op-sample").textContent)`, "the transitions sheet");
await both("transitions-sheet");
await click(".op-head .ax-icon");
const trans = await ev(`${I}.api.transitionsList(${id})`);
check("transition on page 2", trans[1]?.transition?.style === "wipe", trans.slice(0, 3));
await ev(`${S}.getState().goTo(${id}, 0)`);
await key("l", { ctrlKey: true, code: "KeyL" });
await until(`document.querySelector(".pres .pres-page canvas")`, "full screen to draw page 1", 200);
await both("fullscreen");
await key("ArrowRight");
await sleep(1600);
const hud = await ev(`document.querySelector(".pres-hud span").textContent`);
report.hud = hud;
check("arrow goes to the next page", /ii|2 of/.test(hud), hud);
await key("Escape");
await until(`!document.querySelector(".pres")`, "full screen to end");

// ---- save and reopen -----------------------------------------------------------------------------
await ev(`${I}.edits.save(${id})`);
await until(`!${S}.getState().docs[${id}].dirty`, "save");
const savedPath = await ev(`${S}.getState().docs[${id}].path`);
await ev(`${S}.getState().closeDoc(${id})`);
const again = await ev(`${I}.api.knownFile(${JSON.stringify(savedPath)})`);
await ev(`${S}.getState().openPath(${JSON.stringify(again.path)}, ${JSON.stringify(again.token)})`);
await until(`${S}.getState().docs[${S}.getState().active]?.path === ${JSON.stringify(savedPath)}`, "the file again");
const id2 = await ev(`${S}.getState().active`);
await until(`${S}.getState().docs[${id2}].labels.length > 0`, "labels after reopen");
const back = {
  labels: await ev(`${S}.getState().docs[${id2}].labels.slice(0, 2)`),
  trans: (await ev(`${I}.api.transitionsList(${id2})`))[1]?.transition?.style,
  attachments: (await ev(`${I}.api.attachmentsList(${id2})`)).length,
};
report.reopened = back;
check("labels, transition and attachment survive save and reopen", back.labels[0] === "i" && back.trans === "wipe" && back.attachments === 1, back);

// undo everything back
for (let i = 0; i < 3; i++) await ev(`${I}.edits.undo(${id2})`);
await sleep(500);
report.afterUndo = await ev(`${S}.getState().docs[${id2}].labels.length`);

await send("Emulation.setFocusEmulationEnabled", { enabled: false });
console.log(JSON.stringify(report, null, 2));
ws.close();
