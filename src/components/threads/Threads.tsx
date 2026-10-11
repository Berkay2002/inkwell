import { useEffect, useRef, useState } from "react";
import type { ArticleThread, ThreadInfo } from "../../lib/api";
import { useAnnot } from "../../lib/annot";
import { useForms } from "../../lib/forms";
import { computeLayout } from "../../lib/layout";
import { useRedact } from "../../lib/redact";
import { beadZoom, threadOf, useThreads } from "../../lib/threads";
import { type DocState, type ZoomMode, useStore } from "../../store";
import { Icon } from "../Icons";
import { Shell, useEscape } from "../OrganizePlus";
import "./threads.css";

const th = () => useThreads.getState();
const EMPTY: ArticleThread[] = [];
const inText = (e: KeyboardEvent) => !!(e.target as HTMLElement | null)?.closest("input, textarea, select, [contenteditable='true']");
/** A menu or a sheet is open: it hears the keys first. */
const modal = () => !!document.querySelector(".ink-menu, .scrim");

/**
 * The part of article threads that lives beside the page: it keeps the list read while anything shows beads, brings
 * the bead being read into view (zoom to its width, scroll to its top), answers the reading keys, and draws the
 * pill and the Properties sheet.
 */
export function ThreadHost({ doc }: { doc: DocState }) {
  const id = doc.id;
  const list = useThreads((s) => s.docs[id]) ?? EMPTY;
  const follow = useThreads((s) => (s.follow?.doc === id ? s.follow : null));
  const tool = useThreads((s) => s.tool);
  const selected = useThreads((s) => (s.selected?.doc === id ? s.selected : null));
  const props = useThreads((s) => (s.props?.doc === id ? s.props : null));
  const show = useThreads((s) => s.show);
  const panel = useStore((s) => s.sidebar === "articles");
  const wantList = show || tool || !!follow || panel || !!props;

  // The list is read when something needs it and again after every edit, undo or redo.
  useEffect(() => {
    if (wantList) void th().load(id);
  }, [wantList, id, doc.rev]);

  // One page tool at a time: arming the Article tool puts the others down, and arming another puts it down.
  const redacting = useRedact((s) => s.tool);
  const annotating = useAnnot((s) => s.tool) !== "select";
  const preparing = useForms((s) => s.tool) !== "select";
  const other = redacting || annotating || preparing;
  useEffect(() => {
    if (!tool) return;
    useRedact.getState().setTool(false);
    useAnnot.getState().setTool("select");
    useForms.getState().setTool("select");
  }, [tool]);
  useEffect(() => {
    if (other) th().setTool(false);
  }, [other]);

  // One document at a time: leaving a tab ends reading and the tool there.
  useEffect(
    () => () => {
      useThreads.setState({ follow: null, selected: null, tool: false, props: null });
    },
    [id],
  );

  // Reading: the bead comes into view at the width of the view; the zoom the person had comes back when it ends.
  const before = useRef<{ doc: number; zoom: number; zoomMode: ZoomMode } | null>(null);
  useEffect(() => {
    const s = useStore.getState();
    if (!follow) {
      // the zoom goes back to the document it was taken from (a tab switch ends reading, and this id is the new tab's)
      const b = before.current;
      before.current = null;
      if (b && s.docs[b.doc]) s.patch(b.doc, { zoom: b.zoom, zoomMode: b.zoomMode });
      return;
    }
    const d = s.docs[id];
    if (!d) return;
    const b = threadOf(th().get(id), follow.thread)?.beads.find((x) => x.id === follow.bead);
    const view = document.querySelector<HTMLElement>(".viewer");
    if (!b || !view) return;
    before.current ??= { doc: id, zoom: d.zoom, zoomMode: d.zoomMode };
    const z = beadZoom(b.rect, d.rot, view.clientWidth);
    s.patch(id, { zoom: z, zoomMode: "custom" });
    s.goTo(id, b.page, Math.max(0, b.rect[1] - 8 / z), b.rect[0]);
    // the zoom the view was left at belongs to the document it was read in
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, follow?.thread, follow?.bead]);

  // Reading keys: Enter and Down scroll through the bead, then go to the next; Shift+Enter and Up the other way.
  useEffect(() => {
    if (!follow) return;
    const key = (e: KeyboardEvent) => {
      if (inText(e) || modal() || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        th().stopFollow();
        return;
      }
      const dir = e.key === "Enter" ? (e.shiftKey ? -1 : 1) : e.key === "ArrowDown" && !e.shiftKey ? 1 : e.key === "ArrowUp" && !e.shiftKey ? -1 : 0;
      if (!dir) return;
      e.preventDefault();
      e.stopPropagation();
      advance(id, dir);
    };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  }, [id, follow]);

  // The Article tool: Esc or End finishes the article (its Properties open), a second Esc puts the tool down.
  useEffect(() => {
    if (!tool) return;
    const key = (e: KeyboardEvent) => {
      if (inText(e) || modal() || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key !== "Escape" && e.key !== "End") return;
      const s = th();
      if (s.selected?.doc === id) s.finish();
      else if (e.key === "Escape") s.setTool(false);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  }, [id, tool]);

  const thread = follow ? threadOf(list, follow.thread) : undefined;
  const at = thread ? thread.beads.findIndex((b) => b.id === follow!.bead) : -1;
  const propsThread = props ? threadOf(list, props.thread) : undefined;
  return (
    <>
      {follow && thread && at >= 0 && (
        <div className="th-pill" role="toolbar" aria-label="Reading an article">
          <button className="ax-icon" title="Previous bead (Shift+Enter)" aria-label="Previous bead" disabled={at === 0} onClick={() => th().step(-1)}>
            <span className="th-flip">
              <Icon name="right" size={15} />
            </span>
          </button>
          <span className="th-pill-title" aria-live="polite">
            {thread.title || "Untitled"} · {at + 1} / {thread.beads.length}
          </span>
          <button className="ax-icon" title="Next bead (Enter)" aria-label="Next bead" disabled={at === thread.beads.length - 1} onClick={() => th().step(1)}>
            <Icon name="right" size={15} />
          </button>
          <button className="ax-icon" title="Stop reading (Esc)" aria-label="Stop reading" onClick={() => th().stopFollow()}>
            <Icon name="close" size={14} />
          </button>
        </div>
      )}
      {tool && !follow && (
        <div className="th-pill" role="toolbar" aria-label="Article tool">
          <span className="th-pill-title">
            {selected ? "Drag on a page to add a bead. Delete removes this one." : "Drag over a column to start an article."}
          </span>
          <button className="btn ghost" onClick={() => (selected ? th().finish() : th().setTool(false))}>
            {selected ? "Finish article" : "Done"}
          </button>
        </div>
      )}
      {propsThread && <PropsSheet key={propsThread.id} doc={id} thread={propsThread} />}
    </>
  );
}

/** One step of reading: scroll on inside the bead while there is more of it, else go to the next (or previous) bead. */
function advance(id: number, dir: 1 | -1) {
  const s = useStore.getState();
  const d = s.docs[id];
  const f = th().follow;
  const view = document.querySelector<HTMLElement>(".viewer");
  const b = f ? threadOf(th().get(id), f.thread)?.beads.find((x) => x.id === f.bead) : undefined;
  if (d && view && b && d.rot === 0) {
    const L = computeLayout(d.sizes, d.rot, d.zoom, view.clientWidth);
    const top = L.tops[b.page] + b.rect[1] * d.zoom;
    const bottom = L.tops[b.page] + b.rect[3] * d.zoom;
    const h = view.clientHeight;
    if (dir === 1 && bottom > view.scrollTop + h + 4) {
      view.scrollTop = Math.min(view.scrollTop + h * 0.85, bottom - h + 16);
      return;
    }
    if (dir === -1 && top < view.scrollTop - 4) {
      view.scrollTop = Math.max(view.scrollTop - h * 0.85, top - 8);
      return;
    }
  }
  th().step(dir);
}

function PropsSheet({ doc, thread }: { doc: number; thread: ArticleThread }) {
  const close = () => th().closeProps();
  const [info, setInfo] = useState<ThreadInfo>({ ...thread.info, title: thread.info.title || thread.title });
  const set = (k: keyof ThreadInfo) => (e: React.ChangeEvent<HTMLInputElement>) => setInfo({ ...info, [k]: e.target.value });
  useEscape(true, close);
  const apply = () => {
    close();
    const same = (Object.keys(info) as (keyof ThreadInfo)[]).every((k) => info[k] === thread.info[k]);
    if (!same) void th().describe(doc, thread.id, info);
  };
  const field = (k: keyof ThreadInfo, label: string, first = false) => (
    <label className="op-field">
      <span className="op-label">{label}</span>
      <input
        className="tl-field"
        value={info[k]}
        autoFocus={first}
        spellCheck={false}
        onChange={set(k)}
        onKeyDown={(e) => e.key === "Enter" && apply()}
      />
    </label>
  );
  return (
    <Shell title="Article properties" label="Article properties" onClose={close}>
      <div className="op-body">
        {field("title", "Title", true)}
        {field("subject", "Subject")}
        {field("author", "Author")}
        {field("keywords", "Keywords")}
        {thread.damaged && (
          <p className="op-error" role="alert">
            {thread.damaged}
          </p>
        )}
      </div>
      <footer className="op-foot">
        <span className="grow" />
        <button className="btn ghost" onClick={close}>
          Cancel
        </button>
        <button className="btn primary" onClick={apply}>
          Apply
        </button>
      </footer>
    </Shell>
  );
}
