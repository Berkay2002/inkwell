import { message } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState } from "react";
import { useFind } from "../../lib/findStore";
import * as api from "../../lib/api";
import { run as editRun } from "../../lib/edit";
import {
  type FileGroup, FIRST_ITEMS, MORE_ITEMS, type Hit, SOURCE_LABEL, estimateLine, filesLine, folderName, groupByPage, optsKey, resultsLine, shortDir,
  staleLine, staleResultsLine, visibleRows,
} from "../../lib/searchLogic";
import { useSearch } from "../../lib/searchStore";
import { type DocState, useStore } from "../../store";
import { Icon } from "../Icons";
import { OptionChips, OptionsMenu } from "./FindOptions";
import "./search.css";

const st = () => useStore.getState();
const sr = () => useSearch.getState();

/** Runs the search of the open document as the text or an option changes, whichever of the find box or the rail it came from. */
export function SearchDriver({ doc }: { doc: DocState }) {
  const { id } = doc;
  const draft = useFind((s) => s.drafts[id]);
  const key = useFind((s) => optsKey(s.opts));
  const ran = useRef(key);
  useEffect(() => {
    const text = draft ?? doc.search.query;
    const optionChanged = ran.current !== key;
    if (!optionChanged && text.trim() === doc.search.query.trim()) return;
    const t = setTimeout(() => {
      ran.current = key;
      st().runSearch(id, text);
    }, optionChanged ? 0 : 220);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft, key, id]);
  return null;
}

/** "…before **match** after…": the hit in bold on its amber wash. */
function Snippet({ hit }: { hit: Hit }) {
  return (
    <span className="sr-snip">
      {hit.before && <span>…{hit.before}</span>}
      <mark>{hit.text}</mark>
      {hit.after && <span>{hit.after}…</span>}
    </span>
  );
}

const ROWS_AT_FIRST = 300;

function FileResults({ doc, draft }: { doc: DocState; draft: string }) {
  const { id, search } = doc;
  const [limit, setLimit] = useState(ROWS_AT_FIRST);
  const list = useRef<HTMLDivElement>(null);
  // A new search starts from the top again.
  useEffect(() => setLimit(ROWS_AT_FIRST), [search.id]);
  const groups = useMemo(() => groupByPage(search.hits), [search.hits]);
  const shown = Math.max(limit, search.current + 50);
  const pages = groups.length;

  useEffect(() => {
    list.current?.querySelector(`[data-i="${search.current}"]`)?.scrollIntoView({ block: "nearest" });
  }, [search.current]);

  const n = search.hits.length;
  const running = !search.done;
  const pct = search.total > 0 ? Math.min(100, (search.scanned / search.total) * 100) : 0;
  let rows = 0;

  return (
    <>
      <div className="sr-status" aria-live="polite">
        <span>{search.query ? resultsLine(n, pages, search.done) : "Type to search this file"}</span>
        <span className="grow" />
        {running && (
          <button className="sr-link" onClick={() => st().stopSearch(id)}>
            Cancel
          </button>
        )}
      </div>
      {running && (
        <div className="sr-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)} aria-label="Searching">
          <i style={{ width: `${pct}%` }} />
        </div>
      )}
      {search.done && search.capped && <p className="sr-note">Showing the first {n.toLocaleString("en-US")} results. Narrow the search to see the rest.</p>}
      <div
        ref={list}
        className="sr-list"
        tabIndex={0}
        role="listbox"
        aria-label="Search results"
        onKeyDown={(e) => {
          if (n === 0) return;
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            st().stepHit(id, e.key === "ArrowDown" ? 1 : -1);
          } else if (e.key === "Home" || e.key === "End") {
            e.preventDefault();
            st().selectHit(id, e.key === "Home" ? 0 : n - 1);
          } else if (e.key === "Enter") {
            useFind.getState().remember(draft || search.query);
          }
        }}
      >
        {groups.map((g) => {
          if (rows >= shown) return null;
          const items = g.items.slice(0, Math.max(0, shown - rows));
          rows += items.length;
          return (
            <section key={g.page} className="sr-group">
              <h4>
                <button className="sr-page" onClick={() => st().goTo(id, g.page)}>
                  Page {g.page + 1}
                </button>
                <span className="dim">{g.items.length}</span>
              </h4>
              {items.map(({ index, hit }) => (
                <button
                  key={index}
                  data-i={index}
                  role="option"
                  aria-selected={index === search.current}
                  className={index === search.current ? "sr-row cur" : "sr-row"}
                  tabIndex={-1}
                  onClick={() => {
                    useFind.getState().remember(draft || search.query);
                    st().selectHit(id, index);
                  }}
                >
                  <Snippet hit={hit} />
                  {SOURCE_LABEL[hit.source] && <span className="sr-src">{SOURCE_LABEL[hit.source]}</span>}
                </button>
              ))}
            </section>
          );
        })}
        {rows < n && (
          <button className="sr-link more" onClick={() => setLimit(shown + ROWS_AT_FIRST)}>
            Show {Math.min(ROWS_AT_FIRST, n - rows).toLocaleString("en-US")} more
          </button>
        )}
      </div>
    </>
  );
}

