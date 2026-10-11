import { beforeEach, describe, expect, it, vi } from "vitest";

// store.ts reads localStorage at import time; vitest runs in node.
vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => Promise.resolve()), Channel: class {} }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("./edit", () => ({ save: vi.fn(), undo: vi.fn(), applyState: vi.fn() }));
vi.mock("./api", () => ({
  pickSave: vi.fn(),
  securityInfo: vi.fn(),
  protectApply: vi.fn(),
  protectAndSave: vi.fn(),
  removeSecurity: vi.fn(),
  saveDocument: vi.fn(),
  closeDocument: vi.fn(() => Promise.resolve()),
  cancelSearch: vi.fn(() => Promise.resolve()),
}));

import { useStore } from "../store";
import * as api from "./api";
import type { Rights, SecurityInfo } from "./api";
import { useConvert } from "./convertStore";
import * as edits from "./edit";
import { copyName, outcome, type ProtectSheet, protectError, readiness, restrictionNotice, securityRows, specOf, strength, useProtect } from "./protectStore";
import { useTools } from "./toolsStore";

const ALL: Rights = { printing: "high", modify: true, annotate: true, forms: true, assemble: true, copy: true, accessibility: true };
const info = (p: Partial<SecurityInfo> = {}): SecurityInfo => ({
  encrypted: false,
  method: null,
  revision: null,
  openPassword: false,
  rights: ALL,
  restricted: false,
  unlocked: true,
  origin: "file",
  unsaved: false,
  ...p,
});
const state = (path: string) => ({ rev: 1, pageCount: 1, sizes: [], path, dirty: false, undo: null, redo: null, redacted: false });
const flush = () => new Promise((r) => setTimeout(r, 0));

function sheet(p: Partial<ProtectSheet> = {}): ProtectSheet {
  useProtect.getState().open("protect", 1);
  useProtect.getState().set(p);
  return useProtect.getState().sheet!;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.securityInfo).mockResolvedValue(info());
  useProtect.setState({ sheet: null, security: {}, dismissed: {} });
  useConvert.setState({ sheet: null });
  useTools.setState({ sheet: null });
  useStore.setState({
    docs: { 1: { id: 1, path: "C:\\d\\Lease agreement.pdf", name: "Lease agreement.pdf", dirty: false, currentPage: 0, pageCount: 3 } as never },
    order: [1],
    active: 1,
  });
});

describe("strength", () => {
  it("is empty for nothing and weak for short or common passwords", () => {
    expect(strength("")).toEqual({ segments: 0, label: null });
    expect(strength("abc").label).toBe("Weak");
    expect(strength("password").label).toBe("Weak");
    expect(strength("Ab1!").segments).toBeLessThanOrEqual(2);
  });
  it("calls twelve characters of one kind fair and twelve of three kinds strong", () => {
    expect(strength("lanternfish").label).toBe("Weak");
    expect(strength("lanternfish77").label).toBe("Fair");
    expect(strength("Lantern7fish!").label).toBe("Strong");
    expect(strength("Lantern7fish!").segments).toBe(5);
  });
  it("counts characters, not bytes", () => {
    expect(strength("åäöåäöåäöåäö").segments).toBeGreaterThanOrEqual(3);
  });
});

