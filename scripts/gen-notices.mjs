#!/usr/bin/env node
// Third-party notice generator (design §3.12 / appendix E). Run: npm run notices
//
//   THIRD-PARTY-NOTICES.md                         two generated sections (the hand-written text around them is
//                                                  kept; committed — CI fails when either is stale):
//                                                  - "Models" and "Inference engine" between
//                                                    <!-- generated:models-engine:start --> / …:end -->, from
//                                                    models/registry.json and engines/manifest.json
//                                                  - the crate / package inventory between
//                                                    <!-- generated:start --> / <!-- generated:end -->
//   sbom.cdx.json                                  CycloneDX 1.5 inventory (committed)
//   src-tauri/resources/licenses/THIRD-PARTY-LICENSES.txt
//                                                  concatenated license texts (generated at build time, git-ignored;
//                                                  the installer ships it as licenses/THIRD-PARTY-LICENSES.txt)
//
// Sources: `cargo metadata --locked` (every non-workspace crate in Cargo.lock) and package-lock.json
// (production npm dependencies only). License texts come from the cargo registry source directory
// (~/.cargo/registry/src/*/<name>-<version>/LICENSE*) and node_modules/<pkg>/LICENSE*.
// The output is deterministic: no timestamps, sorted entries, content-derived SBOM serial number.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const NOTICES = path.join(ROOT, "THIRD-PARTY-NOTICES.md");
const SBOM = path.join(ROOT, "sbom.cdx.json");
const LICENSES_DIR = path.join(ROOT, "src-tauri", "resources", "licenses");
const LICENSES_TXT = path.join(LICENSES_DIR, "THIRD-PARTY-LICENSES.txt");
const START = "<!-- generated:start -->";
const END = "<!-- generated:end -->";
const MODELS_ENGINE_START = "<!-- generated:models-engine:start -->";
const MODELS_ENGINE_END = "<!-- generated:models-engine:end -->";
const SHIPPED_TARGET = "x86_64-pc-windows-msvc";
const APP_PACKAGE = "doculens"; // src-tauri — the shipped binary; every other workspace member is a build/eval tool

const appVersion = JSON.parse(readFileSync(path.join(ROOT, "src-tauri", "tauri.conf.json"), "utf8")).version;
const engineManifest = JSON.parse(readFileSync(path.join(ROOT, "engines", "manifest.json"), "utf8"));
const pdfiumManifest = JSON.parse(readFileSync(path.join(ROOT, "engines", "pdfium.json"), "utf8"));
const kodicManifest = JSON.parse(readFileSync(path.join(ROOT, "engines", "kodic.json"), "utf8"));
const modelRegistry = JSON.parse(readFileSync(path.join(ROOT, "models", "registry.json"), "utf8"));

// engines/manifest.json records the pinned llama.cpp tag, the release URL, the asset names and their hashes, but no
// license field. The license is the file committed from the pinned tag (engines/licenses/llama.cpp-LICENSE.txt):
// its copyright line is quoted in the notice, its full text goes into the bundled license texts, and
// scripts/fetch-engine.mjs copies it into every unpacked pack as LICENSE-llama.cpp.txt.
const LLAMA_LICENSE_FILE = path.join(ROOT, "engines", "licenses", "llama.cpp-LICENSE.txt");
const llamaLicenseText = readFileSync(LLAMA_LICENSE_FILE, "utf8").replace(/\r\n?/g, "\n").trim();
const llamaCopyright = /^Copyright \(c\) (.+)$/m.exec(llamaLicenseText)?.[1];
if (!/^MIT License$/m.test(llamaLicenseText) || !llamaCopyright) {
  console.error(`${path.relative(ROOT, LLAMA_LICENSE_FILE)}: expected llama.cpp's MIT license with its copyright line`);
  process.exit(1);
}
const LLAMA_CPP = { license: "MIT", copyright: `© ${llamaCopyright}` };
// The official packs link the LLVM OpenMP runtime and ship it as libomp.dll with its own license file.
const LLVM_OPENMP = { license: "Apache-2.0 WITH LLVM-exception", copyright: "© LLVM Project contributors", file: "LICENSE-LLVM-OpenMP" };

