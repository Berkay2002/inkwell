// Drives a running Inkwell through WebView2's DevTools endpoint: makes every kind of comment, moves, resizes,
// recolours, replies, resolves, saves and reopens, then captures the screens (light and dark). Works on a copy of a
// generated sample (scripts/redact-sample.mjs), never on real files.
//   $d = "$env:TEMP\inkwell-cm"; mkdir $d; node scripts/redact-sample.mjs "$d\Lease agreement.pdf"
//   npx vite --port 1420        (the debug exe loads the dev server)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9225'
//   src-tauri\target\debug\inkwell.exe "$d\Lease agreement.pdf"
//   node scripts/comments-shots.mjs docs/screens/comments 9225
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/comments";
const port = process.argv[3] ?? "9225";
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
  await sleep(600);
  for (const t of ["light", "dark"]) {
    await theme(t);
    await sleep(400);
    const r = await send("Page.captureScreenshot", { format: "png" });
    fs.writeFileSync(path.join(out, `${name}-${t}.png`), Buffer.from(r.data, "base64"));
    console.log("screenshot", `${name}-${t}.png`);
  }
  await theme("light");
};
const click = (sel, text) =>
  ev(`(() => { const els = [...document.querySelectorAll(${JSON.stringify(sel)})]; const b = ${text ? `els.find((e) => e.textContent.trim() === ${JSON.stringify(text)})` : "els[0]"}; b?.click(); return !!b })()`);
const mouse = (type, x, y) => send("Input.dispatchMouseEvent", { type, x, y, button: "left", buttons: type === "mouseReleased" ? 0 : 1, clickCount: 1 });
const drag = async (a, b, steps = 6) => {
  await mouse("mouseMoved", a[0], a[1]);
  await mouse("mousePressed", a[0], a[1]);
  for (let i = 1; i <= steps; i++) await mouse("mouseMoved", a[0] + ((b[0] - a[0]) * i) / steps, a[1] + ((b[1] - a[1]) * i) / steps);
  await mouse("mouseReleased", b[0], b[1]);
  await sleep(150);
};
const tap = async (p) => {
  await mouse("mouseMoved", p[0], p[1]);
  await mouse("mousePressed", p[0], p[1]);
  await mouse("mouseReleased", p[0], p[1]);
  await sleep(150);
};
const tool = (label) => click(`.an-strip button[aria-label="${label}"]`);
const report = {};
const fail = (m) => {
  throw new Error(m);
};

await ev(`(() => { window.__S = window.__inkwell.useStore; window.__A = window.__inkwell.useAnnot; return true })()`);
if (!(await ev("!!window.__A"))) fail("window.__inkwell.useAnnot is missing: rebuild the app");
const doc = await until(`(() => { const d = Object.values(window.__S.getState().docs).find((d) => d.name === "Lease agreement.pdf"); return d ? d.id : null })()`, "the sample");
const S = `window.__S.getState()`;
const A = `window.__A.getState()`;
await ev(`${S}.activate(${doc})`);
await ev(`${S}.patch(${doc}, { organize: false })`);
await ev(`${S}.setSidebar("none")`);
await until(`document.querySelectorAll(".viewer .page").length > 0 && ${S}.docs[${doc}].sizes.length === 9`, "the pages");
await ev(`${S}.setZoom(${doc}, 0.85)`);
await ev(`${S}.goTo(${doc}, 1)`);
await sleep(600);
await click('button[aria-label="Comments"]');
await until(`${A}.open && !!document.querySelector(".an-strip")`, "the rail and strip");
const items = () => ev(`${A}.docs[${doc}]?.items?.length ?? 0`);
const kinds = () => ev(`(${A}.docs[${doc}]?.items ?? []).filter((a) => a.parent == null).map((a) => a.kind).sort()`);
const pg = async () => {
  const r = await ev(`(() => { const r = document.querySelector('.viewer .page[data-page="1"]').getBoundingClientRect(); return { x: r.x, y: r.y } })()`);
  const z = await ev(`${S}.docs[${doc}].zoom`);
  return (x, y) => [r.x + x * z, r.y + y * z];
};
let at = await pg();
const waitCount = async (n, what) => until(`(${A}.docs[${doc}]?.items?.length ?? 0) >= ${n}`, what);

