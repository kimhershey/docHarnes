import { HelpCircle } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";

/**
 * Clarification card (design §3.7.6 "명확화"): the planner's question, with
 * the composer refilled by the parent. When no planner model was available
 * (`source === "none"`, issue `no_model`) it offers to load the model.
 */
export function ClarifyCard({
  question,
  confidence,
  noModel,
  modelId,
  loading,
  onLoadModel,
  onRephrase,
}: {
  question: string;
  confidence: number;
  noModel: boolean;
  modelId: string | null;
  loading: boolean;
  onLoadModel: () => void;
  onRephrase: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex w-full flex-col gap-2 rounded-lg border border-border bg-bg-elevated p-3 text-sm" role="group" aria-label={t("planner.clarify.title")} data-testid="assistant-card-clarify">
      <div className="flex flex-wrap items-center gap-2">
        <HelpCircle className="size-4 text-info" aria-hidden="true" />
        <Badge variant="info">{t("planner.clarify.title")}</Badge>
        <span className="text-xs text-fg-subtle">{t("assistant.confidence", { value: confidence.toFixed(2) })}</span>
      </div>
      <p className="selectable" data-testid="clarify-question">
        {question}
      </p>
      {noModel && (
        <p className="rounded-md bg-warn-soft px-2 py-1.5 text-xs text-warn" data-testid="clarify-no-model">
          {t("planner.clarify.no_model")}
        </p>
      )}
      <div className="flex flex-wrap items-center gap-2">
        {noModel && (
          <Button size="sm" variant="secondary" disabled={loading} onClick={onLoadModel} data-testid="clarify-load-model">
            {loading ? t("planner.clarify.loading") : modelId ? t("planner.clarify.load_model", { model: modelId }) : t("planner.clarify.open_models")}
          </Button>
        )}
        <Button size="sm" variant="ghost" onClick={onRephrase} data-testid="clarify-rephrase">
          {t("planner.card.rephrase")}
        </Button>
        <span className="text-xs text-fg-subtle">{t("planner.clarify.retry_hint")}</span>
      </div>
    </div>
  );
}
