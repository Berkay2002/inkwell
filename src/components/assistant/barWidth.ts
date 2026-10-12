/** The command bar follows the document area: ~62 % of it, never cramped, never wall-to-wall. */
export function barWidth(stageW: number): number {
  return Math.round(Math.min(960, stageW - 48, Math.max(360, stageW * 0.62)));
}
