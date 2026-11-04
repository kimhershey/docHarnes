import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Spinner } from "@/components/ui/spinner";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useAnswers, useKnowledgeStatus } from "@/lib/queries";
import type { Citation } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

import { AnswerCard } from "./AnswerCard";

export function openCitation(navigate: (to: string) => void, c: Citation): void {
  const params = new URLSearchParams({ page: String(c.page_start), chunk: String(c.chunk_id) });
  navigate(`/library/${c.document_id}?${params.toString()}`);
}

export function AnswersPanel() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const { data: answers, isLoading } = useAnswers(null, null);
  const { data: status } = useKnowledgeStatus();

  const feedback = async (id: number, value: number | null) => {
    try {
      await api.knowledgeAnswerFeedback(id, value);
      toast(t("answer.feedback_saved"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const remove = async (id: number) => {
    try {
      await api.knowledgeAnswerDelete(id);
      toast(t("answer.deleted"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  return (
    <div className="flex flex-col gap-2" data-testid="knowledge-answers">
      <div className="flex items-center gap-2">
        <h2 className="text-sm font-semibold">{t("knowledge.answers.title")}</h2>
        {status && <span className="text-xs text-fg-subtle">{t("knowledge.answers.stats", { helpful: status.answers_helpful, wrong: status.answers_wrong })}</span>}
      </div>
      {isLoading && <Spinner />}
      {answers && answers.length === 0 && <p className="text-sm text-fg-muted">{t("knowledge.answers.empty")}</p>}
      {answers?.map((a) => (
        <AnswerCard key={a.id} answer={a} onCitation={(c) => openCitation(navigate, c)} onFeedback={(id, v) => void feedback(id, v)} onDelete={(id) => void remove(id)} compact />
      ))}
    </div>
  );
}
