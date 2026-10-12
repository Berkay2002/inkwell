import { useEffect, useState } from "react";
import { pickOpen } from "../../lib/api";
import { type Source, type VersionInfo, historyPageCount, sourceKey } from "../../lib/history";
import { type SetupContent, rangeField, sameSides, setupShow, versionTitle, when } from "../../lib/historyLogic";
import { type CompareReq, useHistory } from "../../lib/historyStore";
import { useStore } from "../../store";
import { Icon } from "../Icons";
import { Thumb } from "./PageStrip";

const EMPTY: VersionInfo[] = [];
const toast = (error: string) => useStore.getState().setUi({ error });
const baseName = (p: string) => p.split(/[\\/]/).pop() || "file";

type How = "doc" | "past" | "file";

/** Which of the three ways a side is chosen it is on. */
const howOf = (s: Source): How => (s.kind === "version" ? "past" : s.kind === "picked" ? "file" : "doc");

const CONTENT: { id: keyof SetupContent; label: string }[] = [
  { id: "text", label: "Text" },
  { id: "formatting", label: "Formatting" },
  { id: "images", label: "Images" },
];

/** How many pages a source has: null until known, and for a file that cannot be read (the engine says why on Compare). */
function usePageCount(doc: number | null, src: Source, now: number | undefined): number | null {
  const key = `${doc}|${sourceKey(src)}`;
  const [got, setGot] = useState<{ key: string; n: number | null } | null>(null);
  useEffect(() => {
    let live = true;
    historyPageCount(doc, src).then(
      (n) => live && setGot({ key, n }),
      () => live && setGot({ key, n: null }),
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return got?.key === key ? got.n : src.kind === "current" ? (now ?? null) : null;
}

/** The page before a comparison: two file slots, how each is chosen, what to compare, and the Compare button. */
export function CompareSetup() {
  const req = useHistory((s) => s.compare)!;
  const versions = useHistory((s) => (req.doc != null ? (s.docs[req.doc]?.versions ?? EMPTY) : EMPTY));
  const docName = useStore((s) => (req.doc != null ? s.docs[req.doc]?.name : undefined));
  const docPath = useStore((s) => (req.doc != null ? (s.docs[req.doc]?.path ?? null) : null));
  const pageCount = useStore((s) => (req.doc != null ? s.docs[req.doc]?.pageCount : undefined));
  const h = useHistory.getState();
  const [content, setContent] = useState<SetupContent>({ text: true, formatting: true, images: true });
  const names = req.names ?? ["", ""];
  const ranges = req.ranges ?? ["", ""];
  const counts: [number | null, number | null] = [usePageCount(req.doc, req.a, pageCount), usePageCount(req.doc, req.b, pageCount)];
  const fields = [rangeField(ranges[0], counts[0]), rangeField(ranges[1], counts[1])];
  const rangeBad = fields.some((f) => f.error !== null);

  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !(e.target instanceof HTMLSelectElement) && !(e.target instanceof HTMLInputElement)) h.closeCompare();
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const set = (side: "a" | "b", src: Source, name = "") => {
    const i = side === "a" ? 0 : 1;
    const next: [string, string] = [names[0], names[1]];
    next[i] = name;
    // another file has other pages: the range typed for the old one is dropped
    const r: [string, string] = [ranges[0], ranges[1]];
    r[i] = "";
    h.setCompare({ [side]: src, names: next, ranges: r });
  };
  const setRange = (side: "a" | "b", text: string) => h.setCompare({ ranges: side === "a" ? [text, ranges[1]] : [ranges[0], text] });
  const chooseFile = async (side: "a" | "b") => {
    try {
      const f = (await pickOpen(false, "compare"))[0];
      if (f) set(side, { kind: "picked", token: f.token, path: f.path }, baseName(f.path));
    } catch (e) {
      toast(`Could not pick the file: ${String(e)}`);
    }
  };
  const swap = () => h.setCompare({ a: req.b, b: req.a, names: [names[1], names[0]], ranges: [ranges[1], ranges[0]] });

  /** A side with nothing chosen: no document is open and the placeholder is still "current". */
  const unset = (side: "a" | "b") => req.doc === null && (side === "a" ? req.a : req.b).kind === "current";
  const none = !Object.values(content).some(Boolean);
  // the same file is fine when the two sides cover different pages
  const same = !rangeBad && sameSides(req.a, req.b, docPath, [fields[0].pages, fields[1].pages], counts);
  const ready = !unset("a") && !unset("b") && !same && !none && !rangeBad;
  const hint = unset("a") || unset("b")
    ? "Choose both files to compare."
    : same
      ? "Both sides are the same pages of the same file"
      : rangeBad
        ? "Fix the page range."
        : none
          ? "Switch on something to compare."
          : "";

  const go = () => h.setCompare({ stage: undefined, show: setupShow(content) });

  return (
    <div className="hx-compare hx-setup">
      <div className="hx-setup-body">
        <h2>Compare files</h2>
        <p className="dim">Choose the files you want to compare and adjust the settings below.</p>
        <div className="hx-slots">
          <Slot side="a" label="Old file" req={req} versions={versions} docName={docName} pageCount={pageCount} names={names} unset={unset("a")} onSet={set} onFile={chooseFile} range={ranges[0]} rangeError={fields[0].error} count={counts[0]} onRange={setRange} />
          <button className="hx-swap-round" title="Swap old and new" aria-label="Swap old and new" onClick={swap}>
            <Icon name="swap" size={18} />
          </button>
          <Slot side="b" label="New file" req={req} versions={versions} docName={docName} pageCount={pageCount} names={names} unset={unset("b")} onSet={set} onFile={chooseFile} range={ranges[1]} rangeError={fields[1].error} count={counts[1]} onRange={setRange} />
        </div>
        <div className="hx-setup-foot">
          <fieldset className="hx-content">
            <legend>Compare content</legend>
            <div>
              {CONTENT.map((c) => (
                <label key={c.id} className="hx-switch-row compact">
                  <input type="checkbox" role="switch" checked={content[c.id]} onChange={(e) => setContent({ ...content, [c.id]: e.target.checked })} />
                  <span className="hx-switch" aria-hidden />
                  {c.label}
                </label>
              ))}
            </div>
          </fieldset>
          <span className="grow" />
          {hint && (
            <span className="dim" role="status">
              {hint}
            </span>
          )}
          <button className="hx-btn" onClick={() => h.closeCompare()}>
            Cancel
          </button>
          <button className="hx-btn primary hx-go" disabled={!ready} aria-disabled={!ready} onClick={go}>
            Compare
          </button>
        </div>
      </div>
    </div>
  );
}

function Slot({
  side,
  label,
  req,
  versions,
  docName,
  pageCount,
  names,
  unset,
  onSet,
  onFile,
  range,
  rangeError,
  count,
  onRange,
}: {
  side: "a" | "b";
  label: string;
  req: CompareReq;
  versions: VersionInfo[];
  docName: string | undefined;
  pageCount: number | undefined;
  names: [string, string];
  unset: boolean;
  onSet: (side: "a" | "b", src: Source, name?: string) => void;
  onFile: (side: "a" | "b") => void;
  /** The pages to compare as typed, what is wrong with them, and how many pages the file has (null = not known). */
  range: string;
  rangeError: string | null;
  count: number | null;
  onRange: (side: "a" | "b", text: string) => void;
}) {
  const src = side === "a" ? req.a : req.b;
  const how = howOf(src);
  const picked = names[side === "a" ? 0 : 1];
  const v = src.kind === "version" ? versions.find((x) => x.id === src.id) : undefined;
  const title = unset ? "No file chosen" : src.kind === "picked" ? picked || "File" : (docName ?? "Open document");
  const sub = unset
    ? "Choose a PDF to compare"
    : src.kind === "picked"
      ? "Another file"
      : src.kind === "version"
        ? v
          ? `${versionTitle(v)} · ${when(v.created, Date.now())}`
          : "Past version"
        : pageCount
          ? `${pageCount} ${pageCount === 1 ? "page" : "pages"} · open document`
          : "Open document";
  const options: { id: How; label: string }[] =
    req.doc !== null
      ? [
          { id: "doc", label: "This document" },
          ...(versions.length > 0 ? [{ id: "past" as const, label: "Past version" }] : []),
          { id: "file", label: "Another file…" },
        ]
      : [];
  const choose = (id: How) => {
    if (id === "doc") onSet(side, { kind: "current" });
    else if (id === "file") onFile(side);
    else if (versions[0]) onSet(side, { kind: "version", id: (versions.find((x) => sourceKey({ kind: "version", id: x.id }) !== sourceKey(side === "a" ? req.b : req.a)) ?? versions[0]).id });
  };
  return (
    <section className="hx-slot" aria-label={label}>
      <h3>{label}</h3>
      <div className="hx-slot-card">
        <div className="hx-slot-thumb" aria-hidden>
          {!unset && <Thumb doc={req.doc} source={src} page={0} label="First page" />}
        </div>
        <div className="hx-slot-info">
          <strong title={title}>{title}</strong>
          <span className="dim">{sub}</span>
          <button className="hx-link" onClick={() => onFile(side)}>
            {unset ? "Choose…" : "Change"}
          </button>
        </div>
      </div>
      {!unset && (
        <div className="hx-slot-range">
          <label>
            <span>Pages</span>
            <input
              className={rangeError ? "bad" : ""}
              value={range}
              placeholder={count ? `All ${count}, or e.g. 1-3, 5` : "All, or e.g. 1-3, 5"}
              aria-label={`${label}: pages to compare`}
              aria-invalid={rangeError !== null}
              spellCheck={false}
              onChange={(e) => onRange(side, e.target.value)}
            />
          </label>
          {rangeError && (
            <span className="bad" role="status">
              {rangeError}
            </span>
          )}
        </div>
      )}
      {options.length > 0 && (
        <div className="hx-slot-how">
          <div className="hx-seg inline" role="group" aria-label={`${label} is`}>
            {options.map((o) => (
              <button key={o.id} className={how === o.id ? "on" : ""} aria-pressed={how === o.id} onClick={() => choose(o.id)}>
                {o.label}
              </button>
            ))}
          </div>
          {how === "past" && (
            <select
              aria-label={`${label}: past version`}
              value={src.kind === "version" ? src.id : ""}
              onChange={(e) => onSet(side, { kind: "version", id: e.target.value })}
            >
              {versions.map((x) => (
                <option key={x.id} value={x.id}>
                  {versionTitle(x)} {"·"} {when(x.created, Date.now())}
                </option>
              ))}
            </select>
          )}
        </div>
      )}
    </section>
  );
}
