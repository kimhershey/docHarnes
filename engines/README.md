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
