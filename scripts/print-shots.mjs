// Drives a running Inkwell through WebView2's DevTools endpoint: opens the Print sheet (Ctrl+P), changes the
// options, captures the screens (light and dark), and prints a COPY of a generated sample to "Microsoft Print to
// PDF" into INKWELL_PRINT_DIR (never a physical printer: the script stops if that printer is missing).
//   $d = "$env:TEMP\inkwell-print"; mkdir $d; node scripts/redact-sample.mjs "$d\Lease agreement.pdf"
//   npx vite --port 1420        (the debug exe loads the dev server)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:INKWELL_PRINT_DIR = "$d\out"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9226'
//   src-tauri\target\debug\inkwell.exe "$d\Lease agreement.pdf"
//   node scripts/print-shots.mjs docs/screens/print 9226 "$d\out"
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/print";
const port = process.argv[3] ?? "9226";
const printDir = process.argv[4];
const PRINTER = "Microsoft Print to PDF";
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
const P = "window.__inkwell.usePrint";
const sheet = () => ev(`${P}.getState().sheet`);
/** Waits until the sheet has a plan and the preview of the current side is drawn. */
async function settled(what) {
  for (let i = 0; i < 100; i++) {
    const s = await ev(`(() => { const s = ${P}.getState().sheet; return s && { plan: !!s.plan, err: s.planError, url: !!s.previewUrl, busy: s.previewing } })()`);
    if (s && s.plan && s.url && !s.busy) return;
    if (s?.err) throw new Error(`${what}: ${s.err}`);
    await sleep(100);
  }
  throw new Error(`${what}: preview did not settle`);
}
const shot = async (name, wait = 350) => {
  await sleep(wait);
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  console.log("screenshot", name);
};
const theme = (t) => ev(`${S}.getState().setTheme("${t}")`);
const set = async (p, what) => {
  await ev(`${P}.getState().set(${JSON.stringify(p)})`);
  await sleep(250);
  await settled(what ?? JSON.stringify(p));
};
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

// Ctrl+P through the page's own key handling (a synthetic event on window: no OS-level input)
await ev(`window.dispatchEvent(new KeyboardEvent("keydown", { key: "p", code: "KeyP", ctrlKey: true, bubbles: true, cancelable: true }))`);
await sleep(300);
if (!(await sheet())) throw new Error("Ctrl+P did not open the Print sheet");
for (let i = 0; i < 50 && !(await sheet())?.caps; i++) await sleep(100);
const s0 = await sheet();
report.printers = s0.printers.map((p) => p.name);
if (!report.printers.includes(PRINTER)) throw new Error(`"${PRINTER}" is not installed; refusing to print anywhere else`);
await set({ printer: PRINTER, copies: 1 }, "printer");
report.papers = (await sheet()).caps.papers.length;
await both("print-options");

// range, several per sheet, booklet, custom scale, grayscale
await set({ scope: "range", range: "2-5" });
report.rangePlan = (await sheet()).plan.faces.map((f) => f.pages);
await both("print-range");
await set({ scope: "all", mode: "multiple", perSheet: 4, border: true });
report.fourUp = (await sheet()).plan.sheets;
await both("print-4-up");
await set({ mode: "booklet", sides: "both", binding: "left", duplex: "shortEdge" });
report.booklet = (await sheet()).plan.faces.map((f) => f.pages);
await both("print-booklet");
await set({ mode: "size", sizeMode: "custom", percent: 60, grayscale: true });
await both("print-custom-gray");

// print the document, as set, to Microsoft Print to PDF
await set({ mode: "size", sizeMode: "fit", grayscale: false, scope: "range", range: "1-3", duplex: "off", copies: 2, collate: true });
report.planBeforePrint = { papers: (await sheet()).plan.papers, sides: (await sheet()).plan.sheets };
if (printDir) for (const f of fs.readdirSync(printDir)) fs.rmSync(path.join(printDir, f), { force: true });
await ev(`${P}.getState().start()`);
for (let i = 0; i < 150; i++) {
  const ph = (await sheet())?.phase;
  if (ph === "done" || ph === "error") break;
  await sleep(200);
}
const done = await sheet();
report.phase = done.phase;
report.error = done.error;
report.report = done.report;
await both("print-done");
if (printDir) {
  const files = fs.readdirSync(printDir);
  report.files = files.map((f) => {
    const b = fs.readFileSync(path.join(printDir, f));
    return { name: f, bytes: b.length, pdf: b.subarray(0, 5).toString() === "%PDF-", pages: (b.toString("latin1").match(/\/Type\s*\/Page(?![s\w])/g) ?? []).length };
  });
}
await ev(`${P}.getState().close()`);

await send("Emulation.setFocusEmulationEnabled", { enabled: false });
console.log(JSON.stringify(report, null, 2));
ws.close();
