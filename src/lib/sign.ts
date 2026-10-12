import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import type { EditState } from "./api";
import type { Annot, Pt, Rect4, RGB } from "./annotLogic";
import { applyState } from "./edit";

/**
 * Fill & Sign and digital signatures for the UI (src-tauri/src/fillsign, src-tauri/src/sign).
 * Placed items are annotations: every placement is one engine edit (one undo step) that Save
 * keeps. Signing is one step too, and saves the file at once.
 */

// ---- engine types ----

export type MarkKind = "check" | "cross" | "dot" | "circle" | "line";
export type SavedKind = "signature" | "initials";

export type Spec =
  | { type: "text"; at: Pt; text: string; fontSize?: number }
  | { type: "mark"; mark: MarkKind; at: Pt; size?: number }
  | { type: "image"; kind: SavedKind; rect: Rect4; savedId?: string; pngBase64?: string };

export type NewItem = { page: number; color?: RGB } & Spec;

/** A placed item: an annotation tagged with its Fill & Sign kind. */
export type Item = Annot & { item: "text" | MarkKind | SavedKind };

export interface Saved {
  id: string;
  kind: SavedKind;
  source: "typed" | "drawn" | "image";
  font: string | null;
  created: number;
  width: number;
  height: number;
  dataUrl: string;
}

export interface Identity {
  source: "store" | "file" | "test";
  handle: number | null;
  fingerprint: string | null;
  name: string;
  email: string;
  issuer: string;
  selfSigned: boolean;
  /** YYYY-MM-DD */
  notAfter: string;
  unusable: string | null;
}

export type IdRef = { kind: "store"; fingerprint: string } | { kind: "session"; handle: number };

/** `warning`: intact and trusted, with a caution in `warnings`. An untrusted signer stays `unknownSigner`. */
export type Status = "valid" | "unknownSigner" | "warning" | "invalid" | "unsigned";

export interface SignatureInfo {
  field: string;
  page: number;
  rect: Rect4;
  status: Status;
  summary: string;
  signer: string;
  issuer: string;
  /** PDF date string, D:YYYYMMDDHHmmSS+hh'mm' */
  time: string;
  reason: string;
  location: string;
  subFilter: string;
  digest: string;
  /** DocMDP permission 1-3 for a certification signature. */
  certify: number | null;
  intact: boolean;
  changes: "none" | "signaturesOnly" | "changed";
  trusted: boolean;
  trustNote: string;
  /** Cautions about an intact signature, one sentence each. */
  warnings: Warning[];
  certNotAfter: string;
  certFingerprint: string;
}

export interface SignOptions {
  field?: string;
  page: number;
  rect?: Rect4;
  reason?: string;
  location?: string;
  certify?: number;
  /** PNG base64 (no data: prefix) shown in place of the name. */
  image?: string;
  offsetMin: number;
}

export const signApi = {
  items: (doc: number) => invoke<Item[]>("fillsign_list", { doc }),
  add: (doc: number, item: NewItem) => invoke<{ state: EditState; xref: number }>("fillsign_add", { doc, item }),
  flatten: (doc: number, pages: number[] | null) => invoke<EditState>("fillsign_flatten", { doc, pages }),
  move: (doc: number, page: number, xref: number, rect: Rect4) => invoke<EditState>("annot_edit", { doc, page, xref, patch: { rect }, author: "" }),
  remove: (doc: number, page: number, xref: number) => invoke<EditState>("annot_delete", { doc, items: [[page, xref]] }),
  saved: () => invoke<Saved[]>("fillsign_saved_list"),
  saveDrawn: (kind: SavedKind, source: "typed" | "drawn", font: string | null, pngBase64: string) =>
    invoke<Saved>("fillsign_saved_add", { kind, source, font, pngBase64 }),
  saveFile: (kind: SavedKind, token: string) => invoke<Saved>("fillsign_saved_add_file", { kind, token }),
  forget: (id: string) => invoke<void>("fillsign_saved_remove", { id }),
  identities: () => invoke<Identity[]>("sign_identities"),
  openIdFile: (file: string, password: string) => invoke<Identity>("sign_open_id_file", { file, password }),
  createTestId: (name: string, email: string, org: string) => invoke<Identity>("sign_create_test_id", { name, email, org }),
  sign: (doc: number, id: IdRef, options: SignOptions, to: string | null) => invoke<EditState>("sign_document", { doc, id, options, to }),
  list: (doc: number) => invoke<SignatureInfo[]>("sign_list", { doc }),
};

