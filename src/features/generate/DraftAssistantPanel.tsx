import { Sparkles } from "lucide-react";
import { forwardRef } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Input, Textarea } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { useGenerateBudgetInfo } from "@/lib/queries";
import type { Draft } from "@/lib/types";
import { cn } from "@/lib/utils";

import { CharLimit } from "./CharLimit";
import { NO_SCOPE, sectionsOf } from "./draft";

/** One finished revision: what was asked, where, and the note the draft recorded. */
export interface ReviseLogEntry {
  id: number;
  instruction: string;
  scope: string;
  note: string | null;
}

/**
 * 초안 어시스턴트 (0.7.0 plan §1.5) — in the request column once a draft exists, below the form (`placement`
 * "bottom") or, in the screen's narrow layout, above it ("top", UI-5): a scope (the whole draft, one section to
 * rewrite, a new section after one or at the end), an instruction and the run button.
 * The revision is the ordinary draft job with `DraftRequest.revise`; the page follows it like a generation
 * and appends the outcome to the log. The ref is the instruction box, focused by the editor's per-section
 * "AI 수정" buttons.
 */
export const DraftAssistantPanel = forwardRef<
  HTMLTextAreaElement,
  {
    draft: Draft;
    placement?: "top" | "bottom";
    /** Scope picker value (`reviseScopeToPicker`). */
    scope: string;
    onScope: (value: string) => void;
    heading: string;
    onHeading: (value: string) => void;
    instruction: string;
    onInstruction: (value: string) => void;
    onRun: () => void;
    running: boolean;
    excludedCount: number;
    log: ReviseLogEntry[];
  }
>(function DraftAssistantPanel({ draft, placement = "bottom", scope, onScope, heading, onHeading, instruction, onInstruction, onRun, running, excludedCount, log }, ref) {
  const { t, i18n } = useTranslation();
  const { data: budgetInfo } = useGenerateBudgetInfo();
  const sections = sectionsOf(draft.body);
  const untitled = t("generate.revise.untitled");
  // No scope (the picked section was removed by an edit, GEN-10): nothing runs until one is picked again.
  const noScope = scope === NO_SCOPE;
  const canRun = instruction.trim().length > 0 && !running && !noScope;
  const isInsert = scope.startsWith("insert_after:");
  return (
    <section className={cn("flex flex-col gap-2 border-border", placement === "top" ? "border-b pb-3" : "border-t pt-3")} aria-labelledby="gen-revise-title" data-testid="generate-revise-panel" data-placement={placement}>
      <h2 id="gen-revise-title" className="flex items-center gap-1.5 text-sm font-semibold">
        <Sparkles className="size-4 text-accent" aria-hidden="true" />
        {t("generate.revise.title")}
      </h2>
      <p className="text-xs text-fg-subtle">{t("generate.revise.hint")}</p>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="gen-revise-scope">{t("generate.revise.scope")}</Label>
        <Select value={scope} onValueChange={onScope}>
          <SelectTrigger id="gen-revise-scope" className={cn("h-8", noScope && "border-warn")} aria-label={t("generate.revise.scope")} data-testid="generate-revise-scope" data-scope={noScope ? "none" : scope}>
            <SelectValue placeholder={t("generate.revise.scope_none")} />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="whole" data-testid="generate-revise-scope-whole">
              {t("generate.revise.scope_whole")}
            </SelectItem>
            {sections.map((s) => (
              <SelectItem key={`section:${s.ordinal}`} value={`section:${s.ordinal}`} data-testid={`generate-revise-scope-section-${s.ordinal}`}>
                {t("generate.revise.scope_section", { index: s.ordinal, heading: s.heading ?? untitled })}
              </SelectItem>
            ))}
            {sections.map((s) => (
              <SelectItem key={`insert_after:${s.ordinal}`} value={`insert_after:${s.ordinal}`} data-testid={`generate-revise-scope-insert-${s.ordinal}`}>
                {t("generate.revise.scope_insert_after", { index: s.ordinal, heading: s.heading ?? untitled })}
              </SelectItem>
            ))}
            <SelectItem value="insert_after:end" data-testid="generate-revise-scope-append">
              {t("generate.revise.scope_append")}
            </SelectItem>
          </SelectContent>
        </Select>
        {noScope && (
          <p className="text-xs text-warn" role="status" data-testid="generate-revise-scope-lost">
            {t("generate.revise.scope_lost")}
          </p>
        )}
      </div>
      {isInsert && (
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="gen-revise-heading">{t("generate.revise.heading")}</Label>
          <Input id="gen-revise-heading" className="h-8" value={heading} onChange={(e) => onHeading(e.target.value)} placeholder={t("generate.revise.heading_placeholder")} data-testid="generate-revise-heading" />
        </div>
      )}
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="gen-revise-instruction">{t("generate.revise.instruction")}</Label>
        <Textarea
          ref={ref}
          id="gen-revise-instruction"
          rows={3}
          value={instruction}
          onChange={(e) => onInstruction(e.target.value)}
          onKeyDown={(e) => {
            // Ctrl/Cmd+Enter runs the revision (Enter alone keeps writing).
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && canRun) {
              e.preventDefault();
              onRun();
            }
          }}
          placeholder={t("generate.revise.instruction_placeholder")}
          lang={i18n.language}
          data-testid="generate-revise-instruction"
        />
        <CharLimit text={instruction} max={budgetInfo?.instruction_max_chars} testId="generate-revise-limit" />
      </div>
      {excludedCount > 0 && <p className="text-xs text-fg-muted">{t("generate.form.excluded_hint", { count: excludedCount })}</p>}
      <Button variant="secondary" onClick={onRun} disabled={!canRun} data-testid="generate-revise-run">
        {running ? <Spinner /> : <Sparkles />}
        {running ? t("generate.revise.running") : t("generate.revise.run")}
      </Button>
      <div className="flex flex-col gap-1" data-testid="generate-revise-log">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-fg-subtle">{t("generate.revise.log")}</h3>
        {log.length === 0 ? (
          <p className="text-xs text-fg-subtle">{t("generate.revise.log_empty")}</p>
        ) : (
          <ol className="flex flex-col gap-1 text-xs">
            {log
              .slice(-5)
              .reverse()
              .map((e) => (
                <li key={e.id} className="rounded-md border border-border px-2 py-1" data-testid="generate-revise-log-entry">
                  <div className="font-medium selectable">{e.instruction}</div>
                  <div className="text-fg-subtle">{t("generate.revise.done", { scope: e.scope })}</div>
                  {e.note && <div className="text-fg-muted selectable">{e.note}</div>}
                </li>
              ))}
          </ol>
        )}
      </div>
    </section>
  );
});
