// Drives a running Inkwell through WebView2's DevTools endpoint: shows a COPY of a docs/study sample (a photo, a coloured
// link line, colour swatches) with normal and dark pages in the light and dark app theme, checks that the toggle, the
// three-way setting and the persistence work, that the photo keeps its colours and the chrome is not inverted, and
// captures search hits, a text selection, a comment and a form field on dark pages. Never the user's files.
//   $d = "$env:TEMP\inkwell-page-dark"; mkdir $d\in, $d\data, $d\wv
//   python make_sample.py docs\study\samples\edit-source.pdf $d\in\dark-sample.pdf ; copy form-source.pdf -> $d\in
//   vite --port 1433 --strictPort            (any free port; the debug exe is built for 1420, see PAGE_DARK_VITE below)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:WEBVIEW2_USER_DATA_FOLDER = "$d\wv"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9247'
//   node scripts/make-rgb-photo.mjs $d\in\rgb-photo.pdf         (six flat colours for the exact-colour check)
//   foreach ($n in 1,91,1500) { node scripts/make-blank-pages.mjs $d\in\pages-$n.pdf $n }   (page-number box sizes)
//   inkwell.exe "$d\in\dark-sample.pdf" "$d\in\form-source.pdf" "$d\in\rgb-photo.pdf" "$d\in\pages-1.pdf" "$d\in\pages-91.pdf" "$d\in\pages-1500.pdf"
//   node scripts/page-dark-shots.mjs docs/screens/page-dark 9247 1433
// When another dev server already holds 1420 the page is redirected to ours with Fetch.continueRequest (same origin,
// so the Tauri IPC keeps working): pass our vite port as the third argument.
import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";

