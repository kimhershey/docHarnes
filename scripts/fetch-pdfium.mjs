#!/usr/bin/env node
// Downloads the pinned PDFium build (bblanchon/pdfium-binaries) and places the
// shared library where the app looks for it:
//
//   node scripts/fetch-pdfium.mjs                 # <app-data>/pdfium/        (development)
//   node scripts/fetch-pdfium.mjs --bundle        # src-tauri/resources/pdfium/ (packaged with the installer)
//   node scripts/fetch-pdfium.mjs --dest DIR      # anywhere (point DOCULENS_PDFIUM at it)
//
// The archive's LICENSE and third-party notices (licenses/) are copied next to
// the library; the installer ships them (design §3.2, PDFium BSD-3 + 18 notices).
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, cpSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const manifest = JSON.parse(readFileSync(path.join(ROOT, "engines", "pdfium.json"), "utf8"));

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] && !args[i + 1].startsWith("--") ? args[i + 1] : fallback;
};
const flag = (name) => args.includes(`--${name}`);

const platform = opt("platform", process.platform === "win32" ? "win-x64" : "linux-x64");
const entry = manifest.assets[platform];
if (!entry) {
  console.error(`no pdfium asset for platform ${platform} (known: ${Object.keys(manifest.assets).join(", ")})`);
  process.exit(1);
}
const appData =
  process.platform === "win32"
    ? path.join(process.env.LOCALAPPDATA ?? path.join(process.env.USERPROFILE ?? ".", "AppData", "Local"), "DocuLens")
    : path.join(process.env.XDG_DATA_HOME ?? path.join(process.env.HOME ?? ".", ".local", "share"), "DocuLens");
const dest = opt("dest", flag("bundle") ? path.join(ROOT, "src-tauri", "resources", "pdfium") : path.join(appData, "pdfium"));

const url = `https://github.com/bblanchon/pdfium-binaries/releases/download/${encodeURIComponent(manifest.tag)}/${entry.file}`;
console.log(`downloading ${url}`);
const res = await fetch(url, { headers: { "User-Agent": "DocuLens-fetch-pdfium" } });
if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
const buf = Buffer.from(await res.arrayBuffer());
const sha = createHash("sha256").update(buf).digest("hex");
console.log(`  sha256 ${sha}`);
if (entry.sha256 && entry.sha256 !== sha) {
  console.error(`  MISMATCH: manifest pins ${entry.sha256}`);
  process.exit(1);
}
const tmp = path.join(tmpdir(), `doculens-pdfium-${process.pid}`);
rmSync(tmp, { recursive: true, force: true });
mkdirSync(tmp, { recursive: true });
const archive = path.join(tmp, entry.file);
writeFileSync(archive, buf);
const r = spawnSync("tar", ["-xzf", archive, "-C", tmp], { stdio: "inherit" });
if (r.status !== 0) throw new Error("tar failed");
mkdirSync(dest, { recursive: true });
const lib = path.join(tmp, ...entry.library.split("/"));
if (!existsSync(lib)) throw new Error(`library ${entry.library} missing in archive`);
cpSync(lib, path.join(dest, path.basename(lib)));
for (const extra of ["LICENSE", "VERSION", "licenses"]) {
  const p = path.join(tmp, extra);
  if (existsSync(p)) cpSync(p, path.join(dest, extra), { recursive: true });
}
writeFileSync(
  path.join(dest, "pdfium.json"),
  JSON.stringify({ tag: manifest.tag, version: manifest.version, platform, file: entry.file, sha256: sha, fetched_at: new Date().toISOString() }, null, 2),
);
rmSync(tmp, { recursive: true, force: true });
console.log(`installed → ${path.join(dest, path.basename(lib))}`);
