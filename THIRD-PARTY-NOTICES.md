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
