// Regression tests for scripts/check-licenses.mjs (PKG-13). Run: node --test "scripts/tests/*.test.mjs"
//
// The gate used to derive its root from `new URL(import.meta.url).pathname`: under a path with a space or Hangul
// (and on every Windows checkout) it looked at a folder that does not exist, checked 0 packages and passed.
import assert from "node:assert/strict";
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const SCRIPT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "check-licenses.mjs");

/** A project root whose path needs percent-encoding in a file: URL, with the given packages installed. */
function project(packages) {
  const base = mkdtempSync(path.join(tmpdir(), "doculens-licenses-"));
  const root = path.join(base, "sp ace 한글");
  mkdirSync(path.join(root, "scripts"), { recursive: true });
  cpSync(SCRIPT, path.join(root, "scripts", "check-licenses.mjs"));
  for (const [name, license] of Object.entries(packages ?? {})) {
    const dir = path.join(root, "node_modules", ...name.split("/"));
    mkdirSync(dir, { recursive: true });
    writeFileSync(path.join(dir, "package.json"), JSON.stringify({ name, version: "1.0.0", license }));
  }
  const run = () => {
    const r = spawnSync(process.execPath, [path.join(root, "scripts", "check-licenses.mjs")], { encoding: "utf8" });
    return { status: r.status, out: r.stdout + r.stderr };
  };
  return { run, cleanup: () => rmSync(base, { recursive: true, force: true }) };
}

test("packages are found under a path with a space and Hangul", () => {
  const p = project({ alpha: "MIT", "@scope/beta": "Apache-2.0", gamma: "(MIT OR GPL-3.0)" });
  try {
    const r = p.run();
    assert.equal(r.status, 0, r.out);
    assert.match(r.out, /checked 3 packages/);
    assert.match(r.out, /all licenses allowed/);
  } finally {
    p.cleanup();
  }
});

test("a disallowed license fails", () => {
  const p = project({ alpha: "MIT", delta: "GPL-3.0-only" });
  try {
    const r = p.run();
    assert.equal(r.status, 1, r.out);
    assert.match(r.out, /delta@1\.0\.0: GPL-3\.0-only/);
  } finally {
    p.cleanup();
  }
});

test("zero packages is an error, not a pass", () => {
  const p = project(null);
  try {
    const r = p.run();
    assert.equal(r.status, 1, r.out);
    assert.match(r.out, /checked 0 packages/);
    assert.match(r.out, /nothing was checked/);
    assert.doesNotMatch(r.out, /all licenses allowed/);
  } finally {
    p.cleanup();
  }
});