/** Packs the release installers bundle: the official llama.cpp release assets (not self-built, not an alias). */
function officialEnginePacks() {
  return Object.entries(engineManifest.assets ?? {}).filter(([, a]) => !a.self_built && !a.pack);
}

/** Self-built packs (cu121 / cu129): optional, never part of the Lite installer. */
function selfBuiltEnginePacks() {
  return Object.entries(engineManifest.assets ?? {}).filter(([, a]) => a.self_built);
}

// ---------------------------------------------------------------------------
// Cargo
// ---------------------------------------------------------------------------
function cargoMetadata(extra = []) {
  const r = spawnSync("cargo", ["metadata", "--format-version", "1", "--locked", ...extra], {
    cwd: ROOT,
    encoding: "utf8",
    maxBuffer: 512 * 1024 * 1024,
    shell: process.platform === "win32",
  });
  if (r.status !== 0) throw new Error(`cargo metadata failed:\n${r.stderr}`);
  return JSON.parse(r.stdout);
}

/** `MIT/Apache-2.0` and `Apache-2.0 / MIT` (pre-SPDX cargo syntax) → `MIT OR Apache-2.0`. */
function normalizeSpdx(expr) {
  if (!expr) return null;
  return expr.replace(/\s*\/\s*/g, " OR ").replace(/\s+/g, " ").trim();
}

/** Classifies every external crate by how it reaches the shipped Windows binary. */
function cargoScopes(full, win) {
  const members = new Set(full.workspace_members);
  const byId = new Map(full.packages.map((p) => [p.id, p]));
  const nodes = new Map(win.resolve.nodes.map((n) => [n.id, n]));
  const idOf = (name) => full.workspace_members.find((id) => byId.get(id)?.name === name);
  const appId = idOf(APP_PACKAGE) ?? win.resolve.root;
  const kinds = (dep, wanted) => dep.dep_kinds.some((k) => (k.kind ?? "normal") === wanted);
  const closure = (roots, follow) => {
    const seen = new Set();
    const stack = [...roots];
    while (stack.length) {
      const id = stack.pop();
      const node = nodes.get(id);
      if (!node) continue;
      for (const dep of node.deps) {
        if (!follow(dep, id) || seen.has(dep.pkg)) continue;
        seen.add(dep.pkg);
        stack.push(dep.pkg);
      }
    }
    return seen;
  };
  const normal = (dep) => kinds(dep, "normal");
  const runtime = closure([appId], normal);
  // build-dependencies (and whatever they need) of everything that is built
  const buildRoots = [appId, ...runtime];
  const build = closure(buildRoots, (dep, from) => (buildRoots.includes(from) && kinds(dep, "build")) || normal(dep));
  const dev = closure([...members], (dep, from) => (members.has(from) && kinds(dep, "dev")) || normal(dep) || kinds(dep, "build"));
  const tool = closure([...members].filter((id) => id !== appId), (dep) => normal(dep) || kinds(dep, "build"));
  const scope = new Map();
  for (const p of full.packages) {
    if (members.has(p.id)) continue;
    const procMacro = p.targets.some((t) => t.kind.includes("proc-macro"));
    let s;
    if (!nodes.has(p.id)) s = "other target";
    else if (runtime.has(p.id)) s = procMacro ? "build (proc-macro)" : "runtime";
    else if (build.has(p.id)) s = "build";
    else if (dev.has(p.id)) s = "dev";
    else if (tool.has(p.id)) s = "tool";
    else s = "unused";
    scope.set(p.id, s);
  }
  // Workspace members that the shipped app does not depend on (dictionary builder, eval CLI …).
  const tools = [...members].filter((id) => id !== appId && !runtime.has(id)).map((id) => byId.get(id).name).sort(cmp);
  return { scope, tools };
}

function cargoRegistrySrcDirs() {
  const home = process.env.CARGO_HOME || path.join(homedir(), ".cargo");
  const src = path.join(home, "registry", "src");
  if (!existsSync(src)) return [];
  return readdirSync(src).map((d) => path.join(src, d)).filter((d) => statSync(d).isDirectory());
}

