import { useForms } from "../../lib/forms";

/**
 * What field scripts raised and the person has to see: `app.alert` as an in-app dialog (never a native one), and the
 * links and submits a script asked for. Nothing here is done for them: a link can only be copied (Inkwell never opens
 * a browser), and a submit is explained and dropped. One dialog at a time, in the order the scripts raised them.
 */
export function ScriptPrompts() {
  const prompt = useForms((s) => s.prompts[0]);
  const more = useForms((s) => s.prompts.length - 1);
  if (!prompt) return null;
  const close = () => useForms.getState().dismissPrompt(prompt.id);

  let title: string;
  let body: React.ReactNode;
  let actions: React.ReactNode;
  if (prompt.kind === "alert") {
    title = prompt.title;
    body = (
      <>
        <p className="fm-pmsg">{prompt.message}</p>
        {prompt.field && <p className="dim small">From the field {prompt.field}.</p>}
      </>
    );
    actions = (
      <button autoFocus className="btn primary" onClick={close}>
        OK
      </button>
    );
  } else if (prompt.kind === "link") {
    title = "This form wants to open a link";
    body = (
      <>
        <p className="fm-purl">{prompt.url}</p>
        <p className="dim small">Inkwell does not open links by itself. Copy it if you want to look at it.</p>
      </>
    );
    actions = (
      <>
        <button className="btn" onClick={close}>
          Cancel
        </button>
        <button
          autoFocus
          className="btn primary"
          onClick={() => {
            void navigator.clipboard.writeText(prompt.url).catch(() => undefined);
            close();
          }}
        >
          Copy link
        </button>
      </>
    );
  } else {
    title = "This form wants to send its data";
    body = (
      <>
        <p className="fm-pmsg">
          This form wants to send its data to <span className="fm-purl inline">{prompt.url}</span>. Inkwell never submits forms.
        </p>
      </>
    );
    actions = (
      <button autoFocus className="btn primary" onClick={close}>
        OK
      </button>
    );
  }

  return (
    <div className="scrim" role="presentation">
      <div
        className="dialog fm-prompt"
        role="alertdialog"
        aria-modal="true"
        aria-label={title}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            close();
          }
        }}
      >
        <h2>{title}</h2>
        {body}
        <div className="dialog-actions">
          {more > 0 && <span className="dim small fm-more">{more} more</span>}
          {more > 0 && (
            <button className="btn" onClick={() => useForms.getState().dismissAll()}>
              Dismiss all
            </button>
          )}
          {prompt.kind === "alert" && (
            <button className="btn" title="Turns the scripts of this file off until you turn them on again" onClick={() => void useForms.getState().stopScripts(prompt.doc)}>
              Stop scripts in this file
            </button>
          )}
          {actions}
        </div>
      </div>
    </div>
  );
}
