// The plan editor's op table (`opSpecs.ts`) against the backend's (`planner/schema.rs`, CON-13). The Rust
// file is read as text — no build step — and every op, parameter, enum value and `required` flag found
// there must be in the TypeScript mirror, or the editor shows an empty select for a value the planner
// produced (`group_by = project`) or hides a parameter altogether (`add_to_project.as_reference`).
import { describe, expect, it } from "vitest";

import schemaRs from "../../../crates/doculens-core/src/planner/schema.rs?raw";

import { OP_KINDS, OP_SPECS, type ParamType } from "./opSpecs";

interface RustParam {
  name: string;
  /** `Str` | `Bool` | `Int` | `Number` | `Date` | `Enum` | `StrList` | `IntList` | `EnumList` | `Scalar`. */
  type: string;
  /** Enum values; null when they come from another file (`SETTING_KEYS`, `settings_catalog::BACKENDS`). */
  values: string[] | null;
  required: boolean;
}

interface RustOp {
  op: string;
  params: RustParam[];
}

function strings(list: string): string[] {
  return [...list.matchAll(/"([^"]*)"/g)].map((m) => m[1]!);
}

/** `pub const NAME: &[&str] = &["a", "b"];` → NAME → values. */
function constants(source: string): Map<string, string[]> {
  const out = new Map<string, string[]>();
  for (const m of source.matchAll(/pub const (\w+): &\[&str\] =\s*&\[([^\]]*)\];/g)) out.set(m[1]!, strings(m[2]!));
  return out;
}

function snake(camel: string): string {
  return camel.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();
}

/** The `OP_SPECS` table of schema.rs: one entry per `OpSpec { kind: OpKind::…, params: &[ p(…), … ], … }`. */
function parseRustSpecs(source: string): RustOp[] {
  const start = source.indexOf("pub const OP_SPECS");
  const end = source.indexOf("\n];", start);
  expect(start, "schema.rs has an OP_SPECS table").toBeGreaterThan(-1);
  expect(end, "the OP_SPECS table ends").toBeGreaterThan(start);
  const consts = constants(source);
  const ops: RustOp[] = [];
  for (const chunk of source.slice(start, end).split(/OpSpec\s*\{/).slice(1)) {
    const kind = chunk.match(/kind:\s*OpKind::(\w+)/);
    expect(kind, `an OpSpec without a kind: ${chunk.slice(0, 60)}`).not.toBeNull();
    const params: RustParam[] = [];
    // p("name", ParamType::Type or ParamType::Type(args), true | false, …
    for (const m of chunk.matchAll(/\bp\(\s*"(\w+)"\s*,\s*ParamType::(\w+)(?:\(([^)]*)\))?\s*,\s*(true|false)\s*,/g)) {
      const [, name, type, args, required] = m;
      let values: string[] | null = null;
      if (type === "Enum" || type === "EnumList") {
        const a = (args ?? "").trim();
        values = a.startsWith("&[") ? strings(a) : (consts.get(a) ?? null);
      }
      params.push({ name: name!, type: type!, values, required: required === "true" });
    }
    ops.push({ op: snake(kind![1]!), params });
  }
  return ops;
}

/** The editor's field kinds that can stand for a Rust parameter type (a string may be shown as a date picker). */
const KINDS: Record<string, ParamType["kind"][]> = {
  Str: ["string", "date"],
  Bool: ["boolean"],
  Int: ["integer"],
  Number: ["number"],
  Date: ["date"],
  Enum: ["enum"],
  StrList: ["list"],
  IntList: ["int_list"],
  EnumList: ["enum_list"],
  Scalar: ["scalar"],
};

const rust = parseRustSpecs(schemaRs);

describe("opSpecs.ts mirrors planner/schema.rs", () => {
  it("parses the Rust table", () => {
    // 63 ops at 0.7.0; the parser must not silently lose entries or parameters.
    expect(rust.length).toBeGreaterThanOrEqual(63);
    expect(new Set(rust.map((o) => o.op)).size).toBe(rust.length);
    expect(rust.find((o) => o.op === "export")?.params.map((p) => p.name)).toEqual(["format", "path", "columns", "layout", "include", "target"]);
    expect(rust.find((o) => o.op === "aggregate")?.params.find((p) => p.name === "group_by")?.values).toContain("project");
    expect(rust.find((o) => o.op === "set_setting")?.params.find((p) => p.name === "key")).toMatchObject({ type: "Enum", values: null, required: true });
    expect(rust.reduce((n, o) => n + o.params.length, 0)).toBeGreaterThan(150);
  });

  it("has every op of the backend, and no op of its own", () => {
    const missing = rust.map((o) => o.op).filter((op) => !(op in OP_SPECS));
    expect(missing, "ops in schema.rs that opSpecs.ts lacks").toEqual([]);
    const extra = OP_KINDS.filter((op) => !rust.some((o) => o.op === op));
    expect(extra, "ops in opSpecs.ts that schema.rs does not have").toEqual([]);
  });

  it("has every parameter of every op, with the same type, enum values and required flag", () => {
    const problems: string[] = [];
    for (const op of rust) {
      const spec = (OP_SPECS as Record<string, (typeof OP_SPECS)[keyof typeof OP_SPECS] | undefined>)[op.op];
      if (!spec) continue; // reported by the test above
      for (const p of op.params) {
        const at = `${op.op}.${p.name}`;
        const mine = spec.params.find((x) => x.name === p.name);
        if (!mine) {
          problems.push(`${at}: missing`);
          continue;
        }
        if (!(KINDS[p.type] ?? []).includes(mine.type.kind)) problems.push(`${at}: type ${mine.type.kind}, backend ${p.type}`);
        if (mine.required !== p.required) problems.push(`${at}: required ${String(mine.required)}, backend ${String(p.required)}`);
        if (p.values && (mine.type.kind === "enum" || mine.type.kind === "enum_list")) {
          const have = new Set(mine.type.values);
          const lacking = p.values.filter((v) => !have.has(v));
          const surplus = [...have].filter((v) => !p.values!.includes(v));
          if (lacking.length > 0) problems.push(`${at}: enum values missing: ${lacking.join(", ")}`);
          if (surplus.length > 0) problems.push(`${at}: enum values the backend does not have: ${surplus.join(", ")}`);
        }
      }
      for (const mine of spec.params) if (!op.params.some((p) => p.name === mine.name)) problems.push(`${op.op}.${mine.name}: not a parameter of the backend`);
    }
    expect(problems).toEqual([]);
  });
});
