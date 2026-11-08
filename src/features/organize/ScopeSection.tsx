import { useTranslation } from "react-i18next";

import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { docTypeLabel } from "@/features/pipeline/labels";
import { useDocTypes, useDocumentStats } from "@/lib/queries";

import type { ScopeState } from "./scope";

// Radix Select cannot represent "" / null values: sentinels for the "all" options.
const ALL = "__all__";
const SELECTION = "__selection__";
const FOLDER_PREFIX = "folder:";

/**
 * "문서 범위" shared by the organise and export tabs: everything / the library selection / a
 * folder, narrowed by document type and a date range (document date, import date as fallback).
 */
export function ScopeSection({ value, onChange, selectedCount, testId }: { value: ScopeState; onChange: (next: ScopeState) => void; selectedCount: number; testId: string }) {
  const { t, i18n } = useTranslation();
  const { data: stats } = useDocumentStats();
  const { data: docTypes } = useDocTypes();
  const folders = stats?.folders ?? [];
  const modeValue = value.mode === "selection" ? SELECTION : value.mode === "folder" ? `${FOLDER_PREFIX}${value.folder ?? ""}` : ALL;
  const setMode = (v: string) => {
    if (v === SELECTION) onChange({ ...value, mode: "selection", folder: null });
    else if (v.startsWith(FOLDER_PREFIX)) onChange({ ...value, mode: "folder", folder: v.slice(FOLDER_PREFIX.length) });
    else onChange({ ...value, mode: "all", folder: null });
  };
  const slugs = [...new Set([...(docTypes ?? []).map((d) => d.slug), "other"])];
  return (
    <div className="flex flex-wrap items-end gap-3" data-testid={testId}>
      <div className="flex flex-col gap-1">
        <Label htmlFor={`${testId}-mode`}>{t("organize.scope.mode")}</Label>
        <Select value={modeValue} onValueChange={setMode}>
          <SelectTrigger id={`${testId}-mode`} className="h-8 w-60" data-testid={`${testId}-mode`}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL}>{t("organize.scope.all")}</SelectItem>
            {selectedCount > 0 && <SelectItem value={SELECTION}>{t("organize.scope.selection", { count: selectedCount })}</SelectItem>}
            {folders.map((f) => (
              <SelectItem key={f.rel_dir} value={`${FOLDER_PREFIX}${f.rel_dir}`}>
                {t("organize.scope.folder", { folder: f.rel_dir === "" ? t("organize.scope.root_folder") : f.rel_dir })} ({f.count})
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="flex flex-col gap-1">
        <Label htmlFor={`${testId}-type`}>{t("organize.scope.doc_type")}</Label>
        <Select value={value.docType ?? ALL} onValueChange={(v) => onChange({ ...value, docType: v === ALL ? null : v })}>
          <SelectTrigger id={`${testId}-type`} className="h-8 w-44" data-testid={`${testId}-type`}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL}>{t("organize.scope.all_types")}</SelectItem>
            {slugs.map((slug) => (
              <SelectItem key={slug} value={slug}>
                {docTypeLabel(slug, docTypes, t, i18n.language)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="flex flex-col gap-1">
        <Label htmlFor={`${testId}-from`}>{t("organize.scope.date_from")}</Label>
        <Input id={`${testId}-from`} type="date" value={value.dateFrom} onChange={(e) => onChange({ ...value, dateFrom: e.target.value })} className="h-8 w-40" data-testid={`${testId}-from`} />
      </div>
      <div className="flex flex-col gap-1">
        <Label htmlFor={`${testId}-to`}>{t("organize.scope.date_to")}</Label>
        <Input id={`${testId}-to`} type="date" value={value.dateTo} onChange={(e) => onChange({ ...value, dateTo: e.target.value })} className="h-8 w-40" data-testid={`${testId}-to`} />
      </div>
      <span className="pb-2 text-xs text-fg-subtle">{t("organize.scope.date_hint")}</span>
    </div>
  );
}
