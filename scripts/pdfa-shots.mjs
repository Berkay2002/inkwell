// Drives a running Inkwell through WebView2's DevTools endpoint: Preflight on copies of docs/study samples (never
// the originals): Tools > Preflight, the outcome card and its live line, convert in this document (one undo step),
// save, reopen, re-run shows conformance, the claim badge and Verify, the other profiles, and a 1200-page document
// for progress, cancel and responsiveness. The save dialog is native and cannot be driven without OS-level input,
// so "Save as new file" is covered by the Rust tests and its result card is put into the store for the screen.
// Screens (light and dark) go to docs/screens/pdfa.
//   $d = "$env:TEMP\inkwell-pdfa"; mkdir $d; copy docs\study\samples\{compress-source,form-source,redact-source}.pdf $d
//   $env:INKWELL_SEED_DIR = $d; cargo test --lib seed_compare_screens -- --ignored      (from src-tauri; writes "Big v1.pdf")
//   npx vite --port 1420                                   (the debug exe loads the dev server)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:INKWELL_HISTORY_ROOT = "$d\store"
//   $env:WEBVIEW2_USER_DATA_FOLDER = "$d\wv2"
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9227'
//   src-tauri\target\debug\inkwell.exe "$d\compress-source.pdf" "$d\form-source.pdf" "$d\redact-source.pdf" "$d\Big v1.pdf"
//   node scripts/pdfa-shots.mjs docs/screens/pdfa 9227       (PDFA_REPORT=<file> also writes the JSON report there)
//   The app converts a 1200-page document in well under a second here, so the live progress bar is caught with a
//   poll and the cancel is tried at once; the 412-of-1200 check screen is put into the store.
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/pdfa";
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
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }).catch((e) => {
    throw new Error(`${e.message} in: ${expression.slice(0, 120)}`);
  });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? "eval failed");
  return r.result.value;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const until = async (expr, what, limit = 30000, every = 100) => {
  const t0 = Date.now();
  for (;;) {
    const v = await ev(expr);
    if (v) return v;
    if (Date.now() - t0 > limit) throw new Error(`timed out waiting for ${what}`);
    await sleep(every);
  }
};
const theme = (t) => ev(`window.__S.getState().setTheme(${JSON.stringify(t)})`);
const shot = async (name, themes = ["light", "dark"]) => {
  await sleep(700);
  for (const t of themes) {
    await theme(t);
    await sleep(350);
    const r = await send("Page.captureScreenshot", { format: "png" });
    fs.writeFileSync(path.join(out, `${name}-${t}.png`), Buffer.from(r.data, "base64"));
    console.log("screenshot", `${name}-${t}.png`);
  }
  await theme("light");
};
const click = (sel, text) =>
  ev(
    `(() => { const els = [...document.querySelectorAll(${JSON.stringify(sel)})]; const b = ${
      text ? `els.find((e) => e.textContent.includes(${JSON.stringify(text)}))` : "els[0]"
    }; b?.click(); return !!b })()`,
  );
const text = (sel) => ev(`document.querySelector(${JSON.stringify(sel)})?.innerText.replace(/\\s+/g, " ") ?? null`);
const report = {};

await ev(`(() => { window.__S = window.__inkwell.useStore; window.__P = window.__inkwell.usePreflight; return true })()`);
await until(`Object.keys(window.__S.getState().docs).length >= 3`, "the sample documents", 60000);
const dir = await ev(`window.__S.getState().docs[window.__S.getState().order[0]].path.replace(/[\\/][^\\/]*$/, "")`);
if (!(await ev(`Object.values(window.__S.getState().docs).some((d) => d.name === "Big v1.pdf")`))) {
  await ev(`window.__S.getState().openPath(${JSON.stringify(dir + "/Big v1.pdf")})`);
}
await until(`Object.keys(window.__S.getState().docs).length >= 4`, "four documents", 60000);
const docByName = (n) => ev(`(() => { const d = Object.values(window.__S.getState().docs).find((d) => d.name === ${JSON.stringify(n)}); return d ? { id: d.id, path: d.path } : null })()`);
const activate = (id) => ev(`window.__S.getState().activate(${id})`);
const pf = (id) =>
  ev(`(() => { const d = window.__P.getState().docs[${id}]; const r = d?.report; return d ? { phase: d.phase, error: d.error, rev: d.checkedRev, profile: r?.profile ?? null, conforms: r?.conforms ?? null, claimed: r?.claimed ?? null, plan: r ? { total: r.plan.total, groups: r.plan.groups.map((g) => [g.id, g.issues, g.newFileIssues, g.defaultOn]), remaining: r.plan.remaining } : null, results: r?.results.map((x) => [x.rule, x.status, x.count]) ?? null } : null })()`);
