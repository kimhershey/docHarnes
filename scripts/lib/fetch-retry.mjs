// Shared download helper for the fetch-*.mjs scripts: retries transient failures
// (network errors, HTTP 408/425/429/5xx) with exponential backoff. Release downloads
// from github.com occasionally answer 502/504 or a truncated body for a few minutes.

const RETRY_STATUS = new Set([408, 425, 429, 500, 502, 503, 504]);

/**
 * `fetch` that retries transient failures. Resolves with the first non-retryable response
 * (OK or a client error such as 404) or rejects after `attempts` tries.
 * @param {string} url
 * @param {RequestInit} [init]
 * @param {{ attempts?: number, baseDelayMs?: number, log?: (msg: string) => void }} [options]
 */
export async function fetchWithRetry(url, init = {}, { attempts = 4, baseDelayMs = 2000, log = console.warn } = {}) {
  let lastError;
  for (let attempt = 1; attempt <= attempts; attempt++) {
    try {
      const res = await fetch(url, init);
      if (!RETRY_STATUS.has(res.status)) return res;
      lastError = new Error(`${url}: HTTP ${res.status}`);
      // Drain the body so the connection can be reused before retrying.
      await res.arrayBuffer().catch(() => {});
    } catch (e) {
      lastError = e instanceof Error ? e : new Error(String(e));
    }
    if (attempt === attempts) break;
    const delay = baseDelayMs * 2 ** (attempt - 1);
    log(`  retry ${attempt}/${attempts - 1} in ${delay / 1000}s — ${lastError.message}`);
    await new Promise((r) => setTimeout(r, delay));
  }
  throw lastError;
}

/** Downloads `url` fully into a Buffer, retrying transient failures (including a cut-off body). */
export async function downloadBuffer(url, init = {}, options = {}) {
  const { attempts = 4, baseDelayMs = 2000, log = console.warn } = options;
  let lastError;
  for (let attempt = 1; attempt <= attempts; attempt++) {
    const res = await fetchWithRetry(url, init, { attempts: attempts - attempt + 1, baseDelayMs, log });
    if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
    try {
      const buf = Buffer.from(await res.arrayBuffer());
      const expected = Number(res.headers.get("content-length") ?? -1);
      if (expected >= 0 && buf.length !== expected) {
        throw new Error(`${url}: body cut off (${buf.length} of ${expected} bytes)`);
      }
      return buf;
    } catch (e) {
      lastError = e instanceof Error ? e : new Error(String(e));
      if (attempt === attempts) break;
      const delay = baseDelayMs * 2 ** (attempt - 1);
      log(`  retry ${attempt}/${attempts - 1} in ${delay / 1000}s — ${lastError.message}`);
      await new Promise((r) => setTimeout(r, delay));
    }
  }
  throw lastError;
}
