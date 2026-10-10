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
const samples = "docs/study/samples";
const PORT = 9361;
fs.mkdirSync(out, { recursive: true });

const root = fs.mkdtempSync(path.join(os.tmpdir(), "inkwell-landing-"));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// A believable document tree, so the screenshots read like a real machine.
const files = [
  ["Documents/Work/Finance/Quarterly report Q3.pdf", "edit-source.pdf", 12 * 60e3, false],
  ["Documents/Home/Lease agreement.pdf", "form-source.pdf", 3 * 3600e3, true],
  ["Downloads/Invoice 2026-0412.pdf", "compress-source.pdf", 26 * 3600e3, false],
  ["Documents/Work/Conference notes.pdf", "pages-a.pdf", 3 * 86400e3, false],
  ["Documents/Personal/Passport scan.pdf", "scan-source.pdf", 9 * 86400e3, false],
  ["Documents/Work/Project proposal.pdf", "redact-source.pdf", 14 * 86400e3, false],
  ["Documents/Taxes/Tax return 2025.pdf", null, 40 * 86400e3, false], // missing on purpose
  ["Documents/Work/Meeting minutes.pdf", "org-out-01-ops.pdf", 60 * 86400e3, false],
];
const seed = files.map(([rel, src, ago, pinned]) => {
  const p = path.join(root, "Users", "Alex", ...rel.split("/"));
  if (src) {
    fs.mkdirSync(path.dirname(p), { recursive: true });
    fs.copyFileSync(path.join(samples, src), p);
  }
  return { path: p, opened: Date.now() - ago, ...(pinned ? { pinned: true } : {}) };
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
    console.log("screenshot", file);
  };
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
  return { ev, shot, send, close, data };
}

const theme = (app, t) => app.ev(`window.__inkwell.useStore.getState().setTheme('${t}'); true`);

try {
  // 1. nothing opened yet
  let app = await launch("empty", null);
  try {
    await app.ev("document.fonts.ready.then(() => true)");
    for (const t of ["light", "dark"]) {
      await theme(app, t);
      await sleep(1200);
      await app.shot(`empty-${t}.png`);
    }
  } finally {
    app.close();
  }
  await sleep(1500);

  // 2. a filled list, one pinned, one missing
  app = await launch("filled", seed);
  try {
    const present = seed.filter((r) => fs.existsSync(r.path)).length;
    for (let i = 0; i < 80; i++) {
      if ((await app.ev("document.querySelectorAll('.thumb-box img').length")) >= present) break;
      await sleep(250);
    }
    console.log("thumbnails", await app.ev("document.querySelectorAll('.thumb-box img').length"), "of", present);
    for (const t of ["light", "dark"]) {
      await theme(app, t);
      await sleep(1200);
      await app.shot(`recents-${t}.png`);

      // hover the second card (pin + remove buttons appear)
      const r = await app.ev(
        `(() => { const b = document.querySelectorAll('.card')[2].getBoundingClientRect(); return { x: b.left + b.width / 2, y: b.top + 60 }; })()`,
      );
      await app.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: r.x, y: r.y });
      await sleep(300);
      await app.shot(`hover-${t}.png`);
      await app.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: 2, y: 2 });

      // an OS drag cannot be synthesized from a page: the DOM drag events drive the same state
      await app.ev("document.querySelector('.home').dispatchEvent(new DragEvent('dragenter', { bubbles: true, cancelable: true })); true");
      await sleep(400);
      await app.shot(`dragover-${t}.png`);
      await app.ev("document.querySelector('.home').dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true })); true");
    }
    await theme(app, "light");
    await app.ev("document.querySelector('[aria-label=\"List\"]').click(); true");
    await sleep(600);
    await app.shot("list-light.png");
    // a narrow window: cards reflow
    await app.send("Emulation.setDeviceMetricsOverride", { width: 760, height: 700, deviceScaleFactor: 1, mobile: false });
    await app.ev("document.querySelector('[aria-label=\"Thumbnails\"]').click(); true");
    await sleep(600);
    await app.shot("narrow-light.png");
  } finally {
    app.close();
  }
  await sleep(1500);

  // 3. opening documents records them: one entry per file however the path is spelled, with a thumbnail
  app = await launch("open", null);
  try {
    const f = seed[0].path;
    const open = async (p) => {
      await app.ev(`window.__inkwell.useStore.getState().openPath(${JSON.stringify(p)})`);
      await sleep(1500);
    };
    await open(f);
    await open(f.replaceAll("\\", "/").toLowerCase());
    const n = await app.ev("Object.keys(window.__inkwell.useStore.getState().docs).length");
    await app.ev("(() => { const s = window.__inkwell.useStore.getState(); Object.keys(s.docs).forEach((id) => s.closeDoc(Number(id))); return true; })()");
    await sleep(1500);
    const saved = JSON.parse(fs.readFileSync(path.join(app.data, "recents.json"), "utf8"));
    console.log("docs opened", n, "| recents entries", saved.length, "| thumb", !!saved[0]?.thumb, "| pages", saved[0]?.pages);
    await theme(app, "light");
    await sleep(800);
    await app.shot("after-open-light.png");
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
