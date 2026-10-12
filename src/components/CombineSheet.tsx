import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { badRangeMessage, pagesIn, planFiles, securityDropped } from "../lib/combineLogic";
import { type CombineEntry, type CombineSheet as SheetState, pageUrl, useCombine } from "../lib/combineStore";
import type { CombineFilePlan } from "../lib/api";
import { Icon } from "./Icons";
import { PasswordPrompt } from "./Overlays";
import "./combine.css";

const act = () => useCombine.getState();

/** Combine files: one centred sheet, like Print. The list on the left of the outcome sentence, the options below it. */
export function CombineSheet() {
  const sheet = useCombine((s) => s.sheet);
  const open = sheet != null;
  const running = sheet?.phase === "running";
  const unlocking = sheet?.unlock != null;
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      // the password prompt over the sheet takes its own Escape
      if (e.key === "Escape" && !running && !unlocking) {
        // a field being typed in keeps its own Escape (blur)
        if ((e.target as HTMLElement | null)?.closest("input")) return;
        e.stopPropagation();
        act().close();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, running, unlocking]);
  if (!sheet) return null;
  const asking = sheet.unlock ? sheet.entries.find((e) => e.key === sheet.unlock?.key) : undefined;
  return (
    <>
      <div className="scrim cb-scrim" onMouseDown={() => !running && act().close()}>
        <section className="cb-sheet" role="dialog" aria-label="Combine files" onMouseDown={(e) => e.stopPropagation()}>
          <header className="cb-head">
            <Icon name="merge" size={16} />
            <span className="cb-title">Combine files</span>
            <span className="grow" />
            <button className="ax-icon" title="Close" aria-label="Close Combine files" disabled={running} onClick={() => act().close()}>
              <Icon name="close" size={14} />
            </button>
          </header>
          <Body sheet={sheet} />
          <Footer sheet={sheet} />
        </section>
      </div>
      {asking && (
        <PasswordPrompt
          className="cb-unlock"
          path={asking.path}
          error={sheet.unlock?.error}
          onSubmit={(pw) => act().unlock(pw)}
          onCancel={() => act().cancelUnlock()}
        />
      )}
    </>
  );
}