// ---- markup on a text selection ----
await tool("Highlight");
await ev(`(() => {
  const line = [...document.querySelectorAll('.viewer .page[data-page="1"] .textlayer > div')].find((d) => d.textContent.includes("Alex Johnson"));
  const t = line.firstChild, i = t.textContent.indexOf("Alex Johnson");
  const r = document.createRange(); r.setStart(t, i); r.setEnd(t, i + 12);
  const s = getSelection(); s.removeAllRanges(); s.addRange(r);
  window.dispatchEvent(new PointerEvent("pointerup"));
  return true })()`);
await waitCount(1, "the highlight");
await tool("Underline");
await ev(`(() => {
  const line = [...document.querySelectorAll('.viewer .page[data-page="1"] .textlayer > div')].find((d) => d.textContent.includes("121212-1212"));
  const t = line.firstChild, i = t.textContent.indexOf("121212");
  const r = document.createRange(); r.setStart(t, i); r.setEnd(t, i + 11);
  const s = getSelection(); s.removeAllRanges(); s.addRange(r);
  window.dispatchEvent(new PointerEvent("pointerup"));
  return true })()`);
await waitCount(2, "the underline");

// ---- note, text box, pen, shapes, stamp ----
await tool("Note");
await tap(at(420, 420));
await waitCount(3, "the note");
await until(`!!document.querySelector(".an-pop-text")`, "the note's popover");
await ev(`document.querySelector(".an-pop-text").focus()`);
await send("Input.insertText", { text: "Please confirm the rent with the landlord." });
await ev(`document.querySelector(".an-pop-text").blur()`);
await until(`(${A}.docs[${doc}].items.find((a) => a.kind === "note")?.contents ?? "").includes("confirm")`, "the note text");

await tool("Text box");
await drag(at(64, 470), at(300, 520));
await until(`!!document.querySelector(".an-box-edit")`, "the text box editor");
await send("Input.insertText", { text: "Initial each page" });
await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", modifiers: 2, windowsVirtualKeyCode: 13 });
await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", modifiers: 2, windowsVirtualKeyCode: 13 });
await waitCount(4, "the text box");

await tool("Pen");
await drag(at(330, 560), at(500, 600), 14);
await waitCount(5, "the pen stroke");

await tool("Rectangle");
await drag(at(64, 560), at(200, 640));
await waitCount(6, "the rectangle");
await tool("Arrow");
await drag(at(240, 640), at(320, 580));
await waitCount(7, "the arrow");

await tool("Stamp");
await tap(at(430, 700));
await waitCount(8, "the stamp");
report.created = await kinds();
report.author = { default: await ev(`${A}.defaultAuthor`), onComments: await ev(`[...new Set(${A}.docs[${doc}].items.map((a) => a.author))]`) };

// ---- select, move, resize, recolour ----
await tool("Select");
const rectOf = (kind) => ev(`${A}.docs[${doc}].items.find((a) => a.kind === ${JSON.stringify(kind)}).rect`);
const boxBefore = await rectOf("rectangle");
const c = [(boxBefore[0] + boxBefore[2]) / 2, (boxBefore[1] + boxBefore[3]) / 2];
await drag(at(...c), at(c[0] + 40, c[1] - 30));
await until(`${A}.docs[${doc}].items.find((a) => a.kind === "rectangle").rect[0] > ${boxBefore[0] + 20}`, "the move");
const moved = await rectOf("rectangle");
report.move = { before: boxBefore, after: moved };
await until(`!!document.querySelector(".an-h.se")`, "the handles");
const h = await ev(`(() => { const r = document.querySelector(".an-h.se").getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2] })()`);
await drag(h, [h[0] + 50, h[1] + 30]);
await until(`${A}.docs[${doc}].items.find((a) => a.kind === "rectangle").rect[2] > ${moved[2] + 20}`, "the resize");
report.resize = await rectOf("rectangle");
await until(`!!document.querySelector(".an-pop")`, "the popover");
await click('.an-dot[title="Green"]');
await until(`(${A}.docs[${doc}].items.find((a) => a.kind === "rectangle").color ?? [])[1] > 0.7`, "the colour");
report.colour = await ev(`${A}.docs[${doc}].items.find((a) => a.kind === "rectangle").color`);
await shot("selection-popover");