// ---- pure helpers (tested) ----

export const INK: RGB = [31 / 255, 42 / 255, 68 / 255];

export const idRef = (i: Identity): IdRef | null =>
  i.handle != null ? { kind: "session", handle: i.handle } : i.fingerprint ? { kind: "store", fingerprint: i.fingerprint } : null;
export const idKey = (i: Identity) => (i.handle != null ? `s${i.handle}` : `f${i.fingerprint}`);

export const SOURCE_LABEL: Record<Identity["source"], string> = {
  store: "Windows certificate store",
  file: "Digital ID file",
  test: "Test ID",
};

/** Minutes east of UTC, as the engine stamps the signing time. */
export const offsetMin = (d = new Date()) => -d.getTimezoneOffset();

/** `D:20261011143200+02'00'` → `11 Oct 2026, 14:32`; anything else is returned as is. */
export function pdfDate(s: string): string {
  const m = /^D:(\d{4})(\d{2})(\d{2})(\d{2})?(\d{2})?/.exec(s);
  if (!m) return s;
  const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  const day = `${Number(m[3])} ${months[Number(m[2]) - 1] ?? m[2]} ${m[1]}`;
  return m[4] ? `${day}, ${m[4]}:${m[5] ?? "00"}` : day;
}

/** Today as Fill & Sign's date tool writes it: `11 Oct 2026`. */
export function today(d = new Date()): string {
  return d.toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });
}

/** The appearance's detail lines, as the engine writes them (sign/pdf.rs detail_lines). */
export function detailLines(name: string, d: Date, reason: string, location = ""): string[] {
  const p = (n: number) => String(n).padStart(2, "0");
  const off = offsetMin(d);
  const sign = off < 0 ? "-" : "+";
  const a = Math.abs(off);
  const date = `${d.getFullYear()}.${p(d.getMonth() + 1)}.${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())} ${sign}${p(Math.floor(a / 60))}'${p(a % 60)}'`;
  const out = [`Digitally signed by ${name}`, `Date: ${date}`];
  if (reason.trim()) out.push(`Reason: ${reason.trim()}`);
  if (location.trim()) out.push(`Location: ${location.trim()}`);
  return out;
}

/** A caution about an intact signature: changed after signing, a SHA-1 hash, a later visible box. */
export type WarningCode = "changed" | "sha1" | "laterBox" | "laterBoxLarge" | "hybridXref" | "damaged";
export interface Warning {
  code: WarningCode;
  text: string;
}

const has = (s: SignatureInfo, ...codes: WarningCode[]) => s.warnings.some((w) => codes.includes(w.code));

export type Tone = "valid" | "unknown" | "invalid";

export const STATUS: Record<Exclude<Status, "unsigned">, { tone: Tone; word: string }> = {
  valid: { tone: "valid", word: "Valid" },
  unknownSigner: { tone: "unknown", word: "Unknown signer" },
  warning: { tone: "unknown", word: "Valid, with a warning" },
  invalid: { tone: "invalid", word: "Invalid" },
};

/** The status word on a signature's card. */
export function statusWord(s: SignatureInfo): string {
  if (s.status === "warning") {
    if (has(s, "changed")) return "Valid, but changed after signing";
    if (has(s, "laterBox", "laterBoxLarge")) return "Valid, but a box was added later";
    return "Valid, but weak";
  }
  return STATUS[s.status as keyof typeof STATUS]?.word ?? "";
}

/** The warnings a card lists under the change note (which already says the document changed). */
export const otherWarnings = (s: SignatureInfo) => s.warnings.filter((w) => w.code !== "changed").map((w) => w.text);

export const signed = (l: SignatureInfo[]) => l.filter((s) => s.status !== "unsigned");

/** The banner over a signed document: the worst state wins. Null when nothing is signed. */
export function banner(list: SignatureInfo[]): { tone: Tone; text: string } | null {
  const s = signed(list);
  if (s.length === 0) return null;
  const all = s.length === 1 ? "The signature is" : "All signatures are";
  if (s.some((x) => x.status === "invalid"))
    return { tone: "invalid", text: s.length === 1 ? "The signature is invalid." : "At least one signature is invalid." };
  const one = s.length === 1;
  // Trust first: an untrusted signature is never called valid, whatever else is true.
  const changed = s.some((x) => has(x, "changed"));
  if (s.some((x) => x.status === "unknownSigner"))
    return {
      tone: "unknown",
      text: `Signed. ${one ? "The signature is intact, but its signer is" : "The signatures are intact, but a signer is"} not in your trusted list.${changed ? " The document was changed after signing." : ""}`,
    };
  if (changed) return { tone: "unknown", text: `${one ? "Signature" : "Signatures"} valid, but the document was changed after signing.` };
  if (s.some((x) => has(x, "laterBox", "laterBoxLarge")))
    return { tone: "unknown", text: "Signatures valid, but a later signature added a visible box." };
  if (s.some((x) => x.status === "warning"))
    return { tone: "unknown", text: `Signed, but ${one ? "the signature uses" : "a signature uses"} a weak hash algorithm.` };
  return { tone: "valid", text: `Signed. ${all} valid.` };
}

