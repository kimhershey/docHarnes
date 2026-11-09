# DocuLens — project instructions for Claude Code sessions

## What this is
Offline, Windows-only desktop client (Tauri 2 + React/TypeScript UI, Rust core)
that processes Korean/English business documents with a local VLM
(Qwen3.5-4B via llama-server). Design: `docs/design/doculens-design-v1.1.md`
(Korean, authoritative). Status and next steps: `docs/HANDOFF.md`. How to install / run / build / verify on a
user PC: `docs/RUN-GUIDE.md`.

## Hard rules
- **Licenses**: only dependencies without a source-disclosure obligation
  (`docs/LICENSE-POLICY.md`). No GPL/LGPL/AGPL/SSPL/BUSL/MS-RL/community
  licenses. MPL-2.0 only unmodified. Run `npm run licenses:check` and
  `cargo deny check licenses` before adding dependencies.
- **Korean first**: UI strings default to Korean (`src/i18n/ko.json`) with an
  English twin (`en.json`); the parity test fails on missing keys.
- **No telemetry, no network at runtime** except explicit user-initiated model
  downloads.
- Rust core (`crates/doculens-core`) must stay free of Tauri/UI dependencies.
- Keep `models/registry.json` the single source of truth for models.

## Working method (user decision 2026-11-03)
1. One phase per session (P0…P5 for v1.0; P6 + P7 ran together on 2026-11-08 by user decision); end each phase with a demo checkpoint
   (tests green, `docs/HANDOFF.md` updated, commit + push).
2. Trim build logs (`| tail`, `grep -v Compiling`); compile-check per module.
3. Spawn review subagents only at the end of a phase.
4. v1.0 scope = P0–P4 + core planner ops; document generation and project
   consistency checks (P6) and quality / packaging (P7) are v1.1.
5. Installer builds (Actions "Windows installer (manual)") are dispatched only after every task of the
   request is finished and the full verification (`cargo test --workspace`, clippy, vitest, Playwright,
   CI on the pushed commit) reports no FAILED — never mid-work (user decision 2026-11-09).

