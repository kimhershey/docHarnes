import { FileOutput, FolderOpen } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { schemaForType } from "@/features/library/aggregate";
import { fieldLabel } from "@/features/pipeline/labels";
import { api, revealPath } from "@/lib/ipc";
import { useSchemas } from "@/lib/queries";
import {
  EXPORT_FORMATS,
  SCALAR_FIELD_KINDS,
  isJobFinished,
  type ExportFormat,
  type ExportInclude,
  type ExportLayout,
  type ExportPreview,
  type ExportRequest,
  type ExportResult,
  type ExportTarget,
  type Language,
  type SchemaRecord,
} from "@/lib/types";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

import { describeExportError, knownCode } from "./errors";
import { ScopeSection } from "./ScopeSection";
import { defaultScope, toScope, type ScopeState } from "./scope";
import { useJob } from "./useJob";

const TARGETS: ExportTarget[] = ["fields", "table", "text"];
const INCLUDES: ExportInclude[] = ["evidence", "confidence", "status"];
const LAYOUTS: ExportLayout[] = ["sheet_per_type", "single"];

interface ColumnOption {
  path: string;
  label: string;
}

/** Scalar fields (and the scalar children of groups) of a schema, as export column paths. */
function columnOptions(schema: SchemaRecord | undefined, lang: string): ColumnOption[] {
  if (!schema) return [];
  const out: ColumnOption[] = [];
  for (const f of schema.definition.fields) {
    if (SCALAR_FIELD_KINDS.includes(f.kind)) out.push({ path: f.key, label: fieldLabel(f, lang) });
    else if (f.kind === "group") {
      for (const c of f.fields) if (SCALAR_FIELD_KINDS.includes(c.kind)) out.push({ path: `${f.key}.${c.key}`, label: `${fieldLabel(f, lang)} · ${fieldLabel(c, lang)}` });
    }
  }
  return out;
}

export function asExportResult(v: unknown): ExportResult | null {
  if (!v || typeof v !== "object") return null;
  const r = v as Partial<ExportResult>;
  if (typeof r.path !== "string") return null;
  return {
    path: r.path,
    absolute: typeof r.absolute === "string" ? r.absolute : "",
    format: r.format ?? "xlsx",
    target: r.target ?? "fields",
    documents: r.documents ?? 0,
    rows: r.rows ?? 0,
    sheets: Array.isArray(r.sheets) ? r.sheets : [],
    overwritten: r.overwritten ?? false,
    skipped: Array.isArray(r.skipped) ? r.skipped : [],
    extra_files: Array.isArray(r.extra_files) ? r.extra_files : [],
  };
}

export interface ExportPreset {
  target: ExportTarget;
  /** Bumped per request so the same target applies again. */
  nonce: number;
}

/**
 * 내보내기 wizard: target → format → scope → columns → layout → path, live preview, export job.
 * `active` tells whether the tab is visible (the debounced preview only runs then); `preset`
 * carries a target chosen elsewhere (`/organize?tab=export&target=table` from the viewer).
 */