const LICENSE_FILE = /^(LICEN[CS]E|COPYING|COPYRIGHT|NOTICE|UNLICENSE|THIRD[-_]?PARTY)/i;

function licenseFiles(dir) {
  if (!dir || !existsSync(dir)) return null;
  return readdirSync(dir)
    .filter((f) => !f.startsWith(".") && LICENSE_FILE.test(f) && statSync(path.join(dir, f)).isFile())
    .sort(cmp)
    .map((f) => path.join(dir, f));
}

function collectCargo() {
  const full = cargoMetadata();
  const win = cargoMetadata(["--filter-platform", SHIPPED_TARGET]);
  const { scope: scopes, tools } = cargoScopes(full, win);
  collectCargo.tools = tools;
  const members = new Set(full.workspace_members);
  const srcDirs = cargoRegistrySrcDirs();
  const crates = full.packages
    .filter((p) => !members.has(p.id))
    .map((p) => {
      const folder = `${p.name}-${p.version}`;
      const dir = srcDirs.map((d) => path.join(d, folder)).find((d) => existsSync(d)) ?? null;
      const files = licenseFiles(dir) ?? [];
      if (dir && p.license_file && existsSync(path.join(dir, p.license_file)) && !files.includes(path.join(dir, p.license_file))) {
        files.push(path.join(dir, p.license_file));
      }
      return {
        ecosystem: "cargo",
        name: p.name,
        version: p.version,
        license: normalizeSpdx(p.license),
        licenseRaw: p.license ?? (p.license_file ? `see ${p.license_file}` : null),
        repository: p.repository ?? null,
        scope: scopes.get(p.id) ?? "unused",
        sourceDir: dir,
        licenseFiles: files,
        purl: `pkg:cargo/${p.name}@${p.version}`,
      };
    });
  return sortEntries(crates);
}

// ---------------------------------------------------------------------------
// npm
// ---------------------------------------------------------------------------
function npmRepository(repo) {
  if (!repo) return null;
  let url = typeof repo === "string" ? repo : repo.url;
  if (!url) return null;
  url = url.trim();
  if (/^github:/.test(url)) url = `https://github.com/${url.slice(7)}`;
  else if (/^gitlab:/.test(url)) url = `https://gitlab.com/${url.slice(7)}`;
  else if (/^bitbucket:/.test(url)) url = `https://bitbucket.org/${url.slice(10)}`;
  else if (/^[\w.-]+\/[\w.-]+$/.test(url)) url = `https://github.com/${url}`;
  url = url.replace(/^git\+/, "").replace(/^git:\/\//, "https://").replace(/^ssh:\/\/git@/, "https://").replace(/^git@([^:]+):/, "https://$1/");
  url = url.replace(/\.git$/, "");
  return url;
}

function npmLicense(pkg) {
  let license = pkg.license;
  if (!license && Array.isArray(pkg.licenses)) license = pkg.licenses.map((l) => l.type || l).join(" OR ");
  if (license && typeof license === "object") license = license.type;
  return license || null;
}

function collectNpm() {
  const lock = JSON.parse(readFileSync(path.join(ROOT, "package-lock.json"), "utf8"));
  const out = [];
  for (const [key, entry] of Object.entries(lock.packages)) {
    if (!key || entry.dev || entry.devOptional) continue; // production dependency tree only
    const dir = path.join(ROOT, key);
    const pkgJson = path.join(dir, "package.json");
    const pkg = existsSync(pkgJson) ? JSON.parse(readFileSync(pkgJson, "utf8")) : {};
    const name = pkg.name ?? key.slice(key.lastIndexOf("node_modules/") + "node_modules/".length);
    const version = pkg.version ?? entry.version;
    const license = npmLicense(pkg) ?? entry.license ?? null;
    out.push({
      ecosystem: "npm",
      name,
      version,
      license,
      licenseRaw: license,
      repository: npmRepository(pkg.repository) ?? null,
      scope: "runtime",
      sourceDir: existsSync(dir) ? dir : null,
      licenseFiles: licenseFiles(dir) ?? [],
      purl: `pkg:npm/${name.startsWith("@") ? "%40" + name.slice(1) : name}@${version}`,
    });
  }
  const unique = new Map(out.map((e) => [`${e.name}@${e.version}`, e]));
  return sortEntries([...unique.values()]);
}

// Code-point ordering (no ICU collation) keeps the output identical across Node builds and platforms.
const cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
function cmpVersion(a, b) {
  const pa = a.split(/[.+-]/);
  const pb = b.split(/[.+-]/);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const x = pa[i] ?? "";
    const y = pb[i] ?? "";
    const c = /^\d+$/.test(x) && /^\d+$/.test(y) ? Number(x) - Number(y) : cmp(x, y);
    if (c) return c;
  }
  return 0;
}
function sortEntries(list) {
  return list.sort((a, b) => cmp(a.name, b.name) || cmpVersion(a.version, b.version));
}

