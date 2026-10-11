// Drives a running Inkwell through WebView2's DevTools endpoint: fills in the sample's signatures page, makes a
// typed signature, flattens and undoes, then signs twice with a self-signed test ID (never a real Digital ID),
// reopens and checks both signatures. Captures the screens light and dark. Works on a generated sample only.
//   $d = "$env:TEMP\inkwell-sg"; mkdir $d; node scripts/sign-sample.mjs "$d\Lease agreement.pdf"
//   npx vite --port 1420        (the debug exe loads the dev server)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:INKWELL_NO_CERT_STORE = "1"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9226'
//   src-tauri\target\debug\inkwell.exe "$d\Lease agreement.pdf"
//   node scripts/sign-shots.mjs docs/screens/sign 9226
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/sign";
const port = process.argv[3] ?? "9226";
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
const key = async (k, code, vk, modifiers = 0) => {
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: k, code, modifiers, windowsVirtualKeyCode: vk });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, modifiers, windowsVirtualKeyCode: vk });
};
const typeInto = async (sel, t) => {
  await ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); e.focus(); e.select?.(); return true })()`);
  await send("Input.insertText", { text: t });
  await sleep(120);
};
const tool = (label) => click(`.sg-strip button[aria-label="${label}"]`);
const report = {};
const fail = (m) => {
  throw new Error(m);
};

await ev(`(() => { window.__S = window.__inkwell.useStore; window.__G = window.__inkwell.useSign; window.__A = window.__inkwell.useAnnot; return true })()`);
if (!(await ev("!!window.__G"))) fail("window.__inkwell.useSign is missing: rebuild the app");
// The screens show a made-up person, not the login name (in memory only; nothing is stored).
await ev(`window.__A.setState({ author: "Alex Johnson", defaultAuthor: "Alex Johnson" })`);
const doc = await until(`(() => { const d = Object.values(window.__S.getState().docs).find((d) => d.name === "Lease agreement.pdf"); return d ? d.id : null })()`, "the sample");
const S = `window.__S.getState()`;
const G = `window.__G.getState()`;
await ev(`${S}.activate(${doc})`);
await ev(`${S}.patch(${doc}, { organize: false })`);
await ev(`${S}.setSidebar("none")`);
await until(`document.querySelectorAll(".viewer .page").length > 0 && ${S}.docs[${doc}].sizes.length === 2`, "the pages");
await ev(`${S}.setZoom(${doc}, 1.1)`);
await ev(`${S}.goTo(${doc}, 1, 40)`);
await sleep(700);
await ev(`${G}.open || document.querySelector('button[aria-label="Fill & Sign"]').click()`);
await until(`${G}.open && !!document.querySelector(".sg-strip")`, "the strip");
const items = (n, what) => until(`(${G}.docs[${doc}]?.items?.length ?? 0) >= ${n}`, what);
const pg = async () => {
  const r = await ev(`(() => { const r = document.querySelector('.viewer .page[data-page="1"]').getBoundingClientRect(); return { x: r.x, y: r.y } })()`);
  const z = await ev(`${S}.docs[${doc}].zoom`);
  return (x, y) => [r.x + x * z, r.y + y * z];
};
const at = await pg();

// ---- text, date, check (page space is y down: PDF y 637 is 155 from the top) ----
await tool("Text");
await tap(at(156, 146));
await until(`!!document.querySelector(".sg-text-edit")`, "the text editor");
await send("Input.insertText", { text: "Alex Johnson" });
await key("Enter", "Enter", 13);
await items(1, "the name");
await tool("Today's date");
await tap(at(156, 176));
await items(2, "the date");
await tool("Check");
await tap(at(70, 216));
await items(3, "the check");

// ---- a typed signature, saved encrypted, placed on the Tenant line ----
await ev(`document.querySelectorAll(".sg-tile-btn")[0].click()`);
await until(`!!document.querySelector(".sg-modal")`, "the creator");
await until(`document.querySelector(".sg-type input")?.value === "Alex Johnson"`, "the typed name");
await ev(`document.querySelectorAll(".sg-font")[1].click()`);
await sleep(300);
await shot("creator");
await click(".sg-modal .cm-btn.primary");
await until(`!document.querySelector(".sg-modal") && ${G}.saved.length === 1 && ${G}.tool === "signature"`, "the saved signature");
report.saved = await ev(`${G}.saved.map((s) => ({ kind: s.kind, source: s.source, font: s.font, size: [s.width, s.height] }))`);
await tap(at(440, 290));
await items(4, "the placed signature");
report.items = await ev(`${G}.docs[${doc}].items.map((i) => i.item)`);
await until(`!!document.querySelector(".sg-itembar")`, "the item bar");
await shot("fill");

// ---- flatten (then undo) ----
const before = await ev(`${G}.docs[${doc}].items.length`);
await click(".sg-itembar .sg-ib", "Flatten");
await until(`${G}.docs[${doc}].items.length === 0`, "the flatten");
report.flatten = { before, after: await ev(`${G}.docs[${doc}].items.length`), undo: await ev(`${S}.docs[${doc}].undoLabel`) };
await ev(`document.activeElement?.blur?.()`);
await key("z", "KeyZ", 90, 2);
await until(`${G}.docs[${doc}].items.length === ${before}`, "the undo of the flatten");

// ---- digital signature in the empty Landlord field, with a test ID ----
await ev(`${G}.select(${doc}, null)`);
await tool("Digital signature");
await until(`${G}.rail === "sheet" && !!document.querySelector(".sg-panel")`, "the sheet");
await until(`${G}.idsLoaded`, "the IDs");
report.idsListed = await ev(`${G}.ids.length`);
await click(".sg-idlinks .sg-link", "Create a test ID…");
await typeInto('input[aria-label="Test ID name"]', "Maple Property");
await typeInto('input[aria-label="Test ID email"]', "landlord@example.com");
await click(".sg-sub .cm-btn.primary", "Create");
await until(`${G}.ids.some((i) => i.source === "test")`, "the test ID", 60000);
await typeInto("#sg-reason", "Lease signed");
await until(`!!document.querySelector(".sg-field")`, "the empty field");
await click(".sg-field");
await until(`${G}.docs[${doc}].draft?.field === "Landlord" && !!document.querySelector(".sg-preview")`, "the preview");
await shot("sign-sheet");
await click(".sg-sign");
await until(`${G}.rail === "list" && (${G}.docs[${doc}].sigs ?? []).some((s) => s.status !== "unsigned")`, "the signature", 60000);
await until(`!!document.querySelector(".sg-banner")`, "the banner");
report.first = await ev(`${G}.docs[${doc}].sigs.map((s) => ({ field: s.field, status: s.status, intact: s.intact, changes: s.changes, subFilter: s.subFilter }))`);
report.dirtyAfterSign = await ev(`${S}.docs[${doc}].dirty`);

// ---- a second signature in a dragged box on the Witness line ----
await tool("Digital signature");
await until(`${G}.rail === "sheet"`, "the sheet again");
await drag(at(66, 352), at(278, 410));
await until(`!!${G}.docs[${doc}].draft && !${G}.docs[${doc}].draft.field`, "the dragged box");
await click(".sg-sign");
await until(`${G}.rail === "list" && ${G}.docs[${doc}].sigs.filter((s) => s.status !== "unsigned").length === 2`, "the second signature", 60000);
await sleep(300);
await ev(`document.querySelector(".sg-more")?.click()`);
await ev(`${G}.setOpen(false)`);
await shot("signed");

// ---- close and reopen: both signatures and the items are still there ----
const file = await ev(`${S}.docs[${doc}].path`);
await ev(`${S}.closeDoc(${doc})`);
await sleep(500);
await ev(`${S}.openPath(${JSON.stringify(file)})`);
const doc2 = await until(`(() => { const d = Object.values(${S}.docs).find((d) => d.path === ${JSON.stringify(file)}); return d ? d.id : null })()`, "the reopened doc");
await ev(`${S}.activate(${doc2})`);
await until(`${S}.docs[${doc2}].sizes.length === 2`, "reopened pages");
await ev(`${G}.refresh(${doc2})`);
await until(`(${G}.docs[${doc2}]?.sigs ?? []).filter((s) => s.status !== "unsigned").length === 2`, "reopened signatures");
report.reopened = await ev(`({ sigs: ${G}.docs[${doc2}].sigs.map((s) => [s.field, s.status, s.intact, s.changes]), items: ${G}.docs[${doc2}].items.map((i) => i.item) })`);

console.log(JSON.stringify(report, null, 2));
ws.close();
