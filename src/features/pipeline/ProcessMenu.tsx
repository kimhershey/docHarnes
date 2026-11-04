import { Play } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { ConfirmDialog, type ConfirmRequest } from "@/components/ui/confirm-dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { describeError } from "@/lib/errors";
import { api } from "@/lib/ipc";
import { usePipelineStatus } from "@/lib/queries";
import { queryClient, queryKeys } from "@/lib/query-client";
import type { DocumentView, ProcessRequest } from "@/lib/types";
import { useUiStore } from "@/stores/ui";

/** The part of a document record the menu needs to warn before replacing existing extraction results. */
export type ProcessMenuDoc = Pick<DocumentView, "id" | "extracted_at" | "review_status">;

/**
 * "처리 ▾" menu shared by the library bulk bar and the viewer toolbar
 * (design §3.7.2 "다시 처리 ▾: 표준 / 잔글씨 / 강한 모델", bulk 분류·OCR·추출).
 * `docs` (the library passes its records) lets the menu confirm before an extraction replaces the
 * results of already extracted / reviewed documents; without it the records are fetched on demand.
 */
export function ProcessMenu({
  ids,
  docs,
  label,
  variant = "secondary",
  size = "sm",
  testId = "process-menu",
  onStarted,
}: {
  ids: string[];
  docs?: ProcessMenuDoc[];
  label?: string;
  variant?: "secondary" | "ghost" | "default";
  size?: "sm" | "default";
  testId?: string;
  onStarted?: (jobId: string | null) => void;
}) {
  const { t } = useTranslation();
  const toast = useUiStore((s) => s.toast);
  const { data: status } = usePipelineStatus();
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  // Only "no vision model at all" blocks the menu: an unloaded model is loaded when the job starts.
  const noModel = status !== undefined && status.model.model === null;

  const start = async (req: Omit<ProcessRequest, "document_ids">) => {
    try {
      const job = await api.pipelineProcess({ document_ids: ids, ...req });
      toast(job ? t("pipeline.process.started", { count: ids.length }) : t("pipeline.process.queued", { count: ids.length }), "info");
      // running / queued counters of the status chip.
      void queryClient.invalidateQueries({ queryKey: queryKeys.pipelineStatus });
      onStarted?.(job);
    } catch (e) {
      toast(describeError(e, t), "danger");
    }
  };

  // Extraction always creates a new run (the runner never skips it), which demotes the review of the
  // affected documents: ask first when any of them already has results (review M3).
  const run = async (req: Omit<ProcessRequest, "document_ids">) => {
    if (ids.length === 0) return;
    if (req.level !== "extract") return start(req);
    let records: ProcessMenuDoc[] = docs ?? [];
    if (!docs) {
      try {
        records = await Promise.all(ids.map((id) => api.documentsGet(id)));
      } catch {
        records = [];
      }
    }
    const replaced = records.filter((d) => ids.includes(d.id) && (d.extracted_at !== null || d.review_status === "done")).length;
    if (replaced === 0) return start(req);
    // Opened once the dropdown has unmounted so the two Radix layers do not fight over focus.
    window.setTimeout(
      () =>
        setConfirm({
          title: t("pipeline.process.confirm_title"),
          description: t("pipeline.process.confirm_body", { count: replaced }),
          confirmLabel: t("pipeline.process.confirm_ok"),
          danger: true,
          onConfirm: () => start(req),
        }),
      0,
    );
  };

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button size={size} variant={variant} disabled={ids.length === 0 || noModel} title={noModel ? t("pipeline.process.model_none") : undefined} data-testid={testId} data-no-model={noModel || undefined}>
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
      <ConfirmDialog request={confirm} onClose={() => setConfirm(null)} />
    </>
  );
}