function FolderResults({ query }: { query: string }) {
  const run = useSearch((s) => s.run);
  const shown = useSearch((s) => s.shown);
  const [cursor, setCursor] = useState(0);
  const list = useRef<HTMLDivElement>(null);
  const files = run?.files ?? [];
  const rows = useMemo(() => visibleRows(files, shown), [files, shown]);

  useEffect(() => setCursor(0), [run?.id]);
  useEffect(() => {
    list.current?.querySelector(`[data-cur="1"]`)?.scrollIntoView({ block: "nearest" });
  }, [cursor, rows.length]);

  if (!run) return <p className="sr-note">Choose a folder or some PDFs, then search them. Inkwell reads each file in turn and lists where the words are.</p>;
  const pct = run.total > 0 ? Math.min(100, (run.scanned / run.total) * 100) : 0;
  const open = (g: FileGroup, item: number) => void sr().openAt(g, item);
  const at = rows[Math.min(cursor, Math.max(0, rows.length - 1))];

  return (
    <>
      <div className="sr-status" aria-live="polite">
        <span>
          {run.running && run.total === 0
            ? "Listing files…"
            : filesLine(run.matched || files.length, run.hits || files.reduce((a, f) => a + f.hits, 0), run.scanned, run.total, !run.running, run.cancelled)}
        </span>
        <span className="grow" />
        {run.running && (
          <button className="sr-link" onClick={() => sr().cancelFolder()}>
            Cancel
          </button>
        )}
      </div>
      {run.running && (
        <div className="sr-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)} aria-label="Searching files">
          <i style={{ width: `${pct}%` }} />
        </div>
      )}
      {!run.running && run.skipped > 0 && (
        <p className="sr-note">
          {run.skipped} {run.skipped === 1 ? "file" : "files"} could not be read (damaged, or protected with a password).
        </p>
      )}
      {!run.running && run.stale > 0 && <p className="sr-note">{staleResultsLine(run.stale)}</p>}
      {!run.running && run.cloudOnly > 0 && (
        <p className="sr-note">
          {run.cloudOnly} cloud-only {run.cloudOnly === 1 ? "file" : "files"} skipped (not downloaded to this PC, so not read).
        </p>
      )}
      {run.capped && <p className="sr-note">The folder has more PDFs than one search reads; only the first {run.total.toLocaleString("en-US")} were searched.</p>}
      <div
        ref={list}
        className="sr-list"
        tabIndex={0}
        role="listbox"
        aria-label="Results by file"
        onKeyDown={(e) => {
          if (rows.length === 0) return;
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            setCursor((c) => (c + (e.key === "ArrowDown" ? 1 : -1) + rows.length) % rows.length);
          } else if (e.key === "Enter" && at) {
            e.preventDefault();
            open(files[at.file], at.item);
          }
        }}
      >
        {files.map((g, fi) => {
          const count = Math.min(g.items.length, shown[g.index] ?? FIRST_ITEMS);
          const more = g.items.length - count;
          const unlisted = g.hits - g.items.length;
          return (
            <section key={g.index} className="sr-group file">
              <h4>
                <button className="sr-file" title={g.path} onClick={() => open(g, 0)}>
                  <Icon name="file" size={14} />
                  <span className="sr-fname">{g.name}</span>
                </button>
                <span className="dim">{g.hits}</span>
              </h4>
              <div className="sr-dir dim">{shortDir(g.path) || "—"}</div>
              {g.items.slice(0, count).map((hit, i) => {
                const isCur = at?.file === fi && at.item === i;
                return (
                  <button
                    key={i}
                    role="option"
                    aria-selected={isCur}
                    data-cur={isCur ? "1" : undefined}
                    tabIndex={-1}
                    className={isCur ? "sr-row cur" : "sr-row"}
                    onClick={() => {
                      setCursor(rows.findIndex((r) => r.file === fi && r.item === i));
                      open(g, i);
                    }}
                  >
                    <span className="sr-pg">Page {hit.page + 1}</span>
                    <Snippet hit={hit} />
                    {SOURCE_LABEL[hit.source] && <span className="sr-src">{SOURCE_LABEL[hit.source]}</span>}
                  </button>
                );
              })}
              {more > 0 && (
                <button className="sr-link more" onClick={() => sr().showMore(g.index)}>
                  Show {Math.min(MORE_ITEMS, more)} more
                </button>
              )}
              {more === 0 && unlisted > 0 && (
                <button className="sr-link more" onClick={() => open(g, 0)}>
                  {unlisted.toLocaleString("en-US")} more in this file: open it
                </button>
              )}
            </section>
          );
        })}
      </div>
      {!run.running && files.length === 0 && <p className="sr-note">Nothing found for “{query}”.</p>}
    </>
  );
}

