import { useTranslation } from "react-i18next";

import type { Clipped } from "@/stores/assistant";

/**
 * The line on a card that was too large to be stored whole and was read back with less (`storedMessage`,
 * CON-16): its lists hold the first rows (the counts are those of the whole), or — `all` — it is a record
 * nothing can be run from again.
 */
export function ClippedNote({ clipped }: { clipped?: Clipped | null }) {
  const { t } = useTranslation();
  if (!clipped) return null;
  return (
    <p className="text-xs text-fg-subtle" data-testid="card-clipped" data-clipped={clipped}>
      {clipped === "all" ? t("assistant.clipped.all") : t("assistant.clipped.lists")}
    </p>
  );
}
