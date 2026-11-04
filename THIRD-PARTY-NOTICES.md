# Third-party notices

DocuLens bundles or links the following third-party software and assets. Every
component is licensed under terms that carry **no source-disclosure
obligation** for the application (Apache-2.0, MIT, BSD, ISC, OFL-1.1, zlib,
CC0 …). The full per-crate / per-package list is generated at build time
(`cargo about` / `scripts/check-licenses.mjs`, phase P7) and appended below
this header; the entries here cover direct dependencies and bundled assets.

## Application frameworks

| Component | License | Copyright / notes |
|---|---|---|
| Tauri 2 (`tauri`, `tauri-build`, plugins `opener`, `dialog`, `log`) | Apache-2.0 OR MIT | © Tauri Programme within The Commons Conservancy |
| Microsoft Edge WebView2 SDK / Loader (via `webview2-com`) | BSD-3-Clause | © Microsoft Corporation |
| React, React DOM | MIT | © Meta Platforms, Inc. |
| react-router | MIT | © Remix Software Inc. |
| i18next, react-i18next | MIT | © i18next |
| Zustand | MIT | © Poimandres |
| TanStack Query | MIT | © Tanner Linsley |
| Radix UI primitives | MIT | © WorkOS |
| cmdk | MIT | © Paco Coursey |
| Lucide icons | ISC | © Lucide Contributors; portions © Cole Bemis (Feather, MIT) |
| class-variance-authority | Apache-2.0 | © Joe Bell |
| clsx | MIT | © Luke Edwards |
| tailwind-merge | MIT | © Dany Castillo |
| Tailwind CSS v4 | MIT | © Tailwind Labs |
| shadcn/ui component patterns (re-implemented in `src/components/ui`) | MIT | © shadcn |

## Rust core

| Crate | License |
|---|---|
| serde, serde_json | MIT OR Apache-2.0 |
| thiserror | MIT OR Apache-2.0 |
| log | MIT OR Apache-2.0 |
| chrono | MIT OR Apache-2.0 |
| uuid | MIT OR Apache-2.0 |
| dirs | MIT OR Apache-2.0 |
| sysinfo | MIT |
| sha2 (RustCrypto) | MIT OR Apache-2.0 |
| blake3 | CC0-1.0 OR Apache-2.0 |
| ureq (HTTP client; `native-tls` on Windows via schannel) | MIT OR Apache-2.0 |
| native-tls, schannel | MIT OR Apache-2.0 / MIT |
| flate2 | MIT OR Apache-2.0 |
| notify (file-system watcher) | CC0-1.0 |
| libc (Unix process groups, CI only) | MIT OR Apache-2.0 |
| windows-sys (job objects) | MIT OR Apache-2.0 |
| nvml-wrapper | MIT OR Apache-2.0 (NVML itself is loaded from the user's NVIDIA driver at runtime; not redistributed) |
| rusqlite, libsqlite3-sys (bundled SQLite) | MIT (SQLite itself is public domain) |
| pdfium-render | MIT OR Apache-2.0 (© Alastair Carey) |
| zip (deflate via flate2 / zlib-rs) | MIT |
| quick-xml | MIT |
| image (JPEG/PNG/TIFF/WebP/BMP/GIF codecs, pure Rust) | MIT OR Apache-2.0 |
| trash (Recycle Bin / freedesktop trash) | MIT |
| hwp (hahnlee/hwp-rs — HWP 5.0 parser; "본 제품은 한글과컴퓨터의 한/글 문서 파일(.hwp) 공개 문서를 참고하여 개발하였습니다.") + cfb, aes, num | Apache-2.0 / MIT OR Apache-2.0 |
| unicode-normalization | MIT OR Apache-2.0 |
| lindera, lindera-dictionary (Korean morphological analysis; dictionary loaded at runtime) + crawdad, daachorse, rkyv, encoding_rs, memmap2, csv, byteorder, strum | MIT / MIT OR Apache-2.0 / (encoding_rs: Apache-2.0 OR MIT, BSD-3-Clause) / (csv, byteorder: Unlicense OR MIT) |
| sqlite-vec (vector search inside SQLite, statically linked C) | MIT OR Apache-2.0 (© Alex Garcia) |
| gtk, webkit2gtk (Linux CI/dev builds only — print-to-PDF) | MIT |
| webview2-com, windows-core (Windows print-to-PDF) | MIT / MIT OR Apache-2.0 |
| @tanstack/react-virtual | MIT (© Tanner Linsley) |

MPL-2.0 components (consumed unmodified only — decision 10): `lightningcss`
(via Tailwind), `selectors` / `cssparser` / `dtoa-short` (via `tauri-utils` →
`dom_query`), `option-ext` (via `dirs` → `dirs-sys`). OpenSSL-licensed code is
linked only on Linux (CI builds) through `native-tls`.

## PDF renderer (PDFium, installed by `scripts/fetch-pdfium.mjs`)

- **PDFium** — BSD-3-Clause, © The PDFium Authors / The Chromium Authors. Prebuilt by
  **bblanchon/pdfium-binaries** (MIT, © Benoit Blanchon), pinned in `engines/pdfium.json`
  (chromium/8125). The library (`pdfium.dll` / `libpdfium.so`) is loaded at runtime from the
  installer's `pdfium/` resource folder or `<AppData>/DocuLens/pdfium/`.
- The archive's `LICENSE` and the 18 third-party notices under `licenses/` (FreeType FTL,
  libjpeg-turbo IJG/zlib, OpenJPEG BSD-2, ICU Unicode-3.0, Abseil / Highway Apache-2.0,
  llvm-libc Apache-2.0 WITH LLVM-exception, HarfBuzz Old-MIT, lcms MIT, libpng, zlib, agg23,
  fast_float, simdutf, dragonbox) are copied next to the library by the fetch script and must
  ship with the installer unchanged (design §3.2).