/** One line under the signer: what the signature promises about later changes. */
export function changeNote(s: SignatureInfo): string {
  if (!s.intact) return "The document was changed after it was signed, or the signature is damaged.";
  if (s.changes === "none") return "No changes since it was signed.";
  if (s.changes === "signaturesOnly") return "Signed again later; the content is unchanged.";
  return "The document was changed after it was signed.";
}

export const CERTIFY_LABEL: Record<number, string> = {
  1: "Certified · no changes allowed",
  2: "Certified · form filling and signing allowed",
  3: "Certified · form filling, signing and comments allowed",
};

/** Whether a new signature may be added: a P=1 certification forbids it. */
export const locked = (list: SignatureInfo[]) => signed(list).some((s) => s.certify === 1);

export const LOCKED_REASON = "The document is certified with no changes allowed, so it can't be signed again.";

/** How a signature form field on the page behaves: `sig` is the field's entry in the document's signature list, if read yet. */
export interface SigFieldView {
  /** `unknown`: not read yet (inert); `locked`: refused, with the reason in the title. */
  state: "unknown" | "locked" | "sign" | "signed";
  title: string;
}

export function sigFieldView(label: string, sig: SignatureInfo | undefined, list: SignatureInfo[]): SigFieldView {
  if (!sig) return { state: "unknown", title: label };
  if (sig.status !== "unsigned") return { state: "signed", title: `${label}: signed${sig.signer ? ` by ${sig.signer}` : ""}, click for details` };
  if (locked(list)) return { state: "locked", title: `${label}: ${LOCKED_REASON}` };
  return { state: "sign", title: `${label}: click to sign` };
}

/** The outcome line next to Sign. */
export function outcome(list: SignatureInfo[], lock: boolean, dirty: boolean): string {
  const n = signed(list).length;
  const save = dirty ? "Saves the file with your other changes." : "Saves the file.";
  if (lock) return `${save} Locks the document: no more changes.`;
  if (n === 0) return `${save} Later changes will show as made after signing.`;
  return `${save} Adds a new revision; ${n === 1 ? "the earlier signature stays" : "earlier signatures stay"} valid.`;
}

/** A placed signature or initials image: `width` points wide at `at` (its centre), keeping the picture's shape. */
export function imageRect(at: Pt, s: { width: number; height: number }, width: number, page: [number, number]): Rect4 {
  const w = Math.min(width, page[0]);
  const h = (w * Math.max(1, s.height)) / Math.max(1, s.width);
  const x0 = Math.min(Math.max(at[0] - w / 2, 0), page[0] - w);
  const y0 = Math.min(Math.max(at[1] - h / 2, 0), page[1] - h);
  return [x0, y0, x0 + w, y0 + h];
}

export const DEFAULT_WIDTH: Record<SavedKind, number> = { signature: 150, initials: 50 };

/** A dragged digital signature box: at least 60 x 24 points, else a 200 x 60 box at the click. */
export function sigBox(a: Pt, b: Pt, page: [number, number]): Rect4 {
  let r: Rect4 = [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])];
  if (r[2] - r[0] < 60 || r[3] - r[1] < 24) r = [a[0], a[1], a[0] + 200, a[1] + 60];
  const dx = Math.min(0, page[0] - r[2]);
  const dy = Math.min(0, page[1] - r[3]);
  return [Math.max(0, r[0] + dx), Math.max(0, r[1] + dy), r[2] + dx, r[3] + dy];
}

// ---- store ----

export type Tool = "select" | "text" | MarkKind | "date" | SavedKind | "digital";

export interface Draft {
  page: number;
  rect: Rect4;
  /** An existing empty signature field. */
  field?: string;
}

