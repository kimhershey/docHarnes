# DocuLens 실행 가이드 (v1.0 · P0–P5)

DocuLens를 **실제 Windows PC에서 설치·실행·빌드·검증**하는 절차를 한곳에 모았습니다.
설계는 `docs/design/doculens-design-v1.1.md`, 구현 현황은 `docs/HANDOFF.md`,
사용자 PC에서 직접 확인해야 하는 항목은 `docs/user-pc-checklist.md`를 보세요.

> 한눈에: ① 도구 설치 → ② `npm install` → ③ 엔진 팩·PDFium 받기 → ④ `npm run tauri dev`
> → ⑤ 온보딩 3단계(워크스페이스 → 하드웨어 → 모델) → ⑥ 문서 가져오기·처리 → ⑦ 어시스턴트 명령.

## 1. 실행 방식 세 가지

| 방식 | 명령 | 필요 조건 | 용도 |
|---|---|---|---|
| A. 데스크톱 개발 실행 | `npm run tauri dev` | Windows 10/11, Node 22, Rust stable(MSVC), VS Build Tools C++, WebView2 | 실제 엔진·모델·파일 처리까지 전 기능 |
| B. 브라우저 미리보기 | `npm run dev` → http://localhost:1420 | Node 22만 | 화면 흐름 확인(모의 백엔드 — 실제 파일·모델 처리 없음) |
| C. 설치파일 빌드 | `npm run tauri build` → `target\release\bundle\nsis\DocuLens_0.1.0_x64-setup.exe` | A와 동일(첫 빌드 때 NSIS를 자동으로 받음) | 다른 PC에 설치(사용자 단위 설치, 관리자 권한 불필요) |

- 첫 `tauri dev`/`tauri build`는 Rust 의존성 전체를 컴파일하므로 수 분~십여 분 걸립니다. 이후에는 증분 빌드입니다.
- GitHub Actions의 "Windows installer (manual)" 워크플로를 수동 실행하면 같은 설치파일(Lite)을 아티팩트
  `DocuLens-Setup-Lite`로 받을 수 있습니다(런타임 자산은 포함되지 않음 — §2.2).
- Linux 실행은 개발·CI 전용입니다(배포 범위 외). 필요한 패키지는 `CLAUDE.md`의 "Linux build deps".

## 2. 준비물 (Windows)

### 2.1 개발 도구

- **Node.js 22** LTS (`node --version`)
- **Rust stable** (rustup, 대상 `x86_64-pc-windows-msvc`, 1.85 이상)
- **Visual Studio 2022 Build Tools** — "C++를 사용한 데스크톱 개발" 워크로드(MSVC + Windows SDK)
- **WebView2 Runtime** — Windows 11과 최신 Windows 10에는 기본 포함. 없으면 설치파일이 자동으로 받습니다.
- Git

```powershell
git clone <저장소 URL> docHarnes
cd docHarnes
npm install
```

### 2.2 런타임 자산 (저장소에 포함되지 않음 — 앱이 실행 중에 찾는 파일)