const out = process.argv[2] ?? "docs/screens/page-dark";
const port = process.argv[3] ?? "9247";
const vitePort = process.argv[4];
fs.mkdirSync(out, { recursive: true });

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const ws = new WebSocket(targets.find((t) => t.type === "page").webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let seq = 0;
const waiting = new Map();
const events = [];
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id) waiting.get(d.id)?.(d);
  else events.push(d);
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
const until = async (expr, what, tries = 150) => {
  for (let i = 0; i < tries; i++) {
    if (await ev(`(() => { try { return !!(${expr}) } catch { return false } })()`)) return;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${what}`);
};

// ---- png -----------------------------------------------------------------------------------------
function decodePng(buf) {
  let p = 8;
  let w = 0, h = 0, bpp = 4;
  const idat = [];
  while (p < buf.length) {
    const len = buf.readUInt32BE(p);
    const type = buf.toString("ascii", p + 4, p + 8);
    const body = buf.subarray(p + 8, p + 8 + len);
    if (type === "IHDR") {
      w = body.readUInt32BE(0);
      h = body.readUInt32BE(4);
      bpp = body[9] === 6 ? 4 : 3;
    } else if (type === "IDAT") idat.push(body);
    p += 12 + len;
  }
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = w * bpp;
  const px = Buffer.alloc(h * stride);
  for (let y = 0; y < h; y++) {
    const f = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const a = x >= bpp ? px[y * stride + x - bpp] : 0;
      const b = y > 0 ? px[(y - 1) * stride + x] : 0;
      const c = x >= bpp && y > 0 ? px[(y - 1) * stride + x - bpp] : 0;
      let v = line[x];
      if (f === 1) v += a;
      else if (f === 2) v += b;
      else if (f === 3) v += (a + b) >> 1;
      else if (f === 4) {
        const pp = a + b - c;
        const pa = Math.abs(pp - a), pb = Math.abs(pp - b), pc = Math.abs(pp - c);
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      px[y * stride + x] = v & 255;
    }
  }
  return { w, h, bpp, px };
}
/** Mean colour of a device-pixel rectangle. */
function mean(png, r) {
  const x0 = Math.max(0, Math.round(r.x)), y0 = Math.max(0, Math.round(r.y));
  const x1 = Math.min(png.w, Math.round(r.x + r.w)), y1 = Math.min(png.h, Math.round(r.y + r.h));
  const sum = [0, 0, 0];
  let n = 0;
  for (let y = y0; y < y1; y++)
    for (let x = x0; x < x1; x++) {
      const i = (y * png.w + x) * png.bpp;
      sum[0] += png.px[i];
      sum[1] += png.px[i + 1];
      sum[2] += png.px[i + 2];
      n++;
    }
  return sum.map((s) => Math.round(s / Math.max(n, 1)));
}
const dist = (a, b) => Math.max(...a.map((v, i) => Math.abs(v - b[i])));

let dpr = 1;
const shot = async (name, wait = 350) => {
  await sleep(wait);
  const r = await send("Page.captureScreenshot", { format: "png" });
  const buf = Buffer.from(r.data, "base64");
  fs.writeFileSync(path.join(out, name), buf);
  console.log("screenshot", name);
  return decodePng(buf);
};
const rectOf = (sel) =>
  ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null; const r = e.getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width, h: r.height } })()`);
const dev = (r) => r && { x: r.x * dpr, y: r.y * dpr, w: r.w * dpr, h: r.h * dpr };
const report = { checks: {} };
const check = (name, ok, detail) => {
  report.checks[name] = ok ? "ok" : `FAILED ${JSON.stringify(detail)}`;
  if (!ok) console.error("FAILED", name, JSON.stringify(detail));
};
const setTheme = (t) => ev(`${S}.getState().setTheme("${t}")`);
const setPages = (m) => ev(`${S}.getState().setPageDark("${m}")`);
const pagesAttr = () => ev(`document.documentElement.dataset.pages ?? null`);
const stored = () => ev(`localStorage.getItem("inkwell.pageDark")`);
const TOGGLE = '.toolbar [aria-label^="Dark pages"], .toolbar [aria-label^="Normal pages"]';
const toggleLabel = () =>
  ev(`(() => { const b = document.querySelector(${JSON.stringify(TOGGLE)}); return b ? { label: b.getAttribute("aria-label").replace(/ \\(.*$/, ""), on: b.classList.contains("on") } : null })()`);
const clickToggle = () => ev(`document.querySelector(${JSON.stringify(TOGGLE)}).click()`);

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
await until(`window.__inkwell && ${S}.getState().order.length >= 2`, "the two open documents", 300);
dpr = await ev("window.devicePixelRatio");
await ev(`localStorage.removeItem("inkwell.pageDark")`);
await ev(`${S}.getState().setPageDark("follow")`);
const docs = await ev(`${S}.getState().order.map((id) => ({ id, name: ${S}.getState().docs[id].name }))`);
const sample = docs.find((d) => /dark-sample/.test(d.name));
const form = docs.find((d) => /form-source/.test(d.name));
report.docs = docs;
await ev(`${S}.getState().activate(${sample.id})`);
await until(`document.querySelectorAll(".page canvas.tile").length > 0`, "page tiles");
await ev(`${S}.getState().setZoomMode(${sample.id}, "fitWidth")`);
await sleep(1200);
await until(`document.querySelectorAll(".page canvas.tile:not(.preview)").length > 0`, "sharp tiles", 200);

// ---- normal pages, light app --------------------------------------------------------------------
await setTheme("light");
await sleep(300);
check("follow + light app: pages normal", (await pagesAttr()) === null, await pagesAttr());
const tb0 = await toggleLabel();
check("toolbar toggle says Dark pages", tb0?.label === "Dark pages" && !tb0.on, tb0);
const rev0 = await ev(`${S}.getState().docs[${sample.id}].rev`);
const img = await ev(`document.querySelectorAll(".nat-img").length`);
check("no photo overlays while pages are normal", img === 0, img);
const normal = await shot("pages-normal-light-app.png");
const pageBox = dev(await rectOf(".page"));
const corner = { x: pageBox.x + 6, y: pageBox.y + 6, w: 24, h: 24 };
const tabbar = dev(await rectOf(".tabbar"));
const lightCorner = mean(normal, corner);
const lightTab = mean(normal, tabbar);
check("normal page margin is white", lightCorner.every((v) => v > 235), lightCorner);

// ---- the toggle ----------------------------------------------------------------------------------
await clickToggle();
await sleep(500);
const afterOn = { attr: await pagesAttr(), setting: await ev(`${S}.getState().pageDark`), stored: await stored(), tb: await toggleLabel() };
report.afterOn = afterOn;
check("toggle turns dark pages on in the light app", afterOn.attr === "dark" && afterOn.setting === "on" && afterOn.stored === "on", afterOn);
check("toolbar toggle says Normal pages when on", afterOn.tb?.label === "Normal pages" && afterOn.tb.on, afterOn.tb);
await until(`document.querySelectorAll(".nat-img").length > 0`, "the photo to be marked natural", 100);
const natN = await ev(`document.querySelectorAll(".nat-img").length`);
report.photoOverlays = natN;
const dark = await shot("pages-dark-light-app.png", 700);
const darkCorner = mean(dark, corner);
// paper becomes a dark grey (about #1c1c1c), not pure black, and sits above the viewer's canvas (#0e0f11)
check("dark page margin is a dark grey", darkCorner.every((v) => v >= 22 && v <= 40), darkCorner);
const pressed = await ev(`document.querySelector(${JSON.stringify(TOGGLE)}).getAttribute("aria-pressed")`);
check("toggle exposes aria-pressed", pressed === "true", pressed);
const darkTab = mean(dark, tabbar);
check("app chrome is not inverted", dist(lightTab, darkTab) <= 3, { lightTab, darkTab });
const nat = dev(await rectOf(".nat-img"));
const inner = { x: nat.x + nat.w * 0.1, y: nat.y + nat.h * 0.1, w: nat.w * 0.8, h: nat.h * 0.8 };
const photoNormal = mean(normal, inner);
const photoDark = mean(dark, inner);
report.photo = { photoNormal, photoDark };
check("the photo keeps its own colours", dist(photoNormal, photoDark) <= 8, { photoNormal, photoDark });
check("the file is untouched", (await ev(`${S}.getState().docs[${sample.id}].rev`)) === rev0 && !(await ev(`${S}.getState().docs[${sample.id}].dirty`)), null);
const filtered = await ev(`[...document.querySelectorAll("*")].filter((e) => getComputedStyle(e).filter !== "none").map((e) => e.className?.baseVal ?? e.className)`);
report.filtered = [...new Set(filtered)];
check("only page images are filtered", filtered.length > 0 && filtered.every((c) => /\btile\b|mk-preview/.test(c)), report.filtered);

await clickToggle();
await sleep(300);
const back = { attr: await pagesAttr(), setting: await ev(`${S}.getState().pageDark`), stored: await stored() };
check("toggling again returns to follow in the light app", back.attr === null && back.setting === "follow", back);
await ev(`document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "d", code: "KeyD", ctrlKey: true, altKey: true, bubbles: true, cancelable: true }))`);
await sleep(300);
check("Ctrl+Alt+D toggles dark pages", (await pagesAttr()) === "dark", await pagesAttr());
await ev(`document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "d", code: "KeyD", ctrlKey: true, altKey: true, bubbles: true, cancelable: true }))`);
await sleep(200);

// ---- placement and the choice menu -----------------------------------------------------------------
// page box | zoom out, Fit width, zoom in, rotate | dark pages
const order = await ev(`(() => { const t = document.querySelector(".toolbar"); const all = [...t.querySelectorAll("button, select, #page-input, .dp-split")]; const idx = (s) => all.indexOf(t.querySelector(s)); return { page: idx("#page-input"), zoom: idx(".zoom-select"), rotate: all.findIndex((b) => /^Rotate/.test(b.getAttribute?.("aria-label") ?? "")), dark: idx(".dp-split") } })()`);
report.toolbarOrder = order;
check("dark pages sits after rotate, after the zoom group", order.page < order.zoom && order.zoom < order.rotate && order.rotate < order.dark, order);
const clip = async (name, selector) => {
  const r = await rectOf(selector);
  const res = await send("Page.captureScreenshot", { format: "png", clip: { x: r.x, y: r.y, width: r.w, height: r.h, scale: 1 } });
  fs.writeFileSync(path.join(out, name), Buffer.from(res.data, "base64"));
  console.log("screenshot", name);
};
await setPages("off");
await sleep(300);
await clip("toolbar-normal-light-app.png", ".toolbar");
await setPages("on");
await sleep(300);
await clip("toolbar-dark-light-app.png", ".toolbar");
// the chevron opens the choice
await ev(`document.querySelector(".dp-chev").click()`);
await until(`document.querySelector(".ink-menu")`, "the page colours menu");
const items = await ev(`[...document.querySelectorAll(".ink-menu button")].map((b) => ({ t: b.textContent.trim(), c: !!b.querySelector("svg") }))`);
report.menu = items;
check("menu offers follow, dark, normal and marks the current one", items.length === 3 && items[1].c && !items[0].c && !items[2].c, items);
await shot("_probe.png");
fs.rmSync(path.join(out, "_probe.png"), { force: true });
await ev(`[...document.querySelectorAll(".ink-menu button")].find((b) => /Always normal/.test(b.textContent)).click()`);
await sleep(300);
check("the menu sets always normal", (await ev(`${S}.getState().pageDark`)) === "off" && (await pagesAttr()) === null, await ev(`${S}.getState().pageDark`));
// right-click on the button opens the same menu
await ev(`document.querySelector(".dp-split button").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }))`);
await until(`document.querySelector(".ink-menu")`, "the menu on right-click");
await ev(`[...document.querySelectorAll(".ink-menu button")].find((b) => /Follow/.test(b.textContent)).click()`);
await sleep(300);
check("right-click menu sets follow", (await ev(`${S}.getState().pageDark`)) === "follow", await ev(`${S}.getState().pageDark`));

// ---- Tools menu: beside the mode pill, grouped, filterable -------------------------------------------
await setPages("follow");
const before = await ev(`(() => { const t = document.querySelector(".toolbar"); const all = [...t.children]; const pill = all.findIndex((e) => e.classList.contains("mode-pill")); const tools = all.findIndex((e) => e.matches('button[aria-label="Tools"]')); const sep = all.findIndex((e, i) => i > tools && e.classList.contains("sep")); return { pill, tools, sep, rightTools: !!t.querySelector(".tb-cluster > button[aria-label='Tools']") } })()`);
report.toolsPlacement = before;
check("Tools sits right after Read | Edit | Organize, not in the right-hand cluster", before.tools === before.pill + 1 && !before.rightTools, before);
await ev(`document.querySelector('button[aria-label="Tools"]').click()`);
await until(`document.querySelector(".tools-menu .tools-filter")`, "the Tools menu");
const menu1 = await ev(`({ heads: [...document.querySelectorAll(".tools-head")].map((e) => e.textContent), items: [...document.querySelectorAll('.tools-menu [role="option"]')].map((e) => e.textContent.trim()), focus: document.activeElement.className })`);
report.toolsMenu = menu1;
const want = ["Compress file…", "Recognize text…", "Header & footer…", "Watermark…", "Background…", "Bates numbering…", "Protect with a password…", "Security…", "Compare files…", "Check accessibility…", "Export…", "Create PDF…"];
check("every Tools item is still there", want.every((w) => menu1.items.includes(w)), menu1.items);
check("items sit under Optimize, Page marks, Protect, Convert & create headers", ["Optimize", "Page marks", "Protect", "Convert & create"].every((h) => menu1.heads.includes(h)), menu1.heads);
check("the filter box has focus on open", /tools-filter/.test(menu1.focus), menu1.focus);
await clip("tools-button-light.png", ".toolbar");
await shot("tools-menu-light.png", 300);
// typing narrows the list
await ev(`(() => { const i = document.querySelector(".tools-filter"); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(i, "mark"); i.dispatchEvent(new Event("input", { bubbles: true })); })()`);
await sleep(250);
const narrowed = await ev(`({ heads: [...document.querySelectorAll(".tools-head")].map((e) => e.textContent), items: [...document.querySelectorAll('.tools-menu [role="option"]')].map((e) => e.textContent.trim()) })`);
report.toolsFiltered = narrowed;
check("typing 'mark' leaves the page marks", narrowed.heads.length === 1 && narrowed.heads[0] === "Page marks" && narrowed.items.length === 4, narrowed);
// keyboard: focus stays in the box (combobox + listbox); the arrows move the highlighted option, Escape closes
const sel0 = await ev(`(() => { const i = document.querySelector(".tools-filter"); const o = document.getElementById(i.getAttribute("aria-activedescendant")); return { role: i.getAttribute("role"), expanded: i.getAttribute("aria-expanded"), list: document.getElementById(i.getAttribute("aria-controls"))?.getAttribute("role"), text: o?.textContent.trim(), selected: o?.getAttribute("aria-selected") } })()`);
report.toolsCombobox = sel0;
check("the filter is a combobox on a listbox, first match highlighted", sel0.role === "combobox" && sel0.expanded === "true" && sel0.list === "listbox" && sel0.selected === "true" && sel0.text === "Header & footer…", sel0);
await ev(`document.querySelector(".tools-filter").dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }))`);
const sel1 = await ev(`(() => { const i = document.querySelector(".tools-filter"); return { text: document.getElementById(i.getAttribute("aria-activedescendant"))?.textContent.trim(), focus: document.activeElement === i } })()`);
check("arrow down moves the highlight, focus stays in the box", sel1.text === "Watermark…" && sel1.focus, sel1);
await ev(`document.querySelector(".tools-filter").dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }))`);
check("arrow up moves it back", (await ev(`(() => { const i = document.querySelector(".tools-filter"); return document.getElementById(i.getAttribute("aria-activedescendant"))?.textContent.trim() })()`)) === "Header & footer…", null);
await ev(`document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))`);
await sleep(200);
check("escape closes the menu", (await ev(`!!document.querySelector(".tools-menu")`)) === false, null);
// Tab closes the menu and moves focus on from the Tools button
await ev(`document.querySelector('button[aria-label="Tools"]').click()`);
await until(`document.querySelector(".tools-menu")`, "the menu for Tab");
await ev(`document.querySelector(".tools-filter").dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true }))`);
await sleep(200);
const afterTab = await ev(`({ open: !!document.querySelector(".tools-menu"), onTools: document.activeElement?.getAttribute("aria-label") })`);
check("Tab closes the menu (focus goes back to Tools)", !afterTab.open && afterTab.onTools === "Tools", afterTab);
// moving focus elsewhere closes it too
await ev(`document.querySelector('button[aria-label="Tools"]').click()`);
await until(`document.querySelector(".tools-menu")`, "the menu for focus-out");
await ev(`document.querySelector("#page-input").focus()`);
await sleep(200);
check("focus leaving the menu closes it", (await ev(`!!document.querySelector(".tools-menu")`)) === false, null);
// an item opens its sheet
await ev(`document.querySelector('button[aria-label="Tools"]').click()`);
await until(`document.querySelector(".tools-menu")`, "the menu again");
await ev(`[...document.querySelectorAll('.tools-menu [role="option"]')].find((b) => /Watermark/.test(b.textContent)).click()`);
await sleep(700);
check("an item runs and closes the menu", (await ev(`!!document.querySelector(".tools-menu")`)) === false, null);
const sheetOpen = await ev(`window.__inkwell.useMarks.getState().kind ?? window.__inkwell.useMarks.getState().sheet ?? null`);
report.sheetAfterItem = sheetOpen == null ? null : "open";
await ev(`window.__inkwell.useMarks.getState().close()`);
await sleep(300);
await ev(`${S}.getState().setTheme("dark")`);
await sleep(400);
await clip("tools-button-dark.png", ".toolbar");
await ev(`document.querySelector('button[aria-label="Tools"]').click()`);
await until(`document.querySelector(".tools-menu")`, "the menu in the dark app");
await shot("tools-menu-dark.png", 300);
await ev(`document.querySelector(".tools-menu").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))`);
await ev(`document.querySelector(".tools-filter")?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))`);
await sleep(200);
await ev(`window.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }))`);
await sleep(200);
await setTheme("light");
await sleep(300);

// ---- follow the app theme -----------------------------------------------------------------------
await setTheme("dark");
await sleep(500);
check("follow + dark app: pages dark", (await pagesAttr()) === "dark", await pagesAttr());
check("setting is still follow", (await ev(`${S}.getState().pageDark`)) === "follow", null);
const followDark = await toggleLabel();
check("toolbar says Normal pages under follow in the dark app", followDark?.label === "Normal pages", followDark);
const darkApp = await shot("pages-dark-dark-app.png", 700);
// the viewer's canvas is the strip between the sidebar and the page; the page must sit just above it
const pb2 = dev(await rectOf(".page"));
const canvasPx = mean(darkApp, { x: pb2.x - 12, y: pb2.y + 300, w: 8, h: 8 });
const pagePx = mean(darkApp, { x: pb2.x + 6, y: pb2.y + 6, w: 24, h: 24 });
report.darkGreys = { canvasPx, pagePx };
check("the page sits just above the app's canvas", pagePx[0] > canvasPx[0] && pagePx[0] - canvasPx[0] < 40, report.darkGreys);
await clickToggle();
await sleep(500);
const toNormal = { attr: await pagesAttr(), setting: await ev(`${S}.getState().pageDark`) };
check("toggle in the dark app gives normal pages", toNormal.attr === null && toNormal.setting === "off", toNormal);
await shot("pages-normal-dark-app.png", 700);

// ---- a picture of flat colours comes back exactly -------------------------------------------------------
// rgb-photo.pdf (scripts/make-rgb-photo.mjs) is one picture of 3 x 2 blocks: red green blue / cyan magenta yellow.
const rgb = docs.find((d) => /rgb-photo/.test(d.name));
if (rgb) {
  const WANT = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [0, 255, 255], [255, 0, 255], [255, 255, 0]];
  const tilesReady = `[...document.querySelectorAll(".page canvas.tile:not(.preview)")].length > 0 && [...document.querySelectorAll(".page canvas.tile:not(.preview)")].every((c) => c.dataset.ready)`;
  const blocks = (png, box, cols, rows, index) => {
    const got = [];
    for (let r = 0; r < rows; r++)
      for (let c = 0; c < cols; c++) {
        const cx = box.x + (box.w * (c + 0.5)) / cols;
        const cy = box.y + (box.h * (r + 0.5)) / rows;
        got.push({ at: [c, r], rgb: mean(png, { x: cx - 6, y: cy - 6, w: 12, h: 12 }), want: WANT[index(c, r)] });
      }
    return got;
  };
  await setTheme("light");
  await ev(`${S}.getState().activate(${rgb.id})`);
  await ev(`${S}.getState().setZoomMode(${rgb.id}, "fitWidth")`);
  await setPages("off");
  await until(tilesReady, "the picture's sharp tiles", 200);
  const plainShot = await shot("_rgb-normal.png", 700);
  await setPages("on");
  await until(`document.querySelector(".nat-img")`, "the picture box", 100);
  const darkShot = await shot("photo-natural-dark-pages.png", 900);
  fs.rmSync(path.join(out, "_rgb-normal.png"), { force: true });
  const rbox = dev(await rectOf(".nat-img"));
  const idx = (c, r) => 3 * r + c;
  const plainBlocks = blocks(plainShot, rbox, 3, 2, idx);
  const darkBlocks = blocks(darkShot, rbox, 3, 2, idx);
  report.rgbPhoto = { plain: plainBlocks.map((b) => b.rgb), dark: darkBlocks.map((b) => b.rgb) };
  check("pure red, green, blue, cyan, magenta, yellow render exactly with normal pages", plainBlocks.every((b) => dist(b.rgb, b.want) <= 3), plainBlocks);
  check("the same colours on dark pages, within 3 levels of normal", darkBlocks.every((b, i) => dist(b.rgb, plainBlocks[i].rgb) <= 3), { plainBlocks, darkBlocks });
  // a page turned a quarter: the picture box must follow the turn (3 x 2 blocks become 2 x 3)
  await ev(`${S}.getState().rotate(${rgb.id}, 1)`);
  await sleep(1200);
  await until(tilesReady, "tiles after the turn", 200);
  await sleep(700);
  const turned = await shot("_rgb-turned.png", 300);
  fs.rmSync(path.join(out, "_rgb-turned.png"), { force: true });
  const tbox = dev(await rectOf(".nat-img"));
  const turnedBlocks = blocks(turned, tbox, 2, 3, (c, r) => 3 * (1 - c) + r);
  report.rgbTurned = turnedBlocks.map((b) => b.rgb);
  check("a turned page keeps the picture's colours in place", turnedBlocks.every((b) => dist(b.rgb, b.want) <= 3), turnedBlocks);
  await ev(`${S}.getState().rotate(${rgb.id}, -1)`);
  await sleep(600);
  await setPages("follow");
  await ev(`${S}.getState().activate(${sample.id})`);
  await sleep(400);
}

