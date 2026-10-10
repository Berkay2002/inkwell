// Drives a running Inkwell through WebView2's DevTools endpoint: frame timing, tile transport,
// memory and screenshots. Start the app first with
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9222'; inkwell.exe <big.pdf> <small.pdf>
// then: node scripts/perf.mjs <screens-dir>
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/phase1";
fs.mkdirSync(out, { recursive: true });

const targets = await (await fetch("http://127.0.0.1:9222/json")).json();
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
const shot = async (name) => {
  await ev("window.__settle()");
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  console.log("screenshot", name);
};

await ev(`(() => {
  const S = () => window.__inkwell.useStore;
  window.__S = S;
  window.__sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  // resolves (with ms waited) once every wanted full-resolution tile canvas has pixels
  window.__settle = async () => {
    const t0 = performance.now();
    await new Promise((r) => requestAnimationFrame(r));
    for (;;) {
      const missing = [...document.querySelectorAll('.viewer canvas.tile:not(.preview):not(.under)')].filter((c) => !c.dataset.ready);
      if (missing.length === 0 || performance.now() - t0 > 15000) break;
      await new Promise((r) => requestAnimationFrame(r));
    }
    await window.__sleep(120);
    return performance.now() - t0;
  };
  // runs step(msSinceStart) every animation frame for ms; resolves with the frame deltas
  window.__frames = (ms, step) => new Promise((done) => {
    const d = [];
    let last = performance.now();
    const t0 = last;
    const tick = (now) => {
      d.push(now - last);
      last = now;
      step(now - t0);
      if (now - t0 < ms) requestAnimationFrame(tick); else done(d);
    };
    requestAnimationFrame(tick);
  });
  window.__stats = (d) => {
    const s = [...d].sort((a, b) => a - b);
    const q = (p) => s[Math.min(s.length - 1, Math.floor(p * s.length))];
    return { frames: d.length, fps: +(1000 / (d.reduce((a, b) => a + b, 0) / d.length)).toFixed(1),
      p50: +q(0.5).toFixed(1), p95: +q(0.95).toFixed(1), p99: +q(0.99).toFixed(1), max: +s[s.length - 1].toFixed(1),
      over25: d.filter((x) => x > 25).length, over50: d.filter((x) => x > 50).length };
  };
  return true;
})()`);

const docs = await ev(`Object.values(__S().getState().docs).map((d) => ({ id: d.id, name: d.name, pages: d.pageCount }))`);
console.log("docs", JSON.stringify(docs));
const big = docs.reduce((a, b) => (b.pages > a.pages ? b : a));
await ev(`__S().getState().activate(${big.id}); __S().getState().setSidebar('thumbs'); __S().getState().setTheme('light'); true`);
await ev("__sleep(600)");
const result = { doc: big };

// --- scrolling and zoom frame timing
await ev("document.querySelector('.viewer').scrollTop = 0; __settle()");
result.scrollSteady = await ev(`(async () => {
  const v = document.querySelector('.viewer');
  const d = await __frames(5000, () => { v.scrollTop += 50; });
  return { ...__stats(d), settleMs: Math.round(await __settle()) };
})()`);
result.scrollFling = await ev(`(async () => {
  const v = document.querySelector('.viewer');
  const d = await __frames(3000, () => { v.scrollTop += 400; });
  return { ...__stats(d), settleMs: Math.round(await __settle()) };
})()`);
result.zoom = await ev(`(async () => {
  const v = document.querySelector('.viewer');
  const r = v.getBoundingClientRect();
  const wheel = (dy) => v.dispatchEvent(new WheelEvent('wheel', { ctrlKey: true, deltaY: dy, clientX: r.left + r.width / 2, clientY: r.top + r.height / 2, bubbles: true, cancelable: true }));
  const dIn = await __frames(2000, () => wheel(-8));
  const s1 = await __settle();
  const dOut = await __frames(2000, () => wheel(8));
  const s2 = await __settle();
  return { in: { ...__stats(dIn), settleMs: Math.round(s1) }, out: { ...__stats(dOut), settleMs: Math.round(s2) } };
})()`);
result.stats = await ev(`window.__TAURI_INTERNALS__.invoke('engine_stats')`);

// --- text layer sanity: lines exist and sit inside their page box
result.textLayer = await ev(`(async () => {
  await __S().getState().setZoomMode(${big.id}, 'fitWidth');
  await __S().getState().goTo(${big.id}, 5);
  await __settle();
  const page = [...document.querySelectorAll('.page')].find((p) => p.dataset.page === '5');
  const pr = page.getBoundingClientRect();
  const lines = [...page.querySelectorAll('.textlayer > div')];
  const inside = lines.filter((l) => { const r = l.getBoundingClientRect(); return r.left >= pr.left - 1 && r.right <= pr.right + 1 && r.top >= pr.top - 1 && r.bottom <= pr.bottom + 1; });
  return { lines: lines.length, inside: inside.length, sample: lines[0]?.textContent.slice(0, 40) };
})()`);

