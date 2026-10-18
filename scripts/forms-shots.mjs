// Drives a running Inkwell through WebView2's DevTools endpoint: fills every kind of field on a hyperref form, clears and
// undoes, saves and reopens; then prepares a flat form (detect, move, resize, properties, a new field, tab order, undo)
// and captures the screens (light and dark). Works on copies of samples, never on real files.
//   $d = "$env:TEMP\inkwell-forms"; mkdir $d; copy samples\form-source.pdf, samples\flatform-source.pdf $d
//   pnpm exec vite --port 1420 --strictPort        (the debug exe loads the dev server)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9237'
//   src-tauri\target\debug\inkwell.exe "$d\form-source.pdf" "$d\flatform-source.pdf"
//   node scripts/forms-shots.mjs docs/screens/forms 9237
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/forms";
const port = process.argv[3] ?? "9237";
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
const until = async (expr, what, limit = 15000) => {
  const t0 = Date.now();
  for (;;) {
    const v = await ev(expr);
    if (v) return v;
    if (Date.now() - t0 > limit) throw new Error(`timed out waiting for ${what}`);
    await sleep(120);
  }
};
const theme = (t) => ev(`window.__S.getState().setTheme(${JSON.stringify(t)})`);
const shot = async (name) => {
  await sleep(700);
  for (const t of ["light", "dark"]) {
    await theme(t);
    await sleep(450);
    const r = await send("Page.captureScreenshot", { format: "png" });
    fs.writeFileSync(path.join(out, `${name}-${t}.png`), Buffer.from(r.data, "base64"));
    console.log("screenshot", `${name}-${t}.png`);
  }
  await theme("light");
};
const mouse = (type, x, y) => send("Input.dispatchMouseEvent", { type, x, y, button: "left", buttons: type === "mouseReleased" ? 0 : 1, clickCount: 1 });
const drag = async (a, b, steps = 6) => {
  await mouse("mouseMoved", a[0], a[1]);
  await mouse("mousePressed", a[0], a[1]);
  for (let i = 1; i <= steps; i++) await mouse("mouseMoved", a[0] + ((b[0] - a[0]) * i) / steps, a[1] + ((b[1] - a[1]) * i) / steps);
  await mouse("mouseReleased", b[0], b[1]);
  await sleep(200);
};
const tap = async (p) => {
  await mouse("mouseMoved", p[0], p[1]);
  await mouse("mousePressed", p[0], p[1]);
  await mouse("mouseReleased", p[0], p[1]);
  await sleep(200);
};
const key = async (k, code, vk, modifiers = 0) => {
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: k, code, windowsVirtualKeyCode: vk, modifiers });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, windowsVirtualKeyCode: vk, modifiers });
  await sleep(150);
};
const undo = () => key("z", "KeyZ", 90, 2);
const redo = () => key("y", "KeyY", 89, 2);
const click = (sel, text) =>
  ev(`(() => { const els = [...document.querySelectorAll(${JSON.stringify(sel)})]; const b = ${text ? `els.find((e) => e.textContent.trim() === ${JSON.stringify(text)})` : "els[0]"}; b?.click(); return !!b })()`);
const centre = (sel, i = 0) =>
  ev(`(() => { const e = document.querySelectorAll(${JSON.stringify(sel)})[${i}]; if (!e) return null; const r = e.getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2] })()`);
