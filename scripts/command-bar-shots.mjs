// Drives a running Inkwell through WebView2's DevTools endpoint and captures the command bar (the assistant's
// third layout): the bar alone, the header strip, the open chat while it streams and when it is done, the full
// conversation, an error and an approval inside the chat, the chats and layout menus, a page chip hovered
// (connector to the badge), a selection row, the bar moved to the left, and a 900 px window. Light and dark, at
// 100 % and 150 %. The conversation is staged in the store: no model, no network, no real files.
//   $d = "$env:TEMP\inkwell-cb"; mkdir $d; copy <any multi-page PDF> $d\report.pdf
//   npx vite --port 1420        (the debug exe loads the dev server)
//   $env:INKWELL_DATA_DIR = "$d\data"; $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9231'
//   src-tauri\target\debug\inkwell.exe "$d\report.pdf"
//   node scripts/command-bar-shots.mjs docs/screens/assistant 9231
import fs from "node:fs";
import path from "node:path";

const out = process.argv[2] ?? "docs/screens/assistant";
const port = process.argv[3] ?? "9231";
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
const files = [];
const shot = async (name) => {
  await sleep(450); // the fold transition (180 ms) and tiles settle
  const r = await send("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(path.join(out, name), Buffer.from(r.data, "base64"));
  files.push(name);
  console.log("screenshot", name);
};

const full = await ev(`({ w: window.innerWidth, h: window.innerHeight })`);
// 150 % is a 1920x1080 screen at Windows scaling 150: a 1280x720 page drawn at 1.5 device pixels per CSS pixel.
const view = (w, h, scale) =>
  send("Emulation.setDeviceMetricsOverride", { width: scale === 1 ? w : 1280, height: scale === 1 ? h : 720, deviceScaleFactor: scale, mobile: false });

await ev(`(() => {
  window.__ai = window.__inkwell.useAi;
  window.__bar = null;
  window.__S = window.__inkwell.useStore;
  window.__doc = () => window.__S.getState().active;
  return true;
})()`);
// Keys and clicks go to the page through CDP, never to the OS.
const setView = (v) => ev(`window.__inkwell.useBar.setState({ view: ${JSON.stringify(v)} })`);
const key = (k) =>
  send("Input.dispatchKeyEvent", { type: "keyDown", key: k, code: k }).then(() => send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code: k }));
const click = (sel, text) =>
  ev(`(() => { const els = [...document.querySelectorAll(${JSON.stringify(sel)})]; const b = ${text ? `els.find((e) => e.textContent.trim() === ${JSON.stringify(text)})` : "els[0]"}; b?.click(); return !!b })()`);
const rectOf = (sel) =>
  ev(`(() => { const r = document.querySelector(${JSON.stringify(sel)})?.getBoundingClientRect(); return r ? { x: r.x, y: r.y, w: r.width, h: r.height } : null })()`);
const mouseTo = (x, y) => send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
const theme = (t) => ev(`window.__S.getState().setTheme(${JSON.stringify(t)})`);

