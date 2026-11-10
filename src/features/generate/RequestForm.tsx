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
import { useDocuments, useGenerateBudgetInfo, useGenerateTemplates, useSchemas } from "@/lib/queries";
import { DRAFT_LENGTHS, LENGTH_TOKENS, OUTPUT_FORMATS, TOKEN_BUDGET_PRESETS, TONES, type DraftLength, type DraftRequest, type Job, type OutputFormat, type Tone } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

import { documentSourceIds, formRefToPicker, pickerToFormRef, withDocumentSources } from "./draft";
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
  hasDraft,
  feedback,
  onFeedback,
  onRegenerate,
  excludedCount,
  onCancel,
}: {
  request: DraftRequest;
  onChange: (patch: Partial<DraftRequest>) => void;
  onGenerate: () => void;
  job: Job | null | undefined;
  running: boolean;
  hasDraft: boolean;
  feedback: string;
  onFeedback: (text: string) => void;
  onRegenerate: () => void;
  excludedCount: number;
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
  // Token budget (0.3.0): the length preset or an explicit number, capped by the active backend's limit.
  const { data: budgetInfo } = useGenerateBudgetInfo();
  const cap = budgetInfo?.cap ?? 32_768;
  const splitThreshold = budgetInfo?.split_threshold ?? 3_000;
  const presetTokens = LENGTH_TOKENS[request.length];
  const effectiveTokens = Math.min(request.max_tokens ?? presetTokens, cap);
  const fmt = (n: number) => n.toLocaleString(ko ? "ko-KR" : "en-US");
  const tokPerSec = budgetInfo && !budgetInfo.gpu ? 6 : 25;
  const minutes = Math.max(1, Math.round(effectiveTokens / tokPerSec / 60));
  const budgetPicker = request.max_tokens == null ? "auto" : String(request.max_tokens);
  const budgetIsPreset = request.max_tokens == null || (TOKEN_BUDGET_PRESETS as readonly number[]).includes(request.max_tokens);
  const setBudget = (value: number | null) => onChange({ max_tokens: value === null ? null : Math.min(cap, Math.max(200, Math.round(value))) });

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
                  <span className="max-w-40 truncate">{doc?.title ?? doc?.original_name ?? id.slice(0, 8)}</span>
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
      <div className="grid grid-cols-3 gap-2">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="gen-tone">{t("generate.form.tone")}</Label>
          <Select value={request.tone} onValueChange={(v) => onChange({ tone: v as Tone })}>
            <SelectTrigger id="gen-tone" className="h-8" aria-label={t("generate.form.tone")} data-testid="generate-tone">
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
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="gen-length">{t("generate.form.length")}</Label>
          <Select value={request.length} onValueChange={(v) => onChange({ length: v as DraftLength })}>
            <SelectTrigger id="gen-length" className="h-8" aria-label={t("generate.form.length")} data-testid="generate-length">
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
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="gen-output">{t("generate.form.output")}</Label>
          <Select value={request.output} onValueChange={(v) => onChange({ output: v as OutputFormat })}>
            <SelectTrigger id="gen-output" className="h-8" aria-label={t("generate.form.output")} data-testid="generate-output">
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
        <div className="flex items-center justify-between gap-2">
          <Label htmlFor="gen-budget">{t("generate.form.token_budget")}</Label>
          <span className="text-xs text-fg-subtle">{budgetInfo?.ctx ? t("generate.form.budget_info", { ctx: fmt(budgetInfo.ctx), cap: fmt(cap) }) : t("generate.form.budget_info_cap", { cap: fmt(cap) })}</span>
        </div>
        <div className="flex items-center gap-2">
          <Select value={budgetPicker} onValueChange={(v) => setBudget(v === "auto" ? null : Number(v))}>
            <SelectTrigger id="gen-budget" className="h-8 flex-1" aria-label={t("generate.form.token_budget")} data-testid="generate-token-budget-select">
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
            className="h-8 w-28 text-right tabular-nums"
            min={200}
            max={cap}
            step={100}
            value={request.max_tokens ?? ""}
            placeholder={String(presetTokens)}
            onChange={(e) => setBudget(e.target.value === "" ? null : Number(e.target.value))}
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
      {hasDraft && (
        <div className="flex flex-col gap-1.5 border-t border-border pt-3" data-testid="generate-regenerate-panel">
          <Label htmlFor="gen-feedback">{t("generate.form.feedback")}</Label>
          <Textarea id="gen-feedback" rows={2} value={feedback} onChange={(e) => onFeedback(e.target.value)} placeholder={t("generate.form.feedback_placeholder")} lang={i18n.language} data-testid="generate-feedback" />
          {excludedCount > 0 && <p className="text-xs text-fg-muted">{t("generate.form.excluded_hint", { count: excludedCount })}</p>}
          <Button variant="secondary" onClick={onRegenerate} disabled={running} data-testid="generate-regenerate">
            {t("generate.form.regenerate")}
          </Button>
        </div>
      )}
      <SourcePickerDialog open={pickerOpen} onOpenChange={setPickerOpen} selected={sourceIds} onChange={(ids) => onChange({ sources: withDocumentSources(request.sources, ids) })} />
    </div>
  );
}
