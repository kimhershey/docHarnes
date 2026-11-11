// Regression tests for scripts/release-guard.mjs (PKG-4). Run: node --test "scripts/tests/*.test.mjs"
//
// The guard is run as a command with a fake `gh` first on PATH, so no network is used. The fake answers
// `gh api <path>` from a table and reports HTTP 404 for everything else, like the GitHub CLI does.
// (The fake is a script with a shebang, so these tests are skipped on Windows.)
import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const SCRIPT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "release-guard.mjs");
const skip = process.platform === "win32" ? "the fake gh needs a shebang" : false;
const A = "a".repeat(40);
const B = "b".repeat(40);
const C = "c".repeat(40);
const TAG = "v9.9.9";
const REFS = `repos/o/r/git/matching-refs/tags/${TAG}`;
const light = (sha, name = TAG) => ({ ref: `refs/tags/${name}`, object: { type: "commit", sha } });

const bin = mkdtempSync(path.join(tmpdir(), "doculens-guard-"));
after(() => rmSync(bin, { recursive: true, force: true }));

function fakeGh(body) {
  writeFileSync(path.join(bin, "gh"), `#!/usr/bin/env node\n${body}\n`);
  chmodSync(path.join(bin, "gh"), 0o755);
}
const answering = (table) =>
  fakeGh(
    `const t = ${JSON.stringify(table)}; const p = process.argv[3];\n` +
      `if (process.argv[2] === "api" && p in t) console.log(JSON.stringify(t[p]));\n` +
      `else { console.log(JSON.stringify({ message: "Not Found", status: "404" })); console.error("gh: Not Found (HTTP 404)"); process.exit(1); }`,
  );

/** The environment never leaks the real repository or Actions mode; retries pause 20 ms instead of seconds. */
const ENV = { ...process.env, GITHUB_ACTIONS: "", GITHUB_REPOSITORY: "o/r", DOCULENS_GUARD_RETRY_MS: "20" };

/** Runs the guard for TAG with the fake gh first on PATH. */
function guard(args, env = {}) {
  const r = spawnSync(process.execPath, [SCRIPT, "--tag", TAG, ...args], {
    encoding: "utf8",
    env: { ...ENV, PATH: `${bin}${path.delimiter}${process.env.PATH}`, ...env },
  });
  return { status: r.status, out: r.stdout + r.stderr };
}

test("no tag yet: allowed, the workflow creates the Release", { skip }, () => {
  answering({ [REFS]: [] });
  const r = guard(["--commit", A]);
  assert.equal(r.status, 0, r.out);
  assert.match(r.out, /tag v9\.9\.9 does not exist yet — a new Release will be created at a{40}/);
});

test("a longer tag name that only shares the prefix does not count", { skip }, () => {
  answering({ [REFS]: [light(B, `${TAG}-rc1`)] });
  const r = guard(["--commit", A]);
  assert.equal(r.status, 0, r.out);
  assert.match(r.out, /does not exist yet/);
});

test("tag on the built commit: allowed", { skip }, () => {
  answering({ [REFS]: [light(B, `${TAG}-rc1`), light(A)] });
  const r = guard(["--commit", A]);
  assert.equal(r.status, 0, r.out);
  assert.match(r.out, /already points at the built commit a{40}/);
});

test("tag on another commit: refused", { skip }, () => {
  answering({ [REFS]: [light(B)] });
  const r = guard(["--commit", A]);
  assert.equal(r.status, 1, r.out);
  assert.match(r.out, /points at b{40}, but this run builds a{40}/);
  const onActions = guard(["--commit", A], { GITHUB_ACTIONS: "true" });
  assert.equal(onActions.status, 1);
  assert.match(onActions.out, /^::error title=Release guard::tag v9\.9\.9 points at/m);
});

