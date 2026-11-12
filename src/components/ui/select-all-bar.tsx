import { useTranslation } from "react-i18next";

import { cn } from "@/lib/utils";

import { Button } from "./button";

/**
 * 1.0.0: "전체 선택 / 전체 해제 · n개 선택" above a check list (user request 2026-11-12: every
 * folder / file selection has both). It acts on the rows that are visible — after a search or a
 * project filter — and leaves rows picked earlier alone, so several filtered batches add up.
 * The bar wraps instead of pushing the dialog wider.
 */
export function SelectAllBar({
  visibleIds,
  picked,
  onChange,
  disabled = false,
  className,
  testId = "select-all",
}: {
  visibleIds: string[];
  picked: ReadonlySet<string>;
  onChange: (next: Set<string>) => void;
  disabled?: boolean;
  className?: string;
  testId?: string;
}) {
  const { t } = useTranslation();
  const visiblePicked = visibleIds.reduce((n, id) => n + (picked.has(id) ? 1 : 0), 0);
  const all = () => {
    const next = new Set(picked);
    for (const id of visibleIds) next.add(id);
    onChange(next);
  };
  const none = () => {
    const next = new Set(picked);
    for (const id of visibleIds) next.delete(id);
    onChange(next);
  };
  return (
    <div className={cn("flex min-w-0 flex-wrap items-center gap-1 text-xs text-fg-muted", className)} data-testid={testId}>
      <Button size="sm" variant="ghost" onClick={all} disabled={disabled || visibleIds.length === 0 || visiblePicked === visibleIds.length} data-testid={`${testId}-all`}>
        {t("common.select_all")}
      </Button>
      <Button size="sm" variant="ghost" onClick={none} disabled={disabled || visiblePicked === 0} data-testid={`${testId}-none`}>
        {t("common.select_none")}
      </Button>
      <span className="ml-auto shrink-0 tabular-nums" aria-live="polite" data-testid={`${testId}-count`}>
        {t("common.selected_count", { count: picked.size })}
      </span>
    </div>
  );
}
