import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import type { Changes, Cipher, Printing, ProtectSpec, SecurityInfo } from "./api";
import { useAi } from "./aiStore";
import { useAnnot } from "./annot";
import { useConvert } from "./convertStore";
import { applyState } from "./edit";
import { useHistory } from "./historyStore";
import { useRedact } from "./redact";
import { useTools } from "./toolsStore";

/**
 * Protect (passwords and permissions) and Remove security: one rail sheet per window, plus what each
 * open document is protected with (`security`), which the tab lock, the notice above the page and
 * Document properties read. UI-agnostic; `ProtectPanel` renders it.
 *
 * Setting or removing security is one edit step in the engine (src-tauri/src/engine/edit/security.rs):
 * undoable, written by Save or Save as. "Protect and save a copy" picks the file first, applies the
 * step, then saves to the picked file, so a dismissed dialog changes nothing.
 */

export type Phase = "options" | "working" | "done" | "error";
export type View = "protect" | "security";
/** What the button does: write the protected file as a copy, or replace this file after a confirm. */
export type Output = "copy" | "inPlace";

/** Longest password the engine accepts, in UTF-8 bytes (MuPDF's 128-byte buffers). */
export const MAX_PASSWORD_BYTES = 127;

export interface ProtectSheet {
  doc: number;
  view: View;
  phase: Phase;
  error: string | null;
  // what to set
  requireOpen: boolean;
  openPassword: string;
  openConfirm: string;
  restrict: boolean;
  permissionsPassword: string;
  permissionsConfirm: string;
  printing: Printing;
  changes: Changes;
  copy: boolean;
  accessibility: boolean;
  cipher: Cipher;
  /** Needed to change or remove the security of a file that has restrictions. */
  currentPassword: string;
  /** "Protect this file instead" asked for, waiting for the explicit confirm. */
  confirmInPlace: boolean;
  /** What happened, for the card shown when the run is done. */
  result: { output: Output | "removed"; name: string } | null;
}

interface ProtectStore {
  sheet: ProtectSheet | null;
  /** What each open document is protected with, as Save would write it. */
  security: Record<number, SecurityInfo>;
  /** The restricted-file notice was dismissed for this document. */
  dismissed: Record<number, boolean>;
  open: (view?: View, doc?: number) => void;
  close: () => void;
  set: (p: Partial<ProtectSheet>) => void;
  refresh: (doc: number) => Promise<void>;
  dismiss: (doc: number) => void;
  /** Applies the options to the document and writes the file. */
  run: (output: Output) => Promise<void>;
  /** Removes the security of the document (one undo step; Save writes it). */
  remove: () => Promise<void>;
}

const st = () => useStore.getState();
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const stem = (p: string) => baseName(p).replace(/\.pdf$/i, "");

const blank = (doc: number, view: View): ProtectSheet => ({
  doc,
  view,
  phase: "options",
  error: null,
  requireOpen: true,
  openPassword: "",
  openConfirm: "",
  restrict: false,
  permissionsPassword: "",
  permissionsConfirm: "",
  printing: "high",
  changes: "none",
  copy: true,
  accessibility: true,
  cipher: "aes256",
  currentPassword: "",
  confirmInPlace: false,
  result: null,
});

// ---- pure helpers (tested in protectStore.test.ts) ----

export type StrengthLabel = "Weak" | "Fair" | "Strong";

const COMMON = new Set(["password", "passw0rd", "123456", "12345678", "123456789", "qwerty", "qwertyuiop", "letmein", "111111", "abc123", "iloveyou", "admin", "welcome", "secret"]);

/**
 * Password strength as five segments and a word. Length and variety only (the same idea as the
 * pdfcraft dialog: 12 characters with three kinds is Strong, 8 with two is Fair); a short list of
 * the most common passwords is capped at one segment.
 */
export function strength(pw: string): { segments: 0 | 1 | 2 | 3 | 4 | 5; label: StrengthLabel | null } {
  const n = [...pw].length;
  if (n === 0) return { segments: 0, label: null };
  const kinds = [/[a-z]/, /[A-Z]/, /[0-9]/, /[^a-zA-Z0-9]/].filter((r) => r.test(pw)).length;
  let s = 1 + (n >= 8 ? 1 : 0) + (n >= 12 ? 1 : 0) + (kinds >= 2 ? 1 : 0) + (kinds >= 3 ? 1 : 0);
  if (COMMON.has(pw.toLowerCase())) s = 1;
  if (n < 8) s = Math.min(s, 2);
  const segments = Math.min(5, s) as 1 | 2 | 3 | 4 | 5;
  return { segments, label: segments >= 5 ? "Strong" : segments >= 3 ? "Fair" : "Weak" };
}

