// Drives a running Inkwell through WebView2's DevTools endpoint and exercises bookmarks and links in Edit mode
// on a COPY of a study sample: adds, renames, reorders (drag), styles and re-points a bookmark, draws a link,
// undoes and redoes, saves, reopens and reads everything back. Mouse and keys are CDP Input events to the page
// (no global input); the native "Open link" dialog is answered at the page's fetch (Cancel, always).
//   $d = "$env:TEMP\inkwell-bm"; mkdir $d; copy samples\bookmarks-links-source.pdf "$d\bm-copy.pdf"
//   node node_modules\vite\bin\vite.js --port 1420 --strictPort        (the debug exe loads the dev server)
//   $env:INKWELL_DATA_DIR = "$d\data"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9229'
//   src-tauri\target\debug\inkwell.exe "$d\bm-copy.pdf"
//   node scripts/bookmarks-shots.mjs docs/screens/bookmarks 9229
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/bookmarks";
const port = process.argv[3] ?? "9229";
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
const T = "window.__inkwell.useTextEdit";
const N = "window.__inkwell.useNav";
const report = { checks: {} };
const check = (name, ok, extra) => {
  report.checks[name] = ok ? "ok" : `FAILED ${extra ?? ""}`;
  console.log(ok ? "ok    " : "FAILED", name, ok ? "" : (extra ?? ""));
  if (!ok) process.exitCode = 1;
};

const shot = async (name, wait = 350) => {
  await sleep(wait);
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  console.log("screenshot", name);
};
const theme = (t) => ev(`${S}.getState().setTheme("${t}")`);
async function both(name) {
  await theme("light");
  await shot(`${name}-light.png`);
  await theme("dark");
  await shot(`${name}-dark.png`);
  await theme("light");
}