## Commands
- Rust: `cargo test --workspace` (lib + `tests/engine_process.rs`), `cargo check -p doculens`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`.
- Real engine (optional): `DOCULENS_LLAMA_SERVER=<llama-server> DOCULENS_TEST_MODELS_DIR=$PWD/crates/doculens-core/tests/fixtures cargo test -p doculens-core --test real_engine -- --nocapture` (absolute path: the test binary runs from the crate directory; covers the chat, KIE and planner grammars).
- PDFium (optional, enables the render/import tests): `node scripts/fetch-pdfium.mjs --dest /tmp/pdfium` then
  `DOCULENS_PDFIUM=/tmp/pdfium/libpdfium.so cargo test -p doculens-core`.
- Sample documents: `DOCULENS_FIXTURE_OUT=<dir> cargo test -p doculens-core --lib -- --ignored gen_fixtures`.
- Export samples (XLSX / CSV / JSONL / MD / searchable PDF from seeded data): `DOCULENS_EXPORT_OUT=<dir> cargo test -p doculens-core --lib -- --ignored gen_export_samples`.
- ko-dic (optional, enables the morphological tokenizer tests; the built-in simple analyzer is used otherwise):
  `node scripts/fetch-kodic.mjs --dest /tmp/kodic` (downloads the pinned mecab-ko-dic tarball and builds it with
  `cargo run -p doculens-kodic`) then `DOCULENS_KODIC=/tmp/kodic cargo test -p doculens-core`.
- Eval harness: `cargo run -p doculens-eval -- planner --golden eval/golden/planner.json --baseline eval/baselines/planner-rules.json`
  (CI gate; see `eval/README.md` for the pipeline runner and the scorecard).
- Notices / SBOM: `npm run notices` regenerates `THIRD-PARTY-NOTICES.md`, `sbom.cdx.json` and the bundled license texts;
  CI fails when the committed files are stale.
- Web: `npm run typecheck`, `npm test` (vitest), `npm run build`,
  `PW_CHROMIUM_PATH=/opt/pw-browsers/chromium npm run e2e` (browser mode, mock backend),
  `npm run nsis:check` (the NSIS template copy `src-tauri/nsis/installer.nsi` must match the installed Tauri CLI).
- Desktop (Windows / Linux with webkit2gtk): `npm run tauri dev`, `npm run tauri build`. Installers: GitHub Actions
  "Windows installer (manual)" (`lite` / `full`, optional Release publish) and "CUDA engine packs (manual)"; bundled
  runtime assets come from `scripts/fetch-*.mjs --bundle`.
- Linux build deps: `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev`.

## Layout
- `crates/doculens-core/` — settings, workspace, hw/tier, memory, registry, retention, prerouter,
  `gguf` (header parser, hashes), `modelstore` (drop-in folders, import, download), `fswatch` (debounced watcher),
  `engine/` (locate, devices, presets, client, process, manager, fake_router),
  `db/` (rusqlite + embedded migrations), `documents/` (repo, import plan/run/rescan with content-based
  classification (`sniff`: magic bytes / zip & CFB entries → `import::classify`), render via PDFium, `layout` (word boxes of the
  page text: PDFium char boxes grouped into visual rows (wrapped table cells), or a rulings-free ink-band / cell-cluster estimate
  for OCR pages; v1 layouts get their rows relabelled on load; on-image edits rewrite `pages.text`),
  `convert/` HWPX·DOCX·HWP·XLSX·PPTX·text (txt/md/csv/html…) + legacy XLS/XLSB/ODS (calamine)·DOC·PPT (cfb)·RTF·HML → model
  (flow blocks + positioned `slides` for PPTX) → HTML → PDF through a `PdfPrinter` (page size from the source), fixtures),
  `knowledge/` (tokenize: lindera ko-dic / simple analyzer, chunker: structure-aware chunking, repo, index,
  embed: sqlite-vec + `Embedder`, search: hybrid RRF, answer: grounded answers / explain / summarize / compare with the conversation history +
  screen session as context blocks and follow-up detection),
  `pipeline/` (schema: built-in sample forms + grammar-safe JSON schema + lint, prompts, vlm: retrying caller, image,
  classify, ocr, extract, validate: normalisers / BRN checksum / sum rules / PII masking / confidence, run: orchestrator +
  `extractions/` mirror, review, forms: form registration + NL schemas, split: lopdf split / merge, repo),
  `planner/` (mod: typed ops / scope / plan (appendix D v1.0 subset), schema: op spec table + PlanSchema grammar, rules:
  deterministic phrase rules (legacy `RulesFirst` mode + eval), prompts: tiered planner prompt, llm: `PlanMode::ModelFirst`
  (default; slash → pre-router, everything else → model with one re-query; no model → clarification) / `RulesFirst`, validate,
  scope: scope → documents, exec: deterministic executor + previews), `organize/` (template: file-name / folder templates,
  mod: dry-run diff + apply, journal: `ops_journal` + `journal/ops-YYYYMM.jsonl`, LIFO undo / redo), `aggregate` (sum /
  count / avg / min / max with the reviewed / unreviewed split), `export` (XLSX / CSV / JSONL / MD / searchable PDF),
  `jobs/` (persistent job rows + specs incl. `Draft` / `Maintenance`, priorities / lanes, `JobControl` pause / cancel,
  `EngineGate` interactive cut-in), `generate/` (P6 §3.13: built-in templates, grammar-safe draft schema, prompts, VLM
  draft + rule-based 보완요청서, reverse validation, HTML / MD / DOCX / HWPX / PDF writers, registered-form filling,
  drafts store under `.doculens/drafts/`), `projects/` (P6 §3.15: projects / templates / rules repo, NL → rule JSON,
  deterministic consistency check with evidence, relation graph, Markdown report, 보완요청서 request builder, inbox
  subfolder → project mapping, `classify`: title / content → project suggestions (rule scores + optional grammar-bound model
  pass, preview → apply; `projects.keywords`, migration 0007)), `retention` (policy + purge / LRU file helpers used by the daily maintenance job).
- `crates/doculens-kodic/` — bin that compiles the mecab-ko-dic sources into lindera's dictionary format
  (driven by `scripts/fetch-kodic.mjs`; descriptors in `assets/`).
- `crates/doculens-eval/` — P7 evaluation harness bin (`planner` golden-set accuracy + regression gate, `pipeline`
  golden-manifest runner with CER / exact-match / ANLS* / F1 / latency, `scorecard` merge); golden sets and baselines
  under `eval/`.
- `src-tauri/` — Tauri shell (`src/commands/{mod,documents,knowledge,pipeline}.rs`, `src/state.rs`, `src/engine_bridge.rs`,
  `src/docs_bridge.rs`, `src/knowledge_bridge.rs` (analyzer lifetime, index/embed jobs, answer streaming),
  `src/pipeline_bridge.rs` (vision model, processing / split / merge jobs, auto pipeline after import, forms / NL schemas),
  `src/planner_bridge.rs` (plan + preview, inline / job execution, the persistent-job worker, organise / journal /
  aggregate / export helpers), `src/commands/planner.rs`, `src/generate_bridge.rs` + `src/commands/generate.rs`
  (draft jobs, preview / validate / save, drafts store), `src/projects_bridge.rs` + `src/commands/projects.rs`
  (projects / templates / rules / check / report / 보완요청서 job, `maintenance_*` commands), `src/maintenance.rs`
  (daily retention purge job + scheduler, audit log `logs/purge-YYYYMM.jsonl`), `src/notify.rs` (job-finished OS
  toast), `src/power.rs` (sleep prevention while engine jobs run),
  `src/protocol.rs` (`doculens://` page images / print bundles), `src/printer.rs`
  (WebView2 / WebKitGTK print-to-PDF), `src/jobs.rs` (job book + persistent runner: priority queue, lanes, pause /
  resume / retry / prioritise, restored per workspace)), `tauri.conf.json`, capabilities, `nsis/installer.nsi`
  (Tauri's NSIS template with per-file instead of /SOLID compression — the Full installer exceeds NSIS' 2 GiB solid
  data block; verified against the installed CLI by `npm run nsis:check`).
