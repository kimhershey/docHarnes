import { Play } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { api } from "@/lib/ipc";
import type { ChatTestResult } from "@/lib/types";
import { describeError } from "@/lib/errors";
import { isComposing } from "@/lib/shortcuts";

/** One-shot chat request against the loaded model (smoke test / speed check). */
export function ChatTestDialog({ open, onOpenChange, modelId }: { open: boolean; onOpenChange: (o: boolean) => void; modelId: string | null }) {
  const { t } = useTranslation();
  const [prompt, setPrompt] = useState("");
  const [result, setResult] = useState<ChatTestResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      setResult(await api.engineChatTest(modelId, prompt.trim() || null));
    } catch (e) {
      setError(describeError(e, t));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl" closeLabel={t("common.close")}>
        <DialogHeader>
          <DialogTitle>{t("models.test.title")}</DialogTitle>
          <DialogDescription>{modelId ?? t("topbar.no_model")}</DialogDescription>
        </DialogHeader>
        <div className="flex gap-2">
          <Input
            value={prompt}
            placeholder={t("models.test.prompt")}
            onChange={(e) => setPrompt(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !isComposing(e.nativeEvent)) {
                e.preventDefault();
                void run();
              }
            }}
            data-testid="chat-test-prompt"
          />
          <Button onClick={() => void run()} disabled={busy || !modelId} data-testid="chat-test-run">
            {busy ? <Spinner /> : <Play />}
            {t("models.test.run")}
          </Button>
        </div>
        {error && <p className="text-sm text-danger">{error}</p>}
        {result && (
          <div className="flex flex-col gap-2" data-testid="chat-test-result">
            <p className="text-xs text-fg-muted">{t("models.test.result")}</p>
            <pre className="max-h-60 overflow-auto rounded-md border border-border bg-bg p-3 text-sm selectable whitespace-pre-wrap">{result.content || "(empty)"}</pre>
            <p className="text-xs text-fg-subtle">
              {t("models.test.stats", {
                tokens: result.completion_tokens,
                prompt: result.prompt_tokens,
                ms: result.elapsed_ms,
                tps: result.tokens_per_second?.toFixed(1) ?? "?",
              })}
              {result.finish_reason ? ` · ${result.finish_reason}` : ""}
            </p>
          </div>
        )}
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("common.close")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
