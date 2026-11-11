// Regression tests for scripts/list-pe-imports.mjs (PKG-10). Run: node --test "scripts/tests/*.test.mjs"
//
// The binaries are synthetic: minimal PE32+ images that carry nothing but an import table and a delay-import
// table. (The reader was also compared with `llvm-objdump -p` on all 103 binaries of the two official
// llama.cpp b12066 packs when it was written; those files are not in the repository.)
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

import { analysePack, peImports } from "../list-pe-imports.mjs";

const SCRIPT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "list-pe-imports.mjs");

/** A PE32+ image with one section (RVA 0x1000, file offset 0x200) holding the two import tables and the names. */
function pe(imports, delayed = []) {
  const SECTION_RVA = 0x1000;
  const SECTION_RAW = 0x200;
  const section = Buffer.alloc(0x800);
  const importsAt = 0; // 20-byte descriptors + terminator
  const delayAt = importsAt + (imports.length + 1) * 20; // 32-byte descriptors + terminator
  let nameAt = delayAt + (delayed.length + 1) * 32;
  const putName = (name) => {
    const at = nameAt;
    section.write(`${name}\0`, at, "latin1");
    nameAt += name.length + 1;
    return SECTION_RVA + at;
  };
  imports.forEach((name, i) => {
    section.writeUInt32LE(0x1700, importsAt + i * 20); // OriginalFirstThunk (any non-zero RVA)
    section.writeUInt32LE(putName(name), importsAt + i * 20 + 12);
  });
  delayed.forEach((name, i) => {
    section.writeUInt32LE(1, delayAt + i * 32); // attributes: the fields are RVAs
    section.writeUInt32LE(putName(name), delayAt + i * 32 + 4);
  });

  const headers = Buffer.alloc(SECTION_RAW);
  headers.write("MZ", 0, "latin1");
  headers.writeUInt32LE(0x40, 0x3c); // e_lfanew
  headers.write("PE\0\0", 0x40, "latin1");
  const coff = 0x44;
  headers.writeUInt16LE(0x8664, coff); // x64
  headers.writeUInt16LE(1, coff + 2); // one section
  headers.writeUInt16LE(240, coff + 16); // size of the optional header
  const optional = coff + 20;
  headers.writeUInt16LE(0x20b, optional); // PE32+
  headers.writeUInt32LE(16, optional + 108); // number of data directories
  const directory = (index, rva, size) => {
    headers.writeUInt32LE(rva, optional + 112 + index * 8);
    headers.writeUInt32LE(size, optional + 112 + index * 8 + 4);
  };
  if (imports.length) directory(1, SECTION_RVA + importsAt, (imports.length + 1) * 20);
  if (delayed.length) directory(13, SECTION_RVA + delayAt, (delayed.length + 1) * 32);
  const sectionHeader = optional + 240;
  headers.write(".rdata", sectionHeader, "latin1");
  headers.writeUInt32LE(section.length, sectionHeader + 8); // virtual size
  headers.writeUInt32LE(SECTION_RVA, sectionHeader + 12);
  headers.writeUInt32LE(section.length, sectionHeader + 16); // raw size
  headers.writeUInt32LE(SECTION_RAW, sectionHeader + 20);
  return Buffer.concat([headers, section]);
}