// --- tile transport: custom protocol vs raw IPC, both from the warm cache (pure transport cost)
result.transport = await ev(`(async () => {
  const doc = ${big.id};
  const N = 128, C = 8;
  const cv = document.createElement('canvas'); cv.width = 512; cv.height = 512;
  const ctx = cv.getContext('2d');
  const origin = navigator.userAgent.includes('Windows') ? 'http://inkwell.localhost' : 'inkwell://localhost';
  const pool = async (fn) => {
    let next = 0;
    const t0 = performance.now();
    await Promise.all(Array.from({ length: C }, async () => { for (let i; (i = next++) < N; ) await fn(i); }));
    return +(performance.now() - t0).toFixed(1);
  };
  const viaProtocol = async (i) => {
    const res = await fetch(origin + '/t/' + doc + '/' + i + '/0/1000/0/0?e=1&r=0&s=3');
    const buf = await res.arrayBuffer();
    ctx.putImageData(new ImageData(new Uint8ClampedArray(buf), 512, 512), 0, 0);
  };
  const viaIpc = async (i) => {
    const buf = await window.__TAURI_INTERNALS__.invoke('tile_raw', { doc, page: i, rot: 0, scale: 1000, tx: 0, ty: 0 });
    ctx.putImageData(new ImageData(new Uint8ClampedArray(buf), 512, 512), 0, 0);
  };
  const cold = await pool(viaProtocol);       // renders every tile once
  const protocol = await pool(viaProtocol);   // warm cache
  const ipc = await pool(viaIpc);
  return { tiles: N, tileBytes: 512 * 512 * 4, coldRenderMs: cold, protocolMs: protocol, ipcMs: ipc,
    protocolPerTile: +(protocol / N).toFixed(2), ipcPerTile: +(ipc / N).toFixed(2) };
})()`);

// --- process memory (inkwell + its WebView2 children)
const mem = spawnSync(
  "powershell",
  [
    "-NoProfile",
    "-Command",
    `$p = @(Get-Process inkwell -ErrorAction SilentlyContinue | % { @{n='inkwell';id=$_.Id} });
     $w = Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | ? { $_.CommandLine -match 'app.inkwell.desktop' } | % { @{n='webview2';id=$_.ProcessId} };
     ($p + $w) | % { $q = Get-Process -Id $_.id -ErrorAction SilentlyContinue; if ($q) { [pscustomobject]@{ name=$_.n; ws=[math]::Round($q.WorkingSet64/1MB,1); priv=[math]::Round($q.PrivateMemorySize64/1MB,1) } } } | ConvertTo-Json -Compress`,
  ],
  { encoding: "utf8" },
);
try {
  const rows = JSON.parse(mem.stdout);
  result.memoryMB = { rows: [].concat(rows), totalWs: +[].concat(rows).reduce((a, r) => a + r.ws, 0).toFixed(1) };
} catch {
  result.memoryMB = mem.stdout || mem.stderr;
}

fs.writeFileSync(path.join(out, "perf.json"), JSON.stringify(result, null, 2));
console.log(JSON.stringify(result, null, 2));

// --- screenshots
const S = (c) => ev(`(async () => { const s = __S().getState(); ${c}; return true; })()`);
await S(`s.setZoomMode(${big.id}, 'fitWidth'); s.goTo(${big.id}, 0); s.setTheme('light'); s.setSidebar('thumbs')`);
await shot("viewer-light.png");
await S(`s.setTheme('dark')`);
await shot("viewer-dark.png");
await S(`s.runSearch(${big.id}, 'needle'); s.setUi({ searchOpen: true })`);
await ev(`(async () => { for (let i = 0; i < 100; i++) { const d = __S().getState().docs[${big.id}]; if (d.search.done && d.search.hits.length > 0) return; await __sleep(100); } })()`);
await ev("__sleep(400)");
await shot("search-dark.png");
await S(`s.setTheme('light')`);
await shot("search-light.png");
await S(`s.setUi({ searchOpen: false }); s.runSearch(${big.id}, ''); s.goTo(${big.id}, 300); s.setSidebar('thumbs')`);
await shot("thumbnails-light.png");
await S(`s.setSidebar('outline'); s.loadOutline(${big.id})`);
await ev("__sleep(400)");
await shot("outline-light.png");
await S(`s.setUi({ paletteOpen: true })`);
await ev("__sleep(200)");
await shot("palette-light.png");
await S(`s.setUi({ paletteOpen: false }); s.setTheme('system'); s.setSidebar('thumbs')`);

// --- open a 1000-page document in the running app: IPC open + first sharp tile painted
result.reopenMs = await ev(`(async () => {
  const s = __S().getState();
  const p = s.docs[${big.id}].path;
  s.closeDoc(${big.id});
  await __sleep(200);
  const t0 = performance.now();
  await s.openPath(p);
  while (!document.querySelector('.viewer canvas.tile:not(.preview):not(.under)[data-ready]')) await new Promise((r) => requestAnimationFrame(r));
  return Math.round(performance.now() - t0);
})()`);
console.log("reopen ms", result.reopenMs);
fs.writeFileSync(path.join(out, "perf.json"), JSON.stringify(result, null, 2));
await ev("localStorage.removeItem('inkwell.recent'); localStorage.removeItem('inkwell.theme'); true");
ws.close();
