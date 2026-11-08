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
import { useDocuments, useGenerateTemplates, useSchemas } from "@/lib/queries";
import { DRAFT_LENGTHS, OUTPUT_FORMATS, TONES, type DraftLength, type DraftRequest, type Job, type OutputFormat, type Tone } from "@/lib/types";

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
  const { data: sourceDocs } = useDocuments({ limit: 300, sort: "imported_desc" }, sourceIds.length > 0);
  const [pickerOpen, setPickerOpen] = useState(false);
  const forms = (schemas ?? []).filter((s) => s.source === "form" && s.enabled);
  const picker = formRefToPicker(request.form_ref, templates ?? [], schemas ?? []);
  const ko = i18n.language.startsWith("ko");
  const detail = job ? describeJobDetail(job, t) : null;
  const canGenerate = request.content.trim().length > 0 && !running;

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
      <div className="flex flex-col gap-1.5">
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
