// Drives a running Inkwell through WebView2's DevTools endpoint on a COPY of docs/study/samples/_gen/formsjs-sample.pdf
// (made by `cargo test --lib write_the_form_javascript_sample -- --ignored`): number format, a date keystroke that rejects bad
// input, a SUM total, a range error, a runaway script that times out while the app stays responsive, alerts and links as
// dialogs, save and reopen, the banner switch; then the authoring tabs (live sample, custom script boxes, calculation order).
// Screens, light and dark, go to docs/screens/forms-js/.
//   $d = "$env:TEMP\inkwell-formsjs"; mkdir $d; copy docs\study\samples\_gen\formsjs-sample.pdf $d
//   pnpm exec vite --port 1420 --strictPort        (the debug exe loads the dev server)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:WEBVIEW2_USER_DATA_FOLDER = "$d\wv"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9241'
//   src-tauri\target\debug\inkwell.exe "$d\formsjs-sample.pdf"
//   node scripts/formsjs-shots.mjs docs/screens/forms-js 9241
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/forms-js";
const port = process.argv[3] ?? "9241";
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
/** Runs a store action and returns nothing (some actions return a large object CDP cannot serialise). */
const act = (expression) => ev(`(async () => { await (${expression}); return true })()`);
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
const key = async (k, code, vk, modifiers = 0) => {
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: k, code, windowsVirtualKeyCode: vk, modifiers });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, windowsVirtualKeyCode: vk, modifiers });
  await sleep(150);
};
const undo = () => key("z", "KeyZ", 90, 2);
const click = (sel, text) =>
  ev(`(() => { const els = [...document.querySelectorAll(${JSON.stringify(sel)})]; const b = ${text ? `els.find((e) => e.textContent.trim() === ${JSON.stringify(text)})` : "els[0]"}; b?.click(); return !!b })()`);
/** Sets a controlled input, textarea or select the way React hears it, then blurs it (which commits). */
const setControl = (sel, value) =>
  ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return false;
    if (e.tagName !== "SELECT") e.focus();
    const proto = e.tagName === "SELECT" ? HTMLSelectElement.prototype : e.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value").set.call(e, ${JSON.stringify(value)});
    e.dispatchEvent(new Event(e.tagName === "SELECT" ? "change" : "input", { bubbles: true }));
    if (e.tagName !== "SELECT") e.blur();
    return true })()`);
const results = [];
const check = (what, ok, detail = "") => {
  results.push({ what, ok: !!ok, detail });
  console.log(ok ? "ok  " : "FAIL", what, detail);
};

await ev(`(() => { window.__S = window.__inkwell.useStore; window.__F = window.__inkwell.useForms; return true })()`);
await send("Emulation.setFocusEmulationEnabled", { enabled: true });
const S = `window.__S.getState()`;
const F = `window.__F.getState()`;
const id = await until(`(() => { const d = Object.values(${S}.docs).find((d) => d.name === "formsjs-sample.pdf"); return d ? d.id : null })()`, "formsjs-sample.pdf");
const fields = (d = id) => ev(`JSON.parse(JSON.stringify(${F}.docs[${d}]?.fields ?? []))`);
const field = async (name, d = id) => (await fields(d)).find((f) => f.name === name);
const scripts = (d = id) => ev(`JSON.parse(JSON.stringify(${F}.docs[${d}]?.scripts ?? null))`);
const calcOrder = (d = id) => ev(`JSON.parse(JSON.stringify(${F}.docs[${d}]?.calcOrder ?? []))`);

await act(`${S}.activate(${id})`);
await act(`${S}.patch(${id}, { organize: false })`);
await act(`${S}.setSidebar("none")`);
await until(`document.querySelectorAll(".viewer .page").length > 0 && ${S}.docs[${id}].sizes.length > 0`, "the pages");
await act(`${S}.setZoom(${id}, 1)`);
await act(`${S}.goTo(${id}, 0)`);
await until(`(${F}.docs[${id}]?.fields?.length ?? 0) === 12`, "the 12 fields");
await sleep(600);

/** Types into a field's control like a person: focus, replace, Tab away (which commits). */
async function fillText(name, text) {
  const f = await field(name);
  const sel = `[data-fm-xref="${f.xref}"] input, [data-fm-xref="${f.xref}"] textarea`;
  await until(`!!document.querySelector(${JSON.stringify(sel)})`, `the control of ${name}`);
  await ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); e.focus(); e.select(); return true })()`);
  await send("Input.insertText", { text });
  await key("Tab", "Tab", 9);
  await sleep(450);
}
const bubble = () => ev(`document.querySelector(".fm-err")?.textContent ?? null`);
const promptText = () => ev(`document.querySelector(".fm-prompt")?.innerText ?? null`);