function Body({ sheet }: { sheet: SheetState }) {
  const { entries, plan, planKeys } = sheet;
  const planned = planFiles(entries, plan?.files ?? [], planKeys);
  const [drag, setDrag] = useState<{ from: number; to: number } | null>(null);
  const list = useRef<HTMLUListElement>(null);

  const startDrag = (from: number, e: React.PointerEvent) => {
    if (e.button !== 0 || entries.length < 2) return;
    e.preventDefault();
    const at = (y: number) => {
      const rows = Array.from(list.current?.querySelectorAll<HTMLElement>("[data-row]") ?? []);
      let to = rows.length - 1;
      for (let i = 0; i < rows.length; i++) {
        const r = rows[i].getBoundingClientRect();
        if (y < r.top + r.height / 2) {
          to = i;
          break;
        }
      }
      return to;
    };
    setDrag({ from, to: from });
    const move = (m: PointerEvent) => setDrag({ from, to: at(m.clientY) });
    const up = (m: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      setDrag(null);
      act().move(from, at(m.clientY));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  };

  return (
    <div className="cb-body">
      {entries.length === 0 ? (
        <div className="cb-empty">
          <Icon name="filePlus" size={28} />
          <p className="cb-empty-title">Add the PDFs to combine</p>
          <p className="dim">Choose files, or drag them here from Explorer. You can pick pages and put them in order next.</p>
          <button className="btn primary" onClick={() => void act().pick()}>
            Add files…
          </button>
        </div>
      ) : (
        <>
          <ul className={drag ? "cb-list dragging" : "cb-list"} ref={list} aria-label="Files to combine">
            {entries.map((e, i) => (
              <Row
                key={e.key}
                entry={e}
                index={i}
                count={entries.length}
                plan={planned[i] ?? null}
                dropBefore={drag != null && drag.to === i && drag.from !== i && drag.to < drag.from}
                dropAfter={drag != null && drag.to === i && drag.from !== i && drag.to > drag.from}
                lifted={drag?.from === i}
                onGrip={(ev) => startDrag(i, ev)}
              />
            ))}
          </ul>
          <div className="cb-add">
            <button className="btn ghost" onClick={() => void act().pick()}>
              <Icon name="filePlus" size={16} />
              Add files…
            </button>
            <span className="dim">or drag PDFs here from Explorer</span>
          </div>
        </>
      )}
    </div>
  );
}

function Row({
  entry,
  index,
  count,
  plan,
  dropBefore,
  dropAfter,
  lifted,
  onGrip,
}: {
  entry: CombineEntry;
  index: number;
  count: number;
  plan: CombineFilePlan | null;
  dropBefore: boolean;
  dropAfter: boolean;
  lifted: boolean;
  onGrip: (e: React.PointerEvent) => void;
}) {
  const { key, range, pages } = entry;
  const state = plan?.state.state ?? "ready";
  const bad = state !== "ready";
  const needsPassword = state === "encrypted";
  const taken = pages != null ? pagesIn(range, pages) : null;
  const used = plan ? plan.used : taken ? taken.size : pages;
  const sub = plan
    ? plan.pages === 0
      ? ""
      : `${plan.pages} ${plan.pages === 1 ? "page" : "pages"}${plan.hasOutline ? ", has bookmarks" : ""}${plan.hasFields ? ", has form fields" : ""}${plan.note ? `. ${plan.note}` : ""}`
    : pages != null
      ? `${pages} ${pages === 1 ? "page" : "pages"}`
      : "";
  return (
    <li
      data-row
      className={["cb-row", bad && "left-out", lifted && "lifted", dropBefore && "drop-before", dropAfter && "drop-after"].filter(Boolean).join(" ")}
    >
      <div className="cb-main">
        <button
          className="cb-grip"
          aria-label={`Move ${entry.name}`}
          title="Drag to reorder (or use the arrow keys)"
          onPointerDown={onGrip}
          onKeyDown={(ev) => {
            if (ev.key === "ArrowUp" && index > 0) {
              ev.preventDefault();
              act().move(index, index - 1);
            } else if (ev.key === "ArrowDown" && index < count - 1) {
              ev.preventDefault();
              act().move(index, index + 1);
            }
          }}
        >
          <Icon name="grip" size={14} />
        </button>
        <span className="cb-thumb" aria-hidden>
          {entry.thumb ? <img src={entry.thumb} alt="" draggable={false} /> : <Icon name="file" size={18} />}
        </span>
        <div className="cb-info">
          <span className="cb-name" title={entry.path}>
            {entry.name}
          </span>
          <span className={bad ? "cb-sub bad" : "cb-sub dim"} title={bad && plan?.message ? plan.message : sub}>
            {needsPassword && !entry.password ? "Password needed" : bad && plan?.message ? plan.message : sub}
          </span>
        </div>
        {needsPassword && entry.locked != null && (
          <button className="btn" onClick={() => act().askUnlock(key)}>
            Unlock…
          </button>
        )}
        <label className="cb-range">
          <span className="cb-sr">Pages of {entry.name}</span>
          <input
            className={state === "badRange" ? "tl-field bad" : "tl-field"}
            value={range}
            placeholder={pages == null ? "All pages" : pages === 1 ? "The page" : `All ${pages} pages`}
            spellCheck={false}
            aria-invalid={state === "badRange"}
            disabled={state === "encrypted" || state === "unreadable" || state === "noCopy"}
            onChange={(e) => act().setRange(key, e.target.value)}
          />
        </label>
        <button
          className={entry.open ? "cb-pages on" : "cb-pages"}
          aria-expanded={entry.open}
          disabled={pages == null || bad}
          title="Show the pages and click to leave some out"
          onClick={() => act().toggleOpen(key)}
        >
          {used != null && pages != null && used !== pages ? `${used} of ${pages}` : "Pages"}
          <Icon name="down" size={12} />
        </button>
        <button className="ax-icon" aria-label={`Remove ${entry.name}`} title="Remove" onClick={() => act().remove(key)}>
          <Icon name="close" size={14} />
        </button>
      </div>
      {entry.open && pages != null && <Expanded entry={entry} taken={taken} />}
    </li>
  );
}

function Expanded({ entry, taken }: { entry: CombineEntry; taken: Set<number> | null }) {
  const { key, pages } = entry;
  const n = pages ?? 0;
  return (
    <div className="cb-expanded">
      <label className="cb-title-field">
        <span className="cb-label">Bookmark title</span>
        <input
          className="tl-field"
          value={entry.title}
          placeholder={entry.name.replace(/\.pdf$/i, "")}
          spellCheck={false}
          onChange={(e) => act().setTitle(key, e.target.value)}
        />
      </label>
      <p className="dim cb-hint">
        {taken ? "Click a page to leave it out or put it back." : "The range above does not read as pages: fix it, or click a page to start again."}
      </p>
      <div className="cb-pagegrid" role="group" aria-label={`Pages of ${entry.name}`}>
        {Array.from({ length: n }, (_, i) => (
          <PageThumb key={i} entry={entry} page={i} on={taken ? taken.has(i) : true} />
        ))}
      </div>
    </div>
  );
}

function PageThumb({ entry, page, on }: { entry: CombineEntry; page: number; on: boolean }) {
  const el = useRef<HTMLButtonElement>(null);
  const [url, setUrl] = useState<string | null>(null);
  const size = entry.sizes[page];
  const doc = entry.doc;
  const [seen, setSeen] = useState(false);

  useLayoutEffect(() => {
    const node = el.current;
    if (!node) return;
    const io = new IntersectionObserver((es) => es.some((x) => x.isIntersecting) && setSeen(true), { rootMargin: "200px" });
    io.observe(node);
    return () => io.disconnect();
  }, []);

  useEffect(() => {
    if (!seen || url || doc == null || !size) return;
    let live = true;
    void pageUrl(doc, page, size)
      .then((u) => live && setUrl(u))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [seen, url, doc, page, size]);

  const aspect = size ? `${size[0]} / ${size[1]}` : "3 / 4";
  return (
    <button
      ref={el}
      className={on ? "cb-pg on" : "cb-pg"}
      aria-pressed={on}
      aria-label={`Page ${page + 1}${on ? ", included" : ", left out"}`}
      onClick={() => act().togglePage(entry.key, page)}
    >
      <span className="cb-pg-img" style={{ aspectRatio: aspect }}>
        {url && <img src={url} alt="" draggable={false} />}
      </span>
      <span className="cb-pg-num">{page + 1}</span>
      {on && (
        <span className="cb-pg-check" aria-hidden>
          <Icon name="check" size={12} />
        </span>
      )}
    </button>
  );
}

function Footer({ sheet }: { sheet: SheetState }) {
  const { options, plan, planKeys, planning, planError, phase, error, entries } = sheet;
  const running = phase === "running";
  const outcome =
    entries.length === 0
      ? "Nothing to combine yet"
      : planError
        ? planError
        : plan
          ? plan.canRun
            ? plan.summary
            : (badRangeMessage(entries, plan.files, planKeys) ?? "Fix the files marked in red to combine")
          : "Reading the files…";
  const bad = !!planError || (plan != null && !plan.canRun && entries.length > 0);
  return (
    <>
      <div className="cb-options" role="group" aria-label="Options">
        <Check label="A bookmark for each file" checked={options.bookmarks} disabled={running} onChange={(v) => act().setOptions({ bookmarks: v })} />
        <Check
          label="Keep each file's own bookmarks"
          checked={options.keepOutlines}
          disabled={running}
          onChange={(v) => act().setOptions({ keepOutlines: v })}
        />
        <Check label="A contents page in front" checked={options.contentsPage} disabled={running} onChange={(v) => act().setOptions({ contentsPage: v })} />
        <Check label="Keep the source page labels" checked={options.keepLabels} disabled={running} onChange={(v) => act().setOptions({ keepLabels: v })} />
        {securityDropped(entries, planFiles(entries, plan?.files ?? [], planKeys)) && (
          <span className="cb-note dim">The combined file has no password and no restrictions.</span>
        )}
      </div>
      <footer className="cb-foot">
        <span className={bad ? "cb-outcome bad" : "cb-outcome"} aria-live="polite">
          {running ? "Combining…" : outcome}
          {!running && !bad && plan && plan.leftOut > 0 && (
            <span className="cb-outcome-sub dim">
              {" "}
              {plan.leftOut === 1 ? "1 file is left out." : `${plan.leftOut} files are left out.`}
            </span>
          )}
          {error && <span className="cb-outcome-sub bad"> {error}</span>}
        </span>
        <span className="grow" />
        <button className="btn ghost" disabled={running} onClick={() => act().close()}>
          Cancel
        </button>
        <button className="btn primary" disabled={running || planning || !plan?.canRun || entries.length === 0} onClick={() => void act().run()}>
          Combine
        </button>
      </footer>
    </>
  );
}

function Check({ label, checked, disabled, onChange }: { label: string; checked: boolean; disabled?: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="cb-check">
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>{label}</span>
    </label>
  );
}
