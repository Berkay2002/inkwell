// A contact sheet of the whole icon set (name, Regular and Filled at 16 / 20 / 24 px, and as a tool tile in each
// group colour, light and dark), drawn inside the running app so it uses the real Icon component's data.
//   node scripts/icons-sheet.mjs <outDir> <cdpPort> <vitePort> [custom|all]
// Same setup as scripts/icons-shots.mjs (the exe's dev origin is redirected to our vite).
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/icons";
const port = process.argv[3] ?? "9470";
const vitePort = process.argv[4];
const which = process.argv[5] ?? "all";
fs.mkdirSync(out, { recursive: true });

const page = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find((t) => t.type === "page");
const origin = new URL(page.url).origin;
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let seq = 0;
const waiting = new Map();
const send = (method, params = {}) =>
  new Promise((res, rej) => {
    const id = ++seq;
    waiting.set(id, (d) => (d.error ? rej(new Error(JSON.stringify(d.error))) : res(d.result)));
    ws.send(JSON.stringify({ id, method, params }));
  });
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id) waiting.get(d.id)?.(d);
  if (d.method === "Fetch.requestPaused" && vitePort)
    send("Fetch.continueRequest", { requestId: d.params.requestId, url: d.params.request.url.replace(origin, `http://localhost:${vitePort}`) }).catch(() => {});
};
const ev = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text ?? "eval failed");
  return r.result.value;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

await send("Page.enable");
if (vitePort) {
  await send("Fetch.enable", { patterns: [{ urlPattern: `${origin}/*`, requestStage: "Request" }] });
  await send("Page.reload", { ignoreCache: true });
  await sleep(2500);
}
await ev(`(async () => {
  const I = await import("/src/components/Icons.tsx");
  const m = (await import("/src/assets/icons/manifest.json")).default;
  const names = ${JSON.stringify(which)} === "custom" ? Object.keys(m.inkwell) : [...Object.keys(m.inkwell), ...Object.keys(m.fluent)];
  const svg = (name, filled, size) => {
    const d = I.drawingOf(name, filled ? "filled" : "regular", size);
    if (!d) return "";
    if (d.kind === "fill") return '<svg width="' + size + '" height="' + size + '" viewBox="0 0 ' + d.grid + ' ' + d.grid + '" fill="currentColor">' + d.shapes.map((s) => '<path d="' + s.d + '"' + (s.evenodd ? ' fill-rule="evenodd"' : "") + "/>").join("") + "</svg>";
    const px = d.grid / size;
    return '<svg width="' + size + '" height="' + size + '" viewBox="0 0 ' + d.grid + ' ' + d.grid + '" fill="none" stroke="currentColor" stroke-width="' + d.stroke * px + '" stroke-linecap="round" stroke-linejoin="round">' + d.shapes.map((s) => '<path d="' + s.d + '"' + (s.transform ? ' transform="' + s.transform + '"' : "") + (s.fill ? ' fill="currentColor"' : "") + (s.evenodd ? ' fill-rule="evenodd"' : "") + (s.noStroke ? ' stroke="none"' : "") + (s.stroke ? ' stroke-width="' + s.stroke * px + '"' : "") + "/>").join("") + "</svg>";
  };
  const groups = ["optimize", "marks", "protect", "convert", "check", "edit", "comment", "organize", "sign"];
  document.getElementById("icon-sheet")?.remove();
  const root = document.createElement("div");
  root.id = "icon-sheet";
  root.style.cssText = "position:fixed;inset:0;z-index:99999;overflow:auto;background:var(--window);color:var(--text);padding:16px;font:12px var(--font)";
  const cell = (html) => '<td style="padding:4px 10px;text-align:center">' + html + "</td>";
  const tile = (name, g) => '<span class="tool-icon" data-group="' + g + '" data-size="36">' + svg(name, true, 24) + "</span>";
  root.innerHTML = '<table style="border-collapse:collapse"><thead><tr><th align="left">name</th><th>R16</th><th>R20</th><th>R24</th><th>F16</th><th>F20</th><th>F24</th><th colspan="3">tile</th></tr></thead><tbody>' +
    names.map((n, i) => '<tr style="border-top:1px solid var(--border)"><td>' + n + "</td>" + [[0, 16], [0, 20], [0, 24], [1, 16], [1, 20], [1, 24]].map(([f, s]) => cell(svg(n, !!f, s))).join("") +
      [0, 1, 2].map((k) => cell(tile(n, groups[(i + k * 3) % groups.length]))).join("") + "</tr>").join("") + "</tbody></table>";
  document.body.append(root);
  return names.length;
})()`);
await sleep(400);
for (const t of ["light", "dark"]) {
  await ev(`window.__inkwell.useStore.getState().setTheme("${t}")`);
  await sleep(300);
  const h = await ev(`document.getElementById("icon-sheet").scrollHeight`);
  await send("Emulation.setDeviceMetricsOverride", { width: 760, height: Math.min(h + 40, 6000), deviceScaleFactor: 2, mobile: false });
  await sleep(300);
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, `sheet-${which}-${t}.png`), Buffer.from(r.data, "base64"));
  console.log("sheet", t, h);
}
await send("Emulation.clearDeviceMetricsOverride");
await ev(`document.getElementById("icon-sheet")?.remove()`);
ws.close();
