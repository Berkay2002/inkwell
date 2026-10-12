// Screens of the Action Wizard without Windows: the frontend in headless Chrome against a mocked invoke layer
// (the batch_* commands answer from fixtures below; nothing touches a disk). Light and dark go to docs/screens/batch.
// What this does NOT cover is the real engine: run scripts/pdfa-shots.mjs-style CDP against the debug exe on the PC for that.
//   pnpm exec vite --port 1431 --strictPort &
//   node scripts/batch-shots.mjs docs/screens/batch [port=1431]
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/batch";
const vitePort = process.argv[3] ?? "1431";
const cdpPort = 9333;
fs.mkdirSync(out, { recursive: true });

const chrome = spawn(
  process.env.CHROME ?? "google-chrome",
  [
    "--headless=new",
    `--remote-debugging-port=${cdpPort}`,
    `--user-data-dir=${fs.mkdtempSync(path.join(os.tmpdir(), "inkwell-shots-"))}`,
    "--window-size=1280,820",
    "--no-first-run",
    "--no-sandbox",
    "about:blank",
  ],
  { stdio: "ignore" },
);
process.on("exit", () => chrome.kill());
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let targets;
for (let i = 0; i < 50 && !targets; i++) {
  targets = await fetch(`http://127.0.0.1:${cdpPort}/json`).then((r) => r.json()).catch(() => undefined);
  if (!targets) await sleep(200);
}
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

