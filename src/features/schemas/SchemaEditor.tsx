import type { TFunction } from "i18next";
import { ArrowDown, ArrowUp, Plus, Trash2 } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input, Textarea } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { api } from "@/lib/ipc";
import { FIELD_KINDS, SCALAR_FIELD_KINDS, newFieldDef, type FieldDef, type FieldKind, type LintIssue, type SchemaDraft } from "@/lib/types";

// ---------------------------------------------------------------------------
// Key / kind suggestions — ported from `pipeline::schema::{known_key, suffix_key,
// key_from_label, kind_from_label}` so Korean labels get the same keys the
// backend's form derivation would produce.
// ---------------------------------------------------------------------------

/** Common Korean form labels → canonical keys (`known_key`). */
const KNOWN_KEYS: Record<string, string> = {
  상호: "company_name",
  "상호(법인명)": "company_name",
  법인명: "company_name",
  회사명: "company_name",
  업체명: "company_name",
  사업자등록번호: "brn",
  사업자번호: "brn",
  등록번호: "registration_no",
  법인등록번호: "corporate_no",
  대표자: "representative",
  대표자명: "representative",
  대표이사: "representative",
  성명: "name",
  이름: "name",
  담당자: "contact_person",
  주소: "address",
  사업장소재지: "business_address",
  사업장주소: "business_address",
  본점소재지: "head_office_address",
  업태: "business_type",
  종목: "business_item",
  생년월일: "birth_date",
  주민등록번호: "rrn",
  연락처: "phone",
  전화번호: "phone",
  전화: "phone",
  휴대전화: "mobile",
  핸드폰: "mobile",
  팩스: "fax",
  이메일: "email",
  "e-mail": "email",
  날짜: "date",
  일자: "date",
  작성일: "issue_date",
  작성일자: "issue_date",
  발행일: "issue_date",
  발급일: "issue_date",
  신청일: "application_date",
  계약일: "contract_date",
  계약기간: "contract_period",
  유효기간: "valid_until",
  납기: "due_date",
  납품일: "delivery_date",
  금액: "amount",
  합계: "total",
  합계금액: "total",
  총액: "total",
  공급가액: "supply_amount",
  세액: "tax_amount",
  부가세: "vat",
  부가가치세: "vat",
  수량: "quantity",
  단가: "unit_price",
  품명: "item_name",
  품목: "item_name",
  규격: "spec",
  비고: "note",
  계약명: "contract_title",
  계약금액: "contract_amount",
  제목: "title",
  건명: "title",
  서식명: "form_title",
  수신: "recipient",
  참조: "cc",
  발신: "sender",
  문서번호: "document_no",
  번호: "number",
  부서: "department",
  직위: "position",
  직급: "position",
  승인번호: "approval_no",
  결제수단: "payment_method",
  계좌번호: "account_no",
  은행: "bank",
  예금주: "account_holder",
  서명: "signature",
  날인: "seal",
};

/** Labels ending in a well-known noun (`견적일자` → `date`, `계약번호` → `number`); non-ASCII labels only (`suffix_key`). */
const SUFFIX_KEYS: ReadonlyArray<readonly [string, string]> = [
  ["일자", "date"],
  ["날짜", "date"],
  ["연월일", "date"],
  ["년월일", "date"],
  ["금액", "amount"],
  ["가액", "amount"],
  ["단가", "unit_price"],
  ["수량", "quantity"],
  ["번호", "number"],
  ["기간", "period"],
  ["주소", "address"],
  ["소재지", "address"],
  ["연락처", "phone"],
  ["전화", "phone"],
  ["이메일", "email"],
  ["성명", "name"],
  ["이름", "name"],
  ["명", "name"],
  ["여부", "flag"],
  ["사유", "reason"],
  ["비고", "note"],
];

