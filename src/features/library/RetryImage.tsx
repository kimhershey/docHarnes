import { useEffect, useRef, useState, type ImgHTMLAttributes, type ReactNode } from "react";

/** Retries after this delay (the shell re-renders a purged page on the first 404). */
export const RETRY_DELAY_MS = 1500;
export const MAX_RETRIES = 3;

/**
 * `src` with a cache-busting `r=<attempt>` query, so the WebView does not serve the failed response again.
 * Inline (`data:` / `blob:`) sources have no cache to bust and would be corrupted by a query: retried as they are.
 */
export function retryUrl(src: string, attempt: number): string {
  if (attempt === 0 || /^(data|blob):/i.test(src)) return src;
  const [base, hash = ""] = src.split("#", 2);
  const sep = base!.includes("?") ? "&" : "?";
  return `${base}${sep}r=${attempt}${hash ? `#${hash}` : ""}`;
}

/**
 * A page / thumbnail image whose first load may fail: the page-cache LRU purge can remove rendered
 * pages, and the `doculens://` protocol answers the first request for a missing page with 404 while
 * queuing the re-render. The image is retried with a cache-busting query after ~1.5 s, up to three
 * times; then `fallback` (e.g. the "다시 렌더" affordance) is shown.
 */
export function RetryImage({ src, fallback = null, onGiveUp, ...rest }: Omit<ImgHTMLAttributes<HTMLImageElement>, "src" | "onError"> & { src: string; fallback?: ReactNode; onGiveUp?: () => void }) {
  const [attempt, setAttempt] = useState(0);
  const [failed, setFailed] = useState(false);
  const timer = useRef<number | null>(null);
  // A new source (page turn, re-render) starts over.
  useEffect(() => {
    setAttempt(0);
    setFailed(false);
    return () => {
      if (timer.current) window.clearTimeout(timer.current);
      timer.current = null;
    };
  }, [src]);
  const onError = () => {
    if (timer.current) return;
    if (attempt >= MAX_RETRIES) {
      setFailed(true);
      onGiveUp?.();
      return;
    }
    timer.current = window.setTimeout(() => {
      timer.current = null;
      setAttempt((a) => a + 1);
    }, RETRY_DELAY_MS);
  };
  if (failed) return <>{fallback}</>;
  return <img {...rest} src={retryUrl(src, attempt)} onError={onError} data-retry-attempt={attempt} />;
}
