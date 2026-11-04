import { useCallback, useRef, useState } from "react";

import { api } from "@/lib/ipc";
import type { Answer, AnswerRequest } from "@/lib/types";
import { newRequestId, useKnowledgeStore } from "@/stores/knowledge";

/**
 * One grounded-answer request at a time, with the streamed text while it runs.
 * A newer `ask` supersedes an older one (its late result is ignored); `cancel`
 * stops the running request at its next delta.
 */
export function useAsk() {
  const [requestId, setRequestId] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [answer, setAnswer] = useState<Answer | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [lastRequest, setLastRequest] = useState<AnswerRequest | null>(null);
  const latest = useRef<string | null>(null);
  const streamText = useKnowledgeStore((s) => (requestId ? (s.streams[requestId] ?? "") : ""));
  const beginStream = useKnowledgeStore((s) => s.beginStream);
  const clearStream = useKnowledgeStore((s) => s.clearStream);

  const ask = useCallback(
    async (req: AnswerRequest): Promise<Answer | null> => {
      const id = newRequestId();
      latest.current = id;
      beginStream(id);
      setRequestId(id);
      setPending(true);
      setAnswer(null);
      setError(null);
      setLastRequest(req);
      try {
        const a = await api.knowledgeAnswer(req, id);
        if (latest.current === id) setAnswer(a);
        return a;
      } catch (e) {
        if (latest.current === id) setError(e);
        return null;
      } finally {
        if (latest.current === id) setPending(false);
        clearStream(id);
      }
    },
    [beginStream, clearStream],
  );

  const cancel = useCallback(() => {
    if (latest.current) void api.knowledgeAnswerCancel(latest.current).catch(() => undefined);
  }, []);

  const reset = useCallback(() => {
    latest.current = null;
    setAnswer(null);
    setError(null);
    setRequestId(null);
    setLastRequest(null);
    setPending(false);
  }, []);

  return { ask, cancel, pending, answer, error, streamText, reset, lastRequest, setAnswer };
}
