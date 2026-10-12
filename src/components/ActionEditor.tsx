import { useEffect, useRef, useState } from "react";
import type { BatchAction, BatchBates, BatchIssue, BatchPattern, BatchQuery, BatchSanitize, BatchStep, MarkStyle } from "../lib/api";
import { keyMoveTarget, STEP_GROUPS, STEP_INFO, stepLabel, takesPages } from "../lib/batchLogic";
import { useBatch } from "../lib/batchStore";
import { Icon } from "./Icons";

const act = () => useBatch.getState();

/** The editor: the step library, the sequence, and the options of the step that is selected. */
export function ActionEditor({ action, issues, selected, soft }: { action: BatchAction; issues: BatchIssue[]; selected: number | null; soft: number[] }) {
  return (
    <div className="aw-editor">
      <div className="aw-meta">
        <label className="aw-field aw-grow">
          <span className="aw-label">Name</span>
          <input
            className="tl-field"
            value={action.name}
            maxLength={80}
            placeholder="For example: Prepare for the client"
            spellCheck={false}
            onChange={(e) => act().setName(e.target.value)}
          />
        </label>
        <label className="aw-field aw-grow2">
          <span className="aw-label">Description</span>
          <input
            className="tl-field"
            value={action.description}
            placeholder="What it is for (optional)"
            onChange={(e) => act().setDescription(e.target.value)}
          />
        </label>
      </div>
      <div className="aw-cols">
        <Library full={action.steps.length >= 24} />
        <Sequence action={action} issues={issues} selected={selected} soft={soft} />
        <Options action={action} issues={issues} selected={selected} soft={soft} />
      </div>
    </div>
  );
}

function Library({ full }: { full: boolean }) {
  return (
    <nav className="aw-library" aria-label="Steps to add">
      {STEP_GROUPS.map((g) => (
        <div key={g.id} className="aw-group">
          <h3 className="aw-group-title">{g.title}</h3>
          {g.kinds.map((k) => (
            <button key={k} className="aw-lib-item" disabled={full} title={STEP_INFO[k].description} onClick={() => act().addStep(k)}>
              <Icon name={STEP_INFO[k].icon} size={15} />
              <span className="aw-lib-text">
                <span>{STEP_INFO[k].label}</span>
                <span className="dim aw-lib-desc">{STEP_INFO[k].description}</span>
              </span>
              <Icon name="plus" size={13} />
            </button>
          ))}
        </div>
      ))}
    </nav>
  );
}

