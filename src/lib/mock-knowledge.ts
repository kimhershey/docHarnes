// Browser mock of the knowledge-base commands (P3): collections, versions,
// glossary, chunking, keyword search, canned grounded answers with streaming
// and simulated index / embedding jobs. Keeps its state per workspace in
// localStorage so Playwright can drive the whole UI without the desktop runtime.

import { looksLikeFollowUp } from "./follow-up";
import {
  createJob,
  currentWorkspacePath,
  emit,
  err,
  finishJob,
  loadDocs,
  saveDocs,
  simulate,
  tick,
  wsDocs,
  type MockDoc,
} from "./mock-backend";
import {
  EVENTS,
  type Answer,
  type AnswerDelta,
  type AnswerRequest,
  type AnswerSentence,
  type Chunk,
  type Citation,
  type Collection,
  type CollectionInput,
  type CollectionKind,
  type DocumentRole,
  type DocumentsChanged,
  type GlossaryEntry,
  type GlossaryInput,
  type KnowledgeChanged,
  type KnowledgeStatus,
  type SavedAnswer,
  type SearchHit,
  type SearchRequest,
  type SearchResponse,
  type SearchScope,
  type VersionGroup,
  type VersionInfo,
  type VersionInput,
} from "./types";
import { carriedText } from "./carried-text";

const KEY = "doculens.mock.knowledge";
const EMBEDDING_FLAG = "doculens.mock.embedding"; // "1" = EmbeddingGemma 2 installed + loaded

interface KbState {
  collections: Collection[];
  collectionDocs: Record<string, string[]>;
  glossary: GlossaryEntry[];
  versions: Record<string, Omit<VersionInfo, "title">>;
  answers: SavedAnswer[];
  chunks: Record<string, Chunk[]>;
  /** document id → hash of the page text that was chunked */
  indexed: Record<string, string>;
  embeddedModel: string | null;
  seq: number;
}

function empty(): KbState {
  return { collections: [], collectionDocs: {}, glossary: [], versions: {}, answers: [], chunks: {}, indexed: {}, embeddedModel: null, seq: 1 };
}

function loadAll(): Record<string, KbState> {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "{}") as Record<string, KbState>;
  } catch {
    return {};
  }
}

function load(): KbState {
  const ws = currentWorkspacePath() ?? "";
  const s = loadAll()[ws] ?? empty();
  // 0.4 state: every collection becomes a common reference group (migration 0010 default).
  for (const c of s.collections as (Collection & { watch_folder?: unknown })[]) {
    if (!c.kind) c.kind = "reference";
    if (c.project_id === undefined) c.project_id = null;
    delete c.watch_folder;
  }
  return s;
}

/** Project membership (work documents) without a static import cycle at module evaluation time. */
import * as mp from "./mock-projects";

/** The one `project` group of a project, created on first use (`project_reference_group`). */
export function projectReferenceGroup(projectId: string, name: string): Collection {
  const s = load();
  let c = s.collections.find((x) => x.kind === "project" && x.project_id === projectId);
  if (!c) {
    c = newCollection(s, name.trim() || projectId, "project", projectId);
    s.collections.push(c);
    save(s);
    changed("collections", [c.id]);
  }
  return collectionView(s, c);
}

/** Documents of a project's reference group (read-only: no group is created) and the project of each such document (0.8.1). */
export function projectReferenceDocIds(projectId: string): string[] {
  const s = load();
  const c = s.collections.find((x) => x.kind === "project" && x.project_id === projectId);
  return c ? [...(s.collectionDocs[c.id] ?? [])] : [];
}

/** Documents that sit in a reference group (common or a project's): reference material, not work to be filed. */
export function referenceDocIds(): Set<string> {
  return new Set(Object.values(load().collectionDocs).flat());
}

/** `project_update` renamed the project: its reference group follows. */
export function renameProjectReferenceGroup(projectId: string, name: string): void {
  const s = load();
  const c = s.collections.find((x) => x.kind === "project" && x.project_id === projectId);
  if (!c || c.name === name) return;
  c.name = name;
  c.updated_at = now();
  save(s);
  changed("collections", [c.id]);
}

/** `project_delete`: the project's reference group goes with it (its documents stay in the library). */
export function deleteProjectReferenceGroup(projectId: string): void {
  const s = load();
  const groups = s.collections.filter((x) => x.kind === "project" && x.project_id === projectId);
  if (groups.length === 0) return;
  s.collections = s.collections.filter((x) => !groups.includes(x));
  for (const g of groups) delete s.collectionDocs[g.id];
  save(s);
  changed("collections", groups.map((g) => g.id));
}

function newCollection(s: KbState, name: string, kind: CollectionKind, projectId: string | null): Collection {
  return { id: `col-${nextId(s)}`, name, description: null, color: null, active: true, kind, project_id: projectId, created_at: now(), updated_at: now(), doc_count: 0, chunk_count: 0 };
}

function save(state: KbState): void {
  const ws = currentWorkspacePath() ?? "";
  const all = loadAll();
  all[ws] = state;
  localStorage.setItem(KEY, JSON.stringify(all));
}

function changed(kind: KnowledgeChanged["kind"], ids: string[] = []): void {
  emit(EVENTS.knowledgeChanged, { kind, ids } satisfies KnowledgeChanged);
}