const NOW = Date.now();
const q = (id, text, at = NOW) => ({ id, role: "user", kind: "ask", text, citations: [], followups: [], activity: [], state: "done", at });
const a = (id, text, extra = {}) => ({
  id,
  role: "assistant",
  text,
  citations: [],
  followups: [],
  activity: [],
  state: "done",
  at: NOW,
  timing: { firstTokenMs: 1100, totalMs: 22000, prepMs: 30 },
  ...extra,
});
const cite = (n, page, y, quote) => ({ n, page, rects: [[72, y, 330, y + 14]], quote, chunk: null, exact: true });
const steps = [
  { id: "s1", tool: "search", class: "read", label: "Searched “gross margin”", state: "done", detail: null },
  { id: "s2", tool: "read", class: "read", label: "Read pp. 3-4", state: "done", detail: null },
];
const TURN1 = [
  q(1, "Why did gross margin improve?"),
  a(
    2,
    "Gross margin rose from 41% to 46% because the supplier contract renegotiated in Q2 cut module costs [1], and a larger share of revenue came from the higher-margin service plans [2].\n\nThe report expects the effect to hold through next year [3].",
    { activity: steps, citations: [cite(1, 0, 130, "renegotiated supplier contract"), cite(2, 0, 190, "service plans"), cite(3, 1, 120, "hold through next year")] },
  ),
];
const TURN2 = [
  q(3, "How long can the company run on its cash?"),
  a(4, "At the current burn of 2.1 M a quarter the 14.6 M in cash lasts about seven quarters, to the end of next year [1].", {
    activity: [{ id: "s3", tool: "read", class: "read", label: "Read p. 5", state: "done", detail: null }],
    citations: [cite(1, 0, 250, "14.6 M in cash")],
  }),
  q(5, "And what are the three biggest risks?"),
  a(
    6,
    "1. **Supply concentration**: one vendor makes the lidar modules [1].\n2. **Customer concentration**: the top two customers are 58% of revenue [2].\n3. **Regulatory timing**: certification in the EU could slip a quarter [3].",
    { citations: [cite(1, 0, 130, "one vendor"), cite(2, 0, 190, "top two customers"), cite(3, 1, 120, "certification")] },
  ),
];
const STATUS = `{ installed: true, loggedIn: true, account: "demo", plan: null, error: null, models: [{ id: "gpt-6.1-sol", name: "GPT-6.1 Sol", efforts: ["low", "medium", "high"], defaultEffort: "low", isDefault: true }] }`;

/** Puts the assistant in the command bar with `messages` as this document's chat and returns once it is drawn. */
const stage = async (messages, { past = [], mode = "bar" } = {}) => {
  await ev(`(() => {
    const a = window.__ai.getState();
    a.setSettings({ consent: true, mode: ${JSON.stringify(mode)} });
    if (!a.open) a.setOpen(true);
    return true;
  })()`);
  // the status check the open started must have landed before the staged status goes in
  for (let i = 0; i < 40 && (await ev(`window.__ai.getState().status === "loading" || window.__ai.getState().status === null`)); i++) await sleep(250);
  await ev(`(() => {
    window.__ai.setState({
      status: ${STATUS}, mark: null, selection: null, pending: null,
      chats: { [window.__doc()]: { messages: ${JSON.stringify(messages)}, prep: { pages: 6, chars: 9000, enough: true } } },
      past: { [window.__doc()]: ${JSON.stringify(past)} },
    });
    return true;
  })()`);
  await sleep(250);
};
const press = async (n, k) => {
  for (let i = 0; i < n; i++) await key(k);
  await sleep(120);
};
const onlyBar = async () => {
  const live = await ev(`({ bar: document.querySelectorAll(".ax-bar-surface").length, dock: document.querySelectorAll(".ax-dock, .ax-panel.floating").length })`);
  if (live.bar !== 1 || live.dock !== 0) throw new Error(`layout check failed: ${JSON.stringify(live)}`);
};
/** Which stage the surface is in: "open", "header" or "bar" (from its class). */
const stageOf = () => ev(`document.querySelector(".ax-bar-surface")?.className.replace("ax-bar-surface ", "")`);

