import { memo, useEffect } from "react";
import { type Rot, type Size, pageSpaceTransform } from "../lib/layout";
import { followLink, linkHint, useNav } from "../lib/nav";
import { useTextEdit } from "../lib/textEdit";

interface Props {
  docId: number;
  page: number;
  /** Document generation: the page's links are listed again when it changes. */
  rev: number;
  rot: Rot;
  /** Unrotated page size in points: the layer lives in this space. */
  rawSize: Size;
  zoom: number;
  /** Near the viewport: only these pages list their links. */
  visible: boolean;
}

/**
 * The links of a page, followable in Read mode as in Acrobat: a page link jumps straight there; a web,
 * mail or file link asks first, showing the whole address (nothing in a document opens on its own).
 * Hovering shows where it goes. Edit mode has its own links layer, so this one steps aside.
 */
export const ReadLinks = memo(function ReadLinks(p: Props) {
  const editing = useTextEdit((s) => s.open);
  return editing ? null : <Layer {...p} />;
});

function Layer({ docId, page, rev, rot, rawSize, zoom, visible }: Props) {
  const listed = useNav((s) => s.docs[docId]?.links[page]);

  useEffect(() => {
    if (visible) void useNav.getState().loadLinks(docId, page);
  }, [docId, page, rev, visible]);

  if (!listed || listed.links.length === 0) return null;
  return (
    <div
      className="te-space rl-space"
      style={{ width: rawSize[0], height: rawSize[1], transform: pageSpaceTransform(rawSize, rot, zoom), "--z": zoom } as React.CSSProperties}
    >
      {listed.links.map((l) => {
        const hint = linkHint(l.target);
        const live = l.target.kind === "page" || l.target.kind === "uri" || l.target.kind === "file";
        return (
          <button
            key={l.id}
            type="button"
            className={live ? "rl-link" : "rl-link idle"}
            style={{ left: l.rect[0], top: l.rect[1], width: l.rect[2] - l.rect[0], height: l.rect[3] - l.rect[1] }}
            title={hint}
            aria-label={hint}
            tabIndex={live ? 0 : -1}
            onClick={(e) => {
              e.stopPropagation();
              if (live) void followLink(docId, l.target);
            }}
          />
        );
      })}
    </div>
  );
}