function nextId(s: KbState): number {
  return s.seq++;
}

function now(): string {
  return new Date().toISOString();
}

function embeddingInstalled(): boolean {
  return localStorage.getItem(EMBEDDING_FLAG) === "1";
}

/** Settings override → registry default (mirrors knowledge_bridge::embedding_model). */
function embeddingModelId(): string {
  try {
    const settings = JSON.parse(localStorage.getItem("doculens.mock.settings") ?? "{}") as { knowledge?: { embedding_model?: string | null } };
    return settings.knowledge?.embedding_model || "embed-gemma2-q8";
  } catch {
    return "embed-gemma2-q8";
  }
}

// ---------------------------------------------------------------------------
// Chunking (paragraphs of the mock page text) + indexing
// ---------------------------------------------------------------------------

function textHash(doc: MockDoc): string {
  return doc.pages.map((p) => `${p.page_no}:${p.text ?? ""}`).join("\u0000");
}

function chunkDoc(s: KbState, doc: MockDoc): Chunk[] {
  const out: Chunk[] = [];
  for (const page of doc.pages) {
    const text = page.text?.trim();
    if (!text) continue;
    const paras = text.split(/\n\s*\n/).map((p) => p.trim()).filter(Boolean);
    let heading: string | null = null;
    for (const para of paras) {
      const first = para.split("\n")[0] ?? "";
      if (first.length <= 40 && para.split("\n").length === 1 && !/[.!?]$/.test(first)) {
        heading = first;
        continue;
      }
      out.push({
        id: nextId(s),
        document_id: doc.id,
        chunk_index: out.length,
        page_start: page.page_no,
        page_end: page.page_no,
        kind: /\t|\|/.test(para) ? "table" : "text",
        heading_path: heading,
        clause: (para.match(/제\s?\d+\s?조/) ?? [null])[0],
        text: heading ? `${heading}\n${para}` : para,
        token_estimate: Math.ceil(para.length * 0.75),
        embedded: false,
      });
    }
  }
  return out;
}

function needsIndex(s: KbState, doc: MockDoc): boolean {
  if (!doc.has_text_layer) return false;
  return s.indexed[doc.id] !== textHash(doc);
}

function indexNow(s: KbState, doc: MockDoc): number {
  const chunks = chunkDoc(s, doc);
  s.chunks[doc.id] = chunks;
  s.indexed[doc.id] = textHash(doc);
  return chunks.length;
}

let indexRunning = false;
let embedRunning = false;

async function indexJob(ids: string[] | null, force: boolean): Promise<string | null> {
  if (indexRunning) return null;
  const s = load();
  const docs = wsDocs().filter((d) => (ids ? ids.includes(d.id) : true)).filter((d) => force || ids !== null || needsIndex(s, d) || !(d.id in s.indexed));
  const job = createJob("knowledge_index", ids ? `${ids.length} documents` : "library");
  indexRunning = true;
  void (async () => {
    let indexed = 0;
    let chunks = 0;
    let unchanged = 0;
    let noText = 0;
    const done: string[] = [];
    const ok = await simulate(job.id, Math.max(1, docs.length), 120, (i) => {
      const d = docs[i - 1];
      if (!d) return;
      const st = load();
      if (!d.has_text_layer || d.pages.every((p) => !p.text)) {
        st.chunks[d.id] = [];
        st.indexed[d.id] = textHash(d);
        noText += 1;
      } else if (!force && st.indexed[d.id] === textHash(d)) {
        unchanged += 1;
      } else {
        chunks += indexNow(st, d);
        indexed += 1;
      }
      done.push(d.id);
      save(st);
    });
    indexRunning = false;
    changed("indexed", done);
    if (!ok) finishJob(job.id, { ok: false, error: "cancelled" });
    else finishJob(job.id, { ok: true, detail: `indexed ${indexed} (${chunks} chunks) / unchanged ${unchanged} / no text ${noText} / failed 0` });
    if (ok && embeddingInstalled()) void embedJob(false);
  })();
  return job.id;
}

async function embedJob(loadModel: boolean): Promise<string | null> {
  if (embedRunning) return null;
  if (!embeddingInstalled()) {
    if (loadModel) throw err("invalid", "embedding_model_not_installed");
    return null;
  }
  const s = load();
  const pending = Object.values(s.chunks).flat().filter((c) => !c.embedded);
  if (pending.length === 0 && s.embeddedModel === embeddingModelId()) return null;
  const job = createJob("knowledge_embed", embeddingModelId());
  embedRunning = true;
  void (async () => {
    const steps = Math.max(1, Math.ceil(pending.length / 4));
    const ok = await simulate(job.id, steps, 150, (i) => {
      const st = load();
      const todo = Object.values(st.chunks).flat().filter((c) => !c.embedded).slice(0, 4);
      for (const c of todo) c.embedded = true;
      st.embeddedModel = embeddingModelId();
      save(st);
      void i;
    });
    embedRunning = false;
    changed("embedded");
    if (!ok) finishJob(job.id, { ok: false, error: "cancelled" });
    else finishJob(job.id, { ok: true, detail: `embedded ${pending.length} / failed 0 / remaining 0` });
  })();
  return job.id;
}

/** Called by the document import mock after a batch. */
export function autoIndex(): void {
  const s = load();
  if (wsDocs().some((d) => needsIndex(s, d))) void indexJob(null, false);
}

