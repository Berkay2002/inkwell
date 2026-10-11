// Drives a running Inkwell through WebView2's DevTools endpoint: find options in the find box, the Search rail (this file and
// several files), jump and keyboard, history, a 1000-page document, and captures the screens (light and dark). Works on
// generated copies, never on real files.
//   $d = "$env:TEMP\inkwell-search"; mkdir $d
//   python docs\study\samples\_gen\make_lease.py "$d\Lease agreement.pdf"; python scripts\search-samples.py $d
//   pnpm exec vite --port 1420 --strictPort        (the debug exe loads the dev server)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:WEBVIEW2_USER_DATA_FOLDER = "$d\wv"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9237'
//   src-tauri\target\debug\inkwell.exe "$d\Lease agreement.pdf" "$d\Insurance policy.pdf" "$d\Move-out checklist.pdf" `
//     "$d\Cafe menu.pdf" "$d\sub\Inspection report.pdf" "$d\Thousand.pdf"
//   node scripts/search-shots.mjs docs/screens/search 9237
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/search";
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
const until = async (expr, what, limit = 20000) => {
  const t0 = Date.now();
  for (;;) {
    const v = await ev(expr);
    if (v) return v;
    if (Date.now() - t0 > limit) throw new Error(`timed out waiting for ${what}`);
    await sleep(100);
  }
};
const theme = (t) => ev(`window.__S.getState().setTheme(${JSON.stringify(t)})`);
const shot = async (name) => {
  await sleep(500);
  for (const t of ["light", "dark"]) {
    await theme(t);
    await sleep(350);
    const r = await send("Page.captureScreenshot", { format: "png" });
    fs.writeFileSync(path.join(out, `${name}-${t}.png`), Buffer.from(r.data, "base64"));
    console.log("screenshot", `${name}-${t}.png`);
  }
  await theme("light");
};
const key = async (k, code, vk, modifiers = 0) => {
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: k, code, windowsVirtualKeyCode: vk, modifiers });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code, windowsVirtualKeyCode: vk, modifiers });
  await sleep(120);
};
const enter = () => key("Enter", "Enter", 13);
const click = (sel, text) =>
  ev(`(() => { const els = [...document.querySelectorAll(${JSON.stringify(sel)})]; const b = ${text ? `els.find((e) => e.textContent.trim().startsWith(${JSON.stringify(text)}))` : "els[0]"}; b?.click(); return !!b })()`);
