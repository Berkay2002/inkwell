// Drives a running Inkwell through WebView2's DevTools endpoint and exercises the image editing UI on generated
// fixtures (make-image-edit-sample.mjs): select, move, resize by a handle and by typed size, turn, flip, crop,
// replace, delete, place a new picture, undo and redo, save and reopen. Mouse and keys are CDP Input events to
// the page (no global input). The file picker is answered at the page's fetch (IPC goes through it).
//   $d = "$env:TEMP\inkwell-image-edit"; node scripts/make-image-edit-sample.mjs $d
//   node node_modules\vite\bin\vite.js --port 1457 --strictPort
//   $env:INKWELL_DATA_DIR = "$d\data"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9229'
//   src-tauri\target\debug\inkwell.exe "$d\images.pdf" "$d\logo.png" "$d\photo.png"
//   node scripts/image-edit-shots.mjs docs/screens/image-edit 9229
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/image-edit";
const port = process.argv[3] ?? "9229";
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
const T = "window.__inkwell.useTextEdit";
const I = "window.__inkwell.useImageEdit";
const report = { checks: {} };
const check = (name, ok, extra) => {
  report.checks[name] = ok ? "ok" : `FAILED ${extra ?? ""}`;
  console.log(ok ? "ok    " : "FAILED", name, ok ? "" : (extra ?? ""));
  if (!ok) process.exitCode = 1;
};
const near = (a, b, tol = 1) => Math.abs(a - b) <= tol;

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
async function click(x, y) {
  await mouse("mouseMoved", x, y, { button: "none", clickCount: 0 });
  await mouse("mousePressed", x, y);
  await mouse("mouseReleased", x, y);
  await sleep(150);
}
async function drag(x, y, dx, dy) {
  await mouse("mouseMoved", x, y, { button: "none", clickCount: 0 });
  await mouse("mousePressed", x, y);
  for (let i = 1; i <= 6; i++) await mouse("mouseMoved", x + (dx * i) / 6, y + (dy * i) / 6, { buttons: 1 });
  await mouse("mouseReleased", x + dx, y + dy);
  await sleep(200);
}
async function key(k, code, vk, modifiers = 0) {
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: k, code, windowsVirtualKeyCode: vk, modifiers });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, windowsVirtualKeyCode: vk, modifiers });
  await sleep(150);
}
const CTRL = 2;
const rectOf = (sel) =>
  ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null; const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height, left: r.left, top: r.top }; })()`);
const clickSel = async (sel) => {
  const c = await rectOf(sel);
  if (!c) throw new Error(`no element for ${sel}`);
  await click(c.x, c.y);
};
/** The rail's button with exactly this text (or aria-label). */
const buttonBy = (label) =>
  ev(`(() => { const b = [...document.querySelectorAll(".te-dock button")].find((e) => e.textContent.trim() === ${JSON.stringify(label)} || e.getAttribute("aria-label") === ${JSON.stringify(label)}); if (!b) return null; const r = b.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
const press = async (label) => {
  const c = await buttonBy(label);
  if (!c) throw new Error(`no button ${label}`);
  await click(c.x, c.y);
};
async function typeOver(text) {
  await ev(`document.activeElement.select()`);
  await send("Input.insertText", { text });
  await sleep(150);
}
async function settle(pred, what, tries = 60) {
  for (let i = 0; i < tries; i++) {
    if (await pred()) return;
    await sleep(100);
  }
  throw new Error(`${what}: did not settle`);
}

const doc = () => ev(`(() => { const s = ${S}.getState(); const d = s.docs[s.active]; return { id: d.id, name: d.name, path: d.path, dirty: d.dirty, rev: d.rev, undo: d.undoLabel, redo: d.redoLabel, size: d.sizes[0] }; })()`);
const images = () => ev(`(() => { const s = ${S}.getState(); return ${I}.getState().docs[s.active]?.pages[0]?.images ?? []; })()`);
const selection = () => ev(`(() => { const s = ${S}.getState(); const d = ${I}.getState().docs[s.active]; return { selected: d?.selected ?? null, crop: d?.crop ?? null, placing: d?.placing ?? null, notice: d?.notice ?? null }; })()`);
const selectedImage = async () => {
  const s = (await selection()).selected;
  return s ? ((await images()).find((b) => b.index === s.index) ?? null) : null;
};
const textSel = () => ev(`(() => { const s = ${S}.getState(); return ${T}.getState().docs[s.active]?.selected ?? null; })()`);
const rw = (r) => r[2] - r[0];
const rh = (r) => r[3] - r[1];

await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await theme("light");

// The file picker answers with the next tokens set in `__pick`; every IPC url is recorded.
await ev(`(() => {
  window.__pick = [];
  window.__ipc = [];
  const real = window.fetch.bind(window);
  const json = (v) => new Response(JSON.stringify(v), { headers: { "content-type": "application/json", "Tauri-Response": "ok" } });
  window.fetch = (url, opts) => {
    const u = decodeURIComponent(String(url));
    window.__ipc.push(u);
    if (u.includes("pick_source")) return Promise.resolve(json(window.__pick));
    return real(url, opts);
  };
})()`);
const given = await ev(`window.__inkwell.api.initialFiles().then((l) => l.map((f) => ({ path: f.path, token: f.token })))`);
const tok = (name) => given.find((f) => f.path.toLowerCase().endsWith(name));
const pick = (name) => ev(`window.__pick = [${JSON.stringify(tok(name))}]`);
if (!tok("logo.png") || !tok("photo.png")) throw new Error("start the app with images.pdf logo.png photo.png");

await sleep(900);
const d0 = await doc();
report.file = d0.name;
const [pw] = d0.size;
const pageBox = async () => ev(`(() => { const r = document.querySelector(".page[data-page='0']").getBoundingClientRect(); return { left: r.left, top: r.top, w: r.width }; })()`);
const toScreen = async (x, y) => {
  const b = await pageBox();
  const z = b.w / pw;
  return [b.left + x * z, b.top + y * z, z];
};

// ---- Edit on: pictures listed, outlined ----
await key("e", "KeyE", 69, CTRL);
await settle(async () => (await images()).length === 2, "two images listed");
const list0 = await images();
const A0 = list0.find((b) => near(b.fullRect[0], 72, 1) && near(b.fullRect[1], 120, 1));
const B0 = list0.find((b) => near(b.fullRect[0], 372, 1) && near(b.fullRect[1], 400, 1));
check("both pictures are listed with their boxes", !!A0 && !!B0 && near(rw(A0.fullRect), 240) && near(rh(A0.fullRect), 160), JSON.stringify(list0.map((b) => b.fullRect)));
check("their outlines are drawn on the page", (await ev(`document.querySelectorAll(".page[data-page='0'] .ie-block").length`)) === 2);
check("the rail lists them", (await ev(`document.querySelectorAll(".te-dock .te-list .te-item").length`)) >= 2);
await both("image-outlines");

// ---- select by clicking: frame with handles, Format image in the rail ----
{
  const [x, y] = await toScreen(192, 200);
  await click(x, y);
}
await settle(async () => (await selection()).selected != null, "selected A");
check("clicking a picture selects it with a frame and eight handles", (await ev(`document.querySelectorAll(".ie-frame .ie-h").length`)) === 8);
check("the rail shows its pixels and what they are", /120 × 80 px/.test(await ev(`document.querySelector(".te-dock .te-summary")?.textContent ?? ""`)));
await both("image-selected");

// ---- move by dragging ----
const aBefore = await selectedImage();
{
  const [x, y, z] = await toScreen(192, 200);
  await drag(x, y, 48, 32);
  await settle(async () => near((await selectedImage())?.fullRect[0] ?? 0, aBefore.fullRect[0] + 48 / z, 1.5), "moved");
  const a = await selectedImage();
  check("dragging moves the picture by what was dragged", near(a.fullRect[1], aBefore.fullRect[1] + 32 / z, 1.5), JSON.stringify(a.fullRect));
  check("one undo step named for the move", /Move image/.test((await doc()).undo ?? ""), (await doc()).undo);
  check("the picture is still the selected one", (await selection()).selected != null);
}

// ---- resize by a corner: proportions kept ----
{
  const a = await selectedImage();
  const h = await rectOf(".ie-frame .ie-h.se");
  const [, , z] = await toScreen(0, 0);
  await drag(h.x, h.y, 36, 0);
  await settle(async () => rw((await selectedImage()).fullRect) > rw(a.fullRect) + 5, "resized");
  const b = await selectedImage();
  check("a corner handle resizes and keeps the proportions", near(rw(b.fullRect) / rh(b.fullRect), rw(a.fullRect) / rh(a.fullRect), 0.02), JSON.stringify(b.fullRect));
  check("the top left stays where it was", near(b.fullRect[0], a.fullRect[0], 0.5) && near(b.fullRect[1], a.fullRect[1], 0.5));
  check("growth follows the pointer", near(rw(b.fullRect) - rw(a.fullRect), 36 / z, 2));
  check("one undo step named for the resize", /Resize image/.test((await doc()).undo ?? ""), (await doc()).undo);
}

// ---- turn right, flip ----
{
  const a = await selectedImage();
  await press("Turn right");
  await settle(async () => /Rotate image/.test((await doc()).undo ?? ""), "turned");
  await settle(async () => near(rw((await selectedImage()).fullRect), rh(a.fullRect), 1), "box turned");
  const b = await selectedImage();
  check("a quarter turn swaps width and height about the centre", near(rh(b.fullRect), rw(a.fullRect), 1) && near((b.fullRect[0] + b.fullRect[2]) / 2, (a.fullRect[0] + a.fullRect[2]) / 2, 1), JSON.stringify([a.fullRect, b.fullRect]));
  await press("Flip horizontally");
  await settle(async () => /Flip image/.test((await doc()).undo ?? ""), "flipped");
  const c = await selectedImage();
  check("flipping keeps the box", near(c.fullRect[0], b.fullRect[0], 0.5) && near(rw(c.fullRect), rw(b.fullRect), 0.5));
}

// ---- typed size, proportions kept ----
{
  const a = await selectedImage();
  await clickSel(".ie-num[aria-label='Width in points']");
  await typeOver("100");
  await key("Enter", "Enter", 13);
  await settle(async () => near(rw((await selectedImage()).fullRect), 100, 0.6), "typed width");
  const b = await selectedImage();
  check("a typed width sets the width, the height follows", near(rh(b.fullRect), (rh(a.fullRect) * 100) / rw(a.fullRect), 0.6), JSON.stringify([a.fullRect, b.fullRect]));
}

// ---- a size typed but not committed: the click that selects another picture commits it for its own ----
{
  const isB = (b) => near(b.fullRect[0], 372, 1) && near(b.fullRect[1], 400, 1);
  const bBefore = (await images()).find(isB);
  const aBefore = (await images()).find((b) => !isB(b));
  await clickSel(".ie-num[aria-label='Width in points']");
  await typeOver("110");
  const [x, y] = await toScreen((bBefore.fullRect[0] + bBefore.fullRect[2]) / 2, (bBefore.fullRect[1] + bBefore.fullRect[3]) / 2);
  await click(x, y);
  await settle(async () => {
    const a = (await images()).find((b) => !isB(b));
    return a && near(rw(a.fullRect), 110, 0.6);
  }, "typed width committed for the first picture");
  const a = (await images()).find((b) => !isB(b));
  const b = (await images()).find(isB);
  check("the typed size went to the picture it was typed for", near(rw(a.fullRect), 110, 0.6) && near(a.fullRect[0], aBefore.fullRect[0], 0.6), JSON.stringify([aBefore.fullRect, a.fullRect]));
  check("the picture clicked is untouched and selected", b.fullRect.every((v, i) => near(v, bBefore.fullRect[i], 0.1)) && (await selection()).selected?.index === b.index, JSON.stringify([bBefore.fullRect, b.fullRect, await selection()]));
  // back to the first picture for the steps that follow
  const [ax, ay] = await toScreen((a.fullRect[0] + a.fullRect[2]) / 2, (a.fullRect[1] + a.fullRect[3]) / 2);
  await click(ax, ay);
  await settle(async () => (await selectedImage())?.index === a.index, "selected the first picture again");
}

// ---- crop: a clip, then the rest still there ----
{
  const a = await selectedImage();
  await press("Crop");
  await settle(async () => (await selection()).crop != null, "crop mode");
  check("crop mode shows the crop rectangle and dims the rest", (await ev(`document.querySelectorAll(".ie-crop").length`)) === 1 && (await ev(`document.querySelectorAll(".ie-dim").length`)) === 4);
  await both("image-crop");
  const h = await rectOf(".ie-crop .ie-h.e");
  await drag(h.x, h.y, -30, 0);
  await settle(async () => rw((await selection()).crop.rect) < rw(a.fullRect) - 5, "crop rectangle moved");
  await press("Apply crop");
  await settle(async () => /Crop image/.test((await doc()).undo ?? ""), "cropped");
  await settle(async () => (await selection()).crop == null, "crop mode left");
  const b = await selectedImage();
  check("a clip crop keeps the whole box and shows less of it", near(rw(b.fullRect), rw(a.fullRect), 0.5) && rw(b.rect) < rw(a.fullRect) - 5, JSON.stringify([a.fullRect, b.rect]));
  check("the part cut off is marked on the frame", (await ev(`document.querySelectorAll(".ie-visible").length`)) === 1);
}

// ---- replace with the logo ----
{
  await pick("logo.png");
  await press("Replace…");
  await settle(async () => /Replace image/.test((await doc()).undo ?? ""), "replaced");
  check("replace is one named undo step", true);
  check("the replaced picture stays selected", (await selectedImage()) != null);
  check("the picker asked for images, one file", (await ev(`window.__ipc.some((u) => u.includes("pick_source"))`)) === true);
}

// ---- delete the other one with the key ----
{
  const bBox = (await images()).find((b) => near(b.fullRect[0], 372, 1));
  const [x, y] = await toScreen((bBox.fullRect[0] + bBox.fullRect[2]) / 2, (bBox.fullRect[1] + bBox.fullRect[3]) / 2);
  await click(x, y);
  await settle(async () => (await selectedImage())?.index === bBox.index, "selected B");
  await key("Delete", "Delete", 46);
  await settle(async () => (await images()).length === 1, "deleted");
  check("Delete removes the selected picture", /Delete image/.test((await doc()).undo ?? ""), (await doc()).undo);
  check("nothing is selected after a delete", (await selection()).selected == null);
}

// ---- place a new picture: choose the file, then click the page ----
{
  await pick("photo.png");
  await press("Image");
  await settle(async () => (await selection()).placing != null, "placing");
  check("choosing a file arms the page to put it somewhere", (await ev(`document.querySelectorAll(".ie-place").length`)) === 1 && /photo\.png/.test(await ev(`document.querySelector(".te-dock")?.textContent ?? ""`)));
  await shot("_placing.png");
  await both("image-placing");
  const [x, y] = await toScreen(72, 600);
  await click(x, y);
  await settle(async () => (await images()).length === 2, "placed");
  const s = await selection();
  const added = await selectedImage();
  check("a click puts it there at a sensible size and selects it", s.selected != null && added && near(added.fullRect[0], 72, 12) && rw(added.fullRect) > 40, JSON.stringify(added?.fullRect));
  check("adding is one named undo step", /Add image/.test((await doc()).undo ?? ""), (await doc()).undo);
  check("placing is over", (await selection()).placing == null);
}

// ---- a text block and an image are not selected together ----
{
  const [x, y] = await toScreen(100, 88);
  await click(x, y);
  await sleep(300);
  check("taking up a text block lets go of the picture", (await selection()).selected == null);
  await key("Escape", "Escape", 27);
}

// ---- undo, redo ----
await ev(`document.activeElement?.blur?.()`);
{
  const n = (await images()).length;
  await key("z", "KeyZ", 90, CTRL);
  await settle(async () => (await images()).length === n - 1, "undo of add");
  check("undo takes the added picture away", true);
  await key("y", "KeyY", 89, CTRL);
  await settle(async () => (await images()).length === n, "redo of add");
  check("redo brings it back", true);
}

// ---- save, reopen, read back ----
const before = await images();
await key("s", "KeyS", 83, CTRL);
await settle(async () => !(await doc()).dirty, "saved");
check("Save clears the dirty mark", true);
const p = (await doc()).path;
await ev(`(async () => { const s = ${S}.getState(); s.closeDoc(s.active); await new Promise((r) => setTimeout(r, 300)); await ${S}.getState().openPath(${JSON.stringify(p)}); })()`);
await settle(async () => (await ev(`${S}.getState().active != null`)) === true, "reopened");
await sleep(600);
await settle(async () => (await images()).length > 0, "images after reopen", 100);
const after = await images();
const same = before.length === after.length && before.every((b) => after.some((c) => b.fullRect.every((v, i) => near(v, c.fullRect[i], 0.6))));
check("the edited pictures survive saving and reopening", same, JSON.stringify([before.map((b) => b.fullRect), after.map((b) => b.fullRect)]));
await both("image-reopened");

await key("e", "KeyE", 69, CTRL);
await send("Emulation.setFocusEmulationEnabled", { enabled: false });
fs.rmSync(path.join(out, "_placing.png"), { force: true });
console.log(JSON.stringify(report, null, 2));
ws.close();
