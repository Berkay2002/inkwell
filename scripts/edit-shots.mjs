// Drives a running Inkwell through WebView2's DevTools endpoint and exercises Edit mode on a COPY of a study
// sample: turns Edit on (Ctrl+E), edits a line, changes size and colour, adds a text box, undoes and redoes,
// saves, reopens and checks the file. Mouse and keys are CDP Input events to the page (no global input).
//   $d = "$env:TEMP\inkwell-edit"; mkdir $d; copy samples\edit-source.pdf "$d\edit-copy.pdf"
//   node node_modules\vite\bin\vite.js --port 1420 --strictPort        (the debug exe loads the dev server)
//   $env:INKWELL_DATA_DIR = "$d\data"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9227'
//   src-tauri\target\debug\inkwell.exe "$d\edit-copy.pdf"
//   node scripts/edit-shots.mjs docs/screens/edit 9227
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/edit";
const port = process.argv[3] ?? "9227";
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
async function click(x, y) {
  await mouse("mouseMoved", x, y, { button: "none", clickCount: 0 });
  await mouse("mousePressed", x, y);
  await mouse("mouseReleased", x, y);
  await sleep(120);
}
async function key(k, code, vk, modifiers = 0) {
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: k, code, windowsVirtualKeyCode: vk, modifiers });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, windowsVirtualKeyCode: vk, modifiers });
  await sleep(120);
}
const CTRL = 2;
const centre = (sel) =>
  ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null; const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height }; })()`);
const clickSel = async (sel) => {
  const c = await centre(sel);
  if (!c) throw new Error(`no element for ${sel}`);
  await click(c.x, c.y);
};
/** Replaces the whole content of the focused field with `text`. */
async function typeOver(text) {
  await ev(`document.activeElement.select()`);
  await send("Input.insertText", { text });
  await sleep(150);
}
const doc = () => ev(`(() => { const s = ${S}.getState(); const d = s.docs[s.active]; return { id: d.id, name: d.name, path: d.path, dirty: d.dirty, rev: d.rev, undo: d.undoLabel, redo: d.redoLabel, sizes: d.sizes[0] }; })()`);
const edit = () => ev(`(() => { const s = ${S}.getState(); const t = ${T}.getState(); const d = t.docs[s.active]; return { open: t.open, tool: t.tool, selected: d?.selected ?? null, draft: d?.draft ?? null, notice: d?.notice ?? null, adding: d?.adding ?? null }; })()`);
const blocks = (page = 0) => ev(`(() => { const s = ${S}.getState(); return ${T}.getState().docs[s.active]?.pages[${page}]?.blocks ?? []; })()`);
async function settle(pred, what, tries = 60) {
  for (let i = 0; i < tries; i++) {
    if (await pred()) return;
    await sleep(100);
  }
  throw new Error(`${what}: did not settle`);
}
const withText = (bs, t) => bs.find((b) => b.text.includes(t));

await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await theme("light");
await sleep(900);
const d0 = await doc();
report.file = d0.name;

// ---- Edit on (Ctrl+E), outlines listed once per page ----
await key("e", "KeyE", 69, CTRL);
await settle(async () => (await edit()).open, "Edit open");
await settle(async () => (await blocks()).length > 0, "blocks listed");
const first = await blocks();
report.blocksOnPage1 = first.length;
check("edit rail opens and page 1 lists blocks", first.length > 0 && (await centre(".te-dock")) != null);
check("outlines drawn on the page", (await ev(`document.querySelectorAll(".page[data-page='0'] .te-block").length`)) > 0);
await both("edit-outlines");

// ---- choose a line to edit: the longest editable block under 70 characters ----
const target = first.filter((b) => b.editable && b.text.length >= 12 && b.text.length <= 70).sort((a, b) => b.text.length - a.text.length)[0];
if (!target) throw new Error("no editable block to use");
report.target = { text: target.text, font: target.font, size: target.size };
const [x0, y0, x1, y1] = target.rect;
const pageBox = await ev(`(() => { const r = document.querySelector(".page[data-page='0']").getBoundingClientRect(); return { left: r.left, top: r.top, w: r.width }; })()`);
const zoom = pageBox.w / d0.sizes[0];
const toScreen = (x, y) => [pageBox.left + x * zoom, pageBox.top + y * zoom];
const [cx, cy] = toScreen((x0 + x1) / 2, (y0 + y1) / 2);

// ---- click into the line: an editor in the block's own font, size and colour ----
await click(cx, cy);
await settle(async () => (await centre(".te-editor")) != null, "editor");
const styled = await ev(`(() => { const e = document.querySelector(".te-editor"); const c = getComputedStyle(e); return { value: e.value, size: parseFloat(c.fontSize), color: c.color, focused: document.activeElement === e }; })()`);
check("clicking a line opens the in-place editor, focused", styled.focused && styled.value === target.text, JSON.stringify(styled));
check("editor font size matches the block", Math.abs(styled.size - target.size) < 0.05, `${styled.size} vs ${target.size}`);
await shot("_editor.png");
await both("edit-editing");

// ---- type a change, click away: one undo step ----
const typed = target.text.replace(/\S+$/, "") + "revised";
await typeOver(typed);
check("typing keeps the draft until the block is left", (await doc()).dirty === false && (await edit()).draft?.text === typed);
await click(...toScreen(Math.min(x1 + 150, d0.sizes[0] - 20), y0 + 400 > d0.sizes[1] ? 30 : y0 + 400));
await settle(async () => (await doc()).dirty, "edit committed");
const afterText = await blocks();
check("the line reads the new text after the commit", afterText.some((b) => b.text.includes("revised")), JSON.stringify(afterText.map((b) => b.text)));
const d1 = await doc();
check("one undo step named for the block", /text/i.test(d1.undo ?? ""), d1.undo);
report.undoLabel = d1.undo;

// ---- size and colour from the rail ----
const revised = withText(afterText, "revised");
const [rx0, ry0, rx1, ry1] = revised.rect;
await click(...toScreen((rx0 + rx1) / 2, (ry0 + ry1) / 2));
await settle(async () => (await edit()).selected != null && (await centre(".te-format")) != null, "format controls");
const sizeBefore = revised.size;
await clickSel(".te-stepper input");
await typeOver(String(Math.round(sizeBefore) + 4));
await key("Enter", "Enter", 13);
await settle(async () => (await withText(await blocks(), "revised"))?.size > sizeBefore + 2, "size change");
const resized = withText(await blocks(), "revised");
check("size change applied to the block", Math.abs(resized.size - (Math.round(sizeBefore) + 4)) < 0.6, `${resized.size}`);
await clickSel(".te-sw[aria-label='Colour #2e4b8f']");
await settle(async () => {
  const b = withText(await blocks(), "revised");
  return b && Math.abs(b.color[2] - 0x8f / 255) < 0.02;
}, "colour change");
check("colour change applied", true);
await both("edit-format");

// ---- add a text box in an empty spot ----
const all = await blocks();
let spot = null;
for (let y = 60; y < d0.sizes[1] - 60 && !spot; y += 18) {
  const hit = all.some((b) => b.rect[0] < 64 + 220 && b.rect[2] > 64 - 6 && b.rect[1] < y + 20 && b.rect[3] > y - 6);
  if (!hit) spot = [64, y];
}
if (!spot) throw new Error("no empty spot on the page");
await clickSel(".te-btn:not(:disabled)");
check("Text tool is on", (await edit()).tool === "text");
await click(...toScreen(...spot));
await settle(async () => (await centre(".te-new")) != null, "new text box");
await send("Input.insertText", { text: "Added by Inkwell" });
await sleep(150);
await shot("_adding.png");
await both("edit-add");
await key("Escape", "Escape", 27);
await settle(async () => (await blocks()).some((b) => b.text.includes("Added by Inkwell")), "text box committed");
check("the new text box is a block of its own", true);
check("tool returns to select", (await edit()).tool === "select");
const d2 = await doc();
check("adding is one named undo step", /add text/i.test(d2.undo ?? ""), d2.undo);

// ---- undo and redo through the engine ----
await ev(`document.activeElement?.blur?.()`);
await key("z", "KeyZ", 90, CTRL);
await settle(async () => !(await blocks()).some((b) => b.text.includes("Added by Inkwell")), "undo of add");
check("undo removes the added box", true);
await key("z", "KeyZ", 90, CTRL);
await settle(async () => {
  const b = withText(await blocks(), "revised");
  return b && Math.abs(b.color[2] - 0x8f / 255) > 0.05;
}, "undo of colour");
await key("z", "KeyZ", 90, CTRL);
await settle(async () => {
  const b = withText(await blocks(), "revised");
  return b && Math.abs(b.size - sizeBefore) < 0.6;
}, "undo of size");
check("undo steps back through colour, then size", true);
for (let i = 0; i < 3; i++) await key("y", "KeyY", 89, CTRL);
await settle(async () => (await blocks()).some((b) => b.text.includes("Added by Inkwell")), "redo restores the box");
check("redo brings the box back", true);

// ---- save, reopen, read back ----
const saved = await doc();
await key("s", "KeyS", 83, CTRL);
await settle(async () => !(await doc()).dirty, "saved");
check("Save clears the dirty mark", true);
const p = saved.path;
await ev(`(async () => { const s = ${S}.getState(); s.closeDoc(s.active); await new Promise((r) => setTimeout(r, 300)); await ${S}.getState().openPath(${JSON.stringify(p)}); })()`);
await settle(async () => (await ev(`${S}.getState().active != null`)) === true, "reopened");
await sleep(600);
// the editor is still on: the new document's page lists its blocks again
await settle(async () => (await blocks()).length > 0, "blocks after reopen", 100);
const reopened = await blocks();
check("saved edit survives reopening", reopened.some((b) => b.text.includes("revised")) && reopened.some((b) => b.text.includes("Added by Inkwell")), JSON.stringify(reopened.map((b) => b.text)));
const back = withText(reopened, "revised");
check("size and colour survive reopening", back && back.size > sizeBefore + 2 && Math.abs(back.color[2] - 0x8f / 255) < 0.02, JSON.stringify(back && { size: back.size, color: back.color }));
await both("edit-reopened");

// ---- typed text is not lost on close: Ctrl+W sends it first, then asks about the unsaved edit ----
// The native "Save changes?" dialog is answered at the page's fetch (IPC goes through it; `invoke` itself is
// read-only): Cancel, no OS-level input.
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
const before = withText(await blocks(), "revised");
const [bx0, by0, bx1, by1] = before.rect;
const pb = await ev(`(() => { const r = document.querySelector(".page[data-page='0']").getBoundingClientRect(); return { left: r.left, top: r.top, w: r.width }; })()`);
const z = pb.w / d0.sizes[0];
await click(pb.left + ((bx0 + bx1) / 2) * z, pb.top + ((by0 + by1) / 2) * z);
await settle(async () => (await centre(".te-editor")) != null, "editor before close");
await typeOver(`${before.text} guarded`);
check("the text is typed but not yet sent", (await edit()).draft?.text?.endsWith("guarded") === true);
await key("w", "KeyW", 87, CTRL);
await settle(async () => (await ev(`window.__dlg.length`)) > 0, "close prompt");
const asked = await ev(`window.__dlg[0].message`);
check("closing asks about the unsaved edit instead of dropping it", /Save the changes/.test(asked), asked);
check("the typed text was sent first", (await blocks()).some((b) => b.text.endsWith("guarded")), JSON.stringify((await blocks()).map((b) => b.text)));
check("Cancel keeps the tab open", (await ev(`${S}.getState().active != null`)) === true);

await key("e", "KeyE", 69, CTRL);
await send("Emulation.setFocusEmulationEnabled", { enabled: false });
for (const f of ["_editor.png", "_adding.png"]) fs.rmSync(path.join(out, f), { force: true });
console.log(JSON.stringify(report, null, 2));
ws.close();
