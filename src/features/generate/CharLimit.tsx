import { useTranslation } from "react-i18next";

import { cn } from "@/lib/utils";

import { charCount } from "./draft";

/**
 * "1,234 / 6,000자" under an input the model reads only up to a limit (GEN-14: the content and the instruction).
 * Past the limit it says what happens — the first `max` characters are passed on — instead of leaving the cut to
 * be discovered in the result. Nothing is shown when the backend names no limit.
 */
export function CharLimit({ text, max, testId }: { text: string; max: number | null | undefined; testId: string }) {
  const { t, i18n } = useTranslation();
  if (!max) return null;
  const count = charCount(text);
  const over = count > max;
  const fmt = (n: number) => n.toLocaleString(i18n.language.startsWith("ko") ? "ko-KR" : "en-US");
  return (
    <p className={cn("text-xs tabular-nums", over ? "text-warn" : "text-fg-subtle")} role={over ? "status" : undefined} data-testid={testId} data-over={over}>
      {t("generate.form.char_count", { count: fmt(count), max: fmt(max) })}
      {over && ` · ${t("generate.form.char_over", { max: fmt(max) })}`}
    </p>
  );
}
