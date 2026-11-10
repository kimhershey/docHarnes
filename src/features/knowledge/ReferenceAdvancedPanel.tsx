import { BookOpen, ChevronDown, ChevronRight, RefreshCw } from "lucide-react";
import { useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDialog, type ConfirmRequest } from "@/components/ui/confirm-dialog";
import { Spinner } from "@/components/ui/spinner";
import { describeError } from "@/lib/errors";
import { api, revealPath } from "@/lib/ipc";
import { useKnowledgeStatus } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import { COMMON_REFERENCE_ID, SCREEN_ROUTES, type KnowledgeStatus } from "@/lib/types";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";

import { AnswersPanel } from "./AnswersPanel";
import { GlossaryPanel } from "./GlossaryPanel";
import { QuestionTestPanel } from "./QuestionTestPanel";
import { VersionsPanel } from "./VersionsPanel";

const SECTIONS = ["index", "embedding", "tokenizer", "versions", "glossary", "test", "answers"] as const;
type SectionKey = (typeof SECTIONS)[number];

function Stat({ label, value, testId, tone }: { label: string; value: number | string; testId: string; tone?: "warn" | "danger" }) {
  return (
    <div className="rounded-md border border-border bg-bg p-2">
      <div className="text-xs text-fg-subtle">{label}</div>
      <div className={`text-lg font-semibold tabular-nums ${tone === "warn" ? "text-warn" : tone === "danger" ? "text-danger" : ""}`} data-testid={testId}>
        {value}
      </div>
    </div>
  );
}

function Section({ id, title, open, onToggle, extra, children }: { id: SectionKey; title: string; open: boolean; onToggle: () => void; extra?: ReactNode; children: ReactNode }) {
  return (
    <section className="rounded-lg border border-border bg-bg-elevated" data-testid={`reference-section-${id}`} data-open={open}>
      <h3 className="m-0">
        <button type="button" className="flex w-full items-center gap-2 px-4 py-3 text-left text-sm font-semibold cursor-pointer" onClick={onToggle} aria-expanded={open} aria-controls={`reference-section-body-${id}`} data-testid={`reference-section-toggle-${id}`}>
          {open ? <ChevronDown className="size-4 shrink-0" aria-hidden="true" /> : <ChevronRight className="size-4 shrink-0" aria-hidden="true" />}
          <span className="min-w-0 flex-1">{title}</span>
          {extra}
        </button>
      </h3>
      {open && (
        <div id={`reference-section-body-${id}`} className="border-t border-border px-4 py-3">
          {children}
        </div>
      )}
    </section>
  );
}

/**
 * 설정 > 참고 자료(고급) (0.5.0 plan §1/§3): what the old 지식 베이스 screen kept besides its collections — the
 * index statistics with 색인 갱신 / 전체 재색인, the embedding run / reset, the tokenizer, document versions, the
 * glossary, the question test and the answer log — as collapsible sections. Groups and documents are managed on
 * the projects screen (공통 참고 자료 and each project's 참고 자료 tab).
 */
