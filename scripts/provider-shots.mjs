// The assistant on its Claude provider: the toolbar toggle with the Claude mark, the provider picker open, the
// model button ("Haiku 5.5 · Medium" by default), light and dark. The status is the real one from the installed
// `claude` (plan, model list), nothing is injected. Over WebView2's DevTools endpoint; no OS-level input.
//   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9222'; inkwell.exe report.pdf
//   node scripts/provider-shots.mjs docs/screens/assistant 9222 [vitePort]
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/assistant";
const port = process.argv[3] ?? "9222";
const vitePort = process.argv[4];
fs.mkdirSync(out, { recursive: true });

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const ws = new WebSocket(targets.find((t) => t.type === "page").webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let seq = 0;
const waiting = new Map();
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id) waiting.get(d.id)?.(d);
  if (d.method === "Fetch.requestPaused" && vitePort) {
    const target = d.params.request.url.replace(/^http:\/\/localhost:1420/, `http://localhost:${vitePort}`);
    send("Fetch.continueRequest", { requestId: d.params.requestId, url: target }).catch(() => {});
  }
};
const send = (method, params = {}) =>
  new Promise((res, rej) => {
    const id = ++seq;
    waiting.set(id, (d) => (d.error ? rej(new Error(JSON.stringify(d.error))) : res(d.result)));
    ws.send(JSON.stringify({ id, method, params }));
  });
const ev = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text ?? "eval failed");
  return r.result.value;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const until = async (expr, what, tries = 200) => {
  for (let i = 0; i < tries; i++) {
    if (await ev(`(() => { try { return !!(${expr}) } catch { return false } })()`)) return;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${what}`);
};
const shot = async (name) => {
  await sleep(500);
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  console.log("screenshot", name);
};
const S = "window.__inkwell.useStore";
const A = "window.__inkwell.useAi";
const MODEL_PILL = JSON.stringify(`button[title^="Model and reasoning effort"]`);
const PROVIDER_PILL = JSON.stringify(`.ax-pill[title="Which assistant answers"]`);

await send("Page.enable");
await send("Network.enable");
if (vitePort) {
  await send("Network.setBlockedURLs", { urls: ["ws://localhost:1420/*"] });
  await send("Fetch.enable", { patterns: [{ urlPattern: "http://localhost:1420/*", requestStage: "Request" }] });
  await send("Page.reload", { ignoreCache: true });
  await sleep(1500);
}
await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await until(`window.__inkwell && ${S}.getState().order.length >= 1`, "the open document", 300);
const doc = await ev(`${S}.getState().order[0]`);
await ev(`${S}.getState().activate(${doc})`);
await until(`document.querySelectorAll(".page canvas.tile").length > 0`, "page tiles");

// fresh settings: nothing chosen, so the defaults show
await ev(`(() => { localStorage.removeItem("inkwell.ai"); const a = ${A}.getState(); a.setSettings({ mode: "docked", float: null, provider: "codex", model: null, claudeModel: "haiku", claudeEffort: "medium" }); a.setOpen(true); return true })()`);
await ev(`${A}.getState().setProvider(${doc}, "claude")`);
await until(`typeof ${A}.getState().statuses.claude === "object"`, "the real Claude status", 300);
const status = await ev(`${A}.getState().statuses.claude`);
console.log(JSON.stringify({ installed: status.installed, loggedIn: status.loggedIn, plan: status.plan, version: status.version, haiku: status.models.find((m) => m.id === "haiku") }));
await sleep(1500);
await until(`document.querySelector(${MODEL_PILL})`, "the model button", 100);
const label = await ev(`document.querySelector(${MODEL_PILL}).title`);
console.log("model button:", label);
const toggle = await ev(`(() => { const p = document.querySelector(".ax-toggle path"); return p ? p.getAttribute("d").slice(0, 24) : null })()`);
console.log("toggle path starts:", toggle);

for (const t of ["light", "dark"]) {
  await ev(`${S}.getState().setTheme(${JSON.stringify(t)})`);
  await sleep(600);
  await shot(`provider-claude-${t}.png`);
  await ev(`document.querySelector(${PROVIDER_PILL}).click()`);
  await sleep(300);
  await shot(`provider-picker-claude-${t}.png`);
  await ev(`document.querySelector(${PROVIDER_PILL}).click()`);
}
await ev(`${A}.getState().setProvider(${doc}, "codex")`);
await ev(`${S}.getState().setTheme("light")`);
console.log("done");
process.exit(0);
