// Shared download helpers for the fetch-*.mjs scripts.
//
// - fetchWithRetry / downloadBuffer retry transient failures (network errors, HTTP 408/425/429/5xx,
//   a cut-off body) with exponential backoff. DOCULENS_FETCH_ATTEMPTS overrides the attempt count.
// - downloadGithubAsset fetches a GitHub release asset through the REST asset endpoint first
//   (api.github.com → signed CDN redirect, authenticated with GH_TOKEN / GITHUB_TOKEN when set) and
//   falls back to the public /releases/download/ URL. The two paths fail independently: on
//   2026-11-08 github.com answered 504 for /releases/download/ for hours while the API kept working.

const RETRY_STATUS = new Set([408, 425, 429, 500, 502, 503, 504]);
const REDIRECT_STATUS = new Set([301, 302, 303, 307, 308]);

function defaultAttempts() {
  const n = Number.parseInt(process.env.DOCULENS_FETCH_ATTEMPTS ?? "", 10);
  return Number.isFinite(n) && n >= 1 ? n : 4;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Headers for api.github.com: User-Agent plus a bearer token when GH_TOKEN / GITHUB_TOKEN is set. */
export function githubHeaders(extra = {}, userAgent = "DocuLens-fetch") {
  const headers = { "User-Agent": userAgent, ...extra };
  const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;
  if (token) headers.Authorization = `Bearer ${token}`;
  return headers;
}

/**
 * `fetch` that retries transient failures. Resolves with the first non-retryable response
 * (OK, a redirect under `redirect: "manual"`, or a client error such as 404) or rejects after
 * `attempts` tries.
 * @param {string} url
 * @param {RequestInit} [init]
 * @param {{ attempts?: number, baseDelayMs?: number, log?: (msg: string) => void }} [options]
 */
export async function fetchWithRetry(url, init = {}, { attempts = defaultAttempts(), baseDelayMs = 2000, log = console.warn } = {}) {
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
    await sleep(delay);
  }
  throw lastError;
}

async function readBody(url, res) {
  const buf = Buffer.from(await res.arrayBuffer());
  const expected = Number(res.headers.get("content-length") ?? -1);
  if (expected >= 0 && buf.length !== expected) {
    throw new Error(`${url}: body cut off (${buf.length} of ${expected} bytes)`);
  }
  return buf;
}

/** Downloads `url` fully into a Buffer, retrying transient failures (including a cut-off body). */
export async function downloadBuffer(url, init = {}, options = {}) {
  const { attempts = defaultAttempts(), baseDelayMs = 2000, log = console.warn } = options;
  let lastError;
  for (let attempt = 1; attempt <= attempts; attempt++) {
    const res = await fetchWithRetry(url, init, { attempts: attempts - attempt + 1, baseDelayMs, log });
    if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
    try {
      return await readBody(url, res);
    } catch (e) {
      lastError = e instanceof Error ? e : new Error(String(e));
      if (attempt === attempts) break;
      const delay = baseDelayMs * 2 ** (attempt - 1);
      log(`  retry ${attempt}/${attempts - 1} in ${delay / 1000}s — ${lastError.message}`);
      await sleep(delay);
    }
  }
  throw lastError;
}

/**
 * Looks a release asset up through the GitHub API: `{ url, browser_download_url, size, name }`
 * or `null` when the lookup fails (rate limit, offline, unknown tag) — callers then use the
 * public download URL they already know.
 */
export async function githubReleaseAsset({ owner, repo, tag, name, userAgent = "DocuLens-fetch", log = console.warn }) {
  const api = `https://api.github.com/repos/${owner}/${repo}/releases/tags/${encodeURIComponent(tag)}`;
  try {
    const res = await fetchWithRetry(api, { headers: githubHeaders({ Accept: "application/vnd.github+json" }, userAgent) }, { attempts: 2, log });
    if (!res.ok) {
      log(`  release lookup ${api}: HTTP ${res.status} (using the direct download URL)`);
      return null;
    }
    const release = await res.json();
    return release.assets?.find((a) => a.name === name) ?? null;
  } catch (e) {
    log(`  release lookup failed: ${e instanceof Error ? e.message : e} (using the direct download URL)`);
    return null;
  }
}

/**
 * Downloads a GitHub release asset: the REST asset endpoint (`apiUrl`, redirects to the CDN) first,
 * then the public `browserUrl`. Returns the body as a Buffer.
 */
export async function downloadGithubAsset({ apiUrl, browserUrl, userAgent = "DocuLens-fetch", log = console.warn }) {
  if (apiUrl) {
    try {
      const res = await fetchWithRetry(
        apiUrl,
        { headers: githubHeaders({ Accept: "application/octet-stream" }, userAgent), redirect: "manual" },
        { log },
      );
      const location = res.headers.get("location");
      if (REDIRECT_STATUS.has(res.status) && location) {
        await res.arrayBuffer().catch(() => {});
        // The CDN URL is pre-signed: no Authorization header must go with it.
        return await downloadBuffer(location, { headers: { "User-Agent": userAgent } }, { log });
      }
      if (res.ok) return await readBody(apiUrl, res);
      throw new Error(`${apiUrl}: HTTP ${res.status}`);
    } catch (e) {
      if (!browserUrl) throw e;
      log(`  API download failed — ${e instanceof Error ? e.message : e}; trying ${browserUrl}`);
    }
  }
  if (!browserUrl) throw new Error("downloadGithubAsset: no URL to download");
  return downloadBuffer(browserUrl, { headers: { "User-Agent": userAgent } }, { log });
}