// ---- the banner, before anything is touched ------------------------------------------------------------------
const sc0 = await scripts();
check("the file reports that it has scripts, running", sc0?.hasScripts === true && sc0?.enabled === true, JSON.stringify(sc0));
const bannerText = await ev(`document.querySelector(".fm-banner")?.innerText ?? null`);
check('the banner says "This form runs scripts."', /This form runs scripts/.test(bannerText ?? ""), bannerText);
await shot("banner");

// ---- number format, and the runaway script (it runs with the first fill that recalculates) -------------------
const priceF = await field("Price");
const probe = await ev(`(async () => {
  let max = 0, last = performance.now();
  const t = setInterval(() => { const n = performance.now(); max = Math.max(max, n - last); last = n; }, 40);
  const t0 = performance.now();
  const err = await ${F}.setValue(${id}, ${priceF.page}, ${priceF.xref}, "1234.5");
  clearInterval(t);
  return { err, ms: Math.round(performance.now() - t0), maxGap: Math.round(max) };
})()`);
console.log("first fill (runs the while(true){} calculation):", JSON.stringify(probe));
check("the first fill is accepted", probe.err === null);
check("the runaway calculation is cut off (the fill takes about its limit, not for ever)", probe.ms < 8000, `${probe.ms} ms`);
check("the app stays responsive meanwhile (no gap of 400 ms in the page's event loop)", probe.maxGap < 400, `longest gap ${probe.maxGap} ms`);
const price = await field("Price");
check("number format shows $1,234.50 and keeps 1234.5", price.shown === "$1,234.50" && price.value === "1234.5", `${price.value} / ${price.shown}`);
const sc1 = await scripts();
check("the runaway script is in the Scripts log", sc1.log.some((l) => l.field === "Spin" || /longer|timed out|limit/i.test(l.message)), JSON.stringify(sc1.log));

await act(`${F}.setOpen(true)`);
await until(`!!document.querySelector(".fm-scripts")`, "the Scripts note");
const noteClosed = await ev(`document.querySelector(".fm-scripts").open`);
check("the Scripts note is collapsed by default, with a count badge", noteClosed === false && (await ev(`!!document.querySelector(".fm-scripts .fm-badge")`)), "");
await ev(`document.querySelector(".fm-scripts > summary").click()`);
await sleep(300);
await shot("scripts-note");
await ev(`document.querySelector(".fm-scripts > summary").click()`);

// ---- date keystroke ---------------------------------------------------------------------------------------------
await fillText("Born", "31/02/2000");
const badDate = await bubble();
check("a bad date is rejected with Acrobat's message", /Invalid date/.test(badDate ?? "") && (await field("Born")).value === "", String(badDate));
await shot("bad-date");
await fillText("Born", "18/04/1991");
const born = await field("Born");
check("a good date is kept and shown in the format", born.value === "18/04/1991" && born.shown === "18/04/1991", `${born.value} / ${born.shown}`);

// ---- the SUM total -----------------------------------------------------------------------------------------------
await fillText("Rent", "1000");
await fillText("Fees", "200");
await fillText("Other", "50.5");
const total = await field("Total");
check("the SUM total updates (1000 + 200 + 50.5)", total.value === "1250.5" && total.shown === "$1,250.50", `${total.value} / ${total.shown}`);
const tax = await field("Tax");
check("the custom calculation calls the document-level function (tax = 312.63)", tax.value === "312.63", tax.value);

// ---- range error -------------------------------------------------------------------------------------------------
await fillText("Age", "12");
const rangeMsg = await bubble();
check("an out-of-range value shows the range error", /greater than or equal to 18/.test(rangeMsg ?? "") && (await field("Age")).value === "", String(rangeMsg));
await shot("range-error");
await fillText("Age", "30");