const utf8Bytes = (s: string) => new TextEncoder().encode(s).length;
/** AES-128 documents only carry Latin-1 passwords reliably; AES-256 takes any text. */
const plainText = (s: string) => /^[ -~]*$/.test(s);

/** Why the options can't be applied yet, or null. */
export function readiness(s: ProtectSheet): string | null {
  if (!s.requireOpen && !s.restrict) return "Choose a password to open the file, restrictions, or both.";
  if (s.requireOpen) {
    if (s.openPassword === "") return "Type the password to open the file.";
    if (s.openPassword !== s.openConfirm) return "The two passwords to open the file don't match.";
    if (utf8Bytes(s.openPassword) > MAX_PASSWORD_BYTES) return "That password is too long (127 bytes at most).";
    if (s.cipher === "aes128" && !plainText(s.openPassword)) return "AES-128 files only take letters, digits and symbols from the keyboard. Use AES-256, or change the password.";
  }
  if (s.restrict) {
    if (s.permissionsPassword === "") return "Type a permissions password: it is what stops others lifting the restrictions.";
    if (s.permissionsPassword !== s.permissionsConfirm) return "The two permissions passwords don't match.";
    if (utf8Bytes(s.permissionsPassword) > MAX_PASSWORD_BYTES) return "That permissions password is too long (127 bytes at most).";
    if (s.cipher === "aes128" && !plainText(s.permissionsPassword)) return "AES-128 files only take letters, digits and symbols from the keyboard. Use AES-256, or change the password.";
    if (s.requireOpen && s.openPassword === s.permissionsPassword) return "The two passwords have to be different, or the open password would also lift the restrictions.";
  }
  return null;
}

/** The command's request for the sheet as filled in (call only when `readiness` is null). */
export function specOf(s: ProtectSheet): ProtectSpec {
  return {
    openPassword: s.requireOpen ? s.openPassword : null,
    permissionsPassword: s.restrict ? s.permissionsPassword : null,
    printing: s.restrict ? s.printing : "high",
    changes: s.restrict ? s.changes : "allButExtract",
    copy: s.restrict ? s.copy : true,
    accessibility: s.restrict ? s.accessibility : true,
    cipher: s.cipher,
  };
}

export const PRINTING_LABEL: Record<Printing, string> = { none: "Not allowed", low: "Low resolution (150 dpi)", high: "High resolution" };
export const CHANGES_LABEL: Record<Changes, string> = {
  none: "Not allowed",
  pages: "Inserting, deleting and rotating pages",
  forms: "Filling in forms and signing",
  comments: "Commenting, filling in forms and signing",
  allButExtract: "Anything except extracting pages",
};

/** The outcome as short plain sentences, in reading order. */
export function outcome(s: Pick<ProtectSheet, "requireOpen" | "restrict" | "printing" | "changes" | "copy" | "accessibility">): string[] {
  const out = [s.requireOpen ? "Opens with a password." : "Opens without a password."];
  if (!s.restrict) {
    out.push("Anyone who opens it can print, copy and edit it.");
    return out;
  }
  out.push(s.printing === "none" ? "Can't be printed." : s.printing === "low" ? "Prints at low resolution." : "Prints at full resolution.");
  const edit = s.changes === "none" ? "edited" : null;
  if (!s.copy && edit) out.push("Can't be copied or edited.");
  else if (!s.copy) out.push("Can't be copied.");
  else if (edit) out.push("Can be copied, not edited.");
  if (s.changes === "pages") out.push("Pages can be inserted, deleted and rotated.");
  else if (s.changes === "forms") out.push("Forms can be filled in and signed.");
  else if (s.changes === "comments") out.push("It can be commented on and its forms filled in.");
  else if (s.changes === "allButExtract") out.push("It can be edited, but pages can't be extracted.");
  if (!s.accessibility) out.push("Screen readers are blocked.");
  return out;
}

