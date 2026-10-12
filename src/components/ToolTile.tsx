import { Icon, type IconName } from "./Icons";

/** Tile sizes in px: the glyph is the 24 px one in the big tiles (panel rows, Home cards) and the 16 px one below. */
export type TileSize = 24 | 28 | 36 | 40;

/**
 * A tool's icon the Acrobat way: a rounded square in the colour of the tool's group (allTools.css `--tile-bg` /
 * `--tile-fg`, one pair per group, light and dark) with the Filled glyph on it. Tools are told apart by colour and
 * shape, so the group comes from the catalog and a tool only picks a glyph.
 */
export function ToolTile({ icon, group, size = 36 }: { icon: IconName; group: string; size?: TileSize }) {
  return (
    <span className="tool-icon" data-group={group} data-size={size}>
      <Icon name={icon} size={size >= 36 ? 24 : 16} filled />
    </span>
  );
}