const waitChecked = (id, what = "the report") =>
  until(`(() => { const d = window.__P.getState().docs[${id}]; const s = window.__S.getState().docs[${id}]; return d && d.report && d.phase === "idle" && d.checkedRev === s.rev && d.checkedProfile === window.__P.getState().profile })()`, what, 90000, 100);
const undoLabel = (id) => ev(`window.__S.getState().docs[${id}].undoLabel`);
const rev = (id) => ev(`window.__S.getState().docs[${id}].rev`);
const clickUndo = () => ev(`(document.activeElement?.blur?.(), document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "z", ctrlKey: true, bubbles: true })), true)`);
const setProfile = (p) => ev(`window.__P.getState().setProfile(${JSON.stringify(p)})`);
const menu = async (label) => {
  await until(`!!document.querySelector(".ax-toggle")`, "the Tools button");
  await click(".ax-toggle", "Tools");
  await until(`[...document.querySelectorAll("[role=menuitem], .menu button, button")].some((b) => b.textContent.includes(${JSON.stringify(label)}))`, `the ${label} menu item`);
  return click("button", label);
};

await theme("light");
const comp = await docByName("compress-source.pdf");
const form = await docByName("form-source.pdf");
const big = await docByName("Big v1.pdf");

// ---- open from the Tools menu, PDF/A-2b ----
await activate(comp.id);
await setProfile("pdfa-2b");
report.menuItem = await menu("Preflight");
await until(`!!document.querySelector(".pf-panel")`, "the rail");
await waitChecked(comp.id, "the first report");
report.first = await pf(comp.id);
report.verdict = await text(".a11y-summary");
await shot("pdfa-report");

// ---- the outcome card: fix groups as a checklist, the line follows the ticks ----
await until(`!!document.querySelector(".pf-outcome")`, "the outcome card", 10000);
report.outcome = { title: await text(".pf-outcome > b"), line: await text(".pf-line"), checks: await ev(`[...document.querySelectorAll(".pf-check")].map((c) => ({ name: c.querySelector(".pf-check-name").textContent, n: c.querySelector(".pf-check-n").textContent, on: c.querySelector("input").checked, disabled: c.querySelector("input").disabled }))`) };
await ev(`document.querySelector(".pf-outcome")?.scrollIntoView({ block: "center" })`);
await shot("pdfa-outcome");
const firstOn = await ev(`(() => { const c = [...document.querySelectorAll(".pf-check")].find((c) => c.querySelector("input").checked && !c.querySelector("input").disabled && !c.querySelector(".pf-check-n").textContent.startsWith("nothing")); return c ? c.querySelector(".pf-check-name").textContent : null })()`);
if (firstOn) {
  await ev(`(() => { const c = [...document.querySelectorAll(".pf-check")].find((c) => c.querySelector(".pf-check-name").textContent === ${JSON.stringify(firstOn)}); c.querySelector("input").click(); return true })()`);
  await sleep(150);
  report.untick = { group: firstOn, line: await text(".pf-line") };
  await shot("pdfa-unticked", ["light"]);
  await ev(`(() => { const c = [...document.querySelectorAll(".pf-check")].find((c) => c.querySelector(".pf-check-name").textContent === ${JSON.stringify(firstOn)}); c.querySelector("input").click(); return true })()`);
  await sleep(150);
  report.reticked = await text(".pf-line");
}

// ---- Show on a finding ----
if (await ev(`!!document.querySelector(".a11y-finding-main:not([disabled])")`)) {
  await click(".a11y-finding-main:not([disabled])");
  report.showMark = await until(`!!document.querySelector(".a11y-mark")`, "the highlight on the page", 8000).catch(() => false);
  await shot("pdfa-show", ["light"]);
}

// ---- convert in this document: one undo step ----
const undoBefore = await undoLabel(comp.id);
const revBefore = await rev(comp.id);
await click(".a11y-seg-pick button", "This document");
await sleep(150);
report.inPlaceLine = await text(".pf-line");
await click(".pf-outcome .a11y-btn.primary");
await until(`window.__S.getState().docs[${comp.id}].rev > ${revBefore}`, "the conversion", 90000);
await waitChecked(comp.id, "the report after converting");
report.converted = { undo: await undoLabel(comp.id), result: await pf(comp.id), verdict: await text(".a11y-summary") };
await shot("pdfa-converted");

