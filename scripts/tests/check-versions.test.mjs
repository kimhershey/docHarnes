// Regression tests for scripts/check-versions.mjs (PKG-8). Run: node --test "scripts/tests/*.test.mjs"
//
// Each case copies the version-bearing files of this checkout into a temp tree, breaks one place and expects
// exit code 1 with a BAD line for exactly that place. Nothing here names a version: the current one is read from
// src-tauri/tauri.conf.json, so a version bump does not touch this file.
import assert from "node:assert/strict";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const VERSION = JSON.parse(readFileSync(path.join(REPO, "src-tauri", "tauri.conf.json"), "utf8")).version;
const OTHER = "0.0.1";
// The workspace members come from Cargo.toml, so a new crate does not need a change here.
const MEMBERS = [...(/^members\s*=\s*\[(.*)\]/m.exec(readFileSync(path.join(REPO, "Cargo.toml"), "utf8"))?.[1] ?? "").matchAll(/"([^"]+)"/g)].map((m) => m[1]);
const FILES = [
  "Cargo.toml", "Cargo.lock", "package.json", "package-lock.json", "sbom.cdx.json",
  "src-tauri/tauri.conf.json", "src/lib/mock-backend.ts", "scripts/check-versions.mjs",
  ...MEMBERS.map((member) => `${member}/Cargo.toml`),
];
const esc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const V = esc(VERSION);

