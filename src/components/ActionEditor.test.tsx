import { describe, expect, it, vi } from "vitest";

// the editor reads the store, which reads the api: only the markup is under test
vi.mock("../lib/api", () => ({}));

import React, { type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { BatchAction, BatchStep } from "../lib/api";
import { emptyAction, newStep } from "../lib/batchLogic";
import { useBatch } from "../lib/batchStore";
import { ActionEditor, PagesField } from "./ActionEditor";

// zustand reads its initial state when rendering on the server: read the live one instead
vi.spyOn(React, "useSyncExternalStore").mockImplementation((_sub, snap) => snap());

const html = (steps: BatchStep[], selected = 0) => {
  const action: BatchAction = { ...emptyAction(), name: "A", steps };
  return renderToStaticMarkup(<ActionEditor action={action} issues={[]} selected={selected} soft={[]} />);
};
const options = (out: string) => out.slice(out.indexOf('class="aw-options"') >= 0 ? out.indexOf('class="aw-options"') : out.indexOf("aw-options"));

describe("a step's options in the editor", () => {
  it("offer a page range and the prompt switch to a page by page step", () => {
    const o = options(html([newStep("watermark")]));
    expect(o).toContain("Pages");
    expect(o).toContain('placeholder="All pages"');
    expect(o).toContain("Ask before this step, for each file");
  });

  it("offer only the prompt switch to a step that works on the whole file", () => {
    for (const k of ["compress", "protect", "sanitize", "setProperties", "exportText"] as const) {
      const o = options(html([newStep(k)]));
      expect(o, k).not.toContain("All pages");
      expect(o, k).toContain("Ask before this step, for each file");
    }
  });

  it("show the range and the prompt on the sequence rows, and keep the typed range in the field", () => {
    const s = { ...newStep("bates"), pages: "2-4", prompt: true };
    const out = html([s]);
    expect(out).toContain("(pages 2-4)");
    expect(out).toContain("Asks before it runs, for each file");
    expect(options(out)).toContain('value="2-4"');
    expect(options(out)).toMatch(/type="checkbox"[^>]*checked/);
  });

  it("send the typed range through patch, whole", () => {
    const patch = vi.fn();
    const el = PagesField({ step: newStep("ocr"), patch }) as ReactElement<{ children: ReactElement<{ onChange: (v: string) => void }>[] }>;
    el.props.children[0].props.onChange("1-3, 8-");
    expect(patch).toHaveBeenCalledWith({ pages: "1-3, 8-" });
    expect(PagesField({ step: newStep("compress"), patch })).toBeNull();
  });
});

describe("the store keeps the range and the prompt on the step", () => {
  it("patchStep sets both without touching the step's own options", () => {
    useBatch.setState({ editor: { action: { ...emptyAction(), steps: [newStep("watermark")] }, issues: [], selected: 0, dirty: false, saving: false, error: null } as never });
    useBatch.getState().patchStep(0, { pages: "1-2", prompt: true } as Partial<BatchStep>);
    expect(useBatch.getState().editor!.action.steps[0]).toMatchObject({ kind: "watermark", pages: "1-2", prompt: true });
  });
});
