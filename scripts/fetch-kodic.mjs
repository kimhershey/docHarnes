#!/usr/bin/env node
// Builds the Korean morphological dictionary the app loads at runtime
// (mecab-ko-dic sources → lindera binary format via crates/doculens-kodic):
//
//   node scripts/fetch-kodic.mjs                  # <app-data>/tokenizers/ko-dic/   (development)
//   node scripts/fetch-kodic.mjs --bundle         # src-tauri/resources/ko-dic/     (packaged with the installer)
//   node scripts/fetch-kodic.mjs --dest DIR       # anywhere (point DOCULENS_KODIC at it)
//   --source FILE    use a local copy of the pinned tarball (offline builds)
//   --builder PATH   prebuilt doculens-kodic binary (default: cargo run --release -p doculens-kodic)
//   --force          rebuild even if DIR already holds this dictionary version
//
// The tarball is sha256-pinned in engines/kodic.json; the dictionary's Apache-2.0
// COPYING and a NOTICE are copied next to the built files (design §3.2).
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, copyFileSync, renameSync, rmSync, writeFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { downloadBuffer } from "./lib/fetch-retry.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const manifest = JSON.parse(readFileSync(path.join(ROOT, "engines", "kodic.json"), "utf8"));

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] && !args[i + 1].startsWith("--") ? args[i + 1] : fallback;
};
const flag = (name) => args.includes(`--${name}`);

const appData =
  process.platform === "win32"
    ? path.join(process.env.LOCALAPPDATA ?? path.join(process.env.USERPROFILE ?? ".", "AppData", "Local"), "DocuLens")
    : path.join(process.env.XDG_DATA_HOME ?? path.join(process.env.HOME ?? ".", ".local", "share"), "DocuLens");
const dest = opt(
  "dest",
  flag("bundle") ? path.join(ROOT, "src-tauri", "resources", "ko-dic") : path.join(appData, "tokenizers", "ko-dic"),
);

const versionFile = path.join(dest, "VERSION.json");
if (!flag("force") && existsSync(versionFile) && existsSync(path.join(dest, "dict.words"))) {
  const v = JSON.parse(readFileSync(versionFile, "utf8"));
  if (v.dictionary === `${manifest.name}-${manifest.version}`) {
    console.log(`ko-dic ${manifest.version} already built in ${dest} (use --force to rebuild)`);
    process.exit(0);
  }
}

const tmp = path.join(tmpdir(), `doculens-kodic-${process.pid}`);
rmSync(tmp, { recursive: true, force: true });
mkdirSync(tmp, { recursive: true });

let archive = opt("source", null);
if (archive) {
  archive = path.resolve(archive);
  console.log(`using local ${archive}`);
} else {
  const url = manifest.source.url;
  console.log(`downloading ${url}`);
  const buf = await downloadBuffer(url, { headers: { "User-Agent": "DocuLens-fetch-kodic" }, redirect: "follow" });
  archive = path.join(tmp, manifest.source.file);
  writeFileSync(archive, buf);
}
const sha = createHash("sha256").update(readFileSync(archive)).digest("hex");
console.log(`  sha256 ${sha}`);
if (manifest.source.sha256 !== sha) {
  console.error(`  MISMATCH: engines/kodic.json pins ${manifest.source.sha256}`);
  process.exit(1);
}

const extracted = path.join(tmp, "src");
mkdirSync(extracted, { recursive: true });
const tar = spawnSync("tar", ["-xzf", archive, "-C", extracted], { stdio: "inherit" });
if (tar.status !== 0) throw new Error("tar failed");
const srcDir = path.join(extracted, manifest.source.dir);
if (!existsSync(path.join(srcDir, "matrix.def"))) {
  throw new Error(`${srcDir} does not look like a mecab-ko-dic source directory`);
}

// Build next to the destination and swap at the end: a failed build keeps the
// previous dictionary, and a running app (mmap on Windows) is never half-replaced.
const staging = `${dest}.tmp`;
rmSync(staging, { recursive: true, force: true });
mkdirSync(staging, { recursive: true });
const builder = opt("builder", null);
const cmd = builder
  ? spawnSync(builder, ["build", srcDir, staging], { stdio: "inherit" })
  : spawnSync("cargo", ["run", "--release", "-q", "-p", "doculens-kodic", "--", "build", srcDir, staging], {
      stdio: "inherit",
      cwd: ROOT,
      shell: process.platform === "win32",
    });
if (cmd.status !== 0) {
  rmSync(staging, { recursive: true, force: true });
  throw new Error("doculens-kodic build failed");
}

for (const f of ["COPYING", "README", "NEWS"]) {
  const p = path.join(srcDir, f);
  if (existsSync(p)) copyFileSync(p, path.join(staging, f === "COPYING" ? "LICENSE-mecab-ko-dic.txt" : `${f}-mecab-ko-dic.txt`));
}
writeFileSync(
  path.join(staging, "NOTICE.txt"),
  [
    `This directory contains a binary build of ${manifest.name} ${manifest.version}`,
    `(https://bitbucket.org/eunjeon/mecab-ko-dic), licensed under the Apache License 2.0`,
    `(see LICENSE-mecab-ko-dic.txt), compiled with lindera ${manifest.lindera} (MIT) by DocuLens.`,
    "",
  ].join("\n"),
);
const missing = manifest.output_files.filter((f) => !existsSync(path.join(staging, f)));
if (missing.length) {
  rmSync(staging, { recursive: true, force: true });
  throw new Error(`build incomplete, missing: ${missing.join(", ")}`);
}
const previous = `${dest}.old`;
rmSync(previous, { recursive: true, force: true });
if (existsSync(dest)) renameSync(dest, previous);
try {
  renameSync(staging, dest);
} catch (e) {
  if (existsSync(previous)) renameSync(previous, dest);
  throw e;
}
rmSync(previous, { recursive: true, force: true });
const size = readdirSync(dest).reduce((n, f) => n + (statSafe(path.join(dest, f)) ?? 0), 0);
console.log(`ko-dic ${manifest.version} ready in ${dest} (${(size / 1048576).toFixed(0)} MB)`);
rmSync(tmp, { recursive: true, force: true });

function statSafe(p) {
  try {
    return readFileSync(p).length;
  } catch {
    return null;
  }
}
