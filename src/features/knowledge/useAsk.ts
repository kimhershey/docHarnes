import { useCallback, useState } from "react";

import { api } from "@/lib/ipc";
import type { Answer, AnswerRequest } from "@/lib/types";
import { newRequestId, useKnowledgeStore } from "@/stores/knowledge";

/** One grounded-answer request at a time, with the streamed text while it runs. */
export function useAsk() {
  const [requestId, setRequestId] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [answer, setAnswer] = useState<Answer | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [lastRequest, setLastRequest] = useState<AnswerRequest | null>(null);
  const streamText = useKnowledgeStore((s) => (requestId ? (s.streams[requestId] ?? "") : ""));
  const clearStream = useKnowledgeStore((s) => s.clearStream);

  const ask = useCallback(
    async (req: AnswerRequest): Promise<Answer | null> => {
      const id = newRequestId();
      setRequestId(id);
      setPending(true);
      setAnswer(null);
      setError(null);
      setLastRequest(req);
      try {
        const a = await api.knowledgeAnswer(req, id);
        setAnswer(a);
        return a;
      } catch (e) {
        setError(e);
        return null;
      } finally {
        setPending(false);
        clearStream(id);
      }
    },
    [clearStream],
  );

  const reset = useCallback(() => {
    setAnswer(null);
    setError(null);
    setRequestId(null);
    setLastRequest(null);
  }, []);

  return { ask, pending, answer, error, streamText, reset, lastRequest, setAnswer };
}
