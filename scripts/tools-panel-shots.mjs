// Drives a running Inkwell through WebView2's DevTools endpoint: the All tools panel (open, synonym search, the hover
// card, keyboard use, Recent chips, pinning with its limit of three), the Home tools grid and the palette subtitles,
// in the light and the dark app theme, on a COPY of a docs/study sample. Never the user's files.
//   $d = "$env:TEMP\inkwell-tools-panel"; mkdir $d\in, $d\data, $d\wv
//   python make_sample.py docs\study\samples\edit-source.pdf $d\in\tools-sample.pdf
//   vite --port 1433 --strictPort            (any free port; the debug exe is built for 1420)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:WEBVIEW2_USER_DATA_FOLDER = "$d\wv"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9248'
//   inkwell.exe "$d\in\tools-sample.pdf"
//   node scripts/tools-panel-shots.mjs docs/screens/tools-panel 9248 1433
// When another dev server already holds 1420 the page is redirected to ours with Fetch.continueRequest (same origin,
// so the Tauri IPC keeps working): pass our vite port as the third argument.
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/tools-panel";
const port = process.argv[3] ?? "9248";
const vitePort = process.argv[4];
fs.mkdirSync(out, { recursive: true });

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const ws = new WebSocket(targets.find((t) => t.type === "page").webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let seq = 0;
const waiting = new Map();
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id) waiting.get(d.id)?.(d);
  if (d.method === "Fetch.requestPaused" && vitePort) {
    const u = d.params.request.url;
    const target = u.replace(/^http:\/\/localhost:1420/, `http://localhost:${vitePort}`);
    send("Fetch.continueRequest", { requestId: d.params.requestId, url: target }).catch(() => {});
  }
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
const S = "window.__inkwell.useStore";
const P = "window.__inkwell.useToolPrefs";
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
const rectOf = (sel) =>
  ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null; const r = e.getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width, h: r.height } })()`);
const clip = async (name, selector) => {
  const r = await rectOf(selector);
  const res = await send("Page.captureScreenshot", { format: "png", clip: { x: r.x, y: r.y, width: r.w, height: r.h, scale: 1 } });
  fs.writeFileSync(path.join(out, name), Buffer.from(res.data, "base64"));
  console.log("screenshot", name);
};
const report = { checks: {} };
const check = (name, ok, detail) => {
  report.checks[name] = ok ? "ok" : `FAILED ${JSON.stringify(detail)}`;
  if (!ok) console.error("FAILED", name, JSON.stringify(detail));
};
const theme = (t) => ev(`${S}.getState().setTheme("${t}")`);
const key = async (k, code = k, vk = 0) => {
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: k, code, windowsVirtualKeyCode: vk });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, windowsVirtualKeyCode: vk });
  await sleep(80);
};
const type = async (text) => {
  await ev(`document.querySelector('.tools-panel input[type=search]').focus()`);
  await send("Input.insertText", { text });
  await sleep(150);
};
/** Empties the search box the way typing would, so React sees it. */
const clear = () =>
  ev(`(() => { const i = document.querySelector('.tools-panel input[type=search]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(i, ""); i.dispatchEvent(new Event("input", { bubbles: true })); i.focus(); })()`);
const names = () => ev(`[...document.querySelectorAll(".tools-panel .tool-name")].map((e) => e.textContent.trim())`);
const heads = () => ev(`[...document.querySelectorAll(".tools-panel h3")].map((e) => e.textContent.trim())`);
const panelOpen = () => ev(`!!document.querySelector("nav.tools-panel")`);
const openPanel = async () => {
  if (!(await panelOpen())) await ev(`document.querySelector('button[aria-label="Tools"]').click()`);
  await until(`document.querySelector("nav.tools-panel")`, "the panel");
  await sleep(150);
};
const closeSheets = () =>
  ev(`(() => { const w = window.__inkwell; w.useMarks.getState().close(); w.useTools.getState().close(); w.useConvert.getState().close(); })()`);

// ---- get the app onto our dev server ---------------------------------------------------------------
await send("Page.enable");
await send("Network.enable");
if (vitePort) {
  await send("Network.setBlockedURLs", { urls: ["ws://localhost:1420/*"] });
  await send("Fetch.enable", { patterns: [{ urlPattern: "http://localhost:1420/*", requestStage: "Request" }] });
  await send("Page.reload", { ignoreCache: true });
  await sleep(1500);
}
await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await until(`window.__inkwell && ${S}.getState().order.length >= 1`, "the open document", 300);
await ev(`localStorage.removeItem("inkwell.toolPrefs")`);
await ev(`${P}.setState({ recent: [], pinned: [], panelOpen: false })`);
const docs = await ev(`${S}.getState().order.map((id) => ({ id, name: ${S}.getState().docs[id].name }))`);
report.docs = docs;
await ev(`${S}.getState().activate(${docs[0].id})`);
await until(`document.querySelectorAll(".page canvas.tile").length > 0`, "page tiles");
await ev(`${S}.getState().setZoomMode(${docs[0].id}, "fitWidth")`);
await ev(`${S}.getState().setSidebar("thumbs")`);
await theme("light");
await sleep(800);

// ---- placement, open, landmark --------------------------------------------------------------------
const place = await ev(`(() => { const t = document.querySelector(".toolbar"); const all = [...t.children]; const pill = all.findIndex((e) => e.classList.contains("mode-pill")); const tools = all.findIndex((e) => e.matches('button[aria-label="Tools"]')); return { pill, tools } })()`);
check("Tools sits right after Read | Edit | Organize", place.tools === place.pill + 1, place);
const closedState = await ev(`({ expanded: document.querySelector('button[aria-label="Tools"]').getAttribute("aria-expanded"), panel: !!document.querySelector("nav.tools-panel"), thumbs: !!document.querySelector(".thumbs") })`);
check("the panel is closed to begin with, the thumbnails show", closedState.expanded === "false" && !closedState.panel && closedState.thumbs, closedState);
await openPanel();
const open1 = await ev(`(() => { const n = document.querySelector("nav.tools-panel"); const rows = [...n.querySelectorAll(".tool-main")]; return {
  tag: n.tagName, label: n.getAttribute("aria-label"), expanded: document.querySelector('button[aria-label="Tools"]').getAttribute("aria-expanded"),
  controls: document.querySelector('button[aria-label="Tools"]').getAttribute("aria-controls") === n.id,
  focus: document.activeElement.getAttribute("aria-label"), rows: rows.length,
  withDesc: rows.filter((r) => { const d = document.getElementById(r.getAttribute("aria-describedby")); return d && d.textContent.trim().length > 8 }).length,
  thumbs: !!document.querySelector(".thumbs"), width: Math.round(n.getBoundingClientRect().width) } })()`);
report.open = open1;
check("the panel is a navigation landmark named All tools", open1.tag === "NAV" && open1.label === "All tools", open1);
check("the Tools button reports expanded and controls the panel", open1.expanded === "true" && open1.controls, open1);
check("the search box has focus on open", open1.focus === "Find a tool", open1);
check("it replaces the sidebar while open", !open1.thumbs, open1);
check("every row has its description wired with aria-describedby", open1.rows >= 12 && open1.withDesc === open1.rows, open1);
const groupHeads = await heads();
check("tools sit under group headings", ["Optimize", "Page marks", "Protect", "Convert & create", "Check & compare"].every((h) => groupHeads.includes(h)), groupHeads);
await shot("panel-light.png", 300);

// ---- synonym search -------------------------------------------------------------------------------
await type("smaller");
const smaller = await names();
check("'smaller' finds Compress file", smaller[0] === "Compress file", smaller);
await shot("search-smaller-light.png", 250);
const syn = [
  ["shrink", "Compress file"],
  ["OCR", "Recognize text"],
  ["scan", "Recognize text"],
  ["stamp", "Watermark"],
  ["draft", "Watermark"],
];
for (const [q, want] of syn) {
  await clear();
  await type(q);
  const r = await names();
  check(`'${q}' finds ${want}`, r[0] === want, r);
}
await clear();
await type("page numbers");
const pn = await names();
check("'page numbers' finds Header & footer and Bates numbering", pn.includes("Header & footer") && pn.includes("Bates numbering"), pn);
await clear();
await type("qqzz");
check("no match says so", (await ev(`document.querySelector(".tools-none")?.textContent`))?.includes("No tool matches"), null);
await clear();
await sleep(150);
check("clearing the box brings the groups back", (await heads()).includes("Optimize"), await heads());

// ---- keyboard -------------------------------------------------------------------------------------
await ev(`document.querySelector('.tools-panel input[type=search]').focus()`);
await key("ArrowDown", "ArrowDown", 40);
const down = await ev(`({ cls: document.activeElement.className, tool: document.activeElement.dataset.tool })`);
check("arrow down moves from the box to the first row", /tool-main/.test(down.cls) && down.tool === "compress", down);
await key("ArrowDown", "ArrowDown", 40);
check("arrow down again moves to the next row", (await ev(`document.activeElement.dataset.tool`)) === "ocr", null);
await key("End", "End", 35);
const last = await ev(`document.activeElement.dataset.tool`);
const lastRow = await ev(`[...document.querySelectorAll(".tools-panel .tool-main")].at(-1).dataset.tool`);
check("End goes to the last row", last === lastRow, { last, lastRow });
await key("Home", "Home", 36);
check("Home goes back to the search box", (await ev(`document.activeElement.getAttribute("aria-label")`)) === "Find a tool", null);
await key("Escape", "Escape", 27);
await sleep(200);
const afterEsc = await ev(`({ open: !!document.querySelector("nav.tools-panel"), focus: document.activeElement?.getAttribute("aria-label"), thumbs: !!document.querySelector(".thumbs") })`);
check("Escape closes the panel and focus goes back to Tools", !afterEsc.open && afterEsc.focus === "Tools" && afterEsc.thumbs, afterEsc);
await ev(`document.querySelector('button[aria-label="Tools"]').click()`);
await until(`document.querySelector("nav.tools-panel")`, "the panel again");
await ev(`document.querySelector('button[aria-label="Tools"]').click()`);
await sleep(150);
check("the button closes it again", !(await panelOpen()), null);

// ---- hover card -----------------------------------------------------------------------------------
await openPanel();
const row = await ev(`(() => { const r = document.querySelector('.tools-panel .tool-main[data-tool="compress"]').getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 } })()`);
await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: row.x, y: row.y });
await sleep(250);
check("no card before the delay", (await ev(`!!document.querySelector(".tool-card")`)) === false, null);
await sleep(700);
const card = await ev(`(() => { const c = document.querySelector(".tool-card"); return c && { text: c.textContent, hidden: c.getAttribute("aria-hidden") } })()`);
report.card = card;
check("the card shows the fuller sentence and its shortcut", card && /Shrinks images/.test(card.text), card);
await shot("tooltip-card-light.png", 100);
await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: row.x + 600, y: row.y });
await sleep(200);
check("the card goes when the pointer leaves", (await ev(`!!document.querySelector(".tool-card")`)) === false, null);