// ---- the page-number box is sized to the page count ---------------------------------------------------------
// pages-1 / pages-91 / pages-1500.pdf (scripts/make-blank-pages.mjs): "1 / 1", "12 / 91", "1234 / 1500"
const counts = [[1, 1], [91, 12], [1500, 1234]];
const boxes = [];
for (const [n, at] of counts) {
  const d = docs.find((x) => new RegExp(`pages-${n}\\b`).test(x.name));
  if (!d) continue;
  await ev(`${S}.getState().activate(${d.id})`);
  await ev(`${S}.getState().goTo(${d.id}, ${at - 1})`);
  await until(`document.querySelector("#page-input")?.value === "${at}"`, `the box to show ${at}`, 100);
  for (const theme of ["light", "dark"]) {
    await setTheme(theme);
    await sleep(350);
    const box = await ev(`(() => { const i = document.querySelector("#page-input"); const r = i.getBoundingClientRect(); const cs = getComputedStyle(i); const fit = i.scrollWidth <= i.clientWidth; const n = i.nextElementSibling.getBoundingClientRect(); return { w: r.width, text: i.value, after: i.nextElementSibling.textContent, gap: n.left - r.right, tab: cs.fontVariantNumeric, align: cs.textAlign, fit } })()`);
    if (theme === "light") boxes.push({ n, at, ...box });
    await clip(`pagebox-${at}-of-${n}-${theme}.png`, ".toolbar");
  }
}
report.pageBox = boxes;
if (boxes.length === 3) {
  check("the box is compact: '1 / 1' is narrower than 44px", boxes[0].w < 44, boxes[0]);
  check("it grows with the page count", boxes[0].w <= boxes[1].w && boxes[1].w < boxes[2].w, boxes.map((b) => b.w));
  check("the number fits its box and the total sits close by", boxes.every((b) => b.fit && b.gap <= 8 && /tabular-nums/.test(b.tab) && b.align === "center"), boxes);
}
await setTheme("light");
await ev(`${S}.getState().activate(${sample.id})`);
await sleep(300);

