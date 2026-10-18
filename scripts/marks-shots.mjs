// Drives Page marks through WebView2's DevTools endpoint on a COPY of a sample PDF from samples/ (never the user's
// files): header and footer, watermark and Bates numbering are added (one undo step each), the file is saved,
// closed and opened again, the marks are found and updated from what the file remembers, removed and undone.
// Captures the rail and the preview on the page, light and dark. Native dialogs (pick_open, pick_save,
// pick_folder) can't be driven without OS-level input, so Bates numbering across files is covered by the
// engine tests; picture marks by the engine tests as well.
//   copy redact-source.pdf to <dir>; start the app with it and a scratch data dir:
//   $env:INKWELL_DATA_DIR='<scratch>'; $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9334'
//   inkwell.exe <dir>/marks-in.pdf     (debug build with the dev server running on its devUrl)
//   node scripts/marks-shots.mjs docs/screens/marks 9334
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/marks";
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
const S = "window.__inkwell.useStore";
const M = "window.__inkwell.useMarks";
const shot = async (name, wait = 400) => {
  await sleep(wait);
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  console.log("screenshot", name);
};
const theme = (t) => ev(`${S}.getState().setTheme("${t}")`);
const click = (sel, text) =>
  ev(
    `(() => { const b = [...document.querySelectorAll(${JSON.stringify(sel)})].find((e) => !${JSON.stringify(text ?? "")} || e.textContent.trim() === ${JSON.stringify(text ?? "")}); if (!b || b.disabled) return false; b.click(); return true })()`,
  );
const set = (p) => ev(`${M}.getState().set(${JSON.stringify(p)})`);
const until = async (expr, ms = 15000) => {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    if (await ev(expr)) return true;
    await sleep(150);
  }
  return false;
};
async function both(name) {
  await theme("light");
  await shot(`${name}-light.png`);
  await theme("dark");
  await shot(`${name}-dark.png`);
  await theme("light");
}
const docId = () => ev(`Object.keys(${S}.getState().docs)[0]`);
const kinds = () => ev(`JSON.stringify((${M}.getState().info[Object.keys(${S}.getState().docs)[0]] ?? []).map((k) => k.kind + ":" + k.pages.join(",")))`);
const undoLabel = () => ev(`Object.values(${S}.getState().docs)[0].undoLabel`);
/** Reading order of a page: the first lines the text layer would give. */
const lines = (page) =>
  ev(`(async () => { const id = Number(Object.keys(${S}.getState().docs)[0]); const r = await window.__TAURI_INTERNALS__.invoke("page_text", { doc: id, page: ${page} }); return r.map((l) => l.t); })()`);
const ctrl = async (key, code, vk) => {
  await send("Input.dispatchKeyEvent", { type: "keyDown", modifiers: 2, key, code, windowsVirtualKeyCode: vk });
  await send("Input.dispatchKeyEvent", { type: "keyUp", modifiers: 2, key, code, windowsVirtualKeyCode: vk });
};
const report = {};

await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await theme("light");
await sleep(900);
// The history asks whether a file with a known name is the same document: not the subject here.
if (await click(".ax-btn, button", "No, it's new")) await sleep(300);
await ev(`${S}.getState().setZoomMode(Number(Object.keys(${S}.getState().docs)[0]), "fitPage")`);
const docPath = await ev(`Object.values(${S}.getState().docs)[0].path`);
report.file = path.basename(docPath);
report.bodyBefore = (await lines(0)).slice(0, 3);

// The Tools menu with the new entries, then Header & footer.
await click('button[aria-label="Tools"]');
await sleep(200);
report.menuItems = await ev(`[...document.querySelectorAll('.tools-panel .tool-name')].map((e) => e.textContent.trim())`);
await shot("tools-menu-light.png", 150);
await click(".tools-panel .tool-name", "Header & footer");
await sleep(500);
report.opened = await ev(`${M}.getState().sheet?.kind`);
await both("headerfooter-empty");