// ---------------------------------------------------------------------------
// Bundled runtime assets (from the pinned manifests) — SBOM entries. The notice text for llama.cpp is generated by
// renderModelsAndEngine(); PDFium, ko-dic, the font and the NVIDIA runtime are hand-written in the notice file.
// ---------------------------------------------------------------------------
function bundledAssets() {
  const list = [
    {
      type: "application",
      name: "llama.cpp llama-server",
      version: engineManifest.tag,
      purl: `pkg:github/ggml-org/llama.cpp@${engineManifest.tag}`,
      license: LLAMA_CPP.license,
      scope: "required",
      note: `${officialEnginePacks().map(([pack]) => pack).join(" + ")} packs (official release zips); Lite and Full installers`,
    },
    {
      type: "library",
      name: "LLVM OpenMP runtime (libomp.dll, inside the llama.cpp packs)",
      version: `bundled with llama.cpp ${engineManifest.tag}`,
      purl: `pkg:generic/llvm-openmp?bundled_with=llama.cpp%40${engineManifest.tag}`,
      license: LLVM_OPENMP.license,
      scope: "required",
      note: `libomp.dll + ${LLVM_OPENMP.file} in every official pack folder`,
    },
    {
      type: "library",
      name: "PDFium (bblanchon/pdfium-binaries)",
      version: pdfiumManifest.version,
      purl: `pkg:github/bblanchon/pdfium-binaries@${encodeURIComponent(pdfiumManifest.tag)}`,
      license: "BSD-3-Clause AND MIT",
      scope: "required",
      note: "pdfium.dll + LICENSE + licenses/ (18 notices)",
    },
    {
      type: "data",
      name: kodicManifest.name,
      version: kodicManifest.version,
      purl: `pkg:generic/${kodicManifest.name}@${kodicManifest.version}?download_url=${encodeURIComponent(kodicManifest.source.url)}&checksum=sha256%3A${kodicManifest.source.sha256}`,
      license: kodicManifest.license,
      scope: "required",
      note: `compiled into lindera ${kodicManifest.lindera} binary format by crates/doculens-kodic`,
    },
    {
      type: "file",
      name: "Pretendard Variable",
      version: "1.3.9",
      purl: "pkg:github/orioncactus/pretendard@v1.3.9",
      license: "OFL-1.1",
      scope: "required",
      note: "public/fonts/PretendardVariable.woff2 (unmodified) + LICENSE-Pretendard.txt",
    },
  ];
  for (const pack of ["cu121", "cu129"]) {
    const entry = engineManifest.assets[pack];
    if (!entry?.self_built) continue;
    list.push({
      type: "library",
      name: `NVIDIA CUDA runtime (cudart, cublas, cublasLt) ${entry.toolkit}`,
      version: entry.toolkit_version ?? entry.toolkit,
      purl: `pkg:generic/nvidia-cuda-runtime@${entry.toolkit_version ?? entry.toolkit}?pack=${pack}`,
      license: null,
      licenseName: "NVIDIA Software License Agreement (CUDA Toolkit EULA, Attachment A redistributables)",
      scope: "optional",
      note: `Full installer only — engines/${pack}/ (app-private directory, never on PATH)`,
    });
  }
  return list;
}