// ---- persistence ---------------------------------------------------------------------------------
await setPages("on");
await setTheme("light");
await sleep(200);
if (vitePort) await send("Page.reload", { ignoreCache: false });
else await send("Page.reload");
await sleep(2000);
await until(`window.__inkwell && ${S}.getState().order.length >= 2`, "the documents after reload", 300);
const kept = { setting: await ev(`${S}.getState().pageDark`), attr: await pagesAttr(), stored: await stored() };
report.afterReload = kept;
check("the setting survives a reload", kept.setting === "on" && kept.attr === "dark", kept);
// the documents are opened again after a reload: new ids
const docs2 = await ev(`${S}.getState().order.map((id) => ({ id, name: ${S}.getState().docs[id].name }))`);
sample.id = docs2.find((d) => /dark-sample/.test(d.name)).id;
if (form) form.id = docs2.find((d) => /form-source/.test(d.name)).id;
await ev(`${S}.getState().activate(${sample.id})`);
await ev(`${S}.getState().setZoomMode(${sample.id}, "fitWidth")`);
await setPages("follow");
await setTheme("dark");
await sleep(600);

// ---- readable overlays on dark pages ----------------------------------------------------------------
await until(`document.querySelectorAll(".page canvas.tile:not(.preview)").length > 0`, "sharp tiles again", 200);
await ev(`${S}.getState().setUi({ searchOpen: true })`);
await sleep(300);
await ev(`${S}.getState().runSearch(${sample.id}, "docs")`);
await until(`${S}.getState().docs[${sample.id}].search.hits.length > 0`, "a search hit");
await shot("search-dark-pages.png", 600);
await ev(`${S}.getState().setUi({ searchOpen: false })`);
await ev(`${S}.getState().runSearch(${sample.id}, "")`);

