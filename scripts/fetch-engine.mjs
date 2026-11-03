#!/usr/bin/env node
// Downloads the official llama.cpp release packs (cpu / vulkan) into the
// DocuLens engines directory and verifies them against engines/manifest.json.
//
//   node scripts/fetch-engine.mjs --tag b12066 --packs cpu,vulkan [--dest DIR] [--pin] [--linux]
//
// Only runs on a machine with GitHub access (the app itself never downloads
// engines). Uses `tar` to unpack (bsdtar on Windows 10+ handles zip files).
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import { spawnSync } from "node:child_process";

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const manifestPath = path.join(ROOT, "engines", "manifest.json");
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] && !args[i + 1].startsWith("--") ? args[i + 1] : fallback;
};
const flag = (name) => args.includes(`--${name}`);

const tag = opt("tag", manifest.tag);
const linux = flag("linux") || process.platform === "linux";
const packs = (opt("packs", linux ? "linux" : "cpu,vulkan")).split(",").map((s) => s.trim()).filter(Boolean);
const dest = opt(
  "dest",
  process.platform === "win32"
    ? path.join(process.env.LOCALAPPDATA ?? path.join(process.env.USERPROFILE ?? ".", "AppData", "Local"), "DocuLens", "engines")
    : path.join(process.env.XDG_DATA_HOME ?? path.join(process.env.HOME ?? ".", ".local", "share"), "DocuLens", "engines"),
);
const pin = flag("pin");

async function githubJson(url) {
  const res = await fetch(url, { headers: { "User-Agent": "DocuLens-fetch-engine", Accept: "application/vnd.github+json" } });
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  return res.json();
}

async function download(url, file) {
  const res = await fetch(url, { headers: { "User-Agent": "DocuLens-fetch-engine" } });
  if (!res.ok || !res.body) throw new Error(`${url}: HTTP ${res.status}`);
  const buf = Buffer.from(await res.arrayBuffer());
  writeFileSync(file, buf);
  return createHash("sha256").update(buf).digest("hex");
}

function unpack(archive, target) {
  mkdirSync(target, { recursive: true });
  const r = spawnSync("tar", ["-xf", archive, "-C", target], { stdio: "inherit" });
  if (r.status !== 0) throw new Error(`tar failed for ${archive}`);
  // Flatten a single top-level directory (release zips usually wrap files in one).
  const entries = readdirSync(target);
  if (entries.length === 1 && statSync(path.join(target, entries[0])).isDirectory()) {
    const inner = path.join(target, entries[0]);
    for (const name of readdirSync(inner)) renameSync(path.join(inner, name), path.join(target, name));
    rmSync(inner, { recursive: true, force: true });
  }
}

const release = await githubJson(`https://api.github.com/repos/ggml-org/llama.cpp/releases/tags/${tag}`);
console.log(`release ${tag}: ${release.assets.length} assets`);
let changed = false;
for (const pack of packs) {
  const entry = manifest.assets[pack];
  if (!entry || entry.self_built) {
    console.log(`skip ${pack}: self-built pack (see engines/README.md)`);
    continue;
  }
  const asset = release.assets.find((a) => entry.match.some((m) => a.name.includes(m)) && !a.name.includes("cudart") && !a.name.includes("arm64"));
  if (!asset) {
    console.error(`no asset for ${pack} (patterns ${entry.match.join(", ")}); available: ${release.assets.map((a) => a.name).join(", ")}`);
    process.exitCode = 1;
    continue;
  }
  const tmp = path.join(tmpdir(), asset.name);
  console.log(`downloading ${asset.name} (${(asset.size / 1048576).toFixed(1)} MiB)…`);
  const sha = await download(asset.browser_download_url, tmp);
  console.log(`  sha256 ${sha}`);
  if (entry.sha256 && entry.sha256 !== sha) {
    console.error(`  MISMATCH: manifest pins ${entry.sha256}`);
    rmSync(tmp, { force: true });
    process.exitCode = 1;
    continue;
  }
  if (!entry.sha256 && pin) {
    entry.sha256 = sha;
    entry.asset = asset.name;
    changed = true;
  }
  const target = path.join(dest, tag, entry.pack ?? pack);
  if (existsSync(target)) rmSync(target, { recursive: true, force: true });
  unpack(tmp, target);
  rmSync(tmp, { force: true });
  writeFileSync(path.join(target, "pack.json"), JSON.stringify({ tag, pack: entry.pack ?? pack, asset: asset.name, sha256: sha, fetched_at: new Date().toISOString() }, null, 2));
  const bin = readdirSync(target).find((n) => n === "llama-server" || n === "llama-server.exe");
  console.log(`  installed → ${target}${bin ? ` (${bin})` : " (WARNING: llama-server not found in the archive)"}`);
}
if (changed) {
  writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + "\n");
  console.log(`manifest pinned: ${manifestPath}`);
}
