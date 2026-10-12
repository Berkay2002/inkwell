// Drives a running Inkwell through WebView2's DevTools endpoint and saves the icon-system screens: the whole toolbar
// (light and dark, 100 / 125 / 150 % device scale, with the measured control heights), the All tools panel, the Home
// grid, a pinned tool, the palette and the hover card. Works on a COPY of a docs/study sample, never the user's files.
//   $d = "$env:TEMP\inkwell-icons"; mkdir $d\in, $d\data, $d\wv
//   Copy-Item docs\study\samples\edit-source.pdf $d\in\icons-sample.pdf
//   vite --port 1470 --strictPort            (any free port; the debug exe is built for 1420)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:WEBVIEW2_USER_DATA_FOLDER = "$d\wv"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9470'
//   inkwell.exe "$d\in\icons-sample.pdf"
//   node scripts/icons-shots.mjs <outDir> 9470 1470 [label] [toolbar|all]
// Page requests for the exe's dev origin (localhost:1420 unless it was built for another) are redirected to our vite port with Fetch.continueRequest (same origin, so the
// Tauri IPC keeps working). `label` goes into the file names ("before" / "after"); with `toolbar` only the toolbar
// shots are taken (what the before run needs).
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/icons";
const port = process.argv[3] ?? "9470";
const vitePort = process.argv[4];
const label = process.argv[5] ?? "after";
const only = process.argv[6] ?? "all";
fs.mkdirSync(out, { recursive: true });

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = targets.find((t) => t.type === "page");
// the origin the exe was built for (1420, or whatever its devUrl says): our vite stands in for it
const origin = new URL(page.url).origin;
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let seq = 0;
const waiting = new Map();
const send = (method, params = {}) =>
  new Promise((res, rej) => {
    const id = ++seq;
    waiting.set(id, (d) => (d.error ? rej(new Error(JSON.stringify(d.error))) : res(d.result)));
    ws.send(JSON.stringify({ id, method, params }));
  });
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id) waiting.get(d.id)?.(d);
  if (d.method === "Fetch.requestPaused" && vitePort) {
    const u = d.params.request.url;
    send("Fetch.continueRequest", { requestId: d.params.requestId, url: u.replace(origin, `http://localhost:${vitePort}`) }).catch(() => {});
  }
};
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
const save = async (name, params = {}) => {
  const r = await send("Page.captureScreenshot", { format: "png", ...params });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  console.log("screenshot", name);
};
const rectOf = (sel) =>
  ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null; const r = e.getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width, height: r.height } })()`);
const clip = async (name, selector, pad = 0) => {
  const r = await rectOf(selector);
  if (!r) throw new Error(`no ${selector}`);
  await save(name, { clip: { x: Math.max(0, r.x - pad), y: Math.max(0, r.y - pad), width: r.width + 2 * pad, height: r.height + 2 * pad, scale: 1 } });
};
const theme = (t) => ev(`${S}.getState().setTheme("${t}")`);
const scale = async (k) => {
  const [w, h] = await ev(`[window.innerWidth, window.innerHeight]`);
  await send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: k, mobile: false });
  await sleep(300);
};

// ---- get the app onto our dev server ---------------------------------------------------------------
await send("Page.enable");
await send("Network.enable");
if (vitePort) {
  await send("Network.setBlockedURLs", { urls: [`ws://${new URL(origin).host}/*`] });
  await send("Fetch.enable", { patterns: [{ urlPattern: `${origin}/*`, requestStage: "Request" }] });
  await send("Page.reload", { ignoreCache: true });
  await sleep(1500);
}
await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await until(`window.__inkwell && ${S}.getState().order.length >= 1`, "the open document", 300);
await ev(`localStorage.removeItem("inkwell.toolPrefs")`);
await ev(`${P}.setState({ recent: [], pinned: [], panelOpen: false })`);
const docId = await ev(`${S}.getState().order[0]`);
await ev(`${S}.getState().activate(${docId})`);
await until(`document.querySelectorAll(".page canvas.tile").length > 0`, "page tiles");
await ev(`${S}.getState().setZoomMode(${docId}, "fitWidth")`);
await ev(`${S}.getState().setSidebar("thumbs")`);
await sleep(600);

