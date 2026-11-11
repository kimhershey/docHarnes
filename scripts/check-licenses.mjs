#!/usr/bin/env node
// License gate for npm dependencies (design §3.2 / docs/LICENSE-POLICY.md).
// Fails when any installed package (production or dev) carries a license that
// imposes a source-disclosure obligation or is unknown. Run: npm run licenses:check
import { readFileSync, readdirSync, existsSync, statSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

// fileURLToPath, not `new URL(…).pathname`: the URL path is percent-encoded and starts with "/C:/…" on Windows,
// so a checkout on Windows or under a path with spaces / Hangul pointed at a folder that does not exist.
const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const NODE_MODULES = path.join(ROOT, "node_modules");

const ALLOWED = new Set([
  "MIT", "MIT-0", "ISC", "BSD-2-Clause", "BSD-3-Clause", "0BSD", "Apache-2.0", "Unlicense", "CC0-1.0", "BlueOak-1.0.0",
  "Python-2.0", "PSF-2.0", "Zlib", "WTFPL", "OFL-1.1", "CC-BY-4.0", "BSL-1.0", "Unicode-3.0", "Unicode-DFS-2016",
  "MPL-2.0", // allowed only unmodified (decision 10); flagged below as "review"
]);
const DENY_PATTERNS = [/GPL/i, /AGPL/i, /LGPL/i, /SSPL/i, /BUSL/i, /BSL-1\.1/i, /MS-RL/i, /CC-BY-NC/i, /EUPL/i, /OSL/i, /CPAL/i, /LicenseRef/i, /UNLICENSED/i];

function parseExpr(expr) {
  // Handles "A OR B", "(A OR B)", "A AND B". For OR we accept if any side is allowed.
  const cleaned = expr.replace(/[()]/g, " ").trim();
  if (/\bOR\b/.test(cleaned)) return { kind: "or", parts: cleaned.split(/\bOR\b/).map((s) => s.trim()) };
  if (/\bAND\b/.test(cleaned)) return { kind: "and", parts: cleaned.split(/\bAND\b/).map((s) => s.trim()) };
  return { kind: "one", parts: [cleaned] };
}

function allowed(expr) {
  if (!expr) return false;
  const { kind, parts } = parseExpr(expr);
  const ok = (p) => ALLOWED.has(p) && !DENY_PATTERNS.some((re) => re.test(p));
  return kind === "or" ? parts.some(ok) : parts.every(ok);
}

function walk(dir, out) {
  if (!existsSync(dir)) return;
  for (const name of readdirSync(dir)) {
    if (name.startsWith(".")) continue;
    const p = path.join(dir, name);
    if (name.startsWith("@")) {
      walk(p, out);
      continue;
    }
    const pkgJson = path.join(p, "package.json");
    if (existsSync(pkgJson)) {
      try {
        const pkg = JSON.parse(readFileSync(pkgJson, "utf8"));
        let license = pkg.license;
        if (!license && Array.isArray(pkg.licenses)) license = pkg.licenses.map((l) => l.type || l).join(" OR ");
        if (license && typeof license === "object") license = license.type;
        out.push({ name: pkg.name, version: pkg.version, license: license || "UNKNOWN", path: path.relative(ROOT, p) });
      } catch {
        /* ignore */
      }
      const nested = path.join(p, "node_modules");
      if (existsSync(nested) && statSync(nested).isDirectory()) walk(nested, out);
    }
  }
}

const pkgs = [];
walk(NODE_MODULES, pkgs);
const unique = new Map();
for (const p of pkgs) unique.set(`${p.name}@${p.version}`, p);

const problems = [];
const review = [];
for (const p of unique.values()) {
  if (/MPL-2\.0/.test(p.license) && !/OR/.test(p.license)) review.push(p);
  if (!allowed(p.license)) problems.push(p);
}

console.log(`checked ${unique.size} packages`);
if (unique.size === 0) {
  // An empty walk is a broken setup (no `npm ci`, wrong root), never a pass.
  console.error(`\nno packages found under ${NODE_MODULES} — run \`npm ci\` first; nothing was checked`);
  process.exit(1);
}
if (review.length) {
  console.log("\nMPL-2.0 (allowed unmodified only — do not patch these packages):");
  for (const p of review) console.log(`  ${p.name}@${p.version}`);
}
if (problems.length) {
  console.error("\nDISALLOWED or unknown licenses:");
  for (const p of problems) console.error(`  ${p.name}@${p.version}: ${p.license}  (${p.path})`);
  process.exit(1);
}
console.log("all licenses allowed");
