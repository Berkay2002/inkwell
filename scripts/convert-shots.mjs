// Captures the Export and Create PDF rail, light and dark, through WebView2's DevTools endpoint on a COPY of a
// sample PDF from samples/ (never the user's files). The native file dialogs cannot be driven without OS-level input,
// so the running and done states are put into the store directly (the same state the commands produce).
//   copy edit-source.pdf to <dir>; start the app with it:
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9334'; inkwell.exe <dir>/edit-source.pdf
//   node scripts/convert-shots.mjs docs/screens/convert 9334
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/convert";
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
const C = "window.__inkwell.useConvert";
const shot = async (name, wait = 400) => {
  await sleep(wait);
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  console.log("screenshot", name);
};
const theme = (t) => ev(`${S}.getState().setTheme("${t}")`);
const click = (sel, text) =>
  ev(`(() => { const b = [...document.querySelectorAll(${JSON.stringify(sel)})].find((e) => !${JSON.stringify(text ?? "")} || e.textContent.trim() === ${JSON.stringify(text ?? "")}); if (!b) return false; b.click(); return true })()`);
const set = (p) => ev(`${C}.getState().set(${JSON.stringify(p)})`);
/** Both themes for one state. */
async function both(name) {
  await theme("light");
  await shot(`${name}-light.png`);
  await theme("dark");
  await shot(`${name}-dark.png`);
  await theme("light");
}
const report = {};

await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await theme("light");
await sleep(900);

// Tools menu with the new entries
await click('button[aria-label="Tools"]');
await sleep(200);
report.menuItems = await ev(`[...document.querySelectorAll('.tools-panel .tool-name')].map((e) => e.textContent.trim())`);
await shot("tools-menu-light.png", 150);
await click(".tools-panel .tool-name", "Export");
await sleep(500);

// export options, page images; then each other format
report.exportOptions = await ev(`${C}.getState().sheet.what`);
await both("export-images");
await set({ imageFormat: "jpeg", dpi: 300, scope: "range", range: "1-2" });
await both("export-jpeg-range");
await set({ what: "text", scope: "all" });
await both("export-text");
await set({ what: "rtf" });
await both("export-rtf");
await set({ phase: "running", progress: 0.45, status: "Page 3 of 6" });
await both("export-running");
await set({ phase: "done", progress: 1 });
await ev(`${C}.setState((s) => ({ sheet: { ...s.sheet, exported: { files: ["C:\\\\Users\\\\you\\\\Documents\\\\edit-source-1.png", "C:\\\\Users\\\\you\\\\Documents\\\\edit-source-2.png"], folder: "C:\\\\Users\\\\you\\\\Documents", bytes: 1843200, pages: 2, warnings: [] } } }))`);
await both("export-done");
await click('.tl [aria-label^="Close"]');
await sleep(200);

// create options, on top of the open document
await click('button[aria-label="Tools"]');
await sleep(200);
await click(".tools-panel .tool-name", "Create PDF");
await sleep(500);
await ev(`${C}.setState((s) => ({ sheet: { ...s.sheet, from: "images", pageSize: "a4", margin: 24, files: [{ token: "a", path: "C:\\\\scans\\\\receipt-1.jpg" }, { token: "b", path: "C:\\\\scans\\\\receipt-2.jpg" }, { token: "c", path: "C:\\\\scans\\\\contract.tif" }] } }))`);
await both("create-images");
await set({ from: "blank", blankPages: 3, pageSize: "letter" });
await both("create-blank");
await click('.tl [aria-label^="Close"]');

// Home, Create PDF rail beside the landing page
const ids = await ev(`${S}.getState().order`);
for (const id of ids) await ev(`${S}.getState().closeDoc(${id})`);
await sleep(700);
await ev(`${C}.getState().openCreate()`);
await sleep(500);
await both("create-home");
report.homeRail = await ev(`!!document.querySelector(".home-wrap .ax-dock")`);
await ev(`${C}.getState().close()`);

await send("Emulation.setFocusEmulationEnabled", { enabled: false });
console.log(JSON.stringify(report, null, 2));
ws.close();