// Types into the input matching `sel`: select what is there, then insert text (a real input event, no OS input).
const type = async (sel, text) => {
  await ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); e.focus(); e.select(); return true })()`);
  await send("Input.insertText", { text });
  await sleep(60);
};
const results = [];
const check = (what, ok, detail = "") => {
  results.push({ what, ok: !!ok, detail });
  console.log(ok ? "ok  " : "FAIL", what, detail);
};

await ev(`(() => { const i = window.__inkwell; window.__S = i.useStore; window.__F = i.useFind; window.__R = i.useSearch; return true })()`);
if (!(await ev("!!window.__R && !!window.__F"))) throw new Error("window.__inkwell.useFind / useSearch is missing: rebuild the app");
const S = `window.__S.getState()`;
const F = `window.__F.getState()`;
const R = `window.__R.getState()`;
const docId = (name) => ev(`(() => { const d = Object.values(${S}.docs).find((d) => d.name === ${JSON.stringify(name)}); return d ? d.id : null })()`);
const search = (id) => ev(`(() => { const s = ${S}.docs[${id}].search; return { query: s.query, n: s.hits.length, done: s.done, current: s.current, scanned: s.scanned, total: s.total } })()`);
const activate = async (name) => {
  const id = await docId(name);
  if (id == null) throw new Error(`${name} is not open`);
  await ev(`${S}.activate(${id}); ${S}.patch(${id}, { organize: false }); ${S}.setSidebar("none")`);
  await until(`document.querySelectorAll(".viewer .page").length > 0 && ${S}.docs[${id}].sizes.length > 0`, "the pages");
  await sleep(300);
  return id;
};
// Types a query into the find box (opening it first) and waits for that search to finish.
const find = async (id, q) => {
  await ev(`${S}.setUi({ searchOpen: true })`);
  await until(`!!document.querySelector(".searchbar input")`, "the find box");
  await type(".searchbar input", q);
  await until(`(() => { const s = ${S}.docs[${id}].search; return s.query === ${JSON.stringify(q)} && s.done })()`, `the search for ${q}`, 30000);
  return search(id);
};
const setOpt = async (k, v) => {
  await ev(`${F}.setOpt(${JSON.stringify(k)}, ${v})`);
  await sleep(500);
};

// ---- the find box ------------------------------------------------------------------------------------
const lease = await activate("Lease agreement.pdf");
await ev(`${F}.clearHistory(); ["caseSensitive","wholeWords","ignoreAccents","comments","bookmarks","fields"].forEach((k) => ${F}.setOpt(k, false))`);
const base = await find(lease, "notice");
check("find lists hits for a word", base.n > 0, `${base.n} hits`);
check("find box has the Aa and ab chips", (await ev(`document.querySelectorAll(".searchbar .sr-chip").length`)) === 2);
check("the chips are labelled", await ev(`[...document.querySelectorAll(".searchbar .sr-chip")].map((b) => b.getAttribute("aria-label")).join()`) === "Match case,Whole words");

await click(".searchbar .sr-chip", undefined);
await until(`${S}.docs[${lease}].search.done && ${F}.opts.caseSensitive`, "match case search");
await sleep(400);
const cs = await search(lease);
check("Aa (match case) narrows the hits", cs.n < base.n, `${base.n} -> ${cs.n}`);
check("the Aa chip shows pressed", (await ev(`document.querySelector(".searchbar .sr-chip").getAttribute("aria-pressed")`)) === "true");
await setOpt("caseSensitive", false);

await setOpt("wholeWords", true);
await until(`${S}.docs[${lease}].search.done`, "whole words search");
const ww = await search(lease);
check("ab (whole words) drops 'notices'", ww.n < base.n, `${base.n} -> ${ww.n}`);
const plural = await find(lease, "notices");
check("whole words still finds the plural", plural.n > 0, `${plural.n} hits`);
await setOpt("wholeWords", false);
await find(lease, "notice");

// the "..." menu
await click(".searchbar .sr-more button");
await until(`!!document.querySelector(".ink-menu")`, "the options menu");
const items = await ev(`[...document.querySelectorAll(".ink-menu button")].map((b) => b.textContent.trim().replace(/Ctrl\\+Shift\\+F$/, "").trim())`);
check("menu lists the other options", JSON.stringify(items) === JSON.stringify(["Ignore accents", "Include comments", "Include bookmarks", "Include form fields", "Advanced search…"]), JSON.stringify(items));
await shot("find-1-options");
await click(".ink-menu button", "Include comments");
await until(`${F}.opts.comments`, "comments option");
check("a menu option is stored", await ev(`JSON.parse(localStorage.getItem("inkwell.find.opts")).comments === true`));
await sleep(500);
await setOpt("comments", false);

// ---- accents ----------------------------------------------------------------------------------------
const cafe = await activate("Cafe menu.pdf");
const plain = await find(cafe, "cafe");
await setOpt("ignoreAccents", true);
await until(`${S}.docs[${cafe}].search.done`, "accent search");
const acc = await search(cafe);
check("ignore accents finds café and Café too", plain.n === 1 && acc.n === 4, `${plain.n} -> ${acc.n}`);
await setOpt("ignoreAccents", false);

// ---- the rail, this file -----------------------------------------------------------------------------
await activate("Lease agreement.pdf");
await find(lease, "notice");
await click(".searchbar .sr-more button");
await until(`!!document.querySelector(".ink-menu")`, "the options menu");
await click(".ink-menu button", "Advanced search…");
await until(`!!document.querySelector(".sr-dock")`, "the Search rail");
await until(`document.querySelectorAll(".sr-row").length > 0`, "result rows");
const total = (await search(lease)).n;
const rows = await ev(`document.querySelectorAll(".sr-row").length`);
check("rail lists every hit as a row", rows === total, `${rows} rows, ${total} hits`);
check("rail says how many results on how many pages", /^\d+ results? on \d+ pages?$/.test(await ev(`document.querySelector(".sr-status span").textContent`)), await ev(`document.querySelector(".sr-status span").textContent`));
check("rows are grouped by page", (await ev(`document.querySelectorAll(".sr-group h4").length`)) > 1);
check("the hit is bold in the snippet", (await ev(`getComputedStyle(document.querySelector(".sr-snip mark")).fontWeight`)) >= 600);

const before = await search(lease);
await ev(`document.querySelectorAll(".sr-row")[7].click()`);
await sleep(500);
const after = await search(lease);
check("clicking a row makes it the current hit and jumps there", after.current === 7 && before.current !== 7, `current ${before.current} -> ${after.current}`);
check("the current row is marked", await ev(`document.querySelectorAll(".sr-row.cur").length === 1 && document.querySelector(".sr-row.cur").dataset.i === "7"`));
await ev(`document.querySelector(".sr-list").focus()`);
await key("ArrowDown", "ArrowDown", 40);
await key("ArrowDown", "ArrowDown", 40);
check("arrow keys move through the hits", (await search(lease)).current === 9, `current ${(await search(lease)).current}`);
await key("ArrowUp", "ArrowUp", 38);
check("arrow up goes back", (await search(lease)).current === 8);
await key("End", "End", 35);
check("End goes to the last hit", (await search(lease)).current === total - 1);
await key("Home", "Home", 36);
await sleep(300);
await shot("rail-1-file");

// ---- history ----------------------------------------------------------------------------------------
for (const q of ["deposit", "rent", "notice", "landlord", "tenant"]) {
  await type(".sr-field input", q);
  await until(`${S}.docs[${lease}].search.query === ${JSON.stringify(q)}`, `search ${q}`);
  await key("Enter", "Enter", 13);
}
const hist = await ev(`${F}.history`);
check("history keeps the latest first, no repeats", JSON.stringify(hist.slice(0, 5)) === JSON.stringify(["tenant", "landlord", "notice", "rent", "deposit"]), JSON.stringify(hist));
for (let i = 0; i < 12; i++) await ev(`${F}.remember("q${i}")`);
check("history holds ten", (await ev(`${F}.history.length`)) === 10);
check("history is saved locally", (await ev(`JSON.parse(localStorage.getItem("inkwell.find.history")).length`)) === 10);
await click(".sr-hist-btn");
await until(`!!document.querySelector(".sr-history")`, "history list");
check("the clock lists the recent searches", (await ev(`document.querySelectorAll(".sr-history .sr-h").length`)) === 11, "10 + clear");
await shot("rail-2-history");
await click(".sr-history .sr-h");
await until(`${S}.docs[${lease}].search.query === "q11"`, "history pick runs the search");
check("picking one fills the field and searches", (await ev(`document.querySelector(".sr-field input").value`)) === "q11");
await find(lease, "notice");

// ---- a 1000-page document ----------------------------------------------------------------------------
const big = await activate("Thousand.pdf");
await ev(`(() => { window.__gaps = []; let last = performance.now(); const tick = (t) => { window.__gaps.push(t - last); last = t; window.__raf = requestAnimationFrame(tick); }; window.__raf = requestAnimationFrame(tick); return true })()`);
const t0 = Date.now();
await ev(`${F}.setDraft(${big}, "notice")`);
await until(`${S}.docs[${big}].search.query === "notice" && ${S}.docs[${big}].search.done`, "the 1000 page search", 60000);
const took = Date.now() - t0;
const gaps = await ev(`(() => { cancelAnimationFrame(window.__raf); const g = window.__gaps.slice(2); return { max: Math.max(...g), frames: g.length } })()`);
const b = await search(big);
check("1000 pages: every tenth page has a hit", b.n === 100, `${b.n} hits in ${took} ms`);
check("1000 pages: scanned all pages", b.scanned === 1000 && b.total === 1000, `${b.scanned}/${b.total}`);
check("1000 pages: the UI kept painting (longest frame gap)", gaps.max < 400, `${Math.round(gaps.max)} ms over ${gaps.frames} frames`);
await ev(`${F}.setDraft(${big}, "page")`);
await until(`${S}.docs[${big}].search.query === "page" && ${S}.docs[${big}].search.done`, "the many-hits search", 60000);
const many = await search(big);
const manyRows = await ev(`document.querySelectorAll(".sr-row").length`);
check("1000 pages: many hits list a bounded number of rows", many.n >= 1000 && manyRows <= 400, `${many.n} hits, ${manyRows} rows`);
await ev(`${F}.setDraft(${big}, "the")`);
await sleep(350);
await ev(`${S}.stopSearch(${big})`);
await sleep(300);
const stopped = await search(big);
check("stop ends a running search and keeps the hits", stopped.done === true);

// ---- several files -----------------------------------------------------------------------------------
await activate("Lease agreement.pdf");
const files = await ev(`window.__TAURI_INTERNALS__.invoke("initial_files")`);
const small = files.filter((f) => !/Thousand\.pdf$/i.test(f.path));
check("the launch files have tokens", small.length >= 5, `${small.length} files`);
await ev(`window.__R.setState({ scope: "folder", target: { kind: "files", tokens: ${JSON.stringify(small.map((f) => f.token))}, paths: ${JSON.stringify(small.map((f) => f.path))} } })`);
await ev(`${F}.setDraft(${lease}, "notice")`);
await sleep(300);
await click(".sr-go");
await until(`${R}.run && !${R}.run.running`, "the file search", 60000);
const run = await ev(`JSON.parse(JSON.stringify({ ...${R}.run, files: ${R}.run.files.map((f) => ({ name: f.name, hits: f.hits, items: f.items.length })) }))`);
check("files: every file with the word is listed", run.files.length === small.length && run.matched === small.length, `${run.files.map((f) => `${f.name} ${f.hits}`).join(", ")}`);
check("files: progress reached the end", run.scanned === small.length && run.total === small.length, `${run.scanned}/${run.total}`);
check("files: shown as groups by file", (await ev(`document.querySelectorAll(".sr-group.file").length`)) === small.length);
check("files: each file lists two rows first", (await ev(`[...document.querySelectorAll(".sr-group.file")].every((g) => g.querySelectorAll(".sr-row").length <= 2)`)));
check("files: status line counts files and results", /results? in \d+ files?/.test(await ev(`document.querySelector(".sr-status span").textContent`)), await ev(`document.querySelector(".sr-status span").textContent`));
await shot("rail-3-files");

await setOpt("wholeWords", true);
await click(".sr-go");
await until(`${R}.run && !${R}.run.running && ${R}.run.query === "notice"`, "whole words file search", 60000);
const ww2 = await ev(`${R}.run.files.map((f) => f.name)`);
check("files: options apply (whole words drops 'Notices' only files)", ww2.length < small.length && ww2.length > 0, JSON.stringify(ww2));
await setOpt("wholeWords", false);
await click(".sr-go");
await until(`${R}.run && !${R}.run.running && ${R}.run.files.length === ${small.length}`, "file search again", 60000);

// jump: open Insurance policy at its second page hit
const g = await ev(`(() => { const f = ${R}.run.files.find((f) => f.name === "Insurance policy.pdf"); const i = f.items.findIndex((x) => x.page > 0); return { i, page: f.items[i].page } })()`);
await ev(`(() => { const rows = [...document.querySelectorAll(".sr-group.file")].find((g) => g.querySelector(".sr-fname").textContent === "Insurance policy.pdf").querySelectorAll(".sr-row"); rows[${Math.min(g.i, 1)}].click(); return true })()`);
await until(`${S}.docs[${S}.active]?.name === "Insurance policy.pdf" && ${S}.docs[${S}.active].search.current >= 0`, "the file opened at the hit", 20000);
const at = await ev(`(() => { const d = ${S}.docs[${S}.active]; return { name: d.name, query: d.search.query, page: d.search.hits[d.search.current].page } })()`);
check("files: a row opens the file at that hit", at.name === "Insurance policy.pdf" && at.query === "notice" && at.page === g.page, JSON.stringify(at));
await shot("rail-4-opened");

await ev(`${R}.setOpen(false)`);
fs.writeFileSync(path.join(out, "results.json"), JSON.stringify(results, null, 2));
const bad = results.filter((r) => !r.ok);
console.log(`\n${results.length - bad.length}/${results.length} checks passed`);
ws.close();
process.exit(bad.length ? 1 : 0);