// ---- input (CDP Input domain: delivered to the page, never the OS) ----
const mouse = (type, x, y, extra = {}) => send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1, ...extra });
async function click(x, y, extra = {}) {
  await mouse("mouseMoved", x, y, { button: "none", clickCount: 0 });
  await mouse("mousePressed", x, y, extra);
  await mouse("mouseReleased", x, y, extra);
  await sleep(150);
}
async function key(k, code, vk, modifiers = 0) {
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: k, code, windowsVirtualKeyCode: vk, modifiers });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, windowsVirtualKeyCode: vk, modifiers });
  await sleep(150);
}
const CTRL = 2;
const centre = (sel) =>
  ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null; const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height, top: r.top, left: r.left }; })()`);
const clickSel = async (sel) => {
  const c = await centre(sel);
  if (!c) throw new Error(`no element for ${sel}`);
  await click(c.x, c.y);
};
/** The centre of the first element matching `sel` whose text is `text`. */
const centreText = (sel, text) =>
  ev(`(() => { const e = [...document.querySelectorAll(${JSON.stringify(sel)})].find((x) => x.textContent.trim() === ${JSON.stringify(text)} || x.getAttribute("aria-label") === ${JSON.stringify(text)}); if (!e) return null; const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height, top: r.top, left: r.left }; })()`);
const clickText = async (sel, text) => {
  const c = await centreText(sel, text);
  if (!c) throw new Error(`no ${sel} with text ${text}`);
  await click(c.x, c.y);
};
async function typeOver(text) {
  await ev(`document.activeElement.select()`);
  await send("Input.insertText", { text });
  await sleep(150);
}
async function settle(pred, what, tries = 80) {
  for (let i = 0; i < tries; i++) {
    if (await pred()) return;
    await sleep(100);
  }
  throw new Error(`${what}: did not settle`);
}
const doc = () => ev(`(() => { const s = ${S}.getState(); const d = s.docs[s.active]; return { id: d.id, name: d.name, path: d.path, dirty: d.dirty, rev: d.rev, undo: d.undoLabel, redo: d.redoLabel, sizes: d.sizes, pages: d.pageCount }; })()`);
const nav = () => ev(`(() => { const s = ${S}.getState(); return ${N}.getState().docs[s.active] ?? null; })()`);
const flat = (items, out = []) => {
  for (const b of items ?? []) {
    out.push(b);
    flat(b.children, out);
  }
  return out;
};
const tree = async () => flat((await nav())?.bookmarks?.items ?? []);
const titles = async () => ((await nav())?.bookmarks?.items ?? []).map((b) => b.title);
const linksOf = async (page) => ((await nav())?.links?.[page]?.links ?? []);
const rows = () => ev(`[...document.querySelectorAll(".te-dock .bm-row .bm-title")].map((e) => e.textContent)`);
const sideRows = () => ev(`[...document.querySelectorAll(".sidebar .bm-row .bm-title")].map((e) => e.textContent)`);

await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await send("Input.setInterceptDrags", { enabled: true });
await theme("light");
await sleep(900);
const d0 = await doc();
report.file = d0.name;
// Dialogs the app would show (the open-link confirm) are answered "Cancel" at the page's fetch.
await ev(`(() => {
  window.__dlg = [];
  const real = window.fetch.bind(window);
  window.fetch = (url, opts) => {
    if (!decodeURIComponent(String(url)).includes("plugin:dialog|message")) return real(url, opts);
    const b = opts && opts.body;
    const text = typeof b === "string" ? b : b ? new TextDecoder().decode(b) : "";
    window.__dlg.push({ message: text });
    return Promise.resolve(new Response(JSON.stringify("Cancel"), { headers: { "content-type": "application/json" } }));
  };
})()`);

// ---- Edit on, Bookmarks tab ----
await key("e", "KeyE", 69, CTRL);
await settle(async () => (await ev(`${T}.getState().open`)) === true, "Edit open");
check("the Edit rail has Text, Links and Bookmarks tabs", (await ev(`[...document.querySelectorAll(".te-tabs button")].map((b) => b.textContent).join()`)) === "Text,Links,Bookmarks");
await clickText(".te-tabs button", "Bookmarks");
await settle(async () => ((await nav())?.bookmarks?.items?.length ?? 0) > 0, "bookmarks listed");
const start = await tree();
report.startBookmarks = start.map((b) => b.title);
check("the outline of the sample is listed as a tree", start.length === 3 && start[0].children.length === 1, JSON.stringify(report.startBookmarks));
check("rows match the open groups", (await rows()).length === 3, JSON.stringify(await rows()));
await both("bookmarks-tree");

// ---- add (named in place) ----
await clickText(".bm-bar .te-btn", "New");
await settle(async () => (await centre(".bm-edit")) != null, "name field");
await typeOver("Appendix");
await key("Enter", "Enter", 13);
await settle(async () => (await tree()).some((b) => b.title === "Appendix"), "bookmark added");
let d = await doc();
check("New adds a bookmark, named in place, as one undo step", /bookmark/i.test(d.undo ?? ""), d.undo);
check("it was added at the top level, last", (await titles()).join() === "Chapter 1: Introduction,Appendix", (await titles()).join());
report.addLabel = d.undo;

// ---- reorder by drag: Appendix above Chapter 1 ----
async function drag(fromText, toText, zone) {
  const a = await centreText(".te-dock .bm-title", fromText);
  const b = await centreText(".te-dock .bm-title", toText);
  if (!a || !b) throw new Error("rows not found");
  const row = await ev(`(() => { const e = [...document.querySelectorAll(".te-dock .bm-row")].find((r) => r.querySelector(".bm-title")?.textContent === ${JSON.stringify(toText)}); const r = e.getBoundingClientRect(); return { top: r.top, h: r.height }; })()`);
  const y = zone === "before" ? row.top + row.h * 0.1 : zone === "after" ? row.top + row.h * 0.9 : row.top + row.h * 0.5;
  events.length = 0;
  await mouse("mouseMoved", a.x, a.y, { button: "none", clickCount: 0 });
  await mouse("mousePressed", a.x, a.y);
  await mouse("mouseMoved", a.x, a.y + 4, { buttons: 1 });
  await sleep(150);
  const ic = events.find((e) => e.method === "Input.dragIntercepted");
  if (!ic) {
    await mouse("mouseReleased", a.x, a.y);
    return false;
  }
  const data = ic.params.data;
  await send("Input.dispatchDragEvent", { type: "dragEnter", x: b.x, y, data });
  await send("Input.dispatchDragEvent", { type: "dragOver", x: b.x, y, data });
  await send("Input.dispatchDragEvent", { type: "drop", x: b.x, y, data });
  await sleep(250);
  return true;
}
const dragged = await drag("Appendix", "Chapter 1: Introduction", "before");
check("the bookmark row can be dragged (real drag events)", dragged);
if (!dragged) await ev(`${N}.getState().moveBookmark(${d.id}, ${(await tree()).find((b) => b.title === "Appendix").id}, null, 0)`);
await settle(async () => (await titles())[0] === "Appendix", "reordered");
check("dropping above Chapter 1 reorders the top level", (await titles()).join() === "Appendix,Chapter 1: Introduction", (await titles()).join());
d = await doc();
check("the move is one named undo step", /move/i.test(d.undo ?? ""), d.undo);

// ---- nest by drag: Appendix inside Chapter 1 ----
const nested = await drag("Appendix", "Chapter 1: Introduction", "into");
check("dragging onto the middle of a row nests it", nested);
if (!nested) await ev(`${N}.getState().moveBookmark(${d.id}, ${(await tree()).find((b) => b.title === "Appendix").id}, ${(await tree()).find((b) => b.title === "Chapter 1: Introduction").id}, 0)`);
await settle(async () => (await nav()).bookmarks.items.length === 1, "nested");
const ch1 = (await nav()).bookmarks.items[0];
check("Appendix is now a child of Chapter 1", ch1.children.some((c) => c.title === "Appendix"), JSON.stringify(ch1.children.map((c) => c.title)));
// undo twice: nest, then reorder; redo once
await ev(`document.activeElement?.blur?.()`);
await key("z", "KeyZ", 90, CTRL);
await settle(async () => (await nav()).bookmarks.items.length === 2, "undo of nest");
await key("z", "KeyZ", 90, CTRL);
await settle(async () => (await titles()).join() === "Chapter 1: Introduction,Appendix", "undo of reorder");
check("undo steps back through the moves", true);
await key("y", "KeyY", 89, CTRL);
await settle(async () => (await titles()).join() === "Appendix,Chapter 1: Introduction", "redo of reorder");
check("redo brings the order back", true);
await both("bookmarks-reordered");

// ---- rename in place (double click), set destination, style ----
const rowAp = await centreText(".te-dock .bm-title", "Appendix");
await click(rowAp.x, rowAp.y);
await settle(async () => (await nav()).selectedBm != null, "selected");
const rowAp2 = await centreText(".te-dock .bm-title", "Appendix");
await mouse("mousePressed", rowAp2.x, rowAp2.y, { clickCount: 2 });
await mouse("mouseReleased", rowAp2.x, rowAp2.y, { clickCount: 2 });
await settle(async () => (await centre(".bm-edit")) != null, "rename field");
await typeOver("Annexes");
await key("Enter", "Enter", 13);
await settle(async () => (await tree()).some((b) => b.title === "Annexes"), "renamed");
check("double-click renames in place", true);
await ev(`${S}.getState().goTo(${d.id}, 2)`);
await sleep(500);
await clickText(".te-dock .te-btn", "Set to current view");
try {
  await settle(async () => (await tree()).find((b) => b.title === "Annexes")?.dest?.page === 2, "destination set");
} catch (e) {
  const n = await nav();
  console.log("DIAG", JSON.stringify((await tree()).find((b) => b.title === "Annexes")), "selected", n.selectedBm, "notice", n.notice, "summary", await ev(`document.querySelector(".te-dock .te-summary")?.textContent`), "cur", JSON.stringify(await doc()));
  throw e;
}
check("Set to current view points the bookmark at the page in view", true);
await clickSel(".te-dock .te-tog[aria-label='Bold']");
await settle(async () => (await tree()).find((b) => b.title === "Annexes")?.bold === true, "bold");
await clickSel(".te-dock .te-sw[aria-label='Colour #2e4b8f']");
await settle(async () => (await tree()).find((b) => b.title === "Annexes")?.color != null, "colour");
check("bold and colour are saved on the item", true);
await both("bookmarks-selected");

// ---- collapse and expand: saved state, one undo step ----
const before = await doc();
const tw = await ev(`(() => { const r = [...document.querySelectorAll(".te-dock .bm-row")].find((x) => x.querySelector(".bm-title")?.textContent === "Chapter 1: Introduction"); const e = r?.querySelector(".twisty"); if (!e) return null; const b = e.getBoundingClientRect(); return { x: b.left + b.width / 2, y: b.top + b.height / 2 }; })()`);
await click(tw.x, tw.y);
try {
  await settle(async () => (await tree()).find((b) => b.title === "Chapter 1: Introduction")?.open === false, "collapsed");
} catch (e) {
  const c = await centre(".te-dock .bm-row .twisty");
  console.log("DIAG", JSON.stringify((await tree()).map((b) => [b.title, b.open, b.children.length])), JSON.stringify(c), await ev(`(() => { const c = ${JSON.stringify(c)}; const e = document.elementFromPoint(c.x, c.y); return e ? e.tagName + "." + e.className + " | " + (e.closest(".bm-row")?.textContent ?? "") : null; })()`), JSON.stringify(await doc()));
  throw e;
}
check("the twisty collapses a group and the state is part of the file", (await doc()).rev > before.rev);
await key("z", "KeyZ", 90, CTRL);
await settle(async () => (await tree()).find((b) => b.title === "Chapter 1: Introduction")?.open === true, "expanded again by undo");

// ---- the left sidebar outline is the same editor while Edit is open ----
await ev(`${S}.getState().setSidebar("outline")`);
await settle(async () => (await sideRows()).length > 0, "sidebar rows");
check("the sidebar outline shows the same bookmarks", JSON.stringify(await sideRows()) === JSON.stringify(await rows()), `${JSON.stringify(await sideRows())} vs ${JSON.stringify(await rows())}`);
const sr = await centreText(".sidebar .bm-title", "Annexes");
await click(sr.x, sr.y, { button: "right", buttons: 2 });
await settle(async () => (await centre(".ink-menu")) != null, "context menu");
const menu = await ev(`[...document.querySelectorAll(".ink-menu button")].map((b) => b.textContent.replace(/\\s+/g, " ").trim())`);
check("right-click offers Add, Delete and Set destination", menu.some((t) => /^Add bookmark/.test(t)) && menu.some((t) => /Delete/.test(t)) && menu.some((t) => /Set destination/.test(t)), JSON.stringify(menu));
await both("bookmarks-sidebar");
await key("Escape", "Escape", 27);
await sleep(200);
await ev(`${S}.getState().setSidebar("none")`);

// ---- Links tab: draw a link on page 1 ----
await ev(`${S}.getState().goTo(${d.id}, 0)`);
await sleep(600);
await clickText(".te-tabs button", "Links");
await settle(async () => (await centre(".lk")) != null, "links panel");
await settle(async () => (await linksOf(0)).length > 0, "page 1 links listed");
const linksBefore = (await linksOf(0)).length;
report.linksBefore = linksBefore;
check("the existing links of page 1 are listed", linksBefore === 3, String(linksBefore));
await clickText(".lk .te-btn", "Draw a link");
await settle(async () => (await centre(".lk-capture")) != null, "draw layer");
const box = await ev(`(() => { const r = document.querySelector(".page[data-page='0']").getBoundingClientRect(); return { left: r.left, top: r.top, w: r.width }; })()`);
const zoom = box.w / d0.sizes[0][0];
const at = (x, y) => [box.left + x * zoom, box.top + y * zoom];
const [ax, ay] = at(72, 420);
const [bx, by] = at(260, 452);
await mouse("mouseMoved", ax, ay, { button: "none", clickCount: 0 });
await mouse("mousePressed", ax, ay);
await mouse("mouseMoved", (ax + bx) / 2, (ay + by) / 2, { buttons: 1 });
await mouse("mouseMoved", bx, by, { buttons: 1 });
await mouse("mouseReleased", bx, by);
await settle(async () => (await nav()).pending != null, "pending rectangle");
check("dragging draws a rectangle and asks where it goes", (await centre(".lk .te-format")) != null);
await clickText(".lk .te-seg button", "A web address");
await clickSel(".lk .te-text");
await send("Input.insertText", { text: "javascript:alert(1)" });
await sleep(500);
check("an address the gate refuses can't be linked", (await ev(`document.querySelector(".lk .te-btn.primary").disabled`)) === true && (await centre(".lk-bad")) != null);
await typeOver("https://example.com/inkwell");
await settle(async () => (await centre(".lk-ok")) != null, "address accepted");
await shot("_pending.png");
await both("links-new");
await clickText(".lk .te-btn.primary", "Create link");
await settle(async () => (await linksOf(0)).length === linksBefore + 1, "link created");
d = await doc();
check("Create link is one named undo step", /link/i.test(d.undo ?? ""), d.undo);
const made = (await linksOf(0)).find((k) => k.target.kind === "uri" && k.target.uri.includes("inkwell"));
check("the new link goes to the address", !!made, JSON.stringify(await linksOf(0)));
check("it is selected, with its state shown as a web address", (await centre(".lk .st-dot.external")) != null);

// ---- border, then Open asks first ----
await clickText(".lk .te-check", "Visible");
await settle(async () => (await linksOf(0)).find((k) => k.id === made.id)?.border.visible === true, "border visible");
check("a visible border is one more step", true);
await clickText(".lk .te-btn", "Open address…");
await settle(async () => (await ev(`window.__dlg.length`)) > 0, "open confirm");
const asked = await ev(`window.__dlg[0].message`);
check("opening an address asks first, showing the address", /example\.com\/inkwell/.test(asked), asked);
await both("links-selected");

// ---- undo and redo ----
await ev(`document.activeElement?.blur?.()`);
await key("z", "KeyZ", 90, CTRL);
await key("z", "KeyZ", 90, CTRL);
await settle(async () => (await linksOf(0)).length === linksBefore, "undo of link");
check("undo steps back through the border, then the link", true);
await key("y", "KeyY", 89, CTRL);
await key("y", "KeyY", 89, CTRL);
await settle(async () => (await linksOf(0)).find((k) => k.id === made.id)?.border.visible === true, "redo of link");
check("redo brings them back", true);

// ---- problems: the audit counts every link ----
await clickText(".lk .te-btn", "Show problems");
await settle(async () => (await nav()).audit != null, "audit");
const audit = (await nav()).audit;
report.audit = { total: audit.total, external: audit.external, broken: audit.broken };
check("Show problems counts the document's links", audit.total >= linksBefore + 1 && audit.external >= 2, JSON.stringify(report.audit));
await clickText(".lk .te-btn", "Show problems");

// ---- save, reopen, read back ----
const saved = await doc();
await key("s", "KeyS", 83, CTRL);
await settle(async () => !(await doc()).dirty, "saved");
check("Save clears the dirty mark", true);
const p = saved.path;
await ev(`(async () => { const s = ${S}.getState(); s.closeDoc(s.active); await new Promise((r) => setTimeout(r, 300)); await ${S}.getState().openPath(${JSON.stringify(p)}); })()`);
await settle(async () => (await ev(`${S}.getState().active != null`)) === true, "reopened");
await sleep(800);
await settle(async () => ((await nav())?.links?.[0]?.links?.length ?? 0) > 0, "links after reopen", 100);
check("the link survives saving and reopening", (await linksOf(0)).some((k) => k.target.kind === "uri" && k.target.uri === "https://example.com/inkwell"));
await clickText(".te-tabs button", "Bookmarks");
await settle(async () => ((await nav())?.bookmarks?.items?.length ?? 0) > 0, "bookmarks after reopen", 100);
const reopened = await tree();
const ann = reopened.find((b) => b.title === "Annexes");
check("the bookmarks survive: renamed, re-pointed, bold, nested", !!ann && ann.dest.page === 2 && ann.bold === true, JSON.stringify(reopened.map((b) => [b.title, b.dest, b.bold])));
await both("bookmarks-reopened");

await key("e", "KeyE", 69, CTRL);
await send("Input.setInterceptDrags", { enabled: false });
await send("Emulation.setFocusEmulationEnabled", { enabled: false });
fs.rmSync(path.join(out, "_pending.png"), { force: true });
console.log(JSON.stringify(report, null, 2));
ws.close();
