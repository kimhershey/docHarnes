# License policy (코드 공개 의무 없는 라이선스만 사용)

DocuLens is distributed as closed-source, commercially licensable software.
Every third-party component — libraries, tools that end up in the artefact,
fonts, model weights — must therefore carry a license **without a
source-disclosure obligation**.

## Allowed

Apache-2.0 (with or without LLVM-exception), MIT, MIT-0, BSD-2/3-Clause, 0BSD,
ISC, Zlib, Unicode-3.0 / Unicode-DFS-2016, CC0-1.0, Unlicense, BSL-1.0 (Boost),
OFL-1.1 (fonts; Reserved Font Name rules apply), FTL / IJG (via PDFium),
CDLA-Permissive-2.0 (datasets), Python-2.0 / PSF-2.0 (eval tooling).

Model weights (0.4.0): **Gemma 4** (E2B / E4B / 12B) and **EmbeddingGemma 2** are
Apache-2.0 — Google dropped the custom Gemma Terms with Gemma 4 (April 2026) and
EmbeddingGemma 2 (October 2026). Gemma ≤ 3 and the first-generation EmbeddingGemma
still carry the Gemma Terms and stay denied.

**MPL-2.0: only unmodified** (design decision 10). The file-level copyleft is
never triggered as long as the MPL files are shipped as received. Never patch
or vendor-modify an MPL dependency (currently: `lightningcss` through Tailwind;
`selectors`, `cssparser`, `dtoa-short` through `tauri-utils`; `option-ext`
through `dirs`). The OpenSSL license (allowed in `deny.toml`) only reaches the
Linux CI build via `native-tls`; Windows uses schannel.

Data / dictionaries: mecab-ko-dic (Apache-2.0) is compiled into lindera's binary
format by `scripts/fetch-kodic.mjs` and shipped with its `COPYING` + `NOTICE`
(no source obligation; attribution only). Kiwi's `kiwi-rs` binding stays denied
(LGPL) — see the design BOM.

Proprietary but documented exceptions (binary redistribution, no source
obligation): NVIDIA CUDA runtime DLLs (optional CUDA pack; EULA conditions —
app-private directory, no system PATH), Microsoft WebView2 Runtime installer
(unmodified redistribution), Windows system runtimes.

## Denied

GPL (any), LGPL (any — including dynamic linking, by company policy), AGPL,
SSPL, BUSL/BSL-1.1, MS-RL, EUPL, CC-BY-NC, "community"/"research" licenses
(Qwen Research License, Gemma Terms ≤3, Llama Community License, Lobe Chat …),
`LicenseRef-*` / unknown.

Origin rule (user decision 2026-11-10): weights from Chinese developers (Qwen /
Qwen3-VL, bge-m3, GLM, InternVL, MiniCPM, DeepSeek, PaddleOCR, Hunyuan …) are
excluded from `models/registry.json` regardless of their license. A folder a user
drops in is still recognised by architecture, but the app never recommends,
downloads or ships them.

Concrete avoid-list: see design Appendix C (`docs/design/doculens-design-v1.1.md`):
PyMuPDF, poppler, PySide6/PyQt, Slint, Nuitka, WiX, Electron (bundled FFmpeg),
opencv-python wheels, docling-eval, kiwi-rs, KoNLPy/soynlp, Label Studio
installation, dots.ocr/mocr, MinerU, Marker/Surya/Chandra weights,
Nanonets-OCR2, HunyuanOCR, Kanana/HyperCLOVA/VARCO/EXAONE, Gemma ≤3, jina v2+,
EmbeddingGemma (1st generation), Qwen / bge-m3 and other Chinese-origin weights
(origin rule), Open WebUI/Lobe Chat code, Redis ≥7.4/MinIO/Grafana/Loki/
Elasticsearch, GPL TEDS implementations, axe-core (dev only).

## Gates

- Rust: `cargo deny check licenses` (`deny.toml`), CI job `license`.
- npm: `npm run licenses:check` (`scripts/check-licenses.mjs`).
- Models: `models/registry.json` `license` field (every entry Apache-2.0 since 0.4.0:
  Gemma 4 + EmbeddingGemma 2) and the origin rule above; LICENSE/NOTICE files are
  copied next to imported weights.
- PDFium: `engines/pdfium.json` pins the bblanchon/pdfium-binaries build;
  `scripts/fetch-pdfium.mjs` verifies the SHA-256 and copies the archive's
  `LICENSE` + `licenses/` (18 notices) next to the library — ship them unchanged.
- HWP 5.0 parsing uses `hwp` (Apache-2.0); keep its notice ("한글과컴퓨터의 한/글
  문서 파일(.hwp) 공개 문서를 참고") in `THIRD-PARTY-NOTICES.md`.
- Generated notices: `npm run notices` (`scripts/gen-notices.mjs`) regenerates the inventory
  section of `THIRD-PARTY-NOTICES.md`, `sbom.cdx.json` (CycloneDX 1.5) and, at build time, the
  bundled license texts `licenses/THIRD-PARTY-LICENSES.txt`; the CI job `notices` fails when the
  committed files are stale.

## UI code reuse

Code may be borrowed only from permissive projects (Jan, AnythingLLM,
LibreChat, assistant-ui, kotaemon, RAGFlow, shadcn/ui, Label Studio
*frontend*). Copyleft/custom-licensed apps (LM Studio, Paperless-ngx, Open
WebUI, Lobe Chat, Cherry Studio, Chatbox, Zotero) are design inspiration only.
Borrowed files keep their copyright header, carry an Apache-2.0 §4(b) change
notice when modified, and are listed per file in `THIRD-PARTY-NOTICES.md`.
