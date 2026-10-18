// Walks the whole Windows app through WebView2's DevTools endpoint and saves one light and one dark screenshot per
// area at 1440x900 (plus the Home and the viewer at 1920x1080), a console log per area and a few automatic defect
// probes. Works on COPIES of samples, never on a user's files. It only records: it fixes nothing.
//   $d = "$env:TEMP\inkwell-tour"; mkdir $d\in, $d\data, $d\wv
//   Copy-Item samples\{edit-source,form-source,bookmarks-links-source,scan-source,redact-source,pages-a,pages-b}.pdf $d\in
//   vite --port 1433 --strictPort            (any free port; the debug exe is built for 1420)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:WEBVIEW2_USER_DATA_FOLDER = "$d\wv"
//   mkdir $d\hist; $env:INKWELL_HISTORY_ROOT = "$d\hist"   (else versions land in %LOCALAPPDATA%\Inkwell\history and a
//   second run shows the "same document as the one last changed" prompt over the page)
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9261'
//   inkwell.exe "$d\in\edit-source.pdf" "$d\in\form-source.pdf" ...      (the order is the tab order)
//   node scripts/tour-shots.mjs docs/screens/tour 9261 1433
// The first PNGs are taken with nothing open (the empty Home), then the files are opened from the launch arguments.
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/tour";
const port = process.argv[3] ?? "9261";
const vitePort = process.argv[4];
const only = process.argv[5] ? new RegExp(process.argv[5]) : null; // run just the areas whose key matches
fs.mkdirSync(out, { recursive: true });

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const ws = new WebSocket(targets.find((t) => t.type === "page").webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let seq = 0;
const waiting = new Map();
let area = "setup";
const consoleLog = [];
const note = (kind, text) => consoleLog.push({ area, kind, text: String(text).slice(0, 1500) });
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id) waiting.get(d.id)?.(d);
  if (d.method === "Fetch.requestPaused" && vitePort) {
    const u = d.params.request.url;
    const target = u.replace(/^http:\/\/localhost:1420/, `http://localhost:${vitePort}`);
    send("Fetch.continueRequest", { requestId: d.params.requestId, url: target }).catch(() => {});
  } else if (d.method === "Runtime.consoleAPICalled" && ["error", "warning", "assert"].includes(d.params.type)) {
    note(d.params.type, d.params.args.map((a) => a.value ?? a.description ?? a.type).join(" | "));
  } else if (d.method === "Runtime.exceptionThrown") {
    note("exception", d.params.exceptionDetails.exception?.description ?? d.params.exceptionDetails.text);
  } else if (d.method === "Log.entryAdded" && ["error", "warning"].includes(d.params.entry.level)) {
    note(`log-${d.params.entry.level}`, `${d.params.entry.text} ${d.params.entry.url ?? ""}`);
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
const W = "window.__inkwell";
const until = async (expr, what, tries = 100) => {
  for (let i = 0; i < tries; i++) {
    if (await ev(`(() => { try { return !!(${expr}) } catch { return false } })()`)) return;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${what}`);
};
const key = async (k, code = k, vk = 0) => {
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: k, code, windowsVirtualKeyCode: vk });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, windowsVirtualKeyCode: vk });
  await sleep(80);
};
const viewport = (width, height) => send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
const theme = (t) => ev(`${S}.getState().setTheme("${t}")`);

// ---- defect probes: leads for the README, not the list itself -------------------------------------
const PROBE = `(() => {
  const vis = (e) => { const r = e.getBoundingClientRect(); const cs = getComputedStyle(e); return r.width > 0 && r.height > 0 && cs.visibility !== "hidden" && cs.display !== "none" && +cs.opacity > 0.05 && r.bottom > 0 && r.right > 0 && r.top < innerHeight && r.left < innerWidth; };
  const sel = (e) => (e.tagName.toLowerCase() + (e.id ? "#" + e.id : "") + (typeof e.className === "string" && e.className ? "." + e.className.trim().split(/\\s+/).slice(0, 2).join(".") : ""));
  const txt = (e) => (e.innerText || e.getAttribute("aria-label") || e.getAttribute("title") || "").trim().replace(/\\s+/g, " ").slice(0, 50);
  const all = [...document.querySelectorAll("body *")].filter(vis);
  const clipped = [], zero = [], overlap = [], contrast = [];
  for (const e of all) {
    const cs = getComputedStyle(e);
    if (e.children.length === 0 && e.textContent.trim() && /hidden|clip|auto|scroll/.test(cs.overflowX) && e.scrollWidth > e.clientWidth + 1 && cs.textOverflow !== "ellipsis" && cs.whiteSpace !== "pre")
      clipped.push({ el: sel(e), text: txt(e), sw: e.scrollWidth, cw: e.clientWidth });
    if (e.children.length === 0 && e.textContent.trim() && cs.textOverflow === "ellipsis" && e.scrollWidth > e.clientWidth + 1)
      clipped.push({ el: sel(e), text: txt(e), sw: e.scrollWidth, cw: e.clientWidth, ellipsis: true });
  }
  for (const e of document.querySelectorAll("svg, img, canvas")) {
    const r = e.getBoundingClientRect();
    if ((r.width === 0 || r.height === 0) && getComputedStyle(e).display !== "none" && e.parentElement && vis(e.parentElement)) zero.push({ el: sel(e), parent: sel(e.parentElement) });
  }
  const btns = all.filter((e) => e.matches("button, [role=button], a, input, select"));
  for (let i = 0; i < btns.length; i++) for (let j = i + 1; j < btns.length; j++) {
    const a = btns[i], b = btns[j];
    if (a.contains(b) || b.contains(a)) continue;
    const ra = a.getBoundingClientRect(), rb = b.getBoundingClientRect();
    const w = Math.min(ra.right, rb.right) - Math.max(ra.left, rb.left), h = Math.min(ra.bottom, rb.bottom) - Math.max(ra.top, rb.top);
    if (w > 4 && h > 4) overlap.push({ a: sel(a) + " '" + txt(a) + "'", b: sel(b) + " '" + txt(b) + "'", w: Math.round(w), h: Math.round(h) });
  }
  const lum = (c) => { const [r, g, b] = c.map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); }); return 0.2126 * r + 0.7152 * g + 0.0722 * b; };
  const parse = (s) => { const m = s.match(/rgba?\\(([^)]+)\\)/); if (!m) return null; const p = m[1].split(",").map(Number); return { c: p.slice(0, 3), a: p.length > 3 ? p[3] : 1 }; };
  const bgOf = (e) => { for (let n = e; n; n = n.parentElement) { const p = parse(getComputedStyle(n).backgroundColor); if (p && p.a > 0.5) return p.c; } return [255, 255, 255]; };
  for (const e of all) {
    if (e.children.length || !e.textContent.trim() || e.closest("canvas, .page")) continue;
    const fg = parse(getComputedStyle(e).color); if (!fg) continue;
    const bg = bgOf(e);
    const l1 = lum(fg.c), l2 = lum(bg);
    const ratio = (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
    if (ratio < 3 && getComputedStyle(e).opacity !== "0") contrast.push({ el: sel(e), text: txt(e), ratio: Math.round(ratio * 10) / 10, fg: fg.c.join(","), bg: bg.join(",") });
  }
  const cut = (a) => a.slice(0, 12);
  return { clipped: cut(clipped), zero: cut(zero), overlap: cut(overlap), contrast: cut(contrast), counts: { clipped: clipped.length, zero: zero.length, overlap: overlap.length, contrast: contrast.length } };
})()`;

const index = []; // { file, area, what, theme, size }
const probes = {}; // file -> probe result
const shotOne = async (file, wait = 350) => {
  await sleep(wait);
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, file), Buffer.from(r.data, "base64"));
  try {
    const p = await ev(PROBE);
    const n = p.counts;
    if (n.clipped || n.zero || n.overlap || n.contrast) probes[file] = p;
  } catch (e) {
    note("probe-failed", e.message);
  }
  console.log("screenshot", file);
};

// ---- resets and openers ----------------------------------------------------------------------------
/** Closes every sheet, panel and menu we may have opened, back to the plain viewer of the active tab. */
const reset = async () => {
  await key("Escape", "Escape", 27);
  await key("Escape", "Escape", 27);
  await ev(`(() => {
    const w = window.__inkwell;
    const call = (store, fn, ...a) => { try { const st = w[store].getState(); if (typeof st[fn] === "function") st[fn](...a) } catch {} };
    // only the panels that are open: closing a closed one can still run its commit path (a stray toast)
    for (const k of ["useAi", "useHistory", "useRedact", "useAnnot", "useForms", "useSearch", "useTextEdit", "useA11y", "usePreflight", "useNav", "useSign"])
      if (w[k]?.getState().open === true) call(k, "setOpen", false);
    for (const k of ["useTools", "useConvert", "usePrint", "useProtect", "useMarks", "useCombine", "useBatch"]) call(k, "close");
    call("useHistory", "closeCompare");
    for (const f of ["closeLabels", "closeTrans", "closePresent"]) call("useSheets", f);
    call("useToolPrefs", "setPanel", false);
    call("useSign", "setRail", null);
    // panels whose store is not exposed (Output Preview, Signatures...): their own Close button
    for (let i = 0; i < 3; i++) for (const b of document.querySelectorAll("button")) {
      const l = (b.getAttribute("aria-label") || b.getAttribute("title") || "").trim();
      if (/^close(?! tab)/i.test(l) && b.offsetParent && !b.closest(".tabs, .tab, .winctl")) b.click();
    }
    const s = w.useStore.getState();
    s.setUi({ paletteOpen: false, searchOpen: false });
    if (s.active != null && s.docs[s.active]?.organize) s.patch(s.active, { organize: false });
    s.setSidebar("thumbs");
  })()`);
  await sleep(250);
};
const docIdOf = (re) => ev(`(() => { const d = Object.values(${S}.getState().docs).find((d) => new RegExp(${JSON.stringify(re)}, "i").test(d.name)); return d ? d.id : null })()`);
const activate = async (re) => {
  const id = await docIdOf(re);
  if (id == null) throw new Error(`no open document matches ${re}`);
  await ev(`${S}.getState().activate(${id})`);
  await until(`${S}.getState().active === ${id} && document.querySelectorAll(".page canvas.tile").length > 0`, `tab ${re}`, 150);
  await sleep(300);
  return id;
};
const home = async () => {
  await ev(`${S}.setState({ active: null })`);
  await until(`document.querySelector(".landing")`, "Home");
  await sleep(500);
};
const openPanel = async () => {
  if (!(await ev(`!!document.querySelector("nav.tools-panel")`))) await ev(`document.querySelector('button[aria-label="Tools"]').click()`);
  await until(`document.querySelector("nav.tools-panel")`, "the All tools panel");
  await sleep(150);
};
/** Runs a tool the way a person does: All tools panel, then the row. */
const tool = async (id) => {
  await openPanel();
  await ev(`document.querySelector('.tools-panel .tool-main[data-tool="${id}"]').click()`);
  await sleep(700);
};
const btn = (re, scope = ".toolbar") =>
  ev(`(() => { const b = [...document.querySelectorAll(${JSON.stringify(scope)} + " button")].find((b) => new RegExp(${JSON.stringify(re)}, "i").test(b.getAttribute("title") || b.getAttribute("aria-label") || b.textContent)); if (!b) return false; b.click(); return true })()`);
const cmd = async (title) => {
  await ev(`${S}.getState().setUi({ paletteOpen: true })`);
  await until(`document.querySelector(".palette input")`, "the palette");
  await send("Input.insertText", { text: title });
  await sleep(250);
  await key("Enter", "Enter", 13);
  await sleep(700);
};
const typeInto = async (selector, text) => {
  await ev(`document.querySelector(${JSON.stringify(selector)}).focus()`);
  await send("Input.insertText", { text });
  await sleep(300);
};

// ---- the areas ---------------------------------------------------------------------------------------
// Each: key (file name part), what (index text), doc (regex of the tab to use, null = Home), open (async).
const AREAS = [
  { key: "home-empty", what: "Home with nothing opened yet: the empty state", doc: null, emptyOnly: true, wide: true },
  { key: "home", what: "Home with recent files and the Quick tools row", doc: null, wide: true, open: async () => {} },
  { key: "viewer", what: "Viewer, one document, continuous scroll, thumbnails open", doc: "edit-source", wide: true, open: async () => {
    await ev(`${S}.getState().setZoomMode(${await docIdOf("edit-source")}, "fitWidth")`);
  } },
  { key: "palette", what: "Command palette with a query", doc: "edit-source", open: async () => {
    await ev(`${S}.getState().setUi({ paletteOpen: true })`);
    await until(`document.querySelector(".palette input")`, "the palette");
    await send("Input.insertText", { text: "page" });
  } },
  { key: "all-tools", what: "All tools panel", doc: "edit-source", open: openPanel },
  { key: "sidebar-outline", what: "Outline in the sidebar", doc: "bookmarks-links", open: async () => ev(`${S}.getState().setSidebar("outline")`) },
  { key: "sidebar-attachments", what: "Attachments in the sidebar", doc: "edit-source", open: async () => ev(`${S}.getState().setSidebar("attachments")`) },
  { key: "organize", what: "Organize pages", doc: "pages-a", open: async () => ev(`${S}.getState().patch(${S}.getState().active, { organize: true })`) },
  { key: "edit", what: "Edit text & images", doc: "edit-source", open: async () => ev(`${W}.useTextEdit.getState().setOpen(true)`) },
  { key: "links", what: "Links tool", doc: "bookmarks-links", open: () => tool("links") },
  { key: "bookmarks", what: "Bookmarks tool", doc: "bookmarks-links", open: () => tool("bookmarks") },
  { key: "articles", what: "Article threads", doc: "edit-source", open: () => tool("articles") },
  { key: "comment", what: "Comments panel", doc: "edit-source", open: async () => ev(`${W}.useAnnot.getState().setOpen(true)`) },
  { key: "measure", what: "Measure", doc: "edit-source", open: () => tool("measure") },
  { key: "forms", what: "Forms panel on a form", doc: "form-source", open: async () => ev(`${W}.useForms.getState().setOpen(true)`) },
  { key: "sign-fill", what: "Fill & Sign", doc: "edit-source", open: () => tool("fill-sign") },
  { key: "sign-digital", what: "Sign with a Digital ID (dialog only, nothing is signed)", doc: "edit-source", open: () => tool("sign-digital") },
  { key: "signatures", what: "Signatures panel", doc: "edit-source", open: () => tool("signatures") },
  { key: "protect", what: "Protect with a password", doc: "edit-source", open: () => tool("protect") },
  { key: "security", what: "Document properties and security", doc: "edit-source", open: () => tool("security") },
  { key: "redact", what: "Redact", doc: "redact-source", open: async () => ev(`${W}.useRedact.getState().setOpen(true)`) },
  { key: "compress", what: "Compress file", doc: "edit-source", open: () => tool("compress") },
  { key: "ocr", what: "Recognize text (OCR) on a scan", doc: "scan-source", open: () => tool("ocr") },
  { key: "preflight", what: "Preflight", doc: "edit-source", open: () => tool("preflight") },
  { key: "export", what: "Export pages, text or images", doc: "edit-source", open: () => tool("export") },
  { key: "create", what: "Create PDF", doc: "edit-source", open: () => tool("create") },
  { key: "compare", what: "Compare files: setup", doc: "pages-a", open: () => tool("compare") },
  { key: "print", what: "Print", doc: "edit-source", open: async () => ev(`${W}.usePrint.getState().open()`) },
  { key: "output-preview", what: "Output Preview", doc: "edit-source", open: () => tool("output-preview") },
  { key: "convert-colors", what: "Convert Colors to CMYK", doc: "edit-source", open: () => tool("convert-colors") },
  { key: "printer-marks", what: "Printer's marks", doc: "edit-source", open: () => tool("printer-marks") },
  { key: "page-boxes", what: "Page boxes (trim, bleed)", doc: "edit-source", open: () => tool("page-boxes") },
  { key: "header-footer", what: "Header & footer", doc: "edit-source", open: () => tool("header-footer") },
  { key: "watermark", what: "Watermark", doc: "edit-source", open: () => tool("watermark") },
  { key: "background", what: "Background", doc: "edit-source", open: () => tool("background") },
  { key: "bates", what: "Bates numbering", doc: "edit-source", open: () => tool("bates") },
  { key: "batch", what: "Action Wizard (batch)", doc: "edit-source", open: () => tool("action-wizard") },
  { key: "combine", what: "Combine files", doc: "pages-a", open: async () => ev(`${W}.useCombine.getState().open()`) },
  { key: "forms-prepare", what: "Forms: Prepare form", doc: "form-source", open: async () => {
    await ev(`${W}.useForms.getState().setOpen(true)`);
    await sleep(400);
    await btn("^Prepare form$", ".forms-panel, aside, .ax-dock");
  } },
  { key: "assistant", what: "AI Assistant panel", doc: "edit-source", open: async () => ev(`${W}.useAi.getState().toggle()`) },
  { key: "history", what: "Version history", doc: "edit-source", open: () => btn("history") },
  { key: "search", what: "Find bar with hits", doc: "edit-source", open: async () => {
    await ev(`${S}.getState().setUi({ searchOpen: true })`);
    await sleep(300);
    await send("Input.insertText", { text: "the" });
    await sleep(900);
  } },
  { key: "a11y", what: "Accessibility check", doc: "edit-source", open: () => tool("a11y") },
  { key: "doc-javascript", what: "Document JavaScript", doc: "form-source", open: () => tool("doc-javascript") },
  { key: "settings", what: "Settings menu on Home", doc: null, open: async () => {
    await ev(`document.querySelector(".land-top .gear").click()`);
    await sleep(400);
  } },
  // last: it has crashed the whole page before
  { key: "search-advanced", what: "Advanced search", doc: "edit-source", open: () => tool("search-advanced") },
];

// ---- get going -----------------------------------------------------------------------------------------
await send("Page.enable");
await send("Network.enable");
await send("Runtime.enable");
await send("Log.enable");
if (vitePort) {
  await send("Network.setBlockedURLs", { urls: ["ws://localhost:1420/*"] });
  await send("Fetch.enable", { patterns: [{ urlPattern: "http://localhost:1420/*", requestStage: "Request" }] });
  await send("Page.reload", { ignoreCache: true });
  await sleep(2000);
}
await send("Emulation.setFocusEmulationEnabled", { enabled: true });
// TOUR_EMPTY=1: a launch with no arguments and a fresh data dir, for the first-run Home only (area home-empty)
await until(`window.__inkwell && (${process.env.TOUR_EMPTY === "1" ? "true" : `${S}.getState().order.length >= 1`})`, "the open documents", 300);
await ev(`localStorage.removeItem("inkwell.toolPrefs")`);
const docs = await ev(`${S}.getState().order.map((id) => ${S}.getState().docs[id].name)`);
console.log("open tabs:", docs.join(", "));

const results = {};
let n = 0;
for (const a of AREAS) {
  n += 1;
  if (only && !only.test(a.key)) continue;
  if (!!a.emptyOnly !== (process.env.TOUR_EMPTY === "1")) continue;
  const nn = String(n).padStart(2, "0");
  area = a.key;
  results[a.key] = "ok";
  try {
    for (const th of ["light", "dark"]) {
      if (th === "light") {
        await reset();
        await viewport(1440, 900);
        if (a.doc === null) await home();
        else await activate(a.doc);
        await theme("light");
        if (a.open) await a.open();
        await sleep(400);
      } else await theme("dark");
      const file = `${nn}-${a.key}-${th}.png`;
      await shotOne(file, th === "light" ? 450 : 600);
      index.push({ file, area: a.key, what: a.what, theme: th, size: "1440x900" });
      if (th === "dark" && a.wide) {
        for (const t2 of ["dark", "light"]) {
          await viewport(1920, 1080);
          await theme(t2);
          const f2 = `${nn}-${a.key}-1920-${t2}.png`;
          await shotOne(f2, 600);
          index.push({ file: f2, area: a.key, what: a.what, theme: t2, size: "1920x1080" });
        }
        await viewport(1440, 900);
      }
    }
  } catch (e) {
    results[a.key] = `FAILED ${e.message.slice(0, 200)}`;
    note("area-failed", e.message);
    console.error("FAILED", a.key, e.message);
    // a crashed React tree leaves a blank page: reload (the launch files open again) and carry on
    if (!(await ev(`document.body.innerText.length`).catch(() => 0))) {
      await send("Page.reload", { ignoreCache: true });
      await sleep(2500);
      await until(`window.__inkwell && ${S}.getState().order.length >= 1`, "the documents after a reload", 300).catch(() => {});
    }
  }
}
area = "end";
await theme("light");
await reset();
await send("Emulation.clearDeviceMetricsOverride");

fs.writeFileSync(path.join(out, "tour.json"), JSON.stringify({ tabs: docs, results, index, probes, console: consoleLog }, null, 1));
console.log(JSON.stringify({ results, consoleCount: consoleLog.length, probed: Object.keys(probes).length }, null, 1));
ws.close();
process.exit(0);
