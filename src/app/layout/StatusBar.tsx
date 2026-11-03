import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { isTauri } from "@/lib/ipc";
import { useAssessment } from "@/lib/queries";
import { formatMiB } from "@/lib/utils";

function useOnline(): boolean {
  const [online, setOnline] = useState(typeof navigator === "undefined" ? false : navigator.onLine);
  useEffect(() => {
    const on = () => setOnline(true);
    const off = () => setOnline(false);
    window.addEventListener("online", on);
    window.addEventListener("offline", off);
    return () => {
      window.removeEventListener("online", on);
      window.removeEventListener("offline", off);
    };
  }, []);
  return online;
}

export function StatusBar() {
  const { t } = useTranslation();
  const online = useOnline();
  const { data: assessment } = useAssessment();
  const installed = assessment?.active_model_installed?.installed ?? false;
  const vram = assessment?.tier.free_vram_mib != null ? `${formatMiB(assessment.tier.free_vram_mib)} / ${formatMiB(assessment.tier.total_vram_mib)}` : "—";

  return (
    <footer className="flex h-7 shrink-0 items-center gap-4 border-t border-border bg-bg-elevated px-3 text-xs text-fg-muted" data-testid="statusbar">
      <span className="flex items-center gap-1.5">
        <span className={installed ? "size-2 rounded-full bg-warn" : "size-2 rounded-full bg-fg-subtle"} aria-hidden="true" />
        {t("statusbar.engine")}: {installed ? t("statusbar.engine_not_started") : t("statusbar.engine_no_model")}
      </span>
      <span>
        {t("statusbar.vram")} {vram}
      </span>
      <span className="truncate">
        {t("statusbar.model")}: {assessment?.active_model?.display ?? t("topbar.no_model")}
      </span>
      <span>{t("statusbar.queue", { count: 0 })}</span>
      <span className="ml-auto">{online ? t("statusbar.network_online") : t("statusbar.network_offline")}</span>
      {!isTauri && <span className="rounded bg-warn-soft px-1.5 text-warn">{t("statusbar.preview")}</span>}
    </footer>
  );
}
