import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
});
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("../../lib/edit", () => ({ applyState: vi.fn(), setLeaveGuard: vi.fn(), queued: vi.fn() }));

import { Handles } from "./MeasureLayer";

const none = () => {};

describe("measure handles", () => {
  it("every point is a labelled, focusable handle", () => {
    const out = renderToStaticMarkup(
      <svg>
        <Handles kind="perimeter" pts={[[10, 10], [60, 10], [60, 50]]} r={5} onDown={none} onMove={none} onUp={none} onCancel={none} onLost={none} onKey={none} onBlur={none} />
      </svg>,
    );
    expect(out.match(/class="ms-handle"/g)).toHaveLength(3);
    expect(out).toContain('aria-label="Perimeter point 2 of 3. Drag it, or use the arrow keys (Shift for 10 points)."');
    expect(out.match(/tabindex="0"/g)).toHaveLength(3);
    expect(out).toContain('cx="60" cy="50" r="5"');
  });
});
