import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./edit", () => ({ applyState: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { applyState } from "./edit";
import {
  type Identity, LOCKED_REASON, clickSigField, type Saved, type SignatureInfo, STATUS, sigFieldView, banner, changeNote, detailLines, imageRect, locked, otherWarnings, outcome, pdfDate, sigBox, statusWord, useSign,
} from "./sign";

const inv = vi.mocked(invoke);
const state = { rev: 2, pageCount: 2, sizes: [], path: "C:\\a.pdf", dirty: true, undo: "Add check on p. 1", redo: null };
const s = () => useSign.getState();

function reply(map: Record<string, unknown>) {
  inv.mockImplementation((cmd: string) => (cmd in map ? Promise.resolve(map[cmd]) : Promise.reject(`unexpected ${cmd}`)));
}

const sig = (p: Partial<SignatureInfo>): SignatureInfo => ({
  field: "Signature1", page: 0, rect: [0, 0, 10, 10], status: "valid", summary: "", signer: "Maple Property", issuer: "Maple Property",
  time: "D:20261011143200+02'00'", reason: "", location: "", subFilter: "ETSI.CAdES.detached", digest: "SHA-256", certify: null,
  intact: true, changes: "none", trusted: true, trustNote: "", warnings: [], certNotAfter: "2031-10-11", certFingerprint: "AA", ...p,
});
const saved = (id: string, kind: Saved["kind"]): Saved => ({ id, kind, source: "drawn", font: null, created: 1, width: 300, height: 100, dataUrl: "data:image/png;base64,AAAA" });
const testId: Identity = {
  source: "test", handle: 3, fingerprint: null, name: "Maple Property", email: "m@example.com", issuer: "Maple Property", selfSigned: true,
  notAfter: "2031-10-11", unusable: null,
};

beforeEach(() => {
  vi.clearAllMocks();
  useSign.setState({ docs: {}, saved: [], pick: {}, ids: [], idsLoaded: false, idChoice: null, reason: "", lock: false, picture: false, rail: null, tool: "select", open: false });
});

describe("helpers", () => {
  it("formats PDF dates and appearance lines", () => {
    expect(pdfDate("D:20261011143200+02'00'")).toBe("11 Oct 2026, 14:32");
    expect(pdfDate("D:20261011")).toBe("11 Oct 2026");
    expect(pdfDate("yesterday")).toBe("yesterday");
    const l = detailLines("Maple Property", new Date(2026, 9, 11, 14, 32, 5), " Lease signed ");
    expect(l[0]).toBe("Digitally signed by Maple Property");
    expect(l[1]).toMatch(/^Date: 2026\.10\.11 14:32:05 [+-]\d\d'\d\d'$/);
    expect(l[2]).toBe("Reason: Lease signed");
    expect(detailLines("A", new Date(), "")).toHaveLength(2);
  });

  it("says the worst state in the banner", () => {
    expect(banner([])).toBeNull();
    expect(banner([sig({ status: "unsigned" })])).toBeNull();
    expect(banner([sig({}), sig({})])).toEqual({ tone: "valid", text: "Signed. All signatures are valid." });
    expect(banner([sig({}), sig({ status: "unknownSigner" })])?.tone).toBe("unknown");
    expect(banner([sig({ status: "unknownSigner" }), sig({ status: "invalid" })])).toEqual({ tone: "invalid", text: "At least one signature is invalid." });
  });

  it("warns when a signed document was changed after signing, the hash is weak, or a box was added later", () => {
    const changedW = { code: "changed" as const, text: "The document was changed after it was signed." };
    const sha1W = { code: "sha1" as const, text: "It uses SHA-1, a hash algorithm that is no longer safe." };
    const boxW = { code: "laterBox" as const, text: "A later signature added a visible box on page 1." };
    const changed = sig({ status: "warning", changes: "changed", warnings: [changedW] });
    const weak = sig({ status: "warning", warnings: [sha1W] });
    // A later box on a document whose signatures are otherwise untouched: not "weak".
    const boxed = sig({ status: "warning", changes: "signaturesOnly", warnings: [boxW] });
    expect(banner([changed])).toEqual({ tone: "unknown", text: "Signature valid, but the document was changed after signing." });
    expect(banner([weak, changed])?.text).toBe("Signatures valid, but the document was changed after signing.");
    expect(banner([weak])?.text).toBe("Signed, but the signature uses a weak hash algorithm.");
    expect(banner([boxed, sig({})])?.text).toBe("Signatures valid, but a later signature added a visible box.");
    expect(banner([changed, sig({ status: "invalid" })])?.tone).toBe("invalid");
    expect(statusWord(changed)).toBe("Valid, but changed after signing");
    expect(statusWord(weak)).toBe("Valid, but weak");
    expect(statusWord(boxed)).toBe("Valid, but a box was added later");
    expect(otherWarnings(changed)).toEqual([]);
    expect(otherWarnings(weak)).toEqual([sha1W.text]);
    // Trust comes first: an untrusted signer with a caution is never called valid.
    const untrusted = sig({ status: "unknownSigner", trusted: false, changes: "changed", warnings: [changedW, sha1W] });
    expect(statusWord(untrusted)).toBe("Unknown signer");
    expect(banner([untrusted, changed])).toEqual({
      tone: "unknown",
      text: "Signed. The signatures are intact, but a signer is not in your trusted list. The document was changed after signing.",
    });
    expect(STATUS.warning.tone).toBe("unknown");
  });

  it("explains changes and what signing will do", () => {
    expect(changeNote(sig({}))).toBe("No changes since it was signed.");
    expect(changeNote(sig({ changes: "changed" }))).toMatch(/changed after/);
    expect(changeNote(sig({ intact: false, status: "invalid" }))).toMatch(/damaged/);
    expect(outcome([], false, false)).toBe("Saves the file. Later changes will show as made after signing.");
    expect(outcome([sig({})], false, true)).toBe("Saves the file with your other changes. Adds a new revision; the earlier signature stays valid.");
    expect(outcome([], true, false)).toMatch(/Locks the document/);
    expect(locked([sig({ certify: 1 })])).toBe(true);
    expect(locked([sig({ certify: 2 })])).toBe(false);
  });

  it("keeps placed pictures and signature boxes on the page", () => {
    // 3:1 picture, 150 pt wide, centred on the click, pushed back inside the page.
    expect(imageRect([100, 100], { width: 300, height: 100 }, 150, [612, 792])).toEqual([25, 75, 175, 125]);
    expect(imageRect([600, 790], { width: 300, height: 100 }, 150, [612, 792])).toEqual([462, 742, 612, 792]);
    expect(sigBox([300, 600], [500, 680], [612, 792])).toEqual([300, 600, 500, 680]);
    // A click (no drag) makes a 200 x 60 box; near the edge it moves inside.
    expect(sigBox([500, 760], [501, 761], [612, 792])).toEqual([412, 732, 612, 792]);
  });
});