await set({ text: ["", "", "Lease agreement", "<<m/d/yyyy>>", "", "Page <<1>> of <<n>>"], style: { family: "helvetica", bold: false, italic: false, size: 10, color: [0.1, 0.1, 0.15], underline: false } });
report.previewShapes = await ev(`document.querySelectorAll(".mk-preview text").length`);
report.sentence = await ev(`document.querySelector(".mk .pr-outcome")?.textContent.trim()`);
await both("headerfooter-preview");
await click(".mk .tl-foot .ax-btn.primary");
report.hfAdded = await until(`${M}.getState().sheet?.done != null || ${M}.getState().sheet?.error != null`);
report.hfError = await ev(`${M}.getState().sheet?.error`);
report.hfUndo = await undoLabel();
report.hfKinds = await kinds();
report.previewAfterAdd = await ev(`document.querySelectorAll(".mk-preview").length`);
report.bodyAfterHf = (await lines(0)).slice(0, 3);
await both("headerfooter-added");

// Watermark in front, then the Bates number at the bottom centre.
await click(".mk-tabs button", "Watermark");
await set({ wmText: "CONFIDENTIAL", wmOpacity: 35 });
await both("watermark-preview");
await click(".mk .tl-foot .ax-btn.primary");
report.wmAdded = await until(`${M}.getState().sheet?.done != null || ${M}.getState().sheet?.error != null`);
report.wmUndo = await undoLabel();
await click(".mk-tabs button", "Bates");
await set({ btPrefix: "ACME-", btStart: 41, btSlot: 4 });
await both("bates-preview");
await click(".mk .tl-foot .ax-btn.primary");
await until(`${M}.getState().sheet?.done != null || ${M}.getState().sheet?.error != null`);
report.batesUndo = await undoLabel();
report.allKinds = await kinds();
report.hasRows = await ev(`[...document.querySelectorAll(".mk-has-row")].map((r) => r.textContent.trim().replace(/\\s+/g, " "))`);
await both("added-all");
report.bodyAfterAll = (await lines(0)).slice(0, 4);
report.allLines = await lines(0);

// One undo takes only the last mark back; redo puts it again.
await ctrl("z", "KeyZ", 90);
await sleep(700);
report.undoLabelAfter = await undoLabel();
report.afterUndo = await kinds();
await ctrl("y", "KeyY", 89);
await sleep(700);
report.afterRedo = await kinds();

// Save, close, open again: the marks are found and Update starts from what was written.
await ctrl("s", "KeyS", 83);
report.saved = await until(`!Object.values(${S}.getState().docs)[0].dirty`);
const id0 = await docId();
await click('.ax-icon[aria-label="Close Page marks"]');
await ev(`${S}.getState().closeDoc(${id0})`);
await sleep(600);
await ev(`${S}.getState().openPath(${JSON.stringify(docPath)})`);
report.reopened = await until(`Object.keys(${S}.getState().docs).length === 1`);
await sleep(500);
await ev(`${M}.getState().open("headerFooter")`);
report.reopenKinds = await until(`(${M}.getState().info[Object.keys(${S}.getState().docs)[0]] ?? []).length === 3`);
await sleep(400);
report.prefilled = await ev(`JSON.stringify(${M}.getState().sheet?.text)`);
await both("update-prefilled");

// Update the footer text: one undo step labelled as an update.
await set({ text: ["", "", "Lease agreement 2026", "", "", "Page <<1>> of <<n>>"] });
report.updateButton = await ev(`document.querySelector(".mk .tl-foot .ax-btn.primary")?.textContent.trim()`);
await click(".mk .tl-foot .ax-btn.primary");
await until(`${M}.getState().sheet?.done != null || ${M}.getState().sheet?.error != null`);
report.updateUndo = await undoLabel();
report.updateText = (await lines(0)).filter((l) => /Lease agreement|Page 1/.test(l));
await shot("updated-light.png");

// Remove the watermark and the header and footer; the page is the body again.
await click(".mk-has-row .tl-link", "Remove");
await sleep(700);
report.afterRemoveOne = await kinds();
await click(".mk .tl-foot .ax-btn.pr-quiet");
await sleep(700);
report.afterRemoveTwo = await kinds();
await ev(`${M}.getState().remove("bates")`);
await sleep(700);
report.afterRemoveAll = await kinds();
report.bodyAfterRemove = (await lines(0)).slice(0, 4);
await shot("removed-light.png");
await ctrl("z", "KeyZ", 90);
await sleep(500);
report.afterUndoRemove = await kinds();

await send("Emulation.setFocusEmulationEnabled", { enabled: false });
console.log(JSON.stringify(report, null, 2));
ws.close();
