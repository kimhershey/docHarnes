import { FileText, Sparkles, X } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Input, Textarea } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { describeJobDetail } from "@/features/jobs/describe";
import { DropHint, importDroppedFiles, useDocumentDropTarget } from "@/lib/dnd";
import { useDocuments, useGenerateBudgetInfo, useGenerateTemplates, useProjects, useSchemas } from "@/lib/queries";
import { isComposing } from "@/lib/shortcuts";
import { DRAFT_LENGTHS, LENGTH_TOKENS, OUTPUT_FORMATS, TOKEN_BUDGET_PRESETS, TONES, type DraftLength, type DraftRequest, type Job, type OutputFormat, type Tone } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

import { CharLimit } from "./CharLimit";
import { clampTokenBudget, documentSourceIds, formRefToPicker, MIN_TOKEN_BUDGET, parseTokenBudget, pickerToFormRef, withDocumentSources } from "./draft";
import { SourcePickerDialog } from "./SourcePickerDialog";

/**
 * Left column of 문서 생성 (design §3.13): template / registered form picker, free-form layout description,
 * content, quoted documents, knowledge grounding toggle, tone / length / output and the generate button
 * with the draft job's progress (live region).
 */
export function RequestForm({
  request,
  onChange,
  onGenerate,
  job,
  running,
  onCancel,
}: {
  request: DraftRequest;
  onChange: (patch: Partial<DraftRequest>) => void;
  onGenerate: () => void;
  job: Job | null | undefined;
  running: boolean;
  onCancel: () => void;
}) {
  const { t, i18n } = useTranslation();
  const { data: templates } = useGenerateTemplates();
  const { data: schemas } = useSchemas();
  const sourceIds = documentSourceIds(request.sources);
  // Names of the quoted documents: the list is only a lookup table, so a generous cap instead of the grid's page size.
  const { data: sourceDocs } = useDocuments({ limit: 2000, sort: "imported_desc" }, sourceIds.length > 0);
  const [pickerOpen, setPickerOpen] = useState(false);
  // Documents / files dropped on the sources box are quoted by the draft.
  const addSources = (ids: string[]) => {
    const next = [...sourceIds, ...ids.filter((id) => !sourceIds.includes(id))];
    if (next.length === sourceIds.length) return;
    onChange({ sources: withDocumentSources(request.sources, next) });
    useUiStore.getState().toast(t("generate.form.sources_added", { count: next.length - sourceIds.length }), "ok");
  };
  const drop = useDocumentDropTarget({
    disabled: running,
    onDocuments: (docs) => addSources(docs.map((d) => d.id)),
    onFiles: (paths) => void importDroppedFiles(paths, {}, t).then(addSources),
  });
  const forms = (schemas ?? []).filter((s) => s.source === "form" && s.enabled);
  const picker = formRefToPicker(request.form_ref, templates ?? [], schemas ?? []);
  const ko = i18n.language.startsWith("ko");
  const detail = job ? describeJobDetail(job, t) : null;
  const canGenerate = request.content.trim().length > 0 && !running;
  // The project a project draft belongs to (its name for the scope notice).
  const { data: projects } = useProjects(!!request.project_id);
  const scopeProject = request.project_id ? (projects?.find((p) => p.id === request.project_id) ?? null) : null;
  // Token budget (0.3.0): the length preset or an explicit number, capped by the active backend's limit.
  const { data: budgetInfo } = useGenerateBudgetInfo();
  const cap = budgetInfo?.cap ?? 32_768;
  const splitThreshold = budgetInfo?.split_threshold ?? 3_000;
  const presetTokens = LENGTH_TOKENS[request.length];
  // The custom number is typed freely and applied — rounded, clamped to 200…cap — when the box loses focus or
  // on Enter (GEN-12); `budgetText` is what is being typed (null = the box shows the stored budget).
  const [budgetText, setBudgetText] = useState<string | null>(null);
  const typedBudget = budgetText === null ? undefined : parseTokenBudget(budgetText, cap);
  // The hints below follow the number being typed, as it will be applied.
  const pendingBudget = typedBudget === undefined ? (request.max_tokens ?? null) : typedBudget;
  const effectiveTokens = Math.min(pendingBudget ?? presetTokens, cap);
  const fmt = (n: number) => n.toLocaleString(ko ? "ko-KR" : "en-US");
  const tokPerSec = budgetInfo && !budgetInfo.gpu ? 6 : 25;
  const minutes = Math.max(1, Math.round(effectiveTokens / tokPerSec / 60));
  const budgetPicker = request.max_tokens == null ? "auto" : String(request.max_tokens);
  const budgetIsPreset = request.max_tokens == null || (TOKEN_BUDGET_PRESETS as readonly number[]).includes(request.max_tokens);
  const setBudget = (value: number | null) => onChange({ max_tokens: value === null ? null : clampTokenBudget(value, cap) });
  const commitBudget = () => {
    if (budgetText === null) return;
    setBudgetText(null);
    if (typedBudget !== undefined && typedBudget !== (request.max_tokens ?? null)) onChange({ max_tokens: typedBudget });
  };

  return (
    <div className="flex flex-col gap-3" data-testid="generate-form">
      <h2 className="text-sm font-semibold">{t("generate.form.title")}</h2>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="gen-template">{t("generate.form.template")}</Label>
        <Select value={picker} onValueChange={(v) => onChange({ form_ref: pickerToFormRef(v) })}>
          <SelectTrigger id="gen-template" aria-label={t("generate.form.template")} data-testid="generate-template">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="auto">{t("generate.form.template_auto")}</SelectItem>
            {(templates ?? []).map((tpl) => (
              <SelectItem key={tpl.kind} value={`tpl:${tpl.kind}`} data-testid={`generate-template-${tpl.kind}`}>
                {ko ? tpl.name_ko : tpl.name_en}
              </SelectItem>
            ))}
            {forms.map((s) => (
              <SelectItem key={s.id} value={`form:${s.id}`} data-testid={`generate-form-${s.id}`}>
                {t("generate.form.registered", { name: s.name })}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="gen-format">{t("generate.form.format_description")}</Label>
        <Input id="gen-format" value={request.format_description ?? ""} onChange={(e) => onChange({ format_description: e.target.value || null })} placeholder={t("generate.form.format_placeholder")} data-testid="generate-format" />
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="gen-content">
          {t("generate.form.content")} <span className="text-danger">*</span>
        </Label>
        <Textarea id="gen-content" rows={5} value={request.content} onChange={(e) => onChange({ content: e.target.value })} placeholder={t("generate.form.content_placeholder")} lang={i18n.language} data-testid="generate-content" />
        <CharLimit text={request.content} max={budgetInfo?.content_max_chars} testId="generate-content-limit" />
      </div>
      <div className="relative flex flex-col gap-1.5" {...drop.props} data-testid="generate-sources-drop">
        <DropHint active={drop.isActive} over={drop.isOver} testId="generate-drop-hint">
          {t("generate.form.drop_hint")}
        </DropHint>
        <div className="flex items-center justify-between">
          <Label>{t("generate.form.sources")}</Label>
          <Button size="sm" variant="outline" onClick={() => setPickerOpen(true)} data-testid="generate-pick-sources">
            <FileText />
            {t("generate.form.pick_sources")}
          </Button>
        </div>
        {sourceIds.length === 0 ? (
          <p className="text-xs text-fg-subtle">{t("generate.form.sources_hint")}</p>
        ) : (
          <ul className="flex flex-wrap gap-1" data-testid="generate-sources">
            {sourceIds.map((id) => {
              const doc = sourceDocs?.find((d) => d.id === id);
              return (
                <li key={id} className="flex items-center gap-1 rounded-full bg-bg-sunken px-2 py-0.5 text-xs">
                  <span className="max-w-40 truncate" title={doc?.original_name}>
                    {doc?.title ?? doc?.original_name ?? id.slice(0, 8)}
                  </span>
                  <button type="button" className="text-fg-muted hover:text-fg cursor-pointer" onClick={() => onChange({ sources: withDocumentSources(request.sources, sourceIds.filter((x) => x !== id)) })} aria-label={t("generate.form.remove_source")}>
                    <X className="size-3" />
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>
      <label className="flex items-center justify-between gap-2 text-sm">
        <span>{t("generate.form.use_knowledge")}</span>
        <Switch checked={request.use_knowledge} onCheckedChange={(v) => onChange({ use_knowledge: v })} aria-label={t("generate.form.use_knowledge")} data-testid="generate-use-knowledge" />
      </label>
      {/* A project draft (CON-3): its reference material is searched inside the project. Said here, with a way out. */}
      {request.project_id && (
        <div className="flex items-start gap-2 rounded-md border border-border bg-bg-sunken px-2 py-1.5 text-xs text-fg-muted" data-testid="generate-project-scope" data-project-id={request.project_id}>
          <span className="min-w-0 flex-1">{scopeProject ? t("generate.form.project_scope", { name: scopeProject.name }) : projects ? t("generate.form.project_scope_missing") : t("generate.form.project_scope", { name: "…" })}</span>
          <Button size="sm" variant="ghost" className="h-6 shrink-0 px-2 text-xs" onClick={() => onChange({ project_id: null })} data-testid="generate-project-scope-clear">
            {t("generate.form.project_scope_clear")}
          </Button>
        </div>
      )}
      {/* 어조 and 길이 share a row, 출력 형식 has its own: three selects side by side left room for one letter each
          in this 16–20 rem column ("격…", "H…", UI-5). */}
      <div className="grid grid-cols-2 gap-2" data-testid="generate-options">
        <div className="flex min-w-0 flex-col gap-1.5">
          <Label htmlFor="gen-tone">{t("generate.form.tone")}</Label>
          <Select value={request.tone} onValueChange={(v) => onChange({ tone: v as Tone })}>
            <SelectTrigger id="gen-tone" className="h-8 px-2" aria-label={t("generate.form.tone")} data-testid="generate-tone">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {TONES.map((x) => (
                <SelectItem key={x} value={x}>
                  {t(`generate.tone.${x}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="flex min-w-0 flex-col gap-1.5">
          <Label htmlFor="gen-length">{t("generate.form.length")}</Label>
          <Select value={request.length} onValueChange={(v) => onChange({ length: v as DraftLength })}>
            <SelectTrigger id="gen-length" className="h-8 px-2" aria-label={t("generate.form.length")} data-testid="generate-length">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {DRAFT_LENGTHS.map((x) => (
                <SelectItem key={x} value={x}>
                  {t(`generate.length.${x}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="col-span-2 flex min-w-0 flex-col gap-1.5">
          <Label htmlFor="gen-output">{t("generate.form.output")}</Label>
          <Select value={request.output} onValueChange={(v) => onChange({ output: v as OutputFormat })}>
            <SelectTrigger id="gen-output" className="h-8 px-2" aria-label={t("generate.form.output")} data-testid="generate-output">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {OUTPUT_FORMATS.map((x) => (
                <SelectItem key={x} value={x}>
                  {t(`generate.output.${x}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>
      <div className="flex flex-col gap-1.5" data-testid="generate-token-budget">
        {/* The label never breaks; the context / cap note drops to its own line when the column is narrow. */}
        <div className="flex flex-wrap items-baseline justify-between gap-x-2 gap-y-0.5">
          <Label htmlFor="gen-budget" className="shrink-0 whitespace-nowrap">
            {t("generate.form.token_budget")}
          </Label>
          <span className="text-xs text-fg-subtle">{budgetInfo?.ctx ? t("generate.form.budget_info", { ctx: fmt(budgetInfo.ctx), cap: fmt(cap) }) : t("generate.form.budget_info_cap", { cap: fmt(cap) })}</span>
        </div>
        <div className="flex items-center gap-2">
          <Select value={budgetPicker} onValueChange={(v) => setBudget(v === "auto" ? null : Number(v))}>
            <SelectTrigger id="gen-budget" className="h-8 min-w-0 flex-1 px-2" aria-label={t("generate.form.token_budget")} data-testid="generate-token-budget-select">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="auto" data-testid="generate-token-budget-auto">
                {t("generate.form.token_presets.auto", { tokens: fmt(presetTokens) })}
              </SelectItem>
              {TOKEN_BUDGET_PRESETS.filter((n) => n <= cap).map((n) => (
                <SelectItem key={n} value={String(n)} data-testid={`generate-token-budget-${n}`}>
                  {t(`generate.form.token_presets.p${n}`, { tokens: fmt(n) })}
                </SelectItem>
              ))}
              {!budgetIsPreset && request.max_tokens != null && <SelectItem value={String(request.max_tokens)}>{t("generate.form.token_presets.custom", { tokens: fmt(request.max_tokens) })}</SelectItem>}
            </SelectContent>
          </Select>
          <Input
            type="number"
            className="h-8 w-24 shrink-0 px-2 text-right tabular-nums"
            min={MIN_TOKEN_BUDGET}
            max={cap}
            step={100}
            value={budgetText ?? request.max_tokens ?? ""}
            placeholder={String(presetTokens)}
            onChange={(e) => setBudgetText(e.target.value)}
            onBlur={commitBudget}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !isComposing(e)) {
                e.preventDefault();
                commitBudget();
              }
            }}
            aria-label={t("generate.form.token_budget_custom")}
            data-testid="generate-token-budget-input"
          />
        </div>
        <p className="text-xs text-fg-subtle" data-testid="generate-token-budget-hint">
          {effectiveTokens > splitThreshold ? t("generate.form.budget_split", { tokens: fmt(effectiveTokens), minutes }) : t("generate.form.budget_single", { tokens: fmt(effectiveTokens), minutes })}
        </p>
        {budgetInfo && !budgetInfo.gpu && effectiveTokens >= 6_000 && (
          <p className="text-xs text-warn" data-testid="generate-token-budget-warning">
            {t("generate.form.cpu_warning", { cap: fmt(cap) })}
          </p>
        )}
      </div>
      <div className="flex items-center gap-2">
        <Button onClick={onGenerate} disabled={!canGenerate} data-testid="generate-run">
          <Sparkles />
          {t("generate.form.generate")}
        </Button>
        {running && (
          <Button variant="ghost" size="sm" onClick={onCancel} disabled={job?.cancel_requested} data-testid="generate-cancel">
            {t("generate.form.cancel")}
          </Button>
        )}
      </div>
      {running && job && (
        <div className="flex flex-col gap-1" role="status" aria-live="polite" aria-atomic="true" data-testid="generate-progress">
          <div className="flex items-center gap-2 text-xs text-fg-muted">
            <Spinner />
            <span>{detail ?? t("generate.form.generating")}</span>
          </div>
          <Progress value={job.state === "queued" ? null : job.progress} label={t("generate.form.generating")} />
        </div>
      )}
      <SourcePickerDialog open={pickerOpen} onOpenChange={setPickerOpen} selected={sourceIds} onChange={(ids) => onChange({ sources: withDocumentSources(request.sources, ids) })} />
    </div>
  );
}
