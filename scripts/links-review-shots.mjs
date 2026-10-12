// A second pass over bookmarks and links in the running app, for the review fixes: Read-mode links (page jump,
// web address asked first, hover titles, a Launch link that does nothing), bookmark add / move / delete (the
// deleted title is not left in the saved file), file links (a file one folder up is linked with ../ and asked
// first; a file on another drive and a program are refused) and a file link naming a path outside the folder.
// Every dialog is answered at the page's fetch; the file picker is answered with tokens of the command line.
//   run-links-review.ps1 (in the job notes) makes the folder and starts the app with these arguments:
//     <d>\bm\bm-copy.pdf <d>\bm\hostile.pdf <d>\other\sibling.pdf <E: pdf> <d>\bm\run.bat
//   node scripts/links-review-shots.mjs docs/screens/bookmarks 9229
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
  await sleep(200);
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
const docs = () => ev(`(() => { const s = ${S}.getState(); return Object.values(s.docs).map((d) => ({ id: d.id, name: d.name, path: d.path })); })()`);
const doc = () => ev(`(() => { const s = ${S}.getState(); const d = s.docs[s.active]; return { id: d.id, name: d.name, path: d.path, dirty: d.dirty, rev: d.rev, undo: d.undoLabel, redo: d.redoLabel, sizes: d.sizes, pages: d.pageCount, page: d.currentPage }; })()`);
const nav = () => ev(`(() => { const s = ${S}.getState(); return ${N}.getState().docs[s.active] ?? null; })()`);
const flat = (items, o = []) => {
  for (const b of items ?? []) {
    o.push(b);
    flat(b.children, o);
  }
  return o;
};
const tree = async () => flat((await nav())?.bookmarks?.items ?? []);
const titles = async () => ((await nav())?.bookmarks?.items ?? []).map((b) => b.title);
const linksOf = async (page) => ((await nav())?.links?.[page]?.links ?? []);
const rows = () => ev(`[...document.querySelectorAll(".te-dock .bm-row .bm-title")].map((e) => e.textContent)`);
const dialogs = () => ev(`window.__dlg.map((d) => d.message)`);
const ipc = () => ev(`window.__ipc.slice()`);
const err = () => ev(`${S}.getState().error`);
const clearErr = () => ev(`${S}.getState().setUi({ error: null })`);
const activate = async (name) => {
  const d = (await docs()).find((x) => x.name === name);
  if (!d) throw new Error(`no tab ${name}: ${JSON.stringify(await docs())}`);
  await ev(`${S}.getState().activate(${d.id})`);
  await sleep(500);
  return d;
};
const rlLinks = () =>
  ev(`[...document.querySelectorAll(".page[data-page='0'] .rl-link")].map((e) => { const r = e.getBoundingClientRect(); return { title: e.title, idle: e.classList.contains("idle"), x: r.left + r.width / 2, y: r.top + r.height / 2 }; })`);
const hasBytes = (file, text) => {
  const b = fs.readFileSync(file);
  const hex = [...text].map((c) => c.charCodeAt(0).toString(16).padStart(4, "0")).join("");
  return b.includes(Buffer.from(text, "latin1")) || b.toString("latin1").toLowerCase().includes(hex);
};

await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await send("Input.setInterceptDrags", { enabled: true });
await theme("light");
await sleep(900);

// Dialogs the app would show are recorded and answered at the page's fetch ("Cancel" until `__answer` says
// otherwise); the file picker answers with the next token set in `__pick`; every IPC url is recorded.
await ev(`(() => {
  window.__dlg = [];
  window.__ipc = [];
  window.__answer = "Cancel";
  window.__pick = null;
  const real = window.fetch.bind(window);
  const json = (v) => new Response(JSON.stringify(v), { headers: { "content-type": "application/json", "Tauri-Response": "ok" } });
  window.fetch = (url, opts) => {
    const u = decodeURIComponent(String(url));
    window.__ipc.push(u);
    if (u.includes("plugin:dialog|message")) {
      const b = opts && opts.body;
      const text = typeof b === "string" ? b : b ? new TextDecoder().decode(b) : "";
      let m = text;
      try { m = JSON.parse(text).message ?? text; } catch {}
      window.__dlg.push({ message: m });
      return Promise.resolve(json(window.__answer));
    }
    if (u.includes("link_pick_file")) return Promise.resolve(json(window.__pick));
    return real(url, opts);
  };
})()`);