// ---- app.alert and app.launchURL as dialogs ---------------------------------------------------------------------
await fillText("Name", "Ada");
await until(`!!document.querySelector(".fm-prompt")`, "the alert dialog");
check("app.alert opens an in-app dialog titled Inkwell", /Inkwell/.test((await promptText()) ?? "") && /Welcome, Ada\./.test((await promptText()) ?? ""), String(await promptText()));
await shot("alert-dialog");
await click(".fm-prompt .btn", "OK");
await sleep(200);
check("OK closes it", (await promptText()) === null);
await fillText("Website", "help");
await until(`!!document.querySelector(".fm-prompt")`, "the link dialog");
const linkText = (await promptText()) ?? "";
check("app.launchURL asks first, showing the address, with Copy link and Cancel", /https:\/\/example\.com\/help/.test(linkText) && /Copy link/.test(linkText) && /Cancel/.test(linkText), linkText);
await shot("link-dialog");
await click(".fm-prompt .btn", "Cancel");
await sleep(200);
check("Cancel closes it and nothing was opened", (await promptText()) === null);

// ---- custom format + keystroke (a field the sample already has) ---------------------------------------------------
await fillText("Code", "ab1");
const code = await field("Code");
check("a custom keystroke script upper-cases and the format script decorates", code.value === "AB1" && code.shown === "#AB1", `${code.value} / ${code.shown}`);
await shot("filled");

// ---- save and reopen ---------------------------------------------------------------------------------------------
await ev(`(() => { document.activeElement?.blur?.(); return true })()`);
await key("s", "KeyS", 83, 2);
try {
  await until(`!${S}.docs[${id}].dirty`, "the save", 12000);
} catch (e) {
  const d = await ev(`(() => { const d = ${S}.docs[${id}]; return JSON.stringify({ dirty: d.dirty, scratch: d.scratch ?? null, path: d.path, active: ${S}.active, focus: document.activeElement?.tagName, text: document.body.innerText.slice(0, 600) }) })()`);
  console.log("save did not finish:", d);
  throw e;
}
const filePath = await ev(`${S}.docs[${id}].path`);
await act(`${S}.closeDoc(${id})`);
await sleep(600);
await act(`${S}.openPath(${JSON.stringify(filePath)})`);
const re = await until(`(() => { const d = Object.values(${S}.docs).find((d) => d.name === "formsjs-sample.pdf"); return d ? d.id : null })()`, "the reopened file");
await until(`(${F}.docs[${re}]?.fields?.length ?? 0) === 12`, "the reopened fields");
const back = Object.fromEntries((await fields(re)).map((f) => [f.name, [f.value, f.shown]]));
check(
  "save and reopen keeps the values (and what they show)",
  back.Price?.[0] === "1234.5" && back.Price?.[1] === "$1,234.50" && back.Born?.[0] === "18/04/1991" && back.Total?.[0] === "1250.5" && back.Code?.[1] === "#AB1" && back.Name?.[0] === "Ada",
  JSON.stringify(back),
);
await act(`${S}.activate(${re})`);
await act(`${S}.patch(${re}, { organize: false })`);
await act(`${S}.setZoom(${re}, 1)`);
await act(`${S}.goTo(${re}, 0)`);
await sleep(700);
await act(`${F}.setOpen(true)`);

// ---- the banner switch ---------------------------------------------------------------------------------------------
const rent = await field("Rent", re);
await click(".fm-banner input[type=checkbox]");
await until(`${F}.docs[${re}]?.scripts?.enabled === false`, "scripts off");
check("the banner switch turns the file's scripts off", (await scripts(re)).enabled === false && /Scripts are off/.test(await ev(`document.querySelector(".fm-banner")?.innerText ?? ""`)));
const e5 = await ev(`${F}.setValue(${re}, ${rent.page}, ${rent.xref}, "5")`);
const t5 = await field("Total", re);
check("with scripts off, a custom calculation no longer runs (Tax unchanged)", e5 === null && (await field("Tax", re)).value === "312.63", `tax ${(await field("Tax", re)).value}`);
await shot("scripts-off");
await click(".fm-banner input[type=checkbox]");
await until(`${F}.docs[${re}]?.scripts?.enabled === true`, "scripts on");
await act(`${F}.setValue(${re}, ${rent.page}, ${rent.xref}, "1000")`);
check("turned back on, the calculation runs again", (await field("Tax", re)).value === "312.63" && (await field("Total", re)).value === "1250.5", `${(await field("Total", re)).value}`);