export interface DocSign {
  /** The signature field the Signatures rail scrolls to and marks. */
  focus: string | null;
  items: Item[];
  selected: number | null;
  sigs: SignatureInfo[];
  /** The rev `sigs` were read at. */
  sigsRev: number;
  draft: Draft | null;
  bannerHidden: boolean;
  error: string | null;
}

const fresh = (): DocSign => ({ focus: null, items: [], selected: null, sigs: [], sigsRev: -1, draft: null, bannerHidden: false, error: null });

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));

interface SignStore {
  /** The Fill & Sign strip is showing. */
  open: boolean;
  setOpen: (open: boolean) => void;
  tool: Tool;
  setTool: (t: Tool) => void;
  /** The right rail: the signing sheet or the signatures list. */
  rail: "sheet" | "list" | null;
  setRail: (r: SignStore["rail"]) => void;
  /** Which saved signature / initials the tiles place. */
  pick: Partial<Record<SavedKind, string>>;
  /** The creator dialog, for a kind. */
  creator: SavedKind | null;
  setCreator: (k: SavedKind | null) => void;

  saved: Saved[];
  loadSaved: () => Promise<void>;
  addSaved: (s: Saved) => void;
  forget: (id: string) => Promise<void>;

  ids: Identity[];
  idsLoaded: boolean;
  idChoice: string | null;
  loadIds: () => Promise<void>;
  addId: (i: Identity) => void;
  setIdChoice: (k: string) => void;

  reason: string;
  lock: boolean;
  /** Show the saved signature picture in the appearance. */
  picture: boolean;
  setSheet: (p: Partial<Pick<SignStore, "reason" | "lock" | "picture">>) => void;
  signing: boolean;

  docs: Record<number, DocSign>;
  get: (doc: number) => DocSign;
  refresh: (doc: number) => Promise<void>;
  refreshSigs: (doc: number, rev: number) => Promise<void>;
  select: (doc: number, xref: number | null) => void;
  place: (doc: number, item: NewItem) => Promise<number | null>;
  move: (doc: number, page: number, xref: number, rect: Rect4) => Promise<void>;
  remove: (doc: number, page: number, xref: number) => Promise<void>;
  flatten: (doc: number, pages: number[] | null) => Promise<void>;
  setDraft: (doc: number, d: Draft | null) => void;
  /** Opens the signing sheet aimed at an existing empty signature field. */
  signField: (doc: number, f: Pick<SignatureInfo, "page" | "rect" | "field">) => void;
  /** Opens the Signatures rail on one signature. */
  showSignature: (doc: number, field: string) => void;
  hideBanner: (doc: number) => void;
  /** Signs the draft; `to` is a save token for a document that has no file yet. */
  sign: (doc: number, to: string | null) => Promise<boolean>;
}