function tempPack(files) {
  const root = mkdtempSync(path.join(tmpdir(), "doculens-pe-"));
  const dir = path.join(root, "cpu");
  mkdirSync(dir);
  for (const [name, content] of Object.entries(files)) writeFileSync(path.join(dir, name), content);
  return { root, dir, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

test("import and delay-import tables are read", () => {
  const p = tempPack({ "a.dll": pe(["KERNEL32.dll", "VCRUNTIME140.dll"], ["vulkan-1.dll"]), "none.dll": pe([]) });
  try {
    assert.deepEqual(peImports(path.join(p.dir, "a.dll")), ["KERNEL32.dll", "VCRUNTIME140.dll", "vulkan-1.dll"]);
    assert.deepEqual(peImports(path.join(p.dir, "none.dll")), []);
  } finally {
    p.cleanup();
  }
});

test("a file that is not a PE image is an error for that file only", () => {
  const p = tempPack({ "text.dll": "not a PE file", "short.dll": Buffer.alloc(8) });
  try {
    assert.throws(() => peImports(path.join(p.dir, "text.dll")), /not a PE file/);
    assert.throws(() => peImports(path.join(p.dir, "short.dll")), /not a PE file/);
  } finally {
    p.cleanup();
  }
});

test("a pack is followed from the server binary and the run-time loaded ggml backends", () => {
  const p = tempPack({
    "llama-server.exe": pe(["helper.dll", "KERNEL32.dll", "VCRUNTIME140.dll", "api-ms-win-crt-runtime-l1-1-0.dll"]),
    "helper.dll": pe(["MSVCP140.dll", "libomp.dll", "WS2_32.dll"]),
    "libomp.dll": "placeholder that is not a PE image",
    "ggml-vulkan.dll": pe(["KERNEL32.dll"], ["vulkan-1.dll"]), // never imported: ggml loads it at run time
    "unrelated-tool.exe": pe(["MFC140.dll"]), // not reachable from llama-server.exe: ignored
  });
  try {
    const r = analysePack(p.dir, "llama-server.exe");
    assert.deepEqual(r.scanned, ["ggml-vulkan.dll", "helper.dll", "libomp.dll", "llama-server.exe"]);
    assert.deepEqual(r.vcRuntime.map((d) => d.name), ["MSVCP140.dll", "VCRUNTIME140.dll"]);
    assert.deepEqual(r.vcRuntime[0].importers, ["helper.dll"]);
    assert.deepEqual(r.ucrt.map((d) => d.name), ["api-ms-win-crt-runtime-l1-1-0.dll"]);
    assert.deepEqual(r.other.map((d) => d.name), ["KERNEL32.dll", "vulkan-1.dll", "WS2_32.dll"]);
    assert.equal(r.unreadable.length, 1);
    assert.match(r.unreadable[0], /^libomp\.dll: not a PE file/);
  } finally {
    p.cleanup();
  }
});

test("a runtime DLL shipped inside the pack is not reported, whatever the letter case", () => {
  const p = tempPack({
    "llama-server.exe": pe(["VCRUNTIME140.dll", "MSVCP140.dll"]),
    "vcruntime140.dll": pe(["KERNEL32.dll"]),
  });
  try {
    const r = analysePack(p.dir, "llama-server.exe");
    assert.deepEqual(r.vcRuntime.map((d) => d.name), ["MSVCP140.dll"]);
  } finally {
    p.cleanup();
  }
});

test("the command lists every pack below a folder, annotates on Actions and always exits 0", () => {
  const p = tempPack({ "llama-server.exe": pe(["KERNEL32.dll", "VCRUNTIME140_1.dll"]) });
  try {
    const run = (args, env = {}) => {
      const r = spawnSync(process.execPath, [SCRIPT, ...args], { encoding: "utf8", env: { ...process.env, GITHUB_ACTIONS: "", ...env } });
      return { status: r.status, out: r.stdout + r.stderr };
    };
    let r = run([p.root]);
    assert.equal(r.status, 0, r.out);
    assert.match(r.out, /^cpu \(/m);
    assert.match(r.out, /Visual C\+\+ runtime, not in the pack: VCRUNTIME140_1\.dll/);
    assert.match(r.out, /^ {2}note: cpu pack: llama-server\.exe needs VCRUNTIME140_1\.dll/m);
    r = run([p.dir], { GITHUB_ACTIONS: "true" });
    assert.equal(r.status, 0, r.out);
    assert.match(r.out, /^::notice title=Visual C\+\+ runtime::cpu pack: llama-server\.exe needs VCRUNTIME140_1\.dll/m);
    r = run([path.join(p.root, "missing")]);
    assert.equal(r.status, 0, r.out);
    assert.match(r.out, /nothing to list/);
    assert.equal(run([]).status, 2);
  } finally {
    p.cleanup();
  }
});
