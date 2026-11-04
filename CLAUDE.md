# DocuLens — project instructions for Claude Code sessions

## What this is
Offline, Windows-only desktop client (Tauri 2 + React/TypeScript UI, Rust core)
that processes Korean/English business documents with a local VLM
(Qwen3.5-4B via llama-server). Design: `docs/design/doculens-design-v1.1.md`
(Korean, authoritative). Status and next steps: `docs/HANDOFF.md`.

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
1. One phase (P0…P5) per session; end each phase with a demo checkpoint
   (tests green, `docs/HANDOFF.md` updated, commit + push).
2. Trim build logs (`| tail`, `grep -v Compiling`); compile-check per module.
3. Spawn review subagents only at the end of a phase.
4. v1.0 scope = P0–P4 + core planner ops; document generation and project
   consistency checks are v1.1.

## Commands
- Rust: `cargo test --workspace` (lib + `tests/engine_process.rs`), `cargo check -p doculens`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`.
- Real engine (optional): `DOCULENS_LLAMA_SERVER=<llama-server> DOCULENS_TEST_MODELS_DIR=$PWD/crates/doculens-core/tests/fixtures cargo test -p doculens-core --test real_engine -- --nocapture` (absolute path: the test binary runs from the crate directory).
- PDFium (optional, enables the render/import tests): `node scripts/fetch-pdfium.mjs --dest /tmp/pdfium` then
  `DOCULENS_PDFIUM=/tmp/pdfium/libpdfium.so cargo test -p doculens-core`.
- Sample documents: `DOCULENS_FIXTURE_OUT=<dir> cargo test -p doculens-core --lib -- --ignored gen_fixtures`.
- ko-dic (optional, enables the morphological tokenizer tests; the built-in simple analyzer is used otherwise):
  `node scripts/fetch-kodic.mjs --dest /tmp/kodic` (downloads the pinned mecab-ko-dic tarball and builds it with
  `cargo run -p doculens-kodic`) then `DOCULENS_KODIC=/tmp/kodic cargo test -p doculens-core`.
- Web: `npm run typecheck`, `npm test` (vitest), `npm run build`,
  `PW_CHROMIUM_PATH=/opt/pw-browsers/chromium npm run e2e` (browser mode, mock backend).
- Desktop (Windows / Linux with webkit2gtk): `npm run tauri dev`, `npm run tauri build`.
- Linux build deps: `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev`.

## Layout
- `crates/doculens-core/` — settings, workspace, hw/tier, memory, registry, retention, prerouter,
  `gguf` (header parser, hashes), `modelstore` (drop-in folders, import, download), `fswatch` (debounced watcher),
  `engine/` (locate, devices, presets, client, process, manager, fake_router),
  `db/` (rusqlite + embedded migrations), `documents/` (repo, import plan/run/rescan, render via PDFium,
  `convert/` HWPX·DOCX·HWP → model → HTML → PDF through a `PdfPrinter`, fixtures),
  `knowledge/` (tokenize: lindera ko-dic / simple analyzer, chunker: structure-aware chunking, repo, index,
  embed: sqlite-vec + `Embedder`, search: hybrid RRF, answer: grounded answers / explain / summarize / compare),
  `pipeline/` (schema: built-in sample forms + grammar-safe JSON schema + lint, prompts, vlm: retrying caller, image,
  classify, ocr, extract, validate: normalisers / BRN checksum / sum rules / PII masking / confidence, run: orchestrator +
  `extractions/` mirror, review, forms: form registration + NL schemas, split: lopdf split / merge, repo).
- `crates/doculens-kodic/` — bin that compiles the mecab-ko-dic sources into lindera's dictionary format
  (driven by `scripts/fetch-kodic.mjs`; descriptors in `assets/`).
- `src-tauri/` — Tauri shell (`src/commands/{mod,documents,knowledge,pipeline}.rs`, `src/state.rs`, `src/engine_bridge.rs`,
  `src/docs_bridge.rs`, `src/knowledge_bridge.rs` (analyzer lifetime, index/embed jobs, answer streaming),
  `src/pipeline_bridge.rs` (vision model, processing / split / merge jobs, auto pipeline after import, forms / NL schemas),
  `src/protocol.rs` (`doculens://` page images / print bundles), `src/printer.rs`
  (WebView2 / WebKitGTK print-to-PDF), `src/jobs.rs`), `tauri.conf.json`, capabilities.
- `src/` — React app: `app/` (layout, providers, hooks), `features/<screen>/`, `components/ui/`, `lib/` (ipc, types, theme, shortcuts, mock backend), `stores/`, `i18n/`, `styles/`.
- `models/registry.json`, `engines/` (llama.cpp manifest, `pdfium.json`, `kodic.json`, placement docs), `docs/`,
  `scripts/` (`fetch-engine.mjs`, `fetch-pdfium.mjs`, `fetch-kodic.mjs`, `gen-tiny-qwen35.py`), `e2e/`,
  `.github/workflows/`.
- Events from Rust to the webview: `engine://status|log|model`, `models://changed`, `jobs://update`,
  `documents://changed`, `knowledge://changed`, `knowledge://answer` (streamed answer deltas), `pipeline://changed`.
- Workspace data: `.doculens/workspace.db` (schema in `crates/doculens-core/src/db/schema/`; knowledge tables
  `chunks` + FTS5 `chunks_fts`/`chunks_fts_tri`, sqlite-vec `chunks_vec`, `collections`, `doc_versions`, `glossary`,
  `answers`; pipeline tables `schemas`, `schema_versions`, `extraction_runs`, `fields`, `field_history`), page cache
  `.doculens/cache/{pages,thumbs,converted}/<doc_id>/`, extraction mirror `.doculens/extractions/<doc_id>/<run_id>/`;
  DB rows store workspace-relative paths only. OCR text lives in `pages.text` (`text_source = ocr`) so the knowledge
  indexer re-chunks it.
- Runtime assets (never committed): PDFium (`scripts/fetch-pdfium.mjs`) and the ko-dic dictionary
  (`scripts/fetch-kodic.mjs`); both are located via env → bundled resources → `<AppData>/DocuLens/`.