// ---------------------------------------------------------------------------
// Outputs
// ---------------------------------------------------------------------------
const md = (s) => String(s ?? "").replace(/\|/g, "\\|").replace(/\r?\n/g, " ");
const link = (url) => (url ? `<${url}>` : "—");

function renderNotices(crates, npm) {
  const lines = [];
  lines.push(START);
  lines.push("## Generated inventory");
  lines.push("");
  lines.push(
    "Generated by `scripts/gen-notices.mjs` (`npm run notices`) from `Cargo.lock` (`cargo metadata --locked`) and",
    "`package-lock.json`; CI fails when this section is stale. The full license texts are concatenated at build time into",
    "`src-tauri/resources/licenses/THIRD-PARTY-LICENSES.txt` and shipped as `licenses/THIRD-PARTY-LICENSES.txt` next to the",
    "application; the machine-readable inventory is `sbom.cdx.json` (CycloneDX 1.5). **Do not edit below this line by hand.**",
  );
  lines.push("");
  const counts = {};
  for (const c of crates) counts[c.scope] = (counts[c.scope] ?? 0) + 1;
  lines.push(`### Rust crates (${crates.length})`);
  lines.push("");
  lines.push(
    `Scope (target \`${SHIPPED_TARGET}\`): \`runtime\` = linked into the shipped application (${counts.runtime ?? 0});`,
    `\`build\` = build-time only — \`tauri-build\`, proc-macros and their dependencies (${(counts.build ?? 0) + (counts["build (proc-macro)"] ?? 0)});`,
    `\`dev\` = tests only (${counts.dev ?? 0}); \`tool\` = only used by the workspace tools ${(collectCargo.tools ?? []).map((n) => `\`${n}\``).join(", ")}, which are not shipped (${counts.tool ?? 0});`,
    `\`other target\` = resolved for Linux / macOS / Android only, never part of the Windows build (${counts["other target"] ?? 0}).`,
  );
  lines.push("");
  lines.push("| Crate | Version | License | Scope | Repository |");
  lines.push("|---|---|---|---|---|");
  for (const c of crates) lines.push(`| ${md(c.name)} | ${md(c.version)} | ${md(c.license ?? c.licenseRaw ?? "—")} | ${md(c.scope)} | ${link(c.repository)} |`);
  lines.push("");
  lines.push(`### npm packages (${npm.length}, production dependencies bundled into the web UI)`);
  lines.push("");
  lines.push("| Package | Version | License | Repository |");
  lines.push("|---|---|---|---|");
  for (const p of npm) lines.push(`| ${md(p.name)} | ${md(p.version)} | ${md(p.license ?? "—")} | ${link(p.repository)} |`);
  lines.push(END);
  return lines.join("\n");
}

/**
 * "Models" and "Inference engine" sections, generated from models/registry.json and engines/manifest.json so the
 * CI freshness check covers them (they were hand-written and still listed the pre-0.4.0 models). License names and
 * links are the manifests' own values; a missing one is stated as missing, never filled in. The NVIDIA CUDA runtime
 * entry stays hand-written right below the end marker.
 */
