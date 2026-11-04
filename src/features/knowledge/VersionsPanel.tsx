import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { useVersions } from "@/lib/queries";
import type { VersionInfo } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

export function VersionsPanel() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const toast = useUiStore((s) => s.toast);
  const { data: groups, isLoading } = useVersions();

  const setLatest = async (v: VersionInfo) => {
    try {
      await api.knowledgeVersionSet({ document_id: v.document_id, group_key: v.group_key, version_label: v.version_label, effective_date: v.effective_date, is_latest: true, note: v.note });
      toast(t("knowledge.versions.saved"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const clear = async (v: VersionInfo) => {
    try {
      await api.knowledgeVersionClear(v.document_id);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  return (
    <div className="flex flex-col gap-2" data-testid="knowledge-versions">
      <h2 className="text-sm font-semibold">{t("knowledge.versions.title")}</h2>
      <p className="text-xs text-fg-muted">{t("knowledge.versions.hint")}</p>
      {isLoading && <Spinner />}
      {groups && groups.length === 0 && <p className="text-sm text-fg-muted">{t("knowledge.versions.empty")}</p>}
      {groups?.map((g) => (
        <div key={g.group_key} className="rounded-md border border-border" data-testid="version-group" data-group={g.group_key}>
          <div className="border-b border-border bg-bg-sunken px-3 py-1.5 text-sm font-medium">{g.group_key}</div>
          <ul className="divide-y divide-border">
            {g.documents.map((v) => (
              <li key={v.document_id} className="flex flex-wrap items-center gap-2 px-3 py-2 text-sm" data-testid="version-doc" data-latest={v.is_latest}>
                <button type="button" className="min-w-0 flex-1 truncate text-left hover:underline cursor-pointer" onClick={() => navigate(`/library/${v.document_id}`)}>
                  {v.title}
                </button>
                {v.version_label && <span className="text-xs text-fg-muted">{v.version_label}</span>}
                {v.effective_date && <span className="text-xs text-fg-subtle">{v.effective_date}</span>}
                <Badge variant={v.is_latest ? "ok" : "muted"}>{v.is_latest ? t("knowledge.versions.latest") : t("knowledge.versions.superseded")}</Badge>
                {!v.is_latest && (
                  <Button size="sm" variant="ghost" onClick={() => void setLatest(v)}>
                    {t("knowledge.versions.set_latest")}
                  </Button>
                )}
                <Button size="sm" variant="ghost" onClick={() => void clear(v)}>
                  {t("knowledge.versions.clear")}
                </Button>
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}
