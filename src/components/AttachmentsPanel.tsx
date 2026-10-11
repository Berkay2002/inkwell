import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "../lib/api";
import type { Attachment } from "../lib/api";
import * as edits from "../lib/edit";
import { type DocState, useStore } from "../store";
import { Icon } from "./Icons";
import "./organizePlus.css";

const st = () => useStore.getState();

const sizeText = (n: number) => (n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`);

/**
 * The sidebar's Attachments tab: the files inside the document and the file pins on its pages.
 * A file is only ever saved to a folder the person picks, or (a PDF) opened in a tab of its own;
 * it is never run.
 */
export function AttachmentsPanel({ doc }: { doc: DocState }) {
  const { id, rev } = doc;
  const [list, setList] = useState<Attachment[] | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const live = useRef(true);

  const reload = useCallback(() => {
    api
      .attachmentsList(id)
      .then((l) => live.current && setList(l))
      .catch((e) => st().setUi({ error: String(e) }));
  }, [id]);

  useEffect(() => {
    live.current = true;
    reload();
    return () => {
      live.current = false;
    };
    // a new generation of the document (an edit, undo, redo) may change the list
  }, [reload, rev]);

  const attach = async () => {
    try {
      const files = await api.pickAttachments();
      if (files.length === 0) return;
      await edits.run(id, () => api.attachmentsAdd(id, files.map((f) => f.token)));
    } catch (e) {
      st().setUi({ error: String(e) });
    }
  };

  const saveOut = async (a: Attachment) => {
    try {
      const out = await api.pickSave(a.name, { doc: id }, "any");
      if (!out) return;
      const path = await api.attachmentSave(id, a.id, out.token);
      st().setUi({ error: `Saved ${a.name} to ${path.split(/[\\/]/).slice(-2, -1)[0] ?? "the folder"}` });
    } catch (e) {
      st().setUi({ error: String(e) });
    }
  };

  const openPdf = async (a: Attachment) => {
    try {
      const f = await api.attachmentOpen(id, a.id);
      await st().openPath(f.path, f.token, true);
    } catch (e) {
      st().setUi({ error: String(e) });
    }
  };

  const remove = (a: Attachment) => void edits.run(id, () => api.attachmentsRemove(id, [a.id])).catch((e) => st().setUi({ error: String(e) }));

  const commit = (a: Attachment) => {
    const text = draft.trim();
    setEditing(null);
    if (text === a.description) return;
    void edits.run(id, () => api.attachmentDescribe(id, a.id, text)).catch((e) => st().setUi({ error: String(e) }));
  };

  return (
    <div className="att">
      <div className="att-bar">
        <button className="btn ghost" onClick={() => void attach()}>
          <Icon name="paperclip" size={15} />
          Attach files
        </button>
      </div>
      {list == null ? (
        <div className="empty">Loading…</div>
      ) : list.length === 0 ? (
        <div className="empty">This document has no attachments.</div>
      ) : (
        <ul className="att-list">
          {list.map((a) => (
            <li key={a.id} className="att-row">
              <button
                className="att-name"
                title={a.kind === "annotation" && a.page != null ? `${a.name}: pinned on page ${a.page + 1}. Click to go there.` : a.name}
                disabled={a.kind !== "annotation" || a.page == null}
                onClick={() => a.page != null && st().goTo(id, a.page)}
              >
                <Icon name="paperclip" size={14} />
                <span>{a.name}</span>
              </button>
              <div className="att-sub dim">
                {[a.hasData ? sizeText(a.size) : "no data in this file", a.kind === "annotation" && a.page != null ? `page ${a.page + 1}` : null]
                  .filter(Boolean)
                  .join(" · ")}
                {a.description && editing !== a.id ? ` — ${a.description}` : ""}
              </div>
              {editing === a.id && (
                <input
                  className="att-desc"
                  value={draft}
                  autoFocus
                  placeholder="Description"
                  aria-label={`Description of ${a.name}`}
                  onChange={(e) => setDraft(e.target.value)}
                  onBlur={() => commit(a)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") commit(a);
                    else if (e.key === "Escape") {
                      e.stopPropagation();
                      setEditing(null);
                    }
                  }}
                />
              )}
              <div className="att-acts">
                <button className="ax-icon" title="Save to a folder…" aria-label={`Save ${a.name}`} disabled={!a.hasData} onClick={() => void saveOut(a)}>
                  <Icon name="download" size={15} />
                </button>
                {a.isPdf && a.hasData && (
                  <button className="ax-icon" title="Open in a new tab" aria-label={`Open ${a.name}`} onClick={() => void openPdf(a)}>
                    <Icon name="filePlus" size={15} />
                  </button>
                )}
                <button
                  className="ax-icon"
                  title="Edit description"
                  aria-label={`Edit description of ${a.name}`}
                  onClick={() => {
                    setDraft(a.description);
                    setEditing(a.id);
                  }}
                >
                  <Icon name="edit" size={15} />
                </button>
                <span className="grow" />
                <button className="ax-icon" title="Delete attachment" aria-label={`Delete ${a.name}`} onClick={() => remove(a)}>
                  <Icon name="trash" size={15} />
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
      <p className="att-note dim">Attachments are saved to a folder you pick. Inkwell never runs them.</p>
    </div>
  );
}