// ---- running a tool, Recent -----------------------------------------------------------------------
await ev(`document.querySelector('.tools-panel .tool-main[data-tool="watermark"]').click()`);
await sleep(600);
const ran = await ev(`({ panel: !!document.querySelector("nav.tools-panel"), kind: window.__inkwell.useMarks.getState().sheet?.kind ?? null, recent: ${P}.getState().recent })`);
report.ran = ran;
check("a row runs the same action as before and closes the panel", !ran.panel && ran.kind === "watermark" && ran.recent[0] === "watermark", ran);
await closeSheets();
await sleep(300);
await openPanel();
await type("smaller");
await key("Enter", "Enter", 13);
await sleep(600);
const enter = await ev(`({ panel: !!document.querySelector("nav.tools-panel"), tool: window.__inkwell.useTools.getState().sheet?.tool ?? null, recent: ${P}.getState().recent })`);
check("Enter in the box runs the first match", enter.tool === "compress" && !enter.panel && enter.recent[0] === "compress", enter);
await closeSheets();
await sleep(300);
await ev(`${P}.getState().used("export"); ${P}.getState().used("bates")`);
await openPanel();
const chips = await ev(`[...document.querySelectorAll(".tools-panel .tool-chip")].map((e) => e.dataset.tool)`);
check("Recent lists the last tools used, newest first", JSON.stringify(chips) === JSON.stringify(["bates", "export", "compress", "watermark"]), chips);
await ev(`${P}.getState().used("a11y")`);
check("Recent keeps four", (await ev(`${P}.getState().recent.length`)) === 4, await ev(`${P}.getState().recent`));
await shot("recent-light.png", 250);

