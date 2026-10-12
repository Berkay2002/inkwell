import { useEffect, useState } from "react";
import type { ImageBlock } from "../../lib/api";
import {
  type AlignHow,
  type ImageRef,
  codecName,
  colorName,
  imageLabel,
  isQuarter,
  picksOf,
  ppi,
  rectH,
  rectW,
  roundAngle,
  turnTo,
  useImageEdit,
} from "../../lib/imageEdit";
import { type DocState, useStore } from "../../store";
import { Icon } from "../Icons";
import "./imageEdit.css";

const ie = () => useImageEdit.getState();

/** What a signed document's image edit says (the rail shows it under the controls). */
const SIGNED_IMAGES =
  "This document is signed. A change to its images is saved after the signature, which will show that the file was changed.";

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
  const count = d ? picksOf(d).length : 0;
  const pageState = d?.selected ? d.pages[d.selected.page] : undefined;
  const locked = pageState?.locked ?? null;
  const here = d?.pages[doc.currentPage]?.images ?? [];
  const hasClip = useImageEdit((s) => s.clip != null);

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

  // Cmd or Ctrl and C copies the picked pictures, and V pastes them on the page in view (text fields and
  // text blocks keep their own).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey || useStore.getState().active !== id) return;
      const t = e.target as HTMLElement | null;
      if (t && (["INPUT", "TEXTAREA", "SELECT"].includes(t.tagName) || t.isContentEditable)) return;
      const k = e.key.toLowerCase();
      const cur = ie().docs[id];
      if (k === "c" && cur && picksOf(cur).length > 0 && !cur.crop) {
        e.preventDefault();
        void ie().copy(id);
      } else if (k === "v" && ie().clip && !cur?.placing) {
        e.preventDefault();
        void ie().paste(id, useStore.getState().docs[id]?.currentPage ?? 0);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [id]);

  if (!placing && !selected && here.length === 0 && !d?.notice && !hasClip) return null;
  return (
    <section className="te-sec" aria-label="Images">
      <h3 className="te-h3">
        {count > 1 ? `${count} images picked` : selected ? "Format image" : placing ? "Add image" : `Images on page ${doc.currentPage + 1}`}
      </h3>
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
      {!placing && count > 1 && <GroupTools id={id} locked={locked} />}
      {!placing && count <= 1 && selected && d?.selected && (
        <Format key={`${d.selected.page}:${selected.index}`} id={id} target={d.selected} image={selected} cropping={d.crop != null} locked={locked} />
      )}
      {!placing && <ClipRow id={id} page={doc.currentPage} picks={count} copyLocked={pageState?.copyLocked ?? null} locked={locked} />}
      {!placing && selected && (locked || pageState?.signed) && (
        <p className="dim te-hint" role="note">
          {locked ?? SIGNED_IMAGES}
        </p>
      )}
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
          <p className="dim te-hint">
            Click a picture on the page to turn, crop, replace or delete it. Shift-click adds to the picks; Shift and a drag on the page
            picks the pictures the box touches.
          </p>
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

/** Copy the picked pictures, and paste the clipboard on the page in view. */
function ClipRow({ id, page, picks, copyLocked, locked }: { id: number; page: number; picks: number; copyLocked: string | null; locked: string | null }) {
  const clip = useImageEdit((s) => s.clip);
  const pageLocked = useImageEdit((s) => s.docs[id]?.pages[page]?.locked ?? locked);
  if (picks === 0 && !clip) return null;
  return (
    <div className="te-row ie-actions">
      {picks > 0 && (
        <button className="te-btn" title={copyLocked ?? "Copy the picked pictures (Ctrl+C)"} disabled={copyLocked != null} onClick={() => void ie().copy(id)}>
          <Icon name="copy" size={16} />
          Copy
        </button>
      )}
      {clip && (
        <button
          className="te-btn"
          title={pageLocked ?? `Paste ${clip.info.count === 1 ? "the copied picture" : `the ${clip.info.count} copied pictures`} on page ${page + 1} (Ctrl+V)`}
          disabled={pageLocked != null}
          onClick={() => void ie().paste(id, page)}
        >
          Paste{clip.info.count > 1 ? ` ${clip.info.count}` : ""} on page {page + 1}
        </button>
      )}
    </div>
  );
}

const round1 = (v: number) => Math.round(v * 10) / 10;

const ALIGN: { how: AlignHow; label: string; icon: "layoutLeft" | "layoutHCenter" | "layoutRight" | "layoutTop" | "layoutVCenter" | "layoutBottom" }[] = [
  { how: "left", label: "Align left edges", icon: "layoutLeft" },
  { how: "hcenter", label: "Centre horizontally", icon: "layoutHCenter" },
  { how: "right", label: "Align right edges", icon: "layoutRight" },
  { how: "top", label: "Align top edges", icon: "layoutTop" },
  { how: "vcenter", label: "Centre vertically", icon: "layoutVCenter" },
  { how: "bottom", label: "Align bottom edges", icon: "layoutBottom" },
];

/** The angle field: the picture's angle as it is now, sent as the turn that gets it there. */
function AngleField({ id, image, target, locked }: { id: number; image: ImageBlock; target: ImageRef; locked: string | null }) {
  const now = roundAngle(image.angle);
  const [text, setText] = useState(String(now));
  useEffect(() => setText(String(now)), [now]);
  const send = () => {
    const v = parseFloat(text.replace(",", "."));
    const d = Number.isFinite(v) ? turnTo(image.angle, v) : null;
    if (d == null) return setText(String(now));
    void ie().turn(id, d, target);
  };
  return (
    <div className="te-row">
      <span className="te-label">Angle</span>
      <span className="ie-size">
        <input
          className="ie-num"
          aria-label="Angle in degrees, clockwise"
          inputMode="decimal"
          value={text}
          disabled={locked != null}
          title={locked ?? "Turn the picture to this angle, about its centre"}
          onChange={(e) => setText(e.target.value)}
          onBlur={send}
          onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
        />
        <span className="dim">°</span>
      </span>
    </div>
  );
}

/** The rail for two or more picked images: align, turn, flip and delete them as one step each. */
function GroupTools({ id, locked }: { id: number; locked: string | null }) {
  const [by, setBy] = useState("");
  const off = locked != null;
  const send = () => {
    const v = parseFloat(by.replace(",", "."));
    setBy("");
    const d = Number.isFinite(v) ? turnTo(0, v) : null;
    if (d != null) void ie().turn(id, d);
  };
  return (
    <div className="te-format">
      <div className="te-row">
        <span className="te-label">Align</span>
        <span className="ie-tools ie-align">
          {ALIGN.map((a) => (
            <button key={a.how} className="te-btn ie-icon" title={locked ?? a.label} aria-label={a.label} disabled={off} onClick={() => void ie().align(id, a.how)}>
              <Icon name={a.icon} size={16} />
            </button>
          ))}
        </span>
      </div>
      <div className="te-row">
        <span className="te-label">Turn</span>
        <span className="ie-tools">
          <button className="te-btn ie-icon" title={locked ?? "Turn each 90° to the left"} aria-label="Turn left" disabled={off} onClick={() => void ie().rotate(id, -1)}>
            <Icon name="rotateLeft" size={16} />
          </button>
          <button className="te-btn ie-icon" title={locked ?? "Turn each 90° to the right"} aria-label="Turn right" disabled={off} onClick={() => void ie().rotate(id, 1)}>
            <Icon name="rotate" size={16} />
          </button>
          <button className="te-btn ie-icon" title={locked ?? "Flip each left to right"} aria-label="Flip horizontally" disabled={off} onClick={() => void ie().flip(id, true)}>
            <Icon name="flipH" size={16} />
          </button>
          <button className="te-btn ie-icon" title={locked ?? "Flip each top to bottom"} aria-label="Flip vertically" disabled={off} onClick={() => void ie().flip(id, false)}>
            <Icon name="flipV" size={16} />
          </button>
        </span>
      </div>
      <div className="te-row">
        <span className="te-label">Turn by</span>
        <span className="ie-size">
          <input
            className="ie-num"
            aria-label="Degrees to turn each picture, clockwise"
            inputMode="decimal"
            placeholder="0"
            value={by}
            disabled={off}
            title={locked ?? "Turn each picture by this many degrees about its own centre"}
            onChange={(e) => setBy(e.target.value)}
            onBlur={send}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
          />
          <span className="dim">°</span>
        </span>
      </div>
      <div className="te-row ie-actions">
        <button className="te-btn" title={locked ?? "Delete the picked pictures (Del)"} disabled={off} onClick={() => void ie().remove(id)}>
          <Icon name="trash" size={16} />
          Delete
        </button>
      </div>
      <p className="dim te-hint">Drag any picked picture to move them all, or drop them on another page. Esc lets go.</p>
    </div>
  );
}

function Format({ id, target, image, cropping, locked }: { id: number; target: ImageRef; image: ImageBlock; cropping: boolean; locked: string | null }) {
  const w = rectW(image.fullRect);
  const h = rectH(image.fullRect);
  const [width, setWidth] = useState(String(round1(w)));
  const [height, setHeight] = useState(String(round1(h)));
  const [keep, setKeep] = useState(true);
  // A picture turned by other than quarter turns is only scaled evenly.
  const turned = !isQuarter(image.angle);
  const off = locked != null;
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
    const nw = which === "w" ? v : keep || turned ? v * (w / Math.max(h, 1e-6)) : w;
    const nh = which === "h" ? v : keep || turned ? v * (h / Math.max(w, 1e-6)) : h;
    if (Math.abs(nw - w) < 0.05 && Math.abs(nh - h) < 0.05) {
      setWidth(String(round1(w)));
      setHeight(String(round1(h)));
      return;
    }
    const [x, y] = image.fullRect;
    // A field commits as it loses focus, which is also when a click elsewhere selects another image: the
    // change is for the image this section shows.
    void ie().resizeTo(id, [x, y, x + nw, y + nh], turned, target);
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
              <button className="te-btn ie-icon" title={locked ?? "Turn 90° to the left"} aria-label="Turn left" disabled={off} onClick={() => void ie().rotate(id, -1)}>
                <Icon name="rotateLeft" size={16} />
              </button>
              <button className="te-btn ie-icon" title={locked ?? "Turn 90° to the right"} aria-label="Turn right" disabled={off} onClick={() => void ie().rotate(id, 1)}>
                <Icon name="rotate" size={16} />
              </button>
            </span>
          </div>
          <div className="te-row">
            <span className="te-label">Flip</span>
            <span className="ie-tools">
              <button className="te-btn ie-icon" title={locked ?? "Flip left to right"} aria-label="Flip horizontally" disabled={off} onClick={() => void ie().flip(id, true)}>
                <Icon name="flipH" size={16} />
              </button>
              <button className="te-btn ie-icon" title={locked ?? "Flip top to bottom"} aria-label="Flip vertically" disabled={off} onClick={() => void ie().flip(id, false)}>
                <Icon name="flipV" size={16} />
              </button>
            </span>
          </div>
          <AngleField id={id} image={image} target={target} locked={locked} />
          <div className="te-row">
            <span className="te-label">Size</span>
            <span className="ie-size">
              <input
                className="ie-num te-keep"
                aria-label="Width in points"
                inputMode="decimal"
                disabled={off}
                title={locked ?? undefined}
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
                disabled={off}
                title={locked ?? undefined}
                value={height}
                onChange={(e) => setHeight(e.target.value)}
                onBlur={() => sendSize("h")}
                onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
              />
              <span className="dim">pt</span>
            </span>
          </div>
          <label className="ie-check">
            <input type="checkbox" checked={keep || turned} disabled={off || turned} onChange={(e) => setKeep(e.target.checked)} />
            Keep proportions{turned ? " (a turned picture is only scaled evenly)" : ""}
          </label>
          {note && <p className="dim te-hint">{note}</p>}
          <div className="te-row ie-actions">
            <button className="te-btn" title={locked ?? "Choose another picture for this place"} disabled={off} onClick={() => void ie().replace(id)}>
              <Icon name="swap" size={16} />
              Replace…
            </button>
            <button className="te-btn" title={locked ?? "Show only part of the picture"} disabled={off} onClick={() => ie().startCrop(id)}>
              <Icon name="crop" size={16} />
              Crop
            </button>
            <button className="te-btn" title={locked ?? "Delete the picture (Del)"} disabled={off} onClick={() => void ie().remove(id)}>
              <Icon name="trash" size={16} />
              Delete
            </button>
          </div>
        </>
      )}
    </div>
  );
}
