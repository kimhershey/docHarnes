import { AlertTriangle, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { api } from "@/lib/ipc";
import { useAssessment, useHardware } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import { formatMiB } from "@/lib/utils";

/** Hardware probe + tier assessment (onboarding step 2 and the model manager). */
export function HardwarePanel() {
  const { t, i18n } = useTranslation();
  const hw = useHardware();
  const assessment = useAssessment();

  const refresh = async () => {
    await api.hwProbe(true);
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: queryKeys.hardware }),
      queryClient.invalidateQueries({ queryKey: queryKeys.assessment }),
    ]);
  };

  if (hw.isLoading || !hw.data) {
    return (
      <div className="flex items-center gap-2 text-fg-muted">
        <Spinner /> {t("common.loading")}
      </div>
    );
  }
  const h = hw.data;
  const gpu = h.gpus.slice().sort((a, b) => (b.free_mib ?? 0) - (a.free_mib ?? 0))[0] ?? null;
  const a = assessment.data;

  return (
    <div className="flex flex-col gap-3" data-testid="hardware-panel">
      <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1.5 text-sm">
        <dt className="text-fg-muted">{t("onboarding.hardware.os")}</dt>
        <dd>
          {h.os} {h.os_version}
        </dd>
        <dt className="text-fg-muted">{t("onboarding.hardware.cpu")}</dt>
        <dd>
          {h.cpu_name} · {t("hw.cores", { physical: h.cpu_cores_physical ?? "?", logical: h.cpu_cores_logical })}
        </dd>
        <dt className="text-fg-muted">{t("onboarding.hardware.ram")}</dt>
        <dd>
          {formatMiB(h.ram_available_mib)} / {formatMiB(h.ram_total_mib)}
        </dd>
        <dt className="text-fg-muted">{t("onboarding.hardware.gpu")}</dt>
        <dd>
          {gpu ? (
            <span>
              {gpu.name}
              {gpu.cuda_driver_version && (
                <span className="text-fg-muted">
                  {" "}
                  · {t("hw.cuda_driver")} {gpu.cuda_driver_version[0]}.{gpu.cuda_driver_version[1]}
                </span>
              )}
              {gpu.compute_capability && (
                <span className="text-fg-muted">
                  {" "}
                  · CC {gpu.compute_capability[0]}.{gpu.compute_capability[1]}
                </span>
              )}
            </span>
          ) : (
            <span className="text-fg-muted">{t("onboarding.hardware.no_gpu")}</span>
          )}
        </dd>
        <dt className="text-fg-muted">{t("onboarding.hardware.vram")}</dt>
        <dd>{gpu ? `${formatMiB(gpu.free_mib)} / ${formatMiB(gpu.total_mib)}` : "—"}</dd>
        <dt className="text-fg-muted">{t("hw.nvml")}</dt>
        <dd className="text-fg-muted">{h.nvml_available ? t("hw.nvml_available") : t("hw.nvml_unavailable")}</dd>
        {a && (
          <>
            <dt className="text-fg-muted">{t("onboarding.hardware.tier")}</dt>
            <dd className="flex items-center gap-2">
              <Badge data-testid="tier-badge">{t(`hw.tier.${a.tier.tier}`)}</Badge>
              <span className="text-xs text-fg-muted">{t(`hw.basis.${a.tier.basis}`)}</span>
            </dd>
            <dt className="text-fg-muted">{t("onboarding.hardware.recommended")}</dt>
            <dd>{a.recommended_model?.display ?? t("common.none")}</dd>
          </>
        )}
        <dt className="text-fg-muted">{t("hw.probed_at")}</dt>
        <dd className="text-fg-muted">{new Date(h.probed_at).toLocaleString(i18n.language)}</dd>
      </dl>
      {a && a.tier.warnings.length > 0 && (
        <ul className="flex flex-col gap-1 rounded-md border border-warn bg-warn-soft p-3 text-sm">
          {a.tier.warnings.map((w) => (
            <li key={w} className="flex items-start gap-2">
              <AlertTriangle className="mt-0.5 size-4 shrink-0 text-warn" aria-hidden="true" />
              <span>{t(w)}</span>
            </li>
          ))}
        </ul>
      )}
      <div>
        <Button variant="outline" size="sm" onClick={() => void refresh()}>
          <RefreshCw />
          {t("onboarding.hardware.refresh")}
        </Button>
      </div>
    </div>
  );
}