// ---- pinning --------------------------------------------------------------------------------------
for (const t of ["compress", "watermark", "export"]) await ev(`document.querySelector('.tools-panel .tool-row:has([data-tool="${t}"]) .tool-pin').click()`);
await sleep(200);
const pins = await ev(`({ stored: JSON.parse(localStorage.getItem("inkwell.toolPrefs")).pinned, bar: [...document.querySelectorAll(".pinned-tools button")].map((b) => b.dataset.tool), disabled: document.querySelectorAll(".tools-panel .tool-pin:disabled").length, rows: document.querySelectorAll(".tools-panel .tool-row").length, pressed: document.querySelectorAll('.tools-panel .tool-pin[aria-pressed="true"]').length })`);
report.pins = pins;
check("three pins show in the toolbar and persist", JSON.stringify(pins.bar) === JSON.stringify(["compress", "watermark", "export"]) && pins.stored.length === 3, pins);
check("a fourth cannot be pinned: the other pins are disabled", pins.disabled === pins.rows - 3 && pins.pressed === 3, pins);
await clip("toolbar-pinned-light.png", ".toolbar");
await shot("pinned-light.png", 200);
await ev(`document.querySelector('.tools-panel .tool-row:has([data-tool="watermark"]) .tool-pin').click()`);
await sleep(150);
check("unpinning frees a slot", (await ev(`document.querySelectorAll(".pinned-tools button").length`)) === 2 && (await ev(`document.querySelectorAll(".tools-panel .tool-pin:disabled").length`)) === 0, null);
await ev(`document.querySelector('.pinned-tools button[data-tool="export"]').click()`);
await sleep(500);
check("a pinned toolbar button runs its tool", (await ev(`window.__inkwell.useConvert.getState().sheet?.kind ?? window.__inkwell.useConvert.getState().sheet?.what ?? null`)) != null, null);
await closeSheets();
await sleep(200);
// another sidebar tab hands the slot back
await openPanel();
await ev(`${S}.getState().setSidebar("outline")`);
await sleep(200);
check("choosing Outline elsewhere closes the panel", !(await panelOpen()), null);
await ev(`${S}.getState().setSidebar("thumbs")`);

