import { useEffect, useMemo, useState } from "react";
import { useStamps } from "../../lib/stamps";
import { DEFAULT_CATEGORY, MAX_CATEGORY, MAX_NAME, MAX_STAMPS, byCategory, categoriesOf } from "../../lib/stampsLogic";
import { Icon } from "../Icons";
import "./stamps.css";

const act = () => useStamps.getState();

/**
 * Custom stamps: make one from a picture or a PDF page, rename it, move it to another category, delete it. Opened
 * from the stamp picker and the command palette. The library is Inkwell's own, so this works with no document open
 * and on a certified one.
 */
export function StampsManager() {
  const open = useStamps((s) => s.managerOpen);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || (e.target as HTMLElement | null)?.closest("input")) return;
      e.stopPropagation();
      act().closeManager();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open]);
  if (!open) return null;
  return <Sheet />;
}

function Sheet() {
  const stamps = useStamps((s) => s.stamps);
  const loaded = useStamps((s) => s.loaded);
  const busy = useStamps((s) => s.busy);
  const error = useStamps((s) => s.error);
  const draft = useStamps((s) => s.draft);
  const groups = useMemo(() => byCategory(stamps), [stamps]);
  const full = stamps.length >= MAX_STAMPS;
  return (
    <div className="scrim st-scrim" onMouseDown={() => act().closeManager()}>
      <section className="st-sheet" role="dialog" aria-label="Custom stamps" onMouseDown={(e) => e.stopPropagation()}>
        <header className="st-head">
          <Icon name="stamp" size={16} />
          <span className="st-title">Custom stamps</span>
          <span className="dim">{stamps.length}</span>
          <span className="grow" />
          <button className="btn primary" disabled={busy || full || !!draft} title={full ? `The library holds ${MAX_STAMPS} stamps` : undefined} onClick={() => void act().pick()}>
            Add from picture or PDF…
          </button>
          <button className="ax-icon" title="Close" aria-label="Close Custom stamps" onClick={() => act().closeManager()}>
            <Icon name="close" size={14} />
          </button>
        </header>
        {draft && <DraftForm categories={categoriesOf(stamps)} />}
        {error && (
          <p className="st-error" role="alert">
            {error}
          </p>
        )}
        <div className="st-body">
          {loaded && stamps.length === 0 && !draft && (
            <p className="dim st-empty">
              No custom stamps yet. A stamp is a picture or a page of a PDF (a logo, a signature, "Paid") that you place on pages like the
              standard ones. They are kept on this computer, not in your documents.
            </p>
          )}
          {groups.map((g) => (
            <section key={g.category} className="st-group">
              <h3 className="st-cat">{g.category}</h3>
              {g.stamps.map((s) => (
                <Row key={s.id} id={s.id} />
              ))}
            </section>
          ))}
        </div>
      </section>
    </div>
  );
}

function DraftForm({ categories }: { categories: string[] }) {
  const d = useStamps((s) => s.draft)!;
  const busy = useStamps((s) => s.busy);
  const set = act().setDraft;
  return (
    <form
      className="st-draft"
      onSubmit={(e) => {
        e.preventDefault();
        void act().make();
      }}
    >
      <div className="st-file dim" title={d.path}>
        {d.path}
      </div>
      <label>
        Name
        <input className="cm-input" value={d.name} maxLength={MAX_NAME} autoFocus onChange={(e) => set({ name: e.target.value })} />
      </label>
      <label>
        Category
        <input className="cm-input" list="st-cats" value={d.category} maxLength={MAX_CATEGORY} placeholder={DEFAULT_CATEGORY} onChange={(e) => set({ category: e.target.value })} />
        <datalist id="st-cats">
          {categories.map((c) => (
            <option key={c} value={c} />
          ))}
        </datalist>
      </label>
      {d.pdf && (
        <label>
          Page
          <input className="cm-input st-page" inputMode="numeric" value={d.page} onChange={(e) => set({ page: e.target.value })} />
        </label>
      )}
      <div className="st-actions">
        <button type="button" className="btn ghost" disabled={busy} onClick={() => act().cancelDraft()}>
          Cancel
        </button>
        <button type="submit" className="btn primary" disabled={busy}>
          Make stamp
        </button>
      </div>
    </form>
  );
}

function Row({ id }: { id: string }) {
  const s = useStamps((x) => x.stamps.find((y) => y.id === id));
  const thumb = useStamps((x) => x.thumbs[id]);
  const [name, setName] = useState(s?.name ?? "");
  const [category, setCategory] = useState(s?.category ?? "");
  const [confirm, setConfirm] = useState(false);
  useEffect(() => {
    void act().thumb(id);
  }, [id]);
  useEffect(() => {
    setName(s?.name ?? "");
    setCategory(s?.category ?? "");
  }, [s?.name, s?.category]);
  if (!s) return null;
  const commit = (field: "name" | "category") => {
    if (field === "name") void act().rename(id, name).then(() => setName(act().stamps.find((x) => x.id === id)?.name ?? s.name));
    else void act().refile(id, category).then(() => setCategory(act().stamps.find((x) => x.id === id)?.category ?? s.category));
  };
  return (
    <div className="st-row">
      <div className="st-thumb">{thumb ? <img src={thumb} alt="" /> : null}</div>
      <input
        className="cm-input st-name"
        aria-label={`Name of ${s.name}`}
        value={name}
        maxLength={MAX_NAME}
        onChange={(e) => setName(e.target.value)}
        onBlur={() => commit("name")}
        onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
      />
      <input
        className="cm-input st-category"
        aria-label={`Category of ${s.name}`}
        value={category}
        maxLength={MAX_CATEGORY}
        onChange={(e) => setCategory(e.target.value)}
        onBlur={() => commit("category")}
        onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
      />
      <span className="dim st-size">
        {s.width} × {s.height}
      </span>
      {confirm ? (
        <>
          <button className="btn" onClick={() => void act().remove(id)}>
            Delete
          </button>
          <button className="btn ghost" onClick={() => setConfirm(false)}>
            Keep
          </button>
        </>
      ) : (
        <button className="ax-icon" title="Delete stamp" aria-label={`Delete ${s.name}`} onClick={() => setConfirm(true)}>
          <Icon name="trash" size={15} />
        </button>
      )}
    </div>
  );
}
