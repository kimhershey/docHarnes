import { Construction } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import type { Screen } from "@/lib/types";

export function PhasePlaceholder({ screen, phase, phase2 = false }: { screen: Screen; phase: string; phase2?: boolean }) {
  const { t } = useTranslation();
  const features = t(`pages.${screen}.features`, { returnObjects: true }) as string[];
  return (
    <div className="mx-auto max-w-3xl p-6" data-testid={`page-${screen}`}>
      <div className="mb-4 flex items-center gap-3">
        <h1 className="text-xl font-semibold">{t(`nav.${screen}`)}</h1>
        <Badge variant={phase2 ? "warn" : "info"}>{phase2 ? t("common.phase2") : phase}</Badge>
      </div>
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Construction className="size-4 text-fg-muted" aria-hidden="true" />
            {phase2 ? t("placeholder.phase2_title") : t("placeholder.title", { phase })}
          </CardTitle>
          <CardDescription>{phase2 ? t("placeholder.phase2_body") : t("placeholder.features")}</CardDescription>
        </CardHeader>
        <CardContent>
          <ul className="list-disc space-y-1.5 pl-5 text-sm text-fg-muted">
            {Array.isArray(features) && features.map((f, i) => <li key={i}>{f}</li>)}
          </ul>
        </CardContent>
      </Card>
    </div>
  );
}