// ---- the mocked Tauri layer, installed before the app loads
const mock = `(() => {
  const FILES = [
    { token: "t1", path: "C:\\\\Clients\\\\Acme\\\\Invoice 2026-001.pdf" },
    { token: "t2", path: "C:\\\\Clients\\\\Acme\\\\Contract (signed).pdf" },
    { token: "t3", path: "C:\\\\Clients\\\\Acme\\\\Handbook.pdf" },
    { token: "t4", path: "C:\\\\Clients\\\\Acme\\\\Scan 0042.pdf" },
  ];
  const style = { family: "helvetica", bold: false, italic: false, size: 12, color: [0, 0, 0], underline: false };
  const act = (id, name, description, steps, suffix, builtIn) => ({ id, name, description, steps, output: { kind: "nextToSource", suffix }, onError: "skip", builtIn });
  const wm = { kind: "watermark", text: "CONFIDENTIAL", picture: null, style: { ...style, size: 0, bold: true, color: [0.75, 0.1, 0.1] }, scale: 0.5, opacity: 0.25, rotation: 45, behind: false, offset: [0, 0], showOnScreen: true, showInPrint: true };
  const san = { metadata: true, hiddenLayers: false, hiddenText: false, scriptsActions: true, embeddedFiles: false, annotations: false, formFieldsUnderMarks: false, bookmarks: false };
  const hf = { kind: "headerFooter", text: ["", "", "", "", "Page <<1>> of <<n>>", ""], style, margins: [36, 36, 72, 72], startNumber: 1 };
  let actions = [
    act("builtin-prepare-for-email", "Prepare for email", "Removes metadata and scripts, then makes the file smaller.", [{ kind: "sanitize", ...san }, { kind: "compress", preset: "balanced" }], " (email)", true),
    act("builtin-make-searchable", "Make searchable", "Recognizes the text of scanned pages so they can be searched and copied.", [{ kind: "ocr", languages: [], force: false, backend: "windows" }], " (searchable)", true),
    act("builtin-publish-with-watermark", "Publish with watermark", "Flattens form fields, stamps a diagonal CONFIDENTIAL watermark and compresses.", [{ kind: "flattenForms" }, wm, { kind: "compress", preset: "highQuality" }], " (published)", true),
    act("builtin-redact-emails-and-phones", "Redact emails and phone numbers", "Finds email addresses and phone numbers, removes them for good and checks the result.", [{ kind: "redact", queries: [{ type: "pattern", pattern: "email" }, { type: "pattern", pattern: "phone" }], overlayText: null, sanitize: san }], " (redacted)", true),
    act("weekly-client-pack", "Weekly client pack", "Number the pages, stamp DRAFT and lock it with a password.", [hf, { ...wm, text: "DRAFT", style: { ...style, size: 0, bold: true, color: [0.6, 0.6, 0.6] }, opacity: 0.4 }, { kind: "compress", preset: "balanced" }, { kind: "protect", printing: "high", changes: "none", copy: false, accessibility: true, cipher: "aes256" }], " (client)", false),
    act("archive-text", "Archive text and images", "", [{ kind: "exportText", layout: true }, { kind: "exportImages", format: "png", dpi: 150, gray: false, quality: 90 }], " (archive)", false),
  ];
  const issuesFor = (a) => {
    const out = [];
    if (!a.name.trim()) out.push({ step: null, code: "name", message: "Give the action a name." });
    if (!a.steps.length) out.push({ step: null, code: "no-steps", message: "Add at least one step." });
    a.steps.forEach((s, i) => {
      if (s.kind === "protect" && !s.openPassword && !s.permissionsPassword) out.push({ step: i, code: "invalid-step", message: "Type the password for this step: a stored action does not keep passwords." });
      if (s.kind === "compress" && s.preset === "custom") out.push({ step: i, code: "invalid-step", message: "Choose Smallest, Balanced or High quality." });
    });
    return out;
  };
  const delay = (ms) => new Promise((r) => setTimeout(r, ms));
  const fire = (ch, i, message) => { const id = Number(String(ch.toJSON ? ch.toJSON() : ch).split(":")[1]); window["_" + id]?.({ index: i, message }); };
  window.__MOCK = { files: FILES, run: { delay: 350, hold: false } };
  const handlers = {
    batch_list: () => actions,
    batch_validate: ({ action }) => issuesFor(action),
    batch_save: ({ action }) => { const a = { ...action, id: action.id || "new-" + actions.length, builtIn: false }; actions = actions.filter((x) => x.id !== a.id).concat(a); return a; },
    batch_delete: ({ id }) => { actions = actions.filter((x) => x.id !== id); },
    batch_import: () => { throw "\\"decrypt\\" is not a step this version of Inkwell knows."; },
    batch_export: () => {},
    pick_open: () => FILES,
    pick_source: () => [{ token: "json1", path: "C:\\\\Temp\\\\bad-action.json" }],
    pick_save: ({ name }) => ({ token: "s1", path: "C:\\\\Temp\\\\" + name }),
    pick_folder: () => ({ token: "out1", path: "C:\\\\Clients\\\\Acme\\\\Out" }),
    batch_folder_files: () => ({ files: FILES, capped: false }),
    batch_preview: ({ action, files }) => {
      const rows = files.map((tok, index) => {
        const f = FILES.find((x) => x.token === tok);
        const name = f.path.split("\\\\").pop();
        const stem = name.replace(/\\.pdf$/i, "");
        if (/signed/.test(name) && action.steps.some((s) => s.kind !== "exportText" && s.kind !== "exportImages"))
          return { index, input: f.path, name, status: "skipped", reason: 'signed: "' + (action.steps[0].kind) + '" would invalidate the signature', steps: [], prompts: [], outputs: [] };
        return { index, input: f.path, name, status: "ok", reason: null, steps: action.steps.map((s) => s.kind), prompts: action.steps.flatMap((s, i) => (s.prompt ? [i] : [])), outputs: [stem + action.output.suffix + ".pdf"] };
      });
      return { action: action.name, files: rows, ok: rows.filter((r) => r.status === "ok").length, skipped: rows.filter((r) => r.status === "skipped").length, errors: 0 };
    },
    batch_run: async ({ action, files, onEvent }) => {
      const n = files.length;
      let k = 0;
      const ev = (m) => fire(onEvent, k++, m);
      ev({ event: "started", files: n });
      const reports = [];
      for (let i = 0; i < n; i++) {
        const f = FILES.find((x) => x.token === files[i]);
        const name = f.path.split("\\\\").pop();
        ev({ event: "fileStarted", index: i, total: n, name });
        const signed = /signed/.test(name);
        const bad = /Scan/.test(name);
        for (let s = 0; s < action.steps.length && !signed; s++) {
          ev({ event: "step", index: i, step: s, steps: action.steps.length, label: action.steps[s].kind });
          await delay(window.__MOCK.run.delay);
          if (window.__MOCK.run.hold && i === 1) await new Promise(() => {});
        }
        const stem = name.replace(/\\.pdf$/i, "");
        const r = signed
          ? { index: i, input: f.path, name, status: "skipped", reason: 'signed: "Compress (balanced)" would invalidate the signature', sizeBefore: 220000, sizeAfter: null, outputs: [], notes: [], warnings: [], stepsDone: 0, ms: 3 }
          : bad
            ? { index: i, input: f.path, name, status: "error", reason: "Recognize text: no text engine could read page 1", sizeBefore: 910000, sizeAfter: null, outputs: [], notes: [], warnings: [], stepsDone: 1, ms: 40 }
            : { index: i, input: f.path, name, status: "ok", reason: null, sizeBefore: 1830000, sizeAfter: 412000, outputs: [{ path: f.path.replace(/\\.pdf$/i, "") + action.output.suffix + ".pdf", token: "o" + i }], notes: ["Compress (balanced): 77% smaller"], warnings: [], stepsDone: action.steps.length, ms: 900 };
        reports.push(r);
        ev({ event: "fileDone", index: i, report: r });
      }
      const ok = reports.filter((r) => r.status === "ok").length, skipped = reports.filter((r) => r.status === "skipped").length, errors = reports.filter((r) => r.status === "error").length;
      ev({ event: "finished", ok, skipped, errors, cancelled: false });
      return { action: action.name, files: reports, ok, skipped, errors, cancelled: false, stopped: false };
    },
    batch_cancel: () => {},
    batch_prompt_answer: () => false,
  };
  let cb = 0;
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
    transformCallback: (f, once) => { const id = ++cb; window["_" + id] = (...a) => { if (once) delete window["_" + id]; return f(...a); }; return id; },
    unregisterCallback: (id) => { delete window["_" + id]; },
    convertFileSrc: (p) => p,
    invoke: async (cmd, args) => {
      const h = handlers[cmd];
      if (h) return h(args ?? {});
      return null; // everything the shell asks at start-up: nothing
    },
  };
})();`;
await send("Page.enable");
await send("Page.addScriptToEvaluateOnNewDocument", { source: mock });
await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 820, deviceScaleFactor: 1, mobile: false });
await send("Page.navigate", { url: `http://127.0.0.1:${vitePort}/` });
for (let i = 0; i < 100; i++) {
  if (await ev("!!window.__inkwell")) break;
  await sleep(200);
}
const wiz = () => `window.__inkwell.useBatch.getState()`;
const setTheme = (t) => ev(`window.__inkwell.useStore.getState().setTheme(${JSON.stringify(t)})`);
const shot = async (name) => {
  await sleep(250);
  for (const t of ["light", "dark"]) {
    await setTheme(t);
    await sleep(250);
    const r = await send("Page.captureScreenshot", { format: "png" });
    fs.writeFileSync(path.join(out, `${name}-${t}.png`), Buffer.from(r.data, "base64"));
    console.log("screenshot", `${name}-${t}.png`);
  }
  await setTheme("light");
};
const until = async (expr, what, limit = 8000) => {
  const t0 = Date.now();
  while (!(await ev(`!!(${expr})`))) {
    if (Date.now() - t0 > limit) throw new Error("timed out waiting for " + what);
    await sleep(80);
  }
};
const click = (sel, text) =>
  ev(`(() => { const b = [...document.querySelectorAll(${JSON.stringify(sel)})].find((e) => !${JSON.stringify(text)} || e.textContent.trim().startsWith(${JSON.stringify(text)})); if (!b) throw new Error("no ${sel} " + ${JSON.stringify(text)}); b.click(); return true; })()`);

