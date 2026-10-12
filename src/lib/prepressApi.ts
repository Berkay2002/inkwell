import { Channel, invoke } from "@tauri-apps/api/core";
import type { EditState, FileRef } from "./api";

/**
 * Print production (src-tauri/src/prepress/app.rs, docs/prepress.md). Boxes are in the page's default user
 * space, `[x0, y0, x1, y1]`, y up; the points of the other results are page points from the top-left of the
 * page as it is shown (/Rotate applied). Pages are 0-based.
 */

export type Rect4 = [number, number, number, number];
export type Cmyk = [number, number, number, number];

export interface Spot {
  name: string;
  rgb: [number, number, number];
  /** Per cent. */
  cmyk: Cmyk;
}

export interface PageSeps {
  process: string[];
  spots: Spot[];
  /** The page paints with overprint. */
  overprint: boolean;
  /** Spots past the 27 that get a plane: painted into CMYK, not switchable. */
  folded: number;
}

export type ViewMode = "plates" | "coverage";

/** What a tile of the `p` route shows (the engine's `View`). */
export interface PrepressView {
  mode: ViewMode;
  process: [boolean, boolean, boolean, boolean];
  hiddenSpots: string[];
  overprint: boolean;
  tacLimit: number;
}

export interface PlaneInk {
  name: string;
  max: number;
  mean: number;
}

export interface InkReport {
  page: number;
  planes: PlaneInk[];
  maxTac: number;
  /** Page points from the top-left. */
  maxAt: [number, number];
  tacLimit: number;
  overLimit: boolean;
  /** 0 to 1. */
  overLimitArea: number;
  regions: Rect4[];
  regionsTruncated: boolean;
  spots: string[];
}

export interface InkAt {
  page: number;
  planes: { name: string; value: number }[];
  total: number;
}

export type ObjectKind = "fill" | "stroke" | "text" | "strokedText" | "image" | "imageMask" | "shading";

/** One painted object under a point (the engine's `Inspected`), topmost first. */
export interface Inspected {
  kind: ObjectKind;
  /** `DeviceCMYK`, `ICCBased (...)`, `Separation / DeviceN`, ... */
  colorSpace: string;
  /** `gray`, `rgb`, `cmyk`, `lab`, `indexed`, `spot` or `shading` (a shading's colours are not read). */
  model: string;
  colorants: string[];
  /** The components as the page gives them (0 to 1). */
  values: number[];
  /** Per cent; null for an image or a shading. */
  cmyk: Cmyk | null;
  /** The four inks added plus any spot tint, per cent. */
  ink: number | null;
  overprint: boolean;
  overprintMode: boolean;
  alpha: number;
  bounds: Rect4;
  font: string | null;
  /** Pixels wide, high, bits per component. */
  image: [number, number, number] | null;
}

export type BoxKind = "trim" | "bleed" | "art" | "crop";

export interface PageBoxes {
  /** /Rotate: 0, 90, 180 or 270. */
  rotate: number;
  media: Rect4;
  crop: Rect4;
  trim: Rect4;
  bleed: Rect4;
  art: Rect4;
  /** Which the page states itself; the others are the defaults of the crop box. */
  stated: Record<BoxKind, boolean>;
}

/**
 * The box as `rect`, or as the box `from` moved outward by `margin` `[left, bottom, right, top]` (negative inward).
 * With `shown` the margin is of the page as it is shown and each page maps it through its own /Rotate; otherwise
 * it is in the page's own space.
 */
export interface BoxSpec {
  pages?: number[] | null;
  kind: BoxKind;
  rect?: Rect4;
  from?: BoxKind | "media";
  margin?: Rect4;
  shown?: boolean;
}

export interface MarkSet {
  crop: boolean;
  bleed: boolean;
  registration: boolean;
  colorBars: boolean;
}

export interface MarksSpec {
  pages?: number[] | null;
  marks: MarkSet;
  weight?: number;
  offset?: number;
  length?: number;
}