/** The saved catalogs of the Folder scope: pick one to search it, refresh or delete it, or make a new one. */
function Catalogs({ disabled }: { disabled: boolean }) {
  /** Asks first, naming the catalog and how many files its index holds (the PDFs themselves are never touched). */
  const remove = async (c: api.CatalogMeta) => {
    const files = `${c.files.toLocaleString("en-US")} ${c.files === 1 ? "file" : "files"}`;
    const ok = await message(`Delete the catalog “${c.name}”? Its search index of ${files} is removed. The PDFs are not touched.`, {
      title: "Inkwell",
      kind: "warning",
      buttons: { ok: "Delete", cancel: "Cancel" },
    });
    if (ok === "Ok" || ok === "Delete") void sr().removeCatalog(c.id);
  };
  const catalogs = useSearch((s) => s.catalogs);
  const stale = useSearch((s) => s.stale);
  const target = useSearch((s) => s.target);
  const build = useSearch((s) => s.build);
  const draft = useSearch((s) => s.draft);
  useEffect(() => void sr().loadCatalogs(), []);
  const busy = build?.running ?? false;
  const pct = build && build.total > 0 ? Math.min(100, (build.done / build.total) * 100) : 0;

  return (
    <div className="sr-cats">
      {catalogs.map((c) => {
        const on = target?.kind === "catalog" && target.id === c.id;
        const badge = staleLine(stale[c.id], c.built);
        return (
          <div key={c.id} className={on ? "sr-cat on" : "sr-cat"}>
            <button className="sr-catname" aria-pressed={on} disabled={disabled} title={c.folder} onClick={() => sr().selectCatalog(c.id)}>
              <Icon name="book" size={14} />
              <span className="sr-fname">Catalog: {c.name}</span>
            </button>
            {badge && <span className="sr-stale" title="What a refresh would change">{badge}</span>}
            <button className="ax-icon" title="Refresh" aria-label={`Refresh ${c.name}`} disabled={busy || disabled} onClick={() => void sr().buildCatalog(c.id)}>
              <Icon name="rotate" size={14} />
            </button>
            <button className="ax-icon" title="Delete" aria-label={`Delete ${c.name}`} disabled={busy || disabled} onClick={() => void remove(c)}>
              <Icon name="trash" size={14} />
            </button>
          </div>
        );
      })}
      {build && (
        <>
          <div className="sr-status" aria-live="polite">
            <span>{build.running ? (build.total ? `Indexing ${build.done.toLocaleString("en-US")} of ${build.total.toLocaleString("en-US")} files` : "Listing files…") : build.line}</span>
            <span className="grow" />
            {build.running && (
              <button className="sr-link" disabled={build.job == null} onClick={() => sr().cancelBuild()}>
                Cancel
              </button>
            )}
          </div>
          {build.running && (
            <div className="sr-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)} aria-label="Indexing">
              <i style={{ width: `${pct}%` }} />
            </div>
          )}
        </>
      )}
      {draft ? (
        <div className="sr-newcat">
          <div className="sr-folderrow">
            <Icon name="open" size={14} />
            <span className="sr-fname" title={draft.path}>{folderName(draft.path)}</span>
          </div>
          <input
            className="sr-catinput"
            value={draft.name}
            aria-label="Catalog name"
            placeholder="Catalog name"
            spellCheck={false}
            onChange={(e) => sr().patchDraft({ name: e.target.value })}
          />
          <label className="sr-switch">
            <input type="checkbox" checked={draft.recursive} onChange={(e) => sr().patchDraft({ recursive: e.target.checked })} />
            <span>Include sub-folders</span>
          </label>
          <p className="sr-note flush" aria-live="polite">{draft.estimate ? estimateLine(draft.estimate) : "Counting files…"}</p>
          {draft.estimate?.capped && <p className="sr-note flush">The folder has more PDFs than a catalog holds; the first ones are indexed.</p>}
          <div className="sr-pickrow">
            <button className="sr-pick" onClick={() => sr().dropDraft()}>
              Cancel
            </button>
            <button className="sr-go" disabled={busy || !draft.estimate || draft.estimate.files === 0} onClick={() => void sr().createCatalog()}>
              Build catalog
            </button>
          </div>
        </div>
      ) : (
        <button className="sr-pick" disabled={busy || disabled} onClick={() => void sr().newCatalog()}>
          <Icon name="plus" size={14} /> New catalog…
        </button>
      )}
    </div>
  );
}

