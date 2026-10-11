import { useFind } from "../../lib/findStore";
import { changedOptions } from "../../lib/searchLogic";
import { useSearch } from "../../lib/searchStore";
import { Icon } from "../Icons";
import { MenuButton, type MenuEntry } from "../Menu";
import "./search.css";

/** The two options people toggle most, as small chips in the find box: Match case (Aa) and Whole words (ab). */
export function OptionChips() {
  const caseSensitive = useFind((s) => s.opts.caseSensitive);
  const wholeWords = useFind((s) => s.opts.wholeWords);
  const set = useFind.getState().setOpt;
  return (
    <span className="sr-chips">
      <button className={caseSensitive ? "sr-chip on" : "sr-chip"} title="Match case" aria-label="Match case" aria-pressed={caseSensitive} onClick={() => set("caseSensitive", !caseSensitive)}>
        Aa
      </button>
      <button className={wholeWords ? "sr-chip on" : "sr-chip"} title="Whole words" aria-label="Whole words" aria-pressed={wholeWords} onClick={() => set("wholeWords", !wholeWords)}>
        <u>ab</u>
      </button>
    </span>
  );
}

/** The "…" menu: the rest of the options, and the way into the Search rail. */
export function OptionsMenu({ advanced = true }: { advanced?: boolean }) {
  const opts = useFind((s) => s.opts);
  const set = useFind.getState().setOpt;
  const extra = changedOptions({ ...opts, caseSensitive: false, wholeWords: false });
  const items: MenuEntry[] = [
    { label: "Ignore accents", checked: opts.ignoreAccents, onSelect: () => set("ignoreAccents", !opts.ignoreAccents) },
    "sep",
    { label: "Include comments", checked: opts.comments, onSelect: () => set("comments", !opts.comments) },
    { label: "Include bookmarks", checked: opts.bookmarks, onSelect: () => set("bookmarks", !opts.bookmarks) },
    { label: "Include form fields", checked: opts.fields, onSelect: () => set("fields", !opts.fields) },
  ];
  if (advanced) items.push("sep", { label: "Advanced search…", hint: "Ctrl+Shift+F", onSelect: () => useSearch.getState().setOpen(true) });
  return (
    <span className="sr-more">
      <MenuButton items={items} label="More find options">
        <Icon name="more" />
      </MenuButton>
      {extra > 0 && <i className="sr-badge" aria-label={`${extra} more options on`} />}
    </span>
  );
}