function Sequence({ action, issues, selected, soft }: { action: BatchAction; issues: BatchIssue[]; selected: number | null; soft: number[] }) {
  const { steps } = action;
  const [drag, setDrag] = useState<{ from: number; to: number } | null>(null);
  const list = useRef<HTMLOListElement>(null);
  // Rows are keyed by position, so focus stays on the old index when a step moves: put it on the grip of its new one.
  const refocus = useRef<number | null>(null);
  useEffect(() => {
    if (refocus.current == null) return;
    list.current?.querySelectorAll<HTMLElement>(".aw-grip")[refocus.current]?.focus();
    refocus.current = null;
  }, [steps]);

  const startDrag = (from: number, e: React.PointerEvent) => {
    if (e.button !== 0 || steps.length < 2) return;
    e.preventDefault();
    const at = (y: number) => {
      const rows = Array.from(list.current?.querySelectorAll<HTMLElement>("[data-row]") ?? []);
      let to = rows.length - 1;
      for (let i = 0; i < rows.length; i++) {
        const r = rows[i].getBoundingClientRect();
        if (y < r.top + r.height / 2) {
          to = i;
          break;
        }
      }
      return to;
    };
    setDrag({ from, to: from });
    const move = (m: PointerEvent) => setDrag({ from, to: at(m.clientY) });
    const up = (m: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      setDrag(null);
      act().moveStep(from, at(m.clientY));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  };

  return (
    <section className="aw-sequence" aria-label="Steps of the action">
      <h3 className="aw-col-title">Steps, in order</h3>
      {steps.length === 0 ? (
        <div className="aw-empty">
          <p className="aw-empty-title">No steps yet</p>
          <p className="dim">Choose a step on the left. They run in the order listed here, on each file.</p>
        </div>
      ) : (
        <ol ref={list} className={drag ? "aw-steps dragging" : "aw-steps"}>
          {steps.map((s, i) => {
            const bad = issues.filter((x) => x.step === i && !soft.includes(i));
            return (
              <li
                key={i}
                data-row
                className={[
                  "aw-step",
                  selected === i && "on",
                  bad.length > 0 && "bad",
                  drag?.from === i && "lifted",
                  drag && drag.to === i && drag.from !== i && drag.to < drag.from && "drop-before",
                  drag && drag.to === i && drag.from !== i && drag.to > drag.from && "drop-after",
                ]
                  .filter(Boolean)
                  .join(" ")}
              >
                <button
                  className="aw-grip"
                  aria-label={`Move step ${i + 1}, ${stepLabel(s)}`}
                  title="Drag to reorder, or use the up and down arrow keys"
                  onPointerDown={(e) => startDrag(i, e)}
                  onKeyDown={(e) => {
                    const to = keyMoveTarget(e.key, i, steps.length);
                    if (to == null) return;
                    e.preventDefault();
                    refocus.current = to;
                    act().moveStep(i, to);
                  }}
                >
                  <Icon name="grip" size={14} />
                </button>
                <button className="aw-step-main" aria-current={selected === i} onClick={() => act().selectStep(i)}>
                  <span className="aw-num">{i + 1}</span>
                  <span className="aw-step-text">
                    <span className="aw-step-name">{stepLabel(s)}</span>
                    {bad[0] ? (
                      <span className="aw-step-sub bad">{bad[0].message}</span>
                    ) : soft.includes(i) ? (
                      <span className="aw-step-sub dim">Password is typed when it runs</span>
                    ) : s.prompt ? (
                      <span className="aw-step-sub dim">Asks before it runs, for each file</span>
                    ) : null}
                  </span>
                </button>
                <button className="ax-icon" aria-label={`Remove step ${i + 1}, ${stepLabel(s)}`} title="Remove" onClick={() => act().removeStep(i)}>
                  <Icon name="close" size={14} />
                </button>
              </li>
            );
          })}
        </ol>
      )}
    </section>
  );
}

function Options({ action, issues, selected, soft }: { action: BatchAction; issues: BatchIssue[]; selected: number | null; soft: number[] }) {
  const step = selected != null ? action.steps[selected] : undefined;
  if (!step || selected == null) {
    return (
      <section className="aw-options" aria-label="Step options">
        <h3 className="aw-col-title">Options</h3>
        <p className="dim aw-pad">Select a step to change its options.</p>
      </section>
    );
  }
  const here = issues.filter((i) => i.step === selected && !soft.includes(selected));
  return (
    <section className="aw-options" aria-label={`Options of ${stepLabel(step)}`}>
      <h3 className="aw-col-title">{step.kind === "invalid" ? "Unknown step" : STEP_INFO[step.kind].label}</h3>
      <div className="aw-opt-body">
        {here.map((i) => (
          <p key={i.message} className="aw-issue" role="alert">
            {i.message}
          </p>
        ))}
        <StepForm step={step} patch={(p) => act().patchStep(selected, p as Partial<BatchStep>)} />
        {step.kind !== "invalid" && (
          <div className="aw-common">
            <PagesField step={step} patch={(p) => act().patchStep(selected, p as Partial<BatchStep>)} />
            <Check
              label="Ask before this step, for each file"
              hint="(shows its options, for that file only)"
              checked={step.prompt === true}
              onChange={(v) => act().patchStep(selected, { prompt: v } as Partial<BatchStep>)}
            />
          </div>
        )}
      </div>
    </section>
  );
}

// ---------------------------------------------------------------- small controls

function Check({ label, checked, onChange, hint }: { label: string; checked: boolean; onChange: (v: boolean) => void; hint?: string }) {
  return (
    <label className="aw-check">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span>
        {label}
        {hint && <span className="dim aw-hint"> {hint}</span>}
      </span>
    </label>
  );
}

function Seg<T extends string>({ label, value, items, onChange }: { label: string; value: T; items: { id: T; label: string }[]; onChange: (v: T) => void }) {
  return (
    <div className="ax-seg tl-seg" role="radiogroup" aria-label={label}>
      {items.map((i) => (
        <button key={i.id} role="radio" aria-checked={value === i.id} className={value === i.id ? "on" : ""} onClick={() => onChange(i.id)}>
          {i.label}
        </button>
      ))}
    </div>
  );
}

function Text({ label, value, onChange, placeholder, wide }: { label: string; value: string; onChange: (v: string) => void; placeholder?: string; wide?: boolean }) {
  return (
    <label className={wide ? "aw-field aw-wide" : "aw-field"}>
      <span className="aw-label">{label}</span>
      <input className="tl-field" value={value} placeholder={placeholder} spellCheck={false} onChange={(e) => onChange(e.target.value)} />
    </label>
  );
}

function Num({ label, value, min, max, step = 1, onChange }: { label: string; value: number; min: number; max: number; step?: number; onChange: (v: number) => void }) {
  return (
    <label className="aw-field aw-num-field">
      <span className="aw-label">{label}</span>
      <input
        className="tl-field"
        type="number"
        value={Number.isFinite(value) ? value : ""}
        min={min}
        max={max}
        step={step}
        onChange={(e) => {
          const v = e.target.valueAsNumber;
          if (Number.isFinite(v)) onChange(Math.min(max, Math.max(min, v)));
        }}
      />
    </label>
  );
}

function Select<T extends string>({ label, value, items, onChange }: { label: string; value: T; items: { id: T; label: string }[]; onChange: (v: T) => void }) {
  return (
    <label className="aw-field">
      <span className="aw-label">{label}</span>
      <select className="tl-field" value={value} onChange={(e) => onChange(e.target.value as T)}>
        {items.map((i) => (
          <option key={i.id} value={i.id}>
            {i.label}
          </option>
        ))}
      </select>
    </label>
  );
}

const SANITIZE_LABELS: { key: keyof BatchSanitize; label: string; redactOnly?: boolean }[] = [
  { key: "metadata", label: "Document information and metadata" },
  { key: "scriptsActions", label: "Scripts, actions and links" },
  { key: "hiddenLayers", label: "Hidden layers" },
  { key: "hiddenText", label: "Hidden text", },
  { key: "embeddedFiles", label: "Attached files" },
  { key: "annotations", label: "Comments and markups" },
  { key: "bookmarks", label: "Bookmarks" },
  { key: "formFieldsUnderMarks", label: "Form fields under the redacted areas", redactOnly: true },
];

function SanitizeChecks({ value, onChange, redact }: { value: BatchSanitize; onChange: (v: BatchSanitize) => void; redact?: boolean }) {
  return (
    <div className="aw-checks">
      {SANITIZE_LABELS.filter((l) => redact || !l.redactOnly).map((l) => (
        <Check
          key={l.key}
          label={l.label}
          checked={value[l.key]}
          hint={l.key === "hiddenText" ? "(also removes the text layer of scans)" : undefined}
          onChange={(v) => onChange({ ...value, [l.key]: v })}
        />
      ))}
    </div>
  );
}

// ---------------------------------------------------------------- one form per step

const PATTERNS: { id: BatchPattern; label: string }[] = [
  { id: "personnummer", label: "Personnummer" },
  { id: "ssn", label: "US social security number" },
  { id: "card", label: "Card number" },
  { id: "iban", label: "IBAN" },
  { id: "email", label: "Email address" },
  { id: "phone", label: "Phone number" },
];

const SLOTS = ["Top left", "Top centre", "Top right", "Bottom left", "Bottom centre", "Bottom right"];

/** The page range of a step that works page by page: empty is every page. */
export function PagesField({ step, patch }: { step: BatchStep; patch: (p: Record<string, unknown>) => void }) {
  if (!takesPages(step)) return null;
  return (
    <>
      <Text label="Pages" value={step.pages ?? ""} placeholder="All pages" onChange={(v) => patch({ pages: v })} />
      <p className="dim aw-hint-block">For example 1-3, 5, 8- . A page a file does not have is left out, and the report says so.</p>
    </>
  );
}

/** The options of one step; `patch` changes them in the action being edited or in the answer to a prompt. */
export function StepForm({ step, patch }: { step: BatchStep; patch: (p: Record<string, unknown>) => void }) {
  switch (step.kind) {
    case "invalid":
      return (
        <p className="aw-issue" role="alert">
          {step.why} Remove this step to save the action.
        </p>
      );
    case "compress":
      return (
        <>
          <Seg
            label="Compression"
            value={step.preset === "custom" ? ("balanced" as const) : step.preset}
            items={[
              { id: "smallest", label: "Smallest" },
              { id: "balanced", label: "Balanced" },
              { id: "highQuality", label: "High quality" },
            ]}
            onChange={(v) => patch({ preset: v })}
          />
          <p className="dim">A file that cannot get smaller is copied as it is.</p>
        </>
      );
    case "ocr":
      return (
        <>
          <Text
            label="Languages (for example en, sv)"
            value={step.languages.join(", ")}
            placeholder="The engine's default"
            onChange={(v) => patch({ languages: v.split(/[,\s]+/).filter(Boolean) })}
          />
          <Seg
            label="Engine"
            value={step.backend}
            items={[
              { id: "windows", label: "Windows OCR" },
              { id: "tesseract", label: "Tesseract" },
            ]}
            onChange={(v) => patch({ backend: v })}
          />
          <Check label="Read pages that already have text" hint="and replace an earlier layer of ours" checked={step.force} onChange={(v) => patch({ force: v })} />
        </>
      );
    case "headerFooter":
      return (
        <>
          <div className="aw-six">
            {step.text.map((t, n) => (
              <Text
                key={n}
                label={SLOTS[n]}
                value={t}
                onChange={(v) => patch({ text: step.text.map((x, m) => (m === n ? v : x)) })}
              />
            ))}
          </div>
          <div className="aw-row">
            <Num label="Size (pt)" value={step.style.size} min={4} max={72} onChange={(v) => patch({ style: { ...step.style, size: v } as MarkStyle })} />
            <Num label="First page number" value={step.startNumber} min={0} max={99999} onChange={(v) => patch({ startNumber: v })} />
          </div>
          <p className="dim aw-hint-block">
            Tokens: {"<<1>>"} page number, {"<<n>>"} page count, {"<<Page 1 of n>>"}, {"<<yyyy-mm-dd>>"} date. Running it again updates the marks, it does
            not stamp twice.
          </p>
        </>
      );
    case "watermark":
      return (
        <>
          <Text label="Text" value={step.text} wide onChange={(v) => patch({ text: v })} />
          <div className="aw-row">
            <Num label="Opacity (%)" value={Math.round(step.opacity * 100)} min={5} max={100} onChange={(v) => patch({ opacity: v / 100 })} />
            <Num label="Rotation (°)" value={step.rotation} min={-180} max={180} onChange={(v) => patch({ rotation: v })} />
            <Num label="Size (pt, 0 fits the page)" value={step.style.size} min={0} max={400} onChange={(v) => patch({ style: { ...step.style, size: v } as MarkStyle })} />
          </div>
          <Check label="Put it behind the page content" checked={step.behind} onChange={(v) => patch({ behind: v })} />
          <p className="dim aw-hint-block">Text only on every page: a picture cannot be kept in a saved action.</p>
        </>
      );
    case "bates": {
      const b = step.bates;
      const set = (p: Partial<BatchBates>) => patch({ bates: { ...b, ...p } });
      return (
        <>
          <div className="aw-row">
            <Text label="Before the number" value={b.prefix} onChange={(v) => set({ prefix: v })} />
            <Text label="After the number" value={b.suffix} onChange={(v) => set({ suffix: v })} />
          </div>
          <div className="aw-row">
            <Num label="Digits" value={b.digits} min={3} max={15} onChange={(v) => set({ digits: v })} />
            <Num label="First number" value={b.start} min={0} max={999999999} onChange={(v) => set({ start: v })} />
            <Select
              label="Place"
              value={String(b.slot)}
              items={SLOTS.map((s, n) => ({ id: String(n), label: s }))}
              onChange={(v) => set({ slot: Number(v) })}
            />
          </div>
          <Check
            label="Carry the numbers on from file to file"
            hint="(off: every file starts again)"
            checked={step.continueAcrossFiles}
            onChange={(v) => patch({ continueAcrossFiles: v })}
          />
        </>
      );
    }
    case "protect":
      return (
        <>
          <Select
            label="Printing"
            value={step.printing}
            items={[
              { id: "none", label: "Not allowed" },
              { id: "low", label: "Low resolution" },
              { id: "high", label: "High resolution" },
            ]}
            onChange={(v) => patch({ printing: v })}
          />
          <Select
            label="Changes"
            value={step.changes}
            items={[
              { id: "none", label: "Not allowed" },
              { id: "pages", label: "Inserting, deleting and rotating pages" },
              { id: "forms", label: "Filling in form fields and signing" },
              { id: "comments", label: "Commenting, filling in fields and signing" },
              { id: "allButExtract", label: "Anything except extracting pages" },
            ]}
            onChange={(v) => patch({ changes: v })}
          />
          <Check label="Allow copying text and images" checked={step.copy} onChange={(v) => patch({ copy: v })} />
          <Check label="Allow screen readers" checked={step.accessibility} onChange={(v) => patch({ accessibility: v })} />
          <Seg
            label="Encryption"
            value={step.cipher}
            items={[
              { id: "aes256", label: "AES-256" },
              { id: "aes128", label: "AES-128" },
            ]}
            onChange={(v) => patch({ cipher: v })}
          />
          <p className="aw-note">
            <Icon name="lock" size={13} /> The passwords are typed each time the action runs. They are never saved in the action or in an exported file.
          </p>
        </>
      );
    case "sanitize":
      return (
        <>
          <p className="dim">Remove from every file:</p>
          <SanitizeChecks value={step} onChange={(v) => patch(v as unknown as Record<string, unknown>)} />
        </>
      );
    case "redact":
      return <RedactForm step={step} patch={patch} />;
    case "flattenForms":
      return <p className="dim">Form fields become plain page content. A file with no fields is left as it is.</p>;
    case "exportImages":
      return (
        <>
          <Seg
            label="Format"
            value={step.format}
            items={[
              { id: "png", label: "PNG" },
              { id: "jpeg", label: "JPEG" },
              { id: "tiff", label: "TIFF" },
            ]}
            onChange={(v) => patch({ format: v })}
          />
          <div className="aw-row">
            <Num label="Resolution (dpi)" value={step.dpi} min={36} max={600} onChange={(v) => patch({ dpi: v })} />
            {step.format === "jpeg" && <Num label="Quality (1–100)" value={step.quality} min={1} max={100} onChange={(v) => patch({ quality: v })} />}
          </div>
          <Check label="Grey instead of colour" checked={step.gray} onChange={(v) => patch({ gray: v })} />
          <p className="dim aw-hint-block">The pictures go in a folder next to the result, named after the file.</p>
        </>
      );
    case "exportText":
      return (
        <>
          <Check label="Keep the columns and indentation of the page" checked={step.layout} onChange={(v) => patch({ layout: v })} />
          <p className="dim aw-hint-block">One .txt next to the result for each file.</p>
        </>
      );
    case "setProperties":
      return (
        <>
          <p className="dim">Tick a property to change it. A ticked, empty one is removed; the others stay as they are.</p>
          {(
            [
              ["title", "Title"],
              ["author", "Author"],
              ["subject", "Subject"],
              ["keywords", "Keywords"],
              ["creator", "Creator"],
              ["producer", "Producer"],
            ] as const
          ).map(([k, label]) => (
            <div key={k} className="aw-prop">
              <label className="aw-check">
                <input type="checkbox" checked={step[k] !== null} aria-label={`Change ${label}`} onChange={(e) => patch({ [k]: e.target.checked ? "" : null })} />
              </label>
              <Text label={label} value={step[k] ?? ""} placeholder={step[k] === null ? "Left as it is" : "Removed"} onChange={(v) => step[k] !== null && patch({ [k]: v })} />
            </div>
          ))}
        </>
      );
  }
}

function RedactForm({ step, patch }: { step: Extract<BatchStep, { kind: "redact" }>; patch: (p: Record<string, unknown>) => void }) {
  const has = (p: BatchPattern) => step.queries.some((q) => q.type === "pattern" && q.pattern === p);
  const toggle = (p: BatchPattern, on: boolean) => {
    const without = step.queries.filter((q) => !(q.type === "pattern" && q.pattern === p));
    patch({ queries: on ? [...without, { type: "pattern", pattern: p }] : without });
  };
  // the typed searches, with their place in the list
  const custom = step.queries.map((q, n) => ({ q, n })).filter((x) => x.q.type !== "pattern");
  const setAt = (n: number, q: BatchQuery) => patch({ queries: step.queries.map((x, m) => (m === n ? q : x)) });
  return (
    <>
      <p className="dim">Find and remove for good:</p>
      <div className="aw-checks">
        {PATTERNS.map((p) => (
          <Check key={p.id} label={p.label} checked={has(p.id)} onChange={(v) => toggle(p.id, v)} />
        ))}
      </div>
      <h4 className="aw-sub">Words and expressions</h4>
      {custom.map(({ q, n }) => (
        <div key={n} className="aw-query">
          <select
            className="tl-field"
            aria-label="Kind of search"
            value={q.type}
            onChange={(e) =>
              setAt(n, e.target.value === "regex" ? { type: "regex", regex: q.type === "text" ? q.text : "", caseSensitive: false } : { type: "text", text: q.type === "regex" ? q.regex : "", caseSensitive: false, wholeWord: false })
            }
          >
            <option value="text">Text</option>
            <option value="regex">Expression</option>
          </select>
          <input
            className="tl-field aw-grow"
            aria-label="What to find"
            value={q.type === "text" ? q.text : q.type === "regex" ? q.regex : ""}
            spellCheck={false}
            onChange={(e) => setAt(n, q.type === "regex" ? { ...q, regex: e.target.value } : q.type === "text" ? { ...q, text: e.target.value } : q)}
          />
          {(q.type === "text" || q.type === "regex") && (
            <label className="aw-check" title="Match capital letters exactly">
              <input type="checkbox" checked={q.caseSensitive} onChange={(e) => setAt(n, { ...q, caseSensitive: e.target.checked })} />
              <span>Aa</span>
            </label>
          )}
          <button className="ax-icon" aria-label="Remove this search" title="Remove" onClick={() => patch({ queries: step.queries.filter((_, m) => m !== n) })}>
            <Icon name="close" size={14} />
          </button>
        </div>
      ))}
      <button className="btn ghost aw-add" onClick={() => patch({ queries: [...step.queries, { type: "text", text: "", caseSensitive: false, wholeWord: false }] })}>
        <Icon name="plus" size={14} /> Add a word or expression
      </button>
      <Text
        label="Text on each black box (optional)"
        value={step.overlayText ?? ""}
        placeholder="None"
        onChange={(v) => patch({ overlayText: v === "" ? null : v })}
      />
      <h4 className="aw-sub">Also remove</h4>
      <SanitizeChecks redact value={step.sanitize} onChange={(v) => patch({ sanitize: v })} />
      {step.pages?.trim() && (
        <p className="dim aw-hint-block">
          With a page range, comments, hidden text and links are removed on those pages only. Document information, scripts, attached files and
          bookmarks are removed from the whole file. Hidden layers cannot be limited to pages: untick it to use a range. A file with redaction marks
          already on other pages is skipped.
        </p>
      )}
    </>
  );
}