const hasText = await ev(`document.querySelectorAll(".textlayer > div").length`);
report.textLines = hasText;
if (hasText > 0) {
  await ev(`(() => { const d = [...document.querySelectorAll(".textlayer > div")].find((e) => /Read the docs/.test(e.textContent)); if (!d) return false; const r = document.createRange(); r.selectNodeContents(d); const s = getSelection(); s.removeAllRanges(); s.addRange(r); return true })()`);
  await shot("selection-dark-pages.png", 500);
  await ev(`getSelection().removeAllRanges()`);
}

// a comment: a rectangle and a note, drawn by the engine / the comment layer
const pg = await ev(`(() => { const d = ${S}.getState().docs[${sample.id}]; return { n: d.pageCount } })()`);
const a1 = await ev(`window.__inkwell.useAnnot.getState().add(${sample.id}, 0, { type: "shape", shape: "rectangle", points: [[60, 50], [250, 120]] })`);
const a2 = await ev(`window.__inkwell.useAnnot.getState().add(${sample.id}, 0, { type: "note", at: [300, 70], text: "Readable on a dark page" })`);
report.annots = { a1, a2, pages: pg.n };
await sleep(900);
await ev(`(() => { const v = document.querySelector(".viewer"); if (v) v.scrollTop = 0; })()`);
await sleep(600);
await shot("comments-dark-pages.png", 600);
await ev(`${S}.getState().setTheme("light")`);
await shot("comments-dark-pages-light-app.png", 600);
await ev(`${S}.getState().setTheme("dark")`);