/** What a document's security says, as short rows for the Security view and Document properties. */
export function securityRows(i: SecurityInfo): { label: string; value: string }[] {
  if (!i.encrypted) return [{ label: "Security", value: "None" }];
  const r = i.rights;
  const rows = [
    { label: "Encryption", value: i.method ?? "Unknown" },
    { label: "Opens with", value: i.openPassword ? "a password" : "no password" },
  ];
  if (!i.restricted) return [...rows, { label: "Restrictions", value: "None" }];
  const editing = !r.modify && !r.annotate && !r.forms && !r.assemble ? "Not allowed" : r.modify ? "Allowed" : [r.annotate && "Commenting", r.forms && "forms", r.assemble && "page changes"].filter(Boolean).join(", ");
  return [
    ...rows,
    { label: "Printing", value: r.printing === "none" ? "Not allowed" : r.printing === "low" ? "Low resolution" : "Allowed" },
    { label: "Copying", value: r.copy ? "Allowed" : "Not allowed" },
    { label: "Editing", value: editing },
    { label: "Page changes", value: r.assemble ? "Allowed" : "Not allowed" },
    { label: "Screen readers", value: r.accessibility ? "Allowed" : "Not allowed" },
  ];
}

/** One line for the notice above the page of a file that restricts something. */
export function restrictionNotice(i: SecurityInfo): string | null {
  if (!i.restricted) return null;
  const r = i.rights;
  const no: string[] = [];
  if (r.printing === "none") no.push("printing");
  if (!r.copy) no.push("copying");
  if (!r.modify && !r.annotate && !r.forms && !r.assemble) no.push("editing");
  const limits = r.printing === "low" ? "Printing is limited to low resolution" : null;
  const off = no.length ? (no.length > 1 ? `${no.slice(0, -1).join(", ")} and ${no[no.length - 1]}` : no[0]) : null;
  if (limits && off) return `This file limits printing to low resolution and doesn't allow ${off}.`;
  if (limits) return `${limits}.`;
  if (off) return `This file doesn't allow ${off}.`;
  return "This file has restrictions set by its owner.";
}

/** The engine's error codes as sentences. */
export function protectError(e: unknown): string {
  const s = String(e);
  const table: Record<string, string> = {
    "needs-permissions-password": "This file has restrictions. Type its permissions password to change them.",
    "bad-permissions-password": "That is not the permissions password of this file.",
    "nothing-to-set": "Choose a password to open the file, restrictions, or both.",
    "same-password": "The two passwords have to be different.",
    "password-too-long": "That password is too long (127 bytes at most).",
    "plain-text-only": "AES-128 files only take letters, digits and symbols from the keyboard.",
    "output-exists": "A file with that name already exists. Pick another name.",
  };
  return table[s] ?? s;
}

// ---- the store ----