/** Runs the check in a fresh copy after `mutate(dir)`; returns { status, out }. */
function run(mutate = () => {}, { args = [], branch = "" } = {}) {
  const dir = mkdtempSync(path.join(tmpdir(), "doculens-versions-"));
  try {
    for (const f of FILES) {
      mkdirSync(path.dirname(path.join(dir, f)), { recursive: true });
      cpSync(path.join(REPO, f), path.join(dir, f));
    }
    mutate(dir);
    const r = spawnSync(process.execPath, [path.join(dir, "scripts", "check-versions.mjs"), ...args], {
      encoding: "utf8",
      env: { ...process.env, GITHUB_REF_NAME: branch },
    });
    return { status: r.status, out: r.stdout + r.stderr };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

/**
 * Rewrites one file. `replacement` is a function (no `$1` templates: a digit after `$1` would change the group
 * number). The pattern must match, so a changed file layout fails the test instead of passing it.
 */
function edit(dir, file, pattern, replacement) {
  const p = path.join(dir, file);
  const before = readFileSync(p, "utf8");
  const after = before.replace(pattern, replacement);
  assert.notEqual(after, before, `${file}: the test pattern ${pattern} no longer matches`);
  writeFileSync(p, after);
}
/** Keeps the captured text in front of the version and puts another version behind it. */
const setTo = (version) => (_match, head) => `${head}${version}"`;

test("the checkout itself is consistent", () => {
  const r = run();
  assert.equal(r.status, 0, r.out);
  assert.match(r.out, new RegExp(`version ${V}: \\d+ entries agree`));
});

const BREAKS = [
  ["Cargo.toml workspace version", "Cargo.toml", new RegExp(`(\\[workspace\\.package\\]\\s*version = ")${V}"`), setTo(OTHER), /BAD\s+Cargo\.toml\s+\[workspace\.package\] version\s+0\.0\.1/],
  ["workspace member in Cargo.lock", "Cargo.lock", new RegExp(`(name = "doculens-core"\\r?\\nversion = ")${V}"`), setTo(OTHER), /BAD\s+Cargo\.lock\s+doculens-core\s+0\.0\.1/],
  ["package.json", "package.json", new RegExp(`("version":\\s*")${V}"`), setTo(OTHER), /BAD\s+package\.json\s+version\s+0\.0\.1/],
  ["package-lock.json root", "package-lock.json", new RegExp(`("version":\\s*")${V}"`), setTo(OTHER), /BAD\s+package-lock\.json\s+version\s+0\.0\.1/],
  ["mock backend app_version", "src/lib/mock-backend.ts", new RegExp(`(\\bapp_version:\\s*")${V}"`), setTo(OTHER), /BAD\s+src\/lib\/mock-backend\.ts\s+app_version\s+0\.0\.1/],
  ["mock backend app_info version", "src/lib/mock-backend.ts", new RegExp(`(name:\\s*"DocuLens",\\s*version:\\s*")${V}"`), setTo(OTHER), /BAD\s+src\/lib\/mock-backend\.ts\s+app_info version\s+0\.0\.1/],
  ["SBOM component version", "sbom.cdx.json", new RegExp(`("name":\\s*"DocuLens",\\s*"version":\\s*")${V}"`), setTo(OTHER), /BAD\s+sbom\.cdx\.json\s+metadata\.component\.version\s+0\.0\.1/],
  ["member crate with its own version", "crates/doculens-eval/Cargo.toml", /version\.workspace = true/, () => `version = "${OTHER}"`, /BAD\s+crates\/doculens-eval\/Cargo\.toml\s+\[package\] version\s+0\.0\.1/],
];
for (const [name, file, pattern, replacement, expected] of BREAKS) {
  test(`stale: ${name}`, () => {
    const r = run((dir) => edit(dir, file, pattern, replacement));
    assert.equal(r.status, 1, r.out);
    assert.match(r.out, expected);
  });
}

test("tauri.conf.json is the reference: changing only it flags every other place", () => {
  const r = run((dir) => edit(dir, "src-tauri/tauri.conf.json", new RegExp(`("version":\\s*")${V}"`), setTo("9.9.9")));
  assert.equal(r.status, 1, r.out);
  assert.match(r.out, /\d+ of \d+ entries differ from src-tauri\/tauri\.conf\.json \(9\.9\.9\)/);
});

test("version strings that can no longer be found are an error, not a pass", () => {
  const r = run((dir) => edit(dir, "src/lib/mock-backend.ts", new RegExp(`"${V}"`, "g"), () => "APP_VERSION"));
  assert.equal(r.status, 1, r.out);
  assert.match(r.out, /BAD\s+src\/lib\/mock-backend\.ts\s+app_version\s+\(not found\)/);
});

test("a missing file is an error", () => {
  const r = run((dir) => rmSync(path.join(dir, "package-lock.json")));
  assert.equal(r.status, 1, r.out);
  assert.match(r.out, /versions: ENOENT/);
});

test("a CRLF checkout (Windows runner) reads the same versions", () => {
  const crlf = (dir) => {
    for (const f of FILES) writeFileSync(path.join(dir, f), readFileSync(path.join(dir, f), "utf8").replace(/\r?\n/g, "\r\n"));
  };
  assert.equal(run(crlf).status, 0);
  const stale = run((dir) => {
    crlf(dir);
    edit(dir, "Cargo.lock", new RegExp(`(name = "doculens"\\r\\nversion = ")${V}"`), setTo(OTHER));
  });
  assert.equal(stale.status, 1, stale.out);
  assert.match(stale.out, /BAD\s+Cargo\.lock\s+doculens\s+0\.0\.1/);
});

test("on a release/<version> branch the version must be the branch's", () => {
  const same = run(undefined, { branch: `release/${VERSION}` });
  assert.equal(same.status, 0, same.out);
  assert.match(same.out, new RegExp(`match branch release/${V}`));
  const other = run(undefined, { branch: "release/9.9.9" });
  assert.equal(other.status, 1, other.out);
  assert.match(other.out, /BAD\s+branch release\/9\.9\.9\s+9\.9\.9/);
});

test("other branch names are not checked, and --branch overrides GITHUB_REF_NAME", () => {
  for (const branch of ["main", "22/merge", "phase/p6-p7", "release/next"]) assert.equal(run(undefined, { branch }).status, 0, branch);
  const r = run(undefined, { args: ["--branch", `release/${VERSION}`], branch: "release/9.9.9" });
  assert.equal(r.status, 0, r.out);
});
