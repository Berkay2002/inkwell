import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => Promise.resolve()) }));

import { invoke } from "@tauri-apps/api/core";
import { type MediaItem, badgeText, durationText, formatLabel, infoKey, mediaUrl, useMedia } from "./media";

const inv = vi.mocked(invoke);
const TOKEN = "0123456789abcdef0123456789abcdef";

const item = (xref: number, over: Partial<MediaItem> = {}): MediaItem => ({
  page: 0,
  xref,
  kind: "richMedia",
  rect: [0, 0, 100, 50],
  title: `Clip ${xref}`,
  playable: true,
  play: `${xref}.0`,
  reason: null,
  scripts: false,
  assets: [{ id: `${xref}.0`, name: "a.mp4", mime: "video/mp4", size: 10, embedded: true, playable: true, reason: null, element: "video" }],
  ...over,
});

const flush = () => new Promise((r) => setTimeout(r, 0));
const playNow = async (doc: number, it: MediaItem) => {
  useMedia.getState().play(doc, it);
  await flush();
};

beforeEach(() => {
  inv.mockReset();
  inv.mockResolvedValue(undefined);
  useMedia.setState({ docs: {}, loading: {}, playing: null, muted: false, info: {} });
});

describe("media store", () => {
  it("plays nothing until a click, and one player at a time", async () => {
    expect(useMedia.getState().playing).toBeNull();
    useMedia.getState().play(1, item(10));
    await flush();
    expect(useMedia.getState().playing).toEqual({ doc: 1, xref: 10, asset: "10.0" });
    useMedia.getState().play(1, item(11));
    await flush();
    expect(useMedia.getState().playing).toEqual({ doc: 1, xref: 11, asset: "11.0" });
  });

  it("arms the asset with the route before the player is mounted, and a stop in between wins", async () => {
    let arm: () => void = () => {};
    inv.mockImplementation((cmd) => (cmd === "media_play" ? new Promise<void>((r) => (arm = r)) : Promise.resolve()));
    useMedia.getState().play(1, item(10));
    expect(inv).toHaveBeenCalledWith("media_play", { doc: 1, asset: "10.0" });
    expect(useMedia.getState().playing).toBeNull();
    arm();
    await flush();
    expect(useMedia.getState().playing?.asset).toBe("10.0");
    // a click, then Esc/scroll-away before the route answers: no player
    useMedia.getState().stop();
    useMedia.getState().play(1, item(11));
    useMedia.getState().stop();
    arm();
    await flush();
    expect(useMedia.getState().playing).toBeNull();
    // a failed arming (the asset left the listing) plays nothing
    inv.mockImplementation((cmd) => (cmd === "media_play" ? Promise.reject(new Error("unknown media")) : Promise.resolve()));
    useMedia.getState().play(1, item(12));
    await flush();
    expect(useMedia.getState().playing).toBeNull();
  });

  it("does not play an item that cannot play", async () => {
    await playNow(1, item(10, { playable: false, play: null, reason: "3D content is not supported" }));
    expect(useMedia.getState().playing).toBeNull();
  });

  it("Esc removes the player, other keys do not", async () => {
    await playNow(1, item(10));
    expect(useMedia.getState().key("a")).toBe(false);
    expect(useMedia.getState().playing).not.toBeNull();
    expect(useMedia.getState().key("Escape")).toBe(true);
    expect(useMedia.getState().playing).toBeNull();
    // nothing playing: Esc is left to everything else
    expect(useMedia.getState().key("Escape")).toBe(false);
    expect(inv).toHaveBeenCalledWith("media_release");
  });

  it("remembers muted for the session", async () => {
    useMedia.getState().setMuted(true);
    await playNow(1, item(10));
    useMedia.getState().stop();
    expect(useMedia.getState().muted).toBe(true);
  });

  it("reads the list once per revision however many pages ask", async () => {
    inv.mockResolvedValue({ token: TOKEN, items: [item(10)] });
    useMedia.getState().ensure(1, 0);
    useMedia.getState().ensure(1, 0);
    useMedia.getState().ensure(1, 0);
    await flush();
    expect(inv.mock.calls.filter((c) => c[0] === "media_list")).toHaveLength(1);
    useMedia.getState().ensure(1, 0);
    expect(inv.mock.calls.filter((c) => c[0] === "media_list")).toHaveLength(1);
    expect(useMedia.getState().docs[1].items).toHaveLength(1);
    useMedia.getState().ensure(1, 1);
    await flush();
    expect(inv.mock.calls.filter((c) => c[0] === "media_list")).toHaveLength(2);
  });

  it("keeps the player's address through an edit that keeps the asset", async () => {
    inv.mockResolvedValue({ token: TOKEN, items: [item(10)] });
    useMedia.getState().ensure(1, 0);
    await flush();
    const url = () => mediaUrl(1, useMedia.getState().docs[1].token, "10.0");
    const before = url();
    expect(before).toMatch(/\/m\/1\/0123456789abcdef0123456789abcdef\/10\.0$/);
    await playNow(1, item(10));
    useMedia.getState().ensure(1, 1);
    await flush();
    expect(url()).toBe(before);
    expect(useMedia.getState().playing).not.toBeNull();
  });

  it("stops what played when an edit takes it away", async () => {
    useMedia.setState({ docs: { 1: { rev: 0, token: TOKEN, items: [item(10)] } } });
    await playNow(1, item(10));
    inv.mockResolvedValue({ token: TOKEN, items: [] });
    useMedia.getState().ensure(1, 1);
    await flush();
    expect(useMedia.getState().playing).toBeNull();
  });

  it("forgets a closed document and stops its player", async () => {
    useMedia.setState({ docs: { 1: { rev: 0, token: TOKEN, items: [item(10)] }, 2: { rev: 0, token: TOKEN, items: [] } } });
    await playNow(1, item(10));
    useMedia.getState().forget(1);
    expect(useMedia.getState().playing).toBeNull();
    expect(Object.keys(useMedia.getState().docs)).toEqual(["2"]);
  });
});

describe("media labels", () => {
  it("shows duration and format once the element knows them", () => {
    const it1 = item(10);
    expect(badgeText(it1, undefined)).toBe("MP4");
    useMedia.getState().setInfo(1, 10, 75);
    expect(badgeText(it1, useMedia.getState().info[infoKey(1, 10)])).toBe("MP4 · 1:15");
    expect(durationText(3725)).toBe("1:02:05");
    expect(durationText(Number.NaN)).toBe("");
    expect(formatLabel("audio/mpeg")).toBe("MP3");
  });

  it("says why a badge is crossed out", () => {
    expect(badgeText(item(10, { playable: false, reason: "Flash (SWF) is end of life and is never run" }), undefined)).toContain("Flash");
  });
});