export const useProtect = create<ProtectStore>((set, get) => {
  const patch = (p: Partial<ProtectSheet>) => set((s) => (s.sheet ? { sheet: { ...s.sheet, ...p } } : s));

  /** One right-side rail at a time; a running Compress / OCR keeps the right side. */
  function claim(): boolean {
    if (useTools.getState().sheet?.phase === "running" || useConvert.getState().sheet?.phase === "running") return false;
    useTools.getState().close();
    useConvert.getState().close();
    useHistory.getState().setOpen(false);
    useRedact.getState().setOpen(false);
    useAnnot.getState().setOpen(false);
    const ai = useAi.getState();
    if (ai.open && ai.settings.mode === "docked") ai.setOpen(false);
    return true;
  }

  return {
    sheet: null,
    security: {},
    dismissed: {},

    open: (view, docId) => {
      const doc = docId ?? st().active;
      const d = doc != null ? st().docs[doc] : undefined;
      if (!d || get().sheet?.phase === "working" || !claim()) return;
      if (d.organize) st().patch(d.id, { organize: false });
      const known = get().security[d.id];
      set({ sheet: blank(d.id, view ?? (known?.encrypted ? "security" : "protect")) });
      void get().refresh(d.id);
    },

    close: () => {
      if (get().sheet?.phase === "working") return;
      set({ sheet: null });
    },

    set: (p) => patch(p),

    refresh: async (doc) => {
      try {
        const info = await api.securityInfo(doc);
        // A document closed meanwhile has nothing to record.
        if (!st().docs[doc]) return;
        set((s) => ({ security: { ...s.security, [doc]: info } }));
      } catch {
        // A document that is still locked or being replaced has nothing to show yet.
      }
    },

    dismiss: (doc) => set((s) => ({ dismissed: { ...s.dismissed, [doc]: true } })),

    run: async (output) => {
      const sheet = get().sheet;
      if (!sheet || sheet.phase === "working" || readiness(sheet)) return;
      const d = st().docs[sheet.doc];
      if (!d) return;
      const info = get().security[sheet.doc];
      const needsOwner = info?.encrypted && !info.unlocked;
      patch({ phase: "working", error: null, confirmInPlace: false });
      try {
        let to: string | undefined;
        if (output === "copy") {
          // Rust shows the dialog; a dismissed one leaves the document as it was.
          const picked = await api.pickSave(`${stem(d.path)} (protected).pdf`, { doc: d.id });
          if (!picked) return void patch({ phase: "options" });
          to = picked.token;
        }
        const after = await api.protectApply(d.id, specOf(sheet), needsOwner ? sheet.currentPassword : null);
        applyState(d.id, after);
        const saved = await api.saveDocument(d.id, to);
        applyState(d.id, saved);
        void get().refresh(d.id);
        patch({
          phase: "done",
          // The passwords are not needed any more.
          openPassword: "",
          openConfirm: "",
          permissionsPassword: "",
          permissionsConfirm: "",
          currentPassword: "",
          result: { output, name: baseName(saved.path) },
        });
      } catch (e) {
        void get().refresh(d.id);
        patch({ phase: "error", error: protectError(e) });
      }
    },

    remove: async () => {
      const sheet = get().sheet;
      if (!sheet || sheet.phase === "working") return;
      const d = st().docs[sheet.doc];
      if (!d) return;
      const info = get().security[sheet.doc];
      patch({ phase: "working", error: null });
      try {
        const after = await api.removeSecurity(d.id, info && !info.unlocked ? sheet.currentPassword : null);
        applyState(d.id, after);
        void get().refresh(d.id);
        patch({ phase: "done", currentPassword: "", result: { output: "removed", name: baseName(after.path) } });
      } catch (e) {
        // Wrong password: stay on the sheet with the field, so it can be typed again.
        patch({ phase: "options", error: protectError(e) });
      }
    },
  };
});

// A new tab (or a reopened file) is read for its security so the lock and the notice show at once, an
// edit state read again; a closed one is forgotten.
useStore.subscribe((s, prev) => {
  for (const k of Object.keys(s.docs)) {
    const d = s.docs[Number(k)];
    const was = prev.docs[Number(k)];
    // An edit state landed (a protect or remove step, undoing one, a save): read it again.
    const moved = was && (d.rev !== was.rev || d.dirty !== was.dirty || d.undoLabel !== was.undoLabel || d.redoLabel !== was.redoLabel);
    if (!was || moved) void useProtect.getState().refresh(Number(k));
  }
  for (const k of Object.keys(prev.docs)) {
    const id = Number(k);
    if (s.docs[id]) continue;
    useProtect.setState((p) => {
      if (!(id in p.security) && !(id in p.dismissed) && p.sheet?.doc !== id) return p;
      const security = { ...p.security };
      const dismissed = { ...p.dismissed };
      delete security[id];
      delete dismissed[id];
      return { security, dismissed, sheet: p.sheet?.doc === id ? null : p.sheet };
    });
  }
});

// Another rail opening closes an idle Protect rail; a running one keeps the right side.
const yield_ = (opening: () => void) => {
  if (useProtect.getState().sheet?.phase === "working") return opening();
  if (useProtect.getState().sheet) useProtect.setState({ sheet: null });
};
useTools.subscribe((s, prev) => s.sheet && !prev.sheet && yield_(() => useTools.getState().close()));
useConvert.subscribe((s, prev) => s.sheet && !prev.sheet && yield_(() => useConvert.setState({ sheet: null })));
useHistory.subscribe((s, prev) => s.open && !prev.open && yield_(() => useHistory.getState().setOpen(false)));
useRedact.subscribe((s, prev) => s.open && !prev.open && yield_(() => useRedact.getState().setOpen(false)));
useAnnot.subscribe((s, prev) => s.open && !prev.open && yield_(() => useAnnot.getState().setOpen(false)));
useAi.subscribe((s, prev) => s.open && !prev.open && s.settings.mode === "docked" && yield_(() => useAi.getState().setOpen(false)));