export interface MarksPreview {
  /** The media box after the marks. */
  media: Rect4;
  trim: Rect4;
  bleed: Rect4;
  /** Drawing operators in user space: `q Q w J j CS SCN m l c re S f k`. */
  content: string;
  notes: string[];
}

export type Intent = "device" | "document" | "profile";

export interface ConvertSpec {
  pages?: number[] | null;
  gray: boolean;
  rgb: boolean;
  spot: boolean;
  grayToK: boolean;
  preserveBlack: boolean;
  intent: Intent;
  condition?: string | null;
}

export interface ConvertReport {
  pages: number[];
  /** DeviceGray, DeviceRGB, DeviceCMYK still in use afterwards. */
  remaining: [boolean, boolean, boolean];
  usedProfile: boolean;
  notes: string[];
}

/** The result of a conversion that was also saved as another file (`saveTo`). */
export interface Converted {
  state: EditState;
  report: ConvertReport;
  /** The file was written. */
  saved: boolean;
  /** The colours were converted in the open document and writing the file failed. */
  saveError: string | null;
}

export interface Protection {
  /** A /DocMDP signature: the engine refuses every edit. */
  certified: boolean;
  /** Signed: edits stay incremental, the signatures keep verifying. */
  signed: boolean;
}

export interface Progress {
  done: number;
  pages: number;
  stage: string;
}

const channel = (onProgress?: (p: Progress) => void) => {
  const ch = new Channel<Progress>();
  ch.onmessage = onProgress ?? (() => {});
  return ch;
};

export const prepressApi = {
  separations: (doc: number, page: number) => invoke<PageSeps>("prepress_separations", { doc, page }),
  /** Registers a view; the id goes in the tile URLs. The same view of a document gets the same id. */
  view: (doc: number, view: PrepressView) => invoke<number>("prepress_view", { doc, view }),
  ink: (job: number, doc: number, pages: number[] | null, limit: number, trimOnly: boolean, onProgress?: (p: Progress) => void) =>
    invoke<InkReport[]>("prepress_ink", { job, doc, pages, limit, trimOnly, onProgress: channel(onProgress) }),
  inkAt: (doc: number, page: number, x: number, y: number) => invoke<InkAt>("prepress_ink_at", { doc, page, x, y }),
  /** What the page paints at the point, topmost first (at most 8). Read-only. */
  inspect: (doc: number, page: number, x: number, y: number) => invoke<Inspected[]>("prepress_inspect", { doc, page, x, y }),
  /**
   * `profile`: an open token of a CMYK ICC profile, for `intent: "profile"`. `saveTo`: a save token; the engine
   * then also writes the result there as Save as, in the same command, and refuses the document's own file.
   */
  convert: (job: number, doc: number, spec: ConvertSpec, profile: string | null, saveTo: string | null, onProgress?: (p: Progress) => void) =>
    invoke<Converted>("prepress_convert", { job, doc, spec, profile, saveTo, onProgress: channel(onProgress) }),
  /** Save as after a conversion whose write failed: no second conversion. */
  saveOther: (doc: number, to: string) => invoke<EditState>("prepress_save_other", { doc, to }),
  protection: (doc: number) => invoke<Protection>("prepress_protection", { doc }),
  boxes: (doc: number, spec: BoxSpec) => invoke<EditState>("prepress_boxes", { doc, spec }),
  marks: (doc: number, spec: MarksSpec) => invoke<{ state: EditState; notes: string[] }>("prepress_marks", { doc, spec }),
  pageBoxes: (doc: number, page: number) => invoke<PageBoxes>("prepress_page_boxes", { doc, page }),
  marksPreview: (doc: number, page: number, spec: MarksSpec) => invoke<MarksPreview>("prepress_marks_preview", { doc, page, spec }),
  pickProfile: () => invoke<FileRef | null>("prepress_pick_profile"),
};
