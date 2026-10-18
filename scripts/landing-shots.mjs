// Screenshots of the start screen through WebView2's DevTools endpoint (page-scoped input only).
// It launches the release build with its own data dir and WebView2 profile, so the user's recents,
// theme and files are never touched, and kills everything it started when done.
//   node scripts/landing-shots.mjs <out-dir> [path\to\inkwell.exe]
import { spawn, execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/landing";
const exe = process.argv[3] ?? "src-tauri/target/release/inkwell.exe";
const samples = "samples";
const PORT = 9361;
fs.mkdirSync(out, { recursive: true });

const root = fs.mkdtempSync(path.join(os.tmpdir(), "inkwell-landing-"));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// A believable document tree, so the screenshots read like a real machine.
// [relative path, sample, opened ago (ms), pinned, last page (0-based)]
const files = [
  ["Documents/Work/Finance/Quarterly report Q3.pdf", "pages-a.pdf", 12 * 60e3, false, 2],
  ["Documents/Home/Lease agreement.pdf", "org-out-02-insert.pdf", 3 * 3600e3, true, 2],
  ["Downloads/Invoice 2026-0412.pdf", "compress-source.pdf", 26 * 3600e3, false, 0],
  ["Documents/Work/Conference notes.pdf", "edit-source.pdf", 3 * 86400e3, false, 0],
  ["Documents/Personal/Passport scan.pdf", "scan-source.pdf", 9 * 86400e3, false, 0],
  ["Documents/Work/Project proposal.pdf", "redact-source.pdf", 14 * 86400e3, false, 0],
  ["Documents/Taxes/Tax return 2025.pdf", null, 40 * 86400e3, false, 0], // missing on purpose
  ["Documents/Work/Meeting minutes.pdf", "org-out-01-ops.pdf", 60 * 86400e3, false, 0],
];
const seed = files.map(([rel, src, ago, pinned, lastPage]) => {
  const p = path.join(root, "Users", "Alex", ...rel.split("/"));
  if (src) {
    fs.mkdirSync(path.dirname(p), { recursive: true });
    fs.copyFileSync(path.join(samples, src), p);
  }
  if (src) fs.utimesSync(p, new Date(Date.now() - ago), new Date(Date.now() - ago)); // a believable "Modified" column
  return { path: p, opened: Date.now() - ago, ...(pinned ? { pinned: true } : {}), ...(lastPage ? { lastPage } : {}) };
});

async function launch(name, recents) {
  const data = path.join(root, `${name}-data`);
  fs.mkdirSync(data, { recursive: true });
  if (recents) fs.writeFileSync(path.join(data, "recents.json"), JSON.stringify(recents));
  const child = spawn(path.resolve(exe), [], {
    stdio: "ignore",
    env: {
      ...process.env,
      INKWELL_DATA_DIR: data,
      WEBVIEW2_USER_DATA_FOLDER: path.join(root, `${name}-profile`),
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`,
    },
  });
  let target;
  for (let i = 0; i < 80 && !target; i++) {
    await sleep(250);
    try {
      target = (await (await fetch(`http://127.0.0.1:${PORT}/json`)).json()).find((t) => t.type === "page");
    } catch {
      /* not up yet */
    }
  }
  if (!target) throw new Error("app did not expose a page target");
  const ws = new WebSocket(target.webSocketDebuggerUrl);
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
  const shot = async (file) => {
    const r = await send("Page.captureScreenshot", { format: "png" });
    fs.writeFileSync(path.join(out, file), Buffer.from(r.data, "base64"));
    console.log("screenshot", path.resolve(out, file));
  };
  const click = (sel, nth = 0) =>
    ev(`(() => { const e = document.querySelectorAll(${JSON.stringify(sel)})[${nth}]; if (!e) return false; e.click(); return true; })()`);
  const close = () => {
    try {
      ws.close();
    } catch {
      /* already closed */
    }
    try {
      execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
    } catch {
      /* already gone */
    }
  };
  for (let i = 0; i < 80 && !(await ev("!!window.__inkwell").catch(() => false)); i++) await sleep(250);
  return { ev, shot, send, click, close, data };
}

const theme = (app, t) => app.ev(`window.__inkwell.useStore.getState().setTheme('${t}'); true`);
const hasThumbs = (app) => app.ev("document.querySelectorAll('.read-page img, .mini-thumb img, .lib-thumb img').length");

try {
  // 1. nothing opened yet: home with empty recents
  let app = await launch("empty", null);
  try {
    await app.ev("document.fonts.ready.then(() => true)");
    for (const t of ["light", "dark"]) {
      await theme(app, t);
      await sleep(1000);
      await app.shot(`home-empty-${t}.png`);
    }
  } finally {
    app.close();
  }
  await sleep(1500);

  // 2. a filled list, one pinned, one missing: home, library, preview
  app = await launch("filled", seed);
  try {
    const present = Math.min(4, seed.filter((r) => fs.existsSync(r.path)).length);
    for (let i = 0; i < 80 && (await hasThumbs(app)) < present; i++) await sleep(250);
    console.log("thumbnails on home", await hasThumbs(app), "of", present);
    // the preview pane's buttons must be visible without scrolling
    const fold = await app.ev("(() => { const b = document.querySelector('.hp-actions')?.getBoundingClientRect(); return b ? [Math.round(b.bottom), innerHeight] : null; })()");
    console.log("preview buttons bottom / viewport height", fold, fold && fold[0] <= fold[1] ? "ok" : "BELOW THE FOLD");
    for (const t of ["light", "dark"]) {
      await theme(app, t);
      await sleep(1000);
      await app.shot(`home-${t}.png`);
    }

    // another row selected: the preview pane follows
    await theme(app, "light");
    await app.click(".trow-open", 1);
    await sleep(1500);
    await app.shot("home-selected-light.png");

    // a "..." menu open on the first recent row
    await theme(app, "light");
    await app.click(".recent-table .more");
    await sleep(300);
    await app.shot("home-menu-light.png");
    await app.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });

    // an OS drag cannot be synthesized from a page: the DOM drag events drive the same state
    await app.ev("document.querySelector('.home').dispatchEvent(new DragEvent('dragenter', { bubbles: true, cancelable: true })); true");
    await sleep(400);
    await app.shot("home-dragover-light.png");
    await app.ev("document.querySelector('.home').dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true })); true");

    // narrow window: tools and cards reflow
    await app.send("Emulation.setDeviceMetricsOverride", { width: 760, height: 700, deviceScaleFactor: 1, mobile: false });
    await sleep(500);
    await app.shot("home-narrow-light.png");
    await app.send("Emulation.clearDeviceMetricsOverride");

    // library via "See all"
    await app.click(".nav-item", 1);
    await sleep(800);
    for (let i = 0; i < 40 && (await app.ev("document.querySelectorAll('.lib-thumb img').length")) < 5; i++) await sleep(250);
    for (const t of ["light", "dark"]) {
      await theme(app, t);
      await sleep(800);
      await app.shot(`library-${t}.png`);
    }

    // select the first file: list + preview
    await app.click(".lib-row", 0);
    await sleep(1500);
    for (const t of ["light", "dark"]) {
      await theme(app, t);
      await sleep(800);
      await app.shot(`preview-${t}.png`);
    }
    // the missing file's preview
    await theme(app, "light");
    await app.click(".lib-row", 6);
    await sleep(600);
    await app.shot("preview-missing-light.png");
  } finally {
    app.close();
  }
  await sleep(1500);

  // 3. opening documents records them (one entry per file however the path is spelled, with a thumbnail)
  //    and reading position is stored and restored
  app = await launch("open", null);
  try {
    const f = seed[0].path;
    const st = "window.__inkwell.useStore.getState()";
    const open = async (p) => {
      await app.ev(`${st}.openPath(${JSON.stringify(p)})`);
      await sleep(1500);
    };
    await open(f);
    await open(f.replaceAll("\\", "/").toLowerCase());
    const n = await app.ev("Object.keys(window.__inkwell.useStore.getState().docs).length");
    const id = await app.ev(`${st}.active`);
    await app.ev(`${st}.goTo(${id}, 2); true`);
    await sleep(1500);
    await app.ev(`(() => { const s = ${st}; Object.keys(s.docs).forEach((i) => s.closeDoc(Number(i))); return true; })()`);
    await sleep(1500);
    const saved = JSON.parse(fs.readFileSync(path.join(app.data, "recents.json"), "utf8"));
    console.log("docs opened", n, "| entries", saved.length, "| thumb", !!saved[0]?.thumb, "| pages", saved[0]?.pages, "| lastPage", saved[0]?.lastPage);
    await open(f);
    const cur = await app.ev(`${st}.docs[${st}.active].currentPage`);
    console.log("reopened at page index", cur, "(expected 2)");
    // many tabs in a narrow window: the window controls must stay inside it
    for (const q of seed.filter((r) => fs.existsSync(r.path))) await open(q.path);
    await app.send("Emulation.setDeviceMetricsOverride", { width: 760, height: 700, deviceScaleFactor: 1, mobile: false });
    await sleep(500);
    const ctl = await app.ev("(() => { const b = document.querySelector('.winctl .close')?.getBoundingClientRect(); return b ? [Math.round(b.right), innerWidth, document.querySelectorAll('.tab').length] : null; })()");
    console.log("close button right / window width / tabs", ctl, ctl && ctl[0] <= ctl[1] ? "ok" : "PUSHED OFF-SCREEN");
    await app.shot("viewer-tabs-narrow-light.png");
    await app.send("Emulation.clearDeviceMetricsOverride");
    // the viewer with a document open, in both themes (a closed-and-reopened file, back at its page)
    await app.ev(`(() => { const s = ${st}; const d = Object.values(s.docs).find((x) => x.path === ${JSON.stringify(f)}); if (d) s.activate(d.id); return !!d; })()`);
    await sleep(800);
    for (const t of ["light", "dark"]) {
      await theme(app, t);
      await sleep(800);
      await app.shot(`viewer-${t}.png`);
    }
  } finally {
    app.close();
  }
} finally {
  await sleep(500);
  try {
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });
  } catch {
    console.log("left behind:", root);
  }
}