describe("sign store", () => {
  it("places an item as one edit and selects it", async () => {
    reply({ fillsign_add: { state, xref: 12 }, fillsign_list: [{ page: 0, xref: 12, item: "check", rect: [0, 0, 12, 12] }] });
    await s().place(1, { page: 0, type: "mark", mark: "check", at: [50, 50] });
    expect(inv).toHaveBeenCalledWith("fillsign_add", { doc: 1, item: { page: 0, type: "mark", mark: "check", at: [50, 50] } });
    expect(applyState).toHaveBeenCalledWith(1, state);
    expect(s().get(1).selected).toBe(12);
    expect(s().get(1).items).toHaveLength(1);
  });

  it("keeps an engine error on the document", async () => {
    inv.mockRejectedValue("the text is empty");
    expect(await s().place(1, { page: 0, type: "text", at: [0, 0], text: " " })).toBeNull();
    expect(s().get(1).error).toBe("the text is empty");
  });

  it("picks the newest saved signature and initials, and keeps a valid pick", async () => {
    reply({ fillsign_saved_list: [saved("a", "signature"), saved("b", "signature"), saved("c", "initials")] });
    await s().loadSaved();
    expect(s().pick).toEqual({ signature: "b", initials: "c" });
    useSign.setState({ pick: { signature: "a", initials: "gone" } });
    await s().loadSaved();
    expect(s().pick).toEqual({ signature: "a", initials: "c" });
  });

  it("chooses the first usable ID", async () => {
    reply({ sign_identities: [{ ...testId, handle: 1, unusable: "expired" }, testId] });
    await s().loadIds();
    expect(s().idChoice).toBe("s3");
  });

  it("never picks a Windows store ID for the user", async () => {
    useSign.setState({ idChoice: null });
    reply({ sign_identities: [{ ...testId, handle: 7, source: "store" }] });
    await s().loadIds();
    expect(s().idChoice).toBeNull();
    reply({ sign_identities: [{ ...testId, handle: 7, source: "store" }, testId] });
    await s().loadIds();
    expect(s().idChoice).toBe("s3");
  });

  it("signs the draft with the sheet's options and shows the list after", async () => {
    useSign.setState({ ids: [testId], idChoice: "s3", reason: "Lease signed", lock: true, picture: true, saved: [saved("a", "signature")], pick: { signature: "a" } });
    s().setDraft(1, { page: 3, rect: [300, 600, 500, 680] });
    const signedState = { ...state, rev: 5, dirty: false };
    reply({ sign_document: signedState, sign_list: [sig({ certify: 1 })] });
    expect(await s().sign(1, null)).toBe(true);
    const call = inv.mock.calls.find((c) => c[0] === "sign_document")![1] as Record<string, unknown>;
    expect(call.id).toEqual({ kind: "session", handle: 3 });
    expect(call.to).toBeNull();
    expect(call.options).toMatchObject({ page: 3, rect: [300, 600, 500, 680], reason: "Lease signed", certify: 1, image: "AAAA" });
    expect(applyState).toHaveBeenCalledWith(1, signedState);
    expect(s().get(1).draft).toBeNull();
    expect(s().get(1).sigsRev).toBe(5);
    expect(s().rail).toBe("list");
    expect(s().lock).toBe(false);
  });

  it("signs an existing field by name and never certifies a signed document", async () => {
    useSign.setState({ ids: [testId], idChoice: "s3", lock: true });
    useSign.setState({ docs: { 1: { ...s().get(1), sigs: [sig({})], draft: { page: 0, rect: [0, 0, 1, 1], field: "Tenant" } } } });
    reply({ sign_document: state, sign_list: [] });
    await s().sign(1, "save-token");
    const call = inv.mock.calls.find((c) => c[0] === "sign_document")![1] as { options: Record<string, unknown>; to: string };
    expect(call.options.field).toBe("Tenant");
    expect(call.options.rect).toBeUndefined();
    expect(call.options.certify).toBeUndefined();
    expect(call.to).toBe("save-token");
  });

  it("does nothing without a box or an ID, and keeps the draft when signing fails", async () => {
    expect(await s().sign(1, null)).toBe(false);
    expect(inv).not.toHaveBeenCalled();
    useSign.setState({ ids: [testId], idChoice: "s3" });
    s().setDraft(1, { page: 0, rect: [0, 0, 100, 40] });
    inv.mockRejectedValue("the password is incorrect");
    expect(await s().sign(1, null)).toBe(false);
    expect(s().get(1).draft).not.toBeNull();
    expect(s().get(1).error).toBe("the password is incorrect");
    expect(s().signing).toBe(false);
  });

  it("shows the banner again when the number of signatures changes", async () => {
    reply({ sign_list: [sig({})] });
    await s().refreshSigs(1, 1);
    s().hideBanner(1);
    await s().refreshSigs(1, 2);
    expect(s().get(1).bannerHidden).toBe(true);
    reply({ sign_list: [sig({}), sig({ field: "B" })] });
    await s().refreshSigs(1, 3);
    expect(s().get(1).bannerHidden).toBe(false);
  });
});

