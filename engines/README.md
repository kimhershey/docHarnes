# engines/

`llama-server` builds the app can run (design §3.3 / §3.12). Binaries are
**never committed**; they live under the app-data directory:

```
%LOCALAPPDATA%\DocuLens\engines\<version>\<pack>\llama-server.exe   (Windows)
~/.local/share/DocuLens/engines/<version>/<pack>/llama-server       (Linux, CI only)
```

| pack | source | notes |
|---|---|---|
| `cpu` | official llama.cpp release `llama-<tag>-bin-win-cpu-x64.zip` | always installed (Lite installer) |
| `vulkan` | official release `llama-<tag>-bin-win-vulkan-x64.zip` | AMD / Intel / NVIDIA fallback |
| `cu121` | self-built with the CUDA 12.1 toolkit (SASS sm_61…sm_90 incl. Turing) | Full installer |
| `cu129` | self-built with the CUDA 12.9 toolkit (sm_120 Blackwell) | Full installer |

Pinned tag and minimum build: `engines/manifest.json` (`b12066`, minimum
`b11747` — router presets, `/models` API and the Qwen3.5 fixes the app relies
on). The app refuses to start an engine whose `--version` reports an older
build number; locally built binaries without tags report `0` and are accepted.

## Resolution order (`doculens_core::engine::locate`)

1. `DOCULENS_LLAMA_SERVER` (+ `DOCULENS_LLAMA_SERVER_BACKEND`) — development and tests
2. `settings.json` → `models.engine_binary` (model manager → "custom binary")
3. newest `engines/<version>/<pack>/` whose pack matches the backend rule
   (NVIDIA + driver CUDA ≥ 12.8 → `cu129`, ≥ 12.1 → `cu121`, any GPU → `vulkan`, else `cpu`;
   the user's backend preference can force one)
4. `engines/<pack>/` next to the executable or in the Tauri resource directory (bundled)

## Placing the official packs

```bash
node scripts/fetch-engine.mjs --tag b12066 --packs cpu,vulkan
```

The script reads the release asset list from the GitHub API, downloads the
matching zips, verifies SHA-256 against `engines/manifest.json` (prints the
hashes with `--pin` so they can be recorded the first time), and unpacks into
`<app-data>/engines/<tag>/<pack>/`. `--dest <dir>` overrides the destination,
`--linux` fetches the `ubuntu-x64` build for CI.

## Building the CUDA packs (user PC, Windows)

```powershell
git clone --depth 1 --branch b12066 https://github.com/ggml-org/llama.cpp
cmake -B build-cu121 -DGGML_CUDA=ON -DCMAKE_CUDA_ARCHITECTURES="61;70;75;80;86;89;90" -DLLAMA_CURL=OFF -DLLAMA_BUILD_TESTS=OFF
cmake --build build-cu121 --config Release --target llama-server -j
# copy build-cu121/bin/Release/* + cudart64_12.dll, cublas64_12.dll, cublasLt64_12.dll
#   → %LOCALAPPDATA%\DocuLens\engines\b12066\cu121\
```

The CUDA runtime DLLs stay inside that pack directory (NVIDIA EULA: app-private,
never on the system PATH). `cu129` is the same with the 12.9 toolkit and
`-DCMAKE_CUDA_ARCHITECTURES="75;80;86;89;90;120"`.

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
backend registers a device, takes over the graph and — for Qwen3.5's hybrid
layers — fails with "Compute error". This is exactly what a too-loose download
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