const KEY_RE = /^[a-z][a-z0-9_]{0,39}$/;
/** Placeholder keys the editor hands out (`field_3`, `col_2`); a label suggestion may replace them. */
const PLACEHOLDER_KEY_RE = /^(field|col)_\d+$/;
const PRINTABLE_ASCII_RE = /^[\x20-\x7e]*$/;

function normalizeLabel(label: string): string {
  return label.trim().replace(/[:：]+$/, "").replace(/[\s ]+/g, "");
}

/**
 * Key for a label (mirrors `pipeline::schema::key_from_label`): dictionary hit,
 * suffix rule, then the ASCII slug; `null` when nothing usable comes out
 * (an unknown Korean label, an empty label).
 */
export function keyFromLabel(label: string): string | null {
  const l = normalizeLabel(label);
  if (!l) return null;
  const known = KNOWN_KEYS[l];
  if (known) return known;
  if (!PRINTABLE_ASCII_RE.test(l)) {
    const hit = SUFFIX_KEYS.find(([suffix]) => l.endsWith(suffix));
    if (hit) return hit[1];
  }
  let ascii = label
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "");
  if (/^[0-9]/.test(ascii)) ascii = `f_${ascii}`;
  return KEY_RE.test(ascii) ? ascii : null;
}

/** Field kind guessed from a label (mirrors `pipeline::schema::kind_from_label`); scalar kinds only. */
export function kindFromLabel(label: string): FieldKind {
  const l = label.replace(/\s+/g, "");
  if (l.includes("주민") || l.includes("외국인등록")) return "rrn";
  if (l.includes("사업자등록번호") || l.includes("사업자번호")) return "brn";
  if (l.includes("계좌")) return "account";
  if (l.includes("이메일") || /mail/i.test(l)) return "email";
  if (/(전화|연락처|휴대|팩스)/.test(l)) return "phone";
  if (/(수량|인원|개월)/.test(l)) return "integer";
  if (/(금액|가액|합계|단가|세액|총액|부가세|비용)/.test(l)) return "amount";
  if (/(일자|날짜|년월일|연월일|기간|일시)/.test(l) || (l.endsWith("일") && [...l].length <= 6)) return "date";
  if (l.includes("여부")) return "boolean";
  return "string";
}

// ---------------------------------------------------------------------------
// Row identity: a client-side uid per field object so React keys survive
// edits, moves and removes (`FieldDef` itself stays exactly what the backend
// expects — the uid never travels over IPC).
// ---------------------------------------------------------------------------

const rowUids = new WeakMap<FieldDef, string>();
let nextRowUid = 0;

function uidOf(field: FieldDef): string {
  let uid = rowUids.get(field);
  if (!uid) {
    nextRowUid += 1;
    uid = `row-${nextRowUid}`;
    rowUids.set(field, uid);
  }
  return uid;
}

/** An edited copy keeps the identity of the row it replaces. */
function withUid(from: FieldDef, to: FieldDef): FieldDef {
  rowUids.set(to, uidOf(from));
  return to;
}

/** Backend lint issue → localized text (`schemas.lint.<code>`), falling back to the backend's English message. */
function lintText(issue: LintIssue, t: TFunction): string {
  const detail = /'([^']*)'/.exec(issue.message)?.[1] ?? "";
  const n = /\d+/.exec(issue.message)?.[0] ?? "";
  return t(`schemas.lint.${issue.code}`, { path: issue.path, detail, n, defaultValue: issue.message });
}