// ---- reply, resolve ----
await ev(`(() => { const b = [...document.querySelectorAll(".cm-card .cm-main")].find((e) => e.textContent.includes("confirm the rent")); b.click(); return !!b })()`);
await sleep(300);
const card = `[...document.querySelectorAll(".cm-card")].find((e) => e.textContent.includes("confirm the rent"))`;
await ev(`${card}.querySelector(".cm-link").click()`);
await until(`!!document.querySelector(".cm-compose textarea")`, "the reply box");
await ev(`document.querySelector(".cm-compose textarea").focus()`);
await send("Input.insertText", { text: "Confirmed by phone, 12 400 SEK." });
await sleep(100);
await click(".cm-compose .cm-btn.primary");
await until(`${A}.docs[${doc}].items.some((a) => a.parent != null)`, "the reply");
const first = await ev(`${A}.docs[${doc}].items.find((a) => a.parent != null)`);
report.reply = { parent: first.parent, state: first.state };
await shot("thread");
await ev(`${card}.querySelectorAll(".cm-link")[1].click()`);
await until(`${A}.docs[${doc}].items.find((a) => a.kind === "note").resolved`, "resolved");
await ev(`${A}.select(${doc}, null)`);
await ev(`document.querySelector(".cm-body").scrollTop = 0`);
await shot("rail");

// ---- save, close, reopen ----
const before = await ev(`${A}.docs[${doc}].items.map((a) => ({ kind: a.kind, contents: a.contents, resolved: a.resolved, parent: a.parent, rect: a.rect.map((v) => Math.round(v)) })).sort((x, y) => JSON.stringify(x).localeCompare(JSON.stringify(y)))`);
await ev(`document.activeElement?.blur?.()`);
await send("Input.dispatchKeyEvent", { type: "keyDown", key: "s", code: "KeyS", modifiers: 2, windowsVirtualKeyCode: 83 });
await send("Input.dispatchKeyEvent", { type: "keyUp", key: "s", code: "KeyS", modifiers: 2, windowsVirtualKeyCode: 83 });
await until(`!${S}.docs[${doc}].dirty`, "the save");
const file = await ev(`${S}.docs[${doc}].path`);
await ev(`${S}.closeDoc(${doc})`);
await sleep(500);
await ev(`${S}.openPath(${JSON.stringify(file)})`);
const doc2 = await until(`(() => { const d = Object.values(${S}.docs).find((d) => d.path === ${JSON.stringify(file)}); return d ? d.id : null })()`, "the reopened doc");
await ev(`${S}.activate(${doc2})`);
await until(`${S}.docs[${doc2}].sizes.length === 9`, "reopened pages");
await ev(`${A}.refresh(${doc2})`);
await until(`(${A}.docs[${doc2}]?.items?.length ?? 0) > 0`, "reopened comments");
const after = await ev(`${A}.docs[${doc2}].items.map((a) => ({ kind: a.kind, contents: a.contents, resolved: a.resolved, parent: a.parent, rect: a.rect.map((v) => Math.round(v)) })).sort((x, y) => JSON.stringify(x).localeCompare(JSON.stringify(y)))`);
const norm = (l) => JSON.stringify(l.map((a) => ({ ...a, parent: a.parent != null })));
report.survived = norm(before) === norm(after);
report.counts = { before: before.length, after: after.length };
if (!report.survived) report.diff = { before, after };
await ev(`${S}.goTo(${doc2}, 1)`);
await sleep(800);
await shot("reopened");

console.log(JSON.stringify(report, null, 2));
ws.close();