// one undo takes all of it back
const revConv = await rev(comp.id);
await clickUndo();
await until(`window.__S.getState().docs[${comp.id}].rev !== ${revConv}`, "undo of the conversion");
await waitChecked(comp.id, "the report after undo");
report.undone = { label: await undoLabel(comp.id), expectedBack: undoBefore, conforms: (await pf(comp.id)).conforms };

// convert again, save, reopen, re-run
await ev(`window.__P.getState().setOutput(${comp.id}, "inPlace")`);
await until(`!!document.querySelector(".pf-outcome .a11y-btn.primary:not([disabled])")`, "the Convert button");
const revAgain = await rev(comp.id);
await click(".pf-outcome .a11y-btn.primary");
await until(`window.__S.getState().docs[${comp.id}].rev > ${revAgain}`, "the second conversion", 90000);
await waitChecked(comp.id, "the report after converting again");
const afterConvert = await pf(comp.id);
await ev(`window.__TAURI_INTERNALS__.invoke("save_document", { doc: ${comp.id}, to: null })`);
await ev(`window.__S.getState().closeDoc(${comp.id})`);
await sleep(500);
await ev(`window.__S.getState().openPath(${JSON.stringify(comp.path)})`);
const reopened = await until(`(() => { const d = Object.values(window.__S.getState().docs).find((d) => d.name === "compress-source.pdf"); return d ? d.id : null })()`, "the reopened file", 20000);
await activate(reopened);
await ev(`window.__P.getState().setOpen(true)`);
await ev(`window.__P.getState().check(${reopened})`);
await waitChecked(reopened, "the report of the reopened file");
const again = await pf(reopened);
report.reopened = { conforms: again.conforms, claimed: again.claimed, sameResults: JSON.stringify(again.results.map((r) => r.slice(0, 2))) === JSON.stringify(afterConvert.results.map((r) => r.slice(0, 2))), failed: again.results.filter((r) => r[1] === "failed") };
await shot("pdfa-reopened");

// ---- the claim badge in Document properties and Verify ----
await ev(`window.__P.getState().setOpen(false)`);
await menu("Security");
await until(`!!document.querySelector(".pf-claim")`, "the claim badge", 15000).catch(() => {});
report.badge = await text(".pf-claim");
await shot("pdfa-badge");
if (await click(".pf-claim-verify")) {
  await until(`!!document.querySelector(".pf-panel")`, "the rail from Verify", 10000);
  await waitChecked(reopened, "the verify report");
  report.verify = { profile: await ev(`window.__P.getState().profile`), conforms: (await pf(reopened)).conforms };
}

// ---- the other profiles on the form ----
await activate(form.id);
await ev(`window.__P.getState().setOpen(true)`);
for (const p of ["pdfa-1b", "pdfa-3b", "pdfa-2u", "pdfua-1", "pdfx-4"]) {
  await setProfile(p);
  await waitChecked(form.id, `the ${p} report`);
  const r = await pf(form.id);
  report[`form-${p}`] = { conforms: r.conforms, failed: r.results.filter((x) => x[1] === "failed").map((x) => `${x[0]}:${x[2]}`), manual: r.results.filter((x) => x[1] === "manual").length, info: r.results.filter((x) => x[1] === "info").length, total: r.plan.total };
  if (p === "pdfua-1" || p === "pdfx-4" || p === "pdfa-1b") await shot(`pdfa-${p}`, ["light"]);
}
await setProfile("pdfa-2b");

// ---- the new-file result card (the save dialog is native: the state it produces is put into the store) ----
await activate(reopened);
await waitChecked(reopened, "the report for the card");
await ev(`(() => { const s = window.__P.getState(); const d = s.docs[${reopened}]; window.__P.setState({ docs: { ...s.docs, [${reopened}]: { ...d, converted: { path: ${JSON.stringify(dir)} + "/compress-source (PDF-A-2b).pdf", report: d.report, fixed: ["Added an sRGB output intent", "Embedded Helvetica", "Wrote the XMP metadata"] } } } }); return true })()`);
await until(`!!document.querySelector(".pf-done")`, "the result card");
await shot("pdfa-new-file");