describe("readiness", () => {
  it("needs something to set", () => {
    expect(readiness({ ...sheet(), requireOpen: false, restrict: false })).toMatch(/Choose a password/);
  });
  it("needs the open password typed twice the same", () => {
    const ok = { ...sheet(), requireOpen: true, openPassword: "Lantern7fish!", openConfirm: "Lantern7fish!" };
    expect(readiness(ok)).toBeNull();
    expect(readiness({ ...ok, openPassword: "" })).toMatch(/Type the password/);
    expect(readiness({ ...ok, openConfirm: "x" })).toMatch(/don't match/);
  });
  it("needs a permissions password for restrictions, different from the open one", () => {
    const ok = { ...sheet(), requireOpen: true, openPassword: "Lantern7fish!", openConfirm: "Lantern7fish!" };
    expect(readiness({ ...ok, restrict: true })).toMatch(/permissions password/);
    expect(readiness({ ...ok, restrict: true, permissionsPassword: "Lantern7fish!", permissionsConfirm: "Lantern7fish!" })).toMatch(/different/);
    expect(readiness({ ...ok, restrict: true, permissionsPassword: "Other-pw-77", permissionsConfirm: "Other-pw-77" })).toBeNull();
  });
  it("limits length in bytes and AES-128 to keyboard characters", () => {
    const ok = { ...sheet(), requireOpen: true, openPassword: "Lantern7fish!", openConfirm: "Lantern7fish!" };
    const long = "ä".repeat(64);
    expect(readiness({ ...ok, openPassword: long, openConfirm: long })).toMatch(/too long/);
    expect(readiness({ ...ok, cipher: "aes128", openPassword: "löv", openConfirm: "löv" })).toMatch(/AES-256/);
    expect(readiness({ ...ok, openPassword: "löv", openConfirm: "löv" })).toBeNull();
  });
  it("refuses control characters, NUL included", () => {
    const ok = { ...sheet(), requireOpen: true, openPassword: "Lantern7fish!", openConfirm: "Lantern7fish!" };
    const c = (n: number) => String.fromCharCode(n);
    for (const bad of ["a" + c(0) + "b-secret", "tab" + c(9) + "here-1", "del" + c(127) + "here-1", "c1" + c(0x85) + "here-1"]) {
      expect(readiness({ ...ok, openPassword: bad, openConfirm: bad })).toMatch(/control characters/);
    }
    const p = { ...ok, restrict: true, permissionsPassword: "ctl" + c(1) + "-pass", permissionsConfirm: "ctl" + c(1) + "-pass" };
    expect(readiness(p)).toMatch(/control characters/);
  });
});

describe("request and outcome", () => {
  it("sends only what is switched on", () => {
    const s = sheet({ requireOpen: true, openPassword: "a-b-c-1234", openConfirm: "a-b-c-1234", restrict: false, printing: "none", copy: false });
    expect(specOf(s)).toEqual({
      openPassword: "a-b-c-1234",
      permissionsPassword: null,
      printing: "high",
      changes: "allButExtract",
      copy: true,
      accessibility: true,
      cipher: "aes256",
    });
  });
  it("sends the restrictions with the permissions password", () => {
    const s = sheet({ requireOpen: false, restrict: true, permissionsPassword: "own3r-pw!", permissionsConfirm: "own3r-pw!", printing: "low", changes: "comments", copy: false, cipher: "aes128" });
    expect(specOf(s)).toMatchObject({ openPassword: null, permissionsPassword: "own3r-pw!", printing: "low", changes: "comments", copy: false, cipher: "aes128" });
  });
  it("writes the outcome in plain sentences", () => {
    expect(outcome({ requireOpen: true, restrict: false, printing: "high", changes: "none", copy: true, accessibility: true })).toEqual([
      "Opens with a password.",
      "Anyone who opens it can print, copy and edit it.",
    ]);
    expect(outcome({ requireOpen: true, restrict: true, printing: "low", changes: "none", copy: false, accessibility: true })).toEqual([
      "Opens with a password.",
      "Prints at low resolution.",
      "Can't be copied or edited.",
    ]);
    expect(outcome({ requireOpen: false, restrict: true, printing: "none", changes: "comments", copy: true, accessibility: false })).toEqual([
      "Opens without a password.",
      "Can't be printed.",
      "It can be commented on and its forms filled in.",
      "Screen readers are blocked.",
    ]);
  });
});

describe("what a file says", () => {
  const locked = info({
    encrypted: true,
    method: "AES-256",
    openPassword: false,
    restricted: true,
    unlocked: false,
    rights: { ...ALL, printing: "low", copy: false, modify: false, assemble: false },
  });
  it("lists the rows and the notice", () => {
    expect(securityRows(info())).toEqual([{ label: "Security", value: "None" }]);
    const rows = securityRows(locked);
    expect(rows).toContainEqual({ label: "Encryption", value: "AES-256" });
    expect(rows).toContainEqual({ label: "Printing", value: "Low resolution" });
    expect(rows).toContainEqual({ label: "Copying", value: "Not allowed" });
    expect(restrictionNotice(locked)).toBe("This file limits printing to low resolution and doesn't allow copying.");
    expect(restrictionNotice(info({ encrypted: true, openPassword: true }))).toBeNull();
  });
  it("turns the engine's codes into sentences", () => {
    expect(protectError("bad-permissions-password")).toMatch(/not the permissions password/);
    expect(protectError("something else")).toBe("something else");
  });
});

describe("the sheet", () => {
  it("opens on Protect for a plain file and on Security for a protected one", async () => {
    useProtect.getState().open(undefined, 1);
    expect(useProtect.getState().sheet).toMatchObject({ doc: 1, view: "protect", phase: "options" });
    useProtect.getState().close();
    useProtect.setState({ security: { 1: info({ encrypted: true, method: "AES-256" }) } });
    useProtect.getState().open(undefined, 1);
    expect(useProtect.getState().sheet?.view).toBe("security");
    await flush();
  });

  it("closes the other idle rails and gives way to them", () => {
    useConvert.setState({ sheet: { mode: "export", doc: 1, phase: "options" } as never });
    useProtect.getState().open("protect", 1);
    expect(useConvert.getState().sheet).toBeNull();
    useConvert.setState({ sheet: { mode: "export", doc: 1, phase: "options" } as never });
    expect(useProtect.getState().sheet).toBeNull();
  });

  it("protects a copy: file picked first, then one protect-and-save", async () => {
    sheet({ openPassword: "Lantern7fish!", openConfirm: "Lantern7fish!" });
    vi.mocked(api.pickSave).mockResolvedValue({ token: "save-1", path: "C:\\d\\Lease agreement (protected).pdf" });
    vi.mocked(api.protectAndSave).mockResolvedValue(state("C:\\d\\Lease agreement (protected).pdf") as never);
    await useProtect.getState().run("copy");
    expect(api.pickSave).toHaveBeenCalledWith("Lease agreement (protected).pdf", { doc: 1 });
    expect(api.protectAndSave).toHaveBeenCalledWith(1, expect.objectContaining({ openPassword: "Lantern7fish!", cipher: "aes256" }), null, "save-1");
    expect(api.protectApply).not.toHaveBeenCalled();
    expect(api.saveDocument).not.toHaveBeenCalled();
    expect(edits.applyState).toHaveBeenCalledTimes(1);
    const s = useProtect.getState().sheet!;
    expect(s.phase).toBe("done");
    expect(s.result).toEqual({ output: "copy", name: "Lease agreement (protected).pdf" });
    // the passwords are gone from the sheet
    expect(s.openPassword).toBe("");
    expect(s.openConfirm).toBe("");
  });

  it("changes nothing when the save dialog is dismissed", async () => {
    sheet({ openPassword: "Lantern7fish!", openConfirm: "Lantern7fish!" });
    vi.mocked(api.pickSave).mockResolvedValue(null);
    await useProtect.getState().run("copy");
    expect(api.protectAndSave).not.toHaveBeenCalled();
    expect(useProtect.getState().sheet?.phase).toBe("options");
  });

  it("protects this file in place with one protect-and-save over the file", async () => {
    sheet({ openPassword: "Lantern7fish!", openConfirm: "Lantern7fish!" });
    vi.mocked(api.protectAndSave).mockResolvedValue(state("C:\\d\\Lease agreement.pdf") as never);
    await useProtect.getState().run("inPlace");
    expect(api.pickSave).not.toHaveBeenCalled();
    expect(api.protectAndSave).toHaveBeenCalledWith(1, expect.anything(), null, undefined);
    expect(useProtect.getState().sheet?.result?.output).toBe("inPlace");
  });

  it("a failed save leaves the sheet on the error with nothing applied here", async () => {
    sheet({ openPassword: "Lantern7fish!", openConfirm: "Lantern7fish!" });
    vi.mocked(api.pickSave).mockResolvedValue({ token: "save-1", path: "C:\\d\\x.pdf" });
    vi.mocked(api.protectAndSave).mockRejectedValue("The file could not be written: disk full");
    await useProtect.getState().run("copy");
    expect(edits.applyState).not.toHaveBeenCalled();
    expect(useProtect.getState().sheet).toMatchObject({ phase: "error", error: "The file could not be written: disk full" });
    // a retry is one call again, never a second step
    vi.mocked(api.protectAndSave).mockResolvedValue(state("C:\\d\\x.pdf") as never);
    await useProtect.getState().run("copy");
    expect(api.protectAndSave).toHaveBeenCalledTimes(2);
    expect(api.protectApply).not.toHaveBeenCalled();
  });

  it("sends the permissions password of a restricted file, and shows the engine's refusal", async () => {
    useProtect.setState({ security: { 1: info({ encrypted: true, restricted: true, unlocked: false, method: "AES-256" }) } });
    sheet({ openPassword: "Lantern7fish!", openConfirm: "Lantern7fish!", currentPassword: "wrong" });
    vi.mocked(api.pickSave).mockResolvedValue({ token: "save-1", path: "C:\\d\\x.pdf" });
    vi.mocked(api.protectAndSave).mockRejectedValue("bad-permissions-password");
    await useProtect.getState().run("copy");
    expect(api.protectAndSave).toHaveBeenCalledWith(1, expect.anything(), "wrong", "save-1");
    expect(useProtect.getState().sheet).toMatchObject({ phase: "error", error: "That is not the permissions password of this file." });
  });

  it("removes security with the permissions password and stays on the sheet when it is wrong", async () => {
    useProtect.setState({ security: { 1: info({ encrypted: true, restricted: true, unlocked: false }) } });
    useProtect.getState().open("security", 1);
    useProtect.getState().set({ currentPassword: "nope" });
    vi.mocked(api.removeSecurity).mockRejectedValueOnce("bad-permissions-password");
    await useProtect.getState().remove();
    expect(api.removeSecurity).toHaveBeenCalledWith(1, "nope");
    expect(useProtect.getState().sheet).toMatchObject({ phase: "options", view: "security" });
    expect(useProtect.getState().sheet?.error).toMatch(/not the permissions password/);

    vi.mocked(api.removeSecurity).mockResolvedValueOnce({ ...state("C:\\d\\Lease agreement.pdf"), dirty: true } as never);
    await useProtect.getState().remove();
    expect(useProtect.getState().sheet).toMatchObject({ phase: "done", result: { output: "removed" } });
  });

  it("does not send a password it doesn't need", async () => {
    useProtect.setState({ security: { 1: info({ encrypted: true, unlocked: true, method: "AES-256", openPassword: true }) } });
    useProtect.getState().open("security", 1);
    vi.mocked(api.removeSecurity).mockResolvedValue(state("C:\\d\\Lease agreement.pdf") as never);
    await useProtect.getState().remove();
    expect(api.removeSecurity).toHaveBeenCalledWith(1, null);
  });

  it("reads the security of a document when its tab appears and forgets it when it closes", async () => {
    vi.mocked(api.securityInfo).mockResolvedValue(info({ encrypted: true, method: "AES-128", restricted: true, unlocked: false }));
    useStore.setState({ docs: { ...useStore.getState().docs, 2: { id: 2, path: "C:\\d\\b.pdf", name: "b.pdf" } as never }, order: [1, 2] });
    await flush();
    expect(useProtect.getState().security[2]?.method).toBe("AES-128");
    useStore.setState({ docs: { 1: useStore.getState().docs[1] }, order: [1] });
    expect(useProtect.getState().security[2]).toBeUndefined();
  });
});

describe("copyName", () => {
  it("names the copy after the file without its extension", () => {
    expect(copyName("C:\\docs\\Annual report.pdf")).toBe("Annual report (protected).pdf");
    expect(copyName("/x/y/scan.PDF")).toBe("scan (protected).pdf");
    expect(copyName("noext")).toBe("noext (protected).pdf");
  });
});