// Tokens of the command-line files (each call issues new ones), by file name.
const given = await ev(`window.__inkwell.api.initialFiles().then((l) => l.map((f) => ({ path: f.path, token: f.token })))`);
const tok = (name) => given.find((f) => f.path.toLowerCase().endsWith(name.toLowerCase()));
report.args = given.map((g) => g.path);
check("the command line gave tokens for the files", given.length >= 4, JSON.stringify(report.args));
await clearErr();
await settle(async () => (await docs()).some((d) => d.name === "bm-copy.pdf"), "tabs open");
await sleep(600);

// ---- Read mode: links of a page are followable ----
const main = await activate("bm-copy.pdf");
await ev(`${S}.getState().goTo(${main.id}, 0)`);
await settle(async () => (await rlLinks()).length >= 3, "read-mode links", 150);
let rl = await rlLinks();
report.readLinks = rl.map((l) => l.title);
check("Read mode lays a hit area over every link of the page", rl.length === 3, JSON.stringify(report.readLinks));
check("each shows where it goes when hovered", rl.every((l) => l.title && l.title.length > 3), JSON.stringify(report.readLinks));
const uriLink = rl.find((l) => /example\.com/.test(l.title));
const pageLink = rl.find((l) => /page/i.test(l.title) && !/example\.com/.test(l.title));
check("a web link and a page link are told apart", !!uriLink && !!pageLink, JSON.stringify(report.readLinks));
await both("read-links");
const dlgBefore = (await dialogs()).length;
await click(uriLink.x, uriLink.y);
await settle(async () => (await dialogs()).length > dlgBefore, "web link asks");
const asked = (await dialogs()).at(-1);
check("a web link asks first and shows the whole address", /https:\/\/example\.com\//.test(asked), asked);
check("answering Cancel opens nothing", !(await ipc()).some((u) => u.includes("link_open_url")));
const p0 = (await doc()).page;
await click(pageLink.x, pageLink.y);
await settle(async () => (await doc()).page !== p0, "page link jumps");
check("a page link jumps straight there", (await doc()).page !== p0, `page ${(await doc()).page}`);
await ev(`${S}.getState().goTo(${main.id}, 0)`);
await sleep(500);

// ---- Read mode: a link naming a path outside the folder, and a Launch link, are refused ----
// (the fixture: link 1 is a GoToR to C:/Windows/x.pdf, link 2 a Launch of run.bat next to the document)
const hostile = await activate("hostile.pdf");
await ev(`${S}.getState().goTo(${hostile.id}, 0)`);
await settle(async () => (await rlLinks()).length >= 2, "hostile links", 150);
rl = await rlLinks();
report.hostileLinks = rl.map((l) => l.title);
const outside = rl.find((l) => /x\.pdf/i.test(l.title));
const launch = rl.find((l) => /run\.bat/i.test(l.title));
check("the file link and the Launch link are listed with their names", !!outside && !!launch, JSON.stringify(report.hostileLinks));
const n0 = (await dialogs()).length;
await click(outside.x, outside.y);
await settle(async () => (await dialogs()).length > n0, "outside refusal shown");
const refused = (await dialogs()).at(-1);
check("a link to a path outside the document's folder is refused, with the reason", /outside this document/i.test(refused), refused);
const n1 = (await dialogs()).length;
await click(launch.x, launch.y);
await settle(async () => (await dialogs()).length > n1, "launch refusal shown");
const refusedLaunch = (await dialogs()).at(-1);
check("a Launch link is refused: a link opens only PDF files", /only PDF/i.test(refusedLaunch) && /run\.bat/.test(refusedLaunch), refusedLaunch);
check("no token was issued for either", !(await ipc()).some((u) => u.includes("link_file_ref")));
await shot("_hostile.png");

// ---- Edit: bookmarks add, move, delete ----
await activate("bm-copy.pdf");
await key("e", "KeyE", 69, CTRL);
await settle(async () => (await ev(`${T}.getState().open`)) === true, "Edit open");
await clickText(".te-tabs button", "Bookmarks");
await settle(async () => ((await nav())?.bookmarks?.items?.length ?? 0) > 0, "bookmarks listed");
const start = await titles();
await clickText(".bm-bar .te-btn", "New");
await settle(async () => (await centre(".bm-edit")) != null, "name field");
await typeOver("Zeta marker");
await key("Enter", "Enter", 13);
await settle(async () => (await tree()).some((b) => b.title === "Zeta marker"), "added");
let d = await doc();
check("New adds a bookmark, as one named undo step", /bookmark/i.test(d.undo ?? ""), d.undo);

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
const first = (await titles())[0];
const dragged = await drag("Zeta marker", first, "before");
check("a bookmark row is dragged (real drag events)", dragged);
if (!dragged) await ev(`${N}.getState().moveBookmark(${d.id}, ${(await tree()).find((b) => b.title === "Zeta marker").id}, null, 0)`);
await settle(async () => (await titles())[0] === "Zeta marker", "moved");
d = await doc();
check("the move is one named undo step, and the bookmark is now first", /move/i.test(d.undo ?? "") && (await titles())[0] === "Zeta marker", d.undo);
await both("bookmarks-moved");

// Saved once with it, so a later Save is known to show whether the title can still be found in the file.
await ev(`document.activeElement?.blur?.()`);
await key("s", "KeyS", 83, CTRL);
await settle(async () => !(await doc()).dirty, "saved with the bookmark");
const file = (await doc()).path;
const control = hasBytes(file, "Zeta marker");
report.controlTitleInFile = control;

const zr = await centreText(".te-dock .bm-title", "Zeta marker");
await click(zr.x, zr.y);
await settle(async () => (await nav()).selectedBm != null, "selected");
await key("Delete", "Delete", 46);
await settle(async () => !(await tree()).some((b) => b.title === "Zeta marker"), "deleted by the Delete key");
d = await doc();
check("the Delete key removes the bookmark, as one named undo step", /delet|remov/i.test(d.undo ?? ""), d.undo);
check("the others are untouched", (await titles()).join() === start.join(), `${(await titles()).join()} vs ${start.join()}`);
await both("bookmarks-deleted");
await ev(`document.activeElement?.blur?.()`);
await key("z", "KeyZ", 90, CTRL);
await settle(async () => (await tree()).some((b) => b.title === "Zeta marker"), "undo of delete");
check("one undo brings it back", true);
await key("y", "KeyY", 89, CTRL);
await settle(async () => !(await tree()).some((b) => b.title === "Zeta marker"), "redo of delete");
await key("s", "KeyS", 83, CTRL);
await settle(async () => !(await doc()).dirty, "saved after delete");
if (control) check("the deleted bookmark's title is not left in the saved file", !hasBytes(file, "Zeta marker"));
else console.log("note  the title is not readable in the file even before the delete (compressed objects): leftover check skipped");

// ---- Edit: file links ----
await clickText(".te-tabs button", "Links");
await settle(async () => (await centre(".lk")) != null, "links panel");
const box = await ev(`(() => { const r = document.querySelector(".page[data-page='0']").getBoundingClientRect(); return { left: r.left, top: r.top, w: r.width }; })()`);
const zoom = box.w / d.sizes[0][0];
const at = (x, y) => [box.left + x * zoom, box.top + y * zoom];
async function drawRect(y) {
  await ev(`${S}.getState().goTo(${d.id}, 0)`);
  if ((await centre(".lk-capture")) == null) await clickText(".lk .te-btn", "Draw a link");
  await settle(async () => (await centre(".lk-capture")) != null, "draw layer");
  const [ax, ay] = at(72, y);
  const [bx, by] = at(260, y + 26);
  await mouse("mouseMoved", ax, ay, { button: "none", clickCount: 0 });
  await mouse("mousePressed", ax, ay);
  await mouse("mouseMoved", (ax + bx) / 2, (ay + by) / 2, { buttons: 1 });
  await mouse("mouseMoved", bx, by, { buttons: 1 });
  await mouse("mouseReleased", bx, by);
  await settle(async () => (await nav()).pending != null, "pending rectangle");
}
async function chooseFile(name) {
  const t = tok(name);
  if (!t) throw new Error(`no token for ${name}`);
  await ev(`window.__pick = ${JSON.stringify({ token: t.token, path: t.path })}`);
  await clickText(".lk .lk-seg", "A file");
  await clickText(".lk .te-btn", "Choose file…");
  await settle(async () => (await centre(".lk .te-format .lk-ok")) != null, "file chosen");
}

// a file one folder up from the document's folder, on the same drive
await drawRect(470);
await chooseFile("sibling.pdf");
await clickText(".lk .te-btn.primary", "Create link");
await settle(async () => (await linksOf(0)).some((k) => k.target.kind === "file"), "file link created");
const made = (await linksOf(0)).find((k) => k.target.kind === "file");
report.fileLinkName = made.target.name;
check("a PDF in a folder beside the document's is linked with ../", made.target.name === "../other/sibling.pdf", made.target.name);
check("the link says where it looks", /looks for \.\.\/other\/sibling\.pdf/.test((await nav()).notice ?? ""), (await nav()).notice);
await both("links-file-up");

// a PDF on another drive
await drawRect(520);
await chooseFile("compress-source.pdf");
const count = (await linksOf(0)).length;
await clickText(".lk .te-btn.primary", "Create link");
await settle(async () => (await err()) != null, "another-drive refusal");
const e1 = await err();
check("a PDF on another drive is refused, with the reason", /another drive/i.test(e1), e1);
check("no link was made", (await linksOf(0)).length === count);
await both("links-file-other-drive");
await clearErr();

// a program: refused as a Launch target
await clickText(".lk .lk-seg", "A file");
await ev(`window.__pick = ${JSON.stringify(tok("run.bat") ? { token: tok("run.bat").token, path: tok("run.bat").path } : null)}`);
await clickText(".lk .te-btn", "Choose file…");
await settle(async () => (await ev(`document.querySelector(".lk .te-format .lk-ok")?.textContent`)) === "run.bat", "program chosen");
await clickText(".lk .te-btn.primary", "Create link");
await settle(async () => (await err()) != null, "program refusal");
const e2 = await err();
check("a program is refused: a link opens only documents", /opens only documents/i.test(e2) && /run\.bat/.test(e2), e2);
check("still no link was made", (await linksOf(0)).length === count);
await both("links-file-program-refused");
await clearErr();
await clickText(".lk .te-btn", "Cancel");

// ---- Read mode again: the new file link asks first, and issues a token only after Ok ----
await ev(`document.activeElement?.blur?.()`);
await key("e", "KeyE", 69, CTRL);
await settle(async () => (await ev(`${T}.getState().open`)) === false, "Edit closed");
await ev(`${S}.getState().goTo(${d.id}, 0)`);
await settle(async () => (await rlLinks()).some((l) => /sibling\.pdf/.test(l.title)), "read-mode file link", 150);
await sleep(900);
await ev(`[...document.querySelectorAll(".page[data-page='0'] .rl-link")].find((e) => /sibling/.test(e.title))?.scrollIntoView({ block: "center" })`);
await sleep(700);
const fl = (await rlLinks()).find((l) => /sibling\.pdf/.test(l.title));
const m0 = (await dialogs()).length;
await click(fl.x, fl.y);
try {
  await settle(async () => (await dialogs()).length > m0, "file link asks", 40);
} catch (e) {
  const hit = await ev(`(() => { const e = document.elementFromPoint(${fl.x}, ${fl.y}); return e ? e.tagName + "." + e.className + " | " + (e.title ?? "") : null; })()`);
  console.log("DIAG at", fl.x, fl.y, "hit", hit, "ipc", JSON.stringify((await ipc()).slice(-6)), "dlg", JSON.stringify((await dialogs()).slice(-2)), "err", await err());
  throw e;
}
const q = (await dialogs()).at(-1);
check("a file link shows the whole path and asks first", /sibling\.pdf/.test(q) && /new tab/i.test(q) && /[\\/]other[\\/]sibling\.pdf/.test(q), q);
check("Cancel issued no token", !(await ipc()).some((u) => u.includes("link_file_ref")));
const tabs0 = (await docs()).length;
await ev(`window.__answer = "Ok"`);
await click(fl.x, fl.y);
await settle(async () => (await ipc()).some((u) => u.includes("link_file_ref")), "token after Ok");
check("Ok issues the token and opens the file", true);
await sleep(800);
report.tabs = [tabs0, (await docs()).length];

await send("Input.setInterceptDrags", { enabled: false });
await send("Emulation.setFocusEmulationEnabled", { enabled: false });
fs.rmSync(path.join(out, "_hostile.png"), { force: true });
console.log(JSON.stringify(report, null, 2));
ws.close();