const report = { label, heights: {} };
const measure = () =>
  ev(`(() => {
    const h = (sel) => [...document.querySelectorAll(sel)].map((e) => { const r = e.getBoundingClientRect(); return { top: +r.top.toFixed(2), height: +r.height.toFixed(2) } });
    const tb = document.querySelector(".toolbar").getBoundingClientRect();
    return {
      toolbar: { top: +tb.top.toFixed(2), height: +tb.height.toFixed(2) },
      pill: h(".mode-pill"), pillItems: h(".mode-pill button"), tools: h(".tools-btn"),
      assistantGroup: h(".ax-toggle-group"), assistant: h(".ax-toggle:not(.tools-btn)"), layouts: h(".ax-layouts button"), iconBtn: h(".toolbar .icon-btn").slice(0, 3),
      // every visible button / select / input in the toolbar, its outer height and top: the distinct heights are what the token governs
      controls: [...document.querySelectorAll(".toolbar button, .toolbar select, .toolbar input")]
        .filter((e) => e.getBoundingClientRect().height > 0 && !e.closest(".ax-toggle-group, .ax-layouts"))
        .map((e) => { const r = e.getBoundingClientRect(); return { cls: (e.className || e.tagName).toString().slice(0, 28), label: e.getAttribute("aria-label") || e.title || "", top: +r.top.toFixed(2), height: +r.height.toFixed(2) } }),
    };
  })()`);

// ---- the toolbar ------------------------------------------------------------------------------------
for (const t of ["light", "dark"]) {
  await theme(t);
  for (const k of [1, 1.25, 1.5]) {
    await scale(k);
    report.heights[`${t}-${k}`] = await measure();
    await clip(`toolbar-${label}-${t}-${Math.round(k * 100)}.png`, ".toolbar");
  }
  await scale(1);
}
fs.writeFileSync(path.join(out, `toolbar-${label}-heights.json`), JSON.stringify(report, null, 2));

if (only !== "toolbar") {
  const toolsBtn = `document.querySelector('button[aria-label="Tools"]').click()`;
  const pins = ["compress", "ocr", "watermark"];
  await ev(`${P}.setState({ pinned: ${JSON.stringify(pins)}, recent: ["compress", "protect", "export"] })`);
  await sleep(300);
  for (const t of ["light", "dark"]) {
    await theme(t);
    await sleep(300);
    // All tools panel, with a hover card
    await ev(toolsBtn);
    await until(`document.querySelector("nav.tools-panel")`, "the panel");
    await sleep(400);
    await save(`panel-${t}.png`);
    await clip(`panel-clip-${t}.png`, "nav.tools-panel");
    const row = await ev(`(() => { const r = document.querySelector('.tools-panel .tool-main[data-tool="compress"]').getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 } })()`);
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: row.x, y: row.y });
    await until(`document.querySelector(".tool-card")`, "the hover card");
    await sleep(250);
    await clip(`hover-card-${t}.png`, ".tool-card", 8);
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 900, y: 500 });
    await ev(`document.querySelector('.tools-panel input[type=search]')?.focus()`);
    await ev(toolsBtn);
    await sleep(300);
    // pinned tools on the toolbar
    await clip(`pins-${t}.png`, ".toolbar");
    // the palette
    await ev(`${S}.getState().setUi({ paletteOpen: true })`);
    await until(`document.querySelector(".palette")`, "the palette");
    await sleep(200);
    await send("Input.insertText", { text: "comp" });
    await sleep(300);
    await clip(`palette-${t}.png`, ".palette", 12);
    await ev(`${S}.getState().setUi({ paletteOpen: false })`);
    await sleep(200);
  }
  // Home: the quick tools and the All tools grid
  await ev(`${S}.setState({ active: null })`);
  await sleep(600);
  for (const t of ["light", "dark"]) {
    await theme(t);
    await sleep(400);
    await save(`home-${t}.png`);
    await ev(`document.querySelector(".tools-grid")?.scrollIntoView({ block: "center" })`);
    await sleep(300);
    await save(`home-grid-${t}.png`);
  }
  await ev(`${S}.getState().activate(${docId})`);
  // 125 % / 150 % device scale: the All tools panel at the two sizes, to check strokes stay crisp
  await theme("light");
  await ev(toolsBtn);
  await until(`document.querySelector("nav.tools-panel")`, "the panel");
  for (const k of [1.25, 1.5]) {
    await scale(k);
    await sleep(200);
    await clip(`panel-clip-light-${Math.round(k * 100)}.png`, "nav.tools-panel");
    await clip(`toolbar-light-${Math.round(k * 100)}.png`, ".toolbar");
  }
  await scale(1);
  await ev(toolsBtn);
}
await send("Emulation.clearDeviceMetricsOverride");
console.log(JSON.stringify(report.heights["light-1"], null, 1));
ws.close();