export function ExportTab({ selectedIds, selectionRequest, active = true, preset = null }: { selectedIds: string[]; selectionRequest: number; active?: boolean; preset?: ExportPreset | null }) {
  const { t, i18n } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const workspace = useWorkspace();
  const { data: schemas } = useSchemas();
  const [scope, setScope] = useState<ScopeState>(() => defaultScope(selectedIds.length > 0));
  const [target, setTarget] = useState<ExportTarget>("fields");
  const [format, setFormat] = useState<ExportFormat>("xlsx");
  const [onlyReviewed, setOnlyReviewed] = useState(false);
  /** null = every column. */
  const [columns, setColumns] = useState<string[] | null>(null);
  const [include, setInclude] = useState<ExportInclude[]>([]);
  const [layout, setLayout] = useState<ExportLayout>("sheet_per_type");
  const [path, setPath] = useState("");
  const pathDirty = useRef(false);
  const [overwrite, setOverwrite] = useState(false);
  const [showOverwrite, setShowOverwrite] = useState(false);
  const [preview, setPreview] = useState<ExportPreview | null>(null);
  const [previewBusy, setPreviewBusy] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [pathError, setPathError] = useState<string | null>(null);
  const [runError, setRunError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const [jobId, setJobId] = useState<string | null>(null);
  const [result, setResult] = useState<ExportResult | null>(null);
  const job = useJob(jobId);
  const handled = useRef<string | null>(null);
  const previewSeq = useRef(0);
  const language: Language = i18n.language.startsWith("ko") ? "ko" : "en";

  useEffect(() => {
    if (selectionRequest > 0 && selectedIds.length > 0) setScope((s) => ({ ...s, mode: "selection" }));
  }, [selectionRequest, selectedIds.length]);
  useEffect(() => {
    if (scope.mode === "selection" && selectedIds.length === 0) setScope((s) => ({ ...s, mode: "all" }));
  }, [scope.mode, selectedIds.length]);
  useEffect(() => {
    if (preset) setTarget(preset.target);
  }, [preset]);

  // A searchable PDF always carries the page text.
  const effectiveTarget: ExportTarget = format === "pdf_searchable" ? "text" : target;
  const schema = useMemo(() => (scope.docType ? schemaForType(schemas, scope.docType) : undefined), [schemas, scope.docType]);
  const options = useMemo(() => columnOptions(schema, i18n.language), [schema, i18n.language]);
  useEffect(() => {
    setColumns(null);
  }, [scope.docType]);

  const trimmedPath = path.trim();
  const request = useMemo<ExportRequest>(
    () => ({
      format,
      target: effectiveTarget,
      path: pathDirty.current && trimmedPath ? trimmedPath : null,
      columns: effectiveTarget === "fields" && columns ? columns : [],
      layout,
      include,
      scope: toScope(scope, selectedIds),
      only_reviewed: onlyReviewed,
      language,
    }),
    [format, effectiveTarget, trimmedPath, columns, layout, include, scope, selectedIds, onlyReviewed, language],
  );
  const reqKey = JSON.stringify(request);

  // Live preview (path, exists, documents, columns, sheets), debounced; stale answers are dropped. The tab
  // stays mounted behind the others (`forceMount`), so the preview only runs while it is visible.
  useEffect(() => {
    if (!active) return;
    const seq = ++previewSeq.current;
    const req = JSON.parse(reqKey) as ExportRequest;
    setPreviewBusy(true);
    const h = setTimeout(() => {
      void (async () => {
        try {
          const p = await api.exportPreview(req);
          if (seq !== previewSeq.current) return;
          setPreview(p);
          setPreviewError(null);
          setPathError(null);
          if (!pathDirty.current) setPath(p.path);
          if (p.exists) setShowOverwrite(true);
        } catch (e) {
          if (seq !== previewSeq.current) return;
          setPreview(null);
          if (knownCode(e, ["path_outside"])) {
            setPathError(t("export.errors.path_outside"));
            setPreviewError(null);
          } else {
            setPreviewError(describeExportError(e, t));
          }
        } finally {
          if (seq === previewSeq.current) setPreviewBusy(false);
        }
      })();
    }, 300);
    return () => clearTimeout(h);
  }, [reqKey, nonce, active, t]);

  const run = async () => {
    setRunError(null);
    setPathError(null);
    try {
      const id = await api.exportRun({ ...request, overwrite });
      handled.current = null;
      setResult(null);
      setJobId(id);
      toast(t("export.started"), "info");
    } catch (e) {
      const code = knownCode(e, ["export_exists", "path_outside", "no_documents"]);
      if (code === "export_exists") {
        setShowOverwrite(true);
        setRunError(t("export.errors.export_exists"));
      } else if (code === "path_outside") {
        setPathError(t("export.errors.path_outside"));
      } else if (code) {
        setRunError(t(`export.errors.${code}`));
      } else {
        toast(describeExportError(e, t), "danger");
      }
    }
  };

  // Follow the export job; its result is the ExportResult.
  useEffect(() => {
    if (!job || !isJobFinished(job) || handled.current === job.id) return;
    handled.current = job.id;
    void (async () => {
      let res = asExportResult(job.result);
      if (!res) {
        try {
          res = asExportResult((await api.jobsGet(job.id))?.result);
        } catch {
          /* ignore */
        }
      }
      if (job.state === "done") {
        const rel = res?.path ?? preview?.path ?? trimmedPath;
        setResult(
          res ?? {
            path: rel,
            absolute: workspace ? `${workspace.path.replace(/[\\/]+$/, "")}/${rel}` : rel,
            format,
            target: effectiveTarget,
            documents: preview?.documents ?? 0,
            rows: 0,
            sheets: preview?.sheets ?? [],
            overwritten: overwrite,
            skipped: [],
            extra_files: [],
          },
        );
        toast(t("export.result_title"), "ok");
      } else if (job.state === "failed") {
        const message = job.error ?? "";
        if (message.includes("export_exists")) {
          setShowOverwrite(true);
          setRunError(t("export.errors.export_exists"));
        } else if (message.includes("path_outside")) {
          setPathError(t("export.errors.path_outside"));
        } else {
          setRunError(t("export.failed", { message }));
        }
      } else {
        toast(t("export.cancelled"), "warn");
      }
      // The target exists now: refresh `exists`.
      setNonce((n) => n + 1);
    })();
  }, [job, preview, trimmedPath, workspace, format, effectiveTarget, overwrite, t, toast]);

  const toggleColumn = (p: string) => {
    const all = options.map((o) => o.path);
    const current = columns ?? all;
    const next = current.includes(p) ? current.filter((x) => x !== p) : [...current, p];
    setColumns(next.length === all.length && all.every((x) => next.includes(x)) ? null : next);
  };
  const toggleInclude = (k: ExportInclude, on: boolean) => setInclude((cur) => (on ? [...new Set([...cur, k])] : cur.filter((x) => x !== k)));
  const running = !!job && !isJobFinished(job);
  // `columns: []` means "every scalar field" to the exporter (export.rs), so an emptied selection blocks 실행.
  const noColumns = effectiveTarget === "fields" && options.length > 0 && columns !== null && columns.length === 0;
  const canRun = !!preview && !previewBusy && !running && preview.documents > 0 && !pathError && !noColumns;
  const selectedColumns = columns ?? options.map((o) => o.path);

  return (
    <div className="flex flex-col gap-4" data-testid="export-tab">
      <Card>
        <CardHeader>
          <CardTitle>{t("export.title")}</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <div className="flex flex-wrap items-end gap-3">
            <div className="flex flex-col gap-1">
              <Label htmlFor="export-target">{t("export.target.title")}</Label>
              <Select value={effectiveTarget} onValueChange={(v) => setTarget(v as ExportTarget)} disabled={format === "pdf_searchable"}>
                <SelectTrigger id="export-target" className="h-8 w-44" data-testid="export-target">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {TARGETS.map((x) => (
                    <SelectItem key={x} value={x}>
                      {t(`export.target.${x}`)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-1">
              <Label htmlFor="export-format">{t("export.format.title")}</Label>
              <Select value={format} onValueChange={(v) => setFormat(v as ExportFormat)}>
                <SelectTrigger id="export-format" className="h-8 w-44" data-testid="export-format">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {EXPORT_FORMATS.map((f) => (
                    <SelectItem key={f} value={f}>
                      {t(`export.format.${f}`)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            {format === "xlsx" && effectiveTarget === "fields" && (
              <div className="flex flex-col gap-1">
                <Label htmlFor="export-layout">{t("export.layout.title")}</Label>
                <Select value={layout} onValueChange={(v) => setLayout(v as ExportLayout)}>
                  <SelectTrigger id="export-layout" className="h-8 w-52" data-testid="export-layout">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {LAYOUTS.map((l) => (
                      <SelectItem key={l} value={l}>
                        {t(`export.layout.${l}`)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            )}
            <span className="pb-2 text-xs text-fg-subtle">{format === "pdf_searchable" ? t("export.pdf_hint") : t(`export.target_hint.${effectiveTarget}`)}</span>
          </div>

          <div className="flex flex-col gap-2">
            <h3 className="text-sm font-semibold">{t("export.scope_title")}</h3>
            <ScopeSection value={scope} onChange={setScope} selectedCount={selectedIds.length} testId="export-scope" />
            <label className="flex items-center gap-2 text-sm">
              <Switch checked={onlyReviewed} onCheckedChange={setOnlyReviewed} data-testid="export-only-reviewed" />
              {t("export.only_reviewed")}
            </label>
          </div>

          {effectiveTarget === "fields" && (
            <div className="flex flex-col gap-2" data-testid="export-columns">
              <h3 className="text-sm font-semibold">{t("export.columns.title")}</h3>
              {options.length === 0 ? (
                <p className="text-xs text-fg-subtle">{t("export.columns.hint")}</p>
              ) : (
                <>
                  <div className="flex flex-wrap items-center gap-2 text-xs text-fg-muted">
                    <span>{t("export.columns.selected", { selected: selectedColumns.length, total: options.length })}</span>
                    <Button size="sm" variant="ghost" onClick={() => setColumns(null)} disabled={columns === null}>
                      {t("export.columns.all")}
                    </Button>
                    <Button size="sm" variant="ghost" onClick={() => setColumns([])} disabled={columns !== null && columns.length === 0}>
                      {t("export.columns.none")}
                    </Button>
                  </div>
                  <div className="flex flex-wrap gap-x-4 gap-y-1">
                    {options.map((o) => (
                      <label key={o.path} className="flex items-center gap-1.5 text-sm">
                        <input type="checkbox" checked={selectedColumns.includes(o.path)} onChange={() => toggleColumn(o.path)} data-testid={`export-column-${o.path}`} />
                        {o.label}
                        <span className="font-mono text-xs text-fg-subtle">{o.path}</span>
                      </label>
                    ))}
                  </div>
                  {noColumns && (
                    <p className="text-xs text-danger" role="alert" data-testid="export-columns-none">
                      {t("export.columns.none_selected")}
                    </p>
                  )}
                </>
              )}
              <div className="flex flex-wrap items-center gap-4 text-sm">
                <span className="text-fg-muted">{t("export.columns.common")}</span>
                {INCLUDES.map((k) => (
                  <label key={k} className="flex items-center gap-2">
                    <Switch checked={include.includes(k)} onCheckedChange={(on) => toggleInclude(k, on)} data-testid={`export-include-${k}`} />
                    {t(`export.columns.${k}`)}
                  </label>
                ))}
              </div>
            </div>
          )}

          <div className="flex flex-col gap-2">
            <Label htmlFor="export-path">{t("export.path.title")}</Label>
            <div className="flex flex-wrap items-center gap-2">
              <Input
                id="export-path"
                value={path}
                onChange={(e) => {
                  pathDirty.current = true;
                  setPath(e.target.value);
                  setRunError(null);
                }}
                spellCheck={false}
                autoComplete="off"
                className="w-96 max-w-full font-mono text-xs"
                aria-invalid={pathError ? true : undefined}
                data-testid="export-path"
              />
              <Button
                size="sm"
                variant="ghost"
                onClick={() => {
                  pathDirty.current = false;
                  setPath(preview?.path ?? "");
                  setNonce((n) => n + 1);
                }}
              >
                {t("organize.name.reset")}
              </Button>
              {(showOverwrite || preview?.exists) && (
                <label className="flex items-center gap-2 text-sm">
                  <Switch checked={overwrite} onCheckedChange={setOverwrite} data-testid="export-overwrite" />
                  {t("export.path.overwrite")}
                </label>
              )}
            </div>
            <p className="text-xs text-fg-subtle">{t("export.path.hint")}</p>
            {pathError && (
              <p className="text-xs text-danger" role="alert" data-testid="export-path-error">
                {pathError}
              </p>
            )}
          </div>

          <div className="flex flex-wrap items-center gap-3 border-t border-border pt-3">
            <span className="text-sm" aria-live="polite" data-testid="export-preview-line">
              {previewBusy && !preview ? (
                t("export.preview_loading")
              ) : previewError ? (
                <span className="text-danger">{previewError}</span>
              ) : preview ? (
                <>
                  {t("export.preview_line", { documents: preview.documents, columns: preview.columns.length })}
                  {preview.sheets.length > 0 && ` · ${t("export.preview_sheets", { sheets: preview.sheets.join(", ") })}`}
                  {preview.skipped > 0 && ` · ${t("export.preview_skipped", { count: preview.skipped })}`}
                  {preview.exists && <span className="ml-2 text-warn">{t("export.path.exists")}</span>}
                </>
              ) : null}
            </span>
            {previewBusy && <Spinner />}
            <Button className="ml-auto" onClick={() => void run()} disabled={!canRun} data-testid="export-run">
              <FileOutput />
              {t("export.run")}
            </Button>
          </div>
          {runError && (
            <p className="text-sm text-danger" role="alert" data-testid="export-run-error">
              {runError}
            </p>
          )}
        </CardContent>
      </Card>

      {running && job && (
        <Card>
          <CardContent className="flex flex-col gap-2 pt-5" data-testid="export-progress">
            <div className="flex items-center gap-2 text-sm">
              <Spinner />
              <span>{t("export.running")}</span>
              {job.detail && <span className="text-xs text-fg-muted">{job.detail}</span>}
            </div>
            <Progress value={job.progress} label={t("export.running")} />
          </CardContent>
        </Card>
      )}

      {result && (
        <Card data-testid="export-result">
          <CardHeader>
            <CardTitle>{t("export.result_title")}</CardTitle>
            <CardDescription className="selectable font-mono text-xs" data-testid="export-result-path">
              {result.path}
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-2 text-sm">
            <div className="flex flex-wrap items-center gap-2">
              <Badge variant="ok">{t("export.result_rows", { documents: result.documents, rows: result.rows })}</Badge>
              {result.sheets.length > 0 && <span className="text-fg-muted">{t("export.result_sheets", { sheets: result.sheets.join(", ") })}</span>}
              {result.overwritten && <Badge variant="info">{t("export.result_overwritten")}</Badge>}
              {result.skipped.length > 0 && <Badge variant="warn">{t("export.result_skipped", { count: result.skipped.length })}</Badge>}
              <Button size="sm" variant="secondary" className="ml-auto" onClick={() => void revealPath(result.absolute || result.path)} data-testid="export-open-folder">
                <FolderOpen />
                {t("export.open_folder")}
              </Button>
            </div>
            {result.extra_files.length > 0 && (
              <div>
                <span className="text-fg-muted">{t("export.extra_files")}:</span>
                <ul className="mt-1 list-disc pl-5 font-mono text-xs" data-testid="export-extra-files">
                  {result.extra_files.map((f) => (
                    <li key={f} className="selectable">
                      {f}
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </CardContent>
        </Card>
      )}
    </div>
  );
}
