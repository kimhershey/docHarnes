import { Play } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import type { ProcessRequest } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

/**
 * "처리 ▾" menu shared by the library bulk bar and the viewer toolbar
 * (design §3.7.2 "다시 처리 ▾: 표준 / 잔글씨 / 강한 모델", bulk 분류·OCR·추출).
 */
export function ProcessMenu({ ids, label, variant = "secondary", size = "sm", testId = "process-menu", onStarted }: { ids: string[]; label?: string; variant?: "secondary" | "ghost" | "default"; size?: "sm" | "default"; testId?: string; onStarted?: (jobId: string | null) => void }) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const run = async (req: Omit<ProcessRequest, "document_ids">) => {
    if (ids.length === 0) return;
    try {
      const job = await api.pipelineProcess({ document_ids: ids, ...req });
      toast(job ? t("pipeline.process.started", { count: ids.length }) : t("pipeline.process.queued", { count: ids.length }), "info");
      onStarted?.(job);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button size={size} variant={variant} disabled={ids.length === 0} data-testid={testId}>
          <Play />
          {label ?? t("pipeline.process.menu")}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        <DropdownMenuItem onSelect={() => void run({ level: "extract" })} data-testid={`${testId}-standard`}>
          {t("pipeline.process.standard")}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => void run({ level: "classify" })} data-testid={`${testId}-classify`}>
          {t("pipeline.process.classify")}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => void run({ level: "ocr" })} data-testid={`${testId}-ocr`}>
          {t("pipeline.process.ocr")}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void run({ level: "extract", force: true, profile: "fine_print" })} data-testid={`${testId}-fine-print`}>
          {t("pipeline.process.fine_print")}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => void run({ level: "extract", precise: true })} data-testid={`${testId}-precise`}>
          {t("pipeline.process.precise")}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => void run({ level: "extract", force: true })} data-testid={`${testId}-force`}>
          {t("pipeline.process.force")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