const report = {};
for (const scale of [1, 1.5]) {
  await view(full.w, full.h, scale);
  const s = scale === 1 ? "" : "-150";
  for (const th of ["light", "dark"]) {
    await theme(th);
    await ev(`window.__inkwell.useBar.getState().setAt("center")`);
    const n = (name) => `bar-${name}-${th}${s}.png`;

    // the bar alone
    await stage([]);
    await onlyBar();
    await shot(n("empty"));

    // a question just sent: the chat opens while the answer streams
    await stage([q(1, "Why did gross margin improve?"), a(2, "Gross margin rose from 41% to 46% because the supplier contract renegotiated in Q2 cut module", { state: "streaming", text: "Gross margin rose from 41% to 46% because the supplier contract renegotiated in Q2 cut module", activity: [{ ...steps[0], state: "done" }, { ...steps[1], state: "running" }], timing: undefined })]);
    await setView("open");
    report.streaming = await stageOf();
    await shot(n("streaming"));

    // done, open
    await stage(TURN1);
    await setView("open");
    report.done = await stageOf();
    await shot(n("done"));

    // the "Worked for" line opened
    await click(".ax-bar .ax-trail-line");
    await shot(n("worked"));
    await click(".ax-bar .ax-trail-line");

    // Esc collapses to the header, Esc again to the bar alone
    await key("Escape");
    await sleep(250);
    report.afterEsc1 = await stageOf();
    await shot(n("header"));
    await key("Escape");
    await sleep(250);
    report.afterEsc2 = await stageOf();
    await shot(n("minimized"));

    // the full conversation scrolls inside the card
    await stage([...TURN1, ...TURN2]);
    await setView("open");
    await shot(n("full"));

    // citation chip hovered: the connector runs to the badge on the page
    // the first chip of the newest answer (the older ones are scrolled out of view)
    const chip = await ev(`(() => { const all = [...document.querySelectorAll(".ax-bar .ax-pgchip")]; const r = all[all.length - 3]?.getBoundingClientRect(); return r ? { x: r.x, y: r.y, w: r.width, h: r.height } : null })()`);
    if (chip) {
      await mouseTo(chip.x + chip.w / 2, chip.y + chip.h / 2);
      await sleep(350);
      report.connector = await ev(`!!document.querySelector(".ax-connector path")`);
      await shot(n("citation"));
      await mouseTo(5, 5);
    }

    // an error inside the card, and a change waiting for approval
    await stage([q(1, "Delete page 4"), a(2, "", { state: "error", error: "Codex could not be reached. Check your connection and try again.", hint: null })]);
    await setView("open");
    await shot(n("error"));
    await stage([
      q(1, "Delete page 4"),
      a(2, "I can remove page 4 for you.", {
        state: "streaming",
        activity: [{ id: "w1", tool: "delete_pages", class: "destructive", label: "Delete page 4", state: "waiting", detail: "northwind-report.pdf, page 4 of 6" }],
        timing: undefined,
      }),
    ]);
    await sleep(300);
    report.approvalOpens = await stageOf();
    await shot(n("approval"));

    if (scale === 1) {
      // setup states live in the same surface: not signed in, and the consent question
      await stage([]);
      await ev(`window.__ai.setState({ status: { installed: true, loggedIn: false, account: null, plan: null, models: [], error: null } })`);
      await shot(n("gate"));
      await ev(`window.__ai.setState({ status: ${STATUS}, pending: { doc: window.__doc(), task: { kind: "ask", question: "q" }, label: "q" } })`);
      await shot(n("consent"));
      await ev(`window.__ai.setState({ pending: null })`);

      // the chats menu and the layout menu, open over the page
      await stage(TURN1, { past: [{ id: 91, messages: [q(11, "Summarize page 2"), a(12, "Page 2 covers the revenue table.")] }] });
      await setView("open");
      await click(".ax-bar-title");
      await shot(n("chats-menu"));
      await key("Escape");
      await click(".ax-bar-head button[aria-label='Chat options']");
      await shot(n("layout-menu"));
      await key("Escape");

      // a selection in the page: a row along the top of the composer
      await ev(`window.__ai.getState().setSelection({ text: "A supplier shortage of lidar modules delayed 140 units in the third quarter.", page: 0 })`);
      await shot(n("selection"));
      await ev(`window.__ai.getState().setSelection(null)`);

      // placement: moved to the left
      await click(".ax-bar-head button[aria-label='Chat options']");
      await click(".ax-bar .ax-pop button", "Move to left");
      await shot(n("left"));
      await click(".ax-bar-head button[aria-label='Chat options']");
      await click(".ax-bar .ax-pop button", "Move to centre");
    }
  }
}

// ---- real presses: CDP mouse and key events on the live page, the stage read back after each ----
await view(full.w, full.h, 1);
await theme("light");
const center = (sel, nth = 0) =>
  ev(`(() => { const r = document.querySelectorAll(${JSON.stringify(sel)})[${nth}]?.getBoundingClientRect(); return r ? { x: r.x + r.width / 2, y: r.y + r.height / 2, w: r.width, h: r.height, left: r.x } : null })()`);