/** Puts the search index of the open document into the file itself, or takes it out. */
function EmbeddedIndex({ doc }: { doc: DocState }) {
  const [info, setInfo] = useState<api.Embedded | null>(null);
  const [busy, setBusy] = useState(false);
  // The file changes with every edit (rev), so the status is read again.
  useEffect(() => {
    let live = true;
    api.indexEmbedded(doc.id).then((e) => live && setInfo(e), () => {});
    return () => {
      live = false;
    };
  }, [doc.id, doc.rev]);
  if (!info) return null;
  const change = async (f: () => Promise<api.EditState>) => {
    setBusy(true);
    try {
      await editRun(doc.id, f);
    } catch (e) {
      st().setUi({ error: String(e) });
    } finally {
      setBusy(false);
    }
  };
  const note =
    info.blocked ?? (!info.present ? "No search index in this file." : info.current ? "This file carries a search index." : "The index in this file is out of date.");
  const off = busy || info.blocked !== null;
  return (
    <div className="sr-embed">
      <span className="dim">{note}</span>
      <span className="grow" />
      <button className="sr-link" disabled={off} title={info.blocked ?? undefined} onClick={() => void change(() => api.indexEmbed(doc.id))}>
        {info.present ? "Update" : "Embed search index"}
      </button>
      {info.present && (
        <button className="sr-link" disabled={off} title={info.blocked ?? undefined} onClick={() => void change(() => api.indexUnembed(doc.id))}>
          Remove embedded index
        </button>
      )}
    </div>
  );
}

function History({ onPick, onClose }: { onPick: (q: string) => void; onClose: () => void }) {
  const history = useFind((s) => s.history);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    box.current?.querySelector<HTMLElement>("button")?.focus();
    const away = (e: MouseEvent) => {
      if (!box.current?.contains(e.target as Node) && !(e.target as HTMLElement).closest?.(".sr-hist-btn")) onClose();
    };
    window.addEventListener("mousedown", away, true);
    return () => window.removeEventListener("mousedown", away, true);
  }, [onClose]);
  return (
    <div
      ref={box}
      className="sr-history"
      role="menu"
      aria-label="Recent searches"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          onClose();
        } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          const all = [...box.current!.querySelectorAll<HTMLElement>("button.sr-h")];
          const i = all.indexOf(document.activeElement as HTMLElement);
          all[(i + (e.key === "ArrowDown" ? 1 : -1) + all.length) % all.length]?.focus();
        }
      }}
    >
      {history.length === 0 ? (
        <p className="sr-note">No recent searches yet.</p>
      ) : (
        <>
          {history.map((q) => (
            <button key={q} role="menuitem" className="sr-h" onClick={() => onPick(q)}>
              <Icon name="clock" size={14} />
              <span className="sr-hq">{q}</span>
            </button>
          ))}
          <hr />
          <button role="menuitem" className="sr-h clear" onClick={() => useFind.getState().clearHistory()}>
            Clear recent searches
          </button>
        </>
      )}
    </div>
  );
}

