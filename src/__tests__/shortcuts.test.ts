import { describe, expect, it } from "vitest";

import { formatChord, isComposing, matchesChord, parseChord } from "@/lib/shortcuts";

function key(init: Partial<KeyboardEvent> & { key: string }): KeyboardEvent {
  return { ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, isComposing: false, ...init } as KeyboardEvent;
}

describe("shortcuts", () => {
  it("parses chords", () => {
    expect(parseChord("Ctrl+Shift+T")).toEqual({ ctrl: true, shift: true, alt: false, meta: false, key: "t" });
    expect(parseChord("Escape").key).toBe("escape");
    expect(parseChord("Ctrl+=").key).toBe("=");
    expect(parseChord("Ctrl+Shift+=").shift).toBe(true);
  });
  it("matches events on Windows and macOS", () => {
    expect(matchesChord(key({ key: "k", ctrlKey: true }), "Ctrl+K")).toBe(true);
    expect(matchesChord(key({ key: "K", ctrlKey: true, shiftKey: true }), "Ctrl+K")).toBe(false);
    expect(matchesChord(key({ key: "k", metaKey: true }), "Ctrl+K", true)).toBe(true);
    expect(matchesChord(key({ key: "k", ctrlKey: true }), "Ctrl+K", true)).toBe(false);
    expect(matchesChord(key({ key: "+", ctrlKey: true, shiftKey: true }), "Ctrl+Shift+=")).toBe(true);
    expect(matchesChord(key({ key: "Escape" }), "Escape")).toBe(true);
  });
  it("formats chords per platform", () => {
    expect(formatChord("Ctrl+Shift+T")).toBe("Ctrl+Shift+T");
    expect(formatChord("Ctrl+Shift+T", true)).toBe("⌘⇧T");
    expect(formatChord("Ctrl+=")).toBe("Ctrl++");
    expect(formatChord("Escape")).toBe("Esc");
  });
  it("detects IME composition (keyCode 229)", () => {
    expect(isComposing({ isComposing: true })).toBe(true);
    expect(isComposing({ keyCode: 229 })).toBe(true);
    expect(isComposing({ nativeEvent: { isComposing: true } })).toBe(true);
    expect(isComposing({ isComposing: false, keyCode: 13 })).toBe(false);
  });
});