## Korean dictionary (mecab-ko-dic, built by `scripts/fetch-kodic.mjs`)

- **mecab-ko-dic 2.1.1-20180720** — Apache-2.0 (© Eunjeon project, https://bitbucket.org/eunjeon/mecab-ko-dic),
  pinned by SHA-256 in `engines/kodic.json`. The CSV sources are compiled by `crates/doculens-kodic`
  (lindera's `DictionaryBuilder`) into lindera's binary layout (~82 MB) and loaded at runtime from the
  installer's `ko-dic/` resource folder or `<AppData>/DocuLens/tokenizers/ko-dic/`. Nothing is downloaded
  at build time and the dictionary is never committed.
- The dictionary's `COPYING` (Apache License 2.0) is copied next to the built files as
  `LICENSE-mecab-ko-dic.txt` together with a `NOTICE.txt`; both must ship with the installer.
- `crates/doculens-kodic/assets/{metadata.json,context_id_freq.txt}` are the dictionary descriptors
  shipped by the `lindera-ko-dic` crate (MIT, © Minoru Osuka / Lindera project).

## Fonts

- **Pretendard Variable 1.3.9** — SIL Open Font License 1.1, © 2021 Kil Hyung-jin,
  with Reserved Font Name "Pretendard"; based on Source Han Sans / Inter
  (© Adobe, Reserved Font Name "Source"). The font file is bundled **unmodified**
  (`public/fonts/PretendardVariable.woff2`); the full license text is in
  `public/fonts/LICENSE-Pretendard.txt`.

## Models (not bundled; downloaded or imported by the user)

- Qwen3.5 family (Alibaba Cloud) — Apache-2.0.
- Qwen3-VL-4B-Instruct (Alibaba Cloud) — Apache-2.0.
- BAAI bge-m3 — MIT.

The license and NOTICE files of each imported model are kept next to the
model files inside the workspace (`.doculens/models/<id>/`).

## Inference engine (installed separately or with the Full installer)

- llama.cpp `llama-server` — MIT, © Georgi Gerganov and contributors.
- NVIDIA CUDA runtime libraries (optional CUDA pack) — NVIDIA Software License
  Agreement (redistributable runtime components; see the installer EULA).

---

<!-- generated section starts here -->
