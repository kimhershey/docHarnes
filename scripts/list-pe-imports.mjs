#!/usr/bin/env node
// Lists the DLLs that a bundled llama-server pack needs but does not contain (informational, PKG-10).
//
//   node scripts/list-pe-imports.mjs src-tauri/resources/engines        # every <pack>/ below it
//   node scripts/list-pe-imports.mjs <pack dir> [--entry llama-server.exe] [--verbose]
//
// For each pack folder it reads the PE import tables (static and delay-load) of llama-server.exe, of every DLL it
// reaches inside the folder and of the ggml-*.dll backends that ggml loads at run time, and prints the imports
// that are not satisfied by a file in the folder:
//
//   Visual C++ runtime   vcruntime140*.dll / msvcp140*.dll / vcomp140.dll … — NOT part of Windows. When a pack
//                        imports them without shipping them, llama-server only starts on a PC that has the
//                        Microsoft Visual C++ 2015–2022 Redistributable (x64) installed.
//   Universal CRT        api-ms-win-crt-*.dll — part of Windows 10 and 11.
//   other                kernel32.dll, ws2_32.dll, vulkan-1.dll, nvcuda.dll … — Windows itself or a GPU driver.
//
// Pure Node (no dumpbin / objdump needed), reads only the headers and the import tables. It never fails a build:
// the exit code is 0 unless the arguments are unusable, and a file it cannot parse is reported and skipped.
import { closeSync, existsSync, openSync, readSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const VC_RUNTIME = /^(vcruntime\d+(_\d+)?|msvcp\d+(_[a-z0-9_]+)?|vcomp\d+|concrt\d+|vccorlib\d+|vcamp\d+|mfc\d+[a-z]*)\.dll$/i;
const API_SET = /^(api|ext)-ms-win-/i;

/** Random-access reads from one file. */
class Reader {
  constructor(file) {
    this.fd = openSync(file, "r");
    this.size = statSync(file).size;
  }
  bytes(offset, length) {
    if (offset < 0 || offset + length > this.size) throw new Error(`read past the end (offset ${offset}, length ${length})`);
    const buf = Buffer.alloc(length);
    let done = 0;
    while (done < length) {
      const n = readSync(this.fd, buf, done, length - done, offset + done);
      if (n <= 0) throw new Error("short read");
      done += n;
    }
    return buf;
  }
  cstring(offset, max = 260) {
    const buf = this.bytes(offset, Math.min(max, this.size - offset));
    const end = buf.indexOf(0);
    return buf.toString("latin1", 0, end < 0 ? buf.length : end);
  }
  close() {
    closeSync(this.fd);
  }
}

/** DLL names in the import and delay-import directories of a PE32 / PE32+ image. */
export function peImports(file) {
  const r = new Reader(file);
  try {
    if (r.size < 0x40 || r.bytes(0, 2).toString("latin1") !== "MZ") throw new Error("not a PE file (no MZ header)");
    const pe = r.bytes(0x3c, 4).readUInt32LE(0);
    if (r.bytes(pe, 4).toString("latin1") !== "PE\0\0") throw new Error("not a PE file (no PE signature)");
    const coff = r.bytes(pe + 4, 20);
    const sectionCount = coff.readUInt16LE(2);
    const optionalSize = coff.readUInt16LE(16);
    const optional = r.bytes(pe + 24, optionalSize);
    const magic = optional.readUInt16LE(0);
    if (magic !== 0x20b && magic !== 0x10b) throw new Error(`unknown optional header magic 0x${magic.toString(16)}`);
    const dirBase = magic === 0x20b ? 112 : 96; // PE32+ / PE32
    const dirCount = optional.readUInt32LE(dirBase - 4);
    const directory = (index) =>
      index < dirCount && dirBase + index * 8 + 8 <= optional.length
        ? { rva: optional.readUInt32LE(dirBase + index * 8), size: optional.readUInt32LE(dirBase + index * 8 + 4) }
        : { rva: 0, size: 0 };
    const sections = [];
    const table = r.bytes(pe + 24 + optionalSize, sectionCount * 40);
    for (let i = 0; i < sectionCount; i++) {
      const s = table.subarray(i * 40, i * 40 + 40);
      sections.push({ virtualSize: s.readUInt32LE(8), rva: s.readUInt32LE(12), rawSize: s.readUInt32LE(16), raw: s.readUInt32LE(20) });
    }
    const offsetOf = (rva) => {
      const s = sections.find((x) => rva >= x.rva && rva < x.rva + Math.max(x.virtualSize, x.rawSize));
      if (!s) throw new Error(`RVA 0x${rva.toString(16)} is outside every section`);
      return rva - s.rva + s.raw;
    };
    const names = [];
    const imports = directory(1); // IMAGE_DIRECTORY_ENTRY_IMPORT: 20-byte descriptors, name RVA at +12
    if (imports.rva) {
      for (let at = offsetOf(imports.rva), i = 0; i < 4096; i++, at += 20) {
        const d = r.bytes(at, 20);
        const nameRva = d.readUInt32LE(12);
        if (!nameRva && !d.readUInt32LE(0) && !d.readUInt32LE(16)) break;
        if (nameRva) names.push(r.cstring(offsetOf(nameRva)));
      }
    }
    const delayed = directory(13); // IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT: 32-byte descriptors, name RVA at +4
    if (delayed.rva) {
      for (let at = offsetOf(delayed.rva), i = 0; i < 4096; i++, at += 32) {
        const d = r.bytes(at, 32);
        const nameRva = d.readUInt32LE(4);
        if (!nameRva) break;
        if (d.readUInt32LE(0) & 1) names.push(r.cstring(offsetOf(nameRva))); // bit 0: the fields are RVAs
      }
    }
    return [...new Set(names)];
  } finally {
    r.close();
  }
}

function packDirs(root, entry) {
  if (!existsSync(root) || !statSync(root).isDirectory()) return [];
  if (existsSync(path.join(root, entry))) return [root];
  return readdirSync(root)
    .map((name) => path.join(root, name))
    .filter((dir) => statSync(dir).isDirectory() && existsSync(path.join(dir, entry)))
    .sort();
}

/** Imports of the entry binary, of what it reaches inside `dir`, and of the run-time loaded ggml backends. */
export function analysePack(dir, entry) {
  const files = new Map(readdirSync(dir).map((name) => [name.toLowerCase(), name])); // Windows resolves names case-insensitively
  const queue = [entry, ...[...files.values()].filter((n) => /^ggml-.*\.dll$/i.test(n)).sort()];
  const seen = new Set();
  const external = new Map(); // lower-case DLL name → { name, importers }
  const unreadable = [];
  while (queue.length) {
    const name = files.get(queue.shift().toLowerCase());
    if (!name || seen.has(name)) continue;
    seen.add(name);
    let imports;
    try {
      imports = peImports(path.join(dir, name));
    } catch (e) {
      unreadable.push(`${name}: ${e.message}`);
      continue;
    }
    for (const dll of imports) {
      if (files.has(dll.toLowerCase())) queue.push(dll);
      else {
        const key = dll.toLowerCase();
        if (!external.has(key)) external.set(key, { name: dll, importers: [] });
        external.get(key).importers.push(name);
      }
    }
  }
  const all = [...external.values()].sort((a, b) => (a.name.toLowerCase() < b.name.toLowerCase() ? -1 : 1));
  return {
    scanned: [...seen].sort(),
    unreadable,
    vcRuntime: all.filter((d) => VC_RUNTIME.test(d.name)),
    ucrt: all.filter((d) => API_SET.test(d.name)),
    other: all.filter((d) => !VC_RUNTIME.test(d.name) && !API_SET.test(d.name)),
  };
}

function main(argv) {
  const verbose = argv.includes("--verbose");
  const entryAt = argv.indexOf("--entry");
  const entryValueAt = entryAt >= 0 ? entryAt + 1 : -1;
  const entry = entryAt >= 0 ? argv[entryValueAt] : "llama-server.exe";
  const roots = argv.filter((a, i) => !a.startsWith("--") && i !== entryValueAt);
  if (!roots.length || !entry) {
    console.error("usage: node scripts/list-pe-imports.mjs <engines dir or pack dir>... [--entry llama-server.exe] [--verbose]");
    return 2;
  }
  const annotate = process.env.GITHUB_ACTIONS === "true";
  const packs = roots.flatMap((root) => packDirs(root, entry));
  if (!packs.length) {
    console.log(`no folder with ${entry} under ${roots.join(", ")} — nothing to list`);
    return 0;
  }
  for (const dir of packs) {
    const label = path.basename(path.resolve(dir));
    let r;
    try {
      r = analysePack(dir, entry);
    } catch (e) {
      console.log(`${label}: could not be analysed (${e.message})`);
      continue;
    }
    const list = (items) => (items.length ? items.map((d) => d.name).join(", ") : "none");
    console.log(`${label} (${dir}): ${entry} + ${r.scanned.length - 1} DLLs read`);
    console.log(`  Visual C++ runtime, not in the pack: ${list(r.vcRuntime)}`);
    console.log(`  Universal CRT (part of Windows 10 / 11): ${r.ucrt.length} api-ms-win-* imports`);
    console.log(`  other DLLs from Windows or a driver: ${list(r.other)}`);
    if (verbose) for (const d of [...r.vcRuntime, ...r.other]) console.log(`    ${d.name} <- ${d.importers.join(", ")}`);
    for (const u of r.unreadable) console.log(`  could not read ${u}`);
    if (r.vcRuntime.length) {
      const message = `${label} pack: ${entry} needs ${list(r.vcRuntime)}, which the pack does not contain — the engine only starts on a PC with the Microsoft Visual C++ 2015-2022 Redistributable (x64) installed (docs/RUN-GUIDE.md, troubleshooting)`;
      console.log(annotate ? `::notice title=Visual C++ runtime::${message}` : `  note: ${message}`);
    }
  }
  return 0;
}

// Run only when executed directly (the helpers are importable) — the same check as scripts/fetch-engine.mjs.
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
