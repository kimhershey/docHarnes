#!/usr/bin/env node
// Seeds the NSIS toolset that tauri-bundler would otherwise download during `tauri build`
// (CI helper for the Windows installer workflow; harmless elsewhere).
//
//   node scripts/fetch-nsis.mjs                # → %LOCALAPPDATA%\tauri\NSIS (where tauri-bundler looks)
//   node scripts/fetch-nsis.mjs --dest DIR     # anywhere (testing)
//
// tauri-bundler fetches nsis-<ver>.zip (tauri-apps/binary-releases) and nsis_tauri_utils.dll
// (tauri-apps/nsis-tauri-utils) with a single unretried request each; a GitHub release-download
// outage then fails the whole build after the Rust compile. This script downloads the same two
// files through scripts/lib/fetch-retry.mjs (REST asset endpoint first, retries, public URL fallback)
// and lays them out as the bundler expects. The bundler still verifies the plugin hash and
// re-downloads anything it does not like, so a wrong seed only costs the download it would have
// done anyway. When the installed Tauri CLI does not reference exactly these versions, nothing
// is seeded.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { downloadGithubAsset, githubReleaseAsset } from "./lib/fetch-retry.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const UA = "DocuLens-fetch-nsis";

// Constants of @tauri-apps/cli 2.12.x (tauri-bundler): URLs + SHA-1s embedded in the CLI binary.
const NSIS = {
  owner: "tauri-apps",
  repo: "binary-releases",
  tag: "nsis-3.13",
  file: "nsis-3.13.zip",
  url: "https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.13/nsis-3.13.zip",
  // SHA-1 of the official archive (2,363,925 bytes, 441 files) as downloaded on 2026-11-08.
  sha1: "db14f8af2ea346b786c6ae19343f61aaf5e09b8e",
};
const UTILS = {
  owner: "tauri-apps",
  repo: "nsis-tauri-utils",
  tag: "nsis_tauri_utils-v0.5.3",
  file: "nsis_tauri_utils.dll",
  url: "https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll",
  relPath: ["Plugins", "x86-unicode", "additional", "nsis_tauri_utils.dll"],
  // SHA-1 the bundler verifies for this plugin (embedded in the CLI binary next to the URL).
  sha1: "75197fee3c6a814fe035788d1c34ead39349b860",
};

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] && !args[i + 1].startsWith("--") ? args[i + 1] : fallback;
};
const localAppData = process.env.LOCALAPPDATA ?? path.join(process.env.USERPROFILE ?? process.env.HOME ?? ".", "AppData", "Local");
const dest = path.resolve(opt("dest", path.join(localAppData, "tauri", "NSIS")));

/** The installed CLI must reference exactly these downloads; otherwise seeding could mismatch. */
function cliMatches() {
  const pkgs = ["cli-win32-x64-msvc", "cli-linux-x64-gnu", "cli-linux-x64-musl", "cli-darwin-arm64", "cli-darwin-x64"];
  for (const pkg of pkgs) {
    const dir = path.join(ROOT, "node_modules", "@tauri-apps", pkg);
    if (!existsSync(dir)) continue;
    const bin = readdirSync(dir).find((f) => f.endsWith(".node"));
    if (!bin) continue;
    // The plugin URL (with its version) is embedded verbatim; the NSIS archive URL is assembled at
    // runtime, so its version is pinned here and the archive is verified by SHA-1 after download.
    const blob = readFileSync(path.join(dir, bin));
    return { pkg, ok: blob.includes(UTILS.url) };
  }
  return { pkg: null, ok: false };
}

const sha1 = (buf) => createHash("sha1").update(buf).digest("hex");

async function fetchAsset(spec) {
  const asset = await githubReleaseAsset({ owner: spec.owner, repo: spec.repo, tag: spec.tag, name: spec.file, userAgent: UA });
  const buf = await downloadGithubAsset({ apiUrl: asset?.url, browserUrl: spec.url, userAgent: UA });
  const hash = sha1(buf);
  console.log(`  ${spec.file}: ${(buf.length / 1024).toFixed(0)} KiB, sha1 ${hash}${hash === spec.sha1 ? "" : " (MISMATCH)"}`);
  if (hash !== spec.sha1) throw new Error(`${spec.file}: sha1 ${hash}, expected ${spec.sha1}`);
  return buf;
}

function unzip(archive, into) {
  mkdirSync(into, { recursive: true });
  const attempts = process.platform === "win32"
    ? [[path.join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe"), ["-xf", archive, "-C", into]], ["tar", ["-xf", archive, "-C", into]]]
    : [["unzip", ["-q", archive, "-d", into]], ["bsdtar", ["-xf", archive, "-C", into]]];
  for (const [bin, a] of attempts) {
    const r = spawnSync(bin, a, { stdio: "inherit" });
    if (r.status === 0) return;
  }
  throw new Error(`could not extract ${archive}`);
}

async function main() {
  const required = ["makensis.exe", path.join("Bin", "makensis.exe"), path.join(...UTILS.relPath)];
  if (required.every((f) => existsSync(path.join(dest, f)))) {
    console.log(`NSIS toolset already present at ${dest}`);
    return;
  }
  const cli = cliMatches();
  if (!cli.ok) {
    console.log(`::notice::Tauri CLI (${cli.pkg ?? "not installed"}) does not reference ${NSIS.tag} + ${UTILS.tag}; leaving the download to tauri-bundler`);
    return;
  }
  console.log(`seeding NSIS for ${cli.pkg} → ${dest}`);
  const [zip, dll] = await Promise.all([fetchAsset(NSIS), fetchAsset(UTILS)]);
  const tmp = path.join(tmpdir(), `doculens-nsis-${process.pid}`);
  rmSync(tmp, { recursive: true, force: true });
  mkdirSync(tmp, { recursive: true });
  const archive = path.join(tmp, NSIS.file);
  writeFileSync(archive, zip);
  const extracted = path.join(tmp, "x");
  unzip(archive, extracted);
  // The zip wraps everything in one folder (nsis-3.13/); tauri-bundler renames it to NSIS/.
  const entries = readdirSync(extracted);
  const top = entries.length === 1 ? path.join(extracted, entries[0]) : extracted;
  if (!existsSync(path.join(top, "makensis.exe"))) throw new Error(`makensis.exe not found under ${top}`);
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(path.dirname(dest), { recursive: true });
  renameSync(top, dest);
  const dllPath = path.join(dest, ...UTILS.relPath);
  mkdirSync(path.dirname(dllPath), { recursive: true });
  writeFileSync(dllPath, dll);
  rmSync(tmp, { recursive: true, force: true });
  const missing = required.filter((f) => !existsSync(path.join(dest, f)));
  if (missing.length) throw new Error(`seeded toolset is missing ${missing.join(", ")}`);
  console.log(`NSIS toolset ready at ${dest}`);
}

await main();
