import { useEffect, useState } from "react";
import type { ImageBlock } from "../../lib/api";
import { codecName, colorName, imageLabel, ppi, rectH, rectW, useImageEdit } from "../../lib/imageEdit";
import { type DocState } from "../../store";
import { Icon } from "../Icons";
import "./imageEdit.css";

const ie = () => useImageEdit.getState();

/** "Image" in Add content: choose a picture, then put it on a page by clicking or dragging a box. */
export function AddImageButton({ id }: { id: number }) {
  const placing = useImageEdit((s) => s.docs[id]?.placing ?? null);
  return (
    <button
      className={placing ? "te-btn on" : "te-btn"}
      aria-pressed={placing != null}
      title="Choose a picture, then click or drag on the page to put it there"
      onClick={() => (placing ? ie().cancelPlacing(id) : void ie().startPlacing(id))}
    >
      <Icon name="area" size={16} />
      Image
    </button>
  );
}

/** The rail's part for pictures: what is being placed, the selected image's controls, the page's list. */
export function ImageSection({ doc }: { doc: DocState }) {
  const id = doc.id;
  const d = useImageEdit((s) => s.docs[id]);
  const placing = d?.placing ?? null;
  const selected = d?.selected ? (d.pages[d.selected.page]?.images[d.selected.index] ?? null) : null;
  const here = d?.pages[doc.currentPage]?.images ?? [];

  // Esc puts a chosen file back (the page's own keys are in the layer).
  useEffect(() => {
    if (!placing) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        ie().cancelPlacing(id);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [placing, id]);

  if (!placing && !selected && here.length === 0 && !d?.notice) return null;
  return (
    <section className="te-sec" aria-label="Images">
      <h3 className="te-h3">{selected ? "Format image" : placing ? "Add image" : `Images on page ${doc.currentPage + 1}`}</h3>
      {placing && (
        <>
          <p className="te-summary">
            <b>{placing.name}</b>
            <br />
            <span className="dim">Click the page to put it there, or drag a box for it. Esc puts it back.</span>
          </p>
          <div className="te-row">
            <button className="te-btn" onClick={() => ie().cancelPlacing(id)}>
              Cancel
            </button>
          </div>
        </>
      )}
      {!placing && selected && <Format key={`${d?.selected?.page}:${selected.index}`} id={id} image={selected} cropping={d?.crop != null} />}
      {!placing && !selected && here.length > 0 && (
        <>
          <ul className="te-list">
            {here.map((b, n) => (
              <li key={b.index}>
                <button
                  className="te-item"
                  onClick={() =>
                    b.editable
                      ? ie().select(id, { page: doc.currentPage, index: b.index })
                      : ie().setNotice(id, b.reason ? `This image can't be edited here: ${b.reason}.` : "This image can't be edited here.")
                  }
                >
                  <span className="te-excerpt">
                    Image {n + 1} · {imageLabel(b)}
                  </span>
                  {!b.editable && <span className="te-tag">locked</span>}
                </button>
              </li>
            ))}
          </ul>
          <p className="dim te-hint">Click a picture on the page to turn, crop, replace or delete it.</p>
        </>
      )}
      {d?.notice && (
        <p className="te-note" role="status">
          {d.notice}
        </p>
      )}
    </section>
  );
}

const round1 = (v: number) => Math.round(v * 10) / 10;

function Format({ id, image, cropping }: { id: number; image: ImageBlock; cropping: boolean }) {
  const w = rectW(image.fullRect);
  const h = rectH(image.fullRect);
  const [width, setWidth] = useState(String(round1(w)));
  const [height, setHeight] = useState(String(round1(h)));
  const [keep, setKeep] = useState(true);
  useEffect(() => {
    setWidth(String(round1(w)));
    setHeight(String(round1(h)));
  }, [w, h]);

  /** Sends the typed size (the top left stays where it is); a value that is not a size puts the old one back. */
  const sendSize = (which: "w" | "h") => {
    const v = parseFloat(which === "w" ? width : height);
    if (!Number.isFinite(v) || v < 1 || v > 14400) {
      setWidth(String(round1(w)));
      setHeight(String(round1(h)));
      return;
    }
    const nw = which === "w" ? v : keep ? v * (w / Math.max(h, 1e-6)) : w;
    const nh = which === "h" ? v : keep ? v * (h / Math.max(w, 1e-6)) : h;
    if (Math.abs(nw - w) < 0.05 && Math.abs(nh - h) < 0.05) {
      setWidth(String(round1(w)));
      setHeight(String(round1(h)));
      return;
    }
    const [x, y] = image.fullRect;
    void ie().resizeTo(id, [x, y, x + nw, y + nh], false);
  };

  const crop = useImageEdit((s) => s.docs[id]?.crop ?? null);
  const dpi = Math.round(ppi(image));
  const note = image.masked ? "Has a transparency mask." : null;

  return (
    <div className="te-format">
      <p className="te-summary">
        <b>
          {image.width} × {image.height} px
        </b>
        <br />
        <span className="dim">
          {codecName(image.filter)} · {colorName(image.colorSpace)}
          {dpi > 0 ? ` · ${dpi} ppi` : ""}
          {image.kind === "inline" ? " · inline" : ""}
        </span>
      </p>
      {cropping && crop ? (
        <div className="ie-crop-rail">
          <p className="dim te-hint">Drag the handles over the picture to set the part that stays, then apply.</p>
          <label className="ie-check">
            <input type="checkbox" checked={crop.pixels} onChange={(e) => ie().setCropPixels(id, e.target.checked)} />
            Delete the cut-off pixels (smaller file)
          </label>
          <p className="dim te-hint">
            {crop.pixels
              ? "The part outside is removed from the file once it is saved."
              : "The part outside is only hidden: it stays in the file and can be shown again by editing."}
          </p>
          <div className="te-row">
            <button className="te-btn ie-primary" onClick={() => void ie().applyCrop(id)}>
              Apply crop
            </button>
            <button className="te-btn" onClick={() => ie().cancelCrop(id)}>
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <>
          <div className="te-row">
            <span className="te-label">Turn</span>
            <span className="ie-tools">
              <button className="te-btn ie-icon" title="Turn 90° to the left" aria-label="Turn left" onClick={() => void ie().rotate(id, -1)}>
                <Icon name="rotateLeft" size={16} />
              </button>
              <button className="te-btn ie-icon" title="Turn 90° to the right" aria-label="Turn right" onClick={() => void ie().rotate(id, 1)}>
                <Icon name="rotate" size={16} />
              </button>
            </span>
          </div>
          <div className="te-row">
            <span className="te-label">Flip</span>
            <span className="ie-tools">
              <button className="te-btn ie-icon" title="Flip left to right" aria-label="Flip horizontally" onClick={() => void ie().flip(id, true)}>
                <Icon name="flipH" size={16} />
              </button>
              <button className="te-btn ie-icon" title="Flip top to bottom" aria-label="Flip vertically" onClick={() => void ie().flip(id, false)}>
                <Icon name="flipV" size={16} />
              </button>
            </span>
          </div>
          <div className="te-row">
            <span className="te-label">Size</span>
            <span className="ie-size">
              <input
                className="ie-num te-keep"
                aria-label="Width in points"
                inputMode="decimal"
                value={width}
                onChange={(e) => setWidth(e.target.value)}
                onBlur={() => sendSize("w")}
                onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
              />
              <span className="dim">×</span>
              <input
                className="ie-num te-keep"
                aria-label="Height in points"
                inputMode="decimal"
                value={height}
                onChange={(e) => setHeight(e.target.value)}
                onBlur={() => sendSize("h")}
                onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
              />
              <span className="dim">pt</span>
            </span>
          </div>
          <label className="ie-check">
            <input type="checkbox" checked={keep} onChange={(e) => setKeep(e.target.checked)} />
            Keep proportions
          </label>
          {note && <p className="dim te-hint">{note}</p>}
          <div className="te-row ie-actions">
            <button className="te-btn" title="Choose another picture for this place" onClick={() => void ie().replace(id)}>
              <Icon name="swap" size={16} />
              Replace…
            </button>
            <button className="te-btn" title="Show only part of the picture" onClick={() => ie().startCrop(id)}>
              <Icon name="crop" size={16} />
              Crop
            </button>
            <button className="te-btn" title="Delete the picture (Del)" onClick={() => void ie().remove(id)}>
              <Icon name="trash" size={16} />
              Delete
            </button>
          </div>
        </>
      )}
    </div>
  );
}