// 1. the list
await ev(`${wiz()}.show()`);
await until(`document.querySelectorAll(".aw-card").length >= 6`, "the list");
await shot("1-list");

// 2. the editor: Weekly client pack, header & footer selected, then the Protect step
await ev(`${wiz()}.edit(${wiz()}.actions.find((a) => a.id === "weekly-client-pack"))`);
await until(`document.querySelector(".aw-step")`, "the editor");
await ev(`${wiz()}.selectStep(0)`);
await sleep(400);
await shot("2-editor-header-footer");
await ev(`${wiz()}.selectStep(3)`);
await sleep(400);
await shot("3-editor-protect");

// 3. the editor on a redact step with a problem shown
await ev(`${wiz()}.edit(${wiz()}.actions.find((a) => a.id === "builtin-redact-emails-and-phones"))`);
await ev(`${wiz()}.addStep("compress"); ${wiz()}.patchStep(1, { preset: "custom" })`);
await sleep(600);
await ev(`${wiz()}.selectStep(0)`);
await shot("4-editor-redact-issue");

// 4. an import that the engine refuses
await ev(`${wiz()}.backToList()`);
await until(`document.querySelectorAll(".aw-card").length >= 6`, "the list again");
await ev(`${wiz()}.importFile()`);
await until(`document.querySelector(".aw-banner")`, "the import error");
await shot("5-import-error");

// 5. the run: files, then the preview, then the progress and the summary
await ev(`${wiz()}.startRun(${wiz()}.actions.find((a) => a.id === "builtin-prepare-for-email"))`);
await ev(`${wiz()}.addFiles()`);
await until(`document.querySelectorAll(".aw-file").length === 4`, "the files");
await shot("6-run-files");
await ev(`${wiz()}.preview()`);
await until(`document.querySelector(".aw-pv")`, "the preview");
await shot("7-run-preview");
await ev(`window.__MOCK.run.delay = 700`);
void ev(`${wiz()}.runNow()`); // not awaited: the run is the thing being watched
await until(`document.querySelector(".aw-result.running")`, "progress");
await sleep(500);
await shot("8-run-progress");
await until(`${wiz()}.run?.phase === "done"`, "the summary", 30000);
await shot("9-run-summary");

// 6. a Protect action: the password fields
await ev(`${wiz()}.backToList()`);
await until(`document.querySelectorAll(".aw-card").length >= 6`, "the list again");
await ev(`${wiz()}.startRun(${wiz()}.actions.find((a) => a.id === "weekly-client-pack"))`);
await ev(`${wiz()}.addFiles()`);
await until(`document.querySelectorAll(".aw-file").length === 4`, "the files");
await ev(`${wiz()}.setPassword("open", "client-2026")`);
await ev(`${wiz()}.preview()`);
await until(`document.querySelector(".aw-pv")`, "the preview");
await shot("10-run-protect");

chrome.kill();
process.exit(0);