function renderModelsAndEngine() {
  const code = (s) => `\`${md(s)}\``;
  const lines = [MODELS_ENGINE_START];
  lines.push("## Models (not bundled; downloaded or imported by the user)");
  lines.push("");
  const models = modelRegistry.models ?? [];
  lines.push(
    `Generated from \`models/registry.json\` (${models.length} entries) — do not edit by hand. No model weights ship with the`,
    "application; the built-in registry offers the files below for a download the user starts. License names and links",
    "are the registry's `license` / `license_url` values.",
  );
  lines.push("");
  // One row per (license, license link, source repository), in registry order.
  const groups = new Map();
  for (const m of models) {
    const key = JSON.stringify([m.license ?? null, m.license_url ?? null, m.source_repo ?? null]);
    if (!groups.has(key)) groups.set(key, { license: m.license, licenseUrl: m.license_url, repo: m.source_repo, ids: [], files: [] });
    const g = groups.get(key);
    g.ids.push(m.id);
    for (const f of m.files ?? []) if (!g.files.includes(f.name)) g.files.push(f.name);
  }
  lines.push("| Registry entries | License | License link | GGUF repository | Files |");
  lines.push("|---|---|---|---|---|");
  const isMmproj = (name) => name.toLowerCase().startsWith("mmproj"); // same rule as registry.rs `is_mmproj`
  for (const g of groups.values()) {
    const files = [...g.files.filter((n) => !isMmproj(n)), ...g.files.filter(isMmproj)]; // weights first
    lines.push(
      `| ${g.ids.map(code).join(", ")} | ${g.license ? md(g.license) : "not recorded in `models/registry.json`"} | ${
        g.licenseUrl ? link(g.licenseUrl) : "not recorded in `models/registry.json`"
      } | ${g.repo ? code(g.repo) : "—"} | ${files.map(code).join(", ") || "—"} |`,
    );
  }
  lines.push("");
  lines.push(
    "The license and NOTICE files of each imported model are kept next to the",
    "model files inside the workspace (`.doculens/models/<id>/`).",
  );
  lines.push("");

  const tag = engineManifest.tag;
  const official = officialEnginePacks();
  const selfBuilt = selfBuiltEnginePacks();
  const packNames = official.map(([pack]) => code(pack));
  const listOf = (items) => (items.length <= 2 ? items.join(" and ") : `${items.slice(0, -1).join(", ")} and ${items.at(-1)}`);
  const bundled = packNames.length ? `the release installers bundle the ${listOf(packNames)} pack${packNames.length > 1 ? "s" : ""}` : "no pack is bundled";
  lines.push(`## Inference engine (llama.cpp \`llama-server\`; ${bundled})`);
  lines.push("");
  lines.push("Generated from `engines/manifest.json` — do not edit by hand.");
  lines.push("");
  lines.push(
    `- llama.cpp \`llama-server\` — ${LLAMA_CPP.license}, ${LLAMA_CPP.copyright}. Pinned to release ${code(tag)}${
      engineManifest.release_url ? ` (${link(engineManifest.release_url)})` : " (no release link in `engines/manifest.json`)"
    }.`,
  );
  lines.push(
    "- The MIT license text of that release ships as `LICENSE-llama.cpp.txt` inside every pack folder and in",
    "  `licenses/THIRD-PARTY-LICENSES.txt` (the official release archives contain no copy of it).",
  );
  if (official.length) {
    lines.push("- Bundled with every release installer (Lite and Full) under `engines/<pack>/`, unpacked from the official release assets:");
    for (const [pack, a] of official) {
      const asset = a.asset ? code(a.asset) : `the asset matching ${(a.match ?? []).map(code).join(" / ") || "(no pattern)"}`;
      lines.push(`  - ${code(pack)} — ${asset} (${a.sha256 ? `SHA-256 ${code(a.sha256)}` : "SHA-256 not pinned in `engines/manifest.json`"})`);
    }
    lines.push(
      `- LLVM OpenMP runtime (\`libomp.dll\`, part of those archives) — ${LLVM_OPENMP.license}, ${LLVM_OPENMP.copyright}. Its license`,
      `  file ships unchanged inside every pack folder as \`${LLVM_OPENMP.file}\`.`,
    );
  }
  if (selfBuilt.length) {
    const packs = selfBuilt.map(([pack, a]) => `${code(pack)} (CUDA ${md(a.toolkit_version ?? a.toolkit ?? "?")} toolkit)`).join(", ");
    lines.push(
      `- Optional CUDA pack${selfBuilt.length > 1 ? "s" : ""}, not part of the Lite installer: ${packs} — the same llama.cpp tag built by`,
      "  `.github/workflows/cuda-packs.yml`.",
    );
  }
  lines.push(MODELS_ENGINE_END);
  return lines.join("\n");
}

/** Replaces the text between two markers (markers included); null when the pair is missing or out of order. */
function replaceBetween(text, start, end, replacement) {
  const s = text.indexOf(start);
  const e = text.indexOf(end);
  if (s < 0 || e <= s) return null;
  return text.slice(0, s) + replacement + text.slice(e + end.length);
}

