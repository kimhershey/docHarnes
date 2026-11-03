import { formatChord } from "@/lib/shortcuts";
import { isMac } from "@/lib/utils";

export function Kbd({ chord }: { chord: string }) {
  if (!chord) return null;
  return <kbd>{formatChord(chord, isMac())}</kbd>;
}
