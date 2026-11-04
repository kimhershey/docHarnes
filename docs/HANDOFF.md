# Handoff — DocuLens implementation status

> 단계마다 새 세션으로 진행합니다. 다음 세션은 이 파일과 `CLAUDE.md`,
> `docs/design/doculens-design-v1.1.md` 만 읽으면 이어서 작업할 수 있습니다.

## 범위 결정 (2026-11-03)

- **1차(v1.0)**: P0 ~ P4 + 플래너 핵심 op (import, classify, ocr, extract,
  validate, review, search, answer, summarize, organize/rename/move, export,
  set_theme/set_language/open/help 등). 토큰 예산 ≈ 400~550만.
- **2차(v1.1)**: 문서 자동 생성(§3.13), 프로젝트 정합성·보완요청서(§3.15),
  그 외 §4.0 "다음" 항목.
- 작업 방식: 단계별 새 세션 + 데모 체크포인트, GitHub 커밋/푸시, 빌드 로그
  축약·모듈 단위 컴파일 검증, 리뷰 서브에이전트는 단계 말에만.

## 단계 현황

| 단계 | 상태 | 비고 |
|---|---|---|
| P0 스캐폴드 | 완료 (`main`) | Tauri 2 셸, 테마 4종, i18n, 설정/워크스페이스, CI |
| P1 엔진·모델 | **완료 (브랜치 `phase/p1-engine`)** | 아래 "P1 산출물" 참조 |
| P2 워크스페이스·문서 | 다음 | SQLite 마이그레이션, 가져오기, HWP/HWPX/DOCX 변환, pdfium 렌더, 뷰어 |
| P3 지식 베이스 | 대기 | 청킹, lindera FTS5 + trigram, bge-m3 + sqlite-vec, 근거 기반 Q&A |
| P4 파이프라인 | 대기 | 분류·OCR·KIE, 양식 등록, 검증기, 검토 큐 |
| P5 자연어·작업 | 대기(핵심 op만 1차) | 플래너 PlanSchema, 확인 카드, Job 러너, 정리/되돌리기, 내보내기 |

## P1 산출물

### Rust 코어 (`crates/doculens-core`)

- `gguf`: GGUF v2/v3 헤더·메타데이터·텐서 디스크립터 파서(텐서 데이터 미로드, 대형 배열은
  길이만 보관), `ModelMeta` 요약(아키텍처, 양자화 라벨, 라이선스, chat_template 유무,
  block_count / head_count_kv / key_length / full_attention_interval → `memory::ModelArch`,
  split 정보, mmproj 프로젝터 타입, GDN 민감 텐서(`ssm_alpha/beta/a/dt/norm/conv1d`) 정밀도
  경고), SHA-256 + BLAKE3 단일 패스 해시(취소 가능), 2 MiB 지문(변경 감지), 테스트/픽스처용
  최소 GGUF 라이터.
- `modelstore`: 드롭인 폴더 스캔(`<models>/<폴더>/*.gguf` + `mmproj*.gguf` + `model.json` +
  LICENSE), split 세트 그룹화·누락 검출, 복사 중(최근 mtime) 판정, 레지스트리 매칭(해시 우선 →
  파일명), `ModelHealth {ready, copying, unsupported, incomplete, invalid}`, 경고 키
  (`models.warn.*`), 해시 캐시(`.doculens-hashes.json`), `install_state`(P0 이름 검사 대체),
  미등록 모델의 `user_added` 레지스트리 항목 합성(`merged_registry`), 가져오기 계획/실행
  (복사·이동·하드링크, mmproj 자동 짝짓기, LICENSE 동반 복사, `.part` → rename, 취소 시 정리),
  이어받기 다운로드(Range, SHA-256 검증, `ureq` + 플랫폼 TLS), `notify` 폴더 감시(2초 안정화).
- `engine::locate`: 실행 파일 탐색 순서(`DOCULENS_LLAMA_SERVER` → 설정 `engine_binary` →
  `engines/<버전>/<팩>` 최신 → 번들 폴더), `--version` 빌드 번호 파싱, 최소 빌드 b11747 검사
  (태그 없는 로컬 빌드 0은 허용).