// ---- 1200 pages: progress, cancel, and the page keeps painting ----
await activate(big.id);
await ev(`window.__P.getState().setOpen(true)`);
await ev(`(() => { window.__gap = 0; let last = performance.now(); const tick = (t) => { window.__gap = Math.max(window.__gap, t - last); last = t; requestAnimationFrame(tick) }; requestAnimationFrame(tick); return true })()`);
await sleep(300);
await waitChecked(big.id, "the 1200-page report");
await ev(`window.__gap = 0`);
await ev(`(() => { window.__seen = []; const id = ${big.id}; window.__unsub = window.__P.subscribe((s) => { const d = s.docs[id]; const last = window.__seen[window.__seen.length - 1]; if (!last || last[0] !== d?.phase) window.__seen.push([d?.phase, d?.progress ? d.progress.stage + " " + d.progress.done + "/" + d.progress.pages : null]); }); return true })()`);
const tBig = Date.now();
await ev(`(() => { void window.__P.getState().check(${big.id}); return true })()`);
await waitChecked(big.id, "the 1200-page report, run again");
report.bigCheck = { ms: Date.now() - tBig, pages: await ev(`window.__S.getState().docs[${big.id}].pageCount`), maxFrameGapMs: Math.round(await ev(`window.__gap`)), rules: (await pf(big.id)).results.length, phases: await ev(`window.__seen`) };

// the progress bar itself: a check this size is quicker than a poll, so the screen is put into the store at 412 of 1200
await ev(`(() => { const s = window.__P.getState(); const d = s.docs[${big.id}]; window.__P.setState({ docs: { ...s.docs, [${big.id}]: { ...d, phase: "checking", job: 0, progress: { stage: "check", done: 412, pages: 1200 } } } }); return true })()`);
await shot("pdfa-checking", ["light"]);
await ev(`(() => { const s = window.__P.getState(); const d = s.docs[${big.id}]; window.__P.setState({ docs: { ...s.docs, [${big.id}]: { ...d, phase: "idle", progress: null } } }); return true })()`);

// cancel a conversion at once: the rail is idle and the document untouched
await ev(`window.__P.getState().setOutput(${big.id}, "inPlace")`);
await until(`!!document.querySelector(".pf-outcome .a11y-btn.primary:not([disabled])")`, "the Convert button (big)");
const revBig = await rev(big.id);
await ev(`(() => { void window.__P.getState().convert(${big.id}); return true })()`);
await sleep(25);
const tCancel = Date.now();
await ev(`window.__P.getState().cancel(${big.id})`);
await until(`window.__P.getState().docs[${big.id}].phase === "idle"`, "the conversion to stop", 60000, 20);
await sleep(400);
report.cancel = { ms: Date.now() - tCancel, revSame: (await rev(big.id)) === revBig, error: (await pf(big.id)).error, undo: await undoLabel(big.id) };
if (!report.cancel.revSame) {
  // 1200 pages convert faster than a cancel can land: take that one step back so the full run below starts clean
  await clickUndo();
  await until(`window.__S.getState().docs[${big.id}].rev === ${revBig} || window.__S.getState().docs[${big.id}].undoLabel !== ${JSON.stringify(report.cancel.undo)}`, "undo of the finished conversion");
  report.cancel.finishedBeforeCancel = true;
}

// the whole conversion in this document: progress shown, frames keep coming, one undo step
await waitChecked(big.id, "the report before converting").catch(() => {});
const revB2 = await rev(big.id);
await ev(`window.__gap = 0`);
await ev(`window.__seen.length = 0`);
const tConv = Date.now();
await ev(`(() => { void window.__P.getState().convert(${big.id}); return true })()`);
const live = await until(`!!document.querySelector(".a11y-run")`, "convert progress", 15000, 5).catch(() => false);
if (live) {
  report.convertProgress = await text(".a11y-run");
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, "pdfa-converting-light.png"), Buffer.from(r.data, "base64"));
  console.log("screenshot pdfa-converting-light.png");
}
await until(`window.__S.getState().docs[${big.id}].rev > ${revB2}`, "the big conversion", 180000, 50);
await waitChecked(big.id, "the report after the big conversion");
const bigAfter = await pf(big.id);
report.bigConvert = { ms: Date.now() - tConv, maxFrameGapMs: Math.round(await ev(`window.__gap`)), phases: await ev(`window.__seen`), progressSeen: !!live, undo: await undoLabel(big.id), conforms: bigAfter.conforms, failed: bigAfter.results.filter((r) => r[1] === "failed") };
await ev(`window.__unsub()`);

await ev(`window.__P.getState().setOpen(false)`);
const dump = JSON.stringify(report, null, 2);
if (process.env.PDFA_REPORT) fs.writeFileSync(process.env.PDFA_REPORT, dump);
console.log(dump);
ws.close();