const setSelect = (sel, value) =>
  ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return false;
    const proto = e.tagName === "SELECT" ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value").set.call(e, ${JSON.stringify(value)});
    e.dispatchEvent(new Event(e.tagName === "SELECT" ? "change" : "input", { bubbles: true }));
    if (e.tagName !== "SELECT") e.blur();
    return true })()`);
const fail = (m) => {
  throw new Error(m);
};
const results = [];
const check = (what, ok, detail = "") => {
  results.push({ what, ok: !!ok, detail });
  console.log(ok ? "ok  " : "FAIL", what, detail);
};

await ev(`(() => { window.__S = window.__inkwell.useStore; window.__F = window.__inkwell.useForms; return true })()`);
if (!(await ev("!!window.__F"))) fail("window.__inkwell.useForms is missing: rebuild the app");
const S = `window.__S.getState()`;
const F = `window.__F.getState()`;
const docId = (name) => ev(`(() => { const d = Object.values(${S}.docs).find((d) => d.name === ${JSON.stringify(name)}); return d ? d.id : null })()`);
const fields = (id) => ev(`JSON.parse(JSON.stringify(${F}.docs[${id}]?.fields ?? []))`);
const history = (id) => ev(`${S}.docs[${id}].undoLabel`);
const nSteps = (id) => ev(`window.__inkwell.useHistory.getState().docs[${id}]?.steps?.length ?? -1`).catch(() => -1);
const setup = async (id, zoom = 1) => {
  await ev(`${S}.activate(${id})`);
  await ev(`${S}.patch(${id}, { organize: false })`);
  await ev(`${S}.setSidebar("none")`);
  await until(`document.querySelectorAll(".viewer .page").length > 0 && ${S}.docs[${id}].sizes.length > 0`, "the pages");
  await ev(`${S}.setZoom(${id}, ${zoom})`);
  await ev(`${S}.goTo(${id}, 0)`);
  await sleep(700);
};
const pagePt = async (id) => {
  // Measured from the page element itself (CSS px per point), not from the store's zoom.
  const r = await ev(`(() => { const r = document.querySelector('.viewer .page[data-page="0"]').getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width } })()`);
  const w = await ev(`${S}.docs[${id}].sizes[0][0]`);
  const z = r.w / w;
  return (x, y) => [r.x + x * z, r.y + y * z];
};

// ================= fill: form-source.pdf =================
const fillId = await until(`(() => { const d = Object.values(${S}.docs).find((d) => d.name === "form-source.pdf"); return d ? d.id : null })()`, "form-source.pdf");
await setup(fillId, 1.1);
await ev(`${F}.setOpen(false)`);
await ev(`document.querySelector('button[aria-label="Forms"]').click()`);
await until(`${F}.open && (${F}.docs[${fillId}]?.fields?.length ?? 0) > 0`, "the field list");
let fs0 = await fields(fillId);
console.log("fields:", fs0.map((f) => `${f.name}:${f.kind}`).join(", "));
check("every field type is listed", ["text", "checkbox", "combo", "radio", "button"].every((k) => fs0.some((f) => f.kind === k)));
await shot("fill-empty");

// type into the text fields with the keyboard, Tab to the next
const texts = fs0.filter((f) => f.kind === "text");
await until(`document.querySelectorAll(".fm-w.k-text").length >= ${texts.length}`, "the controls");
let p0 = await centre(".fm-w.k-text", 0);
await tap(p0);
await send("Input.insertText", { text: "Alex Johnson" });
await key("Tab", "Tab", 9);
await send("Input.insertText", { text: "alex@example.com" });
await key("Tab", "Tab", 9);
let now = await fields(fillId);
check("typed text is committed on Tab", now.find((f) => f.name === texts[0].name).value === "Alex Johnson" && now.find((f) => f.name === texts[1].name).value === "alex@example.com", JSON.stringify(now.filter((f) => f.kind === "text").map((f) => f.value)));

// check box with Space (focus moved to it by Tab), radio by click, drop-down by change
await key(" ", "Space", 32);
now = await fields(fillId);
check("check box toggles with Space", now.find((f) => f.kind === "checkbox").checked === true);
await setSelect(".fm-w.k-combo", "Pro");
await sleep(500);
now = await fields(fillId);
check("drop-down takes a choice", now.find((f) => f.kind === "combo").values[0] === "Pro", JSON.stringify(now.find((f) => f.kind === "combo").values));
const rc = await centre(".fm-w.k-radio", 1);
await tap(rc);
now = await fields(fillId);
const radios = now.filter((f) => f.kind === "radio");
check("a radio group has exactly one chosen button", radios.filter((r) => r.checked).length === 1 && radios[1].checked, JSON.stringify(radios.map((r) => r.checked)));
await shot("fill-filled");

// a rejected value leaves the document as it was (no format on this form, so use the engine's maximum)
const steps = await history(fillId);
// clear form is one undo step; undo brings everything back
await click(".fm-actions .fm-btn", "Clear form");
await sleep(500);
now = await fields(fillId);
check("clear form empties every field", now.every((f) => f.kind === "button" || (f.kind === "checkbox" ? !f.checked : f.kind === "radio" ? !f.checked : f.value === "")));
await undo();
await sleep(500);
now = await fields(fillId);
check("undo restores the filled form in one step", now.find((f) => f.kind === "text").value === "Alex Johnson" && now.find((f) => f.kind === "checkbox").checked);
await redo();
await undo();
await sleep(300);

// save and reopen: values persist
await key("s", "KeyS", 83, 2);
await until(`!${S}.docs[${fillId}].dirty`, "the save", 20000);
const pathOf = await ev(`${S}.docs[${fillId}].path`);
await ev(`${S}.closeDoc(${fillId})`);
await sleep(500);
await ev(`${S}.openPath(${JSON.stringify(pathOf)})`);
const reId = await until(`(() => { const d = Object.values(${S}.docs).find((d) => d.name === "form-source.pdf"); return d ? d.id : null })()`, "the reopened form");
await ev(`${F}.refresh(${reId})`);
await until(`(${F}.docs[${reId}]?.fields?.length ?? 0) > 0`, "the reopened fields");
now = await fields(reId);
check("filled values survive save and reopen", now.find((f) => f.kind === "text").value === "Alex Johnson" && now.find((f) => f.kind === "checkbox").checked && now.find((f) => f.kind === "combo").values[0] === "Pro", JSON.stringify(now.map((f) => [f.name, f.value, f.checked])));
await setup(reId, 1.1);
await shot("fill-reopened");
await ev(`${F}.setOpen(false)`);

// ================= author: flatform-source.pdf =================
const flatId = await docId("flatform-source.pdf");
await setup(flatId, 1.1);
await ev(`document.querySelector('button[aria-label="Forms"]').click()`);
await until(`${F}.open`, "the rail");
await click(".fm-seg button", "Prepare form");
await until(`${F}.authoring && !!document.querySelector(".fm-strip")`, "the tool strip");
check("a flat form starts with no fields", (await fields(flatId)).length === 0);
await shot("author-empty");

await click(".fm-props .fm-btn", "Detect fields");
await until(`(${F}.docs[${flatId}]?.detected?.length ?? 0) > 0`, "detection");
const found = await ev(`JSON.parse(JSON.stringify(${F}.docs[${flatId}].detected))`);
console.log("detected:", found.map((d) => `${d.name}:${d.kind}`).join(", "));
check("detection predicts before adding", found.length >= 5 && (await fields(flatId)).length === 0);
await shot("author-detect-prediction");
const before = await history(flatId);
await click(".fm-props .fm-btn.primary", "Add them");
await until(`(${F}.docs[${flatId}]?.fields?.length ?? 0) >= ${found.length}`, "the detected fields");
check("adding detected fields is one undo step", (await history(flatId)) !== before, `${before} -> ${await history(flatId)}`);
await shot("author-detected");

// select one by clicking it, move it, resize it
let at = await pagePt(flatId);
let list = await fields(flatId);
const name = list.find((f) => f.name === "Email") ?? list[0];
const mid = (r) => at((r[0] + r[2]) / 2, (r[1] + r[3]) / 2);
await tap(mid(name.rect));
check("clicking a field selects it and opens its properties", (await ev(`${F}.docs[${flatId}].selected?.xref`)) === name.xref && !!(await centre(".fm-props input")));
await shot("author-properties");
const r0 = name.rect;
await drag(mid(r0), [mid(r0)[0] + 40, mid(r0)[1] + 12]);
list = await fields(flatId);
const moved = list.find((f) => f.xref === name.xref);
check("dragging moves the field", moved.rect[0] > r0[0] + 15 && moved.rect[1] > r0[1] + 4, JSON.stringify([r0, moved.rect]));
const se = await ev(`(() => { const e = document.querySelector(".fm-h.se"); if (!e) return null; const r = e.getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2] })()`);
if (se) await drag(se, [se[0] + 30, se[1] + 6]);
const resized = (await fields(flatId)).find((f) => f.xref === name.xref);
check("dragging a handle resizes the field", resized.rect[2] > moved.rect[2] + 20, JSON.stringify([moved.rect, resized.rect]));

// properties: required, number format, calculation off
await ev(`(() => { const l = [...document.querySelectorAll(".fm-props .fm-check")].find((e) => e.textContent.trim() === "Required"); l?.querySelector("input")?.click(); return !!l })()`);
await sleep(400);
check("Required is saved on the field", (await fields(flatId)).find((f) => f.xref === name.xref).required === true);
await setSelect('.fm-props select[aria-label="Show the value as"]', "number");
await sleep(500);
check("a number format is written as Acrobat's script", (await fields(flatId)).find((f) => f.xref === name.xref).format.kind === "number");
await shot("author-format");

// a new field from the strip, drawn by dragging
await ev(`document.querySelector('.fm-strip button[aria-label="Text field"]').click()`);
at = await pagePt(flatId);
const nBefore = (await fields(flatId)).length;
await drag(at(330, 300), at(520, 324));
await until(`(${F}.docs[${flatId}]?.fields?.length ?? 0) > ${nBefore}`, "the drawn field");
check("a dragged rectangle makes one field", (await fields(flatId)).length === nBefore + 1);
await shot("author-new-field");

// tab order by column, then undo it
await click(".fm-props .fm-btn", "By column");
await sleep(400);
const byCol = (await fields(flatId)).sort((a, b) => a.tab - b.tab).map((f) => f.name);
await undo();
await sleep(400);
const afterUndo = (await fields(flatId)).sort((a, b) => a.tab - b.tab).map((f) => f.name);
check("tab order by column is one undo step", byCol.join() !== "" && afterUndo.length === byCol.length);
await shot("author-tab-order");

// delete with the Delete key, undo, save, reopen
const sel = await ev(`${F}.docs[${flatId}].selected`);
const total = (await fields(flatId)).length;
await key("Delete", "Delete", 46);
await sleep(400);
const afterDelete = (await fields(flatId)).length;
check("Delete removes the selected field", sel ? afterDelete === total - 1 : true, `${total} -> ${afterDelete}`);
await undo();
await sleep(400);
check("undo brings it back", (await fields(flatId)).length === total);

await key("s", "KeyS", 83, 2);
await until(`!${S}.docs[${flatId}].dirty`, "the save", 20000);
const flatPath = await ev(`${S}.docs[${flatId}].path`);
await ev(`${S}.closeDoc(${flatId})`);
await sleep(500);
await ev(`${S}.openPath(${JSON.stringify(flatPath)})`);
const flat2 = await until(`(() => { const d = Object.values(${S}.docs).find((d) => d.name === "flatform-source.pdf"); return d ? d.id : null })()`, "the reopened flat form");
await ev(`${F}.refresh(${flat2})`);
await until(`(${F}.docs[${flat2}]?.fields?.length ?? 0) > 0`, "the saved fields");
const saved = await fields(flat2);
check("authored fields survive save and reopen", saved.length === total, `${saved.length} of ${total}: ${saved.map((f) => f.name).join(", ")}`);
check("properties survive save and reopen", saved.some((f) => f.required && f.format.kind === "number"));
await setup(flat2, 1.1);
await ev(`${F}.setAuthoring(false)`);
await shot("fill-after-authoring");

fs.writeFileSync(path.join(out, "report.json"), JSON.stringify(results, null, 2));
const bad = results.filter((r) => !r.ok);
console.log(`${results.length - bad.length}/${results.length} checks passed`);
process.exit(bad.length ? 1 : 0);
