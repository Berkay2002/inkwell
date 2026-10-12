import { useEffect, useMemo, useRef, useState } from "react";
import { commands, prettyKeys } from "../lib/commands";
import { useToolPrefs } from "../lib/toolPrefs";
import { toolItems } from "../lib/toolItems";
import { type ToolItem } from "../lib/toolsCatalog";
import { ToolTile } from "./ToolTile";
import { useRecents } from "../recents";
import { useStore } from "../store";

const st = () => useStore.getState();
const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;

interface Item {
  title: string;
  hint?: string;
  /** What the command is for, one plain line (the same text the All tools panel shows). */
  sub?: string;
  /** The tool this command starts: its tile leads the row. */
  tool?: ToolItem;
  /** Extra words the filter matches besides the title. */
  words?: string;
  run: () => void;
}

export function CommandPalette() {
  const recent = useRecents((s) => s.items);
  const hasDoc = useStore((s) => s.active != null);
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const list = useRef<HTMLDivElement>(null);

  const items = useMemo<Item[]>(() => {
    const tools = toolItems(null);
    const cmds = commands
      .filter((c) => hasDoc || !c.needsDoc)
      .map((c): Item => {
        const tool = tools.find((t) => t.command === c.id);
        return {
          title: c.title,
          hint: c.keys?.[0] && prettyKeys(c.keys[0]),
          sub: tool?.description,
          tool,
          words: tool && `${tool.label} ${tool.keywords ?? ""} ${tool.description}`,
          run: tool
            ? () => {
                useToolPrefs.getState().used(tool.id);
                c.run();
              }
            : c.run,
        };
      });
    const files = recent.map(({ path: p }): Item => ({ title: `Open ${fileName(p)}`, hint: p, run: () => void st().openPath(p) }));
    const words = q.toLowerCase().split(/\s+/).filter(Boolean);
    return [...cmds, ...files].filter((i) => {
      const hay = `${i.title} ${i.words ?? ""}`.toLowerCase();
      return words.every((w) => hay.includes(w));
    });
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
              <span className="item-lead">{i.tool && <ToolTile icon={i.tool.icon} group={i.tool.group} size={28} />}</span>
              <span className="item-text">
                <span className="item-title">{i.title}</span>
                {i.sub && <span className="dim item-sub">{i.sub}</span>}
              </span>
              {i.hint && <span className="dim item-hint">{i.hint}</span>}
            </div>
          ))}
          {items.length === 0 && <div className="empty">No matching command</div>}
        </div>
      </div>
    </div>
  );
}

/** The prompt for a locked file's password: one wording wherever a file asks (opening it, Combine files). */
export function PasswordPrompt({
  path,
  error,
  className = "",
  onSubmit,
  onCancel,
}: {
  path: string;
  error?: string;
  /** Extra class of the scrim (a prompt that opens over another sheet). */
  className?: string;
  onSubmit: (password: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [pw, setPw] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => setBusy(false), [error]);
  const submit = async () => {
    setBusy(true);
    await onSubmit(pw);
    setBusy(false);
    setPw("");
  };
  return (
    <div className={`scrim ${className}`.trim()}>
      <form
        className="dialog"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
        onKeyDown={(e) => e.key === "Escape" && onCancel()}
      >
        <h2>Password required</h2>
        <p className="dim">{fileName(path)} is protected.</p>
        <input autoFocus type="password" value={pw} placeholder="Password" onChange={(e) => setPw(e.target.value)} />
        {error && <p className="error">{error}</p>}
        <div className="dialog-actions">
          <button type="button" className="btn" onClick={onCancel}>
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

export function PasswordDialog() {
  const p = useStore((s) => s.password);
  if (!p) return null;
  return <PasswordPrompt path={p.path} error={p.error} onSubmit={(pw) => st().submitPassword(pw)} onCancel={() => st().cancelPassword()} />;
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
