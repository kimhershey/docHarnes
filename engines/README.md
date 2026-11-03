# engines/

Placement scripts and pinned hashes for the `llama-server` builds the app can
run (phase P1):

| pack | source | notes |
|---|---|---|
| `cpu` | official llama.cpp release (≥ b11747) `llama-<tag>-bin-win-cpu-x64.zip` | always installed (Lite) |
| `vulkan` | official release `…-bin-win-vulkan-x64.zip` | AMD / Intel / NVIDIA fallback |
| `cu121` | self-built with CUDA 12.1 toolkit (SASS sm_61…sm_90 + Turing) | Full installer |
| `cu129` | self-built with CUDA 12.9 toolkit (sm_120 Blackwell) | Full installer |

Runtime location: `%LOCALAPPDATA%\DocuLens\engines\<version>\<pack>\`.
Binaries are never committed to this repository.