- `engine::devices`: `--list-devices` 파싱 → `GpuInfo(source = engine)`.
- `engine::presets`: `presets.ini` 생성기(§3.3.2). 전역 `[*]`(flash-attn, q8_0 KV, fit=off,
  image-min-tokens 1024, jinja, metrics, slots=false) + 모델별 섹션(절대경로 model/mmproj,
  프로파일별 ctx/image-max-tokens, 12 GB+ 2슬롯 kv-unified, 샘플링, `reasoning = off` +
  `reasoning-budget = 0`, CPU 백엔드 n-gpu-layers 0, 감사 모드 mmproj-offload=false, 임베딩
  섹션). INI에 표현 불가한 경로(`;` `#`)는 건너뛰고 사유 보고.
  **설계 §3.3.2와의 차이**: 이 빌드(b12066)는 `--reasoning off`가 존재하며 `chat-template-kwargs`
  방식은 deprecated 경고를 내므로 `reasoning = off`를 사용. `--no-webui`는 존재하지 않아 생략.
- `engine::client`: 라우터 API 블로킹 클라이언트(`/health`, `/models[?reload=1]`,
  `/models/load|unload`, `/models/sse` 구독, `/v1/chat/completions` 비스트리밍·스트리밍,
  `/v1/embeddings`, `/props`, `/metrics`), OpenAI 호환 `ChatRequest`(json_schema, 이미지
  data URL 헬퍼), 메모리 실측 합산(`memory.effective.devices[kind=device]`).
- `engine::process`: 스폰 격리 — Windows `CreateJobObject` + `KILL_ON_JOB_CLOSE` +
  `CREATE_NO_WINDOW`, Unix `setpgid` + `killpg` 종료(앱 종료 `RunEvent::Exit`에서 명시적 stop),
  stdout/stderr 라인 펌프. `PR_SET_PDEATHSIG`는 스폰한 *스레드*가 끝나면 발동하므로 사용하지
  않음(헤드리스 검증에서 자동 시작 스레드 종료 직후 엔진이 SIGKILL 되는 현상으로 확인).
- `engine::manager`: 생명주기 상태기계(Stopped/Starting/Running/Restarting/Degraded/Stopping),
  랜덤 루프백 포트, API 키는 `LLAMA_API_KEY` 환경변수로만 전달, `/health` 대기, SSE 스레드,
  크래시 감시 + 지수 백오프(1·2·4초, 3회) 후 Degraded, 60초 이상 정상 가동 시 카운터 리셋,
  재기동 후 활성 모델 재로드, `load_model`(상태 폴링 + 워밍업 1토큰), `unload_model`(라우터의
  비동기 중지 대기), 로그 링(1,000줄), 이벤트 구독.
- `engine::fake_router`: std 전용 가짜 llama-server(라우터 API 부분집합 + SSE + 스트리밍 채팅 +
  `--version`/`--list-devices`/`--models-preset`). 프로세스 수준 테스트는 테스트 실행 파일이
  스스로를 가짜 서버로 재실행(`DOCULENS_FAKE_ROUTER=1`).
- 설정 추가: `models.auto_start_engine`, `warmup_after_load`, `vision_on_cpu`, `engine_binary`.
  오류 코드 추가: `invalid_gguf`, `engine_not_found`, `engine`, `engine_not_running`,
  `download`, `cancelled`, `timeout`.

### Tauri 셸 (`src-tauri`)

- `engine_bridge`: 워크스페이스 변경 시 스캔 + 폴더 감시 + (설정 시) 엔진 자동 시작과 활성 모델
  로드, `presets.ini` 재생성(`<AppData>/DocuLens/presets.ini`) + `?reload=1`, 엔진 이벤트 →
  `engine://status|log|model`, 스캔 → `models://changed`, 작업 → `jobs://update`.
- `jobs`: P1용 경량 작업부(엔진 시작·모델 로드·가져오기·다운로드·해시, 진행률/취소/정리).
  P5의 Job 러너가 대체하되 이벤트 형태는 유지.
