# engines/

`llama-server` builds the app can run (design §3.3 / §3.12). Binaries are
**never committed**; they live under the app-data directory (replaceable per
version) or, bundled by the installer, in the Tauri resource directory (flat,
no version level):

```
%LOCALAPPDATA%\DocuLens\engines\<version>\<pack>\llama-server.exe   (Windows, app data — wins over bundled)
~/.local/share/DocuLens/engines/<version>/<pack>/llama-server       (Linux, CI only)
<install dir>\engines\<pack>\llama-server.exe                        (bundled: Lite = cpu + vulkan, Full = + cu121 + cu129)
src-tauri/resources/engines/<pack>/                                 (where `--bundle` puts them before `tauri build`; git-ignored)
```

| pack | source | notes |
|---|---|---|
| `cpu` | official llama.cpp release `llama-<tag>-bin-win-cpu-x64.zip` | always installed (Lite installer) |
| `vulkan` | official release `llama-<tag>-bin-win-vulkan-x64.zip` | AMD / Intel / NVIDIA fallback |
| `cu121` | self-built with the CUDA 12.1 toolkit (SASS sm_61…sm_90 incl. Turing) | Full installer |
| `cu129` | self-built with the CUDA 12.9 toolkit (sm_120 Blackwell) | Full installer |

Pinned tag and minimum build: `engines/manifest.json` (`b12066`, minimum
`b11747` — router presets, `/models` API and the vision-preprocessing fixes the
app relies on; the 0.4.0 models need the `gemma4` / `gemma-embedding2`
architectures and the `gemma4v` projector, all present in `b12066`). The app refuses to start an engine whose `--version` reports an older
build number; locally built binaries without tags report `0` (or `build 1` from
a shallow clone, in the `version: 0.8.0 (build N, commit …)` format) and are
accepted as "unknown build".

## Resolution order (`doculens_core::engine::locate`)