export function ReferenceAdvancedPanel() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const workspace = useWorkspaceStore((s) => s.current);
  const { data: status, error } = useKnowledgeStatus(!!workspace);
  const [open, setOpen] = useState<Record<SectionKey, boolean>>({ index: true, embedding: false, tokenizer: false, versions: false, glossary: false, test: false, answers: false });
  const toggle = (k: SectionKey) => setOpen((o) => ({ ...o, [k]: !o[k] }));
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const run = async (key: string, fn: () => Promise<void>) => {
    setBusy(key);
    try {
      await fn();
      void queryClient.invalidateQueries({ queryKey: queryKeys.knowledgeStatus });
    } catch (e) {
      toast(describeError(e, t), "danger");
    } finally {
      setBusy(null);
    }
  };
  const index = (force: boolean) =>
    run(force ? "reindex" : "index", async () => {
      const job = await api.knowledgeIndex(null, force);
      toast(job ? t("knowledge.actions.index_started") : t("knowledge.actions.index_running"), "info");
    });
  const reindexAll = () =>
    setConfirm({
      title: t("knowledge.actions.reindex"),
      description: t("knowledge.actions.reindex_confirm"),
      confirmLabel: t("knowledge.actions.reindex"),
      danger: true,
      onConfirm: () => index(true),
    });
  const embed = () =>
    run("embed", async () => {
      const job = await api.knowledgeEmbed(true);
      toast(job ? t("knowledge.actions.embed_started") : t("knowledge.actions.embed_none"), "info");
    });
  const resetEmbeddings = () =>
    setConfirm({
      title: t("knowledge.embedding.reset"),
      description: t("knowledge.embedding.reset_confirm"),
      confirmLabel: t("knowledge.embedding.reset"),
      danger: true,
      onConfirm: () =>
        run("reset", async () => {
          await api.knowledgeResetEmbeddings();
          toast(t("knowledge.embedding.reset_done"), "ok");
        }),
    });
  const indexBadge = (s: KnowledgeStatus) => (
    <Badge variant={s.documents_failed > 0 ? "danger" : s.documents_pending > 0 ? "warn" : "muted"} data-testid="reference-index-badge">
      {t("knowledge.status.indexed")} {s.documents_indexed}/{s.documents_total}
    </Badge>
  );
  const emb = status?.embedding;
  const tokenizerSimple = status ? status.tokenizer.kind !== "ko-dic" : false;
  return (
    <div className="flex flex-col gap-3" data-testid="reference-advanced">
      <div className="flex flex-wrap items-center gap-2">
        <p className="min-w-0 flex-1 text-xs text-fg-muted">{t("knowledge.advanced.hint")}</p>
        <Button size="sm" variant="outline" onClick={() => navigate(`${SCREEN_ROUTES.projects}/${COMMON_REFERENCE_ID}`)} data-testid="reference-open-common">
          <BookOpen />
          {t("knowledge.advanced.open_common")}
        </Button>
      </div>
      {error && (
        <p className="text-sm text-danger" role="alert">
          {describeError(error, t)}
        </p>
      )}
      <Section id="index" title={t("knowledge.advanced.index")} open={open.index} onToggle={() => toggle("index")} extra={status && indexBadge(status)}>
        {!status ? (
          <Spinner />
        ) : (
          <div className="flex flex-col gap-3" data-testid="knowledge-overview">
            <div className="flex flex-wrap items-center gap-2">
              <Button size="sm" variant="outline" onClick={() => void index(false)} disabled={busy !== null || status.index_running} data-testid="knowledge-index">
                <RefreshCw />
                {t("knowledge.actions.index")}
              </Button>
              <Button size="sm" variant="ghost" onClick={reindexAll} disabled={busy !== null || status.index_running}>
                {t("knowledge.actions.reindex")}
              </Button>
              {status.index_running && (
                <span className="flex items-center gap-1 text-xs text-fg-muted" role="status">
                  <Spinner />
                  {t("knowledge.status.index_running")}
                </span>
              )}
            </div>
            <div className="grid grid-cols-2 gap-2 sm:grid-cols-4 lg:grid-cols-6">
              <Stat label={t("knowledge.status.documents")} value={status.documents_total} testId="kb-stat-documents" />
              <Stat label={t("knowledge.status.indexed")} value={status.documents_indexed} testId="kb-stat-indexed" />
              <Stat label={t("knowledge.status.pending")} value={status.documents_pending} testId="kb-stat-pending" tone={status.documents_pending > 0 ? "warn" : undefined} />
              <Stat label={t("knowledge.status.no_text")} value={status.documents_no_text} testId="kb-stat-no-text" />
              <Stat label={t("knowledge.status.failed")} value={status.documents_failed} testId="kb-stat-failed" tone={status.documents_failed > 0 ? "danger" : undefined} />
              <Stat label={t("knowledge.status.knowledge_docs")} value={status.knowledge_documents} testId="kb-stat-knowledge" />
              <Stat label={t("knowledge.status.chunks")} value={status.chunks_total} testId="kb-stat-chunks" />
              <Stat label={t("knowledge.status.embedded")} value={status.chunks_embedded} testId="kb-stat-embedded" />
              <Stat label={t("knowledge.status.collections")} value={status.collections} testId="kb-stat-collections" />
              <Stat label={t("knowledge.status.glossary_terms")} value={status.glossary_terms} testId="kb-stat-glossary" />
              <Stat label={t("knowledge.status.answers")} value={status.answers} testId="kb-stat-answers" />
              <Stat label={`${t("knowledge.status.helpful")} / ${t("knowledge.status.wrong")}`} value={`${status.answers_helpful} / ${status.answers_wrong}`} testId="kb-stat-feedback" />
            </div>
          </div>
        )}
      </Section>
      <Section id="embedding" title={t("knowledge.advanced.embedding")} open={open.embedding} onToggle={() => toggle("embedding")} extra={status && <Badge variant={emb?.loaded ? "ok" : emb?.installed ? "info" : "muted"}>{t("knowledge.embedding.progress", { embedded: status.chunks_embedded, total: status.chunks_total })}</Badge>}>
        {!status || !emb ? (
          <Spinner />
        ) : (
          <div className="flex flex-col gap-2 text-sm">
            <p className="text-fg-muted" data-testid="kb-embedding">
              {!emb.model ? t("knowledge.embedding.none") : !emb.installed ? t("knowledge.embedding.not_installed", { model: emb.model }) : emb.loaded ? t("knowledge.embedding.ready", { model: emb.model }) : t("knowledge.embedding.not_loaded", { model: emb.model })}
            </p>
            <div className="flex flex-wrap items-center gap-2 text-xs">
              <span>{t("knowledge.embedding.progress", { embedded: status.chunks_embedded, total: status.chunks_total })}</span>
              {status.embed_running && (
                <span className="flex items-center gap-1 text-fg-muted" role="status">
                  <Spinner />
                  {t("knowledge.status.embed_running")}
                </span>
              )}
              <span className="ml-auto flex gap-1">
                <Button size="sm" variant="outline" onClick={() => void embed()} disabled={!emb.installed || busy !== null || status.embed_running} data-testid="kb-embed-run">
                  {t("knowledge.embedding.run")}
                </Button>
                <Button size="sm" variant="ghost" onClick={resetEmbeddings} disabled={busy !== null || status.chunks_embedded === 0}>
                  {t("knowledge.embedding.reset")}
                </Button>
              </span>
            </div>
          </div>
        )}
      </Section>
      <Section id="tokenizer" title={t("knowledge.advanced.tokenizer")} open={open.tokenizer} onToggle={() => toggle("tokenizer")} extra={status && <Badge variant={tokenizerSimple ? "warn" : "ok"} data-testid="kb-tokenizer">{tokenizerSimple ? t("knowledge.tokenizer.simple") : t("knowledge.tokenizer.kodic")}</Badge>}>
        {!status ? (
          <Spinner />
        ) : (
          <div className="flex flex-col gap-1 text-xs">
            {status.tokenizer.version && <p className="text-fg-muted">{status.tokenizer.version}</p>}
            {tokenizerSimple && (
              <div className="flex flex-col gap-1" data-testid="kb-tokenizer-hint">
                <p className="text-warn selectable">{t("knowledge.tokenizer.simple_hint")}</p>
                <p className="text-fg-subtle">{t("knowledge.tokenizer.restart_hint")}</p>
              </div>
            )}
            {status.tokenizer.path && (
              <p className="flex items-center gap-2">
                <span className="text-fg-subtle">{t("knowledge.tokenizer.folder")}:</span>
                <code className="truncate selectable" title={status.tokenizer.path}>
                  {status.tokenizer.path}
                </code>
                <Button size="sm" variant="ghost" className="ml-auto h-6 px-2" onClick={() => void revealPath(status.tokenizer.path ?? "").catch(() => undefined)}>
                  {t("knowledge.tokenizer.open_folder")}
                </Button>
              </p>
            )}
          </div>
        )}
      </Section>
      <Section id="versions" title={t("knowledge.advanced.versions")} open={open.versions} onToggle={() => toggle("versions")}>
        <VersionsPanel />
      </Section>
      <Section id="glossary" title={t("knowledge.advanced.glossary")} open={open.glossary} onToggle={() => toggle("glossary")} extra={status && <Badge variant="muted">{status.glossary_terms}</Badge>}>
        <GlossaryPanel />
      </Section>
      <Section id="test" title={t("knowledge.advanced.test")} open={open.test} onToggle={() => toggle("test")}>
        <QuestionTestPanel />
      </Section>
      <Section id="answers" title={t("knowledge.advanced.answers")} open={open.answers} onToggle={() => toggle("answers")} extra={status && <Badge variant="muted">{status.answers}</Badge>}>
        <AnswersPanel />
      </Section>
      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