- 커맨드 23 → 44: `models_scan/list/import_plan/import_run/download/hash/delete`,
  `engine_status/start/stop/restart/load_model/unload_model/refresh_models/chat_test/list_devices/
  binary_info/logs`, `presets_preview`, `jobs_list/cancel/clear_finished`. `hw_assess`·`registry_list`는
  병합 레지스트리 + 스캔 기반 설치 상태를 반환. 앱 종료 시 엔진 정지.

### 프런트엔드

- 모델 관리자: 엔진 카드(상태 배지, 시작/중지/재시작/모델 테스트, 실행 파일·버전·백엔드·주소,
  로드된 모델과 메모리 실측, 설치된 팩, 자동 시작/워밍업/감사 모드/유휴 슬립/상주 수/실행 파일
  직접 지정, 로그·presets.ini·장치 조회), 드롭인 폴더 표(상태·매칭·경고·로드/언로드/해시/삭제),
  레지스트리 표(설치 상태·검증·다운로드·로드·활성화), GGUF 가져오기 다이얼로그(경로 입력 또는
  네이티브 선택, 검증 계획 → 실행), 모델 테스트 다이얼로그.
- 작업 화면(진행 중/완료, 진행률·바이트·취소·정리), 상태바(엔진 ● 상태/모델/VRAM 사용/큐),
  상단바 작업 수, 온보딩 3단계(다운로드 시작·GGUF 가져오기 활성화, 설치됨 표시).
- 브라우저 모의 백엔드가 엔진 생명주기·모델 폴더·가져오기/다운로드/해시·작업을 타이머로
  시뮬레이션(`doculens.mock.fast`로 10배속). 이벤트 구독 추상화(`subscribe()`).

### 테스트·검증

| 종류 | 수 | 비고 |
|---|---|---|
| `cargo test -p doculens-core` (lib) | 62 | gguf 5, modelstore 10, engine 12 포함 |
| `tests/engine_process.rs` (harness=false) | 5 | 실제 자식 프로세스: 생명주기·startup 로드/취소·크래시 재기동·크래시 루프·재기동 실패 Degraded |
| `tests/real_engine.rs` | 1 | `DOCULENS_LLAMA_SERVER` + `DOCULENS_TEST_MODELS_DIR` 설정 시에만 실행 |
| vitest | 27 | i18n 패리티 + 정적 t() 키 해석 검사 포함 |
| Playwright | 10 | P0 7 + P1 3 (가져오기→로드→상태바, 다운로드·중지/시작·삭제, 온보딩 3단계·영어) |

- **실제 llama-server 검증(이 환경, CPU)**: llama.cpp `b12066` 소스를 CPU로 빌드하고,
  `scripts/gen-tiny-qwen35.py`로 만든 2층 랜덤 초기화 Qwen3.5 VLM(텍스트 660 KB + mmproj 195 KB,
  `crates/doculens-core/tests/fixtures/tiny-qwen35/`)으로 `real_engine` 테스트 통과:
  라우터 기동 ≈170 ms → `/models` 목록 → 로드 ≈430 ms(워밍업 포함) → 텍스트 채팅(8토큰,
  timings 수신) → 스트리밍(델타 수신) → json_schema 제약 출력(`{"ok": true}`) → 이미지 요청
  (프롬프트 토큰 1,060, mmproj 경로 확인) → 언로드 → 재시작 → 정지. 출력 텍스트는 랜덤
  가중치라 무의미하며 경로 검증 용도.
- **실제 데스크톱 바이너리 헤드리스 확인(Linux, Xvfb)**: `tauri build --debug --no-bundle` 바이너리를
  `DOCULENS_APP_DATA` 픽스처(워크스페이스 `.doculens/models/tiny-qwen35/` + 설정 `active_model`,
  `auto_start_engine`)와 `DOCULENS_LLAMA_SERVER`로 실행 → 워크스페이스 자동 열기 → 엔진 자동 시작
  (`engine ready`) → 활성 모델 `load-on-startup` 경로로 로드 → 워밍업 완료, 폴더 감시 루프·IPC 오류 없음.
  이 확인에서 두 가지 결함을 잡았다: `PR_SET_PDEATHSIG`가 스폰 스레드 종료 시 엔진을 SIGKILL 하던 문제,
  엔진의 모델 mmap(inotify OPEN 이벤트)이 2초마다 재검색을 유발하던 문제.