function writeNotices(generated, modelsAndEngine) {
  const current = readFileSync(NOTICES, "utf8");
  // The models / engine block sits inside the hand-written text, so there is no sensible place to append it:
  // missing markers are an error (a silently skipped block would let stale text pass the CI freshness check).
  const withModels = replaceBetween(current, MODELS_ENGINE_START, MODELS_ENGINE_END, modelsAndEngine);
  if (withModels === null) {
    throw new Error(
      `${path.relative(ROOT, NOTICES)}: the markers ${MODELS_ENGINE_START} … ${MODELS_ENGINE_END} are missing — ` +
        "put them around the Models and Inference engine sections (above the NVIDIA CUDA runtime entry).",
    );
  }
  let next = replaceBetween(withModels, START, END, generated);
  if (next === null) next = withModels.replace(/\s*$/, "\n\n") + generated + "\n";
  if (!next.endsWith("\n")) next += "\n";
  writeFileSync(NOTICES, next);
}

const SPDX_EXPR = /^[A-Za-z0-9.+\-() ]+$/;

function cdxLicenses(expr, fallbackName) {
  if (expr && SPDX_EXPR.test(expr) && !/^see /i.test(expr)) return [{ expression: expr }];
  return [{ license: { name: fallbackName ?? expr ?? "unknown" } }];
}

function uuidFromHash(hex) {
  // Content-derived serial number (RFC 9562 custom "version 8" layout) so re-runs are stable.
  const h = hex.slice(0, 32).split("");
  h[12] = "8";
  h[16] = ((parseInt(h[16], 16) & 0x3) | 0x8).toString(16);
  const s = h.join("");
  return `urn:uuid:${s.slice(0, 8)}-${s.slice(8, 12)}-${s.slice(12, 16)}-${s.slice(16, 20)}-${s.slice(20, 32)}`;
}

function renderSbom(crates, npm) {
  const components = [];
  for (const c of [...crates, ...npm]) {
    const comp = { type: "library", "bom-ref": c.purl };
    if (c.ecosystem === "npm" && c.name.startsWith("@")) {
      const [group, name] = c.name.split("/");
      comp.group = group;
      comp.name = name;
    } else comp.name = c.name;
    comp.version = c.version;
    comp.scope = c.scope === "runtime" ? "required" : "excluded";
    comp.licenses = cdxLicenses(c.license, c.licenseRaw);
    comp.purl = c.purl;
    if (c.repository) comp.externalReferences = [{ type: "vcs", url: c.repository }];
    comp.properties = [
      { name: "doculens:ecosystem", value: c.ecosystem },
      { name: "doculens:scope", value: c.scope },
    ];
    components.push(comp);
  }
  for (const a of bundledAssets()) {
    components.push({
      type: a.type,
      "bom-ref": a.purl,
      name: a.name,
      version: a.version,
      scope: a.scope,
      licenses: a.license ? [{ expression: a.license }] : [{ license: { name: a.licenseName } }],
      purl: a.purl,
      properties: [
        { name: "doculens:ecosystem", value: "bundled-asset" },
        { name: "doculens:note", value: a.note },
      ],
    });
  }
  const metadata = {
    tools: { components: [{ type: "application", name: "scripts/gen-notices.mjs", version: appVersion }] },
    component: {
      type: "application",
      "bom-ref": `pkg:generic/doculens@${appVersion}`,
      name: "DocuLens",
      version: appVersion,
      description: "Offline VLM document processing desktop client (Windows, Tauri 2)",
      licenses: [{ expression: "LicenseRef-Proprietary" }],
    },
  };
  const body = { metadata, components };
  const hash = createHash("sha256").update(JSON.stringify(body)).digest("hex");
  return {
    bomFormat: "CycloneDX",
    specVersion: "1.5",
    serialNumber: uuidFromHash(hash),
    version: 1,
    ...body,
  };
}

function readText(file) {
  return readFileSync(file, "utf8").replace(/^﻿/, "").replace(/\r\n?/g, "\n").replace(/\s+$/, "");
}

