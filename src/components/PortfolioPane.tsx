import { useMemo } from "react";
import type { PortfolioInfo } from "../lib/api";
import { openAction, paneFields, tilesAt } from "../lib/portfolioLogic";
import { extractFile, openFile } from "../lib/portfolioStore";
import { Icon } from "./Icons";
import { FileThumb } from "./PortfolioTiles";

/** Pixels of the longest side asked for the pane's picture. */
const PANE_SIDE = 560;

/**
 * The preview beside the list, for the selected file: a larger first page of a PDF (the same caps as the tiles),
 * or the file's icon, with every visible field and its value, and Open and Extract. Nothing is launched: a PDF
 * opens as a document in a tab, any other file is only ever written to a folder the person picks.
 */
export function PortfolioPane({ id, info, selected }: { id: number; info: PortfolioInfo; selected: string | null }) {
  const file = useMemo(() => info.files.find((f) => f.id === selected), [info, selected]);
  const dir = useMemo(() => (selected?.startsWith("d:") ? info.folders.find((f) => `d:${f.id}` === selected) : undefined), [info, selected]);
  const inside = useMemo(() => {
    if (!dir) return 0;
    const row = tilesAt(info, [], dir.parent).find((r) => r.kind === "folder" && r.folder.id === dir.id);
    return row?.kind === "folder" ? row.count : 0;
  }, [info, dir]);

  return (
    <aside className="pf-pane" aria-label="Preview">
      {file ? (
        <>
          <div className="pf-pane-pic">
            <FileThumb doc={id} file={file} side={PANE_SIDE} version={info} glyph={48} />
          </div>
          <h2 className="pf-pane-name" title={file.name}>
            {file.name}
          </h2>
          <dl className="pf-fields">
            {paneFields(info, file).map((f) => (
              <div key={f.key} className="pf-field">
                <dt>{f.label}</dt>
                <dd className={f.number ? "num" : undefined}>{f.value || "-"}</dd>
              </div>
            ))}
          </dl>
          <div className="pf-pane-actions">
            {openAction(file) === "open" && (
              <button className="btn" onClick={() => void openFile(id, file)}>
                <Icon name="filePlus" size={15} />
                Open
              </button>
            )}
            <button className="btn ghost" onClick={() => void extractFile(id, file)}>
              <Icon name="download" size={15} />
              Extract…
            </button>
          </div>
        </>
      ) : dir ? (
        <>
          <div className="pf-pane-pic">
            <div className="pf-glyph">
              <Icon name="open" size={48} />
            </div>
          </div>
          <h2 className="pf-pane-name" title={dir.name}>
            {dir.name}
          </h2>
          <p className="pf-pane-hint">
            Folder, {inside} {inside === 1 ? "file" : "files"}
          </p>
        </>
      ) : (
        <p className="pf-pane-hint">Select a file to see it here.</p>
      )}
    </aside>
  );
}
