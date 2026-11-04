import { Brain, FolderInput, MessageSquare, Send, Settings } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Link, useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { executePreRoute } from "@/lib/commands";
import { api } from "@/lib/ipc";
import { useAssessment, useDocumentStats, useDocuments, usePipelineStatus } from "@/lib/queries";
import { isComposing } from "@/lib/shortcuts";
import { SCREEN_ROUTES } from "@/lib/types";
import { formatMiB } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";
import { useWorkspace } from "@/stores/workspace";

export function HomePage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const workspace = useWorkspace();
  const { data: assessment } = useAssessment();
  const { data: libraryStats } = useDocumentStats(!!workspace);
  const { data: pipeline } = usePipelineStatus(!!workspace);
  const { data: recent } = useDocuments({ limit: 6, sort: "imported_desc" }, !!workspace);
  const setPending = useUiStore((s) => s.setPendingAssistantInput);
  const [text, setText] = useState("");
  const examples = t("home.examples", { returnObjects: true }) as string[];

  const submit = async (value = text) => {
    const v = value.trim();
    if (!v) return;
    const r = await api.prerouterRoute(v);
    if (r.matched) {
      await executePreRoute(r, navigate, t);
      setText("");
      return;
    }
    setPending(v);
    navigate(SCREEN_ROUTES.assistant);
  };

  const stats: { key: "new" | "processing" | "review" | "done" }[] = [{ key: "new" }, { key: "processing" }, { key: "review" }, { key: "done" }];

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-4 p-6" data-testid="page-home">
      <div>
        <h1 className="text-xl font-semibold">{t("home.welcome", { name: workspace?.manifest.name ?? t("app.name") })}</h1>
        <p className="text-sm text-fg-muted selectable">{workspace ? t("home.welcome_sub", { path: workspace.path }) : t("home.no_workspace")}</p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>{t("home.command_title")}</CardTitle>
          <CardDescription>{t("home.command_hint")}</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <form
            className="flex gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              void submit();
            }}
          >
            <Input
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder={t("home.command_placeholder")}
              data-testid="home-command"
              onKeyDown={(e) => {
                if (e.key === "Enter" && isComposing(e)) e.preventDefault();
              }}
            />
            <Button type="submit" aria-label={t("assistant.send")} data-testid="home-command-send">
              <Send />
            </Button>
          </form>
          <div className="flex flex-wrap gap-2">
            {examples.map((ex) => (
              <button
                key={ex}
                type="button"
                className="rounded-full border border-border bg-bg px-3 py-1 text-xs text-fg-muted hover:border-accent hover:text-fg cursor-pointer"
                onClick={() => void submit(ex)}
                data-testid="home-example"
              >
                {ex}
              </button>
            ))}
          </div>
        </CardContent>
      </Card>

      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        {stats.map((s) => {
          const by = libraryStats?.by_status ?? {};
          const value =
            s.key === "new"
              ? (by.new ?? 0) + (by.importing ?? 0)
              : s.key === "processing"
                ? by.rendered ?? 0
                : s.key === "review"
                  ? (pipeline?.review_pending ?? by.extracted ?? 0)
                  : by.reviewed ?? 0;
          return (
            <Card key={s.key}>
              <CardContent className="p-4">
                <p className="text-xs text-fg-muted">{t(`home.stats.${s.key}`)}</p>
                <p className="mt-1 text-2xl font-semibold tabular-nums" data-testid={`home-stat-${s.key}`}>
                  {libraryStats ? value : "—"}
                </p>
              </CardContent>
            </Card>
          );
        })}
      </div>
      <p className="-mt-2 flex items-center gap-2 text-xs text-fg-subtle">
        <span>{t("home.stats_hint")}</span>
        {pipeline && pipeline.review_fields > 0 && (
          <Link to={SCREEN_ROUTES.review} className="text-accent hover:underline" data-testid="home-review-link">
            {t("home.review_fields", { count: pipeline.review_fields })} · {t("home.review_link")}
          </Link>
        )}
      </p>

      <div className="grid gap-4 md:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>{t("home.engine_card")}</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-2 text-sm">
            {assessment ? (
              <>
                <div className="flex items-center gap-2">
                  <Badge>{t(`hw.tier.${assessment.tier.tier}`)}</Badge>
                  <span className="text-fg-muted">{assessment.tier.gpu_name ?? t("onboarding.hardware.no_gpu")}</span>
                </div>
                <div>
                  {t("statusbar.vram")}: {formatMiB(assessment.tier.free_vram_mib)} / {formatMiB(assessment.tier.total_vram_mib)}
                </div>
                <div>
                  {t("statusbar.model")}: {assessment.active_model?.display ?? t("topbar.no_model")}{" "}
                  <Badge variant={assessment.active_model_installed?.installed ? "ok" : "warn"}>
                    {assessment.active_model_installed?.installed ? t("common.installed") : t("common.not_installed")}
                  </Badge>
                </div>
                <Link to={SCREEN_ROUTES.models} className="text-accent hover:underline">
                  {t("nav.models")} →
                </Link>
              </>
            ) : (
              <span className="text-fg-muted">{t("common.loading")}</span>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>{t("home.quick.title")}</CardTitle>
          </CardHeader>
          <CardContent className="grid grid-cols-2 gap-2">
            <Button variant="secondary" disabled title={t("common.planned_in", { phase: "P2" })}>
              <FolderInput />
              {t("home.quick.import")}
            </Button>
            <Button variant="secondary" asChild>
              <Link to={SCREEN_ROUTES.models}>
                <Brain />
                {t("home.quick.models")}
              </Link>
            </Button>
            <Button variant="secondary" asChild>
              <Link to={SCREEN_ROUTES.assistant}>
                <MessageSquare />
                {t("home.quick.assistant")}
              </Link>
            </Button>
            <Button variant="secondary" asChild>
              <Link to={SCREEN_ROUTES.settings}>
                <Settings />
                {t("home.quick.settings")}
              </Link>
            </Button>
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>{t("home.recent_docs")}</CardTitle>
        </CardHeader>
        <CardContent>
          {recent && recent.length > 0 ? (
            <ul className="divide-y divide-border" data-testid="home-recent">
              {recent.map((d) => (
                <li key={d.id}>
                  <Link to={`/library/${d.id}`} className="flex items-center gap-3 py-2 text-sm hover:underline">
                    {d.thumb_url ? <img src={d.thumb_url} alt="" className="h-10 w-8 bg-white object-contain" /> : <FolderInput className="size-4 text-fg-subtle" aria-hidden="true" />}
                    <span className="min-w-0 flex-1 truncate">{d.title ?? d.original_name}</span>
                    <Badge variant="outline">{t(`documents.kind.${d.source_kind}`)}</Badge>
                    <span className="text-xs text-fg-subtle">{t("library.pages_count", { count: d.page_count })}</span>
                  </Link>
                </li>
              ))}
              <li className="pt-2">
                <Link to={SCREEN_ROUTES.library} className="text-sm text-accent hover:underline">
                  {t("home.recent_open")}
                </Link>
              </li>
            </ul>
          ) : (
            <p className="text-sm text-fg-muted">{t("home.recent_empty")}</p>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
