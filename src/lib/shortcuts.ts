// Key-chord parsing/matching for settings.shortcuts (e.g. "Ctrl+Shift+T").

export interface Chord {
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  meta: boolean;
  key: string; // lower-case key value as in KeyboardEvent.key
}

const KEY_ALIASES: Record<string, string> = {
  esc: "escape",
  return: "enter",
  plus: "=",
  minus: "-",
  comma: ",",
  space: " ",
  up: "arrowup",
  down: "arrowdown",
  left: "arrowleft",
  right: "arrowright",
};

export function parseChord(chord: string): Chord {
  const parts = chord.split("+").map((p) => p.trim()).filter(Boolean);
  const out: Chord = { ctrl: false, shift: false, alt: false, meta: false, key: "" };
  for (const part of parts) {
    const p = part.toLowerCase();
    if (p === "ctrl" || p === "control") out.ctrl = true;
    else if (p === "shift") out.shift = true;
    else if (p === "alt" || p === "option") out.alt = true;
    else if (p === "meta" || p === "cmd" || p === "win" || p === "super") out.meta = true;
    else out.key = KEY_ALIASES[p] ?? p;
  }
  // "Ctrl+Shift+=" is typed as "+" on US layouts: normalise.
  if (out.key === "+") out.key = "=";
  return out;
}

/** Ctrl on Windows/Linux maps to ⌘ on macOS. */
export function matchesChord(e: KeyboardEvent, chord: Chord | string, mac = false): boolean {
  const c = typeof chord === "string" ? parseChord(chord) : chord;
  const primary = mac ? e.metaKey : e.ctrlKey;
  const secondaryMeta = mac ? e.ctrlKey : e.metaKey;
  if (c.ctrl !== primary) return false;
  if (c.meta !== secondaryMeta) return false;
  if (c.shift !== e.shiftKey) return false;
  if (c.alt !== e.altKey) return false;
  let key = e.key.toLowerCase();
  if (key === "+") key = "=";
  if (c.key.length === 1 && key.length === 1) return key === c.key;
  return key === c.key;
}

export function formatChord(chord: Chord | string, mac = false): string {
  const c = typeof chord === "string" ? parseChord(chord) : chord;
  const keyLabel = (() => {
    switch (c.key) {
      case "escape":
        return "Esc";
      case "enter":
        return mac ? "↩" : "Enter";
      case "=":
        return "+";
      case " ":
        return "Space";
      case "arrowup":
        return "↑";
      case "arrowdown":
        return "↓";
      case "arrowleft":
        return "←";
      case "arrowright":
        return "→";
      default:
        return c.key.length === 1 ? c.key.toUpperCase() : c.key.charAt(0).toUpperCase() + c.key.slice(1);
    }
  })();
  if (mac) {
    return `${c.ctrl ? "⌘" : ""}${c.alt ? "⌥" : ""}${c.shift ? "⇧" : ""}${c.meta ? "⌃" : ""}${keyLabel}`;
  }
  const parts: string[] = [];
  if (c.ctrl) parts.push("Ctrl");
  if (c.meta) parts.push("Win");
  if (c.alt) parts.push("Alt");
  if (c.shift) parts.push("Shift");
  parts.push(keyLabel);
  return parts.join("+");
}

/** True while a Korean/Japanese/Chinese IME composition is in progress (design §3.7.4). */
export function isComposing(e: { isComposing?: boolean; nativeEvent?: { isComposing?: boolean }; keyCode?: number }): boolean {
  return Boolean(e.isComposing || e.nativeEvent?.isComposing || e.keyCode === 229);
}