| 자산 | 받는 방법 | 설치 위치 | 없을 때 |
|---|---|---|---|
| **llama-server 엔진 팩** | `node scripts/fetch-engine.mjs --tag b12066 --packs cpu,vulkan --pin` | `%LOCALAPPDATA%\DocuLens\engines\b12066\cpu\`, `...\vulkan\` | 엔진 카드에 "실행 파일을 찾지 못했습니다" → 분류·OCR·추출·답변 불가 |
| **CUDA 팩**(NVIDIA, 선택) | `engines/README.md`대로 llama.cpp b12066을 CUDA 12.1(또는 12.9) 툴킷으로 자체 빌드 | `...\engines\b12066\cu121\` (`cu129`) | Vulkan/CPU 팩으로 동작(느림) |
| **PDFium**(PDF 렌더·텍스트 레이어) | `node scripts/fetch-pdfium.mjs` | `%LOCALAPPDATA%\DocuLens\pdfium\pdfium.dll` | PDF 가져오기·렌더 불가, 라이브러리 상단 "PDFium 없음" 경고 |
| **ko-dic**(한국어 형태소 사전, 선택) | `node scripts/fetch-kodic.mjs` (Rust 필요, 약 82 MB, 수 분) | `%LOCALAPPDATA%\DocuLens\tokenizers\ko-dic\` | 간이 분석기로 동작(검색 재현율이 낮음) |
| **VLM 모델**(GGUF + mmproj) | §2.3 | `<워크스페이스>\.doculens\models\<폴더>\` | 가져오기·검색·정리·내보내기만 가능 |

- `--pin`은 받은 zip의 SHA-256을 `engines/manifest.json`에 기록합니다(첫 PC에서 한 번).
- 설치파일에 PDFium·ko-dic을 동봉하려면 빌드 전에 `--bundle`로 받습니다(`src-tauri/resources/`에 들어감).
- **팩 폴더 하나에는 백엔드 계열 하나만** 둡니다. CPU 팩 폴더에 `ggml-vulkan.dll`·`ggml-cuda.dll`이 섞이면 다른 백엔드가
  그래프를 가로채 "Compute error"가 납니다. 엔진은 b11747 미만 빌드를 거부합니다.
- 백엔드 선택: NVIDIA 드라이버의 CUDA 12.8+ → `cu129`, 12.1~12.7 → `cu121`, 그 외 GPU → `vulkan`, GPU 없음 → `cpu`.
  모델 관리자 엔진 카드에서 강제할 수 있고 "실행 파일 직접 지정"으로 임의 경로도 가능합니다.

### 2.3 모델 준비

`models/registry.json`의 항목(모두 Apache-2.0):

| id | 표시 이름 | 대상 티어 | 파일 |
|---|---|---|---|
| `qwen3.5-4b-q5km` | Qwen3.5-4B (Q5_K_M, 균형) | 8 GB | `Qwen3.5-4B-Q5_K_M.gguf` + `mmproj-F16.gguf` |
| `qwen3.5-4b-q6k` | Qwen3.5-4B (Q6_K, 정확도) | 8·12 GB | `Qwen3.5-4B-Q6_K.gguf` + `mmproj-F16.gguf` |
| `qwen3.5-4b-q4km` | Qwen3.5-4B (Q4_K_M, 저여유) | 6·8 GB | `Qwen3.5-4B-Q4_K_M.gguf` + `mmproj-F16.gguf` |
| `qwen3.5-2b-q8` | Qwen3.5-2B (Q8_0, CPU) | CPU 전용 | `Qwen3.5-2B-Q8_0.gguf` + `mmproj-F16.gguf` |
| `qwen3.5-9b-q5km` / `q6k`, `qwen3.5-27b-q4km` | 상위 티어 | 12·16·24 GB+ | — |
| `qwen3-vl-4b-q4km` | Qwen3-VL-4B (대체 모델) | 8·12 GB | — |
| `embed-bge-m3-q8` | BGE-M3 (임베딩, CPU) | 전 티어 | `bge-m3-Q8_0.gguf` — 지식 베이스 의미 검색용(선택) |

넣는 방법은 셋 중 하나입니다.

1. **드롭인**: 파일을 `<워크스페이스>\.doculens\models\<아무 이름>\`에 복사합니다. 복사가 끝나면 2초 안에 "준비됨"으로
   등록됩니다(복사 중에는 "복사 중"). 가장 단순하고 USB로 옮길 때 권장.
2. **모델 관리자 > GGUF 가져오기**: 복사/이동/하드링크 선택, 같은 폴더의 mmproj를 자동으로 짝지음, LICENSE 동봉.
3. **모델 관리자 > 다운로드**: 네트워크가 허용된 PC에서 Hugging Face에서 받아 SHA-256으로 검증합니다(`settings.json`의
   `models.allow_online_download`로 끌 수 있음, 기본 켜짐 — 화면에는 토글이 없음). 현재 레지스트리는 크기·해시가 미고정(`verified: false`)이므로 처음 받은 뒤 "해시 검증"으로 얻은 값을
   `models/registry.json`에 고정해 주세요.

8 GB GPU면 `qwen3.5-4b-q5km`(32K 컨텍스트) 또는 `q6k`, GPU가 없으면 `qwen3.5-2b-q8`(8K)을 권장합니다.

## 3. 첫 실행 흐름

1. **온보딩 3단계** — 워크스페이스 폴더(예 `C:\Work\Docs`; 드라이브 루트·홈 폴더는 거부) → 하드웨어 감지(GPU·VRAM·티어·
   권장 모델) → 모델 준비(다운로드 / GGUF 가져오기 / 나중에). 선택한 경로는 `%LOCALAPPDATA%\DocuLens\settings.json`의
   `active_workspace`에 저장되고 설정 > 워크스페이스에서 바꿀 수 있습니다.
2. 워크스페이스 폴더에 `inbox\`, `documents\`, `exports\`, `.doculens\`(DB·모델·캐시·추출 미러·색인·저널·휴지통·작업·로그)가
   만들어집니다. DB는 상대 경로만 저장하므로 폴더를 통째로 옮기거나 USB로 가져갈 수 있습니다.
3. **모델 관리자** — 엔진 카드에 실행 파일·버전(b12066)·백엔드·설치된 팩이 보여야 합니다. 워크스페이스를 열면 엔진이 자동
   시작되고(기본 켜짐) 활성 모델이 로드됩니다. 상태바가 "엔진: 준비"가 되면 "모델 테스트"로 한국어 응답과 tok/s를 확인하세요.
   "장치 조회"는 엔진이 보는 GPU와 가용 VRAM을 보여 줍니다.
4. **문서 라이브러리**(Ctrl+2) — 파일을 드롭하거나 가져오기(PDF·JPG/PNG·HWPX·DOCX·HWP) → 미리보기(신규/중복/미지원) →
   가져오기 작업 → 썸네일 → 뷰어(이미지·텍스트·필드·표·이력 탭). `inbox\`에 파일을 넣으면 3초 안정화 후 자동 등록되고,
   설정 > 처리의 "inbox 자동 처리 수준"(분류만 / 전체)에 따라 자동 처리됩니다.
5. **처리 ▾ → 표준 처리** — 분류 → OCR → 추출 → 검증 → 검토 큐. 검토 큐에서 Enter 승인, E 수정, S 보류, N 다음. 필수 필드를
   모두 처리하면 문서가 "검토 완료"가 됩니다.
6. **지식 베이스** — 적재 마법사(파일·폴더 → 컬렉션 → 미리보기)로 규정·지침을 넣으면 가져오기 → 청킹 → 색인이 이어지고,
   bge-m3가 설치돼 있으면 임베딩도 백그라운드로 돕니다. 어시스턴트 질문은 문장별 출처 칩과 함께 답하고, 근거가 없으면
   "근거 없음" 배너가 뜹니다.
7. **어시스턴트**(Ctrl+3) — 자연어 명령 → 플랜 카드(출처: 문장 규칙 또는 모델, 영향 미리보기) → Ctrl+Enter 승인 / Esc 취소 →
   실행 카드(진행률·일시정지·취소) → 결과 카드(되돌리기·폴더 열기). 정리 / 내보내기 화면과 작업 화면은 같은 작업 큐를 씁니다.
8. 사이드바의 "문서 생성"·"프로젝트"는 2차 범위(v1.1) 자리표시자입니다.

## 4. 어시스턴트 명령·슬래시·단축키

**설계 예시 명령 12종**(규칙으로 해석되어 모델 없이도 플랜 카드가 뜸):

- "inbox 폴더의 영수증을 전부 처리하고 월별 폴더로 정리해줘"
- "세금계산서에서 공급가액·세액·합계만 뽑아서 엑셀로" / "세금계산서 → 엑셀"
- "지난달 영수증 합계 얼마야?"
- "거래처명이 ㈜한빛인 문서 찾아줘" / "find documents from Hanbit"
- "이 계약서의 계약기간과 위약금 조항 알려줘" (뷰어에서 문서를 연 상태)
- "사업자등록증 스키마에 '사업자단위과세' 필드 추가해"
- "다크 테마로 바꾸고 모델을 9B로 올려"
- "선택 영역 다시 OCR 해줘"
- "방금 이름 바꾼 거 되돌려", "작업 일시정지"

규칙이 못 푸는 문장(예: "한빛과 거래한 계약 중 작년 것만 골라서 보여줘")은 로드된 모델이 플랜을 만듭니다(문법 제약 JSON,
검증 실패 시 재질의 1회, 그래도 안 되면 명확화 질문). 문서를 바꾸는 명령에 범위가 없으면 "전부"라고 말하지 않은 한
명확화로 돌아갑니다.

**슬래시 명령**: `/테마 다크` (`/theme dark|light|system|high-contrast`), `/언어 en` (`/lang ko|en`), `/열기 설정`
(`/open models|jobs|…`, 화면 이름만 써도 됨: `/jobs`), `/도움말` (`/help`).

**기본 단축키**(설정 > 단축키에서 변경):

| 동작 | 키 | 동작 | 키 |
|---|---|---|---|
| 명령 팔레트 | Ctrl+K | 플랜 승인 | Ctrl+Enter |
| 닫기·취소 | Esc | 되돌리기 / 다시 실행 | Ctrl+Z / Ctrl+Y |
| 홈 / 라이브러리 / 어시스턴트 | Ctrl+1 / 2 / 3 | 설정 | Ctrl+, |
| 테마 전환 | Ctrl+Shift+T | 선택 문장 설명 | Ctrl+E |
| 검토: 승인 / 수정 / 보류 / 다음 | Enter / E / S / N | 확대 / 축소 / 원래대로 | Ctrl+= / Ctrl+- / Ctrl+0 |
| UI 배율 | Ctrl+Shift+= / Ctrl+Shift+- | | |

한글 입력 조합 중에는 Enter·Ctrl+Enter가 전송·승인으로 새지 않도록 처리돼 있습니다(사용자 PC에서 확인 항목).

## 5. 데이터 위치

| 내용 | 위치 |
|---|---|
| 설정(활성 워크스페이스·테마·언어·모델 설정·단축키) | `%LOCALAPPDATA%\DocuLens\settings.json` |
| 엔진 팩 / 라우터 프리셋 / 로그 | `%LOCALAPPDATA%\DocuLens\engines\`, `presets.ini`, `logs\` |
| PDFium / ko-dic | `%LOCALAPPDATA%\DocuLens\pdfium\`, `tokenizers\ko-dic\` |
| 워크스페이스 DB(SQLite: 문서·필드·청크·FTS·벡터·작업·저널) | `<ws>\.doculens\workspace.db` |
| 모델(드롭인) | `<ws>\.doculens\models\<폴더>\` |
| 페이지·썸네일·변환본 캐시 | `<ws>\.doculens\cache\{pages,thumbs,converted}\<doc_id>\` |
| 추출 결과 미러 | `<ws>\.doculens\extractions\<doc_id>\<run_id>\` |
| 정리·이름변경 저널 | `<ws>\.doculens\journal\ops-YYYYMM.jsonl` (+ DB `journal_batches`/`ops_journal`) |
| 워크스페이스 휴지통(USB 등 OS 휴지통 불가 시) | `<ws>\.doculens\trash\` |
| 자동 가져오기 폴더 / 정리된 문서 / 내보내기 | `<ws>\inbox\`, `<ws>\documents\`, `<ws>\exports\` |

네트워크로 나가는 것은 사용자가 직접 누른 모델 다운로드만입니다(텔레메트리 없음).

## 6. 개발자 검증 명령

| 명령 | 내용 | 2026-11-08 결과(Linux CI 환경) |
|---|---|---|
| `cargo fmt --all -- --check` | 서식 | 통과 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 린트 | 통과 |
| `cargo test --workspace` (`DOCULENS_PDFIUM`·`DOCULENS_KODIC` 설정 시 렌더·형태소 테스트 포함) | Rust 단위·통합 | 205 통과(3 ignored = 생성기) + 프로세스 테스트 5 |
| `cargo deny check licenses bans sources` | Rust 라이선스·중복·출처 게이트 | 통과 |
| `npm run typecheck` / `npm test` / `npm run build` | TS 타입·vitest·프로덕션 빌드 | 통과 / 12파일 66건 / 통과 |
| `npm run licenses:check` | npm 라이선스 게이트 | 모두 허용 |
| `npm run e2e` | Playwright(브라우저, 모의 백엔드) | 28/28 |
| `DOCULENS_LLAMA_SERVER=<llama-server> DOCULENS_TEST_MODELS_DIR=$PWD/crates/doculens-core/tests/fixtures cargo test -p doculens-core --test real_engine -- --nocapture` | 실제 llama-server + 초소형 Qwen3.5 픽스처(채팅·KIE·플래너 문법) | 통과 |
| `DOCULENS_FIXTURE_OUT=<dir> cargo test -p doculens-core --lib -- --ignored gen_fixtures` | 샘플 입력 문서 생성 | — |
| `DOCULENS_EXPORT_OUT=<dir> cargo test -p doculens-core --lib -- --ignored gen_export_samples` | 내보내기 5종 샘플 파일 생성(XLSX·CSV·JSONL·MD·검색 가능 PDF) | 9개 파일 |

GitHub Actions `CI`(푸시·PR)가 위 항목을 License gate / Web / Real llama-server / Rust Linux / Windows compile 다섯 작업으로
실행합니다. Windows 실창 E2E(tauri-driver)와 GPU 실측은 사용자 PC 항목입니다.

개발 중 유용한 환경 변수: `DOCULENS_LLAMA_SERVER`(엔진 실행 파일), `DOCULENS_LLAMA_SERVER_BACKEND`, `DOCULENS_PDFIUM`,
`DOCULENS_KODIC`, `DOCULENS_APP_DATA`(앱 데이터 폴더를 바꿔 격리 실행).

## 7. 문제 해결

| 증상 | 원인 | 조치 |
|---|---|---|
| 엔진 카드 "llama-server 실행 파일을 찾지 못했습니다" | 엔진 팩 미설치 | §2.2의 `fetch-engine.mjs`로 설치하거나 "실행 파일 직접 지정" |
| 엔진 카드 "b11747 이상 필요" | 오래된 llama.cpp 빌드 | b12066 팩으로 교체 |
| 로드 직후 "Compute error" / 엔진 반복 종료 → "중단됨" | 팩 폴더에 두 백엔드 계열이 섞임, 또는 VRAM 부족 | 팩 폴더 정리(한 계열만) → 재시작; 작은 양자화·컨텍스트나 CPU 백엔드로 변경 |
| 라이브러리 상단 "PDFium 없음" | `pdfium.dll` 미설치 | `node scripts/fetch-pdfium.mjs` 후 재시작 |
| 모델 폴더가 "복사 중"에 머묾 | 파일이 아직 쓰이는 중(안정화 대기) | 복사 완료 대기; 2초 넘게 변하지 않으면 폴더 이름 확인(GGUF + mmproj) |
| 다운로드 버튼 비활성 | `settings.json`의 `models.allow_online_download`가 false이거나 오프라인 | 값을 true로 바꾸거나 USB 드롭인 사용 |
| 작업 로그 "prompt exceeds the context" / KIE·플래너 400 | 프리셋 컨텍스트(8K)보다 긴 입력 | 플래너는 작은 프롬프트로 자동 재시도("retrying smaller"); 반복되면 모델 컨텍스트 프리셋 확대 |
| 엔진 로그 "does not match the expected … format"(500) | 문법 제약 출력 파서 오류 | 호출기가 seed·온도를 바꿔 자동 재시도(작업 로그 "kie attempt N"); 지속 시 보고 |
| 작업 화면에 "중단됨" | 앱이 실행 중 작업을 남기고 종료 | 작업 화면에서 "재시도" |
| "다른 인스턴스가 이 워크스페이스를 사용 중" | 잠금 파일 | 다른 창이 없으면 "강제로 열기" |
| 되돌리기 항목이 "불가(변경됨/없음)" | 정리 뒤 탐색기에서 파일을 수정·이동 | 해당 항목만 건너뛰고 나머지는 복구됨 |
| OneDrive·네트워크 폴더에서 느림 | SQLite가 `journal_mode=DELETE`로 전환 | 로컬 폴더 워크스페이스 권장 |

## 8. 사용자 PC에서 확인할 순서

`docs/user-pc-checklist.md`의 항목을 처음 한 번은 아래 순서로 밟는 것을 권장합니다(약 1~2시간, 모델 다운로드 제외).

1. **환경** — 창이 뜨고 온보딩 → 홈이 동작하는가, `settings.json`·워크스페이스 폴더 생성, 테마 4종·한/영 전환, 한글 IME 조합 중
   Enter 미전송, UI 배율 (P0 항목).
2. **엔진·모델** — 팩 설치와 SHA-256 고정(`--pin`), 엔진 카드의 버전·백엔드·장치 조회(VRAM), 실제 Qwen3.5-4B 드롭인 → "준비됨" →
   로드 → 상태바 VRAM 사용량, 모델 테스트의 한국어 응답과 tok/s(RTX 4060 기대 50~80), 강제 종료 후 재기동(1회)·자식 프로세스
   잔존 없음, `<think>` 미출력 (P1).
3. **문서** — 100쪽 이상 스캔 PDF 가져오기 → 썸네일 → 뷰어 속도(200 DPI 쪽당 ≈1초), 실제 HWPX/DOCX/HWP 변환본, EXIF 회전 JPG,
   inbox 자동 등록, 삭제 → 휴지통 (P2).
4. **지식 베이스** — ko-dic 표시, bge-m3 임베딩, 규정 적재 → 조항 경로, 한국어 검색(조사·2자·복합명사·숫자), 출처 칩 → 쪽 이동,
   근거 없음 배너, 설명·요약 (P3).
5. **파이프라인** — 표준 처리 끝까지(쪽당 30~50초 GPU 기대), 분류·OCR 마스킹·KIE 합계 검증, 검토 큐 키, 양식 등록, NL 스키마,
   inbox 전체 자동 처리, 분할/병합, 출력 파서 500 여부, 8K 프리셋에서 긴 쪽 처리 (P4).
6. **자연어·작업** — 예시 명령 12종 플랜 카드, 모델 플랜과 컨텍스트 축소 재시도, 플랜 편집 후 재검증, 실제 파일 이동과 되돌리기
   (탐색기에서 바꾼 항목만 건너뜀), 내보내기 5종을 Excel/Acrobat에서 열어 한글 확인(CSV BOM, PDF Ctrl+F), 집계의 검토 완료/대기
   분리, 배치 중 질문 끼어들기(쪽 경계), 일시정지·재개·우선순위·재시도, 강제 종료 후 "중단됨"과 대기열 복원, 저널 파일과
   `jobs` 테이블 (P5).
7. **골든셋**(사용자 제공) — 분류 F1·OCR CER·KIE 필드 정확도·플래너 명령→플랜 100건.

확인 결과는 이슈로 남기거나 다음 세션에 전달해 주세요. 특히 **레지스트리 크기·SHA-256 고정**(모델·엔진 팩)은 첫 PC에서
한 번만 하면 이후 다운로드 검증이 자동으로 적용됩니다.