function FieldRow({ field, index, count, nested, onChange, onRemove, onMove }: { field: FieldDef; index: number; count: number; nested: boolean; onChange: (f: FieldDef) => void; onRemove: () => void; onMove: (delta: number) => void }) {
  const { t } = useTranslation();
  const kinds = nested ? SCALAR_FIELD_KINDS : FIELD_KINDS;
  // The key follows the labels only while the user has not edited it (rows that
  // arrive with a real key — saved schemas, NL / form results — count as edited).
  const [keyTouched, setKeyTouched] = useState(() => field.key !== "" && !PLACEHOLDER_KEY_RE.test(field.key));
  const [kindTouched, setKindTouched] = useState(() => field.kind !== "string");
  const placeholderKey = useRef(field.key);
  const set = (patch: Partial<FieldDef>) => onChange(withUid(field, { ...field, ...patch }));
  const setChild = (i: number, c: FieldDef) => set({ fields: field.fields.map((x, k) => (k === i ? c : x)) });

  const suggestedKey = (labelKo: string, labelEn: string, final: boolean): string => keyFromLabel(labelKo) ?? keyFromLabel(labelEn) ?? (final ? placeholderKey.current : field.key);
  const changeLabel = (patch: Partial<Pick<FieldDef, "label_ko" | "label_en">>) => {
    const next = { ...field, ...patch };
    set(keyTouched ? patch : { ...patch, key: suggestedKey(next.label_ko, next.label_en, false) });
  };
  /** On label blur: settle the key (placeholder when nothing fits) and guess the kind of an untouched row. */
  const settleLabel = () => {
    const patch: Partial<FieldDef> = {};
    if (!keyTouched) {
      const key = suggestedKey(field.label_ko, field.label_en, true);
      if (key !== field.key) patch.key = key;
    }
    if (!kindTouched && field.kind === "string") {
      const kind = kindFromLabel(field.label_ko || field.label_en);
      if (kind !== "string") patch.kind = kind;
    }
    if (Object.keys(patch).length > 0) set(patch);
  };

  return (
    <div className="rounded-md border border-border bg-bg p-2" data-testid={nested ? "schema-column" : "schema-field"} data-field-key={field.key}>
      <div className="grid grid-cols-[1fr_1fr_1fr_7.5rem_auto] items-end gap-2">
        <div>
          <Label className="text-xs">{t("schemas.editor.key")}</Label>
          <Input
            value={field.key}
            onChange={(e) => {
              const key = e.target.value;
              setKeyTouched(key.trim() !== "");
              set({ key });
            }}
            className="h-8 font-mono text-xs"
            aria-label={t("schemas.editor.key")}
            data-testid="schema-field-key"
          />
        </div>
        <div>
          <Label className="text-xs">{t("schemas.editor.label_ko")}</Label>
          <Input value={field.label_ko} onChange={(e) => changeLabel({ label_ko: e.target.value })} onBlur={settleLabel} className="h-8" aria-label={t("schemas.editor.label_ko")} data-testid="schema-field-label-ko" />
        </div>
        <div>
          <Label className="text-xs">{t("schemas.editor.label_en")}</Label>
          <Input value={field.label_en} onChange={(e) => changeLabel({ label_en: e.target.value })} onBlur={settleLabel} className="h-8" aria-label={t("schemas.editor.label_en")} />
        </div>
        <div>
          <Label className="text-xs">{t("schemas.editor.kind")}</Label>
          <Select
            value={field.kind}
            onValueChange={(v) => {
              setKindTouched(true);
              set({ kind: v as FieldKind, fields: v === "table" || v === "group" ? (field.fields.length ? field.fields : [newFieldDef("value")]) : [] });
            }}
          >
            <SelectTrigger className="h-8" aria-label={t("schemas.editor.kind")} data-testid="schema-field-kind">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {kinds.map((k) => (
                <SelectItem key={k} value={k}>
                  {t(`pipeline.kinds.${k}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="flex items-center gap-1 pb-1">
          <label className="flex items-center gap-1 text-xs">
            <Switch checked={field.required} onCheckedChange={(v) => set({ required: v })} aria-label={t("schemas.editor.required")} />
            {t("schemas.editor.required")}
          </label>
          <Button size="icon-sm" variant="ghost" onClick={() => onMove(-1)} disabled={index === 0} aria-label={t("schemas.editor.up")}>
            <ArrowUp />
          </Button>
          <Button size="icon-sm" variant="ghost" onClick={() => onMove(1)} disabled={index >= count - 1} aria-label={t("schemas.editor.down")}>
            <ArrowDown />
          </Button>
          <Button size="icon-sm" variant="ghost" className="text-danger" onClick={onRemove} aria-label={t("schemas.editor.remove")} data-testid="schema-field-remove">
            <Trash2 />
          </Button>
        </div>
      </div>
      {field.kind !== "table" && field.kind !== "group" && (
        <div className="mt-2 grid grid-cols-[1fr_1fr_1fr_auto] items-end gap-2">
          <div>
            <Label className="text-xs">{t("schemas.editor.regex")}</Label>
            <Input value={field.regex_hint ?? ""} onChange={(e) => set({ regex_hint: e.target.value || null })} className="h-7 font-mono text-xs" aria-label={t("schemas.editor.regex")} />
          </div>
          <div>
            <Label className="text-xs">{t("schemas.editor.example")}</Label>
            <Input value={field.example ?? ""} onChange={(e) => set({ example: e.target.value || null })} className="h-7 text-xs" aria-label={t("schemas.editor.example")} />
          </div>
          <div>
            <Label className="text-xs">{t("schemas.editor.options")}</Label>
            <Input
              value={field.options.join(", ")}
              onChange={(e) => set({ options: e.target.value.split(/[,，]/).map((s) => s.trim()).filter(Boolean) })}
              disabled={field.kind !== "enum"}
              className="h-7 text-xs"
              aria-label={t("schemas.editor.options")}
            />
          </div>
          <label className="flex items-center gap-1 pb-1 text-xs">
            <Switch checked={field.mask} onCheckedChange={(v) => set({ mask: v })} aria-label={t("schemas.editor.mask")} />
            {t("schemas.editor.mask")}
          </label>
        </div>
      )}
      {(field.kind === "table" || field.kind === "group") && (
        <div className="mt-2 flex flex-col gap-1 border-l-2 border-accent pl-2">
          <div className="flex items-center justify-between">
            <span className="text-xs font-medium text-fg-muted">{t("schemas.editor.columns")}</span>
            <Button size="sm" variant="ghost" onClick={() => set({ fields: [...field.fields, newFieldDef(`col_${field.fields.length + 1}`)] })} data-testid="schema-add-column">
              <Plus />
              {t("schemas.editor.add_column")}
            </Button>
          </div>
          {field.fields.map((c, i) => (
            <FieldRow
              key={uidOf(c)}
              field={c}
              index={i}
              count={field.fields.length}
              nested
              onChange={(nc) => setChild(i, nc)}
              onRemove={() => set({ fields: field.fields.filter((_, k) => k !== i) })}
              onMove={(d) => {
                const j = i + d;
                if (j < 0 || j >= field.fields.length) return;
                const next = [...field.fields];
                [next[i], next[j]] = [next[j]!, next[i]!];
                set({ fields: next });
              }}
            />
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * Field grid of a schema draft (design §3.7.2 "필드 그리드: key/라벨/타입/필수/정규식/예시")
 * with the grammar lint shown live (debounced through the backend's lint command).
 */
export function SchemaEditor({ draft, onChange, lintTick = 400 }: { draft: SchemaDraft; onChange: (d: SchemaDraft) => void; lintTick?: number }) {
  const { t } = useTranslation();
  const [issues, setIssues] = useState<LintIssue[]>([]);
  /** Sequence of the lint request whose answer is still wanted; a slower, older response is dropped. */
  const lintSeq = useRef(0);
  const def = draft.definition;
  const set = (patch: Partial<SchemaDraft>) => onChange({ ...draft, ...patch });
  const setFields = (fields: FieldDef[]) => set({ definition: { ...def, fields } });

  useEffect(() => {
    lintSeq.current += 1;
    const seq = lintSeq.current;
    const h = setTimeout(() => {
      api.pipelineSchemaLint(def).then(
        (result) => {
          if (seq === lintSeq.current) setIssues(result);
        },
        () => {
          if (seq === lintSeq.current) setIssues([]);
        },
      );
    }, lintTick);
    return () => clearTimeout(h);
  }, [def, lintTick]);
  useEffect(
    () => () => {
      lintSeq.current += 1;
    },
    [],
  );

  const errors = useMemo(() => issues.filter((i) => i.level === "error"), [issues]);
  const warnings = useMemo(() => issues.filter((i) => i.level === "warn"), [issues]);

  return (
    <div className="flex flex-col gap-3" data-testid="schema-editor">
      <div className="grid grid-cols-2 gap-2 md:grid-cols-3">
        <div>
          <Label htmlFor="schema-name">{t("schemas.editor.name")}</Label>
          <Input id="schema-name" value={draft.name} onChange={(e) => set({ name: e.target.value })} data-testid="schema-name" />
        </div>
        <div>
          <Label htmlFor="schema-name-en">{t("schemas.editor.name_en")}</Label>
          <Input id="schema-name-en" value={draft.name_en ?? ""} onChange={(e) => set({ name_en: e.target.value || null })} />
        </div>
        <div>
          <Label htmlFor="schema-doc-type">{t("schemas.editor.doc_type")}</Label>
          <Input id="schema-doc-type" value={draft.doc_type} onChange={(e) => set({ doc_type: e.target.value.trim() })} className="font-mono" data-testid="schema-doc-type" />
          <p className="text-xs text-fg-subtle">{t("schemas.editor.doc_type_hint")}</p>
        </div>
      </div>
      <div>
        <Label htmlFor="schema-desc">{t("schemas.editor.description")}</Label>
        <Textarea id="schema-desc" rows={2} value={draft.description_ko ?? ""} onChange={(e) => set({ description_ko: e.target.value || null })} />
      </div>
      <div className="flex items-center justify-between">
        <span className="font-medium">{t("schemas.editor.fields")}</span>
        <Button size="sm" variant="secondary" onClick={() => setFields([...def.fields, newFieldDef(`field_${def.fields.length + 1}`)])} data-testid="schema-add-field">
          <Plus />
          {t("schemas.editor.add_field")}
        </Button>
      </div>
      <div className="flex flex-col gap-2" data-testid="schema-fields">
        {def.fields.map((f, i) => (
          <FieldRow
            key={uidOf(f)}
            field={f}
            index={i}
            count={def.fields.length}
            nested={false}
            onChange={(nf) => setFields(def.fields.map((x, k) => (k === i ? nf : x)))}
            onRemove={() => setFields(def.fields.filter((_, k) => k !== i))}
            onMove={(d) => {
              const j = i + d;
              if (j < 0 || j >= def.fields.length) return;
              const next = [...def.fields];
              [next[i], next[j]] = [next[j]!, next[i]!];
              setFields(next);
            }}
          />
        ))}
      </div>
      <div className="flex flex-wrap items-center gap-2 text-xs" role="status" data-testid="schema-lint">
        {errors.length === 0 && warnings.length === 0 && <Badge variant="ok">{t("schemas.editor.lint_ok")}</Badge>}
        {errors.length > 0 && <Badge variant="danger">{t("schemas.editor.lint_errors", { count: errors.length })}</Badge>}
        {warnings.length > 0 && <Badge variant="warn">{t("schemas.editor.lint_warnings", { count: warnings.length })}</Badge>}
        {issues.slice(0, 6).map((i, k) => (
          <span key={`${i.code}:${i.path}:${k}`} className={i.level === "error" ? "text-danger" : "text-warn"} data-lint-code={i.code}>
            {i.path ? `${i.path}: ` : ""}
            {lintText(i, t)}
          </span>
        ))}
      </div>
    </div>
  );
}
