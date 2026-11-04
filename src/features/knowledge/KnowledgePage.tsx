import { BookOpen, Database, RefreshCw, Upload } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { ConfirmDialog, type ConfirmRequest } from "@/components/ui/confirm-dialog";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { describeError } from "@/lib/errors";
import { api, revealPath } from "@/lib/ipc";
import { useKnowledgeStatus } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import type { KnowledgeStatus } from "@/lib/types";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";

import { AnswersPanel } from "./AnswersPanel";
import { CollectionsPanel } from "./CollectionsPanel";
import { GlossaryPanel } from "./GlossaryPanel";
import { IngestWizard } from "./IngestWizard";
import { QuestionTestPanel } from "./QuestionTestPanel";
import { VersionsPanel } from "./VersionsPanel";

const TABS = ["overview", "collections", "versions", "glossary", "test", "answers"] as const;

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

function Overview({ status, onIngest }: { status: KnowledgeStatus | undefined; onIngest: () => void }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
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
  if (!status) return <Spinner />;
  const emb = status.embedding;
  const tokenizerSimple = status.tokenizer.kind !== "ko-dic";
  return (
    <div className="flex flex-col gap-3" data-testid="knowledge-overview">
      <div className="flex flex-wrap gap-2">
        <Button onClick={onIngest} data-testid="knowledge-ingest">
          <Upload />
          {t("knowledge.actions.ingest")}
        </Button>
        <Button variant="outline" onClick={() => void index(false)} disabled={busy !== null || status.index_running} data-testid="knowledge-index">
          <RefreshCw />
          {t("knowledge.actions.index")}
        </Button>
        <Button variant="ghost" onClick={reindexAll} disabled={busy !== null || status.index_running}>
          {t("knowledge.actions.reindex")}
        </Button>
        {status.index_running && (
          <span className="flex items-center gap-1 text-xs text-fg-muted" role="status">
            <Spinner />
            {t("knowledge.status.index_running")}
          </span>
        )}
      </div>
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Database className="size-4" aria-hidden="true" />
            {t("knowledge.status.title")}
          </CardTitle>
        </CardHeader>
        <CardContent>
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
        </CardContent>
      </Card>
      <div className="grid gap-3 md:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>{t("knowledge.tokenizer.label")}</CardTitle>
            <CardDescription>
              <Badge variant={tokenizerSimple ? "warn" : "ok"} data-testid="kb-tokenizer">
                {tokenizerSimple ? t("knowledge.tokenizer.simple") : t("knowledge.tokenizer.kodic")}
              </Badge>
              {status.tokenizer.version && <span className="ml-2 text-xs">{status.tokenizer.version}</span>}
            </CardDescription>
          </CardHeader>
          {tokenizerSimple && (
            <CardContent className="flex flex-col gap-1 text-xs" data-testid="kb-tokenizer-hint">
              <p className="text-warn selectable">{t("knowledge.tokenizer.simple_hint")}</p>
              <p className="text-fg-subtle">{t("knowledge.tokenizer.restart_hint")}</p>
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
            </CardContent>
          )}
        </Card>
        <Card>
          <CardHeader>
            <CardTitle>{t("knowledge.embedding.label")}</CardTitle>
            <CardDescription data-testid="kb-embedding">
              {!emb.model
                ? t("knowledge.embedding.none")
                : !emb.installed
                  ? t("knowledge.embedding.not_installed", { model: emb.model })
                  : emb.loaded
                    ? t("knowledge.embedding.ready", { model: emb.model })
                    : t("knowledge.embedding.not_loaded", { model: emb.model })}
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-wrap items-center gap-2 text-xs">
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
          </CardContent>
        </Card>
      </div>
      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}

/** Knowledge base screen (design §3.14 / §3.7): dashboard, ingest wizard, collections, versions, glossary, question test, answer log. */
export function KnowledgePage() {
  const { t } = useTranslation();
  const workspace = useWorkspaceStore((s) => s.current);
  const { data: status, error } = useKnowledgeStatus(!!workspace);
  const [wizard, setWizard] = useState<{ open: boolean; collectionId: string | null }>({ open: false, collectionId: null });
  const [tab, setTab] = useState<(typeof TABS)[number]>("overview");

  return (
    <div className="flex h-full flex-col" data-testid="page-knowledge">
      <div className="flex items-center gap-3 border-b border-border px-4 py-2">
        <BookOpen className="size-5 text-accent" aria-hidden="true" />
        <div>
          <h1 className="text-base font-semibold">{t("knowledge.title")}</h1>
          <p className="text-xs text-fg-muted">{t("knowledge.subtitle")}</p>
        </div>
      </div>
      {error && (
        <p className="px-4 py-2 text-sm text-danger" role="alert">
          {describeError(error, t)}
        </p>
      )}
      <Tabs value={tab} onValueChange={(v) => setTab(v as (typeof TABS)[number])} className="flex min-h-0 flex-1 flex-col">
        <TabsList className="mx-4 mt-2">
          {TABS.map((k) => (
            <TabsTrigger key={k} value={k} data-testid={`knowledge-tab-${k}`}>
              {t(`knowledge.tabs.${k}`)}
            </TabsTrigger>
          ))}
        </TabsList>
        <div className="min-h-0 flex-1 overflow-y-auto p-4">
          <TabsContent value="overview">
            <Overview status={status} onIngest={() => setWizard({ open: true, collectionId: null })} />
          </TabsContent>
          <TabsContent value="collections">
            <CollectionsPanel onIngest={(collectionId) => setWizard({ open: true, collectionId })} />
          </TabsContent>
          <TabsContent value="versions">
            <VersionsPanel />
          </TabsContent>
          <TabsContent value="glossary">
            <GlossaryPanel />
          </TabsContent>
          <TabsContent value="test">
            <QuestionTestPanel />
          </TabsContent>
          <TabsContent value="answers">
            <AnswersPanel />
          </TabsContent>
        </div>
      </Tabs>
      <IngestWizard open={wizard.open} collectionId={wizard.collectionId} onOpenChange={(open) => setWizard((w) => ({ ...w, open }))} />
    </div>
  );
}
