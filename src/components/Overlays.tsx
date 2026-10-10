import { useEffect, useMemo, useRef, useState } from "react";
import { commands, prettyKeys } from "../lib/commands";
import { useRecents } from "../recents";
import { useStore } from "../store";

const st = () => useStore.getState();
const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;

interface Item {
  title: string;
  hint?: string;
  run: () => void;
}

export function CommandPalette() {
  const recent = useRecents((s) => s.items);
  const hasDoc = useStore((s) => s.active != null);
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const list = useRef<HTMLDivElement>(null);

  const items = useMemo<Item[]>(() => {
    const cmds = commands
      .filter((c) => hasDoc || !c.needsDoc)
      .map((c) => ({ title: c.title, hint: c.keys?.[0] && prettyKeys(c.keys[0]), run: c.run }));
    const files = recent.map(({ path: p }) => ({ title: `Open ${fileName(p)}`, hint: p, run: () => void st().openPath(p) }));
    const words = q.toLowerCase().split(/\s+/).filter(Boolean);
    return [...cmds, ...files].filter((i) => words.every((w) => i.title.toLowerCase().includes(w)));
  }, [q, recent, hasDoc]);

  useEffect(() => setSel(0), [q]);
  useEffect(() => {
    list.current?.children[sel]?.scrollIntoView({ block: "nearest" });
  }, [sel]);

  const close = () => st().setUi({ paletteOpen: false });
  const run = (i?: Item) => {
    close();
    // after the dialog is gone so focus-taking commands (go to page, find) win
    if (i) setTimeout(i.run, 0);
  };

  return (
    <div className="scrim" onMouseDown={close}>
      <div className="palette" onMouseDown={(e) => e.stopPropagation()}>
        <input
          autoFocus
          value={q}
          placeholder="Type a command…"
          spellCheck={false}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") setSel((sel + 1) % Math.max(1, items.length));
            else if (e.key === "ArrowUp") setSel((sel - 1 + items.length) % Math.max(1, items.length));
            else if (e.key === "Enter") run(items[sel]);
            else if (e.key === "Escape") close();
            else return;
            e.preventDefault();
          }}
        />
        <div className="palette-list" ref={list}>
          {items.map((i, n) => (
            <div key={i.title + n} className={n === sel ? "item on" : "item"} onMouseMove={() => setSel(n)} onClick={() => run(i)}>
              <span className="item-title">{i.title}</span>
              {i.hint && <span className="dim item-hint">{i.hint}</span>}
            </div>
          ))}
          {items.length === 0 && <div className="empty">No matching command</div>}
        </div>
      </div>
    </div>
  );
}

export function PasswordDialog() {
  const p = useStore((s) => s.password);
  const [pw, setPw] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => setBusy(false), [p?.error]);
  if (!p) return null;
  const submit = async () => {
    setBusy(true);
    await st().submitPassword(pw);
    setBusy(false);
    setPw("");
  };
  return (
    <div className="scrim">
      <form
        className="dialog"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
        onKeyDown={(e) => e.key === "Escape" && st().cancelPassword()}
      >
        <h2>Password required</h2>
        <p className="dim">{fileName(p.path)} is protected.</p>
        <input autoFocus type="password" value={pw} placeholder="Password" onChange={(e) => setPw(e.target.value)} />
        {p.error && <p className="error">{p.error}</p>}
        <div className="dialog-actions">
          <button type="button" className="btn" onClick={() => st().cancelPassword()}>
            Cancel
          </button>
          <button type="submit" className="btn primary" disabled={busy || pw === ""}>
            Open
          </button>
        </div>
      </form>
    </div>
  );
}

export function Toast() {
  const error = useStore((s) => s.error);
  useEffect(() => {
    if (!error) return;
    const t = setTimeout(() => st().setUi({ error: null }), 8000);
    return () => clearTimeout(t);
  }, [error]);
  if (!error) return null;
  return (
    <div className="toast" onClick={() => st().setUi({ error: null })}>
      {error}
    </div>
  );
}
