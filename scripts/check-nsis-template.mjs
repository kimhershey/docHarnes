#!/usr/bin/env node
// Verifies that src-tauri/nsis/installer.nsi (the DocuLens copy of Tauri's NSIS template, used for
// per-file instead of /SOLID compression — see the comments in that file) is identical to the
// template embedded in the installed @tauri-apps/cli, apart from the marked DocuLens blocks.
// Fails when the CLI is upgraded without re-syncing the copy. Usage: node scripts/check-nsis-template.mjs
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);
const templatePath = path.join(root, "src-tauri", "nsis", "installer.nsi");
const configPath = path.join(root, "src-tauri", "tauri.conf.json");

function fail(msg) {
  console.error(`check-nsis-template: ${msg}`);
  process.exit(1);
}

// 1. tauri.conf.json must reference the copy.
const conf = JSON.parse(fs.readFileSync(configPath, "utf8"));
const configured = conf?.bundle?.windows?.nsis?.template;
if (configured !== "nsis/installer.nsi") {
  fail(`tauri.conf.json bundle.windows.nsis.template is ${JSON.stringify(configured)}, expected "nsis/installer.nsi"`);
}

// 2. Reconstruct the upstream template from the copy: drop the DocuLens header, restore the /SOLID line.
const copy = fs.readFileSync(templatePath, "utf8");
if (copy.includes("\r")) fail("installer.nsi must use LF line endings");
const headerEnd = "; DocuLens-HEADER-END\n";
const headerAt = copy.indexOf(headerEnd);
if (headerAt < 0) fail("installer.nsi lacks the '; DocuLens-HEADER-END' marker");
let upstream = copy.slice(headerAt + headerEnd.length);
const block = /  ; DocuLens-BEGIN[^\n]*\n[\s\S]*?  ; DocuLens-END\n/g;
const blocks = upstream.match(block) ?? [];
if (blocks.length !== 1) fail(`expected exactly one DocuLens-BEGIN/END block, found ${blocks.length}`);
if (!blocks[0].includes('\n  SetCompressor "{{compression}}"\n')) fail("the DocuLens block does not set SetCompressor without /SOLID");
upstream = upstream.replace(block, '  SetCompressor /SOLID "{{compression}}"\n');
if (upstream.includes("DocuLens")) fail("unexpected DocuLens text outside the marked blocks");

// 3. Locate the installed CLI's native binary (it embeds the template via include_str!).
const cliPkg = require.resolve("@tauri-apps/cli/package.json");
const cliVersion = JSON.parse(fs.readFileSync(cliPkg, "utf8")).version;
const scopeDir = path.dirname(path.dirname(cliPkg)); // node_modules/@tauri-apps
const binaries = fs
  .readdirSync(scopeDir)
  .filter((d) => d.startsWith("cli-"))
  .flatMap((d) => {
    const dir = path.join(scopeDir, d);
    return fs.readdirSync(dir).filter((f) => f.endsWith(".node")).map((f) => path.join(dir, f));
  });
if (binaries.length === 0) fail("no @tauri-apps/cli-<platform> native binary found under node_modules (run npm ci)");

// 4. The reconstructed template must appear verbatim, exactly once, in every installed binary.
const needle = Buffer.from(upstream, "utf8");
for (const bin of binaries) {
  const data = fs.readFileSync(bin);
  const first = data.indexOf(needle);
  const second = first < 0 ? -1 : data.indexOf(needle, first + 1);
  if (first < 0 || second >= 0) {
    fail(
      `${path.relative(root, bin)} (@tauri-apps/cli ${cliVersion}) embeds a different NSIS template ` +
        `(found ${first < 0 ? 0 : 2}+ matches). Re-sync src-tauri/nsis/installer.nsi from ` +
        `https://github.com/tauri-apps/tauri/blob/@tauri-apps/cli-v${cliVersion}/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi ` +
        `and re-apply the marked DocuLens blocks.`,
    );
  }
  console.log(`ok: ${path.relative(root, bin)} embeds the template behind src-tauri/nsis/installer.nsi (@tauri-apps/cli ${cliVersion}, ${needle.length} bytes)`);
}
