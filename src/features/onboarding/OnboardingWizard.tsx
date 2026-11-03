import { Check, Download, FolderInput, Clock } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { HardwarePanel } from "@/features/models/HardwarePanel";
import { ImportDialog } from "@/features/models/ImportDialog";
import { RequiredFilesDialog } from "@/features/models/RequiredFilesDialog";
import { api, revealPath } from "@/lib/ipc";
import { useAssessment } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import { describeError } from "@/lib/errors";
import { cn } from "@/lib/utils";
import { useSettingsStore } from "@/stores/settings";
import { useUiStore } from "@/stores/ui";
import { useWorkspaceStore } from "@/stores/workspace";
import { ThemeMenu } from "@/app/layout/ThemeMenu";

import { WorkspaceChooser } from "../workspace/WorkspaceChooser";

type Step = 0 | 1 | 2;

export function OnboardingWizard() {
  const { t } = useTranslation();
  const workspace = useWorkspaceStore((s) => s.current);
  const setActive = useUiStore((s) => s.setOnboardingActive);
  const [step, setStep] = useState<Step>(workspace ? 1 : 0);
  const { data: assessment } = useAssessment();
  const [filesOpen, setFilesOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const online = typeof navigator !== "undefined" ? navigator.onLine : false;
  const allowDownload = useSettingsStore((s) => s.settings?.models.allow_online_download ?? false);
  const toast = useUiStore((s) => s.toast);
  const installed = assessment?.active_model_installed?.installed ?? false;
  const download = async () => {
    if (!assessment?.active_model) return;
    try {
      await api.modelsDownload(assessment.active_model.id);
      toast(t("models.download_started"), "ok");
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  const afterImport = () => {
    void queryClient.invalidateQueries({ queryKey: queryKeys.assessment });
    void queryClient.invalidateQueries({ queryKey: queryKeys.registry });
    void queryClient.invalidateQueries({ queryKey: queryKeys.modelIndex });
  };

  useEffect(() => {
    setActive(true);
  }, [setActive]);

  const steps: { key: Step; label: string }[] = [
    { key: 0, label: t("onboarding.steps.workspace") },
    { key: 1, label: t("onboarding.steps.hardware") },
    { key: 2, label: t("onboarding.steps.model") },
  ];

  const finish = () => setActive(false);

  return (
    <div className="flex h-full flex-col bg-bg text-fg" data-testid="onboarding">
      <header className="flex h-12 items-center justify-between border-b border-border px-4">
        <span className="font-semibold">{t("app.name")}</span>
        <ThemeMenu />
      </header>
      <div className="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-6 overflow-y-auto p-6">
        <div>
          <h1 className="text-2xl font-semibold">{t("onboarding.title")}</h1>
          <p className="mt-1 text-fg-muted">{t("onboarding.subtitle")}</p>
        </div>
        <ol className="flex items-center gap-2 text-sm" aria-label="steps">
          {steps.map((s, i) => (
            <li key={s.key} className="flex items-center gap-2">
              <span
                className={cn(
                  "flex size-6 items-center justify-center rounded-full border text-xs font-semibold",
                  step === s.key && "border-accent bg-accent text-accent-fg",
                  step > s.key && "border-ok bg-ok-soft text-ok",
                  step < s.key && "border-border text-fg-subtle",
                )}
                aria-current={step === s.key ? "step" : undefined}
              >
                {step > s.key ? <Check className="size-3.5" /> : i + 1}
              </span>
              <span className={cn(step === s.key ? "font-medium" : "text-fg-muted")}>{s.label}</span>
              {i < steps.length - 1 && <span className="mx-1 h-px w-8 bg-border" aria-hidden="true" />}
            </li>
          ))}
        </ol>

        {step === 0 && (
          <Card>
            <CardHeader>
              <CardTitle>{t("onboarding.steps.workspace")}</CardTitle>
              <CardDescription>{t("onboarding.workspace.intro")}</CardDescription>
            </CardHeader>
            <CardContent>
              <WorkspaceChooser mode="create" onOpened={() => setStep(1)} />
              <p className="mt-3 text-xs text-fg-subtle">{t("onboarding.workspace.settings_sync")}</p>
            </CardContent>
          </Card>
        )}

        {step === 1 && (
          <Card>
            <CardHeader>
              <CardTitle>{t("onboarding.steps.hardware")}</CardTitle>
              <CardDescription>{t("onboarding.hardware.intro")}</CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              <HardwarePanel />
              <div className="flex justify-between">
                <Button variant="ghost" onClick={() => setStep(0)}>
                  {t("common.back")}
                </Button>
                <Button onClick={() => setStep(2)} data-testid="onboarding-next">
                  {t("common.next")}
                </Button>
              </div>
            </CardContent>
          </Card>
        )}

        {step === 2 && (
          <Card>
            <CardHeader>
              <CardTitle>{t("onboarding.steps.model")}</CardTitle>
              <CardDescription>{t("onboarding.model.intro")}</CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              <div className="grid gap-3 md:grid-cols-3">
                <OptionCard
                  icon={Download}
                  title={t("onboarding.model.download")}
                  body={online ? (allowDownload ? t("onboarding.model.download_desc") : t("models.download_disabled")) : t("onboarding.model.download_offline")}
                  disabled={!online || !allowDownload || !assessment?.active_model || installed}
                  action={() => void download()}
                  actionLabel={t("onboarding.model.download_action")}
                />
                <OptionCard
                  icon={FolderInput}
                  title={t("onboarding.model.import")}
                  body={t("onboarding.model.import_desc")}
                  action={workspace ? () => setImportOpen(true) : undefined}
                  actionLabel={t("onboarding.model.import_action")}
                  secondaryAction={workspace ? () => void revealPath(workspace.models_dir) : undefined}
                  secondaryLabel={t("onboarding.model.models_dir")}
                />
                <OptionCard icon={Clock} title={t("onboarding.model.later")} body={t("onboarding.model.later_desc")} action={finish} actionLabel={t("onboarding.model.finish")} primary />
              </div>
              {assessment?.active_model && (
                <div className="flex flex-wrap items-center gap-2 text-sm text-fg-muted" data-testid="onboarding-model-state">
                  <span>
                    {t("onboarding.hardware.recommended")}: <strong className="text-fg">{assessment.active_model.display}</strong>
                  </span>
                  {installed && <span className="text-ok">{t("onboarding.model.installed")}</span>}
                  <Button variant="link" size="sm" onClick={() => setFilesOpen(true)}>
                    {t("onboarding.model.required_files")}
                  </Button>
                </div>
              )}
              <div className="flex justify-between">
                <Button variant="ghost" onClick={() => setStep(1)}>
                  {t("common.back")}
                </Button>
                <Button onClick={finish} data-testid="onboarding-finish">
                  {t("onboarding.model.finish")}
                </Button>
              </div>
            </CardContent>
          </Card>
        )}
      </div>
      {assessment?.active_model && <RequiredFilesDialog modelId={assessment.active_model.id} open={filesOpen} onOpenChange={setFilesOpen} />}
      <ImportDialog open={importOpen} onOpenChange={setImportOpen} onStarted={afterImport} />
    </div>
  );
}

function OptionCard({
  icon: Icon,
  title,
  body,
  action,
  actionLabel,
  secondaryAction,
  secondaryLabel,
  disabled,
  hint,
  primary,
}: {
  icon: typeof Download;
  title: string;
  body: string;
  action?: () => void;
  actionLabel?: string;
  secondaryAction?: () => void;
  secondaryLabel?: string;
  disabled?: boolean;
  hint?: string;
  primary?: boolean;
}) {
  return (
    <div className={cn("flex flex-col gap-2 rounded-lg border border-border p-4", disabled && "opacity-60")}>
      <Icon className="size-5 text-accent" aria-hidden="true" />
      <p className="font-medium">{title}</p>
      <p className="flex-1 text-sm text-fg-muted">{body}</p>
      {hint && <p className="text-xs text-fg-subtle">{hint}</p>}
      {action && actionLabel && (
        <Button variant={primary ? "default" : "secondary"} size="sm" onClick={action} disabled={disabled}>
          {actionLabel}
        </Button>
      )}
      {secondaryAction && secondaryLabel && (
        <Button variant="ghost" size="sm" onClick={secondaryAction}>
          {secondaryLabel}
        </Button>
      )}
    </div>
  );
}