/** Called by the document import mock when the batch targets a collection. */
export function attachToCollection(collectionId: string, docIds: string[], role: DocumentRole): void {
  const s = load();
  const list = s.collectionDocs[collectionId] ?? [];
  for (const id of docIds) if (!list.includes(id)) list.push(id);
  s.collectionDocs[collectionId] = list;
  save(s);
  if (role !== "work") setRole(docIds, role);
  changed("collections", [collectionId]);
}

function setRole(ids: string[], role: DocumentRole): number {
  const all = loadDocs();
  let n = 0;
  for (const d of all) {
    if (ids.includes(d.id)) {
      d.role = role;
      d.updated_at = now();
      n += 1;
    }
  }
  saveDocs(all);
  emit(EVENTS.documentsChanged, { kind: "updated", ids } satisfies DocumentsChanged);
  return n;
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

function docTitle(d: MockDoc): string {
  return d.title?.trim() ? d.title : d.original_name;
}

function hitFromChunk(c: Chunk, d: MockDoc | undefined, channels: SearchHit["channels"] = []): SearchHit {
  return {
    chunk_id: c.id,
    document_id: c.document_id,
    title: d ? docTitle(d) : c.document_id,
    rel_path: d?.rel_path ?? "",
    role: d?.role ?? "work",
    doc_type: d?.doc_type ?? null,
    page_start: c.page_start,
    page_end: c.page_end,
    kind: c.kind,
    heading_path: c.heading_path,
    clause: c.clause,
    text: c.text,
    snippet: "",
    highlights: [],
    score: 1,
    channels,
    vector_similarity: null,
  };
}

/**
 * The documents a project scope allows (core `scope_doc_ids`, 0.5.0 plan §2): the project's work documents
 * (or the explicitly listed `documents`) ∪ the project's reference group ∪ the active common reference groups.
 */
function projectScopeDocs(s: KbState, projectId: string, documents: string[] | undefined): { allowed: Set<string>; work: Set<string> } {
  const work = new Set(documents?.length ? documents : mp.projectDocIds(projectId));
  const allowed = new Set(work);
  for (const c of s.collections) {
    const own = c.kind === "project" && c.project_id === projectId;
    const common = c.kind === "reference" && c.active;
    if (!own && !common) continue;
    for (const id of s.collectionDocs[c.id] ?? []) allowed.add(id);
  }
  return { allowed, work };
}

function allowedDocs(s: KbState, scope: SearchScope | undefined): Set<string> | null {
  if (!scope) return null;
  let allowed: Set<string> | null = null;
  const intersect = (set: Set<string>) => {
    allowed = allowed ? new Set([...allowed].filter((x) => set.has(x))) : set;
  };
  let work = new Set<string>();
  if (scope.project_id) {
    const p = projectScopeDocs(s, scope.project_id, scope.documents);
    work = p.work;
    intersect(p.allowed);
  } else if (scope.documents?.length) intersect(new Set(scope.documents));
  if (scope.collections?.length) intersect(new Set(scope.collections.flatMap((c) => s.collectionDocs[c] ?? [])));
  if (scope.roles?.length) intersect(new Set(wsDocs().filter((d) => scope.roles?.includes(d.role)).map((d) => d.id)));
  if (scope.folder) intersect(new Set(wsDocs().filter((d) => d.rel_path.startsWith(`${scope.folder}/`)).map((d) => d.id)));
  const excluded = new Set<string>();
  if (scope.latest_only !== false) {
    // An edition is superseded only while its group still has a latest edition whose document is there
    // (`repo::scope_doc_ids`, KNO-7): once that one is cleared or deleted, the editions that are left are all
    // there is — hiding them would leave the title unanswerable.
    const live = new Set(wsDocs().map((d) => d.id));
    const withLatest = new Set(Object.values(s.versions).filter((v) => v.is_latest && live.has(v.document_id)).map((v) => v.group_key));
    for (const [id, v] of Object.entries(s.versions)) if (!v.is_latest && withLatest.has(v.group_key)) excluded.add(id);
  }
  if (!scope.collections?.length && scope.active_collections_only !== false) {
    const inactive = new Set(s.collections.filter((c) => !c.active).map((c) => c.id));
    for (const [cid, ids] of Object.entries(s.collectionDocs)) {
      if (!inactive.has(cid)) continue;
      for (const id of ids) {
        // A project's own work document stays in scope whatever groups it sits in.
        if (work.has(id)) continue;
        const inActive = Object.entries(s.collectionDocs).some(([c2, ids2]) => !inactive.has(c2) && ids2.includes(id));
        if (!inActive) excluded.add(id);
      }
    }
  }
  if (excluded.size === 0) return allowed;
  const base: Set<string> = allowed ?? new Set(wsDocs().map((d) => d.id));
  return new Set([...base].filter((x) => !excluded.has(x)));
}

function highlights(text: string, needles: string[]): [number, number][] {
  const lower = text.toLowerCase();
  const out: [number, number][] = [];
  for (const n of needles.filter(Boolean).sort((a, b) => b.length - a.length)) {
    const nl = n.toLowerCase();
    let i = 0;
    while ((i = lower.indexOf(nl, i)) >= 0) {
      const end = i + nl.length;
      if (!out.some(([a, b]) => i < b && end > a)) out.push([i, end]);
      i = end;
    }
  }
  return out.sort((a, b) => a[0] - b[0]);
}

function snippet(text: string, needles: string[]): { snippet: string; highlights: [number, number][] } {
  const hl = highlights(text, needles);
  if (text.length <= 240) return { snippet: text, highlights: hl };
  const first = hl[0]?.[0] ?? 0;
  const start = Math.max(0, first - 80);
  const end = Math.min(text.length, start + 240);
  const prefix = start > 0 ? "…" : "";
  const s = prefix + text.slice(start, end) + (end < text.length ? "…" : "");
  const off = prefix.length - start;
  return { snippet: s, highlights: hl.filter(([a, b]) => a >= start && b <= end).map(([a, b]) => [a + off, b + off] as [number, number]) };
}

function search(req: SearchRequest): SearchResponse {
  const started = performance.now();
  const s = load();
  const query = req.query.trim();
  const resp: SearchResponse = { hits: [], query_terms: [], expanded_terms: [], used_vector: false, used_like_fallback: false, elapsed_ms: 0, total_candidates: 0 };
  if (!query) return resp;
  const terms = query.split(/[\s,.?!]+/).map((t) => t.toLowerCase()).filter(Boolean);
  resp.query_terms = terms;
  if (req.expand_glossary !== false) {
    for (const t of terms) {
      const g = s.glossary.find((e) => e.term.toLowerCase() === t || e.synonyms.some((x) => x.toLowerCase() === t));
      if (g) resp.expanded_terms.push(...[g.term, ...g.synonyms].filter((x) => x.toLowerCase() !== t));
    }
  }
  const needles = [...terms, ...resp.expanded_terms.map((x) => x.toLowerCase())];
  const allowed = allowedDocs(s, req.scope);
  const docs = new Map(wsDocs().map((d) => [d.id, d]));
  const mode = req.mode ?? "hybrid";
  const semantic = mode !== "keyword" && embeddingInstalled();
  const scored: { chunk: Chunk; score: number; channels: SearchHit["channels"]; sim: number | null }[] = [];
  for (const chunks of Object.values(s.chunks)) {
    for (const c of chunks) {
      if (allowed && !allowed.has(c.document_id)) continue;
      if (!docs.has(c.document_id)) continue;
      const lower = c.text.toLowerCase();
      const matched = needles.filter((n) => lower.includes(n)).length;
      let score = matched / Math.max(1, terms.length);
      const channels: SearchHit["channels"] = [];
      if (mode !== "semantic" && matched > 0) channels.push(terms.some((t) => t.length >= 3 && lower.includes(t)) ? "trigram" : "like", "morph");
      let sim: number | null = null;
      if (semantic) {
        const bigrams = new Set(query.replace(/\s/g, "").match(/.{2}/g) ?? []);
        const overlap = [...bigrams].filter((b) => lower.includes(b.toLowerCase())).length;
        sim = Math.min(0.99, 0.3 + overlap / Math.max(1, bigrams.size) * 0.7);
        if (overlap > 0) {
          channels.push("vector");
          score += sim * 0.5;
        }
      }
      if (channels.length === 0) continue;
      scored.push({ chunk: c, score, channels: [...new Set(channels)] as SearchHit["channels"], sim: channels.includes("vector") ? sim : null });
    }
  }
  resp.used_vector = semantic && scored.some((h) => h.channels.includes("vector"));
  resp.used_like_fallback = scored.length > 0 && scored.every((h) => h.channels.includes("like") && !h.channels.includes("trigram"));
  scored.sort((a, b) => b.score - a.score || a.chunk.id - b.chunk.id);
  resp.total_candidates = scored.length;
  const perDoc = new Map<string, number>();
  const limit = req.limit ?? 20;
  for (const h of scored) {
    const n = perDoc.get(h.chunk.document_id) ?? 0;
    if ((req.per_document ?? 3) > 0 && n >= (req.per_document ?? 3)) continue;
    perDoc.set(h.chunk.document_id, n + 1);
    const doc = docs.get(h.chunk.document_id);
    if (!doc) continue;
    const sn = snippet(h.chunk.text, needles);
    resp.hits.push({
      chunk_id: h.chunk.id,
      document_id: doc.id,
      title: docTitle(doc),
      rel_path: doc.rel_path,
      role: doc.role,
      doc_type: doc.doc_type,
      page_start: h.chunk.page_start,
      page_end: h.chunk.page_end,
      kind: h.chunk.kind,
      heading_path: h.chunk.heading_path,
      clause: h.chunk.clause,
      text: h.chunk.text,
      snippet: sn.snippet,
      highlights: sn.highlights,
      score: Number(h.score.toFixed(4)),
      channels: h.channels,
      vector_similarity: h.sim,
    });
    if (resp.hits.length >= limit) break;
  }
  resp.elapsed_ms = Math.round(performance.now() - started);
  return resp;
}

// ---------------------------------------------------------------------------
// Answers (canned, streamed)
// ---------------------------------------------------------------------------

function firstSentence(text: string): string {
  const body = text.includes("\n") ? text.slice(text.indexOf("\n") + 1) : text;
  const m = body.match(/^[^.!?\n]{1,160}[.!?]?/);
  return (m?.[0] ?? body.slice(0, 120)).trim();
}

function citation(index: number, h: SearchHit): Citation {
  return {
    index,
    chunk_id: h.chunk_id,
    document_id: h.document_id,
    title: h.title,
    page_start: h.page_start,
    page_end: h.page_end,
    heading_path: h.heading_path,
    clause: h.clause,
    score: h.score,
    excerpt: h.text.replace(/\s+/g, " ").slice(0, 160),
  };
}

const cancelledAnswers = new Set<string>();

/** Stand-in for the model's context: the characters of a text-only request (다듬기 · 교정 · 번역) the mock reads. */
export const TEXT_ONLY_CHARS = 6000;

/** Streams the text in small deltas; returns false when the request was cancelled midway. */
async function stream(requestId: string, text: string): Promise<boolean> {
  // Six characters at a time; a long text (a pasted document) in at most ~200 pieces, so it does not take minutes.
  const step = Math.max(6, Math.ceil(text.length / 200));
  for (let i = 0; i < text.length; i += step) {
    if (cancelledAnswers.has(requestId)) {
      cancelledAnswers.delete(requestId);
      emit(EVENTS.knowledgeAnswer, { request_id: requestId, delta: "", done: true } satisfies AnswerDelta);
      return false;
    }
    emit(EVENTS.knowledgeAnswer, { request_id: requestId, delta: text.slice(i, i + step), done: false } satisfies AnswerDelta);
    await new Promise((r) => setTimeout(r, tick(20)));
  }
  emit(EVENTS.knowledgeAnswer, { request_id: requestId, delta: "", done: true } satisfies AnswerDelta);
  return true;
}

function parseSentences(text: string, max: number): AnswerSentence[] {
  return text
    .split(/\n+/)
    .flatMap((line) => line.split(/(?<=[.!?]\s*(?:\[[\d,\s]+\])*)\s+(?=\S)/))
    .map((p) => p.trim())
    .filter(Boolean)
    .map((p) => ({
      text: p,
      citations: [...p.matchAll(/\[(\d+(?:\s*,\s*\d+)*)\]/g)].flatMap((m) => (m[1] ?? "").split(",").map((n) => Number(n.trim()))).filter((n) => n > 0 && n <= max),
    }));
}

function saveAnswer(s: KbState, a: Answer): number {
  const id = nextId(s);
  s.answers.unshift({
    id,
    kind: a.kind,
    question: a.question,
    text: a.text,
    sentences: a.sentences,
    sources: a.sources,
    no_evidence: a.no_evidence,
    used_general_knowledge: a.used_general_knowledge,
    model: a.model,
    document_id: a.document_id,
    feedback: null,
    feedback_note: null,
    created_at: now(),
  });
  save(s);
  changed("answers", [String(id)]);
  return id;
}

async function answer(req: AnswerRequest, requestId: string): Promise<Answer> {
  const started = performance.now();
  const s = load();
  const ko = !/[A-Za-z]{3,}/.test(req.question) || /[가-힣]/.test(req.question) || req.language === "ko";
  const kind = req.kind ?? "answer";
  const base: Answer = {
    id: null,
    kind,
    question: kind === "explain" && !req.question.trim() ? (req.selection ?? "") : req.question,
    text: "",
    sentences: [],
    sources: [],
    no_evidence: false,
    used_general_knowledge: false,
    conflicts: null,
    model: "gemma4-e4b-q5km (mock)",
    elapsed_ms: 0,
    prompt_tokens: 0,
    completion_tokens: 0,
    document_id: req.document_id ?? null,
    created_at: now(),
    feedback: null,
  };
  if (req.style === "chat") {
    // Small talk (0.8.0): a short reply — nothing is looked up, cited or kept in the answer history.
    base.text = ko
      ? "저는 이 PC 안에서만 동작하는 DocuLens 어시스턴트입니다. 문서에 대해 물어보시거나 가져오기·정리·추출·요약 같은 작업을 말씀해 주세요."
      : "I am the DocuLens assistant and run on this PC only. Ask me about your documents, or tell me what to do — import, organise, extract, summarise.";
    if (!(await stream(requestId, base.text))) throw err("cancelled", "cancelled");
    base.sentences = [{ text: base.text, citations: [] }];
    base.prompt_tokens = 200 + req.question.length;
    base.completion_tokens = Math.ceil(base.text.length / 2);
    base.elapsed_ms = Math.round(performance.now() - started);
    return base;
  }
  if (req.retrieve === false) {
    // Text-only request: the pasted text is the material — nothing is looked up or cited (the "model" tidies it).
    const whole = carriedText(req.question) ?? req.question;
    // A text that does not fit the model's context is read from its beginning only, and the answer says so
    // (`Answer.truncated`, KNO-3).
    const body = whole.length > TEXT_ONLY_CHARS ? whole.slice(0, TEXT_ONLY_CHARS) : whole;
    if (body.length < whole.length) base.truncated = "input";
    base.text = body
      .replace(/\s+/g, " ")
      .replace(/\s*([.。])\s*/g, "$1 ")
      .trim();
    if (!(await stream(requestId, base.text))) throw err("cancelled", "cancelled");
    base.sentences = parseSentences(base.text, 0);
    base.prompt_tokens = 300 + base.text.length;
    base.completion_tokens = Math.ceil(base.text.length / 2);
    base.elapsed_ms = Math.round(performance.now() - started);
    if (req.save !== false) base.id = saveAnswer(load(), base);
    return base;
  }
  if (kind === "summary" || kind === "compare") {
    const ids = kind === "compare" ? [req.document_id, req.compare_document_id].filter((x): x is string => !!x) : [req.document_id].filter((x): x is string => !!x);
    const chunks = ids.flatMap((id) => s.chunks[id] ?? []);
    if (chunks.length === 0) throw err("not_found", "no_text");
    const docs = new Map(wsDocs().map((d) => [d.id, d]));
    base.sources = chunks.slice(0, 12).map((c, i) => citation(i + 1, hitFromChunk(c, docs.get(c.document_id))));
    const mode = req.summary_mode ?? "key_points";
    const lines =
      kind === "compare"
        ? [ko ? "| 항목 | 문서 A | 문서 B |" : "| Item | Document A | Document B |", "|---|---|---|", ...base.sources.slice(0, 4).map((c, i) => `| ${c.clause ?? c.heading_path ?? `#${i + 1}`} | ${firstSentence(c.excerpt)} [${c.index}] | - |`)]
        : mode === "one_line"
          ? [`${firstSentence(base.sources[0]?.excerpt ?? "")} [1]`]
          : base.sources.slice(0, mode === "one_page" ? 8 : 5).map((c) => `- ${firstSentence(c.excerpt)} [${c.index}]`);
    base.text = lines.join("\n");
    base.question = req.question.trim() || (ko ? (kind === "compare" ? "두 문서 비교" : "문서 요약") : kind === "compare" ? "Compare documents" : "Document summary");
  } else {
    // Like the core: a follow-up ("그건 누가 작성했어?") is retrieved together with the previous question.
    const previous = [...(req.history ?? [])].reverse().find((t) => t.role === "user")?.text;
    const query =
      kind === "explain" ? `${req.selection ?? ""} ${req.question}`.trim() : previous && looksLikeFollowUp(req.question) ? `${req.question} ${previous}` : req.question;
    const scope: SearchScope = { ...(req.scope ?? {}) };
    if (kind === "explain" && req.document_id && scope.documents?.length && !scope.documents.includes(req.document_id)) scope.documents = [...scope.documents, req.document_id];
    // Evidence kept from the previous answer counts only inside this request's scope (KNO-1): a chunk of a
    // document the scope leaves out is dropped, so the answer can still end in "no evidence".
    const inScope = allowedDocs(s, scope);
    const kept = (req.keep_chunks ?? []).length
      ? Object.values(s.chunks).flat().filter((c) => req.keep_chunks?.includes(c.id) && (!inScope || inScope.has(c.document_id))).map((c) => hitFromChunk(c, wsDocs().find((x) => x.id === c.document_id), ["morph"]))
      : [];
    const fresh = query.trim() ? search({ query, scope, limit: (req.top_k ?? 8) * 2, per_document: 4 }).hits : [];
    const hits = [...kept, ...fresh.filter((h) => !kept.some((k) => k.chunk_id === h.chunk_id))];
    const top = hits.slice(0, req.top_k ?? 8);
    base.sources = top.map((h, i) => citation(i + 1, h));
    if (top.length === 0) {
      base.no_evidence = true;
      // A question the planner routed as "not about the documents" (0.8.0) is answered either way.
      if (!req.allow_general && req.style !== "general") {
        base.text = ko ? "참고 자료에 근거가 없습니다." : "No evidence was found in the reference material.";
        base.sentences = [{ text: base.text, citations: [] }];
        base.elapsed_ms = Math.round(performance.now() - started);
        if (req.save !== false) base.id = saveAnswer(s, base);
        emit(EVENTS.knowledgeAnswer, { request_id: requestId, delta: "", done: true } satisfies AnswerDelta);
        return base;
      }
      base.used_general_knowledge = true;
      base.text = ko ? `(일반 지식 기반 답변) "${req.question}"에 대한 근거 문서가 없어 일반적인 설명만 드립니다.` : `(General-knowledge answer) No document covers "${req.question}"; this is a general explanation only.`;
    } else {
      const lead = kind === "explain" ? (ko ? "선택한 내용은 다음 문서와 관련이 있습니다. " : "The selected passage relates to the following documents. ") : "";
      const parts = top.slice(0, 3).map((h, i) => `${firstSentence(h.text)} [${i + 1}]`);
      base.text = `${lead}${parts.join(" ")}`;
      const docsCited = new Set(top.map((h) => h.document_id));
      if (docsCited.size > 1 && /한도|기한|금액/.test(query)) {
        base.text += ko ? "\n\n상충: 서로 다른 문서가 다른 값을 제시합니다 [1][2]." : "\n\nConflict: the documents state different values [1][2].";
        base.conflicts = ko ? "서로 다른 문서가 다른 값을 제시합니다 [1][2]." : "the documents state different values [1][2].";
      }
    }
  }
  if (!(await stream(requestId, base.text))) throw err("cancelled", "cancelled");
  base.sentences = parseSentences(base.text.replace(/^상충:.*$/m, "").replace(/^Conflict:.*$/m, ""), base.sources.length);
  base.prompt_tokens = 400 + base.sources.length * 300;
  base.completion_tokens = Math.ceil(base.text.length / 2);
  base.elapsed_ms = Math.round(performance.now() - started);
  if (req.save !== false) base.id = saveAnswer(load(), base);
  return base;
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

function collectionView(s: KbState, c: Collection): Collection {
  const ids = s.collectionDocs[c.id] ?? [];
  return { ...c, doc_count: ids.length, chunk_count: ids.reduce((n, id) => n + (s.chunks[id]?.length ?? 0), 0) };
}

function status(): KnowledgeStatus {
  const s = load();
  const docs = wsDocs();
  const chunks = Object.values(s.chunks).flat();
  const indexedIds = new Set(Object.keys(s.indexed));
  return {
    documents_total: docs.length,
    documents_indexed: docs.filter((d) => indexedIds.has(d.id) && s.indexed[d.id] === textHash(d)).length,
    documents_pending: docs.filter((d) => d.has_text_layer && s.indexed[d.id] !== textHash(d)).length,
    documents_no_text: docs.filter((d) => !d.has_text_layer).length,
    documents_failed: 0,
    knowledge_documents: docs.filter((d) => d.role === "knowledge").length,
    chunks_total: chunks.length,
    chunks_embedded: chunks.filter((c) => c.embedded).length,
    embedding_model: s.embeddedModel,
    embedding_dim: s.embeddedModel ? 768 : null,
    tokenizer: { kind: "ko-dic", path: "C:\\Users\\me\\AppData\\Local\\DocuLens\\tokenizers\\ko-dic", version: "mecab-ko-dic-2.1.1-20180720" },
    collections: s.collections.filter((c) => c.kind === "reference").length,
    glossary_terms: s.glossary.length,
    answers: s.answers.length,
    answers_helpful: s.answers.filter((a) => a.feedback === 1).length,
    answers_wrong: s.answers.filter((a) => a.feedback === -1).length,
    embedding: { model: embeddingModelId(), installed: embeddingInstalled(), engine_running: embeddingInstalled(), loaded: embeddingInstalled() },
    index_running: indexRunning,
    embed_running: embedRunning,
  };
}

export async function mockKnowledge(cmd: string, a: Record<string, any>): Promise<unknown> { // eslint-disable-line @typescript-eslint/no-explicit-any
  const s = load();
  switch (cmd) {
    case "knowledge_status":
      return status();
    case "knowledge_collections":
      return s.collections.map((c) => collectionView(s, c)).sort((x, y) => x.name.localeCompare(y.name));
    case "knowledge_collection_save": {
      const input = a.input as CollectionInput;
      const name = input.name.trim();
      if (!name) throw err("invalid", "collection_name_empty");
      let c = input.id ? s.collections.find((x) => x.id === input.id) : undefined;
      // A project's group is named after the project; names are unique among the common reference groups.
      const kind: CollectionKind = c ? c.kind : input.project_id ? "project" : "reference";
      if (kind === "reference" && s.collections.some((x) => x.kind === "reference" && x.name.toLowerCase() === name.toLowerCase() && x.id !== input.id)) throw err("invalid", "collection_name_taken");
      if (!c) {
        c = newCollection(s, name, kind, kind === "project" ? (input.project_id ?? null) : null);
        s.collections.push(c);
      }
      c.name = name;
      c.description = input.description ?? null;
      c.color = input.color ?? null;
      c.active = input.active ?? true;
      c.updated_at = now();
      save(s);
      changed("collections", [c.id]);
      return collectionView(s, c);
    }
    case "knowledge_collection_delete": {
      const id = String(a.id);
      const before = s.collections.length;
      s.collections = s.collections.filter((c) => c.id !== id);
      delete s.collectionDocs[id];
      save(s);
      changed("collections", [id]);
      return s.collections.length < before;
    }
    case "knowledge_collection_docs": {
      const ids = s.collectionDocs[String(a.id)] ?? [];
      const { docView } = await import("./mock-backend");
      return wsDocs().filter((d) => ids.includes(d.id)).map((d) => docView(d));
    }
    case "knowledge_collection_add": {
      const { id, doc_ids, set_role } = a.args as { id: string; doc_ids: string[]; set_role: boolean };
      const list = s.collectionDocs[id] ?? [];
      let n = 0;
      for (const d of doc_ids) if (!list.includes(d)) { list.push(d); n += 1; }
      s.collectionDocs[id] = list;
      save(s);
      if (set_role) setRole(doc_ids, "knowledge");
      changed("collections", [id]);
      return n;
    }
    case "knowledge_collection_remove": {
      const { id, doc_ids } = a.args as { id: string; doc_ids: string[] };
      const before = (s.collectionDocs[id] ?? []).length;
      s.collectionDocs[id] = (s.collectionDocs[id] ?? []).filter((d) => !doc_ids.includes(d));
      save(s);
      changed("collections", [id]);
      return before - s.collectionDocs[id].length;
    }
    case "knowledge_document_collections":
      return s.collections.filter((c) => (s.collectionDocs[c.id] ?? []).includes(String(a.id))).map((c) => [c.id, c.name]);
    case "knowledge_set_role": {
      const n = setRole(a.ids as string[], a.role as DocumentRole);
      changed("status");
      return n;
    }
    case "knowledge_index":
      return indexJob((a.ids as string[] | null) ?? null, Boolean(a.force));
    case "knowledge_embed":
      return embedJob(a.loadModel !== false);
    case "knowledge_reset_embeddings":
      for (const c of Object.values(s.chunks).flat()) c.embedded = false;
      s.embeddedModel = null;
      save(s);
      changed("embedded");
      return undefined;
    case "knowledge_search":
      return search(a.req as SearchRequest);
    case "knowledge_chunks":
      return s.chunks[String(a.documentId)] ?? [];
    case "knowledge_answer":
      return answer(a.req as AnswerRequest, String(a.requestId ?? `req-${Date.now()}`));
    case "knowledge_answer_cancel":
      cancelledAnswers.add(String(a.requestId));
      return true;
    case "knowledge_answers":
      return s.answers.filter((x) => (a.kind ? x.kind === a.kind : true) && (a.documentId ? x.document_id === a.documentId : true)).slice(0, Number(a.limit ?? 50));
    case "knowledge_answer_feedback": {
      const x = s.answers.find((y) => y.id === Number(a.id));
      if (!x) return false;
      x.feedback = a.feedback ?? null;
      x.feedback_note = a.note ?? null;
      save(s);
      changed("answers", [String(x.id)]);
      return true;
    }
    case "knowledge_answer_delete": {
      const before = s.answers.length;
      s.answers = s.answers.filter((y) => y.id !== Number(a.id));
      save(s);
      changed("answers", [String(a.id)]);
      return s.answers.length < before;
    }
    case "knowledge_glossary":
      return [...s.glossary].sort((x, y) => x.term.localeCompare(y.term));
    case "knowledge_glossary_save": {
      const input = a.input as GlossaryInput;
      const term = input.term.trim();
      if (!term) throw err("invalid", "glossary_term_empty");
      const synonyms = [...new Set((input.synonyms ?? []).map((x) => x.trim()).filter((x) => x && x !== term))];
      let g = input.id ? s.glossary.find((x) => x.id === input.id) : s.glossary.find((x) => x.term.toLowerCase() === term.toLowerCase());
      if (input.id && s.glossary.some((x) => x.id !== input.id && x.term.toLowerCase() === term.toLowerCase())) throw err("invalid", "glossary_term_taken");
      if (!g) {
        g = { id: nextId(s), term, definition: null, synonyms: [], created_at: now(), updated_at: now() };
        s.glossary.push(g);
      }
      if (input.id) g.term = term;
      g.definition = input.definition ?? null;
      g.synonyms = synonyms;
      g.updated_at = now();
      save(s);
      changed("glossary", [String(g.id)]);
      return g;
    }
    case "knowledge_glossary_delete": {
      const before = s.glossary.length;
      s.glossary = s.glossary.filter((x) => x.id !== Number(a.id));
      save(s);
      changed("glossary", [String(a.id)]);
      return s.glossary.length < before;
    }
    case "knowledge_versions": {
      const docs = new Map(wsDocs().map((d) => [d.id, d]));
      const groups = new Map<string, VersionGroup>();
      for (const v of Object.values(s.versions)) {
        const d = docs.get(v.document_id);
        if (!d) continue;
        const g = groups.get(v.group_key) ?? { group_key: v.group_key, documents: [] };
        g.documents.push({ ...v, title: docTitle(d) });
        groups.set(v.group_key, g);
      }
      return [...groups.values()].map((g) => ({ ...g, documents: g.documents.sort((x, y) => Number(y.is_latest) - Number(x.is_latest)) })).sort((x, y) => x.group_key.localeCompare(y.group_key));
    }
    case "knowledge_version_set": {
      const input = a.input as VersionInput;
      const group = input.group_key.trim();
      if (!group) throw err("invalid", "version_group_empty");
      const v: Omit<VersionInfo, "title"> = { document_id: input.document_id, group_key: group, version_label: input.version_label ?? null, effective_date: input.effective_date ?? null, is_latest: input.is_latest ?? true, note: input.note ?? null };
      s.versions[input.document_id] = v;
      if (v.is_latest) for (const other of Object.values(s.versions)) if (other.group_key === group && other.document_id !== v.document_id) other.is_latest = false;
      save(s);
      changed("versions", [v.document_id]);
      const d = wsDocs().find((x) => x.id === v.document_id);
      return { ...v, title: d ? docTitle(d) : v.document_id };
    }
    case "knowledge_version_clear": {
      const cleared = s.versions[String(a.documentId)];
      const had = cleared !== undefined;
      delete s.versions[String(a.documentId)];
      // `repo::clear_version`: when the group's latest edition leaves, the newest one that is left takes over
      // (effective date, then import time) — the older ones stay behind a current edition.
      if (cleared?.is_latest) {
        const docs = new Map(wsDocs().map((d) => [d.id, d]));
        const left = Object.values(s.versions).filter((v) => v.group_key === cleared.group_key && docs.has(v.document_id));
        if (left.length > 0 && !left.some((v) => v.is_latest)) {
          const imported = (v: (typeof left)[number]) => docs.get(v.document_id)?.imported_at ?? "";
          left.sort((x, y) => (y.effective_date ?? "").localeCompare(x.effective_date ?? "") || imported(y).localeCompare(imported(x)));
          left[0]!.is_latest = true;
        }
      }
      save(s);
      changed("versions", [String(a.documentId)]);
      return had;
    }
    default:
      throw err("internal", `mock knowledge: unknown command ${cmd}`);
  }
}
