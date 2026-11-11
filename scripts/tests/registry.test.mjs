// Regression tests for the pins in models/registry.json (PKG-6). Run: node --test "scripts/tests/*.test.mjs"
//
// Before 0.8.0 every file had "sha256": null and was downloaded from the moving branch …/resolve/main/…, so an
// in-app download ended without an integrity check. These tests keep the pins well-formed; they do not contact
// Hugging Face.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const registry = JSON.parse(readFileSync(path.join(REPO, "models", "registry.json"), "utf8"));
const isMmproj = (name) => name.toLowerCase().startsWith("mmproj"); // registry.rs `ModelFile::is_mmproj`
const SHA256 = /^[0-9a-f]{64}$/; // lower case: modelstore::match_registry compares the hex strings as they are
const files = registry.models.flatMap((m) => m.files.map((f) => ({ model: m.id, ...f })));

test("every file is downloaded from a repository commit, never from a branch", () => {
  assert.ok(files.length > 0);
  for (const f of files) {
    assert.ok(f.hf, `${f.model}: ${f.name} has no hf repository`);
    const prefix = `https://huggingface.co/${f.hf}/resolve/`;
    assert.ok(typeof f.url === "string" && f.url.startsWith(prefix), `${f.model}: ${f.name} url is ${f.url}`);
    const [revision, ...rest] = f.url.slice(prefix.length).split("/");
    assert.match(revision, /^[0-9a-f]{40}$/, `${f.model}: ${f.name} is not pinned to a commit (${revision})`);
    assert.equal(rest.join("/"), f.name, `${f.model}: the url does not end in the file name`);
  }
});

test("every file has an exact size and every weights file a SHA-256", () => {
  for (const f of files) {
    assert.ok(Number.isSafeInteger(f.size) && f.size > 0 && f.size <= 8_000_000_000, `${f.model}: ${f.name} size ${f.size}`);
    if (f.sha256 !== null) assert.match(f.sha256, SHA256, `${f.model}: ${f.name}`);
    if (!isMmproj(f.name)) assert.match(f.sha256 ?? "", SHA256, `${f.model}: weights file ${f.name} has no SHA-256`);
  }
});

test("verified means that every file of the entry is pinned", () => {
  for (const m of registry.models) {
    assert.equal(m.verified, m.files.every((f) => SHA256.test(f.sha256 ?? "")), m.id);
  }
});

test("a file shared by several entries is pinned identically, and an entry uses one commit", () => {
  const seen = new Map();
  for (const f of files) {
    const key = `${f.hf}/${f.name}`;
    const pin = JSON.stringify([f.sha256, f.size, f.url]);
    if (seen.has(key)) assert.equal(pin, seen.get(key), `${key} differs between entries (${f.model})`);
    seen.set(key, pin);
  }
  for (const m of registry.models) {
    const commits = new Set(m.files.map((f) => f.url.split("/resolve/")[1].split("/")[0]));
    assert.equal(commits.size, 1, `${m.id} mixes commits`);
  }
});
