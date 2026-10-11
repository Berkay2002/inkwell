// Drives Protect and Remove security through WebView2's DevTools endpoint on a COPY of a docs/study sample
// (never the user's files) and captures the rail, light and dark. The save dialog is a native one that can't be
// driven without OS-level input, so "Protect this file instead" (a plain Save) is the path that runs for real:
// the file is encrypted by the engine, closed, reopened with its password, its restrictions shown, and
// finally unprotected with the permissions password.
//   copy prot-in-01.pdf to <dir>; start the app with it and a scratch data dir:
//   $env:INKWELL_DATA_DIR='<scratch>'; $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9334'
//   inkwell.exe <dir>/prot-in-01.pdf     (debug build with `pnpm dev` running: it loads http://localhost:1420)
//   node scripts/protect-shots.mjs docs/screens/protect 9334
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/protect";
const port = process.argv[3] ?? "9222";
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
const S = "window.__inkwell.useStore";
const P = "window.__inkwell.useProtect";
const shot = async (name, wait = 400) => {
  await sleep(wait);
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  console.log("screenshot", name);
};
const theme = (t) => ev(`${S}.getState().setTheme("${t}")`);
const click = (sel, text) =>
  ev(
    `(() => { const b = [...document.querySelectorAll(${JSON.stringify(sel)})].find((e) => !${JSON.stringify(text ?? "")} || e.textContent.trim() === ${JSON.stringify(text ?? "")}); if (!b || b.disabled) return false; b.click(); return true })()`,
  );
const set = (p) => ev(`${P}.getState().set(${JSON.stringify(p)})`);
const until = async (expr, ms = 15000) => {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    if (await ev(expr)) return true;
    await sleep(150);
  }
  return false;
};
/** Both themes for one state. */
async function both(name) {
  await theme("light");
  await shot(`${name}-light.png`);
  await theme("dark");
  await shot(`${name}-dark.png`);
  await theme("light");
}
const report = {};
const OPEN_PW = "Lantern7fish!";
const OWNER_PW = "Harbour-9-lamp";

await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await theme("light");
await sleep(900);
const docPath = await ev(`Object.values(${S}.getState().docs)[0].path`);
report.file = path.basename(docPath);

// The Tools menu with the new entries, then the sheet as it opens.
await click('button[aria-label="Tools"]');
await sleep(200);
report.menuItems = await ev(`[...document.querySelectorAll('.tools-panel .tool-name')].map((e) => e.textContent.trim())`);
await shot("tools-menu-light.png", 150);
await click(".tools-panel .tool-name", "Protect with a password");
await sleep(500);
report.opened = await ev(`${P}.getState().sheet?.view`);
await both("protect-empty");

// Weak, then strong with restrictions.
await set({ openPassword: "abc", openConfirm: "ab" });
report.weakLabel = await ev(`document.querySelector(".pr-strength")?.textContent`);
await both("protect-weak-mismatch");
await set({ openPassword: OPEN_PW, openConfirm: OPEN_PW, restrict: true, permissionsPassword: OWNER_PW, permissionsConfirm: OWNER_PW, printing: "low", changes: "comments", copy: false });
report.strongLabel = await ev(`document.querySelector(".pr-strength")?.textContent`);
report.outcome = await ev(`document.querySelector(".pr-outcome")?.textContent.trim()`);
await both("protect-filled");
await set({ cipher: "aes128" });
await shot("protect-aes128-light.png");
await set({ cipher: "aes256" });

// The explicit confirm for replacing the file, then the run.
await click(".pr .ax-btn", "Protect this file instead");
await sleep(250);
await both("protect-confirm");
await click(".pr .ax-btn", "Protect and replace this file");
report.done = await until(`${P}.getState().sheet?.phase === "done"`);
report.after = await ev(`JSON.stringify(${P}.getState().security[Object.keys(${S}.getState().docs)[0]])`);
await both("protect-done");
report.tabLock = await ev(`!!document.querySelector(".tab.on .tab-lock")`);
await click('.pr .ax-btn', "Done");

// What a stranger sees: the file closed and opened again.
const id0 = await ev(`Object.keys(${S}.getState().docs)[0]`);
await ev(`${S}.getState().closeDoc(${id0})`);
await sleep(600);
await ev(`${S}.getState().openPath(${JSON.stringify(docPath)})`);
report.asked = await until(`${S}.getState().password != null`);
await sleep(300);
await both("password-prompt");
await ev(`${S}.getState().submitPassword(${JSON.stringify(OPEN_PW)})`);
report.reopened = await until(`Object.keys(${S}.getState().docs).length === 1 && !!document.querySelector(".pr-notice")`);
report.notice = await ev(`document.querySelector(".pr-notice")?.textContent.trim()`);
await both("restricted-notice");
await click(".pr-notice .tl-link", "Details");
await sleep(500);
report.securityRows = await ev(`[...document.querySelectorAll(".pr-rows > div")].map((r) => r.textContent.trim())`);
await both("security-view");

// Removing it: the wrong password first.
await set({ currentPassword: "not-the-password" });
await click(".pr .ax-btn", "Remove security");
report.wrong = await until(`!!document.querySelector(".pr [role=alert]")`);
report.wrongText = await ev(`document.querySelector(".pr [role=alert]")?.textContent`);
await both("security-wrong-password");
await set({ currentPassword: OWNER_PW, error: null });
await click(".pr .ax-btn", "Remove security");
report.removed = await until(`${P}.getState().sheet?.phase === "done"`);
await both("security-removed");
report.afterRemove = await ev(`JSON.stringify(${P}.getState().security[Object.keys(${S}.getState().docs)[0]])`);
await click(".pr .ax-btn", "Save");
report.saved = await until(`!Object.values(${S}.getState().docs)[0].dirty`);
await click(".pr .ax-btn", "Done");
await sleep(300);
await shot("after-save-light.png");

await send("Emulation.setFocusEmulationEnabled", { enabled: false });
console.log(JSON.stringify(report, null, 2));
ws.close();
