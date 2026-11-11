// Layout of the 문서 작성 screen by the width it gets, in rem — so a higher UI 배율 counts like a narrower
// window (layout sizes are rem or measured, never fixed px).

/**
 * `narrow`: the form column next to one pane — the preview or the editor, switched with two buttons (UI-5: the
 * old two-row layout left the editor two fifths of the height). `wide` (from 64 rem): form · preview · editor
 * side by side. `xwide` (from 80 rem): the same with roomier side columns.
 */
export type GenerateLayout = "narrow" | "wide" | "xwide";

export const WIDE_FROM_REM = 64;
export const XWIDE_FROM_REM = 80;

export function generateLayout(widthPx: number, rootPx: number): GenerateLayout {
  const rem = rootPx > 0 ? widthPx / rootPx : 0;
  if (rem >= XWIDE_FROM_REM) return "xwide";
  return rem >= WIDE_FROM_REM ? "wide" : "narrow";
}

/** Grid tracks per layout (static class strings: Tailwind reads them from the source). */
export const GENERATE_GRID: Record<GenerateLayout, string> = {
  narrow: "grid-cols-[16rem_minmax(0,1fr)] grid-rows-[auto_minmax(0,1fr)]",
  wide: "grid-cols-[18rem_minmax(0,1fr)_24rem] grid-rows-1",
  xwide: "grid-cols-[20rem_minmax(0,1fr)_26rem] grid-rows-1",
};