- CI: `engine-real` 작업은 핀 고정 태그(`engines/manifest.json`)의 llama.cpp 소스를 받아 **CPU 전용
  정적 빌드**(`GGML_NATIVE=OFF`, `GGML_BACKEND_DL=OFF`)로 `llama-server`를 만들고 태그별로 캐시한 뒤
  같은 테스트를 실행한다(차단 작업). 릴리스 자산을 쓰지 않는 이유: b12066 릴리스의 Linux 패키지는
  CUDA·OpenVINO·ROCm뿐이고, 느슨한 패턴으로 받은 OpenVINO 패키지를 같은 폴더에 풀자 ggml 동적 로더가
  `libggml-openvino.so`를 불러와 `OPENVINO0` 장치로 그래프를 돌리다 Qwen3.5의 GDN/KV 연산에서
  `Compute error`를 냈다(**한 폴더에는 한 백엔드 계열만** — `engines/README.md`). 동적 CPU 변형 빌드
  (`GGML_CPU_ALL_VARIANTS`)는 기본 선택과 `haswell` 강제 모두 로컬 통과. Windows 작업은
  `cargo test -p doculens-core`로 프로세스 수준 테스트까지 실행하므로 Job Object 경로가 매 푸시마다
  검증된다(크래시 재기동 후 SSE 재연결 시 `/models` 재동기화로 Windows 경합 해결).

## P1 단계 말 리뷰 반영 (2026-11-04)

독립 리뷰 2건(Rust 코어·셸 11건, 프런트엔드 11건 + 메모)을 모두 반영했다. 핵심:

- **(치명) 다운로드 HTTPS 패닉**: `ureq`는 TLS 제공자를 자동 선택하지 않아 첫 `https://` 요청에서
  패닉(릴리스 `panic = "abort"` → 앱 종료). `TlsProvider::NativeTls` + 플랫폼 루트 인증서를 명시하고
  회귀 테스트 추가(`download_agent_has_a_tls_provider`). 상태 코드는 직접 처리(`http_status_as_error(false)`),
  본문 타임아웃은 읽기 단위(60초)로 변경, 206은 `Content-Range` 시작 오프셋 검증, 416은 크기가 맞을 때만 완료 처리.
- **로드 중/슬립 모델에 `/models/load` → 400**: 라우터는 loading/loaded/sleeping 모두 "already running"으로 거부.
  `load_model`은 해당 상태면 POST를 건너뛰고 usable(loaded|sleeping)까지 대기. 가짜 라우터도 400을 내고
  `load-on-startup`을 흉내내며, 프로세스 테스트 `load_on_startup_and_cancel`로 고정.
- **가져온/다운로드한 모델이 "복사 중"으로 고정**: 앱이 직접 쓴 파일은 mtime을 settle 윈도 밖으로 되돌리고
  (`mark_settled`), 스캔에 복사 중 모델이 있으면 settle 후 한 번 더 자동 재검색, `models_scan`은 변경 시
  presets 재생성 + 라우터 reload(`rescan_and_sync`).
- **재기동 실패 시 루프 중단**: 모니터가 실패한 재기동 시도도 횟수로 세어 `Degraded`까지 진행
  (`relaunch_failure_degrades` 테스트).
- 생명주기 직렬화: `start/stop/restart`와 모니터 재기동이 `lifecycle` 뮤텍스를 공유, `start()`는 실행 중이면 멱등,
  presets 재생성 + reload는 `AppState::presets_lock`으로 직렬화(`regenerate_and_reload`).
- Windows: `--version`/`--list-devices` 실행에도 `CREATE_NO_WINDOW` + 타임아웃 + stderr 드레인
  (`process::engine_command` / `run_with_timeout`).
