import { create } from "zustand";
import { barWidth } from "./barWidth";

/** The command bar is one surface in three states: `open` (the full chat, header on top, composer at the
 *  bottom), `header` (the header strip over the composer) and `bar` (the composer alone). */
export type BarView = "open" | "header" | "bar";
export type BarEvent =
  | "send" // a question was asked: the answer must be seen
  | "attention" // the assistant waits on an approval
  | "headerClick"
  | "minimize" // the "—" button
  | "esc"
  | "outside" // a press on the page
  | "focus"; // the composer was entered
export type BarAt = "left" | "center" | "right";

/** What each event does to the view. Esc and "—" step down one stage; a press on the page only closes the chat. */
export function barReduce(v: BarView, e: BarEvent): BarView {
  switch (e) {
    case "send":
    case "attention":
      return "open";
    case "headerClick":
      return v === "header" ? "open" : v === "open" ? "header" : v;
    case "minimize":
    case "esc":
      return v === "open" ? "header" : "bar";
    case "outside":
      return v === "open" ? "header" : v;
    case "focus":
      return v === "bar" ? "header" : v;
  }
}

/** What is on screen: the stage the user is in, unless there is nothing for it to show. Gate and consent are
 *  always the open surface; an empty chat (after "New chat", or a tab without one) has nothing to stretch
 *  into, so it is a header while there are earlier chats to go back to and the bare bar otherwise. */
export function barStage(view: BarView, o: { forced: boolean; hasChat: boolean; hasPast: boolean }): BarView {
  if (o.forced) return "open";
  if (o.hasChat) return view;
  return o.hasPast && view !== "bar" ? "header" : "bar";
}

/** The surface keeps one width in every stage so it never jumps: the document area's share, at most 640. */
export function surfaceWidth(stageW: number): number {
  return Math.min(640, barWidth(stageW));
}

/** The open surface's height limit: 70 % of the document area, at least 240 px, always inside the area. */
export function surfaceMaxH(stageH: number): number {
  return Math.max(120, Math.min(Math.max(240, Math.round(stageH * 0.7)), stageH - 48));
}

const EDGE = 24;

/** Left edge of the surface, in window coordinates. */
export function surfaceLeft(at: BarAt, l: number, r: number, w: number): number {
  if (at === "left") return Math.round(l + EDGE);
  if (at === "right") return Math.round(r - EDGE - w);
  return Math.round((l + r) / 2 - w / 2);
}

const KEY = "inkwell.ai.barAt";
const loadAt = (): BarAt => {
  try {
    const v = localStorage.getItem(KEY);
    return v === "left" || v === "right" ? v : "center";
  } catch {
    return "center";
  }
};

interface BarStore {
  view: BarView;
  at: BarAt;
  dispatch: (e: BarEvent) => void;
  setAt: (at: BarAt) => void;
}

export const useBar = create<BarStore>((set, get) => ({
  view: "bar",
  at: loadAt(),
  dispatch: (e) => {
    const next = barReduce(get().view, e);
    if (next !== get().view) set({ view: next });
  },
  setAt: (at) => {
    try {
      localStorage.setItem(KEY, at);
    } catch {
      /* private mode: the placement lasts for the session */
    }
    set({ at });
  },
}));