// ---- authoring: Format tab with live sample, Custom script boxes, Calculation order --------------------------------
await act(`${F}.setAuthoring(true)`);
await until(`!!document.querySelector(".fm-strip")`, "Prepare form");
const select = async (name) => {
  const f = await field(name, re);
  await ev(`${F}.select(${re}, { page: ${f.page}, xref: ${f.xref} })`);
  await until(`!!document.querySelector(".fm-props[aria-label='Field properties']")`, `properties of ${name}`);
  await sleep(350);
};
const scrollTo = (text) =>
  ev(`(() => { const e = [...document.querySelectorAll(".fm-sub")].find((x) => x.textContent.trim() === ${JSON.stringify(text)}); e?.scrollIntoView({ block: "start" }); return !!e })()`);

await select("Price");
await scrollTo("Format");
await sleep(500);
const sample = await ev(`document.querySelector(".fm-sample")?.innerText ?? null`);
check('the Format tab previews "1234.5 → $1,234.50"', /1234\.5\s*→\s*\$1,234\.50/.test(sample ?? ""), String(sample));
await shot("format-tab");

await select("Code");
await scrollTo("Format");
await sleep(400);
const boxes = await ev(`[...document.querySelectorAll("textarea.fm-code")].map((t) => t.value)`);
check("the Custom script boxes show the format and keystroke scripts", boxes.length === 2 && /event\.value/.test(boxes[0]) && /toUpperCase/.test(boxes[1]), JSON.stringify(boxes));
await shot("custom-script");

// Author a custom format on "Other" through the UI: one undo step
await select("Other");
const labelBefore = await ev(`${S}.docs[${re}].undoLabel`);
await setControl("select[aria-label='Show the value as']", "custom");
await until(`document.querySelectorAll("textarea.fm-code").length === 2`, "the script boxes");
await setControl("textarea[aria-label='Custom format script']", 'if (event.value !== "") { event.value = "<" + event.value + ">"; }');
await sleep(500);
const other = await field("Other", re);
check("Custom script writes a custom format (one edit)", other.format.kind === "custom" && /<" \+ event\.value/.test(other.format.value.format), JSON.stringify(other.format));
await act(`${F}.setValue(${re}, ${other.page}, ${other.xref}, "7")`);
check("and it runs", (await field("Other", re)).shown === "<7>", (await field("Other", re)).shown);
void labelBefore;

// Calculation order
await select("Total");
await scrollTo("Calculate");
const before = await calcOrder(re);
const calcBtn = await click(".fm-props .fm-btn", "Calculation order…");
check("Calculate offers Calculation order…", calcBtn);
await until(`!!document.querySelector(".fm-order-dlg")`, "the calculation order dialog");
const rowsBefore = await ev(`[...document.querySelectorAll(".fm-order-list .name")].map((e) => e.textContent)`);
check("the dialog lists the calculated fields in order", rowsBefore.join() === "Total,Tax,Spin", rowsBefore.join());
await shot("calc-order");
await click(".fm-order-list li:first-child button[aria-label^='Move'][aria-label$='later']");
await sleep(200);
await click(".fm-order-dlg .btn.primary", "OK");
await sleep(600);
const after = await calcOrder(re);
const names = (await fields(re)).filter((f) => after.includes(f.node)).sort((a, b) => after.indexOf(a.node) - after.indexOf(b.node)).map((f) => f.name);
check("OK writes the new order", after.join() !== before.join() && names.join() === "Tax,Total,Spin", names.join());
await undo();
await sleep(500);
check("one undo restores it", (await calcOrder(re)).join() === before.join(), (await calcOrder(re)).join());

await send("Emulation.setFocusEmulationEnabled", { enabled: false });
const failed = results.filter((r) => !r.ok);
console.log(`\n${results.length - failed.length} of ${results.length} checks passed`);
if (failed.length) console.log("FAILED:", failed.map((f) => f.what).join("; "));
ws.close();
process.exit(failed.length ? 1 : 0);