- 해시 매칭: 가중치 파일 핀만 모델을 식별(여러 항목이 같은 mmproj를 공유), mmproj 불일치는 "미검증"일 뿐
  Invalid가 아님(`hash_match_ignores_shared_mmproj_pins`).
- 가져오기 계획은 서버에서 재계산(웹뷰 제공 plan 미신뢰), `model.json` pooling 화이트리스트, INI 값 전수
  검증, 비 UTF-8 경로 건너뜀, GGUF 중첩 배열 깊이 제한·산술 포화·데이터 절단 검사.
- 삭제는 공유 위반 대비 재시도, 로드 취소는 반로드 인스턴스를 언로드, 작업 스레드 패닉은 작업 실패로 기록,
  해시 진행률 누적 보정, 워크스페이스 전환 시 자동 시작 꺼져 있어도 presets reload.
- 프런트: `t("models.import")` 객체 키 오류(가져오기 버튼) 수정 + 정적 `t()` 키가 ko/en에서 문자열로 해석되는지
  검사하는 vitest 추가, 엔진 카드 입력은 blur/Enter 시 커밋(키 입력마다 settings.json 저장 금지, 상주 수 1~4),
  오류 코드 7종 지역화 + `engine_too_old` 안내, 로그 패널은 열릴 때만 구독하고 링 버퍼를 불러옴, 상태바
  "중지 중", 작업 취소는 취소 가능한 종류만, 상태 스냅샷이 이벤트를 덮어쓰지 않도록 시퀀스 검사, 접근성
  (select id/label, aria-label i18n, 상태 사유 텍스트 노출), 모의 백엔드 정합성(엔진 시작 작업, 정규화, 중지 중 틱).

## P2 착수 메모

1. `crates/doculens-core/src/db/`: rusqlite(bundled, MIT) + 마이그레이션(§3.6 스키마),
   `journal_mode=WAL`(네트워크/동기화 폴더 감지 시 DELETE), `workspace.db`.
2. 가져오기: 드롭/폴더/inbox 감시(`notify` 재사용), 중복 검사(BLAKE3 — `gguf::hash_file` 재사용 가능),
   미리보기 다이얼로그.
3. 변환: HWPX(zip+XML) → HTML → PDF, DOCX(docx-rs 계열 MIT) → HTML, HWP 5.0은 읽기 전용
   파서(라이선스 확인). 렌더: `pdfium-render` + pdfium-binaries(BSD-3), 페이지 캐시
   `cache/pages/<doc>/<page>.png`, DPI 200, 썸네일.
4. 라이브러리·뷰어 UI(가상 스크롤, 썸네일 스트립, 탭). `models://changed`·`jobs://update`와 같은
   이벤트 패턴으로 `documents://changed`.
5. P1에서 넘기는 숙제: 엔진 로그 파일(`<AppData>/logs/llama-server.log`, 현재는 링 버퍼만),
   `--list-devices` 기반 티어 재산정의 UI 반영(현재는 하드웨어 패널 새로고침 후 반영),
   `engines/manifest.json` SHA-256 고정, 다운로드 재개 시 `.part` 재검증.

## 알려진 제약

- HF 차단으로 `models/registry.json`의 크기·SHA-256 미고정(`verified: false`); 다운로드 검증은
  핀이 생기면 자동 적용. `engines/manifest.json`의 자산 해시도 미고정.
- GPU 없음: CUDA/Vulkan 팩, 실모델 VRAM 실측, `--list-devices` GPU 출력은 사용자 PC
  (`docs/user-pc-checklist.md` P1 항목).
- Linux 빌드는 CI 전용(배포 범위 외). Windows 실창 E2E(tauri-driver)는 사용자 PC.
- 모의 백엔드의 프리라우터는 Rust 구현의 부분 집합(브라우저 데모 용도).
- 라우터의 `memory` 실측은 CPU 전용 빌드에서는 비어 있음(GPU 장치가 있을 때만 채워짐).