- `src/` — React app: `app/` (layout, providers, hooks — `useRootFontSize`: rem → px for virtualiser estimates and column counts so the UI 배율 scales every box; layout sizes are rem or measured, never fixed px), `features/<screen>/`, `components/ui/`, `lib/` (ipc, types, theme, shortcuts, mock backend,
  `dnd/`: pointer-based document drags + native / browser file drops with one drop-target registry, `DndLayer` in the app shell, sidebar
  spring-loading, `importDroppedFiles` import-then-attach), `stores/` (incl. `attachments.ts`: assistant attachments + nav-drop hand-offs), `i18n/`, `styles/`.
- `models/registry.json`, `engines/` (llama.cpp manifest, `pdfium.json`, `kodic.json`, placement docs), `docs/`,
  `scripts/` (`fetch-engine.mjs`, `fetch-pdfium.mjs`, `fetch-kodic.mjs`, `gen-tiny-qwen35.py`), `e2e/`,
  `.github/workflows/`.
- Events from Rust to the webview: `engine://status|log|model`, `models://changed`, `jobs://update`,
  `documents://changed`, `knowledge://changed`, `knowledge://answer` (streamed answer deltas), `pipeline://changed`,
  `projects://changed`.
- Workspace data: `.doculens/workspace.db` (schema in `crates/doculens-core/src/db/schema/`; knowledge tables
  `chunks` + FTS5 `chunks_fts`/`chunks_fts_tri`, sqlite-vec `chunks_vec`, `collections`, `doc_versions`, `glossary`,
  `answers`; pipeline tables `schemas`, `schema_versions`, `extraction_runs`, `fields`, `field_history`; P5 tables
  `journal_batches`, `ops_journal`, `jobs`; P6 tables `projects`, `project_templates`, `project_documents`,
  `project_rules`, `project_runs`, `project_rule_results`; `pages.layout` word-box JSON (0006); `projects.keywords` (0007)), drafts `.doculens/drafts/<id>.json`, page cache
  `.doculens/cache/{pages,thumbs,converted}/<doc_id>/`, extraction mirror `.doculens/extractions/<doc_id>/<run_id>/`;
  DB rows store workspace-relative paths only. OCR text lives in `pages.text` (`text_source = ocr`) so the knowledge
  indexer re-chunks it.
- Runtime assets (never committed): PDFium (`scripts/fetch-pdfium.mjs`) and the ko-dic dictionary
  (`scripts/fetch-kodic.mjs`); both are located via env → bundled resources → `<AppData>/DocuLens/`.