/** Advanced search: the right rail. This file's hits grouped by page, or every PDF in a folder grouped by file. */
export function SearchPanel({ doc }: { doc: DocState }) {
  const open = useSearch((s) => s.open);
  const scope = useSearch((s) => s.scope);
  const target = useSearch((s) => s.target);
  const recursive = useSearch((s) => s.recursive);
  const running = useSearch((s) => s.run?.running ?? false);
  const draft = useFind((s) => s.drafts[doc.id] ?? doc.search.query);
  const [hist, setHist] = useState(false);
  const input = useRef<HTMLInputElement>(null);

  // The field takes the focus when the rail opens.
  useEffect(() => {
    if (open) {
      input.current?.focus();
      input.current?.select();
    }
  }, [open]);

  if (!open) return null;
  const setDraft = (t: string) => useFind.getState().setDraft(doc.id, t);
  const built = useSearch((s) => target?.kind !== "catalog" || (s.catalogs.find((c) => c.id === target.id)?.built ?? 0) > 0);
  const canRun = scope === "folder" && !!target && built && draft.trim().length > 0;
  const runFolder = () => void sr().startFolder(draft);
  const choose = async (files: boolean) => {
    if (await (files ? sr().pickFiles() : sr().pickFolder())) {
      if (draft.trim()) runFolder();
    }
  };

  return (
    <aside className="ax-dock sr-dock" style={{ width: "min(360px, 45vw)" }} aria-label="Search">
      <section className="ax-panel docked sr-panel">
        <div className="ax-head">
          <span className="ax-title">Search</span>
          <span className="grow" />
          <button className="ax-icon" title="Close" aria-label="Close Search" onClick={() => sr().setOpen(false)}>
            <Icon name="close" size={16} />
          </button>
        </div>

        <div className="sr-fieldrow">
          <div className="sr-field">
            <Icon name="search" size={14} />
            <input
              ref={input}
              value={draft}
              placeholder={scope === "folder" ? "Words to look for in the folder" : "Find in this file"}
              spellCheck={false}
              aria-label="Search for"
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  if (scope === "folder") {
                    if (canRun) runFolder();
                  } else {
                    useFind.getState().remember(draft);
                    if (draft.trim() !== doc.search.query.trim()) st().runSearch(doc.id, draft);
                    else st().stepHit(doc.id, e.shiftKey ? -1 : 1);
                  }
                } else if (e.key === "ArrowDown" && !draft) {
                  setHist(true);
                }
              }}
            />
            <button className="icon-btn sr-hist-btn" title="Recent searches" aria-label="Recent searches" aria-expanded={hist} onClick={() => setHist(!hist)}>
              <Icon name="clock" size={14} />
            </button>
          </div>
          <OptionChips />
          <OptionsMenu advanced={false} />
          {hist && (
            <History
              onClose={() => setHist(false)}
              onPick={(q) => {
                setDraft(q);
                setHist(false);
                input.current?.focus();
                if (scope === "folder" && target) void sr().startFolder(q);
              }}
            />
          )}
        </div>

        <div className="sr-seg" role="tablist" aria-label="Where to search">
          <button role="tab" aria-selected={scope === "file"} className={scope === "file" ? "on" : ""} onClick={() => sr().setScope("file")}>
            This file
          </button>
          <button role="tab" aria-selected={scope === "folder"} className={scope === "folder" ? "on" : ""} onClick={() => sr().setScope("folder")}>
            Folder
          </button>
        </div>

        {scope === "folder" && (
          <div className="sr-folder">
            <Catalogs disabled={running} />
            {target && target.kind !== "catalog" && (
              <div className="sr-folderrow">
                <Icon name={target.kind === "folder" ? "open" : "file"} size={14} />
                <span className="sr-fname" title={target.kind === "folder" ? target.path : target.paths.join("\n")}>
                  {target.kind === "folder" ? folderName(target.path) : `${target.tokens.length} ${target.tokens.length === 1 ? "file" : "files"}`}
                </span>
              </div>
            )}
            <div className="sr-pickrow">
              <button className="sr-pick" onClick={() => void choose(false)} disabled={running}>
                <Icon name="open" size={14} /> {target?.kind === "folder" ? "Change folder…" : "Choose a folder…"}
              </button>
              <button className="sr-pick" onClick={() => void choose(true)} disabled={running}>
                <Icon name="file" size={14} /> {target?.kind === "files" ? "Change files…" : "Choose files…"}
              </button>
            </div>
            {(!target || target.kind === "folder") && (
              <label className="sr-switch">
                <input type="checkbox" checked={recursive} disabled={running} onChange={(e) => sr().setRecursive(e.target.checked)} />
                <span>Include sub-folders</span>
              </label>
            )}
            <button className="sr-go" disabled={!canRun || running} onClick={runFolder}>
              {target?.kind === "catalog" ? "Search catalog" : "Search folder"}
            </button>
          </div>
        )}

        {scope === "file" && <EmbeddedIndex doc={doc} />}
        <div className="sr-body">{scope === "file" ? <FileResults doc={doc} draft={draft} /> : <FolderResults query={draft} />}</div>
      </section>
    </aside>
  );
}
