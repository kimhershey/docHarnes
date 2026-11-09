import { CornerDownLeft, HelpCircle, X } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { ClarifyStatus } from "@/stores/assistant";

/**
 * Clarification card (design §3.7.6 "명확화"): the planner's question with a reply box. 되묻기 sends
 * the reply together with the original command and the question (the model continues from where it
 * asked); 취소 drops the question. Both are handled by rules, never by the model (user decision
 * 2026-11-09). When no planner model was available (`source === "none"`, issue `no_model`) it offers
 * to load the model.
 */
export function ClarifyCard({
  question,
  confidence,
  status,
  noModel,
  modelId,
  loading,
  onLoadModel,
  onRephrase,
  onAskBack,
  onCancel,
}: {
  question: string;
  confidence: number;
  status: ClarifyStatus;
  noModel: boolean;
  modelId: string | null;
  loading: boolean;
  onLoadModel: () => void;
  onRephrase: () => void;
  onAskBack: (reply: string) => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const [reply, setReply] = useState("");
  const pending = status === "pending";
  const submit = () => {
    const r = reply.trim();
    if (!r) return;
    onAskBack(r);
    setReply("");
  };
  return (
    <div className="flex w-full flex-col gap-2 rounded-lg border border-border bg-bg-elevated p-3 text-sm" role="group" aria-label={t("planner.clarify.title")} data-testid="assistant-card-clarify" data-clarify-status={status}>
      <div className="flex flex-wrap items-center gap-2">
        <HelpCircle className="size-4 text-info" aria-hidden="true" />
        <Badge variant="info">{t("planner.clarify.title")}</Badge>
        <span className="text-xs text-fg-subtle">{t("assistant.confidence", { value: confidence.toFixed(2) })}</span>
        {status === "answered" && <Badge variant="ok">{t("planner.clarify.answered")}</Badge>}
        {status === "cancelled" && <Badge variant="muted">{t("planner.clarify.cancelled")}</Badge>}
      </div>
      <p className="selectable" data-testid="clarify-question">
        {question}
      </p>
      {noModel && (
        <p className="rounded-md bg-warn-soft px-2 py-1.5 text-xs text-warn" data-testid="clarify-no-model">
          {t("planner.clarify.no_model")}
        </p>
      )}
      {pending && (
        <form
          className="flex flex-wrap items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
        >
          <Input
            value={reply}
            onChange={(e) => setReply(e.target.value)}
            placeholder={t("planner.clarify.reply_placeholder")}
            aria-label={t("planner.clarify.ask_back")}
            className="h-8 min-w-56 flex-1"
            data-testid="clarify-reply"
          />
          <Button type="submit" size="sm" disabled={!reply.trim()} data-testid="clarify-ask-back">
            <CornerDownLeft />
            {t("planner.clarify.ask_back")}
          </Button>
          <Button type="button" size="sm" variant="ghost" onClick={onCancel} data-testid="clarify-cancel">
            <X />
            {t("planner.clarify.cancel")}
          </Button>
          {noModel && (
            <Button type="button" size="sm" variant="secondary" disabled={loading} onClick={onLoadModel} data-testid="clarify-load-model">
              {loading ? t("planner.clarify.loading") : modelId ? t("planner.clarify.load_model", { model: modelId }) : t("planner.clarify.open_models")}
            </Button>
          )}
          <Button type="button" size="sm" variant="ghost" onClick={onRephrase} data-testid="clarify-rephrase">
            {t("planner.card.rephrase")}
          </Button>
        </form>
      )}
      {pending && <span className="text-xs text-fg-subtle">{t("planner.clarify.retry_hint")}</span>}
    </div>
  );
}