// ---- palette subtitles ----------------------------------------------------------------------------
await ev(`${S}.getState().setUi({ paletteOpen: true })`);
await until(`document.querySelector(".palette input")`, "the palette");
await send("Input.insertText", { text: "smaller" });
await sleep(200);
const pal = await ev(`[...document.querySelectorAll(".palette .item")].map((e) => ({ t: e.querySelector(".item-title")?.textContent, s: e.querySelector(".item-sub")?.textContent }))`);
report.palette = pal;
check("the palette finds Compress by 'smaller' and shows its description", pal[0]?.s === "Make the file smaller to email or upload", pal);
await shot("palette-light.png", 150);
await key("Escape", "Escape", 27);
await sleep(200);

// ---- Home grid ------------------------------------------------------------------------------------
await ev(`${S}.setState({ active: null })`);
await until(`document.querySelector(".tools-grid")`, "the Home grid");
await sleep(500);
const grid = await ev(`(() => { const cols = new Set([...document.querySelectorAll(".tools-grid > li")].map((l) => Math.round(l.getBoundingClientRect().left))); return { cards: document.querySelectorAll(".tool-card-btn").length, groups: [...document.querySelectorAll(".tools-group")].map((e) => e.textContent), columns: cols.size, withDesc: [...document.querySelectorAll(".tool-card-btn .tool-desc")].filter((e) => e.textContent.trim()).length } })()`);
report.grid = grid;
check("Home has a card per tool with its description, grouped", grid.cards >= 12 && grid.withDesc === grid.cards && grid.groups.length >= 5, grid);
check("the grid is three or four columns wide", grid.columns >= 3 && grid.columns <= 4, grid);
await ev(`document.querySelector(".tools-grid")?.scrollIntoView({ block: "start" })`);
await shot("home-grid-light.png", 400);
await ev(`document.querySelector('.tool-card-btn[data-tool="create"]').click()`);
await sleep(600);
check("a Home card that needs no file runs at once (Create PDF)", (await ev(`window.__inkwell.useConvert.getState().sheet != null`)) === true, null);
await closeSheets();
await ev(`${S}.getState().activate(${docs[0].id})`);
await sleep(600);

// ---- dark -----------------------------------------------------------------------------------------
await theme("dark");
await sleep(500);
await clip("toolbar-pinned-dark.png", ".toolbar");
await openPanel();
await shot("panel-dark.png", 300);
await type("smaller");
await shot("search-smaller-dark.png", 250);
await clear();
await sleep(150);
const row2 = await ev(`(() => { const r = document.querySelector('.tools-panel .tool-main[data-tool="ocr"]').getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 } })()`);
await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: row2.x, y: row2.y });
await sleep(900);
await shot("tooltip-card-dark.png", 100);
await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: row2.x + 600, y: row2.y });
await key("Escape", "Escape", 27);
await sleep(200);
await ev(`${S}.setState({ active: null })`);
await until(`document.querySelector(".tools-grid")`, "the Home grid in dark");
await ev(`document.querySelector(".tools-grid")?.scrollIntoView({ block: "start" })`);
await shot("home-grid-dark.png", 400);
await ev(`${S}.getState().activate(${docs[0].id})`);
await theme("light");

console.log(JSON.stringify(report, null, 2));
ws.close();
const failed = Object.values(report.checks).filter((v) => v !== "ok").length;
process.exit(failed ? 1 : 0);
