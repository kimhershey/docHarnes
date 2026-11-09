#!/usr/bin/env node
// Downloads the official llama.cpp release packs (cpu / vulkan) and verifies
// them against engines/manifest.json.
//
//   node scripts/fetch-engine.mjs --tag b12066 --packs cpu,vulkan [--dest DIR] [--pin] [--linux]
//   node scripts/fetch-engine.mjs --bundle --packs cpu,vulkan     # src-tauri/resources/engines/<pack>/ (installer)
//   node scripts/fetch-engine.mjs --bundle --dry-run              # print the target folders only (no network)
//
// Layouts (design §3.12, `doculens_core::engine::locate`):
//   app data   <engines>/<tag>/<pack>/llama-server(.exe)   default and --dest (versioned, replaceable)
//   bundled    src-tauri/resources/engines/<pack>/…         --bundle (the Tauri resource dir has no <tag> level)
//
// Only runs on a machine with GitHub access (the app itself never downloads
// engines). Uses `tar` to unpack (bsdtar on Windows 10+ handles zip files).
// Set GH_TOKEN / GITHUB_TOKEN to authenticate the GitHub API call (CI rate limits).
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { downloadGithubAsset, fetchWithRetry, githubHeaders as sharedHeaders } from "./lib/fetch-retry.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const manifestPath = path.join(ROOT, "engines", "manifest.json");

/** Default app-data engines directory (`<AppData>/DocuLens/engines`). */
export function appDataEnginesDir(env = process.env, platform = process.platform) {
  return platform === "win32"
    ? path.join(env.LOCALAPPDATA ?? path.join(env.USERPROFILE ?? ".", "AppData", "Local"), "DocuLens", "engines")
    : path.join(env.XDG_DATA_HOME ?? path.join(env.HOME ?? ".", ".local", "share"), "DocuLens", "engines");
}

/**
 * Resolves where a pack is unpacked. `bundle` selects the flat resource layout
 * (`<root>/<pack>/`), everything else keeps the versioned app-data layout
 * (`<root>/<tag>/<pack>/`). One backend family per folder: callers wipe the
 * returned directory before unpacking.
 */
export function packTarget({ root, tag, pack, bundle }) {
  return bundle ? path.join(root, pack) : path.join(root, tag, pack);
}

function parseArgs(argv) {
  const opt = (name, fallback) => {
    const i = argv.indexOf(`--${name}`);
    return i >= 0 && argv[i + 1] && !argv[i + 1].startsWith("--") ? argv[i + 1] : fallback;
  };
  const flag = (name) => argv.includes(`--${name}`);
  const bundle = flag("bundle");
  const linux = flag("linux") || (!bundle && process.platform === "linux");
  return {
    tag: opt("tag", null),
    linux,
    bundle,
    packs: opt("packs", linux ? "linux" : "cpu,vulkan").split(",").map((s) => s.trim()).filter(Boolean),
    dest: opt("dest", bundle ? path.join(ROOT, "src-tauri", "resources", "engines") : appDataEnginesDir()),
    pin: flag("pin"),
    dryRun: flag("dry-run"),
  };
}

function githubHeaders(extra = {}) {
  return sharedHeaders(extra, "DocuLens-fetch-engine");
}

async function githubJson(url) {
  const res = await fetchWithRetry(url, { headers: githubHeaders({ Accept: "application/vnd.github+json" }) });
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  return res.json();
}

async function download(asset, file) {
  // REST asset endpoint first (token only for api.github.com, the CDN redirect is pre-signed),
  // then the public release URL.
  const buf = await downloadGithubAsset({ apiUrl: asset.url, browserUrl: asset.browser_download_url, userAgent: "DocuLens-fetch-engine" });
  writeFileSync(file, buf);
  return createHash("sha256").update(buf).digest("hex");
}

/** Windows: prefer the built-in bsdtar (handles zip); GNU tar from Git Bash on PATH does not. */
function tarBinary() {
  if (process.platform === "win32") {
    const sys = path.join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe");
    if (existsSync(sys)) return sys;
  }
  return "tar";
}

function unpack(archive, target) {
  mkdirSync(target, { recursive: true });
  const r = spawnSync(tarBinary(), ["-xf", archive, "-C", target], { stdio: "inherit" });
  if (r.status !== 0) throw new Error(`tar failed for ${archive}`);
  // Flatten a single top-level directory (release zips usually wrap files in one).
  const entries = readdirSync(target);
  if (entries.length === 1 && statSync(path.join(target, entries[0])).isDirectory()) {
    const inner = path.join(target, entries[0]);
    for (const name of readdirSync(inner)) renameSync(path.join(inner, name), path.join(target, name));
    rmSync(inner, { recursive: true, force: true });
  }
}

async function main() {
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  const args = parseArgs(process.argv.slice(2));
  const tag = args.tag ?? manifest.tag;
  const { packs, dest, bundle, pin } = args;

  if (args.dryRun) {
    console.log(`layout: ${bundle ? "bundled (src-tauri resources, flat)" : "app data (<tag>/<pack>)"}; tag ${tag}`);
    for (const pack of packs) {
      const entry = manifest.assets[pack];
      if (!entry || entry.self_built) {
        console.log(`  ${pack}: self-built pack, not fetched (see engines/README.md)`);
        continue;
      }
      console.log(`  ${pack} → ${packTarget({ root: dest, tag, pack: entry.pack ?? pack, bundle })}`);
    }
    return;
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
    const excluded = ["cudart", "arm64", ...(entry.exclude ?? [])];
    // A pack directory must hold exactly one backend family: ggml loads every
    // ggml-*.dll/.so it finds next to llama-server.
    const asset = release.assets.find((a) => entry.match.some((m) => a.name.includes(m)) && !excluded.some((x) => a.name.toLowerCase().includes(x)));
    if (!asset) {
      console.error(`no asset for ${pack} (patterns ${entry.match.join(", ")}); available: ${release.assets.map((a) => a.name).join(", ")}`);
      process.exitCode = 1;
      continue;
    }
    const tmp = path.join(tmpdir(), asset.name);
    console.log(`downloading ${asset.name} (${(asset.size / 1048576).toFixed(1)} MiB)…`);
    const sha = await download(asset, tmp);
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
    const target = packTarget({ root: dest, tag, pack: entry.pack ?? pack, bundle });
    // Wipe first: never mix two release archives (CPU + Vulkan …) in one pack folder.
    if (existsSync(target)) rmSync(target, { recursive: true, force: true });
    unpack(tmp, target);
    rmSync(tmp, { force: true });
    writeFileSync(
      path.join(target, "pack.json"),
      JSON.stringify({ tag, pack: entry.pack ?? pack, asset: asset.name, sha256: sha, bundled: bundle, fetched_at: new Date().toISOString() }, null, 2),
    );
    const bin = readdirSync(target).find((n) => n === "llama-server" || n === "llama-server.exe");
    console.log(`  installed → ${target}${bin ? ` (${bin})` : " (WARNING: llama-server not found in the archive)"}`);
  }
  if (changed) {
    writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + "\n");
    console.log(`manifest pinned: ${manifestPath}`);
  }
}

// Run only when executed directly (the layout helpers are importable for tests).
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main();
}