1. `DOCULENS_LLAMA_SERVER` (+ `DOCULENS_LLAMA_SERVER_BACKEND`) — development and tests
2. `settings.json` → `models.engine_binary` (model manager → "custom binary")
3. newest `engines/<version>/<pack>/` whose pack matches the backend rule
   (NVIDIA + driver CUDA ≥ 12.8 → `cu129`, ≥ 12.1 → `cu121`, any GPU → `vulkan`, else `cpu`;
   the user's backend preference can force one)
4. `engines/<pack>/` next to the executable or in the Tauri resource directory (bundled)

## Placing the official packs

```bash
node scripts/fetch-engine.mjs --tag b12066 --packs cpu,vulkan      # <app-data>/engines/b12066/<pack>/ (development, user PC)
node scripts/fetch-engine.mjs --bundle --packs cpu,vulkan          # src-tauri/resources/engines/<pack>/ (tauri build bundles them)
node scripts/fetch-engine.mjs --bundle --dry-run                   # print the target folders, no network
```

The script reads the release asset list from the GitHub API, downloads the
matching zips, verifies SHA-256 against `engines/manifest.json` (prints the
hashes with `--pin` so they can be recorded the first time), wipes the target
pack folder and unpacks into `<app-data>/engines/<tag>/<pack>/` — or, with
`--bundle`, into the flat resource layout `src-tauri/resources/engines/<pack>/`
that the installer ships as `engines/<pack>/`. `--dest <dir>` overrides the
destination (versioned layout), `--linux` fetches the `ubuntu-x64` build for
CI, and `GH_TOKEN` / `GITHUB_TOKEN` authenticates the API call (CI rate limits).
`.github/workflows/release-windows.yml` runs the `--bundle` form before
`tauri build`.

## Building the CUDA packs

The CUDA packs are built by the manual workflow **"CUDA engine packs (manual)"**
(`.github/workflows/cuda-packs.yml`, inputs `pack` = `cu121` | `cu129`,
`publish`, `allow_unsupported_compiler`) on a GitHub-hosted Windows runner. It
reads the pack definition from `engines/manifest.json` (`toolkit_installer_url`
— the official NVIDIA network installer, `toolkit_subpackages` — the silent
`-s` component ids, `msvc_toolset` / `msvc_component` — the host compiler the
toolkit supports, `archs`, `runtime_dlls`), installs the toolkit silently,
clones llama.cpp at the pinned tag, builds with

```
cmake -S llama.cpp -B build -G "Ninja Multi-Config" -DGGML_CUDA=ON -DCMAKE_CUDA_ARCHITECTURES="<archs>" ^
  -DGGML_NATIVE=OFF -DGGML_BACKEND_DL=ON -DGGML_CPU_ALL_VARIANTS=ON ^
  -DLLAMA_CURL=OFF -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF -DLLAMA_BUILD_TOOLS=ON -DLLAMA_BUILD_SERVER=ON
cmake --build build --config Release -j
```

(`GGML_NATIVE=OFF` + `GGML_CPU_ALL_VARIANTS` make the CPU code portable like the
official zips — a `-march=native` build from the runner would crash on other
CPUs), and zips `llama-server.exe` + all `*.dll` from `build/bin/Release/` +
`cudart64_12.dll`, `cublas64_12.dll`, `cublasLt64_12.dll` from the toolkit
`bin/` + `LICENSE-llama.cpp.txt` + `NVIDIA-CUDA-EULA.txt` + `pack.json` as
`DocuLens-EnginePack-<pack>-<tag>.zip` (+ `.sha256`). With `publish=true` the
zip becomes an asset of the GitHub Release the workflow ran for, and that
Release stays the packs' home: they belong to a llama.cpp tag, not to an app
version. The Full installer build (`release-windows.yml`, `variant=full`) reads
them from the Release recorded for the engine tag in its `CUDA_PACK_RELEASES`
list (`b12066=v0.1.0`), requires the `.sha256` next to each zip and bundles them
as `engines/cu121/` and `engines/cu129/`. A user PC can also unzip an asset into
`%LOCALAPPDATA%\DocuLens\engines\<tag>\<pack>\`.

License files inside a pack folder: the official `cpu` / `vulkan` archives ship
`LICENSE-LLVM-OpenMP` (for their `libomp.dll`) but no copy of llama.cpp's own MIT
license, so `scripts/fetch-engine.mjs` adds `LICENSE-llama.cpp.txt` from
`engines/licenses/llama.cpp-LICENSE.txt` (the `LICENSE` file of the pinned tag —
replace it when `manifest.json` moves to another tag). The self-built CUDA packs
take the same file from their llama.cpp checkout.

Installer size: the Full bundle is ~3.1 GB uncompressed (cu121 pack 1,875 MB —
`ggml-cuda.dll` 1,233 MB + cuBLAS 607 MB; cu129 pack 1,017 MB — cuBLAS 735 MB +
`ggml-cuda.dll` 247 MB, built with `-compress-mode=size`, which llama.cpp turns
on for CUDA ≥ 12.8). NSIS keeps a `/SOLID` data block uncompressed behind 32-bit
offsets, so Tauri's stock template overflowed the 2 GiB limit (`Internal compiler
error #12345`); `src-tauri/nsis/installer.nsi` — Tauri's template with per-file
compression, kept in sync by `npm run nsis:check` — is used instead, and the
limit then applies to the compressed installer (Full 0.1.0: 1,156 MB for 3.1 GB of
resources, 219 files).

| pack | toolkit | host compiler | archs |
|---|---|---|---|
| `cu121` | CUDA 12.1.1 network installer | MSVC 14.39 (VS 2022 17.9 toolset, installed on the runner — nvcc 12.1 rejects newer `cl`; `allow_unsupported_compiler=true` passes `-allow-unsupported-compiler` instead) | `61;70;75;80;86;89;90` (Pascal … Hopper incl. Turing SASS) |
| `cu129` | CUDA 12.9.1 network installer | default VS 2022 toolset | `75;80;86;89;90;120` (adds Blackwell) |

Published packs (GitHub Release `v0.1.0`, built on 2026-11-09 from tag `b12066`;
`llama-server.exe --version` → `version: 0.8.0-dev (build 12066, commit 713319e28)`):

| asset | size | SHA-256 |
|---|---|---|
| `DocuLens-EnginePack-cu121-b12066.zip` | 944,112,865 bytes (900 MiB) | `4a15af86d9db8dd6fc8eff5d721dc759ec26b20faa4833667c01c81f01d084ad` |
| `DocuLens-EnginePack-cu129-b12066.zip` | 812,703,965 bytes (775 MiB) | `3116d67428128f682078d7b86387304211fa81a740e2f2fc584713c1d5229cce` |

Notes from those builds: the workflow clones the full history of the tag
(`--single-branch --branch <tag>`, no `--depth 1`) because llama.cpp derives the
build number from `git rev-list --count HEAD` — a shallow clone reports
`build 1`, which the app treats as an unknown version. The NVIDIA EULA text is
taken from the toolkit installation (`documentation_<ver>` subpackage:
`v12.1\EULA.txt`, `v12.9\LICENSE`). The cuBLAS runtime (`cublasLt64_12.dll`,
`cublas64_12.dll`) makes up most of each pack.

Building by hand on a Windows PC with the toolkit installed is the same
sequence: `git clone --single-branch --branch b12066 https://github.com/ggml-org/llama.cpp`
(full history, see above), the `cmake` lines above (from a "x64 Native Tools"
prompt), then copy the files listed above into
`%LOCALAPPDATA%\DocuLens\engines\b12066\cu121\`.

**NVIDIA CUDA EULA** (design appendix E): only the Attachment-A runtime
libraries (cudart, cublas, cublasLt) are redistributed, they stay inside the
app-private pack directory (never on the system PATH or in a shared location),
the toolkit EULA text ships in the pack, no developer tools are redistributed,
and the DocuLens application EULA must mirror NVIDIA's terms before a Full
installer is distributed (still open). If DocuLens itself is ever released
under an open-source license, the CUDA packs must become a separate download.

## Verifying a pack

```powershell
& "$env:LOCALAPPDATA\DocuLens\engines\b12066\cpu\llama-server.exe" --version
& "$env:LOCALAPPDATA\DocuLens\engines\b12066\cu121\llama-server.exe" --list-devices
```

The model manager shows the resolved binary, its build number, the installed
packs and the engine-reported devices (free VRAM is what the tier table uses).

**One backend family per pack directory.** ggml loads every `ggml-*.dll` /
`libggml-*.so` found next to `llama-server`, so never unpack two release
archives (e.g. CPU + OpenVINO, or CPU + Vulkan) into the same folder: the extra
backend registers a device, takes over the graph and — for hybrid or
sliding-window attention layers — fails with "Compute error". This is exactly what a too-loose download
pattern did in CI.

## PDFium (PDF 렌더러)

PDF 페이지 렌더링과 텍스트 레이어 추출은 PDFium(BSD-3)을 런타임에 동적 로드한다
(`pdfium-render` 0.9, 핀 `engines/pdfium.json` = chromium/8125 — 크레이트 기본 기능
`pdfium_8125`와 같은 빌드). 바이너리는 커밋하지 않는다.

```bash
node scripts/fetch-pdfium.mjs             # <app-data>/pdfium/  (개발·사용자 PC)
node scripts/fetch-pdfium.mjs --bundle    # src-tauri/resources/pdfium/ (tauri build 가 설치파일에 동봉)
node scripts/fetch-pdfium.mjs --dest DIR  # 임의 위치 (DOCULENS_PDFIUM=DIR/pdfium.dll)
```

탐색 순서(`doculens_core::documents::render::locate_pdfium`): `DOCULENS_PDFIUM`(파일 또는
폴더) → 리소스 폴더 `pdfium/` → 실행 파일 옆 / `pdfium/` → `<app-data>/pdfium/`. 없으면 PDF는
등록만 되고(텍스트·페이지 없음) 라이브러리 상단에 경고가 뜬다. 아카이브의 `LICENSE`와
`licenses/`(18개 고지)는 스크립트가 같이 복사하며 설치파일에 그대로 포함해야 한다.

## ko-dic (한국어 형태소 사전)

검색 색인과 질의 분석은 lindera(MIT) + mecab-ko-dic(Apache-2.0)을 쓴다. 사전은 빌드 시
내려받지 않고, `scripts/fetch-kodic.mjs`가 핀(`engines/kodic.json`, SHA-256)된 원본 tarball을
받아 `crates/doculens-kodic`(lindera `DictionaryBuilder`)로 바이너리 사전(약 82 MB)을 만든다.
바이너리는 커밋하지 않는다.

```bash
node scripts/fetch-kodic.mjs                   # <app-data>/tokenizers/ko-dic/  (개발·사용자 PC)
node scripts/fetch-kodic.mjs --bundle          # src-tauri/resources/ko-dic/     (설치파일 동봉)
node scripts/fetch-kodic.mjs --dest DIR        # 임의 위치 (DOCULENS_KODIC=DIR)
node scripts/fetch-kodic.mjs --source FILE     # 오프라인: 받아 둔 tarball 사용
```

탐색 순서(`src-tauri/src/knowledge_bridge.rs`): `DOCULENS_KODIC` → 리소스 폴더 `ko-dic/` →
`<app-data>/tokenizers/ko-dic/`. 없으면 간이 분석기(어절 + 2음절)로 동작하고 지식 베이스
화면에 안내가 뜬다. 사전 폴더의 `LICENSE-mecab-ko-dic.txt`·`NOTICE.txt`는 설치파일에 그대로 포함한다.