function renderLicenseTexts(crates, npm) {
  const rule = "=".repeat(78);
  const out = [];
  out.push(`DocuLens ${appVersion} — third-party license texts`);
  out.push("");
  out.push("Generated by scripts/gen-notices.mjs from Cargo.lock and package-lock.json (see THIRD-PARTY-NOTICES.md for");
  out.push("the component table, the bundled runtime assets — llama.cpp, PDFium, mecab-ko-dic, Pretendard, optional CUDA");
  out.push("runtime — and sbom.cdx.json for the machine-readable inventory). Crate texts are copied verbatim from the crate");
  out.push("sources in the cargo registry, npm texts from node_modules. Entries without a license file in their package");
  out.push("state the SPDX expression declared by the package instead.");
  // Bundled binaries that are neither a crate nor an npm package.
  out.push("");
  out.push(rule);
  out.push(`llama.cpp ${engineManifest.tag}  [bundled runtime: engines/<pack>/llama-server and its libraries]`);
  out.push(`License: ${LLAMA_CPP.license}`);
  if (engineManifest.release_url) out.push(`Release: ${engineManifest.release_url}`);
  out.push(rule);
  out.push("");
  out.push(llamaLicenseText);
  out.push("");
  out.push(
    `(The LLVM OpenMP runtime inside the packs, libomp.dll, is licensed under ${LLVM_OPENMP.license}; its license file`,
    `${LLVM_OPENMP.file} ships unchanged inside every pack folder. PDFium, mecab-ko-dic and the Pretendard font carry`,
    "their license files in their own folders — see THIRD-PARTY-NOTICES.md.)",
  );
  let missing = 0;
  let unavailable = 0;
  for (const e of [...crates, ...npm]) {
    out.push("");
    out.push(rule);
    out.push(`${e.name} ${e.version}  [${e.ecosystem}${e.scope !== "runtime" ? `, ${e.scope}` : ""}]`);
    out.push(`License: ${e.license ?? e.licenseRaw ?? "unknown"}`);
    if (e.repository) out.push(`Repository: ${e.repository}`);
    out.push(rule);
    if (!e.sourceDir) {
      unavailable++;
      out.push(`(package sources are not available locally — license expression as declared: ${e.license ?? e.licenseRaw ?? "unknown"})`);
      continue;
    }
    if (!e.licenseFiles.length) {
      missing++;
      out.push(`(no license file in the package; licensed under ${e.license ?? e.licenseRaw ?? "unknown"} as declared in its manifest${e.repository ? ` — see ${e.repository}` : ""})`);
      continue;
    }
    for (const f of e.licenseFiles) {
      out.push("");
      out.push(`--- ${path.basename(f)} ---`);
      out.push(readText(f));
    }
  }
  return { text: out.join("\n") + "\n", missing, unavailable };
}

// ---------------------------------------------------------------------------
const crates = collectCargo();
const npm = collectNpm();
writeNotices(renderNotices(crates, npm), renderModelsAndEngine());
writeFileSync(SBOM, JSON.stringify(renderSbom(crates, npm), null, 2) + "\n");
mkdirSync(LICENSES_DIR, { recursive: true });
const texts = renderLicenseTexts(crates, npm);
writeFileSync(LICENSES_TXT, texts.text);
const scopeCount = crates.reduce((m, c) => ((m[c.scope] = (m[c.scope] ?? 0) + 1), m), {});
console.log(`crates: ${crates.length} (${Object.entries(scopeCount).map(([k, v]) => `${k} ${v}`).join(", ")})`);
console.log(`npm (production): ${npm.length}`);
console.log(`wrote ${path.relative(ROOT, NOTICES)}, ${path.relative(ROOT, SBOM)}, ${path.relative(ROOT, LICENSES_TXT)} (${(texts.text.length / 1048576).toFixed(1)} MB)`);
if (texts.missing) console.log(`note: ${texts.missing} package(s) ship no license file — SPDX expression recorded instead`);
if (texts.unavailable) {
  console.warn(`WARNING: ${texts.unavailable} package(s) have no local sources (run \`cargo fetch --locked\` / \`npm ci\` first)`);
  process.exitCode = 2;
}