export const useSign = create<SignStore>((set, get) => {
  const patch = (doc: number, p: Partial<DocSign>) =>
    set((s) => ({ docs: { ...s.docs, [doc]: { ...(s.docs[doc] ?? fresh()), ...p } } }));
  const fail = (doc: number, e: unknown) => patch(doc, { error: msg(e) });
  /** One engine edit: show its state, re-read the items. */
  const run = async <T>(doc: number, f: () => Promise<T>, state: (r: T) => EditState): Promise<T | null> => {
    try {
      const r = await f();
      applyState(doc, state(r));
      await get().refresh(doc);
      patch(doc, { error: null });
      return r;
    } catch (e) {
      fail(doc, e);
      return null;
    }
  };

  return {
    open: false,
    setOpen: (open) => set(open ? { open } : { open, tool: "select", rail: get().rail === "sheet" ? null : get().rail }),
    tool: "select",
    setTool: (tool) => set({ tool }),
    rail: null,
    setRail: (rail) => set({ rail }),
    pick: {},
    creator: null,
    setCreator: (creator) => set({ creator }),

    saved: [],
    loadSaved: async () => {
      try {
        const saved = await signApi.saved();
        const pick = { ...get().pick };
        for (const k of ["signature", "initials"] as const)
          if (!saved.some((s) => s.id === pick[k])) pick[k] = saved.filter((s) => s.kind === k).pop()?.id;
        set({ saved, pick });
      } catch {
        /* the tiles stay empty */
      }
    },
    addSaved: (s) => set((st) => ({ saved: [...st.saved, s], pick: { ...st.pick, [s.kind]: s.id } })),
    forget: async (id) => {
      await signApi.forget(id);
      await get().loadSaved();
    },

    ids: [],
    idsLoaded: false,
    idChoice: null,
    loadIds: async () => {
      try {
        const ids = await signApi.identities();
        const keep = ids.some((i) => idKey(i) === get().idChoice);
        // A Windows store ID is the user's own certificate: never picked for them.
        const first = ids.find((i) => !i.unusable && i.source !== "store");
        set({ ids, idsLoaded: true, idChoice: keep ? get().idChoice : first ? idKey(first) : null });
      } catch {
        set({ idsLoaded: true });
      }
    },
    addId: (i) => set((s) => ({ ids: [i, ...s.ids.filter((x) => idKey(x) !== idKey(i))], idChoice: idKey(i) })),
    setIdChoice: (idChoice) => set({ idChoice }),

    reason: "",
    lock: false,
    picture: false,
    setSheet: (p) => set(p),
    signing: false,

    docs: {},
    get: (doc) => get().docs[doc] ?? fresh(),
    refresh: async (doc) => {
      try {
        const items = await signApi.items(doc);
        const sel = get().get(doc).selected;
        patch(doc, { items, selected: items.some((i) => i.xref === sel) ? sel : null });
      } catch (e) {
        fail(doc, e);
      }
    },
    refreshSigs: async (doc, rev) => {
      try {
        const sigs = await signApi.list(doc);
        const before = signed(get().get(doc).sigs).length;
        patch(doc, { sigs, sigsRev: rev, ...(signed(sigs).length !== before ? { bannerHidden: false } : {}) });
      } catch {
        patch(doc, { sigs: [], sigsRev: rev });
      }
    },
    select: (doc, selected) => patch(doc, { selected }),
    place: async (doc, item) => {
      const r = await run(doc, () => signApi.add(doc, item), (x) => x.state);
      if (!r) return null;
      patch(doc, { selected: r.xref });
      return r.xref;
    },
    move: async (doc, page, xref, rect) => {
      await run(doc, () => signApi.move(doc, page, xref, rect), (s) => s);
    },
    remove: async (doc, page, xref) => {
      await run(doc, () => signApi.remove(doc, page, xref), (s) => s);
      patch(doc, { selected: null });
    },
    flatten: async (doc, pages) => {
      await run(doc, () => signApi.flatten(doc, pages), (s) => s);
      patch(doc, { selected: null });
    },
    setDraft: (doc, draft) => patch(doc, { draft }),
    signField: (doc, f) => {
      patch(doc, { draft: { page: f.page, rect: f.rect, field: f.field } });
      set({ rail: "sheet" });
    },
    showSignature: (doc, field) => {
      patch(doc, { focus: field });
      set({ rail: "list" });
    },
    hideBanner: (doc) => patch(doc, { bannerHidden: true }),
    sign: async (doc, to) => {
      const s = get();
      const d = s.get(doc).draft;
      const who = s.ids.find((i) => idKey(i) === s.idChoice);
      const ref = who && idRef(who);
      if (!d || !ref) return false;
      set({ signing: true });
      try {
        const options: SignOptions = {
          page: d.page,
          ...(d.field ? { field: d.field } : { rect: d.rect }),
          ...(s.reason.trim() ? { reason: s.reason.trim() } : {}),
          ...(s.lock && !signed(s.get(doc).sigs).length ? { certify: 1 } : {}),
          ...(s.picture && picture(s) ? { image: picture(s)!.dataUrl.split(",")[1] } : {}),
          offsetMin: offsetMin(),
        };
        const state = await signApi.sign(doc, ref, options, to);
        applyState(doc, state);
        patch(doc, { draft: null, error: null, bannerHidden: false });
        await get().refreshSigs(doc, state.rev);
        set({ rail: "list", tool: "select", lock: false });
        return true;
      } catch (e) {
        fail(doc, e);
        return false;
      } finally {
        set({ signing: false });
      }
    },
  };
});

/** A click on a signature form field: starts signing into an empty one, shows a signed one, does nothing on a refused one. */
export function clickSigField(doc: number, label: string, sig: SignatureInfo | undefined, list: SignatureInfo[]) {
  const st = useSign.getState();
  const { state } = sigFieldView(label, sig, list);
  // The signature list's name and box are sent, not the form's: the engine looks the field up by them.
  if (sig && state === "sign") st.signField(doc, sig);
  else if (sig && state === "signed") st.showSignature(doc, sig.field);
}

/** The saved signature the appearance can show (the engine writes the name when there is none). */
export const picture = (s: { saved: Saved[]; pick: Partial<Record<SavedKind, string>> }) =>
  s.saved.find((x) => x.id === s.pick.signature) ?? null;