const pressAt = async (x, y) => {
  await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount: 1 });
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", buttons: 0, clickCount: 1 });
  await sleep(350);
};
const pressOn = async (sel, nth = 0) => {
  const c = await center(sel, nth);
  if (!c) throw new Error(`nothing to press: ${sel}`);
  await pressAt(c.x, c.y);
};
const presses = {};
const check = async (name, expected, act) => {
  await act();
  const got = await stageOf();
  presses[name] = { expected, got, ok: got === expected };
};
await stage(TURN1);
await setView("open");
await sleep(300);
await check("press a page chip", "open", () => pressOn(".ax-bar .ax-pgchip"));
await check("press the Worked-for line", "open", () => pressOn(".ax-bar .ax-trail-line"));
await check("press the page", "header", () => pressAt(800, 200));
await check("press the header background", "open", async () => {
  const c = await center(".ax-bar-head");
  await pressAt(c.left + c.w * 0.62, c.y);
});
await check("press the header background again", "header", async () => {
  const c = await center(".ax-bar-head");
  await pressAt(c.left + c.w * 0.62, c.y);
});
await setView("bar");
await sleep(250);
await check("press the composer from the bar alone", "header", () => pressOn(".ax-bar .ax-input textarea"));
await setView("open");
await sleep(250);
await check("Esc with the composer focused", "header", async () => {
  await pressOn(".ax-bar .ax-input textarea");
  await key("Escape");
  await sleep(250);
});
await stage([
  q(1, "Delete page 4"),
  a(2, "I can remove page 4 for you.", {
    state: "streaming",
    activity: [{ id: "w1", tool: "delete_pages", class: "destructive", label: "Delete page 4", state: "waiting", detail: "northwind-report.pdf, page 4 of 6" }],
    timing: undefined,
  }),
]);
await check("press Apply", "open", () => pressOn(".ax-approve .ax-btn.primary"));
// Ctrl+Shift+A from the page: focuses the open bar and leaves the assistant open
await ev(`document.activeElement?.blur?.()`);
await send("Input.dispatchKeyEvent", { type: "rawKeyDown", modifiers: 10, key: "A", code: "KeyA", windowsVirtualKeyCode: 65 });
await send("Input.dispatchKeyEvent", { type: "keyUp", modifiers: 10, key: "A", code: "KeyA", windowsVirtualKeyCode: 65 });
await sleep(250);
presses["Ctrl+Shift+A focuses the open bar"] = await ev(`({ open: window.__ai.getState().open, inBar: !!document.activeElement?.closest(".ax-bar") })`);
presses["Ctrl+Shift+A focuses the open bar"].ok = presses["Ctrl+Shift+A focuses the open bar"].open && presses["Ctrl+Shift+A focuses the open bar"].inBar;
report.presses = presses;
console.log(JSON.stringify(presses, null, 1));
await ev(`window.__ai.setState({ chats: {} })`);

// a narrow window (about 900 px), 100 %
await view(900, full.h, 1);
for (const th of ["light", "dark"]) {
  await theme(th);
  await stage(TURN1);
  await setView("open");
  await shot(`bar-narrow-${th}.png`);
  await stage([]);
  await shot(`bar-narrow-empty-${th}.png`);
}

// the other two layouts are not touched by any of this
await view(full.w, full.h, 1);
await theme("light");
for (const mode of ["docked", "floating"]) {
  await stage(TURN1, { mode });
  await sleep(700);
  const live = await ev(`({ bar: document.querySelectorAll(".ax-bar-surface").length, dock: document.querySelectorAll(".ax-dock").length, floating: document.querySelectorAll(".ax-panel.floating").length })`);
  report[`layout-${mode}`] = live;
  await shot(`layout-${mode}-light.png`);
}
await ev(`window.__ai.getState().setMode("bar")`);
await send("Emulation.clearDeviceMetricsOverride");
fs.writeFileSync(path.join(out, "command-bar-report.json"), JSON.stringify({ report, files }, null, 2));
console.log(JSON.stringify(report));
ws.close();