describe("signature form fields on the page", () => {
  const empty = sig({ field: "Tenant", status: "unsigned", signer: "", rect: [10, 20, 110, 60], page: 1 });
  const done = sig({ field: "Landlord", signer: "Maple Property" });

  it("titles an empty field, a signed field and a field not read yet", () => {
    expect(sigFieldView("Tenant", empty, [empty])).toEqual({ state: "sign", title: "Tenant: click to sign" });
    expect(sigFieldView("Landlord", done, [done, empty])).toEqual({ state: "signed", title: "Landlord: signed by Maple Property, click for details" });
    expect(sigFieldView("Landlord", sig({ signer: "" }), [])).toEqual({ state: "signed", title: "Landlord: signed, click for details" });
    expect(sigFieldView("Tenant", undefined, [])).toEqual({ state: "unknown", title: "Tenant" });
  });

  it("is not actionable on a document certified with no changes allowed, and says why", () => {
    const cert = sig({ field: "Landlord", certify: 1 });
    expect(sigFieldView("Tenant", empty, [cert, empty])).toEqual({ state: "locked", title: `Tenant: ${LOCKED_REASON}` });
    // P=2 and P=3 allow signing, as the engine does.
    expect(sigFieldView("Tenant", empty, [sig({ certify: 2 }), empty]).state).toBe("sign");
    // a signed field still opens its details
    expect(sigFieldView("Landlord", cert, [cert]).state).toBe("signed");
  });

  it("an empty field click requests the sign flow with that field as the target, and signs nothing", () => {
    clickSigField(7, "Tenant", empty, [empty]);
    expect(s().get(7).draft).toEqual({ page: 1, rect: [10, 20, 110, 60], field: "Tenant" });
    expect(s().rail).toBe("sheet");
    expect(inv).not.toHaveBeenCalled();
  });

  it("a refused field click does nothing", () => {
    clickSigField(7, "Tenant", empty, [sig({ certify: 1 }), empty]);
    expect(s().get(7).draft).toBeNull();
    expect(s().rail).toBeNull();
    expect(inv).not.toHaveBeenCalled();
  });

  it("a signed field click opens the rail on that signature", () => {
    clickSigField(7, "Landlord", done, [done]);
    expect(s().rail).toBe("list");
    expect(s().get(7).focus).toBe("Landlord");
    expect(s().get(7).draft).toBeNull();
    expect(inv).not.toHaveBeenCalled();
  });
});