test("only the literal value true overrides", { skip }, () => {
  answering({ [REFS]: [light(B)] });
  const allowed = guard(["--commit", A, "--allow-overwrite", "true"], { GITHUB_ACTIONS: "true" });
  assert.equal(allowed.status, 0, allowed.out);
  assert.match(allowed.out, /^::warning title=Release guard::/m);
  assert.equal(guard(["--commit", A, "--allow-overwrite=true"]).status, 0);
  for (const form of [["--allow-overwrite"], ["--allow-overwrite", "false"], ["--allow-overwrite", ""], ["--allow-overwrite=false"], ["--allow-overwrite", "TRUE"], ["--allow-overwrite", "1"]]) {
    assert.equal(guard(["--commit", A, ...form]).status, 1, `--allow-overwrite form ${JSON.stringify(form)}`);
  }
  // a bare flag in front of another option must not swallow it or switch the override on
  assert.equal(guard(["--allow-overwrite", "--commit", A]).status, 1);
});

test("an annotated tag is peeled to its commit", { skip }, () => {
  answering({ [REFS]: [{ ref: `refs/tags/${TAG}`, object: { type: "tag", sha: C } }], [`repos/o/r/git/tags/${C}`]: { object: { type: "commit", sha: A } } });
  assert.equal(guard(["--commit", A]).status, 0);
  const other = guard(["--commit", B]);
  assert.equal(other.status, 1, other.out);
  assert.match(other.out, /points at a{40}, but this run builds b{40}/);
});

test("anything that prevents a verdict refuses the publish", { skip }, () => {
  answering({ [REFS]: [{ ref: `refs/tags/${TAG}`, object: { type: "tag", sha: C } }] }); // the tag object cannot be read
  let r = guard(["--commit", A]);
  assert.equal(r.status, 1, r.out);
  assert.match(r.out, /refusing to publish/);

  answering({ [REFS]: [{ ref: `refs/tags/${TAG}`, object: { type: "tree", sha: A } }] });
  r = guard(["--commit", A]);
  assert.equal(r.status, 1, r.out);
  assert.match(r.out, /does not resolve to a commit/);

  answering({ [REFS]: { message: "not a list" } });
  assert.equal(guard(["--commit", A]).status, 1);

  fakeGh(`console.log("<html>not json</html>");`);
  r = guard(["--commit", A]);
  assert.equal(r.status, 1, r.out);
  assert.match(r.out, /not JSON/);

  // a failing API call is tried three times, then the publish is refused
  fakeGh(`require("node:fs").appendFileSync(${JSON.stringify(path.join(bin, "calls.log"))}, "x"); console.error("gh: Service Unavailable (HTTP 503)"); process.exit(1);`);
  r = guard(["--commit", A]);
  assert.equal(r.status, 1, r.out);
  assert.match(r.out, /Service Unavailable \(HTTP 503\).*refusing to publish/);
  assert.equal(readFileSync(path.join(bin, "calls.log"), "utf8"), "xxx");
});

test("unusable arguments are refused", { skip }, () => {
  answering({ [REFS]: [] });
  assert.match(guard(["--commit", "abc123"]).out, /not a full SHA/);
  assert.equal(guard(["--commit", "abc123"]).status, 1);
  assert.equal(guard(["--commit", A], { GITHUB_REPOSITORY: "" }).status, 1);
  const badTag = spawnSync(process.execPath, [SCRIPT, "--tag", "../x", "--commit", A], { encoding: "utf8", env: ENV });
  assert.equal(badTag.status, 1);
  assert.match(badTag.stdout + badTag.stderr, /unusable tag name/);
});

test("without the GitHub CLI the publish is refused", { skip }, () => {
  const empty = mkdtempSync(path.join(tmpdir(), "doculens-guard-nogh-"));
  try {
    const r = spawnSync(process.execPath, [SCRIPT, "--tag", TAG, "--commit", A], { encoding: "utf8", env: { ...ENV, PATH: empty } });
    assert.equal(r.status, 1, r.stdout + r.stderr);
    assert.match(r.stdout + r.stderr, /could not run the GitHub CLI/);
  } finally {
    rmSync(empty, { recursive: true, force: true });
  }
});