// a form field in focus
if (form) {
  await ev(`${S}.getState().activate(${form.id})`);
  await until(`document.querySelectorAll(".page canvas.tile:not(.preview)").length > 0`, "form page tiles", 200);
  await sleep(900);
  // the "same document?" question over the page is the history panel's, not this feature's: answer it
  await ev(`[...document.querySelectorAll("button")].find((b) => /No, it's new/.test(b.textContent))?.click()`);
  await sleep(500);
  const focused = await ev(`(() => { const f = document.querySelector(".page input, .page textarea, .page select"); if (!f) return false; f.focus(); return true })()`);
  report.formFieldFocused = focused;
  await shot("form-dark-pages.png", 600);
}

// thumbnails and the Organize grid
await ev(`${S}.getState().activate(${sample.id})`);
await ev(`${S}.getState().setSidebar("thumbs")`);
await sleep(900);
await shot("thumbnails-dark-pages.png", 500);
await ev(`${S}.getState().patch(${sample.id}, { organize: true })`);
await until(`document.querySelector(".org-page canvas.tile")`, "the organize grid", 100);
await shot("organize-dark-pages.png", 700);
await ev(`${S}.getState().patch(${sample.id}, { organize: false })`);

// a light-app version of the same dark page, for the docs
await setTheme("light");
await setPages("on");
await shot("pages-dark-light-app-final.png", 700);
await setPages("follow");
await send("Emulation.setFocusEmulationEnabled", { enabled: false });
console.log(JSON.stringify(report, null, 2));
ws.close();
