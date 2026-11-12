# Handoff — DocuLens implementation status

> 단계마다 새 세션으로 진행합니다. 다음 세션은 이 파일과 `CLAUDE.md`,
> `docs/design/doculens-design-v1.1.md` 만 읽으면 이어서 작업할 수 있습니다.
> 사용자 PC에서 설치·실행·빌드·검증하는 절차는 `docs/RUN-GUIDE.md`, 확인 항목은 `docs/user-pc-checklist.md`.

## 범위 결정 (2026-11-03)

- **1차(v1.0)**: P0 ~ P4 + 플래너 핵심 op (import, classify, ocr, extract,
  validate, review, search, answer, summarize, organize/rename/move, export,
  set_theme/set_language/open/help 등). 토큰 예산 ≈ 400~550만.
- **2차(v1.1)**: 문서 자동 생성(§3.13), 프로젝트 정합성·보완요청서(§3.15),
  그 외 §4.0 "다음" 항목.
- 작업 방식: 단계별 새 세션 + 데모 체크포인트, GitHub 커밋/푸시, 빌드 로그
  축약·모듈 단위 컴파일 검증, 리뷰 서브에이전트는 단계 말에만.
- 설치파일 빌드(Actions "Windows installer (manual)")는 **요청된 작업이 모두 끝나고 전체 검증(Rust 테스트·clippy·vitest·
  Playwright·푸시한 커밋의 CI)에 FAILED가 없음을 확인한 뒤에만** 실행한다(사용자 결정 2026-11-09). 작업 도중 빌드 금지.
- **버전별 브랜치**(사용자 결정 2026-11-10 "앞으로는 마이너와 패치 버전을 올려서 브랜치를 생성하고 작업하자"): 작업을 시작할 때 버전을 올리고
  (새 기능·구조 변경 = 마이너, 수정 = 패치; `Cargo.toml`과 **`Cargo.lock`**(`cargo update -w --offline` 또는 아무 cargo 빌드 — 고지 잡은
  `cargo metadata --locked`로 돈다)·`package.json`/lock·`src-tauri/tauri.conf.json`·모의 백엔드 버전 문자열·`npm run notices`)
  `main`에서 `release/<major.minor.patch>` 브랜치를 만들어 그 위에서 작업·WIP 푸시·설치파일 빌드를 한다(`v0.5.0` 같은 이름은 릴리스 태그와 겹치므로
  쓰지 않음). **버전 범프 커밋은 그 브랜치의 첫 병합 가능 상태**이므로 `[skip ci]` 없이 푸시해 CI를 확인한다(0.6.0의 범프 e6a2e4f는 `Cargo.lock`·SBOM이
  옛 버전인 채 CI를 건너뛰었고, 사용자가 PR #20으로 병합해 `main`의 고지/SBOM 잡이 깨졌다 — 통합 커밋 2efc181 이후 다시 병합하면 해소).
  끝나면 사용자가 PR로 `main`에 병합하고 게시 실행이 `v<버전>` 태그를 만든다. 첫 적용: `release/0.5.0`(`main` 4c2a7ca = PR #18까지 병합된
  `phase/p6-p7`에서 분기; `phase/*` 브랜치는 0.1.0~0.4.0과 0.5.0 초반의 기록으로만 남김).

## 단계 현황

| 단계 | 상태 | 비고 |
|---|---|---|
| P0 스캐폴드 | 완료 (`main`) | Tauri 2 셸, 테마 4종, i18n, 설정/워크스페이스, CI |
| P1 엔진·모델 | **완료 (`main`에 병합, PR #2)** | 아래 "P1 산출물" 참조. 병합 후 CI 엔진 빌드 수정은 `phase/p1-engine` 후속 PR |
| P2 워크스페이스·문서 | **완료 (`main`에 병합, PR #3)** | 아래 "P2 산출물" 참조 |
| P3 지식 베이스 | **완료 (`main`에 병합, PR #4; 리뷰 반영 3커밋은 `phase/p3-knowledge` 후속 PR 대기)** | 아래 "P3 산출물" 참조. 청킹, ko-dic FTS5 + trigram + LIKE, bge-m3 + sqlite-vec, 하이브리드 검색, 근거 기반 Q&A·설명·요약·비교 |
| P4 파이프라인 | **완료 (`phase/p4-pipeline`, PR #7 열림 — 사용자 생성; `phase/p3-knowledge` 위에 쌓음, 단계 말 리뷰 반영 완료)** | 아래 "P4 산출물" 참조. 분류·OCR·KIE 호출기, 내장 양식 7종 + 양식 등록 + NL 스키마, 검증기·신뢰도, 검토 큐, 추출 미러, 분할/병합 |
| P5 자연어·작업 | **완료 (`main`에 병합, PR #8)** | 아래 "P5 산출물" 참조. 플래너(PlanSchema·규칙·검증기·실행기), 확인 카드, 영속 Job 러너, 정리/저널/되돌리기, 집계, 내보내기 5종 |
| P6 문서 생성·프로젝트 정합성 (v1.1) | **완료 (`main`에 병합, PR #9 — 2026-11-08 10:18, P7과 한 세션에서 진행 — 사용자 결정)** | 아래 "P6·P7 산출물" 참조. §3.13 문서 생성(템플릿·양식 채우기·HWPX/DOCX/PDF/MD), §3.15 프로젝트·규칙·정합성 검사·보완요청서·관계 그래프, 플래너 op 10종 추가 |
| P7 품질·배포 (v1.1) | **완료 (`main`에 병합, PR #9; 후속으로 `phase/p6-p7`에서 CUDA 팩 cu121/cu129 빌드·Release 게시, Lite·Full 설치파일 빌드 검증 완료, v0.2.0 Release에 Lite·Full 설치파일 게시 완료)** | 평가 하네스(`doculens-eval`)·플래너 골든셋 100건·회귀 게이트, NSIS Lite/Full 릴리스 워크플로·CUDA 팩 워크플로, NOTICES/SBOM 자동화, 보존/파기 일일 작업, 완료 알림, 절전 방지 |

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

## P2 산출물

### Rust 코어 (`crates/doculens-core`)

- `db`: rusqlite(bundled SQLite, MIT) + 컴파일 타임 임베드 마이그레이션(`db/schema/0001_init.sql`,
  `PRAGMA user_version`), 로컬 디스크는 `journal_mode=WAL`, UNC·원격 드라이브(`GetDriveTypeW`)·
  OneDrive/Dropbox/Google Drive 경로는 `DELETE`. `Workspace::open`이 잠금과 함께 DB를 열고(`ws.db`),
  닫을 때 WAL 체크포인트. 테이블: `documents`(상대경로·BLAKE3·상태·역할·유형·쪽수·변환 메모·오류),
  `pages`(픽셀/포인트 크기·DPI·회전·텍스트 레이어·공백 판정), `tags`/`document_tags`, `import_batches`, `kv`.
- `documents::repo`: 삽입/조회/필터(상태·형식·폴더 접두사·이름 검색(LIKE 이스케이프)·태그·유형·정렬·페이지네이션)/
  통계(상태·형식·폴더별 건수, 총 쪽수)/페이지 교체/회전/경로 재연결/태그·유형·제목/삭제(CASCADE).
- `documents::import`: **계획**(드롭·폴더 재귀 수집 ≤5,000, 숨김·`.doculens`·심링크 제외; 확장자 지원 판정과
  미지원 사유 `heic|office_legacy|text|no_extension|unsupported_extension|empty|too_large|unreadable`;
  BLAKE3 전수 해시로 워크스페이스 중복·목록 내 중복·이미 등록·**파일 없음 문서와 해시 일치 시 다시 연결**;
  inbox 소스는 3초 안정화 전 "복사 중") → **실행**(워크스페이스 밖 파일은 `documents/` 또는 `inbox/`로
  복사·이동(`_2` 충돌 접미사, `.part` → rename), 등록 → 변환 → 렌더, 문서별 오류 격리, 취소, 배치 기록) →
  **재검사**(파일 없음/복귀/내용 변경 재렌더/중단된 `importing` 재처리) → **inbox 계획**(미등록·안정화 파일만) →
  **삭제**(OS 휴지통 `trash` 크레이트, 실패 시 `.doculens/trash/` 폴백, 캐시·행 정리).
- `documents::convert`: 공통 문서 모델(문단·런 서식·제목 레벨·목록·병합 셀 표·그림·쪽 나눔) → HTML 번들
  (`index.html` + `img/`, A4 CSS, 여백은 콘텐츠 패딩) → `PdfPrinter` 트레이트로 PDF.
  - `hwpx`: OWPML 리더(`zip` + `quick-xml` 미니 DOM, 접두사 무시) — `content.hpf` 스파인·바이너리 항목,
    `header.xml` 글자/문단 속성(굵게·기울임·밑줄·크기, 정렬, 개요 레벨→제목, 번호/글머리), 섹션 `pagePr` 용지,
    `tab/lineBreak/nbSpace`, 표 `cellAddr/cellSpan/cellSz`, 그림 `binaryItemIDRef`, 글상자·도형의 `subList` 텍스트,
    수식 스크립트. 테스트용 HWPX 작성기 포함.
  - `docx`: WordprocessingML 리더 — `styles.xml`(heading N / outlineLvl, basedOn 체인), `numbering.xml`,
    `document.xml.rels` 이미지, `gridSpan`/`vMerge` 병합 해석, `br type=page`, 하이퍼링크·sdt·텍스트 상자.
  - `hwp`: HWP 5.0은 `hwp` 크레이트(Apache-2.0)로 파싱하되 `catch_unwind`로 패닉 격리, CFB 매직·크기 검사,
    암호화 문서 거부, 배포용 문서는 ViewText, UTF-16 서로게이트 처리, 표·그림·글상자·각주.
  - 스텁 프린터(`write_blank_pdf`)로 셸 없이도 PDF 경로를 테스트.
- `documents::render`: PDFium(`pdfium-render` 0.9, `thread_safe`, 런타임 동적 로드, 프로세스당 1회 바인딩)을
  전용 워커 스레드에서 실행하는 `RenderService`; 8쪽 단위 청크로 뷰어 요청과 교차; DPI→스케일(긴 변 ≤4096),
  사용자 회전, 폼 데이터 렌더, 텍스트 레이어 추출, JPEG(q90) 페이지 + 320px 썸네일(`.part` → rename),
  공백 페이지 휴리스틱(루마<160 비율<0.03%); 이미지는 `image` 크레이트로 디코드 + **EXIF Orientation** 적용;
  `locate_pdfium`(`DOCULENS_PDFIUM` → 후보 폴더).
- `fswatch::DirWatcher`: 모델 폴더 감시기를 일반화(경로 필터 + 안정화 창), inbox 감시에 재사용.
- `documents::fixtures`: 100쪽 PDF·3구역 HWPX·2쪽 DOCX·JPG 샘플 세트 생성(`gen_fixtures` ignored 테스트).

### Tauri 셸 (`src-tauri`)

- `protocol.rs`: `doculens://` 스킴(Windows `http://doculens.localhost`) — `/pages|thumbs/<doc>/pNNNN.jpg`는
  id 검증 후 코어가 만든 캐시 경로만 서빙, `/print/<token>/…`은 등록된 번들 폴더 안에서만(canonicalize) 서빙.
- `printer.rs`: 숨은 `WebviewWindow`(미표시·작업 표시줄 제외)가 번들을 로드하면 `on_page_load(Finished)`에서
  **Windows WebView2 `ICoreWebView2_7::PrintToPdf`**(A4, 여백 0, 배경 인쇄; `ICoreWebView2Environment6::CreatePrintSettings`)
  또는 **WebKitGTK `PrintOperation`**(Print to File, A4)으로 인쇄 → 완료 신호 → 창 파괴; 직렬화·120초 타임아웃.
- `docs_bridge.rs`: PDFium 탐색(리소스 `pdfium/` → 실행 파일 옆 → `<AppData>/pdfium/`)과 1회 바인딩,
  `DocsContext`(Arc<Db> + 경로 + 렌더 서비스 + 프린터 + 렌더 옵션), inbox `DirWatcher`(3초) → `inbox_job`
  (복사 중이면 4초 후 재시도), 워크스페이스 전환 시 재검사 작업 + inbox 스캔, 작업 종류
  `document_import`/`document_rescan`/`document_render`(진행률 = 문서 + 쪽 비율, 취소), `documents://changed`
  (`imported|updated|removed|rescanned|batch` + id 목록).
- 명령 18종(`documents_list/stats/get/pages/page_text/import_plan/import_run/rescan/inbox_scan/rerender/
  rotate_page/delete/set_tags/set_doc_type/set_title/tags/set_inbox_watch/pdfium_info`). 미리보기 계획은 토큰으로
  보관했다가 실행 시 소비(해시 재계산 없음); `DocumentView`/`PageView`는 레코드 + 절대경로 + 이미지 URL(`?v=` 캐시 무효화).
- `tauri.conf.json`: CSP `img-src`에 `doculens:` / `http://doculens.localhost`, 번들 리소스 `resources/pdfium/`.

### 프런트엔드

- 라이브러리(`features/library/LibraryPage.tsx`): 폴더 트리(통계의 `rel_dir`), 상태·형식·정렬 필터 + 디바운스
  검색, **가상 스크롤** 그리드/리스트(`@tanstack/react-virtual`), 카드 체크박스 다중 선택 → 다시 렌더·삭제(휴지통),
  모두 선택, inbox 감시 토글(백엔드 감시기 재시작), 다시 검사, PDFium 없음 경고, 빈 상태(가져오기 / 폴더의 기존
  파일 가져오기), 드롭 존(`useDocumentDrop`: Tauri `onDragDropEvent` 경로, 브라우저는 파일명).
- 가져오기 미리보기(`ImportDocumentsDialog.tsx`): 경로 입력/파일 선택 → 항목별 배지(신규·중복(대상)·목록 내
  중복·이미 등록·다시 연결·미지원 사유·복사 중) + 요약·용량, 저장 위치(documents/inbox)·이동·렌더 옵션 → 작업 시작.
- 뷰어(`DocumentViewer.tsx`, 경로 `/library/:id`): 제목 편집·형식/상태/변환본 배지·유형 드롭다운(내장 8종)·태그
  편집기, 툴바(쪽 이동·확대/축소/맞춤·회전 저장(좌/우)·다시 렌더·원본 폴더·삭제), 썸네일 스트립, 페이지 이미지,
  우측 탭 [텍스트(텍스트 레이어)] + 필드/표/Q&A/이력 자리표시(P3·P4), 정보 패널(경로·크기·쪽수·DPI·텍스트 레이어·
  변환 메모·오류), 키보드 ←/→/+/−/0.
- 홈: 최근 문서 6건 + 통계 타일(신규/렌더/추출/검토) 실데이터. 작업 화면: 문서 작업 3종 취소 가능.
- 모의 백엔드: 문서 저장소·가져오기 계획/작업(파일명 `-12p.pdf` → 12쪽)·SVG 페이지/썸네일 데이터 URL·회전·
  태그·삭제·통계를 시뮬레이션. i18n: `library.*`, `documents.*`, `import_docs.*`, `viewer.*`, 작업 종류, 오류 코드
  `db/not_found/unsupported/convert/render`.

### 테스트·검증

| 종류 | 수 | 비고 |
|---|---|---|
| `cargo test -p doculens-core` (lib) | 91 (+1 ignored) | db 4, documents 25(변환 10, 렌더 4, 가져오기 4, 저장소 2, 픽스처 1 + `gen_fixtures` ignored …), fswatch 1; PDFium 테스트는 `DOCULENS_PDFIUM` 설정 시 실행(없으면 건너뜀) |
| `tests/engine_process.rs` / `tests/real_engine.rs` | 5 / 1 | P1과 동일 |
| vitest | 27 | i18n 패리티 + 정적 키 해석(새 화면 키 포함) |
| Playwright | 14 | P0 7 + P1 3 + **P2 4**(가져오기 미리보기→렌더→라이브러리·필터·홈 / 뷰어 쪽 이동·키보드·회전·태그·유형·삭제 / 일괄 작업·선택 정리·inbox 토글 / PDFium 없음 배너·폴더 가져오기 미리보기) |

- **실제 변환·렌더 검증(이 환경)**: PDFium chromium/8125 Linux 빌드로 100쪽 PDF 렌더(텍스트 레이어·공백 판정·
  회전·취소), 생성 HWPX/DOCX 파싱, 스텁 프린터 경로의 전 파이프라인(가져오기→변환→렌더→DB) 통과.
- **실제 데스크톱 바이너리 헤드리스 확인(Linux, Xvfb)**: 샘플 4종(100쪽 PDF, 3구역 HWPX, 2쪽 DOCX, JPG)을
  `inbox/`에 둔 워크스페이스로 앱 실행 → DB 마이그레이션 → PDFium 로드 → 시작 시 inbox 스캔이 가져오기 작업을 만들고
  **WebKitGTK 인쇄로 HWPX·DOCX 변환본 PDF 생성** → 106쪽 렌더(텍스트 레이어 105쪽) → 상태 `rendered`, 배치 4/4,
  오류 0. HWPX 변환본 1쪽을 눈으로 확인: 제목(가운데·굵게), 서식 런, 글머리표, 병합 헤더 표, 그림 정상.
- CI: Rust 작업과 Windows 작업이 `scripts/fetch-pdfium.mjs`(SHA-256 핀)로 PDFium을 받아 캐시하고 렌더 테스트까지
  실행한다. Windows WebView2 인쇄 경로는 컴파일만 검증(실행은 사용자 PC 체크리스트).

### 설계 대비 범위 메모

- 가져오기 미리보기의 "예상 소요 시간"과 "자동 처리 수준 선택"은 P4(분류·OCR 파이프라인)에서 붙인다(지금은
  등록+렌더만). 양식 등록·분할/병합·집계 바도 P3·P4.
- 페이지 캐시는 설계의 WebP 대신 JPEG(q90)로 저장한다(순수 Rust 인코더, 손실 WebP는 libwebp C 의존). 교체는
  `render::write_jpeg` 한 곳. LRU 상한(설정 `cache.page_cache_max_gb`)은 보존/파기 작업(P5/P7)에서 적용.
- 멀티 페이지 TIFF와 CCITT G4 TIFF는 첫 페이지/미지원(`image` 크레이트 한계) — PDF로 변환 후 투입 안내.
- HWPX/DOCX 변환은 근사 렌더("변환본" 배지): 글상자·도형은 텍스트만, 수식은 스크립트 텍스트, 머리글/꼬리글 생략.

## P2 단계 말 리뷰 반영 (2026-11-04)

독립 리뷰 2건(Rust 코어·셸: 치명 2·높음 5·중간 9·낮음 16 / 프런트엔드: 치명 1·높음 2·중간 8·낮음 15)을 반영했다. 핵심:

- **릴리스 프로파일 `panic = "unwind"`**: 작업 스레드·HWP 파서(`hwp` 크레이트는 손상 파일에서 panic)·PDFium
  워커가 `catch_unwind`에 의존하므로 `abort`면 앱이 통째로 죽고, 행이 `importing`으로 남아 시작 시 재검사가
  같은 파일을 다시 처리하는 크래시 루프가 된다. 추가로 `process_document`가 **처리 전에 상태를 `error`("interrupted
  while processing")로 기록**해 두고 끝나면 덮어쓰므로, 프로세스가 중간에 죽어도(OOM·abort) 다시 반복되지 않는다.
  PDFium 워커는 작업별 `catch_unwind`로 살아남고, 바인딩 실패는 캐시하지 않아 라이브러리를 나중에 넣으면 복구된다.
- **Windows 빌드**: `DRIVE_REMOTE`는 `Win32::System::WindowsProgramming`에 있어 상수값(4)을 직접 사용,
  `PrintToPdfCompletedHandler` 클로저는 `bool`을 받음, 드라이브 접두사가 없는 경로는 원격 판정에서 제외(`\`가 UNC로
  읽혀 DELETE 저널이 되던 문제). 로컬에서 `x86_64-pc-windows-gnu` 타깃으로 `cargo check` + `clippy`를 교차 검증하고
  CI Windows 작업이 통과.
- **경로 철자**: `rel_path`는 디스크의 철자를 그대로 저장(NTFS/ext4는 NFC≠NFD). 앱이 복사해 넣는 파일 이름과
  `original_name`만 NFC. NFD 파일을 제자리/복사로 가져오는 테스트 추가.
- **워크스페이스 전환**: 전환·닫기 전에 문서 작업(가져오기·재검사·렌더)을 취소하고 최대 8초 대기
  (`before_workspace_change`), `DocsContext`가 워크스페이스 세대를 들고 있어 늦게 끝나는 작업은 이벤트를 내지 않음.
- **inbox**: 가져오기 작업 중 들어온 파일은 `inbox_dirty` 플래그로 작업 종료 후 재검사; 해시 계산은 감시 스레드가
  아니라 별도 스레드(`spawn_inbox_scan`)에서; inbox 대상 가져오기와 감시기의 경합은 "가져오기 작업 종류 단위"
  활성 검사로 회피. 재검사는 다른 문서 작업이 돌 때 거부(`null` → "이미 진행 중" 안내).
- **재검사 내성**: 파일 하나가 잠겨 있어도 문서별로 실패를 기록하고 계속 진행; 재처리 시 문서의 저장 DPI 사용.
- **입력 파일 방어**: 표 행/열·`gridSpan`·`cellAddr`를 10,000행·1,024열로 clamp(속성값 기반 할당 OOM 방지),
  XML 파트 64 MiB·이미지 64 MiB/문서 합계 256 MiB, HTML 번들 생성 시 이미지 바이트를 복제하지 않음,
  `percent_decode` 바이트 슬라이스, 캐시 폴더 id 검증, 최상위 심링크도 건너뜀.
- **DB**: 페이지 교체·태그 교체·다시 연결은 트랜잭션; 같은 경로가 이미 등록된 경우(파일 없음 행 등)는 UNIQUE 오류
  대신 그 행을 갱신; 복사 후 등록에 실패하면 복사본 제거; poisoned 뮤텍스 복구.
- **인쇄 경로**: `on_page_load` 안에서 재진입 호출하지 않고 `run_on_main_thread`로 다음 턴에 인쇄, GTK 출력 URI는
  `Url::from_file_path`(공백·한글 경로).
- **로그**: 파일명·경로는 debug 레벨로만, warn은 건수(§3.8).
- **프런트엔드**: 선택 집합은 보이는 문서로만 유지(필터·폴더·검색·삭제 시 자동 정리, 부분 선택 indeterminate);
  뷰어 단축키는 뷰어 루트 스코프 + Radix 탭/셀렉트/버튼 제외; 검색·필터 변경 시 이전 목록 유지(`keepPreviousData`);
  드롭 중 재드롭도 즉시 미리보기(요청 순번으로 지연 응답 폐기); "기존 파일 가져오기"는 미리보기 다이얼로그 경유;
  썸네일 스트립 가상화·페이지 전환 시 스크롤 리셋·문서별 상태 초기화; 명령 반환값을 캐시에 직접 반영(이중 refetch
  제거); `window.confirm` 대신 테마 적용 ConfirmDialog; 작업 제목/요약·`invalid` 사유 한국어화; 리스트 grid 시맨틱·
  라벨·포커스 복귀; 모의 백엔드는 동일 경로 중복 제거·재검사 `null`·PDFium 없음 스위치; 로컬 e2e는 빌드 후 실행.
- 보류(기록): 삭제 되돌리기(저널, P5), 다시 렌더 DPI 선택 UI, 태그 필터, 일괄 태그, 온보딩 마법사 내 "지금
  가져오기"(라이브러리 빈 상태로 이동), 가져오기 "검토 완료 전 이동 금지"(정리 기능 P5), 변환본 텍스트의 쪽 배분
  (인쇄 PDF에 텍스트 레이어가 없을 때 1쪽에 몰림), 페이지 이미지 프로토콜의 비동기 응답(USB/NAS).

## P3 산출물

### Rust 코어 (`crates/doculens-core`)

- `db/schema/0002_knowledge.sql` + `db/mod.rs`: `documents` 색인 컬럼(indexed_at·index_version·index_error·text_hash·chunk_count),
  `collections`·`collection_docs`·`doc_versions`·`chunks`(rowid 공유)·`chunks_fts`(unicode61, 사전 분석 토큰 열)·
  `chunks_fts_tri`(trigram, 원문)·트리거 동기화·`glossary`·`answers`. sqlite-vec(`vec0`, cosine)는 `register_extensions()`로
  자동 확장 등록(정적 링크), `chunks_vec`는 임베딩 차원을 알게 될 때 지연 생성·모델 교체 시 재생성.
- `knowledge/tokenize.rs`: `Analyzer` — lindera 6.2 + ko-dic(런타임 로드, `DOCULENS_KODIC` → 리소스 `ko-dic/` →
  `<AppData>/tokenizers/ko-dic/`) 또는 간이 분석기(어절+2음절 바이그램) 폴백. 조사·어미 제거, 활용형 어간 복원(expression),
  사전 복합어 + 인접 명사 결합형(`사업자등록번호`) + 복합어·미등록어 바이그램(`정산서`→`정산`), 코드 토큰(`1,000,000`→`1000000`,
  `제 12 조`→`제12조`), 4,000자 창 분할. 질의 측은 같은 분석기(1음절 의존명사·단위명사 제외).
- `knowledge/chunker.rs`: 구조 인지 청킹 — 장/절/조(괄호 제목+본문 분리)·항(①/(1))·호(1)/1.)·목(가.)·점 번호·마크다운·대괄호
  제목 인식, 제목 스택 → `heading_path`("제2장 출장비 > 제3조 출장비의 구성")·`clause`("제3조 제2항"), 표 행 그룹(머리행 반복),
  긴 문단 문장 단위 분할, 목표 500/최대 800/최소 60 토큰(설정 `chunk_target_tokens`), 쪽 범위 기록, 토큰 추정기.
- `knowledge/repo.rs`: 컬렉션·버전·용어집·청크(트랜잭션 교체, 벡터 정리)·검색 프리미티브(FTS MATCH bm25, LIKE 폴백,
  범위→문서 집합(컬렉션·문서·역할·폴더·최신본·비활성 컬렉션 제외))·답변 저장/피드백·상태 집계·색인 후보 조회.
- `knowledge/index.rs`: 페이지 텍스트 해시로 증분 재청킹, 텍스트 없는 문서 처리, 문서별 실패 격리, 상태 대시보드.
- `knowledge/embed.rs`: `Embedder` 트레이트(`RouterEmbedder` = llama-server `/v1/embeddings`, `HashEmbedder` = 테스트·모의),
  배치 임베딩(HTTP는 트랜잭션 밖, L2 정규화, 커서로 재시도 무한루프 방지), 모델 변경 감지 → 전체 재생성, 범위 필터 KNN(부족 시 확장).
- `knowledge/search.rs`: 하이브리드 — 형태소 AND → 부족하면 OR(용어 절반 이상 포함 필터) + trigram OR + (둘 다 0건이면) LIKE
  폴백 + 벡터 KNN → RRF(k=60, 채널 가중) → 문서당 상한 → 스니펫·하이라이트(문자 오프셋). 용어집 동의어로 질의 확장.
- `knowledge/answer.rs`: `ChatBackend`(EngineManager 구현, `ScriptedChat` 테스트용), 근거 검색(후속 질문 keep_chunks, 토큰 예산),
  번호 근거 프롬프트(한/영, thinking off, T=0.1), 스트리밍, 문장 분할 + `[n]` 인용 파싱, "상충:" 단락 추출, 근거 없음 처리
  (`allow_general` 전까지 모델 호출 없음), 요약(map-reduce)·비교(표) 저장.
- `documents/import.rs`: `ImportOptions.collection_id` — 배치의 신규·다시 연결·이미 등록·중복 문서를 컬렉션에 붙이고 역할 지정.
- `settings.rs`: `KnowledgeSettings`(auto_index, auto_embed, embedding_model, latest_only, allow_general_knowledge,
  context_tokens, top_k, chunk_target_tokens).
- `crates/doculens-kodic`: mecab-ko-dic 원본 → lindera 바이너리 사전 빌더(`build`/`check`), 사전 디스크립터 벤더링.
- 테스트: 토크나이저(간이·ko-dic), 청킹(규정·표·긴 문단·제목 규칙), 저장소 CRUD, 증분 색인, 합성 골든셋 15문항
  (재현율@10·top-1 조항), 조사/2자/숫자 질의, 용어집 확장, 범위·버전·컬렉션, 임베딩·KNN·모델 교체·삭제 정리, 답변·설명·후속·
  근거 없음·요약·비교.

### Tauri 셸 (`src-tauri`)

- `knowledge_bridge.rs`: 분석기 `OnceLock`(리소스/AppData 후보), `KnowledgeContext`(세대 검사), 임베딩 모델 상태
  (레지스트리 `default_embedding`/설정 → 설치·로드 여부), `index_job`(진행 중이면 dirty 표시 후 재실행, 끝나면 auto_embed),
  `embed_job`(사용자 실행 시 엔진 기동·모델 로드, 자동 실행은 로드된 모델만), 가져오기·재검사·재렌더 후 `auto_index`,
  답변 스트리밍(`knowledge://answer {request_id, delta, done}`), `knowledge://changed {kind, ids}`.
- `commands/knowledge.rs`: 24개 명령(status/collections CRUD·문서 추가·역할/index/embed/reset/search/chunks/answer/answers·
  feedback·delete/glossary/versions). `docs_bridge`: 워크스페이스 전환 시 지식 작업도 취소·대기, 작업 종류
  `knowledge_index`·`knowledge_embed`.
- `tauri.conf.json` 리소스 `ko-dic/`, `.gitignore`, CI(ko-dic 빌드 캐시 + `DOCULENS_KODIC`, Linux·Windows).

### 프런트엔드

- 지식 베이스 화면: 개요(색인 통계, 형태소 분석기/임베딩 카드, 색인 갱신·전체 재색인·임베딩 실행·초기화), 적재 마법사
  (파일·폴더 → 컬렉션·옵션 → 미리보기 → 실행; `role=knowledge`, `collection_id`), 컬렉션(생성·편집·활성·삭제·문서 목록·제거),
  버전 그룹(최신본 지정·해제), 용어집(용어·동의어·정의), 질문 테스트(검색 근거·토큰·채널), 답변 기록(피드백·삭제).
- 검색 화면: 방식(하이브리드/키워드/의미)·범위(전체/지식 베이스/컬렉션)·최신본만, 결과(제목·쪽·조항·제목 경로·채널 배지·
  하이라이트 스니펫), 키워드 전용/LIKE 폴백 안내, 열기(뷰어 `?page&q&chunk`)·이 결과로 질문.
- 어시스턴트: 프리라우터 미매칭 입력은 지식 베이스 Q&A(범위 선택, 후속 질문은 직전 근거 유지), `AnswerCard`(문장별 출처 칩 →
  뷰어 쪽 이동, 근거 없음 배너 + "일반 지식으로 답하기", 상충 단락, 피드백·복사), 스트리밍 표시.
- 뷰어: 지식 배지(컬렉션), "지식 베이스에 추가"·"버전"·"설명(Ctrl+E)" 버튼, 검색어 하이라이트(`?q=`; 2026-11-09부터 이미지 위 글자 레이어의 `<mark>`), Q&A 탭(→ "AI 검색")
  (이 문서/전체 범위 질문, 설명, 요약 모드 3종, 문서별 답변 기록). 라이브러리: 일괄 "지식 베이스에 추가", 지식 역할 배지.
- 설정 > 지식 베이스 탭. 작업 화면: `knowledge_index`/`knowledge_embed` 종류·요약 번역. 오류 사유 번역 8종.
- 모의 백엔드 `mock-knowledge.ts`(문단 청킹, 부분 문자열 검색, 스트리밍 정답 생성, 색인/임베딩 작업 시뮬레이션,
  `doculens.mock.embedding=1`로 임베딩 모델 설치 상태). Playwright `knowledge.spec.ts` 3건, P0 어시스턴트 테스트는 Q&A 흐름으로 갱신.

### 테스트·검증

| 항목 | 결과 |
|---|---|
| `cargo test --workspace` (ko-dic + PDFium) | 코어 lib 124 통과(+1 ignored; 지식 모듈 33), 엔진 프로세스·real_engine 통과 |
| 합성 골든셋 15문항 | ko-dic: 재현율@10 15/15, top-1 조항 ≥ 75% · 간이 분석기: 14/15, 14/15 |
| `cargo clippy --workspace --all-targets -D warnings`, `cargo fmt --check` | 통과 |
| vitest / Playwright | 27 / 17 통과 (리뷰 반영 후 재실행) |
| `npm run licenses:check`, `cargo deny check licenses bans sources` | 통과 (lindera MIT, mecab-ko-dic Apache-2.0, sqlite-vec MIT/Apache) |
| CI (c51ca53) | Rust·Web·License·실엔진·Windows(ko-dic 빌드 포함) 전부 성공 |
| ko-dic 빌드 | `scripts/fetch-kodic.mjs`: 다운로드(sha256 핀) → 빌드 5초 → 82 MB, 재실행 시 건너뜀 |
| 헤드리스 실행(Linux) | 아래 "헤드리스 검증" 참조 |

### 헤드리스 검증 (실제 데스크톱 바이너리, Xvfb + WebKitGTK)

`cargo build -p doculens` 결과를 `DOCULENS_PDFIUM`·`DOCULENS_KODIC`과 함께 실행, inbox에 P2 샘플 4건(100쪽 PDF·HWPX·DOCX·JPG)
→ 시작 시 inbox 가져오기·렌더 → `auto_index`가 청킹·색인. 결과(`workspace.db`): `user_version=2`, 문서 4건 모두 `indexed_at`
기록·오류 없음(JPG는 텍스트 없음 처리), 청크 4개(910 토큰; 100쪽 PDF는 2청크 p.1-62/63-100), 형태소 색인 MATCH
`견적`·`계약`·`홍길동`·`1250000` 각 1건, trigram MATCH `노트북`·`1,250,000`·`계약서` 각 1건, 로그에 ko-dic 적재 확인·경고 없음.
스크립트: 스크래치패드 `smoke-p3/run-p3.sh`(P2 스모크 변형).

### 설계 대비 범위 메모

- 형태소 토크나이저는 FTS5 사용자 토크나이저(C API) 대신 **사전 분석 토큰 열** 방식(`chunks.tokens` + unicode61)으로 구현 —
  동일 분석기를 색인·질의에 쓰므로 기능은 같고 C FFI가 없다. 하이라이트는 원문에서 표면형을 찾아 계산.
- 사전은 저장소에 벤더링하지 않고(82 MB) PDFium처럼 스크립트로 빌드·설치한다(설계의 `<AppData>/tokenizers/`).
  lindera의 `embed-ko-dic`은 빌드 시 lindera.dev에서 내려받는데 이 환경에서 차단되어 bitbucket 원본(sha256 핀)을 쓴다.
- 리랭커는 미도입(설계 §3.14.2 "RRF 상위" 경로). 컬렉션 폴더 감시(`watch_folder` 열만 예약)·질의 확장의 형태소 동의어
  자동 생성은 미구현. 답변 피드백 통계는 개요·답변 기록에 표시(품질 루프의 골든셋 평가는 사용자 PC).
- 어시스턴트의 작업 명령(정리·추출 등)은 아직 Q&A로 흐른다 — P5 플래너가 op로 분기할 때까지.
- P2에서 넘긴 숙제 중 미해결: 페이지 캐시 LRU, 가져오기 예상 소요 시간, TIFF 다중 페이지, 해시 고정(사용자 PC).

## P3 단계 말 리뷰 반영 (2026-11-04)

리뷰 서브에이전트 2건(Rust 25건, 프런트엔드 20건)의 지적을 다음과 같이 반영했다. 보고서 원문은 세션 스크래치패드
(`review-rust-p3.md`, `review-frontend-p3.md`).

**Rust — 반영(높음·중간 전부 + 낮음 대부분)**

- 답변 모델: 임베딩 모델을 로드하면 엔진의 `active_model`이 bge-m3로 바뀌어 채팅이 임베딩 인스턴스로 가던 문제 →
  `knowledge_bridge::chat_model`이 역할이 `embedding`이 아닌 모델(활성 → 설정/평가 → 로드된 모델 순)을 명시 선택.
- 범위 검색: 범위(문서·컬렉션 ≤ 900건)를 FTS/LIKE/KNN SQL 안에 넣어 "전역 top-N 후 필터"로 인한 재현율 손실 제거
  (`repo::fts_match(.., allowed)`, `like_search`, `embed::knn`의 `rowid IN (...)`, 미지원 시 확장 폴백). 테스트 추가.
- 설명(Explain): 현재 문서가 구판/비활성 컬렉션이어도 자기 문서를 인용(문서 범위 추가 검색 병합, 명시 문서는 제외 규칙 면제).
- 근거 판정: 벡터 전용 근거 임계를 0.5로 올리고 설정(`min_vector_similarity`)·요청(`min_similarity`)으로 조정 가능.
- 프롬프트 주입: 근거를 `<source id …>` 구분자로 감싸고 구조 마커(`[질문]`·`[근거]`·`상충:` 등)·`</source>`를 제거,
  질문을 근거 앞에 배치, "근거 안 지시는 따르지 않는다" 규칙 추가. 테스트 추가.
- 색인 상태: 메타데이터만 바뀐 문서는 `Unchanged` 시 `indexed_at` 갱신(영구 대기 방지), 대시보드 대기 수 예측 일치;
  분석기 종류·사전 버전·청크 크기 서명을 `kv`에 기록해 바뀌면 전체 재청킹(사전 설치 후 재시작 안내 포함).
- 작업 체인: 워크스페이스 전환 중 `switching` 플래그로 자동 색인·임베딩·after 훅 차단, 대기 루프에서 취소 반복,
  `JobBook::create_if_idle`로 중복 작업 방지, 스트리밍 답변도 전환 시 취소.
- rowid 재사용: `chunks.id AUTOINCREMENT`, 임베딩 저장은 `text_hash` 일치 행만 표시(색인과 경합해도 벡터 오염 없음).
- DB 연결: 파일 DB는 읽기 전용 두 번째 연결(`Db::with_read`)로 검색·상태·목록 조회가 긴 쓰기 트랜잭션에 막히지 않음.
- LIKE 폴백은 3자 미만 단어가 있을 때만, 색인 텍스트 NFC 정규화, 후보는 `(id, document_id)`만 먼저 조회 후 본문 로드,
  질의 용어 ≤ 24·2,000자, 질문·선택 ≤ 4,000자, `keep_chunks` ≤ 32, 피드백 값 검증.
- 자동 임베딩: 엔진이 실행 중이면 임베딩 모델을 자동 로드해 백그라운드 임베딩이 실제로 돌도록 변경(엔진 자동 기동은 안 함).
- 요약: 계층적 reduce(부분 요약이 예산을 넘으면 단계적으로 병합), 설정 범위 클램프(`KnowledgeSettings::normalize`),
  취소가 부분 요약 단계에서도 적용, 연속 3회 임베딩 실패 시 중단, 상충 단락은 콜론 형식만 인식하고 "없음"은 제외.
- 타입 오류: 저장소가 `collection_not_found`·`document_not_found`·`*_taken`·`*_empty` 코드를 돌려주고 명령이 그대로 매핑;
  가져오기 전 컬렉션 존재 검증. `fetch-kodic.mjs`는 임시 폴더에 빌드 후 교체, CI 캐시 키에 빌더·매니페스트 해시 포함.

**Rust — 보류(HANDOFF 숙제)**: 벡터 양자화·FTS optimize 등 대용량(20만 청크) 성능 작업, 컬렉션 폴더 감시(`watch_folder`),
리랭커, 실모델 임계 검증 테스트(사용자 PC), `knowledge_collection_docs` 외 사소한 N+1.

**프런트엔드 — 반영**

- 설정 숫자 입력을 로컬 초안(blur/Enter 확정)으로 바꿔 입력 가능하게 함; 후속 질문은 명시 토글("후속 질문")일 때만 이전 근거
  유지(그 외엔 매번 새로 검색 → 근거 없음 배너 정상 동작), 모의 백엔드도 Rust처럼 유지 청크 + 새 검색을 병합.
- 답변 동시성: `useAsk`가 최신 요청만 반영, 전송 버튼은 진행 중 비활성, 중지 버튼(`knowledge_answer_cancel`) 추가.
- 인용 칩 → 뷰어 `?chunk=`를 읽어 해당 청크 첫 줄을 텍스트 탭에 하이라이트, 하이라이트 시 텍스트 탭 강제, 같은 쪽 재이동
  반영; Ctrl+E는 설정의 `explain_selection` 단축키 사용(`matchesChord`/`formatChord`), 선택은 `selectionchange`로 추적.
- 작업 명령(날짜 범위·정리/추출/합계 등)은 Q&A 대신 "P5 플래너" 대기 카드로 안내, 질문형 입력만 지식 베이스 Q&A.
- 뷰어 Q&A 탭 `forceMount`(탭 전환 시 스트리밍 유지), 질문 테스트 결과는 스토어에 보관, 컬렉션 추가 다이얼로그는 열릴 때만
  초기화, 버전 다이얼로그는 사용자 입력을 덮어쓰지 않음·연도로 시작하는 제목 처리, 마법사 상태 초기화·재시도 안전.
- 간이 분석기 안내에 사전 폴더 경로 + "폴더 열기" + 재시작 안내; 용어 중복은 `glossary_term_taken`; 5개 다이얼로그 닫기 라벨,
  칩·피드백 aria, 영어 복수형, 문단 구조 유지 렌더링, 검색 최신본 스위치 설정 연동, 용어집 변경 시 검색 캐시 무효화,
  임베딩 모델 표시가 설정을 따르도록 모의 수정, E2E를 i18n 번들 기반으로 안정화.

**프런트엔드 — 보류**: 어시스턴트 범위 "현재 폴더/선택 문서"(P5), 검색 결과 문서 단위 그룹 보기, 답변 내보내기(P5).

## P4 산출물

### Rust 코어 (`crates/doculens-core`)

- `db/schema/0003_pipeline.sql`: `documents` 파이프라인 열(doc_type_source·classified_at·ocr_at·extracted_at·last_run_id·schema_id·
  review_status·confidence·pipeline_error), `pages` 열(text_source layer|converted|ocr·ocr_at·ocr_profile·ocr_model·classified_at·
  page_doc_type·page_language·page_rotation), `schemas`(+`name_en`, 출처 builtin|nl|user|form, 유형 슬러그, 기본 여부, 사용 횟수, 현재 버전 메모 `note`)·
  `schema_versions`·`extraction_runs`(모델·프롬프트 버전·프로파일·정밀 모드·토큰·시간·신뢰도·검토 수)·`fields`(경로·행·종류·값·정규화값·
  원본값·쪽·bbox·근거·신뢰도·모델 신뢰도·상태·검토 사유·검증 JSON)·`field_history`. `documents::repo::replace_pages`는 재렌더 시
  OCR 텍스트·분류 열을 보존한다(자기 텍스트 레이어가 생기면 OCR 사본은 버림).
- `pipeline/schema.rs`: `FieldDef`(ASCII 키 + 한/영 라벨, 종류 string·amount·integer·date·boolean·enum·brn·rrn·account·phone·
  email·table·group, 필수·정규식·예시·선택지·마스킹) → llama.cpp 문법용 JSON Schema(스칼라 = `{value, evidence, confidence 0~100}`,
  표 = 행 객체 배열, 그룹 = 인라인 객체, `$ref` 없음, nullable, `additionalProperties:false`) + 린트(키 형식·중복·예약어·
  정규식 앵커·enum·중첩 금지·열 수) + 프롬프트용 필드 요약 + 분류 스키마(동적 유형 enum) + 메타스키마(NL/양식 도출). 내장
  샘플 양식 7종(세금계산서·계산서(면세)·영수증·사업자등록증·계약서·신청서·일반) — §3.4.1 필드와 교차 규칙 id.
- `pipeline/validate.rs`: 금액(쉼표·원·₩·괄호 음수·만/억)·정수·날짜(YYYY.MM.DD, 년월일, 2자리 연도, 8자리, 시각 포함)·전화·
  사업자등록번호(10자리 + 체크섬 Σdᵢ·[1,3,7,1,3,7,1,3,5]+⌊d₉·5/10⌋)·이메일·불리언·enum·정규식 힌트 검증과 정규화; 주민/외국인
  등록번호 마스킹(`YYMMDD-G******`, OCR 텍스트·근거·값 전부)·계좌/식별번호 끝 4자리·생년월일 연도만; 교차 규칙(공급가액+세액=합계,
  Σ품목=공급가액/합계, 세액≈10%, 영세율 세액 0, 영수증 구성액=합계, 기간 순서, 면세 합계); 근거 문자열 공백 무시 탐색; 신뢰도 식
  (모델 자기보고 기반, 근거/값 발견 +, 검증 실패 ≤0.4, 2차 샘플 불일치 ≤0.5).
- `pipeline/prompts.rs`: 단계별 프리셋(분류·KIE·스키마 T=0, OCR 0.1, top_p 0.9, top_k 20; 이미지 예산 분류 1,024·OCR 1,200~3,072·
  KIE 1,024~2,048 토큰), `enable_thinking:false`, 자체 작성 프롬프트(분류 동적 enum, OCR → Markdown+HTML 표+`<stamp>`/`<signature/>`/
  ☐☑, KIE 2단계 복사 원칙·근거 인용, 양식 필드 도출, NL 스키마 생성/수정), n-gram 루프 감지.
- `pipeline/vlm.rs`: `Vlm` 호출기 — `ChatBackend::complete_cancellable`(엔진·대본 공용, 스트림 토큰마다 취소 검사), json_schema
  응답, 재시도 ≤3회(`finish_reason=length` → 예산 ×1.5, OCR은 ×2 한 번; 루프 → 온도 +0.2·seed 변경; `reasoning_content`만 오면
  seed 변경; HTTP 4xx는 즉시 실패; 서버 출력 파서 500은 seed·온도를 바꿔 재시도; 컨텍스트를 채워 잘린 답은 예산을 늘리지
  않음), 정밀 모드용 온도·seed 재정의(`with_sampling`), 로드된 인스턴스의 컨텍스트 크기
  (`with_context`)로 본문 앵커 예산 산정(`prompts::text_budget`) + 서버가 컨텍스트 초과(400)를 돌려주면 앵커를 ½→¼→제거하며
  재시도, 코드 펜스 허용 JSON 파서. `pipeline/image.rs`: 캐시 페이지 JPEG를 토큰 예산(32 px/토큰)에 맞춰 축소 → data URL.
- `pipeline/classify.rs`(페이지 분류 파싱, 문서 투표 — 첫 쪽 우선, `other` 다수여도 유형 쪽이 이김) · `pipeline/ocr.rs`(펜스/`null` 정리,
  표 셀 수, HTML·마크다운 표 파서, CSV) · `pipeline/extract.rs`(쪽별 KIE, 쪽 병합 — 신뢰도 높은 값·표 행 연결, 필드 행 생성 —
  검증·마스킹(모델 원문도 `mask_output`으로 가린 뒤 저장)·근거/값 탐색·정밀 모드 일치(2차 샘플 T=0.3)·검토 사유
  low_confidence|validation_failed|required_missing|audit(문서 단위 결정적 해시 샘플링, 문서당 1필드)·문서 메타(거래처·날짜·합계)).
- `pipeline/run.rs`: 실행기 — 문서별 격리, 분류(앞 8쪽+마지막, 회전≠0이면 `rotate_page` 재렌더 후 재분류, 수동 유형 보존) → OCR
  (이미지 있는 쪽, 텍스트 레이어는 `ocr_digital_pdf`일 때만, 밀집 쪽은 잔글씨 재패스, PII 마스킹, `pages.text`에 기록 → P3 색인이
  `updated_at`으로 재청킹) → 추출(스키마: 명시 → 문서 재정의 → 유형 매핑 → 일반; 앞 N−1쪽+마지막; 정밀 모드 2차 샘플) → 필드
  저장·실행 기록·문서 상태(`extracted`/`reviewed`, `review_status`) → `extractions/<doc>/<run>/{result.json, pNNNN.md}` + `LATEST` 미러.
- `pipeline/review.rs`: 승인/수정(스키마 `FieldDef`로 재검증·마스킹, 교정값의 검증 이슈 보존)/보류/거부/되돌림, 이력 기록, 문서
  재계산(대기 없음 + 필수 필드 값 있음 → `reviewed`), 문서 일괄 승인, 이력 복원, 재검증(교차 규칙 포함, 검증 결과가 같으면 신뢰도
  유지 — 멱등). `pipeline/forms.rs`: HWPX/DOCX 표 구조(라벨 셀+빈 셀, 머리행+빈 행 → 반복 표,
  "라벨: ____")·텍스트 휴리스틱·VLM 메타스키마 도출, 양식 등록(문서 role=form·유형=슬러그), NL 생성/수정(수정은 유형·규칙 유지),
  한국어 양식명 → 슬러그. `pipeline/split.rs`: lopdf로 PDF 분할(`<stem>_partN.pdf`, 원본 `archived`)·병합(이미지는 JPEG→PDF 래핑),
  새 문서 등록·렌더.
- `settings.rs`: `AutoProcessingLevel::None` 추가, `ProcessingSettings`에 inbox 자동 처리 수준(기본 full; 드롭/폴더는 classify_only),
  `ocr_digital_pdf`·`kie_max_pages`·`precise_mode`·`dense_reocr`·`dense_cells`. `ImportOptions.auto_level`(가져오기 다이얼로그 선택).
- `engine/client.rs`: `ChatResponse.reasoning`(`message.reasoning_content`, 스트림 델타 포함), `RouterModel::context_size`
  (`memory.effective.n_ctx`) → `ModelInstanceStatus.ctx`. `knowledge/answer.rs`: `ChatBackend::complete` / `complete_cancellable`.
  `documents/repo.rs`: `set_file_info`가 내용 해시 변경 시 파이프라인 상태 초기화(`reset_pipeline_state`).
- 테스트(43): 스키마·린트·메타스키마, 검증기 전반, 프롬프트/프리셋/루프/텍스트 예산, 호출기 재시도(4xx·OCR 예산·샘플링)·취소,
  이미지 예산, 분류 투표, OCR 표 파서·CSV 주입 방지, 추출 병합·필드 생성·정밀 모드·감사·PII 원문 마스킹·컨텍스트 초과 축소,
  분할/병합·이름 검증, 양식 도출, 대본 백엔드 통합: 스캔 영수증 end-to-end(분류→OCR(PII 마스킹)→KIE→
  검증→검토 큐→미러→지식 색인 재청킹), 디지털 PDF(OCR 생략·텍스트 KIE·필수 누락), 회전 재렌더·수동 유형, 취소·실패 격리, 검토
  흐름(이력·승격·복원·재검증), 스키마 CRUD·기본·버전·재정의, 양식 등록(구조/VLM)·NL 생성/수정, 분할 등록·병합, 리뷰 공백
  (정밀 샘플링·PII 미러·재검증 멱등·교정 검증·내용 변경 초기화·병합 이름·기본 스키마 이동·추출 쪽 예산).

### Tauri 셸 (`src-tauri`)

- `pipeline_bridge.rs`: 비전 모델 선택(`knowledge_bridge::chat_model` + 드롭인 재스캔 + 엔진 기동/취소 가능한 로드, 라우터가
  보고한 컨텍스트 크기를 `PipelineDeps.ctx`로 전달), `process_job`(`JobKind::Pipeline`, 진행률 = 문서+쪽, 끝나면
  `knowledge_bridge::auto_index` + 대기열(`ProcessRequest` 전체 보존) 처리), `auto_process`(가져오기 후: 드롭/폴더 → `auto_level`, inbox →
  `inbox_auto_level`, 다이얼로그 선택 우선; 모델 없음/엔진 꺼짐이면 `pipeline://changed {kind: no_model|engine_off}`), 분할/병합 작업,
  양식 도출(로드된 모델이 있을 때만 VLM, 아니면 구조/텍스트), NL 스키마(모델 로드), 워크스페이스 열 때 내장 양식 시드·중단 실행 정리.
- `commands/pipeline.rs` 28개 명령(status/process/doc_types/schemas CRUD·기본·사용·린트·NL·양식 도출/등록/문서 스키마 재정의/
  runs/fields/history/restore/revalidate/review_queue(문서명·유형·쪽 이미지 URL 포함)/review_apply/approve_document/tables/
  table_csv/table_export/split/merge; 오류 코드는 `UiError.code`에 — `no_documents`·`description_empty`·`merge_name_invalid`·
  `no_model`·`engine_not_running`·`model_not_loaded`…). `docs_bridge`: 작업 종류 `pipeline`·`document_split`·`document_merge`를
  워크스페이스 작업으로 취소·대기, 가져오기 작업 후 자동 파이프라인, 작업 실패 로그. 문서 삭제 시 추출 미러 폴더 삭제. 수동 유형
  변경은 `doc_type_source=manual`.

### 프런트엔드

- 추출 스키마/양식 화면: 등록 양식·사용자 스키마 / 내장 샘플 목록(기본·사용 배지), 필드 그리드(키·한/영 라벨·종류·필수·정규식·예시·
  선택지·마스킹·표/그룹 열, 위/아래/삭제) + 실시간 문법 린트, 자연어로 만들기/수정, 양식 등록(문서 선택·이름 → 도출 → 그리드 확인 →
  등록), 유형→스키마 매핑표, 버전 이력(불러오기)·변경 메모, 샘플 문서 테스트 실행, 새 유형 추가 시 "기타 문서 재분류" 배너,
  기본 지정·사용 토글·복제·삭제.
- 검토 큐: 유형/사유/문서 필터, 임계치 슬라이더(설정 연동), 목록 + 현재 항목(쪽 이미지·값·근거·검증), 키보드 Enter 승인·E 수정·
  S 보류·N 다음·R 거부(IME 조합 중 Enter 무시), 문서 전체 승인, 잔글씨/정밀 모드 재추출, 문서 열기(쪽·근거 강조).
- 뷰어: 유형 드롭다운이 등록 스키마의 유형으로 구성(출처 툴팁), 검토 상태·신뢰도·처리 오류 배지, "처리 ▾"(표준/분류만/OCR/잔글씨/
  정밀/처음부터), 분할(썸네일 선택 → 새 문서 시작), 필드 탭(실행 선택·스키마 변경·대기 수·모두 승인·검증 다시 실행, 값·신뢰도
  색+글리프·상태·사유·근거 클릭 → 쪽 이동+강조, 인라인 승인/수정/보류/거부/다음, 표 행 그리드), 표 탭(HTML/MD 표 → 복사·CSV 저장),
  이력 탭(실행 목록·필드 변경·이 값으로 복원), OCR 배지.
- 라이브러리: 일괄 "처리 ▾"·"양식으로 등록"·"병합", 유형/검토 필터, 카드 유형·검토 대기 배지, 자동 처리 건너뜀 안내; 가져오기
  다이얼로그 "가져온 뒤 자동 처리"(설정값/없음/분류/OCR/전체) + 예상 소요; 설정 > 처리(없음 수준·inbox 수준·텍스트 레이어 OCR·
  추출 쪽수·정밀 모드·밀집 재OCR); 홈 검토 대기 수 + 검토 큐 링크; 사이드바 검토 큐 배지; 작업 종류·요약 번역.
- 모의 백엔드 `mock-pipeline.ts`(파일명 분류, 유형별 OCR 텍스트·KIE 템플릿, 검증·마스킹, 검토·이력, 스키마 CRUD·버전·NL 파서·
  양식 도출, 표 파서, 분할/병합 시뮬레이션, 비전 모델 상태 플래그·대기열). Playwright `pipeline.spec.ts` 4건(IME 조합 포함),
  공용 헬퍼 `e2e/helpers.ts`.

### 테스트·검증

| 항목 | 결과 |
|---|---|
| `cargo test --workspace` (ko-dic + PDFium) | 코어 lib 166 통과(+1 ignored; 파이프라인 43), 셸·엔진 프로세스·real_engine 통과 |
| `cargo clippy --workspace --all-targets -D warnings`, `cargo fmt --check` | 통과 |
| vitest / Playwright | 27 / 21 통과(P4 4건: 처리·검토 큐·스키마/양식·IME) |
| `npm run licenses:check`, `cargo deny check licenses bans sources` | 통과 (lopdf MIT, regex MIT/Apache) |
| 실제 llama-server(CPU) + 초소형 Qwen3.5 | `real_engine` 테스트에 P4 경로 추가: 이미지+분류 문법 JSON, KIE 문법 수용, `reasoning_content` 없음 |
| 헤드리스 실행(Linux) | 아래 "헤드리스 검증" 참조 |

### 헤드리스 검증 (실제 데스크톱 바이너리 + 실제 llama-server, Xvfb + WebKitGTK)

스크립트 스크래치패드 `smoke-p4/run-p4.sh`: 초소형 Qwen3.5 fixture(무작위 가중치, 4K 컨텍스트)를 드롭인 모델 폴더에 두고
`active_model`로 지정, inbox에 `sample-photo.jpg`·`sample-contract.docx`, 자동 처리 `full`, `DOCULENS_LLAMA_SERVER`(CPU 빌드),
PDFium·ko-dic 지정. 결과(5회차, 2026-11-04):

- 앱 기동 → 내장 양식 7종 시드 → inbox 가져오기(DOCX 변환 포함) → 엔진 자동 기동 → 모델 로드(드롭인 재스캔 뒤; 라우터가 보고한
  컨텍스트 4,096을 파이프라인이 받음) → 두 문서 자동 처리가 모델 로드 후 9초 안에 끝남.
- 분류: 두 문서 모두 실제 서버의 문법 제약 JSON으로 분류됨(무작위 모델이라 유형은 `tax_invoice`, 쪽 유형·언어·회전 기록).
- OCR: 사진 1쪽 → `pages.text` 3,185자(`text_source=ocr`, `ocr_profile=accuracy`), `has_text_layer=1`, 지식 색인이 4청크로
  재청킹; 변환된 DOCX는 텍스트 레이어라 OCR 생략.
- KIE: 계약서(세금계산서 스키마) → 프롬프트 3,462토큰이 4K 컨텍스트를 거의 채워 생성이 630토큰에서 잘림 → "컨텍스트에 의해
  잘림, 재시도 안 함" 경고 후 실행 `failed`·`pipeline_error` 기록(무작위 모델은 JSON을 닫지 못하므로 예상된 결과). 사진 →
  서버가 출력 파서 오류 500("output does not match the expected peg-native format", 무작위 토큰이 `<tool_call>` 조각을
  만든 것)을 돌려줘 3회 재시도 후 실패 — 이후 이런 500은 seed·온도를 바꿔 재시도하도록 수정(단위 테스트). 6회차(수정 반영):
  무작위 모델에서는 T=0.1·0.2 재시도로도 같은 500이 나와 결과는 동일(실모델에서 재확인, 체크리스트). 필드·미러는 생성되지
  않음(실패 실행은 미러를 쓰지 않음).
- 교훈: 4 vCPU 상자에서 다른 프로세스가 코어 하나를 점유하면 llama.cpp CPU 스레드 풀이 초당 ~200토큰에서 1.4토큰으로 무너진다
  (3·4회차가 "멈춘" 원인 — 이전 셸 명령이 남긴 `rev` 프로세스). CPU 폴백 프리셋의 `--threads`를 물리 코어−1로 두는 것을 P5
  엔진 튜닝 항목으로 둔다. 또 `timeout`으로 앱을 죽이면 Linux에서는 엔진 자식이 남는다(Windows는 작업 객체로 정리) — 스크립트가
  뒤처리.

### 설계 대비 범위 메모

- 근거 좌표(bbox)는 미구현: 필드는 쪽 번호 + 근거 문자열을 가지며 뷰어는 쪽 이동 + 텍스트 탭 문자열 강조로 대신한다(§5.2 17번
  "v1은 디지털 PDF만 영역 단위"도 보류 — pdfium 문자 박스 추출은 P5/P7에서). 검토 큐의 이미지 크롭도 쪽 전체 이미지.
- 밀집 영역 재OCR은 영역 크롭 대신 **쪽 전체를 잔글씨 예산으로 한 번 더** OCR한다(셀 수 임계 기본 80). 토큰 로그확률 기반
  신뢰도는 미사용(모델 자기보고 + 근거/검증/2차 샘플 일치).
- 분류는 앞 8쪽 + 마지막 쪽만 모델에 보내고 나머지 쪽은 문서 유형을 따른다; 추출은 앞 N−1쪽 + 마지막 쪽(`kie_max_pages`).
- 스캔 배치 PDF의 **자동 경계 탐지**(§5.2 19번)는 미구현 — 수동 분할만. HWPX/DOCX 양식은 셀 좌표를 템플릿으로 보존하지
  않는다(§3.13 문서 생성은 2차).
- 양식 등록의 VLM 도출은 엔진에 모델이 이미 로드된 경우만 사용(다이얼로그가 분 단위로 막히지 않도록); 아니면 구조/텍스트
  휴리스틱. NL 스키마는 모델을 로드한다.
- 어시스턴트의 분류/추출/검토 명령(`classify`·`extract`·`review` op)은 P5 플래너에서 연결; 지금은 라이브러리/뷰어 GUI만.
- 실제 Qwen3.5-4B로는 검증하지 못했다(HF 차단): 분류 정확도·OCR CER·KIE 필드 F1·신뢰도 임계 보정은 사용자 PC 골든셋 항목.
  실서버에서 본 두 경계 사례 — 4K 컨텍스트에서 KIE 프롬프트가 컨텍스트를 채우는 것(속도·GPU 프리셋은 8K/16K라 본문 앵커가
  그대로 들어감)과 출력 파서 500 — 는 실모델에서 재확인한다(`docs/user-pc-checklist.md` P4).

## P4 단계 말 리뷰 반영 (2026-11-04)

리뷰 서브에이전트 2건(Rust 2 High·13 Medium·17 Low, 프런트엔드 3 High·14 Medium·14 Low)의 지적을 다음과 같이 반영했다.
보고서 원문은 세션 스크래치패드(`review-rust-p4.md`, `review-frontend-p4.md`).

**Rust — 반영(높음·중간 전부 + 낮음 대부분)**

- PII(§3.8): 모델 원문(`extraction_runs.result_json`·미러 `result.json`)과 표 행 근거에 주민번호·전화·계좌가 그대로 남던 문제 →
  `extract::mask_output`이 스키마를 따라 PII 종류·`mask` 필드를 `mask_value`로, 모든 문자열을 주민번호 패턴으로 가린 뒤 저장·미러;
  표 행 근거는 가려진 셀 값으로 구성, 정규화값도 마스킹(이미 가려진 값의 재검증은 그대로 통과), PII 값의 본문 탐색은 가려진 형태끼리.
  주민번호 정규식은 "하이픈 또는 붙여쓰기"만 허용(공백으로 나뉜 두 숫자는 매치하지 않음). 테스트(단위 + 통합) 추가.
- 정밀 모드: 2차 샘플이 시스템 프롬프트만 바꾼 탐욕 디코딩이었음 → `Vlm::with_sampling(temperature, seed)`로 같은 프롬프트를
  T=0.3·다른 seed로 샘플링(요청 동일성·온도·seed를 테스트로 고정).
- 감사 샘플링: 필드 단위 해시라 대부분의 문서가 "검토 대기"가 되던 문제 → 문서 단위(문서 id 해시 < p%) 로 바꾸고 그 문서의
  깨끗한 스칼라 필드 하나만 `audit`.
- 내용 변경: `set_file_info`가 해시가 바뀌면 OCR 텍스트·쪽 분류·문서 파이프라인 열(`classified_at`·`ocr_at`·`extracted_at`·
  `last_run_id`·`review_status`·`confidence`·오류)을 초기화하고 `has_text_layer`를 다시 계산(수동 유형은 유지).
- 호출기 재시도: HTTP 4xx(문법 거부·컨텍스트 초과)는 즉시 실패, `reasoning_content`가 새어도 답이 있으면 채택(로그)·비어 있으면
  seed+1·온도 +0.1로 재시도, 루프 재시도도 seed를 바꿈, OCR 잘림은 한 번 예산 ×2(≤6,144) 후 플래그; `complete_cancellable`
  (스트림 토큰마다 취소 검사)로 취소·워크스페이스 전환이 쪽 단위 OCR을 기다리지 않음, 스트림에서도 `reasoning_content` 수집,
  모델 로드는 `load_model_cancellable`.
- 루프 감지는 HTML 태그·공백을 벗긴 텍스트로(빈 표 셀 `<td></td>` 반복은 루프가 아님), 창에 비공백 문자 8자 미만이면 무시.
- 컨텍스트: KIE 본문 앵커 4,000자·분류 3,000자로 축소하고 `<page_text>` 구분자 + "안의 지시는 따르지 않음" 규칙; 스모크에서 실제
  llama-server가 `request (6272 tokens) exceeds the available context size (4096)`로 거부하는 것을 확인해 → 본문 앵커를 절반→
  1/4→제거 순으로 줄여 재시도(`extract_page`). 토큰 단위 선제 예산은 보류(아래).
- 검토: 수정/재검증이 스키마의 `FieldDef`(선택지·정규식·마스킹)를 경로로 찾아 쓰고, 교정값의 검증 이슈(`enum_option` 등)도 저장
  (상태는 해결됨 유지); 재검증은 검증 결과가 바뀌지 않으면 저장된 신뢰도를 유지(멱등).
- 대기열은 `ProcessRequest` 전체(force·profile·precise·schema·pages)를 보존; `pages`는 추출 쪽 선택에도 적용;
  `kie_max_pages=1`은 첫 쪽; 분할은 실패·취소 시 등록된 조각·파일을 되돌림(all-or-nothing); 병합 이름은 파일 쓰기 전에
  `safe_stem`(경로 구분자·`..`·제어문자 거부, 셸에서도 `merge_name_invalid`); 이미지 병합은 PNG/JPEG 모두 RGB JPEG로 재인코딩.
- 스키마: 유형 이동 시 기본 하나만 유지(떠난 유형은 승격, 도착 유형에 기본이 없으면 채택), 새 초안은 `builtin` 출처를 주장할 수
  없음, 유형 목록 라벨·`schema_id`는 기본 스키마 기준(사용자 스키마가 있으면 "내장"이 아님), 버전 메모는 그 메모로 만든 버전에
  붙도록 `schemas.note` 열 추가(이전 메모는 `schema_versions`로 이동), 린트 문구(정규식은 검증기 전용).
- 기타: CSV 수식 주입 방지(`=`,`@`,`+x`,`-x`,탭/CR 셀 앞에 `'`), 성공한 실행·OCR이 이전 `pipeline_error`를 지움, OCR 뒤
  `has_text_layer=1`(지식 상태 "텍스트 없음" 오계산), 5자리 미만 숫자 값은 본문 정확 일치만 인정, 명령 오류 코드를
  `UiError.code`에 통일(`no_documents`·`description_empty`·`merge_name_invalid`·`no_model`·`engine_not_running`…),
  `vision_model`이 로드 전에 드롭인 폴더를 재스캔(`rescan_and_sync`; 스모크의 "unknown model id" 원인), 작업 실패 로그.

**Rust — 보류(HANDOFF 숙제)**: 프리셋 컨텍스트 크기 기반 선제 토큰 예산(지금은 400 응답에 적응), 문법 속성 순서(알파벳 —
동작엔 무관), 워크스페이스 전환 후 대기열 폐기(P5 러너로 교체), OCR 출력 건전성 검사(이전 패스보다 훨씬 짧거나 쪽 어휘가 없으면
거부), llama.cpp 문법 변환기 CI 검사, 실모델 임계 보정(사용자 PC).

**프런트엔드 — 반영**

- 필드 패널: 행 컴포넌트를 모듈 수준 `memo`로 올려 한글 조합 중 입력 상자가 다시 마운트되지 않음(인라인 수정 DOM 유지, IME Enter
  무시); 확신이 높아 큐에 없는 필드도 수정·거부·되돌리기 가능(표 셀은 ⋯ 메뉴), 보류 행도 되돌리기; "검토 큐에서 보기" 진입점;
  검증 메시지 언어 분리(한/영), 실행 선택 aria, 정밀 표시 i18n; 뷰어 패널 `key={doc.id}`; 유형 출처·처리 오류를 정보 목록에 표시.
- 검토 큐: Enter가 포커스된 버튼·링크·슬라이더를 가로채지 않음(거부 버튼에서 Enter → 승인되던 문제), 목록을 roving-tabindex
  `listbox/option`으로, 진입 시·편집 다이얼로그 닫힌 뒤 루트 포커스(단축키가 바로 동작), `aria-live`로 현재 항목 안내,
  `@tanstack/react-virtual` 가상화 + 행 memo + 낙관적 제거(`setQueryData`), 임계치 변경 시 파이프라인 쿼리 무효화 + 안내문,
  문서 칩에 문서명, 보류는 처리 수에서 제외, 빈 값 Enter 저장 금지·진행 중 다이얼로그 유지.
- 신뢰도 색·글리프가 설정 임계치를 따름(`confidenceTone(c, threshold)`: ok ≥ 임계치, warn ≥ 임계치−0.2), 동적 Tailwind
  클래스 → 정적 맵.
- 스키마 화면: NL/양식 다이얼로그가 닫힌 뒤 도착한 모델 결과를 버림(요청 시퀀스), 저장 안 한 초안을 바꾸는 모든 경로에 확인
  다이얼로그(`schemas.editor.discard`), 목록 행 memo, 린트 코드 16종 i18n(`schemas.lint.*`), 복제 이름 i18n, 키 제안은 라벨
  blur 시·사용자가 키를 만지기 전까지만 + Rust `key_from_label`의 한국어 표(80항목)·접미 규칙·종류 추정 이식, 행 uid 키(인덱스
  키 제거), 린트 응답 순서 보호, 매핑표 `<thead>`, 재분류 배너에 스키마 이름. (라우트 이동 차단은 `HashRouter`라 `useBlocker`
  불가 — 보류.)
- 라이브러리·처리: 이미 추출/검토된 문서를 다시 "표준 처리"하면 새 실행으로 대체한다는 확인 다이얼로그, 병합은 라이브러리
  목록 순서, 검토 배지 클릭 → 그 문서로 필터된 검토 큐, 목록 보기에도 유형·검토 열, 파이프라인 상태 칩(모델 준비/미로드/없음/
  엔진 꺼짐 + 처리 중·대기 수; 상태 표시줄·라이브러리) + 모델 없으면 처리 메뉴 비활성, 가져오기 예상 소요를 수준별로
  (분류만은 초 단위), 설정 처리 숫자 입력을 `NumberField`(Enter 확정·외부 변경 반영)로 + `dense_cells` 편집, 작업 진행
  단계명 번역, 사이드바 검토 큐 링크 이름에 대기 수, 홈 빠른 가져오기 활성(`/library?import=1`), 오래된 문구(P1/P4 안내) 정리,
  미사용 i18n 키 삭제.
- 모의 백엔드: 비전 모델 상태 플래그(`doculens.mock.vision` = ready|not_loaded|none|engine_off)로 `no_model`/`engine_off`
  경로 재현, 파이프라인 작업 중 요청은 대기열(`null`) 후 순서대로 실행, 버전 저장 부작용 제거. E2E 헬퍼를 `e2e/helpers.ts`로
  통합(5개 spec), 작업 요약 문자열을 i18n 템플릿에서 생성, IME 조합 중 Enter가 승인/저장되지 않는 케이스 추가(21건).

**프런트엔드 — 보류**: 라우트 이동 시 미저장 초안 차단(데이터 라우터 전환 필요), 큐의 "강한 모델로 재추출"(단일 모델 구성이라
해당 없음 — 설계 §3.7.2 문구만 남김), bbox 근거 크롭, 표 XLSX 내보내기(P5 내보내기와 함께), 가져오기 ETA의 실제 쪽수 반영
(가져오기 계획에 쪽수 없음).

## P5 산출물

### Rust 코어 (`crates/doculens-core`)

- `planner/`: 설계 §3.5·부록 D의 v1.0 서브셋 33개 op를 serde 타입으로 고정(`Op`, `deny_unknown_fields` — 표에 없는
  파라미터는 거부), `Scope{folder, doc_ids, filter{doc_type, date_from/to, keyword, vendor, status, tags, min_confidence,
  review_status}}`, `Plan{intent, scope, operations ≤12, requires_confirmation, confidence, clarification_question, summary}`.
  - `schema.rs`: op·파라미터 표(`OP_SPECS`, 한/영 설명)가 프롬프트 도구 표와 `response_format` PlanSchema(`anyOf` 33개 op 객체,
    `const` 판별자, `additionalProperties:false`)를 함께 생성. 테스트가 serde 타입과 표의 일치를 검사.
  - `rules.rs`: 결정적 문장 규칙 — 되돌리기/다시 실행, 작업 제어(일시정지·재개·취소·재시도·우선), 엔진, 설정 복합문
    ("다크 테마로 바꾸고 모델을 9B로 올려"), 스키마 정의/수정, 내보내기(형식·필드·대상), 집계(함수·필드·그룹·날짜), inbox 처리
    (가져오기→분류→OCR→추출→이동), 정리/이동/이름변경, 태그, 삭제, 처리(분류/OCR/추출/검증/요약, 선택 문서·현재 쪽 범위),
    검토, 검색(프레임 단어 제거), 문서 열기, 질문→answer. 유형·필드는 컨텍스트의 라벨/키로 해석("합계만"→`grand_total`).
  - `prompts.rs`: 시스템 규칙(한/영) + op 표 + 워크스페이스 컨텍스트(유형·스키마 필드·폴더·태그·선택 문서·모델) + few-shot
    (한 8·영 3). 컨텍스트 예산(`ctx − 1,024 − 128`)에 맞춰 Full → Compact(설명 생략·언급 스키마만·예시 4) → Minimal(영문 규칙·
    op 이름+필수 파라미터·예시 2) 단계 선택, 서버가 더 많이 세면 한 단계 작게 재시도.
  - `validate.rs`: op 수·ISO 날짜·`date_to ≥ date_from`·워크스페이스 상대 경로(`..`·드라이브·`.doculens` 거부)·유형/스키마/
    필드/모델/설정값 해석(라벨→슬러그·키), 템플릿 변수, 확인 플래그 재계산, 상대 날짜의 결정적 치환, 신뢰도 정규화.
  - `llm.rs`: 프리라우터 → 규칙 → 모델(thinking off, T=0, 문법 제약) → 검증 실패 시 오류 피드백 재질의 1회 → 명확화;
    신뢰도 < 0.6이면 명확화 우선; 모델 오류는 `model_error` 이슈 + 명확화로 돌려주고 취소만 전파.
  - `scope.rs`: 범위 → 문서 목록(날짜 기준 `doc_date` → 없으면 가져온 날짜). `exec.rs`: 결정적 실행기 — 단계별로 가져오기
    (inbox/폴더)·분류·OCR·추출(파이프라인 `run::process` 재사용, 쪽 경계 체크포인트)·검증·검토 일괄 승인/거부·검색·답변·요약·
    집계·정리/이름변경/이동·태그·삭제·내보내기·되돌리기/다시 실행; UI op(테마·언어·설정·모델·열기·작업·엔진·도움말)는
    `ui_actions`로 반환; 첫 실패 이후 단계는 건너뜀; `preview()`가 확인 카드용 영향(정리 diff, 내보내기 경로·덮어쓰기, 삭제 건수,
    되돌릴 배치)을 계산.
- `organize/`: `template.rs`(`{{year}} {{month}} {{day}} {{doc_type}} {{vendor}} {{total}} {{title}} {{original_name}} {{name}}
  {{tags}} {{seq}} {{ext}}`, 값 없음 → `미상` + 검토 필요 표시, Windows 금지문자·예약어·NFC·≤240자), `mod.rs`(dry-run diff:
  이동/변경 없음/보류(검토 대기)/파일 없음, 폴더별 `{{seq}}`, 배치 내·디스크 충돌 `_2`; `apply`가 그 diff를 그대로 재생 —
  rename(복사 폴백) → DB 경로 갱신 → 저널 기록, 취소 시 부분 적용 유지), `journal.rs`(`journal_batches`/`ops_journal` +
  `journal/ops-YYYYMM.jsonl` 미러, LIFO 되돌리기 전 blake3 해시 검증·대상 점유 검사 → 항목별 "불가(사유: changed/missing/
  target_occupied/os_trash)", 되돌리기 자체도 배치로 기록되어 redo = 마지막 undo 되돌리기, 워크스페이스 휴지통 폴백 삭제는
  복원 후 재가져오기 경로 반환, 보존일 정리 `purge`).
- `aggregate.rs`: sum/count/avg/min/max, 필드는 유형별 스키마에서 키·라벨·기본 합계 필드로 해석, 검토 미완료(문서 pending 또는
  필드 검토 대기) 분리 집계 + `include_unreviewed`, 월/거래처/유형 그룹, 값 없는 문서 `missing`, 문서 목록 ≤500.
- `export.rs`: 필드/표/본문 대상 × XLSX(유형별 시트 + 품목 시트 또는 단일 시트, 공통 열 토글)·CSV(BOM, 품목 `_items.csv`)·
  JSONL·Markdown·검색 가능 PDF(pdf-writer: 쪽 JPEG + 보이지 않는 텍스트 레이어, Identity-H + ToUnicode), 기본 경로
  `exports/YYYY-MM-DD_<유형>.<ext>`, 워크스페이스 밖 경로 거부, 기존 파일은 `overwrite` 필요(`export_exists`), `.part` → rename,
  검토 완료만 옵션. 새 의존성 `rust_xlsxwriter`(MIT/Apache-2.0)·`csv`(Unlicense/MIT)·`pdf-writer`(MIT/Apache-2.0) —
  `cargo deny` 통과.
- `jobs/`: `jobs` 테이블(스펙 JSON·우선순위·상태 queued/running/paused/done/failed/cancelled/interrupted·시도 횟수·결과),
  `JobSpec{Pipeline, Plan, Export, Organize, Undo, Redo}`와 레인(엔진 레인은 한 번에 하나), `pick_next`(우선순위 → 생성 순),
  `JobControl`(취소·일시정지·재개), `EngineGate`(배치가 쪽 경계 `checkpoint`에서 양보 → 대화형 요청 `interactive` 끼어들기,
  상태바용 예상 대기 시간), 재시작 시 running → interrupted(재시도 가능).
- 마이그레이션 `0004_jobs_journal.sql`(`journal_batches`, `ops_journal`, `jobs`), `pipeline::prompts::Stage::Plan` 프리셋
  (T=0, 1,024 토큰), `documents::import` 재사용.

### Tauri 셸 (`src-tauri`)

- `jobs.rs`: 작업부를 영속 러너로 확장 — 가벼운 셸 작업(엔진·모델·가져오기·렌더·색인·분할/병합)은 그대로, 스펙이 있는 작업
  (`pipeline`·`plan`·`export`·`organize`·`undo`·`redo`)은 워크스페이스 DB에 저장하고 우선순위·레인에 따라 스케줄(`submit` →
  `schedule`), 일시정지/재개/재시도/우선/취소, 워크스페이스 열 때 복원(running → interrupted), 닫을 때 분리(대기열은 DB에
  보존 — P4 숙제 "전환 후 대기열 보존" 해결). `jobs://update` 페이로드에 `priority`·`persistent`·`attempts`·`result`·`label` 추가.
- `pipeline_bridge::process_job`이 큐 대신 러너에 제출(`pipeline_queue` 제거), 쪽 경계마다 `EngineGate::checkpoint`(일시정지·
  대화형 양보); 답변·NL 스키마·양식 도출은 `gate.interactive`로 끼어듦. `pipeline_status`에 `gate` 상태.
- `planner_bridge.rs`: `plan`(컨텍스트 = 유형·스키마 필드·폴더·태그·모델 id + UI 범위; 규칙으로 못 풀 때만 모델 로드 후 게이트
  안에서 호출; 미리보기 동봉), `execute`(재검증 → 검색/집계/태그/유형/검증/검토 승인·거부/UI op는 인라인, 나머지는 `plan` 작업),
  러너 워커(`run_spec`: 파이프라인/플랜/내보내기/정리/되돌리기 — 변경 이벤트·자동 색인·휴지통 복원 파일 재가져오기),
  정리 미리보기/적용(작업), 저널 조회/되돌리기(≤50건 인라인, 그 이상 작업)/다시 실행, 집계, 내보내기 미리보기/실행(작업).
- 명령(`commands/planner.rs`): `planner_plan`, `planner_execute`, `organize_plan`, `organize_apply`, `journal_batches`,
  `journal_entries`, `journal_undo`, `journal_redo`, `aggregate_run`, `export_preview`, `export_run`, `jobs_get`, `jobs_pause`,
  `jobs_resume`, `jobs_retry`, `jobs_prioritize`. 오류 코드: `plan_invalid`(detail=이슈), `plan_clarify`, `organize_nothing`,
  `nothing_to_undo`/`nothing_to_redo`, `export_exists`, `path_outside`, `template_variable`, `no_model`.

### 프런트엔드

- `types.ts`/`ipc.ts`: P5 계약(Op 33종·Scope·Plan·PlanResult/PlanResponse/PlanPreview·PlanExecution·Organize*·Journal*·Aggregate*·
  Export*·UiPlanContext, Job의 `priority`/`persistent`/`attempts`/`result`/`label`, `PipelineStatus.gate`)과 16개 명령 래퍼.
- 어시스턴트(`features/assistant/`): 입력 → 프리라우터(슬래시·설정은 즉시) → `plannerPlan`(범위 선택기: 전체/현재 폴더/선택 문서 N건/
  지식 베이스·컬렉션; `lastDocument`로 "이 문서"). 단일 `answer` 플랜은 기존 스트리밍 Q&A로, UI op는 `commands.ts`
  `executeUiOp`(테마·언어·설정·모델·프로파일·열기(문서 이름 해석)·스키마 화면·작업 제어·엔진·재색인·inbox 감시·도움말)로 즉시 실행.
  **플랜 카드**(`PlanCard`): 출처·신뢰도·해석, 단계 체크리스트(op 라벨·파라미터 요약 `describePlan.ts`), 미리보기(정리 diff 행·
  보류 표시·전체 보기, 내보내기 경로·덮어쓰기, 삭제 건수), 영향 줄, "검토 미완료 문서는 이동하지 않음" 토글, 검증 경고, 인라인 편집
  (`PlanEditor`: `opSpecs.ts` = Rust `OP_SPECS` 사본으로 생성하는 자체 폼 + 범위 편집), 승인 Ctrl+Enter / 취소 Esc(IME 가드).
  **실행 카드**(`ExecutionCard`): 작업 진행률·단계 상세·일시정지/재개/취소/Jobs 열기, 게이트 대기 안내. **결과 카드**(`ResultCard`):
  단계별 ✔/✖·요약, 검토 큐 열기(N)·이동 내역·↶ 되돌리기(`journalUndo` + 토스트)·폴더 열기·검색 결과 칩·요약·답변 카드 재사용,
  `ui_actions` 순차 실행. **집계 카드**(`AggregateCard`): 기준 날짜·건수·헤드라인 값·검토 완료/대기 분리·"검토 대기 포함" 재집계·
  월별 그룹·문서 목록. **명확화 카드**(`ClarifyCard`): 질문 + 입력 프리필, `no_model`이면 "모델 로드" 버튼. 예시 칩 추가.
- 작업(`features/jobs/`): 대기/진행 중/완료/실패 탭, 우선순위·시도 횟수 배지, 일시정지/재개/재시도/우선 버튼, 결과 섹션(`JobResult`:
  플랜 단계·내보내기 경로(폴더 열기)·정리 건수), `describe.ts`가 러너의 상세 문자열(`plan 3/5 · …`, `1/5 · import`, `exported N → path`,
  `moved/failed/skipped`, `restored/skipped`, interrupted/workspace closed)을 번역. `stores/jobs.ts`에 pause/resume/retry/prioritize.
- 정리/내보내기(`features/organize/`): 세 탭 — **정리**(범위 섹션 `ScopeSection`: 전체/폴더/유형/날짜/라이브러리 선택, 파일명 템플릿
  편집기 `TemplateEditor`(변수 칩·실시간 예시), 폴더 프리셋 월별/거래처별/유형별/사용자 정의, 검토 완료만, 미리보기 diff 표(상태 배지·
  값 미상 ⚠·50행 + 전체 보기), 확인 다이얼로그 → `organizeApply` 작업 추적(`useJob`) → 결과 토스트 + 되돌리기), **내보내기**(대상·형식·
  범위·열 체크박스·공통 열·레이아웃·경로(미리보기 기본값)·덮어쓰기, 미리보기 줄, 작업 추적, 결과 경로·폴더 열기·추가 파일),
  **이력**(저널 배치 목록·되돌리기/다시 실행·항목 펼치기·불가 사유 번역; `pipeline://changed{journal}`로 갱신). `/organize?tab=&selection=1`.
- 라이브러리: 선택을 `useDocumentsStore.selectedIds`로 공유, 일괄 바에 "정리…"/"내보내기…", **집계 바**(`AggregateBar`: 유형 필터 또는
  선택 시 표시, 필드/함수/검토 대기 포함, 300 ms 디바운스 재집계, 월별 칩); 표 탭 XLSX 내보내기(`exportRun target=table`);
  뷰어가 `lastDocument`를 기록. 상태바: `gate.batch_busy`면 "배치 작업 중 — 약 N초 후 응답". 전역 Ctrl+Z/Ctrl+Y(입력 중·다이얼로그 제외).
- 모의 백엔드(`lib/mock-planner.ts` + `mock-backend.ts`/`mock-pipeline.ts`): 규칙 플래너 TS 포트(`planText`/`rulesPlan`), 미리보기,
  인라인/작업 실행(파이프라인 모의 재사용), 정리 템플릿 렌더·저널(`doculens.mock.journal`)·되돌리기/다시 실행, 집계, 내보내기 레지스트리
  (`doculens.mock.exports`), 영속 작업(우선순위·레인·일시정지/재개/재시도/우선), `doculens.mock.planner_model=on`이면 미해석 문장을
  "모델" 출처 검색 플랜으로. i18n 섹션 `planner`(ops·params·card·execution·result·aggregate·clarify·ui), `organize`, `export`,
  `aggregate`, `jobs` 확장(탭 4개·우선순위·결과), `assistant` 확장.

### 테스트·검증

| 항목 | 결과 |
|---|---|
| `cargo test --workspace` | 205 통과(3 ignored: `gen_fixtures`·`gen_export_samples` 생성기 등) — P5 모듈 38건: 플래너 10(예시 명령 12종+α, 규칙 오탐 14문장, 스펙↔serde, 검증기·범위 필수, 모델 경로 재질의·단계·축소 재시도, 실행기 2), 정리/저널 9(템플릿·diff·충돌·보류·적용·되돌리기/다시 실행·삭제 복원·복구 0건·redo 무효화), 작업 3(저장소·우선순위·게이트·패닉/취소), 집계 7, 내보내기 11 |
| `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` | 통과 |
| `cargo deny check licenses bans sources` | 통과(rust_xlsxwriter·csv·pdf-writer 추가) |
| 실엔진(`tests/real_engine.rs`, 초소형 모델) | 채팅·KIE·**플래너 문법** 통과(아래) |
| `npm run typecheck`, `npm test` | 통과 — vitest 12파일 66건(플랜 설명·템플릿·집계·작업 설명·숫자 강제·어시스턴트 스토어·i18n 패리티) |
| `npm run e2e` | 28건 통과 — 신규 `planner.spec.ts` 4건(집계 카드, 내보내기 플랜→실행→결과, 정리 diff→Ctrl+Enter→되돌리기, 슬래시/명확화/Esc/Ctrl+Z), `organize.spec.ts` 3건(보류→월별 이동→이력 되돌리기, CSV 내보내기, 집계 바·정리 바로가기); P3 지식 Q&A spec은 플래너 경유로 유지 |
| `npm run licenses:check` | 통과 |

### 헤드리스 검증 (실제 데스크톱 바이너리 + 실제 llama-server, Xvfb + WebKitGTK)

- P4 스모크(`inbox/`의 사진·DOCX → 가져오기 → inbox 자동 처리 full)를 P5 러너 경로로 다시 실행: 자동 처리 요청이 `jobs` 테이블의
  영속 작업(`kind=pipeline`, 스펙 JSON 212 B, 우선순위 normal, 시도 1회)으로 제출되어 `planner_bridge` 워커가 모델을 로드
  (`pipeline: vision model tiny-qwen35 (context 4096)`)하고 분류·OCR을 끝냈으며(두 문서 모두 `doc_type` 기록), KIE는 P4와 같은
  무작위 모델 한계(컨텍스트 630토큰 절단 / 출력 파서 500 재시도 3회)로 실패해 작업이 `failed` + 오류 문자열 + 결과 JSON(409 B)으로
  저장됐다. 저널 배치 0건(정리 없음), 스키마 버전 4 마이그레이션 적용. 스크립트: 세션 스크래치패드 `smoke-p5/run-p5.sh`.

### 실엔진 검증 (`tests/real_engine.rs`, 초소형 무작위 Qwen3.5 + llama-server CPU)

- 채팅·KIE 문법에 이어 **플래너 PlanSchema 문법**(33개 op `anyOf`)이 실제 서버에 받아들여지고(400 없음), 4K 컨텍스트에서
  프롬프트 단계가 Minimal로 내려가 요청이 컨텍스트에 맞으며, 무작위 가중치의 출력은 문법 안의 JSON(`{"confidence": 1553,
  "intent":"undo", …}`)으로 돌아와 검증기가 신뢰도 정규화 → 명확화로 끝낸다(모델 오류 없음, 0.7초).
- 첫 실행에서 Compact 프롬프트(추정 1.9K 토큰)를 서버가 5.7K 토큰으로 세어 400이 났고(초소형 모델의 바이트 수준 토크나이저),
  그래서 "컨텍스트 초과 → 한 단계 작은 프롬프트로 재시도"를 넣었다. 실제 Qwen 토크나이저에서의 실제 토큰 수는 사용자 PC
  체크리스트 항목.
- 무작위 모델은 가끔 도구 호출 조각을 내어 서버의 출력 파서 500("does not match the expected … format")으로 끝난다(P4와 동일
  한 현상, seed·온도 재시도 3회 후에도). 테스트는 이 경우만 모델 아티팩트로 허용하고 400(문법·컨텍스트)과 그 밖의 엔진 오류는
  여전히 실패로 본다 — CI의 실엔진 잡이 두 번 중 한 번꼴로 이 경로를 탄다.

### 설계 대비 범위 메모

- op 집합은 부록 D 전수가 아니라 v1.0 핵심 서브셋(33개): render/rotate/split/merge/register_form/draft_document/add_to_kb/
  manage_collection/define_term/create_project/add_to_project/define_rule/check_consistency/request_supplement/set_retention/
  set_tier/set_backend/import_model/download_model/benchmark/open_workspace/compare/explain은 GUI 전용(P6·v1.1).
  `review correct`(필드·값)는 검증만 하고 실행은 검토 큐로 안내. `ocr region=selection`은 현재 쪽 + 잔글씨 프로파일로 근사
  (bbox 영역 재OCR은 P4 숙제 그대로).
- 플랜 카드 인라인 편집은 rjsf 대신 op 표(`OP_SPECS`)로 생성하는 자체 폼(의존성 추가 없음).
- 집계 `count`(필드 없음)는 검토 상태와 무관하게 범위 문서 수; 값이 있는 집계만 검토 완료/대기 분리.
- 삭제의 되돌리기는 워크스페이스 휴지통 폴백(USB 등)만 앱이 복원하고 OS 휴지통은 OS에 위임(설계 §3.5).
- Job 러너의 "완료 OS 알림·절전 방지"는 P7(패키징)에서 Windows API와 함께; 작업 로그 파일(`jobs/<id>.log`)은 DB `result`로 대체.
- 보존/파기 작업(저널 90일 등)은 `journal::purge`·`jobs::prune` 함수만 두고 일일 파기 작업은 P7.

## P5 단계 말 리뷰 반영 (2026-11-08)

리뷰 서브에이전트 2건(Rust 3 High·9 Medium·13 Low, 프런트엔드 2 High·7 Medium·10 Low)의 지적을 다음과 같이 반영했다. 보고서 원문은
세션 스크래치패드(`review-rust-p5.md`, `review-frontend-p5.md`).

**Rust — 반영**

- 플랜 안전장치: `import source=folder`는 항상 확인 카드(절대 경로 허용, 기본 비재귀, 미리보기에 폴더·파일 수 표시); 문서를 바꾸는
  op(`set_doc_type`·`tag`·`delete`·`rename`·`move`·`organize`·`review approve|reject|correct`)가 빈 범위(=전체 라이브러리)로 오면
  "전부/모든 문서"라고 말하지 않은 한 명확화로 전환(`validate::scope_required`, 실행 명령에서도 재검사 → `plan_invalid`); 2건 이상의
  `tag`/`set_doc_type`/검토 일괄은 확인 필요; 플래너 내보내기 경로는 `exports/` 아래만; 내보내기가 등록된 문서 경로를 덮어쓰지
  않도록 `path_is_document` 거부; 기본 이름 내보내기가 이미 있으면 `_2`로 비켜감; `define_schema`·`open` 문자열 길이 상한.
- 규칙 오탐: 라틴 트리거는 단어 경계 매칭(`"move"`≠remove, `"tag"`≠stage; 접두 매칭은 `classif*`처럼 표기), 영어 일시정지/재개는
  작업 단어 필요("find my resume"), 내보내기는 조사("로/to/as")가 아니라 동사·화살표·4단어 이하 짧은 명령만, 질문문(알려줘·있어?·
  what/how…)은 내보내기/inbox/정리/삭제/태그/처리 규칙을 건너뛰어 답변으로, "정리해서 알려줘"는 요약 질문, "읽어줘"는 OCR 아님,
  "재"만으로 강제 재실행 안 함, 삭제 규칙을 정리 규칙보다 먼저("remove these documents"), "이동"은 동사형만, 집계는 유형·기간·
  그룹·"전체"가 있을 때만(“합계 금액은?”은 Q&A), 짧은 주제어("xyzzy 플루토늄 원자로")는 지식 베이스 조회.
- 저널: 복구가 0건이면 배치를 되돌린 것으로 표시하지 않고 빈 되돌리기 배치도 지움(드라이브 재연결 후 재시도 가능); 삭제 복원의
  "다시 실행"은 재가져온 문서를 고아로 만들므로 `restore_not_redoable`로 건너뜀; 더 새로운 파일 배치가 생기면 redo 스택 무효화.
- 정리: 파일 이동 후 DB 갱신이 실패하면 파일을 되돌리고 그 항목만 실패 처리; 충돌 검사는 대소문자 무시(Windows); 렌더된 폴더/이름이
  `.`으로 시작하면 `_`로(`.doculens` 생성 차단).
- 러너/게이트: `EngineGate`에 RAII 가드(`batch_begin → BatchGuard`, 대화형 카운터 가드) — 워커 패닉이 게이트를 잠그지 않음,
  대기 중 취소 가능(`interactive_cancellable`); 플랜 작업은 모델 단계(분류/OCR/추출/답변/요약)에서만 엔진을 점유(`EngineLease`),
  가져오기·내보내기 중에는 Q&A가 기다리지 않음; 워크스페이스 전환 시 실행 중 영속 작업이 끝날 때까지(≤5초) 대기 후 분리, 전환
  중에는 새 작업을 시작하지 않음; `jobs://update`는 `result`를 빼고 보냄(`jobs_list`/`jobs_get`이 반환); 작업 제어 명령은 비동기;
  "마지막 작업 되돌리기"는 카드가 보여 준 배치 id로 고정; `.part` 임시 파일은 쓰기 실패 시에도 정리.
- 테스트 추가: 규칙 오탐 14문장, 범위 필수·폴더 가져오기 확인·내보내기 경로, 모델 플랜의 빈 범위 → 명확화, 복구 0건 되돌리기·
  redo 무효화·삭제 복원 redo 거부, 게이트 패닉/취소, 문서 경로 내보내기 거부, 기본 이름 중복 `_2`.

**Rust — 보류(HANDOFF 숙제)**: `set_rel_path`가 `original_name`을 덮어써 `{{original_name}}`이 첫 이름변경 후 바뀜(표시 이름과 분리
필요), 요약 `set` 모드(묶음 요약) 미구현, 작업 결과 payload 상한(집계 500건·정리 항목 전체), 플래너 모델 로드가 명령 바 호출
안에서 동기 실행(진행 표시만), `jobs.rs` 셸 러너 단위 테스트 부재, 집계/내보내기 N+1 질의.

**프런트엔드 — 반영**

- 계약 변경 대응: `jobs://update`에서 빠진 `result`를 스토어가 id별로 캐시·병합하고(`useJobsStore.fetch`/`useJobResultLoaded`), 완료
  전이 시 한 번 `jobs_list`를 새로고침; 실행 카드·작업 화면·정리/내보내기 탭이 결과를 `jobs_get`으로 읽음(로딩 표시).
- "이 문서" 범위 복구: 뷰어가 `lastDocument`를 언마운트 때 지우지 않고(워크스페이스 전환 때만), 어시스턴트 범위 선택기에 "현재 문서:
  <이름>" 옵션 + 빈 상태 힌트; `uiContextFor`에 `document` 범위.
- 정리 탭: 입력(템플릿·범위·검토 완료만)이 바뀌면 미리보기 플랜을 무효화(요청 키 비교) — 오래된 diff를 실행하지 않음.
- 모의 백엔드 `plan_invalid.detail`을 Rust처럼 `PlanIssue[]`로; E2E에 잘못된 편집(폴더 `../x`) → `plan-errors` 표시 → 수정 후 실행 흐름 추가.
- 내보내기 탭: 열 0개면 실행 차단 + 안내; 보이지 않는 탭에서는 미리보기 디바운스 중단(`active`).
- 실행 카드: 스토어에 없는 작업은 `jobs_get`/새로고침으로 복구, 사라진 작업은 "작업 목록에서 제거됨", 결과는 완료 후 로드,
  `onFinished` 1회 보장.
- 플랜 편집기 숫자 강제(`numeric.ts`: 정수 trunc·0 이상·NaN 거부), Esc는 입력 중(컴포저·편집기)엔 카드를 취소하지 않음.
- 뷰어 표 XLSX: 미리보기 후 같은 이름이 있으면 내보내기 탭으로 안내, 언어 전달, 작업 추적 후 "폴더 열기" 토스트(토스트 액션 추가).
- 어시스턴트 대화를 `stores/assistant.ts`(워크스페이스별)로 옮겨 화면 이동에도 유지; 작업 기반 플랜의 `ui_actions`는 job id별로
  추적해 완료 관찰 시 정확히 한 번 실행(`claimUiActions`), 워크스페이스 전환 시 초기화.
- 백엔드 오류 문자열 번역(`describeJobError`: engine_not_running·model_not_loaded·workspace closed·interrupted·schema_not_found·
  export_exists·path_outside·no_documents·단계별 실패), 작업 결과의 파이프라인 요약 번역, `plan_clarify` 사유 추가.
- 접근성: 미리보기 diff·이동 표에 `<caption>`/`<th>`, 아이콘 `role="img"`, 결과 카드 `aria-live`, 실행 카드 라이브 텍스트에 일시정지,
  여러 배치면 "마지막 배치 되돌리기" 라벨, 열거 라벨 번역.
- 테스트: Esc 취소/비취소, 집계 헤드라인·분리 값 변화, 잘못된 편집, 현재 문서 힌트(E2E); `describe.test.ts`·`numeric.test.ts`·
  `assistant.test.ts`(vitest 66건).

**프런트엔드 — 보류**: 플랜·실행·작업 행의 ETA/tok/s(백엔드 수치 없음), 집계 카드 "월별 보기" 재집계·"엑셀로 내보내기" 버튼,
결과 카드의 추출 결과 인용 칩, 명확화 카드 선택지 버튼, 어시스턴트 세션 목록(§3.7.6), 모델 경로에서 다중 op 플랜을 보여 주는 모의
(현재 미해석 문장 → 검색 플랜).

## P6·P7 산출물 (2026-11-08, 브랜치 `phase/p6-p7`)

사용자 결정으로 P6(문서 생성·프로젝트 정합성, v1.1)과 P7(품질·배포)을 한 세션에서 진행했다. 코어 두 모듈·프런트엔드·
평가 하네스·패키징은 서브에이전트 5개가 병렬로 구현하고(계약은 `generate/mod.rs`·`projects/mod.rs`에 먼저 고정),
플래너 op 확장과 셸 통합은 직접 작업했다.

### 플래너 (`planner/`, 직접)

- op 33 → **43**: `draft_document`(초안 생성 → 문서 생성 화면), `register_form`(UI), `explain`, `compare`, `create_project`,
  `add_to_project`, `define_rule`, `check_consistency`, `request_supplement`, `set_retention`(UI, `days < 0` = 무기한).
  `OP_SPECS`·PlanSchema·serde 타입·검증기(내용 길이, `sources` = 문서 id | `selection` | `aggregate`, 비교 2~5건, 프로젝트
  참조 → id 정규화 + 열린 프로젝트 기본값 채움, 보존 일수 범위)·확인 카드(초안·규칙 그리드·보완요청서·양식·보존 단축) 확장.
- `PlanContext`에 `projects[]`·`current_project` 추가(셸이 `projects_repo::list_projects`로 채움; UI가 열린 프로젝트를 넘김),
  프롬프트 컨텍스트 블록에 프로젝트 줄, few-shot 3건(견적서 초안·규칙 정의·검사 → 보완요청서) 추가.
- 규칙: `draft_document`(템플릿 키워드 표 + 등록 양식 우선순위 — "<유형> 양식으로"가 있을 때만 양식, 집계 결과 인용이면
  `aggregate → draft_document` 2단계), `project_ops`(보완요청서 → 정합성 검사 → 규칙 정의(비교 어구: 같아야/필수/이내/must equal…)
  → 프로젝트에 추가 → 프로젝트 만들기; 알려진 프로젝트 이름은 문장에서 가장 긴 일치로 해석), `explain_compare`(선택·용어·조항
  어구가 있을 때만 설명, 선택 문서 2건 이상일 때 비교), `set_retention`(항목 + N일/주/개월/년, 무기한, 즉시), `register_form`.
  오탐 방지: 질문문·정리/내보내기/검색 문장·"작성일자" 제외, "이어서 설명해줘"는 일반 질문 유지.
- 실행기: 초안(`generate::draft`, 출처 = 범위 문서 / 선택 / 직전 집계 payload / 지식 베이스, 결과 payload = `DraftOutcome` →
  결과 카드 "문서 생성 화면에서 열기"), 설명/비교(`knowledge::answer` explain·compare), 프로젝트 생성(+ 필수 유형이면 임시
  템플릿, 범위 문서 자동 추가)·추가·규칙 정의(확신 ≥0.6·미해석 없음이면 저장, 아니면 그리드용 초안)·검사·보완요청서.

### 셸 (`src-tauri`, 직접)

- `generate_bridge`: `draft` 작업(`JobSpec::Draft`, 엔진 레인, 보완요청서처럼 규칙으로 채울 수 있으면 모델 미로드) → 결과
  `DraftOutcome`(+`project_id`·`source`), 미리보기(HTML·MD)·재검증·저장(라이브러리 등록 → `documents://changed`)·초안 저장소.
- `projects_bridge`: 프로젝트/템플릿/규칙 CRUD, NL 규칙 정의와 검사는 **로드된 모델이 있을 때만** 대화형 게이트로 보조(없으면
  구문 규칙만), 요약 리포트, 보완요청서 → 초안 작업, `projects://changed { project_ids }`. 가져오기 작업 끝에 `inbox/<폴더>/`
  문서를 같은 이름 프로젝트에 자동 배정(없으면 생성).
- `maintenance`: `JobSpec::Maintenance`(IO 레인, 낮은 우선순위) — 저널 `purge` + 워크스페이스 휴지통, 완료 작업
  (`jobs::purge_finished_before`), 대화(`knowledge::repo::purge_answers_before`), 로그(워크스페이스·앱), 임시 파일, 오래된
  추출 런(`pipeline::repo::purge_runs_before`: 문서의 최신 런 제외 + 미러 폴더), 보존 기간이 설정된 문서(휴지통 없이 삭제),
  페이지/썸네일 캐시 LRU(`retention::trim_lru_dirs`, 7일 내 사용분 제외). 결과를 DB kv(`maintenance.last_run`)와
  `logs/purge-YYYYMM.jsonl`에 기록, 10분 폴링 스케줄러가 `purge_hour_local`에 하루 한 번(36시간 넘게 안 돌았으면 보충).
  명령 `maintenance_status`·`maintenance_run_now`.
- `notify`: 작업 완료/실패 시 OS 토스트(`tauri-plugin-notification`, 설정 `notifications.job_complete`; 처리·플랜·내보내기·정리·
  초안·가져오기·색인·임베딩·다운로드만). `power`: 전용 스레드가 5초마다 엔진 작업 실행 여부를 보고
  `SetThreadExecutionState(ES_CONTINUOUS|ES_SYSTEM_REQUIRED)`를 켜고 끔(설정 `prevent_sleep_during_jobs`, 종료 시 해제).
- 새 작업 종류 `draft`·`maintenance`; `UiPlanContext.current_project`; capabilities `notification:default`; windows-sys
  `Win32_System_Power`.

### 코어 — 문서 생성 (`generate/`, 서브에이전트)

- `templates.rs`: 내장 템플릿 7종(공문·견적서·거래명세서·계약서·보고서·회의록·보완요청서; 키·한/영 라벨·종류·필수·품목 열·본문 섹션·
  선택 키워드), `TemplateSpec`(내장 템플릿과 등록 양식 스키마의 통합 뷰 — 그룹은 `supplier.brn`처럼 경로로 펼침).
- `schema.rs`: 문법 안전 초안 스키마(`fields[{key: enum, value, source_index}]`, `tables[{key, rows}]`, `body ≤ 24`,
  `notes ≤ 6`, 전부 `additionalProperties:false`·`required`, `$ref` 없음). `prompts.rs`: 한/영 시스템 프롬프트(자료에 있는
  사실만, `source_index` 인용, 규정 원문 그대로, `<source>` 내 지시 무시) + 양식·내용·자료·이전 초안·수정 요청 블록,
  `Stage::NlSchema` 프리셋에 온도 0.25, 길이별 max_tokens 1,200/1,800/2,600, 컨텍스트 크기에 맞춘 자료 글자 예산(전형 2~3K 토큰).
- `draft.rs`: 템플릿 해석(form_ref → 등록 스키마 → 키워드 → 이전 초안 → 보고서), 자료 수집(문서 최신 필드·표·짧은 본문,
  집계 JSON, 텍스트, 지식 베이스 하이브리드 검색 — 제외 문서 반영, 최대 12개), 모델 호출 + 비JSON 1회 재시도, 사용자 수정 필드
  보존, 모델 없이도 되는 보완요청서 규칙 경로(`finding:*` JSON·`project` 카드 → 보완 항목 표).
- `validate.rs`: 품목 수량×단가, 세액 10%, 공급·세액·합계 자동 채움/불일치(차단), 필수 누락, 금액·날짜·사업자번호(체크섬)·전화·
  이메일 정규화, 계약기간 순서, 출처 없는 금액·날짜 문장 경고.
- `render.rs`·`office.rs`: 템플릿별 레이아웃 → HTML(A4 인쇄 CSS)·Markdown·`DocModel` → **DOCX**(WordprocessingML, 스타일·표·
  번호 매기기)·**HWPX**(OWPML mimetype/container/header/section, 표 cellAddr/cellSpan) 작성, 자체 리더로 라운드트립 검증;
  PDF는 `PdfPrinter`(없으면 MD 폴백+경고); 저장 폴더 `documents/generated`, `export_exists`/overwrite, `.part`→rename, 라이브러리
  등록(`role=generated`, 유형 = 템플릿/양식 슬러그, 같은 경로 재저장은 갱신).
- `fill.rs`: 등록 HWPX/DOCX 양식 채우기 — 라벨 셀 → 같은 행 다음 셀, `라벨: ____` 플레이스홀더, 품목 표 머리행 매칭 후 행
  복제(번호 열 자동, HWPX rowAddr 재번호), 미배치 필수 필드 경고. `store.rs`: `.doculens/drafts/<id>.json`.
- 테스트 20건(템플릿 일관성, 스키마 문법 안전, 스크립트 모델 초안·인용·검증·재생성, 지식 자료 번호·제외, 보완요청서 규칙 경로·
  프로젝트 JSON 매핑, 검증기, HTML/MD, DOCX/HWPX 라운드트립, 저장소, PDF 폴백·overwrite, 등록, 양식 채우기 DOCX/HWPX).
- 제한: 양식 채우기는 휴리스틱(라벨 오른쪽 셀만, 중첩 표 미지원), from-scratch HWPX/DOCX는 한컴/Word 실제 열기 미검증(사용자 PC).

### 코어 — 프로젝트·정합성 (`projects/`, 서브에이전트)

- 마이그레이션 `0005_projects.sql`(`project_templates`·`projects`·`project_documents`·`project_rules`·`project_runs`·
  `project_rule_results`, user_version 5). 카운터는 조회 시 계산, `last_check_at`만 저장.
- `repo.rs`: 내장 템플릿 3종(인증 심사 = 사업자등록증·신청서·계약서 + 규칙 3, 거래처 등록 = 사업자등록증·견적서, 구매 정산 =
  견적서·거래명세서·세금계산서 — 합계 일치·견적 ≥ 공급가액 within 5%), 템플릿/프로젝트/규칙/검사 런 CRUD(프로젝트 생성 시
  템플릿 규칙을 새 id로 상속, 빌트인 삭제 거부), 체크리스트, `detail`, `inbox/<폴더>/` → 프로젝트 자동 배정.
- `rules.rs`: 구문 규칙 — 연산 우선순위(기호 → neq → lte → gte → matches → eq → present), `N%`/`within N%`/`N원 이내` 허용 오차,
  한/영 피연산자 분리, 유형 별칭(세금계산서·invoice…), 필드 라벨→어간→표 열→종류 동의어(사업자번호→BRN, 합계→grand_total,
  서명→signed …), 리터럴(따옴표·숫자·날짜·정규식·`숫자 10자리`), `선택/optional` → required=false, 신뢰도·`unresolved`·`issues`;
  모델 폴백은 `Stage::NlSchema` + 문법 안전 RuleSchema(유형·필드 enum).
- `check.rs`: 문서별 최신 필드 정규화(BRN 10자리, 금액, ISO 날짜, 불리언, 전화/계좌, 명칭 NFC·㈜ 제거), 경로 폴백(표 열·마지막
  키·별칭군·문서 레코드), 판정 Pass/Mismatch/Undetermined(값 부재·검토 대기·거부·종류 불일치·정규식 오류·명칭 표기 차이)/Missing,
  같은 유형 문서 여러 건은 모든 쌍 중 최악, 명칭 비교는 결정적 실패 시에만 모델 yes/no 보조, 런 저장.
- `relations.rs`(brn·contract_no·amount·date·name·account 간선, 결정적 정렬), `report.rs`(Markdown 요약: 헤더·문서 표·체크리스트·
  검사 표·관계·다음 단계; 보완요청서 요청 = `finding:<verdict>:<rule_id>` JSON 소스 + 누락 유형 + `project` 카드).
- 테스트 15건(마이그레이션, 템플릿·프로젝트·규칙 CRUD, 체크리스트, 구문 규칙 ko 13·en 5, 모델 폴백, 판정 13규칙(통과 8·불일치 3·
  판단불가 1·누락 1), 동일 유형 다건, 불리언 present, 관계, 리포트, 보완요청서 소스, inbox 매핑, 정규화).
- 제한: 견적서·거래명세서·재무제표는 내장 추출 스키마가 없어 기본 어휘로 규칙 작성(사용자 스키마 등록 시 라벨 추종, 키 불일치는
  별칭군 폴백), 두 피연산자 단문만(합산식·복문은 모델/그리드), 검사는 ProjectStatus를 바꾸지 않음.

### 프런트엔드 (서브에이전트)

- 타입/IPC: `types.ts`에 Rust serde 미러(초안·프로젝트·규칙·검사·유지보수 타입, OpKind 43·Op 변형, `PlanContext.projects/
  current_project`, 작업 종류 `draft`/`maintenance`, 이벤트 `projects://changed`), `ipc.ts` 커맨드 31개, TanStack 쿼리 훅.
- **문서 생성 화면**(`features/generate/`): 요청 폼(내장 템플릿·등록 양식 선택, 형식 설명, 내용, 출처 문서 선택기, 지식 베이스
  토글, 어조·분량·형식) → `draft` 작업 추적 → 좌 미리보기(`sandbox=""` iframe) · 우 필드/품목표/본문 편집기(필수 표시, 검증 이슈
  배지, "제안 적용"), 참고 문서 패널(제외 → 재생성), 다시 생성(피드백), 검증, 저장 다이얼로그(형식·파일명·라이브러리 등록 →
  "라이브러리에서 보기"/"폴더 열기" 토스트), 초안 서랍(목록·열기·삭제). `stores/generate.ts`는 워크스페이스별 초안·어시스턴트
  핸드오버 보관.
- **프로젝트 화면**(`features/projects/`): 목록/칸반(접수→추출→검증→보완요청→완료, 이동 버튼), 새 프로젝트 다이얼로그(템플릿·
  접수번호·담당·기한·inbox 하위 폴더), 상세(`/projects/:id?tab=`) — 문서 탭(필수 유형 체크리스트·누락 표시·라이브러리에서 추가),
  규칙 탭(NL 입력 → `project_define_rule` → 규칙 그리드 확인·저장, 미해석 이름 강조, 켜기/끄기/삭제), 결과 탭(검사 실행, 판정
  칩, 근거 → 뷰어, 실행 이력), 관계 탭(SVG 그래프), 리포트 탭(Markdown 렌더), 보완요청서 다이얼로그(결과 선택 → 초안 작업 →
  문서 생성 화면), 템플릿 관리 다이얼로그.
- 어시스턴트: op 스펙·설명·플랜 편집기·확인 카드(규칙 그리드 미리보기, 보존 단축 경고)·결과 카드(초안 → "문서 생성 화면에서
  열기", 프로젝트/규칙/검사 → 프로젝트 링크, 설명/비교 → 답변 렌더), UI op `register_form`(→ 양식 등록 화면)·`set_retention`
  (→ `workspace_set_retention`), 열린 프로젝트를 컨텍스트로 전달.
- 설정: 일반 탭에 작업 완료 알림·절전 방지 토글, 프라이버시·보존 탭에 파기 작업 카드(마지막/다음 실행·요약·"지금 파기 실행").
  작업 화면: `draft`/`maintenance` 결과 표시·취소. 사이드바 "2차 범위" 그룹 제거(문서 생성·프로젝트가 주 메뉴로), 자리표시자
  컴포넌트 삭제. 다이얼로그는 한글 조합 중 Esc로 닫히지 않음.
- 모의 백엔드: `mock-generate.ts`(템플릿 7종, 문장 → 초안, 합계 불일치 제안, 검증, 렌더, 저장·등록, 초안 저장소, 보완요청서 규칙
  채움), `mock-projects.ts`(CRUD·내장 템플릿 3종·NL 규칙 파서·결정적 검사·이력·관계·리포트·보완요청서·유지보수), `mock-planner.ts`
  새 문장 규칙·검증·미리보기·실행.
- i18n ko/en 각 387 키 추가(총 2,159; 리뷰 반영에서 +26/−7), vitest 21파일 124건, Playwright 34건(`generate.spec.ts`: 템플릿 → 생성 →
  편집·제안 적용 → 저장 → 라이브러리, 같은 이름 저장 충돌 → 덮어쓰기, 재생성(피드백·참고 문서 제외); 어시스턴트 "견적서 만들어줘" →
  플랜 → 작업 → 결과 → 핸드오버, `projects.spec.ts`: 템플릿 → 문서 추가 → NL 규칙 2건 → 검사 → 관계 → 리포트 → 보완요청서 →
  칸반 이동(완료 확인 다이얼로그), 템플릿 관리(복제·불완전 규칙 차단·저장); 어시스턴트 "정합성 검사해" + 프로젝트 칩).
- 미완: 뷰어 드래그 "이 내용으로 문서 작성" 팝오버, 템플릿 다이얼로그의 NL 규칙 입력(그리드 수동 입력만), 모의의 inbox 하위
  폴더 매핑·`aggregate` 출처.

### 평가 하네스 (`crates/doculens-eval`, 서브에이전트)

- Rust bin 크레이트(설계의 Python 대신 — 툴체인·라이선스 단순화, `eval/README.md`에 기록). 하위 명령 `planner`(규칙+프리라우터,
  `--engine`이면 모델 경로까지; 의도·연산 순서·params 부분집합·scope 비교, 카테고리/의도별 정확도, 원인별 불일치 보고, 기준선 대비
  0.5pt 회귀 게이트 → exit 1), `pipeline`(골든 매니페스트 → 실제 엔진으로 분류 F1·OCR CER·필드 정확일치·ANLS*·p50/p95·VRAM,
  `--dry-run`은 엔진 없이 매니페스트 검증), `scorecard`(results.json 병합 → 양자화 비교표). 지표 단위 테스트 15건.
- 골든셋 `eval/golden/planner.json` 100건(ko 70·en 20·mixed 10, `OpKind::ALL` 43종 전부 + 설계 예시 12종, 항목별 컨텍스트
  덮어쓰기 지원) + `planner-context.json`; 파이프라인 매니페스트 예시; `docs/scorecard.md` 빈 템플릿; CI `eval-gate` 잡.
- **규칙 플래너 정확도 73.0% → 99.0%**(ko 100 · en 100 · mixed 90): 불일치 27건 분석에 따라 규칙 보정 — 라틴 단어+한글 조사 경계
  ("CSV로", "inbox에"), 구두점 트리거(".pdf"), "이 <유형>" 선택 참조, 스키마 필드 제거 동사, 검증 단독 명령, inbox 분류만, 추출
  문장·열린 문서 질문에서 집계 양보, 검토 단어 없는 승인/거부, "X 태그 빼줘"/"tag … as X", 설명 규칙 축소("what does … say about"
  제외), 검색어에서 유형 라벨 제거, 인사말 → noop, `keep … for 6 months`, 영어 `reviewed`, 추출 필드 채움, 새 규칙
  `quick_settings`(inbox 감시 on/off, 색인 재구축, 프로파일, DPI·검토 임계값, 수동 유형 — 한정어가 붙은 문장은 모델에 양보).
  남은 1건(거래처 키워드 범위 "Hanbit 거래 문서에 tag …")은 모델 몫. 기준선 `eval/baselines/planner-rules.json` 재기록.
- 미구현: TEDS/TEDS-S, 근거 bbox IoU, "골든셋으로 내보내기" UI, `pipeline --baseline` 자동 비교; 엔진 실행 경로는 이 환경에
  llama-server/PDFium 조합이 없어 컴파일·타입 검증까지(사용자 PC 첫 실행 항목).

### 패키징·고지 (서브에이전트)

- `scripts/fetch-engine.mjs --bundle`(→ `src-tauri/resources/engines/<pack>/`, 평평한 레이아웃, 팩 폴더 wipe 후 해제)·
  `--dry-run`·`GH_TOKEN` 인증; 레이아웃 결정 함수 분리. 앱데이터 레이아웃(`engines/<tag>/<pack>/`)은 그대로.
- `tauri.conf.json` resources: `engines/`, `licenses/`, `sbom.cdx.json` 추가(+ `.gitkeep`, `.gitignore`).
- `release-windows.yml` 재작성: 입력 `ref`·`variant(lite|full)`·`publish`; CPU/Vulkan 팩·PDFium·ko-dic 동봉 → 번들 검증
  (팩 폴더당 백엔드 계열 1개, CUDA 팩 DLL·EULA) → `gen-notices` → `tauri build` → `DocuLens-Setup-<Lite|Full>-<버전>-x64.exe`
  + `.sha256` → 아티팩트 → `publish`면 `gh release create v<버전>` + 자산 업로드. Full은 최신 릴리스의
  `DocuLens-EnginePack-cu121/cu129-<tag>.zip`을 받아 동봉. WebView2는 `downloadBootstrapper` 유지(부록 E).
- `cuda-packs.yml`(신규): CUDA 12.1/12.9 네트워크 설치기 무인 설치(+ cu121용 MSVC 14.39 툴셋) → llama.cpp 태그 빌드
  (`GGML_CUDA`, 아키텍처는 `engines/manifest.json`) → `llama-server.exe` + DLL + cudart/cublas/cublasLt + EULA →
  zip + sha256 → 아티팩트/릴리스. NVIDIA EULA 제약(앱 전용 폴더·PATH 미노출·앱 EULA 전제)을 주석으로 기록.
- `scripts/gen-notices.mjs`(`npm run notices`): `cargo metadata --locked`(+ Windows 타깃 scope 분류) + `package-lock.json`
  프로덕션 트리 → `THIRD-PARTY-NOTICES.md` 생성 구간, `sbom.cdx.json`(CycloneDX 1.5, 결정적 serialNumber, 번들 자산 포함),
  `src-tauri/resources/licenses/THIRD-PARTY-LICENSES.txt`(라이선스 전문, git-ignored). CI `notices` 잡이 stale을 잡음.
  현재 수치: crates 628(runtime 354), npm 프로덕션 75, SBOM 컴포넌트 709.
- 문서: `docs/RUN-GUIDE.md` §1–2(릴리스 설치파일·SHA-256·`--bundle`), `engines/README.md`(CUDA 팩 워크플로), `LICENSE-POLICY.md`.
- 여기서 못 한 것: 실제 팩 다운로드·NVIDIA 설치기·CUDA 빌드·`tauri build`·릴리스 게시(Windows 러너 필요);
  `engines/manifest.json`의 cpu/vulkan sha256은 첫 릴리스 로그로 `--pin`.

- **첫 설치파일 빌드 성공(2026-11-08, Actions run 42471185353, ref `phase/p6-p7` @ fbd360c)**: `DocuLens-Setup-Lite-0.1.0-x64.exe`
  43.1 MB, SHA-256 `cf7e113f0bdeef399dd971e7dc9d172f45951825d2099182f47bfed6eab2aeb0`, 아티팩트 `DocuLens-Setup-Lite-0.1.0-x64`
  (45.2 MB zip, 2027-02-06 만료, 릴리스 게시 안 함). 동봉 검증 단계 통과(engines/cpu·vulkan, pdfium, ko-dic). 첫 시도는 GitHub
  릴리스 다운로드 장애(HTTP 504)로 llama.cpp 팩 단계에서 실패 → fetch 스크립트가 REST 자산 엔드포인트(API → 서명된 CDN
  리디렉션)를 먼저 쓰고 공개 URL로 폴백하도록 바꿔 통과. 이때 받은 공식 팩 해시를 `engines/manifest.json`에 고정(cpu
  `cc2aad48…`, vulkan `8eec3c8a…`; PDFium은 기존 핀과 일치). 설치파일 내용 7-Zip 목록·필수 항목 검사 단계를 워크플로에 추가.
- **검증 빌드(run 42475854955, 5adb585)**: 같은 장애로 3·4번째 빌드가 `tauri build`의 NSIS 도구 다운로드(단일 요청, 재시도 없음)에서
  504로 실패 → `scripts/fetch-nsis.mjs`가 NSIS 3.13 + nsis_tauri_utils 0.5.3을 API 우선 경로로 받아 SHA-1 검증 후
  `%LOCALAPPDATA%\tauri\NSIS`에 배치하고(설치된 CLI가 그 플러그인 버전을 참조할 때만), 워크플로는 NSIS 캐시 + `tauri build` 3회
  재시도. 결과 `DocuLens-Setup-Lite-0.1.0-x64.exe` 43.0 MB, SHA-256 `1f16c170810da8f01969b7d3a0adbbe64e072fd238950a843a3efda34d884b17`,
  7-Zip 목록 159개 파일·303 MB(압축 전): `engines\cpu`·`engines\vulkan`(ggml-base, ggml-cpu-* 14종, ggml-rpc, ggml-vulkan 42 MB,
  llama-server.exe), `pdfium\pdfium.dll` 7.6 MB + 라이선스 18종, `ko-dic\` 사전 85 MB, 고지 파일·SBOM·라이선스 원문. 고정한 팩
  해시와 일치. 참고: 최근 llama.cpp 빌드의 `llama-server.exe`는 9.7 KB 런처이고 서버 본체는 별도 라이브러리다(로컬 참조 빌드는
  `libllama-server-impl.so` + `libllama.so` + `libmtmd.so`); 목록 필터가 ggml DLL과 런처만 찍어 나머지 DLL은 다음 빌드부터 출력된다.
  공식 zip의 다른 도구 실행파일(llama-cli 등)도 팩 폴더에 그대로 들어가므로 설치파일을 더 줄이려면 `llama-server*` + DLL만 남기는
  정리가 가능하다(v1.x 메모).
- **CUDA 팩 빌드 성공(2026-11-09, Actions "CUDA engine packs (manual)", ref `phase/p6-p7`)**: 첫 cu129 빌드(run 42494546661,
  16047af)는 성공해 Release `v0.1.0`을 만들었지만 `--version`이 `build 1`(얕은 클론 — llama.cpp가 `git rev-list --count HEAD`로
  빌드 번호를 만듦)이었고, cu121(run 42494544606)은 툴킷 폴더에 EULA 텍스트가 없어 조립 단계에서 실패. fb0f03f에서 전체 이력
  클론(`--single-branch --branch <tag>`), EULA 재귀 탐색(`EULA|License|LICENSE`, 실패 시 루트 목록 출력), 매니페스트에
  `documentation_<ver>` 서브패키지 추가, `engine::locate::parse_version_output`이 llama.cpp 0.8 형식
  `version: 0.8.0-dev (build 12066, commit …)`을 읽도록(`build ≤ 1` = 알 수 없음) 고친 뒤 재빌드 두 건 모두 성공
  (cu121 run 42533483890 약 65분, cu129 run 42533486332 약 116분; `--version` → `build 12066`; EULA는 `v12.1\EULA.txt`·
  `v12.9\LICENSE`). Release `v0.1.0` 자산: `DocuLens-EnginePack-cu121-b12066.zip` 944,112,865 B SHA-256 `4a15af86…`,
  `DocuLens-EnginePack-cu129-b12066.zip` 812,703,965 B SHA-256 `3116d674…`(각 `.sha256` 동봉; 첫 cu129 팩은 `--clobber`로 덮어씀).
  Full 설치파일 빌드는 이 `.sha256`로 팩을 검증해 동봉한다(매니페스트 고정은 후속 — `cu121`/`cu129` 항목에 sha256 필드 없음).
- **Full 설치파일 1차 빌드 실패 → NSIS 압축 방식 변경(2026-11-09, run 42543620988 @ fb0f03f)**: CUDA 팩 다운로드·SHA-256 검증·번들
  검사(207 파일, 3,142.6 MB — cu121 팩 1,875 MB: `ggml-cuda.dll` 1,233 MB + cuBLAS 607 MB; cu129 팩 1,017 MB: cuBLAS 735 MB +
  `ggml-cuda.dll` 247 MB, llama.cpp가 CUDA ≥ 12.8에서 켜는 `-compress-mode=size` 덕)까지 통과했지만 makensis가 세 번 모두
  `Internal compiler error #12345: error mmapping file (2065039454, 33554432) is out of range`로 실패. 원인은 Tauri 2.12.4 NSIS
  템플릿의 `SetCompressor /SOLID`: solid 모드에서는 데이터 블록을 압축 전 상태로 32비트 오프셋에 두므로 2 GiB를 넘을 수 없다.
  해결: 템플릿을 `src-tauri/nsis/installer.nsi`로 복사해 `/SOLID`만 뺀 파일별 LZMA 압축(한계가 압축 후 크기에 적용, Full ≈ 1.6 GB
  예상)으로 바꾸고 `tauri.conf.json` `bundle.windows.nsis.template`로 지정. `scripts/check-nsis-template.mjs`(`npm run nsis:check`,
  CI web 잡 + 설치파일 워크플로)가 복사본에서 표시 블록을 원복한 결과가 설치된 `@tauri-apps/cli` 네이티브 바이너리에 그대로
  들어 있는지 확인해 CLI 업그레이드 시 재동기화를 강제한다. 워크플로에 2 GiB 크기 가드(GitHub 릴리스 자산 한계), 목록 필터에
  cuBLAS/cudart/EULA, 아티팩트 무압축 업로드 추가. 팩 자체 축소(cu121 `-Xfatbin=-compress-all`, 도구 DLL 제거)는 v1.x 메모.
- **Lite 재빌드(2026-11-09, 9e56dd6 — 이미지 위 글자 레이어·우측 패널 AI 검색/이력 반영; run 42567728857, 아티팩트만)**:
  `DocuLens-Setup-Lite-0.1.0-x64.exe` 50,171,425 B(47.8 MB), SHA-256 `346cf36cb15e75b2455570149020a0048cc0d7c42a20292e66acbe12ed097d93`,
  7-Zip 목록 159개 파일·필수 7개 통과, 워크플로 12분 37초, 아티팩트 `DocuLens-Setup-Lite-0.1.0-x64`(id 13651909906, 2027-02-07 만료).
  전체 검증(Rust 285건·vitest 139·Playwright 41, 두 커밋의 CI 7개 잡) 통과 후 실행. `publish=true` 게시는 이 세션의 권한 분류기가
  막아(공개 표면 생성) 실행하지 못했으므로 **Release v0.1.0의 Lite는 아직 16c224f 빌드** — 사용자가 `variant=lite`·`publish=true`로
  워크플로를 실행하거나 다음 세션에서 게시를 지시해야 함. 같은 소스(488af39)로 먼저 시작한 run 42567337239는 PDF `/Rotate`·MediaBox
  오프셋 수정(9e56dd6)을 넣기 위해 취소.
- **Lite Release 게시(2026-11-09, 16c224f = 14577a6 + 문서; run 42561958088, `publish=true` — 사용자 지시)**: Release `v0.1.0`에
  `DocuLens-Setup-Lite-0.1.0-x64.exe` 50,116,430 B(47.8 MB), SHA-256 `d665687ad4fa1139766cff6a7cf9682fd5a4106a3123459bb40d12ed35ab993c`,
  `.sha256`, `THIRD-PARTY-NOTICES.md`, `sbom.cdx.json` 업로드(CUDA 팩 2종과 함께 자산 8개). 같은 소스의 아티팩트 빌드(아래)와 해시가
  다른 것은 NSIS 빌드 타임스탬프 때문(내용 동일). Full 설치파일은 아직 Release에 없음(아티팩트만).
- **Lite 재빌드(2026-11-09, 14577a6 — 드래그 앤 드롭·XLSX/PPTX/텍스트·내용 감지·뷰어 텍스트 직접 편집 반영; run 42560517332)**:
  `DocuLens-Setup-Lite-0.1.0-x64.exe` 50,117,790 B(47.8 MB), SHA-256 `5a178421b7091355f85fbed7e25e5d7e7660fd48d3799ab438c3a9efa9da1a30`,
  159개 파일·필수 7개 통과, `tauri build` 10분, 아티팩트 `DocuLens-Setup-Lite-0.1.0-x64`(id 13650165163, 2027-02-07 만료). 사용자
  지시로 Lite만 다시 빌드(Full은 76a3b0d 빌드 그대로 — 새 기능을 Full로도 쓰려면 variant=full 재실행).
- **Full·Lite 설치파일 빌드 성공(2026-11-09, 76a3b0d; Full run 42547228170, Lite run 42547230697)**: 템플릿 검사는 처음에 Windows
  체크아웃의 CRLF 때문에 실패(73a96d1) → 줄바꿈 정규화 + `.gitattributes`(`eol=lf`)로 고쳐 통과(Windows CLI 바이너리에도 LF로
  내장). Full: `DocuLens-Setup-Full-0.1.0-x64.exe` 1,156,143,051 B(1.1 GB — 압축 전 3.1 GB; 파일별 LZMA가 예상 1.6 GB보다 잘 줄임),
  SHA-256 `4c8229246bba54e18a78180b9eb145f78183bf28bab0902ca778a05ab76c8de9`, 7-Zip 목록 219개 파일, 필수 항목 9개 통과
  (`engines\cu121`·`cu129`에 llama-server·ggml-cuda·cublas·cublasLt·cudart·NVIDIA-CUDA-EULA 확인), `tauri build` 24분(Rust 5분 +
  NSIS 압축 약 18분), 아티팩트 `DocuLens-Setup-Full-0.1.0-x64`(id 13644687803, 2027-02-07 만료). Lite: 47.6 MB(49,949,072 B,
  solid 43.0 MB 대비 +4.6 MB), SHA-256 `29501b8fdd4b19da7a73403e7346b25519d7f21acc57801ea91a323aa5874960`, 159개 파일·필수 7개
  통과, 아티팩트 id 13643799614. 둘 다 `publish=false`(이 세션의 권한 정책이 릴리스 게시를 막음) — Release 게시는 사용자가
  `publish=true`로 실행. 사용자 PC 검증(설치·엔진 카드 백엔드 cu129/cu121·`CUDA0` 로그)은 체크리스트 P7.
### 드래그 앤 드롭 (2026-11-09, 사용자 요청 "문서를 추가하는 모든 기능에 드래그 앤 드롭")

- **공통 인프라 `src/lib/dnd/`**: `store.ts`(zustand 상태 + 드롭 대상 레지스트리: 요소에 `data-doc-drop="<id>"`, 포인터 위치를
  `elementFromPoint`로 판정, 중첩 대상은 안쪽부터 페이로드 종류(`documents`/`files`)를 받는 대상으로 폴백, 스프링 로드 타이머),
  `useDragSource.ts`(`startDocumentDrag`: 포인터 6 px 이동 후 드래그 시작 — HTML5 DnD가 아니라 포인터 이벤트. Tauri의
  `dragDropEnabled`(네이티브 파일 드롭에 필요)가 Windows에서 HTML5 드래그를 막기 때문), `useDropTarget.ts`(`useDocumentDropTarget`:
  `onDocuments`/`onFiles`/`onHover`), `DndLayer.tsx`(앱 셸에 1개: 창 단위 pointermove/up, 드래그 뒤 클릭 1회 삼킴, Tauri
  `onDragDropEvent`의 물리 좌표를 `devicePixelRatio`로 나눠 판정, 브라우저 폴백은 HTML5 `dragover`/`drop`의 파일 이름, 드래그 고스트),
  `importAttach.ts`(`importDroppedFiles`: 계획 → 이미 있는 문서 id 수집 → 가져오기 잡 → `waitForJob` → 잡 결과 `ImportReport`의
  imported/relinked id 반환; 새 문서가 없으면 호출자가 기존 id를 직접 연결), `DropHint.tsx`(점선 안내 오버레이).
- **드래그 소스**: 라이브러리 카드·행(선택에 포함된 문서를 끌면 선택 전체, 아니면 그 문서; `absolute_path` 동봉).
- **드롭 대상**: 어시스턴트(페이지 전체 → 첨부 문서 칩, 범위 "첨부 문서 N개", 질문은 `scope.documents`, 플래너는 `selected_doc_ids`,
  사용자 메시지에 첨부 이름 기록; 스토어 `stores/attachments.ts`), 홈 명령 카드(첨부 후 어시스턴트로), 지식 컬렉션 항목·선택 컬렉션
  문서 목록(문서 → `knowledge_collection_add`, 파일 → role knowledge + `collection_id`로 가져오기), 적재 마법사 1단계(파일 경로 추가,
  라이브러리 문서는 경로로 재등록 → "이미 등록됨" → 컬렉션에만 추가), 프로젝트 문서 탭·칸반 카드·목록 행(문서 →
  `project_add_documents`, 파일 → `ImportOptions.project_id`로 가져오기), 문서 생성 원본 상자(원본 추가), 양식 등록 대화상자(샘플
  문서 선택), 병합 대화상자(순서 목록에 추가·제거). **사이드바 메뉴**는 각 화면의 "추가"를 대신 수행(어시스턴트/홈 → 첨부, 라이브러리
  → 가져오기 대화상자, 지식 → 컬렉션 선택 대화상자/적재 마법사, 프로젝트 → 현재 프로젝트에 추가 또는 프로젝트 선택 대화상자
  `PickProjectDialog`, 문서 생성 → 원본, 스키마 → 양식 등록)하고, 끌고 있는 동안 700 ms 머물면 그 화면으로 전환(스프링 로드)해 화면 간
  드롭을 가능하게 한다.
- **백엔드**: `ImportOptions.project_id`(컬렉션처럼 배치 전체 — 신규·재연결·이미 등록·중복 — 를 프로젝트에 연결, 없는 프로젝트는
  `project_not_found`), 가져오기 잡의 `result`에 `ImportReport`(`JobBook::set_result`), 프로젝트 이벤트 발행. 모의 백엔드 동일.
- **검증**: `src/lib/dnd/store.test.ts` 7건(레지스트리·중첩 폴백·드래그 디스패치·스프링 로드 타이머·네이티브 드롭 폴백), Playwright
  `e2e/dnd.spec.ts` 5건(카드 → 어시스턴트 메뉴 첨부·제거·메시지 기록, 다중 선택 고스트 "문서 2개", 스프링 로드로 프로젝트 화면 전환 후
  카드에 드롭, 문서 생성 원본, 브라우저 파일 드롭 → 컬렉션 가져오기), Rust `run_attaches_the_batch_to_a_project_and_rejects_unknown_projects`.
  Tauri 네이티브 좌표 판정·WebView2 동작은 사용자 PC(체크리스트).

### 입력 형식 확장·알 수 없는 형식 처리 (2026-11-09, 사용자 요청 "xlsx·pptx 처리 추가, unknown 데이터 타입 처리 방안 검토")

- **XLSX** `convert/xlsx.rs`: 시트(보이는 것만) → 제목(시트 이름) + 표. 공유/인라인 문자열(리치 런 포함), 숫자(셀 서식의 내장·사용자
  정의 번호 형식으로 날짜·시각 판별 → `YYYY-MM-DD`, `HH:MM`), 불리언, 오류값, 수식의 캐시 값, 병합 셀(`mergeCells` → rowspan/colspan),
  열 너비(문자 단위 → mm), 1904 날짜 체계. 시트당 5,000행·64열 상한(초과 시 메모), 숨김 시트는 메모만. 첫 행이 단독 셀이면 문서 제목.
- **PPTX** `convert/pptx.rs`: `sldIdLst` 순서로 슬라이드 → 제목 자리표시자(제목 1, 부제목 2) 먼저, 나머지 도형은 읽기 순서(위→아래,
  왼→오른; 그룹 도형 평탄화)로 텍스트 상자(글머리 기호 상속: 본문 자리표시자는 기본 불릿, `buNone`/`buAutoNum` 반영, 서식 b/i/u/sz,
  `a:br`), 표(`gridSpan`/`rowSpan`, `hMerge`/`vMerge` 연속 셀 제거, `firstRow` 머리글), 그림(`blip` → 슬라이드 rels → `ppt/media`,
  슬라이드 간 중복 1회 저장), 슬라이드 번호·날짜·바닥글 자리표시자 제외, 발표자 노트(기울임, "발표자 노트" 소제목). 차트·SmartArt·OLE는
  메모. 슬라이드 크기 → `page_mm`(16:9 = 339×191 mm; 프린터는 A4 세로 고정이라 표/그림은 폭에 맞춤). 제목 없는 슬라이드는 "슬라이드 N".
- **텍스트 계열** `convert/text.rs`(새 `SourceKind::Text`): 디코딩 BOM(UTF-8/UTF-16) → UTF-8 → CP949(`encoding_rs`, Apache/MIT) → 손실
  UTF-8 순. CSV/TSV → 표(RFC 4180 따옴표·개행, 구분자 추정 `,`/`;`/탭, 첫 행 머리글), Markdown → 제목·목록·표·코드 블록·링크 라벨,
  HTML → `<title>`·h1~h6·문단·목록·표 셀(탭), `<script>`/`<style>` 제거·엔티티 복원, JSON/XML/LOG → 120줄 단위 문단, 그 외 → 빈 줄
  기준 문단(첫 줄이 제목). 16 MB 상한(초과 메모). `.txt`라도 HTML로 시작하면 HTML로 읽음.
- **알 수 없는 형식 처리 방안(검토 결과 → 구현)** `documents/sniff.rs` + `import::classify`:
  | 상황 | 이전 | 지금 |
  |---|---|---|
  | 확장자만 보고 판정 | `from_path` 전용 | **내용 우선**: `%PDF`, PNG/JPEG/GIF/BMP/WebP/TIFF 시그니처, zip 컨테이너는 항목 접두(`word/`→DOCX, `xl/`→XLSX, `ppt/`→PPTX, `mimetype`/`Contents/`→HWPX), CFB는 스트림 이름(`HwpSummaryInformation`/`BodyText`→HWP, `WordDocument`/`Workbook`/`PowerPoint Document`→`office_legacy`, `EncryptedPackage`→`encrypted`), `MZ`/ELF→`executable`, RTF→`office_legacy`, RAR/GZ/7z→`archive`, 그 밖에 NUL 없이 97 % 이상 출력 가능 문자로 디코딩되면 텍스트 |
  | 확장자와 내용 불일치(.pdf인 HTML, .zip인 HWPX, .docx인 구형 DOC) | PDFium 오류 또는 미지원 | 내용 기준 형식으로 가져오고 미리보기에 메모 "확장자(.pdf)와 내용이 달라 텍스트(으)로 처리"; 읽을 수 없는 컨테이너는 정확한 사유 |
  | 확장자 없음/모르는 확장자 | `no_extension`/`unsupported_extension` | 내용이 인식되면 가져오기(메모), 아니면 기존 사유 |
  | 텍스트 파일 | 미지원("텍스트 파일은 문서로 가져오지 않습니다") | 지원(위 텍스트 계열) |
  | XLSX/PPTX | `office_legacy`로 거부 | 지원 |
  | DOC/XLS/PPT/HML/RTF | `office_legacy` | 동일(허용 라이선스의 Rust 파서가 없음 → 사용자 PC에서 DOCX/XLSX/PPTX/PDF로 저장 안내) |
  | HEIC/HEIF | `heic` | 동일(디코더 특허·의존성 → JPG 변환 안내) |
  | 암호 문서 | PDFium/zip 오류로 실패 | CFB `EncryptedPackage`는 `encrypted` 사유로 사전 거부(암호 PDF는 PDFium 오류 유지 — 사유 세분화는 후속) |
  | 알 수 없는 이진 파일 | 미지원 | 동일. **보류한 안**: "원본만 보관"(페이지 없는 첨부 문서, 이름 검색만) — 상태·UI 추가가 필요해 v1.x 후보로 기록 |
  재스캔(inbox)도 같은 분류를 써서 확장자 없는 파일을 집어낸다. 미지원 사유 문자열(`documents.unsupported.*`)에 `archive`·`executable`·
  `encrypted` 추가, `office_legacy`·`unsupported_extension` 문구 갱신, `text` 제거.
- **부수 효과**: 문서 생성의 Markdown 저장도 이제 라이브러리에 '생성' 문서로 등록된다(텍스트 리더로 변환·검색). 처음 커밋(4fe082d)은
  이를 가정한 기존 테스트 `save_registers_generated_documents_in_the_library`를 깨뜨려 CI Rust 잡이 실패했고(로컬에서 `documents::`
  테스트만 돌린 탓), 테스트·경고 문구·모의 백엔드를 새 동작에 맞춰 고쳤다.
- **프런트엔드**: `SourceKind`에 `xlsx`/`pptx`/`text`, 종류 배지·아이콘(스프레드시트·프레젠테이션), 파일 선택 필터 확장, 미리보기의 분류 메모
  (`ImportItem.note`), 모의 백엔드 `kindOf`/사유/쪽수. 샘플 세트(`gen_fixtures`)에 `sample-ledger.xlsx`·`sample-plan.pptx`·`sample-notes.md`·
  CP949 `sample-ledger.csv` 추가(8개).
- **검증**: Rust 단위 테스트 — xlsx(참조·서식·생성 통합문서 읽기), pptx(생성 덱: 제목 순서·읽기 순서·불릿 상속·표 병합·그림·노트·슬라이드
  크기), text(디코딩 3종·CSV 따옴표·마크다운 구조·CP949 CSV·HTML/TXT), sniff(시그니처·컨테이너·CFB 변형·텍스트·잡음), `SourceKind`
  확장자 테스트; 실제 Excel/PowerPoint 산출물·CP949 메모장 파일은 사용자 PC(체크리스트 "입력 형식 확장").

### 뷰어 텍스트 직접 편집 (2026-11-09, 사용자 요청 "문서 내용 수정이 너무 어려움 — 글자를 드래그해 바로 수정")

- **UI** `features/library/PageTextPanel.tsx`(뷰어 텍스트 탭 분리): 출처 배지(텍스트 레이어/변환/OCR/직접 수정) + '수정'('텍스트 입력')
  버튼, 글자를 드래그하면 고정 선택 툴바(선택 문구 · **수정** · **필드 값으로 ▾**(문서의 추출 필드 목록, 현재 값 표시 →
  `pipeline_review_apply correct`) · **설명**(기존 Ctrl+Shift+E 경로)). 편집 모드는 textarea에 쪽 전체 텍스트를 넣고 드래그한 범위를
  `selectionOffsets`(Range → 문자 오프셋; `<mark>` 강조와 무관)로 그대로 선택해 두므로 바로 타이핑하면 그 글자만 바뀐다. 뷰어 키:
  선택 상태에서 Enter/F2 → 편집, Ctrl+Enter 저장, Esc 취소(IME 조합 중 무시).
- **저장 경로** `documents_set_page_text` → `repo::set_page_text`(`pages.text`, `text_source = 'manual'`, 빈 텍스트면 지움,
  `documents.updated_at`·`has_text_layer` 갱신) → `documents://changed` + `knowledge_bridge::auto_index`(텍스트 해시가 바뀌어 재청킹).
  보호: `replace_pages`(다시 렌더)는 manual 텍스트를 새 레이어보다 우선 보존, OCR 단계는 `ocr`처럼 `force`일 때만 덮어씀.
- **모의 백엔드** 동일 동작(저장 → 이벤트 → 자동 색인). i18n `viewer.text_edit.*`, `viewer.text_empty` 문구 갱신.
- **검증**: Rust `manual_page_text_survives_rerender_and_clears`, `manual_text_is_kept_unless_ocr_is_forced`; Playwright
  `documents.spec` "viewer text can be corrected in place"(드래그 선택 → Enter → 범위 선택 확인 → 치환 → Ctrl+Enter → 출처 배지 →
  검색 반영), `pipeline.spec` "a text selection becomes a field value". 설계 §3.7의 "직접 수정"으로 기록. → 같은 날 아래
  "페이지 이미지 위 글자 레이어"로 대체(텍스트 탭 자체가 사라짐).

### 페이지 이미지 위 글자 레이어 + 우측 패널 단순화 (2026-11-09, 사용자 요청 "이미지에서 글자로 인식된 부분을 드래그해 수정, 오른쪽은 AI 검색·이력만")

- **설계 편차(사용자 결정)**: §3.7.2 뷰어의 우측 탭 [텍스트][필드][표][Q&A][이력] 중 텍스트·필드·표 탭을 없애고 **[AI 검색](Q&A 탭
  개명)·[이력]** 만 남겼다. 필드 검토는 검토 큐 화면(Ctrl+?)·"필드 값으로"(이미지 선택)·이력 탭(복원)으로, 표 복사/CSV는 내보내기
  (`export target=table`)로 대신한다. `FieldsPanel`·`TablesPanel`·`PageTextPanel` 삭제, `viewer.fields.*`는 이력·검토 큐가 쓰는 3키만 유지.
- **코어 `documents/layout.rs`** (`PageLayout { version, source: layer|estimated, words: [{t,x,y,w,h,line,edited}] }`, 좌표는 회전 적용
  후 페이지 이미지의 0~1 비율): (a) **디지털 PDF·변환본** — PDFium 문자 박스(`render::pdf_page_chars`, loose → tight 폴백)를 공백·수평
  점프(0.5h)·수직 이동(0.5h)으로 단어로 묶고 줄을 나눈 뒤 회전을 비율 변환으로 적용 — PDFium 문자 박스는 회전 전 콘텐츠 좌표라
  표시 상자(CropBox∩MediaBox, 없으면 MediaBox; 원점 오프셋 포함)로 정규화하고 페이지 고유 `/Rotate` + 사용자 저장 회전을 합산
  (`render::PageChars { page_box, rotation, chars }`); (b) **스캔·사진(OCR)·수동
  전체 교체 텍스트** — 문자 좌표가 없으므로 페이지 JPEG(긴 변 1400px)의 행 잉크 프로파일로 줄 띠를 찾고(간격 ≤h/400 병합, 높이
  <h/300 제거) 텍스트 줄 수에 맞춰 병합/분할한 뒤 각 줄의 단어를 띠의 잉크 범위에 표시 폭(한글 1·라틴 0.55) 비례로 배치
  (`source=estimated`, 뷰어 배지 "위치 추정"); OCR 마크업(`<table>`·`<br>`·`<stamp>`)은 `text_lines`가 줄/셀로 환원. (c) **편집**
  `apply_edit(start..end, 치환문)` — 레이아웃 단어를 페이지 텍스트에서 **순서대로 찾아**(같은 단어 반복 시 올바른 n번째) 단어 수가 같으면
  제자리 치환(표 태그 등 사이 마크업 보존), 다르면 구간 치환(삭제는 인접 공백 흡수), 찾지 못하면 레이아웃 텍스트로 재생성; 새 단어는
  `edited=true`로 기존 박스를 상속(수 다르면 합집합 박스에 비례 배치). `add_words(bbox, 글)` — 빈 영역에 그린 박스에 새 단어를
  읽기 순서 위치(줄 중심 비교)로 삽입하고 텍스트에도 같은 자리에 넣음. `PageLayout::rotate(delta)`.
- **DB** 마이그레이션 0006 `pages.layout TEXT`(JSON; 계산 캐시 + 편집 영속). `replace_pages`는 manual/ocr 텍스트와 함께 layout 유지,
  `set_page_text`(전체 교체)·`set_page_ocr`·`reset_pipeline_state`(ocr)는 NULL로 초기화, `rotate_page`는 저장된 레이아웃을 함께 회전.
  `repo::page_layout_json / set_page_layout / set_page_text_and_layout`.
- **오케스트레이션 `import::ensure_layout`**(저장본 → PDF면 문자 박스 → 없거나 비면 이미지 추정 → 캐시), `edit_words`, `add_words`;
  셸 명령 `documents_page_layout`, `documents_edit_page_words(start,end,text)`, `documents_add_page_words(bbox,text)` (저장 →
  `documents://changed` + `auto_index`). `SCHEMA_VERSION = 6`.
- **UI** `features/library/PageTextLayer.tsx`(이미지 위 절대 배치 레이어; 단어 span은 투명 글자 + 선택/강조/수정됨 스타일, 구분용
  텍스트 노드로 `textContent`가 "단어 단어\n…"로 읽힘), `textLayer.ts`(선택 텍스트·연속성·히트 테스트(가장 가까운 줄/단어)·사각형
  교차·검색어→단어 강조). 상호작용: 단어 클릭/Shift+클릭/드래그(읽기 순서 범위) 또는 빈 곳 드래그(사각형 교차); 떠 있는 툴바(선택
  문구 · 텍스트 수정 · 필드 값으로 ▾ · 설명 · 복사 · 닫기); Enter/F2 → 선택 박스 위 인라인 입력(Enter 저장·Esc 취소), 더블클릭 =
  단어 바로 편집; Esc 선택 해제, Ctrl+C 복사; 빈 곳에 그린 사각형 → "글자 추가" 입력. 검색어(`?q=`)·인용(`?chunk=`) 강조는 단어
  `<mark>`로, 배너·출처 배지·"위치 추정" 배지는 캔버스 위 줄(`viewer-layer-bar`). 이미지가 없는 쪽(렌더러 없음)은 본문 텍스트를
  캔버스에 그대로 표시(`viewer-text`). 회전 저장 뒤 레이아웃 쿼리 무효화. `DocumentViewer` 탭 `TABS = ["qa","history"]`, 기본 탭 AI 검색,
  `?tab=` 은 두 값만 허용(프로젝트 결과 탭의 `tab=fields` 링크 제거).
- **모의 백엔드** `mock-layout.ts`(Rust와 같은 규칙의 TS 트윈: 모의 페이지 SVG의 회색 줄 위에 줄 배치, `applyEdit`/`addWords`),
  `documents_page_layout / edit_page_words / add_page_words`. i18n `viewer.tabs.qa` "AI 검색", `viewer.layer.*` 추가, 텍스트·필드·표
  탭 문구 제거(파리티 유지).
- **검증**: Rust `documents::layout` 8건(문자→단어/줄/회전, 공백 없는 수평 점프 분리, OCR 마크업 줄 환원, 잉크 띠 추정(줄 수
  병합/분할·빈 이미지), 제자리/구간/삭제 치환·n번째 반복·표 태그 보존·오래된 레이아웃 폴백·추가 삽입 순서·JSON 왕복),
  `render::pdf_renders_pages_text_and_thumbnails`(실 PDFium 문자 박스 → "Hello"/"DocuLens"), `import::page_layout_is_computed_edited_and_rotated`
  (계산·캐시 → 편집 → manual → 회전 → 추가 → 다시 렌더 보존), `projects` 마이그레이션 테스트 6으로 갱신; vitest `text-layer.test.ts` 8건;
  Playwright `documents.spec`(클릭/드래그 선택 → Enter → 입력값 확인 → 치환 → 수정됨 3단어 → 출처 배지 → Esc → 빈 곳 드래그 "글자 추가"
  → 검색 반영), `pipeline.spec`(OCR 쪽 "위치 추정" 배지, 검토는 큐에서 → 이력 탭 확인; 이미지 선택 → 필드 값으로 → 이력에 "스타벅스
  역삼점 → 역삼점"), `knowledge.spec`(검색/인용 강조는 레이어 `<mark>`, 설명은 이미지 선택으로). 실 PDF/스캔에서의 박스 정확도는 사용자 PC
  체크리스트 "페이지 이미지 위 글자 레이어".

### 사용자 PC 보고 수정 2차 (2026-11-09; 5건 모두)

- **1. 양식 화면 양식 등록·드롭** (보고: "양식 등록과 드래그 앤 드롭이 작동하지 않음"): 양식 화면 본문에는 드롭 대상이 없어
  스프링 로딩으로 화면이 바뀐 뒤 떨어뜨린 카드나 탐색기 파일이 아무 데도 닿지 않았다(사이드바 메뉴 항목과 열린 대화상자만 받음).
  `SchemasPage` 전체를 문서·파일 드롭 대상으로(`schemas-drop-hint`): 문서 → 그 문서로 "양식 등록" 대화상자 열기, 파일 → 가져온 뒤
  열기(대화상자가 열려 있으면 비활성). 필드 도출은 실엔진(tiny Qwen3.5, llama-server b12066 CPU 빌드)으로 `form_fields`·`nl_schema`
  메타스키마, 문서 초안 스키마, 프로젝트 규칙 스키마 문법이 모두 서버에 수용됨을 확인(`tests/real_engine.rs` 확장) — 문법 거부는
  원인이 아니었음. 남는 실패 유형(응답 잘림·출력 파서 500·엔진 오류)에 대비해 `forms::derive`가 텍스트가 있으면 **텍스트·표 규칙
  도출로 폴백**하고 초안 메모에 사유를 남기도록 바꿈(모델 없이도 초안은 항상 나온다). Playwright `dnd.spec` 2건 추가(스프링 로딩 →
  화면 본문 드롭 → 대화상자에 문서 선택; 파일 드롭 → 가져오기 → 등록 제안).
- **2. PPTX 변환 이미지 깨짐·비율** (보고: "PPT 로드 시 이미지 깨짐 + 가로세로 비율 안 맞음"): 슬라이드를 A4 세로 흐름 레이아웃으로
  찍고 그림은 폭만 지정하던 것이 원인. `DocModel::slides`(슬라이드별 절대 좌표 도형: 텍스트 상자(anchor·inset·autofit 비율·기본
  크기/색)·그림·표·선·자리표시(차트/SmartArt/OLE/EMF))를 추가하고 `pptx.rs`를 다시 썼다 — 레이아웃/마스터에서 자리표시자 좌표·기본
  글자 크기·색 상속(idx 우선), 테마 색(`schemeClr` + lumMod/lumOff/tint/shade, 마스터 clrMap·슬라이드 override), 그룹 변환
  (chOff/chExt 비례), 회전·반전, `srcRect` 자르기와 TIFF/BMP를 image 크레이트로 PNG 재인코딩, 배경색(slide→layout→master).
  HTML은 `@page { size: <슬라이드 크기> }` + `<section class="slide">` 1장=1쪽(노트가 있으면 노트 쪽 추가); 흐름 문서도 이제
  `page_mm`(DOCX/HWPX 페이지 설정)을 `@page`에 쓴다. `PdfPrinter::print_to_pdf(html, pdf, PageSize)`로 바뀌어 WebView2는
  짧은 변×긴 변 + `COREWEBVIEW2_PRINT_ORIENTATION_LANDSCAPE`, WebKitGTK는 사용자 지정 용지 + Landscape로 찍는다(Chromium은
  용지를 세로 기준으로 받고 가로 플래그로 돌림). 스텁 프린터도 용지 크기를 따르므로(`write_blank_pdf_sized`) PDFium 렌더 비율이
  테스트에서 확인된다. 읽기 순서 텍스트(`blocks`)는 그대로라 검색·양식 도출·텍스트 폴백은 영향 없음. `Run.color`·`Paragraph.level`
  필드 추가(다른 리더는 None/0).
- **검증**: `documents::convert::pptx` 4건(상속·색·자르기·그룹·z-order 7도형), `html` 2건(슬라이드 HTML·페이지 크기·색 주입 차단),
  `convert` 1건(스텁 프린터 가로 용지 960×540 pt 3쪽), 전체 `cargo test --workspace` 코어 273·셸 3·평가 15·통합 1, clippy·fmt,
  `dnd.spec` 7/7.
- **3. xls 등 미지원 형식** (보고: "xls 등 unknown 데이터타입 로드 불가"): `SourceKind`에 **Xls·Doc·Ppt·Rtf·Hml** 5종을 더했다
  (확장자 xls/xlsb/ods · doc/dot · ppt/pps/pot · rtf · hml/hwpml; 내용 감지는 CFB 스트림 이름(WordDocument / Workbook·Book /
  PowerPoint Document)·`{\rtf`·`<HWPML`·zip `xl/workbook.bin`). 리더: `convert/xls.rs` — calamine 0.38(MIT, `dates`)로
  시트 → 표(숨김 시트 생략, 날짜·기간·숫자 서식, 병합 제목 → 제목), 픽스처 `tests/fixtures/legacy/sample.xls`(xlwt로 만든 BIFF8,
  한글 시트 2장); `convert/doc.rs` — cfb 0.7(MIT, hwp 크레이트와 같은 버전)로 `WordDocument` FIB → `0Table/1Table` 피스 테이블
  (Unicode·8비트 cp949/cp1252) → 본문 ccpText → `\r` 문단·`0x07` 셀/행 → 표·`0x0C` 쪽 나눔·`0x0B` 줄바꿈·필드 코드 제거
  (`0x13/0x14/0x15`), Word 6/95는 fcMin..fcMac 폴백; 생성기 `write_sample_doc`로 자체 일관성 테스트; `convert/ppt.rs` —
  `PowerPoint Document` 레코드 트리에서 Slide 컨테이너의 TextHeaderAtom/TextCharsAtom/TextBytesAtom만 읽고(마스터·노트·핸드아웃 제외,
  증분 저장 중복 슬라이드 제거) 제목 유형은 머리글; `convert/rtf.rs` — 그룹/제어어 토크나이저(`\ucN`·`\uN?`·`\'hh`+`\ansicpg`,
  무시 목적지 목록, `\fldinst` 제거·`\fldrslt` 유지, `\cell`/`\row` → 표); `convert/hml.rs` — HWPML `BODY/SECTION/P/TEXT/CHAR`,
  `TABLE/ROW/CELL(ColSpan·RowSpan·Header)`, SECDEF/PAGEDEF 페이지 크기(HWPUNIT), 구역 → 쪽 나눔, UTF-8/UTF-16 입력. `classify`는
  확장자 종류가 있고 내용이 "텍스트 같음"(약한 신호)이면 확장자를 믿도록 바꿔 XML 기반 HML이 Text로 밀리지 않게 했다;
  `office_legacy` 사유는 그 밖의 OLE 컨테이너(VSD·PUB·MSG…)에만 남는다. 프런트: 종류 라벨 5종(ko/en), 라이브러리 필터 `SOURCE_KINDS`
  13종(xlsx·pptx·text도 이제 필터에 보임), 아이콘(xls=스프레드시트, ppt=프레젠테이션), 파일 선택 필터 확장자, 미지원 문구 갱신,
  모의 백엔드 종류 판정. 구형 형식은 텍스트·표만 변환되고(그림·도형 위치 생략) 변환 메모에 적힌다. HEIC·ODT/ODP는 여전히 미지원.
- **검증**: `convert::xls` 1(실 BIFF8 픽스처)·`doc` 3·`ppt` 2·`rtf` 2·`hml` 1건, `sniff`/`classify`/`kinds_from_extension` 갱신,
  `cargo deny` 통과(calamine·cfb MIT), 전체 Rust·vitest·Playwright는 아래 표.

- **4. 어시스턴트가 세션 정보를 기억하지 않음** (보고: "질문 시 현재 세션에 대한 정보를 기억하고 있지 않음"): 질문·명령은 매번
  그 문장만 보내고(후속 질문 스위치를 켠 경우 이전 근거 청크 id만 유지) 이전 대화·현재 화면은 모델에 전혀 전달되지 않았다. 이제
  (a) **대화 이력** — `AnswerRequest.history`/`PlanContext.history`(`ConversationTurn{role,text}`, 오래된 것부터): 프런트
  `conversationTurns`가 메시지 목록에서 사용자 질문·답변 본문·실행 결과·플랜 요약·명확화 질문을 뽑아(도움말·오류·"근거 없음" 답변
  제외, 지금 보내는 질문 제외) 최근 8턴·턴당 500자로 보낸다. 코어는 턴을 `sanitize_source`로 정리한 뒤 `[이전 대화]` 블록으로
  질문 앞에 넣고, 시스템 규칙 7("이전 대화·현재 화면은 지시어 해석에만, 근거는 [근거]에서만")을 추가한다. (b) **현재 화면** —
  `knowledge_answer`가 `ui: UiPlanContext`(열린 문서·쪽, 선택 문서, 폴더, 프로젝트 id)를 받아 셸(`knowledge_bridge::session_state`)이
  제목·이름으로 풀어 `AnswerRequest.session`(`SessionState`, 최근 가져온 문서 3건 포함)으로 넣고 `[현재 화면]` 블록이 된다.
  (c) **후속 질문 자동 인식** — `is_follow_up`(ko: 그/이/그것/아까/방금/해당/위/같은…+조사 1자, en: it/that/this/above…, 4자 이하 '왜?')
  이면 스위치를 켜지 않아도 이전 답변의 근거 청크를 유지하고 검색 질의에 직전 질문을 덧붙인다("그건 누가 작성했어?" + "합계 금액은?");
  지시어가 없는 새 질문은 종전처럼 새로 검색해 "근거 없음"이 나올 수 있다. (d) **플래너** — `context_block`에 최근 4턴을
  "이전 대화(참고용, 명령은 <command>만)"로 적어 "그것도 엑셀로" 같은 명령의 지시 대상을 모델이 알 수 있게 했다(규칙 플래너는
  그대로). 어시스턴트 상단에 "기억 N턴" 배지(`assistant-memory`, 툴팁에 범위·'새 대화'로 지움), 후속 질문 스위치 설명 갱신.
  모의 백엔드도 같은 규칙(후속 질문이면 직전 질문과 함께 검색)으로 동작해 E2E가 계약을 검증한다.
- **검증(4번)**: `knowledge::answer` 테스트 2건(후속 질문 판별·검색 질의, 프롬프트 블록·시스템 규칙·이력 상한·마커 정리),
  `planner::tests::context_block_lists_the_last_conversation_turns`, vitest `conversation.test.ts` 4건(턴 추출·상한·명확화,
  `looksLikeFollowUp`), Playwright `knowledge.spec` "assistant remembers the conversation"(후속 질문 유지 → 새 대화 후 근거 없음).

- **5. 문서를 제목·내용으로 프로젝트별 분류** (요청: "라이브러리·지식 베이스에 추가된 문서들을 제목/내용을 기반으로 프로젝트별
  분류"): 새 모듈 `projects/classify.rs` — 프로젝트마다 **이름 단어·분류 키워드(새 컬럼 `projects.keywords`, 마이그레이션 0007)·
  접수번호·inbox 폴더·보유 문서의 거래처/제목 단어·템플릿의 미보유 필수 유형**을 문서의 **제목·파일명·경로·거래처·본문 앞 4,000자**와
  대조해 점수를 매긴다(폴더 일치 4 · 접수번호 3/본문 2.5 · 키워드 2/본문 1.2 · 이름 단어 1.5/0.8 · 같은 거래처 2 · 거래처가 이름에 1.5 ·
  비슷한 제목 0.5 · 미보유 필수 유형 0.7; 신뢰도 = 점수/(점수+2), 2위가 1위의 60% 이상이면 ×0.6; 0.45 이상이면 제안, 아니면
  미결정 + 가장 가까운 후보 표시). **모델 보완**(`use_model`)은 불러온 모델이 있을 때 미결정·애매(0.65 미만) 문서를 최대 20건까지
  후보 프로젝트 id enum 문법(`project_classify`)으로 묻고 0.5 이상이면 반영(출처 `model`). 아무것도 자동 배정하지 않는다:
  `classify`(미리보기) → 사용자가 행별 프로젝트를 바꾸거나 제외 → `apply_classification`(한 트랜잭션, 중복 무시). 대상은 명시한
  문서 id, 아니면 프로젝트 미배정 문서 전체(`include_assigned`로 전체). 셸 `project_classify_preview`/`project_classify_apply`
  (모델 패스는 define_rule처럼 `dialog_model` + 인터랙티브 게이트). 프런트: 프로젝트 화면 "자동 분류" 버튼과 라이브러리 선택 바
  "프로젝트로 분류"(`/projects?classify=<ids>`) → `ClassifyDialog`(범위 배지, "모델로 보완" 스위치(불러온 모델 있을 때만), 미리보기
  표: 문서·제안 프로젝트(바꾸기/배정 안 함)·신뢰도·근거·다른 후보, 적용(N)); 새 프로젝트/편집 대화상자에 **분류 키워드** 입력(쉼표 구분).
  모의 백엔드가 같은 점수 규칙을 구현해 E2E가 계약을 검증한다. 어시스턴트 op는 추가하지 않았다(라이브러리·프로젝트 화면에서 실행;
  필요하면 `classify_documents` op로 확장 가능).
- **검증(5번)**: `projects::tests` 3건(키워드 정규화·부분 갱신, 점수 규칙(접수번호·키워드·이름·같은 거래처·미보유 필수 유형)·
  이미 속한 프로젝트·적용·재실행, 스크립트 모델로 미결정 문서 결정·"none"), Playwright `projects.spec` "auto-classify"(라이브러리 선택 →
  대화상자 → 제안/미결정 → 수동 지정 → 적용 → 카드 문서 수; 프로젝트 화면에서 미배정만 다시).

### 사용자 PC 보고 수정 3차 (2026-11-09; 표 내부 글자 인식 · 수정 표시)

- **6. 표 내부 글자 인식** (보고: "문서 라이브러리에서 표 내부의 글자가 정상적으로 인식되지 않음"): 원인 두 가지. (a) PDF 텍스트
  레이어(디지털 PDF·변환본)의 `layout::from_chars`가 줄 번호를 PDF 글자 순서대로 매겼는데 표는 **셀 단위**로 글자가 나오므로
  (번/호/품목/수/량…) 한 행이 여러 줄로 쪼개지고(LibreOffice로 만든 표 PDF 덤프에서 머리행이 6줄) 줄바꿈된 셀의 조각이 서로 다른
  줄 번호를 받았다 → 드래그 선택 텍스트가 줄바꿈으로 조각나고 셀 여러 개를 고르면 '텍스트 수정'이 비활성(비연속). 이제 **세그먼트
  (한 기준선·단어 간격 이내의 연속 런) → 위아래로 쌓인 세그먼트 체인(줄바꿈된 셀: ≤4줄, 옆에 0.6em 이상 떨어진 이웃이 있고 마지막
  줄이 오른쪽으로 이어지지 않을 때만) → 세로 범위가 겹치는 세그먼트를 같은 행**으로 묶는다(`assign_rows`). 단어 순서(= `pages.text`
  순서)는 그대로이고 `line`만 시각적 행이 된다. 제목+문단, 양쪽 정렬의 넓은 간격, 2단 문서는 종전처럼 줄마다 행. 저장된 v1 레이아웃은
  로드 시 박스로 행을 다시 매겨 v2로 올려 저장(수정 표시 유지). (b) OCR/사진의 위치 추정(`estimate`)은 표 **괘선**(세로선이 모든 행을
  잇는 잉크) 때문에 표 전체가 한 밴드가 되고 행의 단어를 표 전체 폭에 비례 배치해 셀과 어긋났다 → 긴 가로/세로 잉크 런(폭의 1/10·
  높이의 1/20 이상)을 지운 뒤 밴드를 찾고, `<td>` 셀 경계를 보존(`line_cells`)해 밴드 안의 잉크 덩어리 수와 맞으면 **셀별로 배치**.
  표 PDF를 렌더한 이미지로 측정: 단어 중심이 실제 박스 안에 든 비율 63% → 89%, 평균 중심 거리 0.019 → 0.005(페이지 비율). 진단 덤프:
  `DOCULENS_PDFIUM=… DOCULENS_LAYOUT_DUMP=<pdf> [DOCULENS_LAYOUT_DUMP_TEXT=<ocr 텍스트> DOCULENS_LAYOUT_DUMP_OUT=<png>]
  cargo test -p doculens-core --lib -- --ignored dump_layout --nocapture`(문자 박스·행·추정 대 실제 비교·주석 이미지).
- **7. 수정 후 다시 렌더해도 미반영** (보고: "수정한 뒤 다시 렌더링 해도 변경사항이 적용되지 않음"): 코어는 고친 텍스트·레이아웃을
  `text_source=manual`로 저장하고 다시 렌더(`replace_pages`)·회전 저장·재처리(강제 아님)에서 유지함을 코드·테스트로 재확인 — 데이터는
  바뀌어 있었다. 문제는 **표시**: 고친 단어가 투명 글자 + 점선 밑줄뿐이어서 이미지의 원래 글자가 그대로 보였고 다시 렌더해도 이미지는
  당연히 그대로라 "적용되지 않음"으로 보였다. 이제 `edited` 단어는 **흰 바탕에 파란 글자로 이미지 위에 그려서**(박스 높이·폭에 맞춘
  글꼴 크기 `fitFontSize`) 수정이 페이지에서 바로 보인다(툴팁: 원본 이미지는 바뀌지 않음). 레이어 안내문·체크리스트 갱신.
- **검증**: `documents::layout` 15건(표 행·제목/문단·양쪽 정렬·2단·v1 업그레이드·셀 분리·괘선 제거·셀 배치), `textLayer.test.ts`
  4건, `documents.spec`에 수정 글자 색 단언; 전체 결과는 아래 표.

### 어시스턴트 해석 방식 변경 — 모델 우선 + 되묻기·취소 (2026-11-09; 사용자 결정)

- **배경**: 어시스턴트가 "질문을 제대로 이해하지 못한다"는 보고. 원인은 **규칙 우선** 라우팅이었다. 문장 규칙 24종이 단어 포함 여부로 먼저
  해석하고 모델은 어떤 규칙도 맞지 않을 때만 호출됐으므로, 질문 안의 명령 단어("합계", "찾아")가 집계·검색 카드를 만들고 규칙이
  틀려도 모델이 바로잡을 기회가 없었다.
- **결정**: 슬래시(`/`) 명령과 **되묻기·취소** 두 가지만 추론 없이 규칙으로 최우선 처리하고, 나머지 입력은 **무조건 모델 추론**으로 해석한다.
- **코어**: `planner::PlanMode { ModelFirst(기본), RulesFirst }`. `llm::plan(deps, text, ctx, mode)` — ModelFirst는 `/`로 시작하는
  문장만 프리라우터로 보내고 나머지는 모델(검증 + 재질의 1회)로 간다. 모델이 없으면 "플래너 모델이 로드되지 않아 해석할 수 없습니다 …"
  명확화(`no_model`)를 내고 **키워드 추측을 하지 않는다**. RulesFirst는 종전 경로(프리라우터 → 규칙 → 모델)로 평가 하네스와 선택 모드에만
  쓰인다. `prerouter::is_cancel`(취소·그만·됐어·cancel·stop… 단독 어구). 모든 모델 요청에 `cache_prompt: true`를 명시해 시스템
  프롬프트·op 표·예시 접두부의 KV 캐시를 재사용한다(두 번째 호출부터 새 문장과 답만 처리).
- **셸**: `UiPlanContext.routing`(기본 model_first). ModelFirst면 모델을 필요 시 로드해 인터랙티브 게이트로 호출하고, 모델을 얻지 못하면
  `no_model` 명확화를 돌려준다. RulesFirst는 종전처럼 규칙 결과가 없을 때만 모델.
- **프런트**: 입력 순서 = ① 대기 중인 질문·플랜이 있을 때 단독 "취소"(`isCancelCommand`) → 즉시 취소 카드, ② `/` 명령 → 프리라우터 실행,
  ③ 나머지 → 모델(대화 이력·화면·routing 포함). 문장형 프리라우터(“다크 테마로 바꿔”)는 더 이상 쓰지 않고 모델이 UI op로 처리한다.
  명확화 카드에 **답변 입력란 + 되묻기 + 취소** 버튼(`clarify-reply`/`clarify-ask-back`/`clarify-cancel`, 상태 배지 답변함/취소됨,
  Esc로 취소). 되묻기는 원문 + 어시스턴트 질문 + 답변을 묶어(`askBackText`) 모델에 다시 보내고 답변은 사용자 턴으로 남는다. 플랜
  카드에도 되묻기 입력란(`plan-ask-back`): 답하면 그 카드는 닫히고 재해석 카드가 뒤따른다. 상단 "해석: 모델 / 규칙(구형)" 선택
  (`assistant-routing`, localStorage 보존)은 CPU 전용 PC용 탈출구다. 모의 백엔드는 모델 우선 흐름을 흉내 낸다(문장 규칙 파서의 해석을
  모델 출처로 표시, 못 읽으면 명확화, 되묻기 합성문은 답변을 읽음; `doculens.mock.planner_model = "off"`로 모델 없음 상황).
- **지연·한계**: GPU는 명령당 1~3초. CPU 전용은 프롬프트 캐시가 있어도 명령당 10초 안팎이라 "규칙(구형)"으로 바꾸는 편이 낫다.
  실제 Qwen3.5-4B의 모델 경로 정확도는 이 환경에서 측정할 수 없고(가중치 없음) 사용자 PC에서 `doculens-eval planner --engine …`로
  측정해야 한다. CI의 플래너 골든셋 게이트는 규칙 경로(`RulesFirst`)를 계속 검사한다.
- **검증**: `planner::tests::model_first_mode_sends_every_sentence_but_slash_commands_to_the_model`(스크립트 모델: 규칙이 잡을 문장도
  모델로, 슬래시는 프리라우터, 모델 없음 → no_model 명확화, RulesFirst는 규칙), `prerouter::tests::bare_cancel_words_are_recognised`,
  Playwright `planner.spec` "되묻기 replies with the question's context; 취소 (button or typed) …" 추가, 기존 어시스턴트 E2E 전부 통과.

### 0.2.0 릴리스 (2026-11-09; 사용자 요청 "0.2.0 릴리즈 만들어줘")

- **버전**: `package.json`/`package-lock.json`, `src-tauri/tauri.conf.json`(설치파일 이름과 Release 태그의 출처), Cargo 워크스페이스 버전·
  `Cargo.lock`, SBOM 루트 컴포넌트, 모의 백엔드 앱 버전, RUN-GUIDE 예시 파일명을 0.2.0으로(3d55db8). 고지 파일은 변경 없음.
- **순서**: 3d55db8 CI 7개 잡 통과 확인 → Full(publish=true) 먼저 디스패치 → Full이 "CUDA 팩 내려받기" 단계를 지난 뒤 Lite(publish=true).
  Full은 CUDA 팩을 `v<버전>` 릴리스에서 찾고 없으면 **최신 릴리스**로 내려가므로(v0.1.0, 같은 엔진 태그 b12066), Lite가 먼저 v0.2.0을
  만들어 버리면 Full의 팩 다운로드가 실패한다. 다음 릴리스에서도 Full → Lite 순서를 지키거나 CUDA 팩 워크플로를 새 버전으로 먼저 돌릴 것.
- **결과**: Release `v0.2.0`(워크플로가 `--generate-notes`로 생성 — PR이 없어 본문은 비교 링크 한 줄) — Full 1,156,786,994 B ·
  Lite 50,579,259 B · `.sha256` 2 · 고지 · SBOM. CUDA 팩 zip은 v0.1.0 자산 그대로(복사 안 함).
- **릴리스 노트**: 한국어 노트는 `docs/releases/v0.2.0.md`. 이 세션은 릴리스를 만들거나 고칠 수 없고(REST PATCH → HTTP 403 "not permitted
  for this session type"), 본문을 바꾸는 새 워크플로 "Release notes (manual)"의 디스패치도 권한 분류기가 거부했다. **본문 적용은 사용자
  작업**: Actions → "Release notes (manual)" → tag `v0.2.0`(또는 릴리스 편집 화면에 파일 내용 붙여넣기). 설치파일·CUDA 팩 워크플로는
  앞으로 릴리스를 *만들* 때 `docs/releases/<tag>.md`가 있으면 그 파일을 쓰고 없으면 `--generate-notes`로 돌아간다.
- **권한**: 이전 세션에서 거부됐던 `publish=true` 디스패치(설치파일 워크플로)는 이번에는 통과했다(사용자가 릴리스를 명시적으로 요청).

### 사용자 PC 보고 수정 4차 (2026-11-10; 요약 컨텍스트 초과 400 · 불필요한 RAG 호출)

- **보고**: "제공된 문장을 다듬어 정리" 요청이 **요약(문서별, 55건)** 플랜으로 해석돼 실행됐고, 실행 중 `HTTP 400: request (28107 tokens)
  exceeds the available context size (16384 tokens)`로 실패. 두 가지 결함이 겹친 사례.
- **원인 1 — 근거 예산이 모델 컨텍스트와 무관**: 지식 응답(질문·설명·요약·비교)의 근거 예산은 설정 `knowledge.context_tokens`(1,000~24,000,
  기본 6,000)만 따랐고 로드된 모델의 n_ctx(16K)는 보지 않았다. 게다가 청커의 토큰 추정(한글 0.75 토큰/자)이 OCR 업무 문서에서 서버
  토크나이저보다 약 25 % 낮아 설정 24K가 실제 28K가 됐고, 400 응답에 재시도가 없었다.
- **수정 1**: `AnswerDeps.context_size`(셸이 `engine.status().model(..).ctx`를 넣음 — `knowledge_bridge::answer`, `planner::exec`의 answer·
  summarize·explain·compare) → `evidence_budget` = min(설정, ctx − 응답 예산(1,200/1,500) − 스캐폴딩 700 − 질문·선택·이전 대화·현재 화면
  블록 비용), 프롬프트 비용은 추정 × 1.3 + 12. 서버가 그래도 거부하면(`engine::client::context_overflow`, 추출기와 공용) answer는 근거를
  절반씩 줄여, summarize는 그룹을 반으로 나누고(단일 청크는 뒷부분 절단) 병합 단계는 부분 요약을 다음 단계로 미뤄, compare는 마지막
  근거를 빼며 재시도한다. 테스트: `LimitedChat`(토큰 상한을 넘는 프롬프트를 400으로 거부하는 더블)로 10쪽 규정 요약이 2.6K 슬롯에서
  모두 슬롯 이하 요청으로 쪼개지고, 컨텍스트를 모를 때 거부당한 그룹이 분할돼 모든 청크가 요약에 닿는 것, 질문이 근거를 줄여 성공하는 것.
- **원인 2 — 붙여 넣은 글을 라이브러리 작업으로 해석**: 플래너 프롬프트에 "사용자가 글을 함께 보내며 다듬기·교정·번역을 요청"하는 경우의
  규칙이 없어 모델이 `summarize(each_doc)` + 빈 scope(= 전체 문서)를 냈다. summarize는 라이브러리 문서용이라 문서마다 검색·요약이 돈다.
- **수정 2**: `Op::Answer.knowledge: Option<bool>`(false = 검색 없이 보낸 글만으로 답), 플래너 규칙 7 확장 + few-shot("다음 문장을 다듬어
  정리해줘: …" → answer, knowledge=false), `AnswerRequest.retrieve`(false → 검색·인용 없음, 전용 시스템 프롬프트 "보낸 글만 재료로, 사실
  추가 금지"). **`planner::guard::prefer_text_only_answer`**: 모델이 낸 플랜이 폴더·내용 필터 없는 지식 op(summarize/answer/explain/compare/
  search)뿐이고 문장이 본문을 담고 있으면(다듬기 동사 + 콜론/줄바꿈/따옴표 + 60자 이상, 또는 본문 뒤 지시문) answer{knowledge:false}로
  재지정하고 경고 `text_only_answer`를 카드에 남긴다. 날짜만 채워진 필터(본문 속 "12월말"을 날짜 해석기가 채움)는 명시 범위로 보지 않는다.
  모델 우선 원칙은 그대로이며 이 규칙은 모델 결과에 대한 사후 안전장치다. 프런트: `handlePlan`이 `retrieve=false`로 `knowledge_answer`를
  부르고 답변 카드에 "검색 없이 답변" 배지(`answer-text-only`), 플랜 설명에 "문서 검색 없이". 모의 백엔드도 같은 흐름(`carried-text.ts`).
- **남은 것**: 55건 문서별 요약 같은 대량 작업은 플랜 카드의 "대상 문서 n건"을 보고 승인하는 현재 흐름 그대로다(명확화로 바꾸지 않음).
  실제 Qwen3.5-4B가 규칙 7을 얼마나 따르는지는 사용자 PC에서 확인(안전장치가 받쳐 줌).

### UI 배율 대응 — 고정 px 상자 제거 (2026-11-09; 사용자 보고 "UI 배율이 높아질 때 글자 잘림", A안 결정)

- **증상·원인**: 설정 > 일반 > UI 배율(90~150 %, Ctrl+Shift+=/−)은 루트 글꼴 크기(rem)만 키운다. Tailwind 유틸리티는 rem이라 함께
  커지지만 네 곳이 CSS px로 상자 크기를 고정해 두어(라이브러리 카드 200×268, 목록 행 44 px + px 열 너비, 검토 큐 행 추정 72 px,
  뷰어 썸네일 행 128 px) 150 %에서는 썸네일(11 rem)만으로 카드가 차서 제목·쪽수·배지가 잘렸다. Windows 디스플레이 배율은 원인이
  아니다(WebView2는 px와 rem을 같은 비율로 키움). 100 %에서도 배지가 두 줄인 카드(견적서.hwpx)는 제목이 이미 잘리고 있었다.
- **점검**: 모의 백엔드로 15개 화면 × (1280×800 100 %/150 %, 960×600 150 %, 1920×1080 150 %)를 Playwright 스크립트로 자동 점검
  (overflow 숨김 + 넘침, 고정 높이 넘침, 문서 가로·세로 스크롤). 깨지는 곳은 라이브러리 카드·목록, 뷰어 썸네일, 라이브러리 필터
  줄(가로 스크롤), 최소 창의 상단바 Ctrl+K 세로 줄바꿈·상태바 두 줄 잘림뿐이었고 나머지 11개 화면은 줄바꿈만 늘었다.
- **결정(A안)**: 전면 반응형 전환 대신 "글자만 안 잘리게" — px 고정값을 rem·측정 기반으로 바꾼다. 사이드바 자동 접힘·툴바 메뉴
  접기·우측 패널 쌓기 같은 반응형 전환(B안)은 보류(최소 창 960 px의 데스크톱 앱이라 필요한 조건이 "최소 창 + 125 % 이상"뿐).
- **수정**: `src/app/hooks/useRootFontSize.ts`(`useRootFontSize` = 16 px × 배율, `remPx`) — px로 계산해야 하는 코드(가상 스크롤 추정치,
  열 개수)가 rem에서 px를 구한다. 라이브러리 카드 폭 12.5 rem·높이 자동(행을 `measureElement`로 측정, 배지가 두 줄이면 카드가 자람;
  배율이 바뀌면 `virtualizer.measure()`), 목록 열을 rem 트랙으로 + 행 `measureElement`(최소 2.75 rem, 머리행 2.75 rem) + 폭이 모자라면
  목록 안에서만 가로 스크롤(`min-w-[58rem]`, 컨테이너 `overflow-auto`), 필터 컨트롤 줄 `flex-wrap`(화면 전체가 가로 스크롤되던 것),
  그리드·목록 `min-h-40`(선택 바·집계 바가 세로 공간을 다 먹어도 0이 되지 않고 메인 영역이 스크롤됨); 검토 큐 행 추정 4.5 rem(측정은
  기존); 뷰어 썸네일은 래퍼(`pb-2`가 간격)를 측정 + 추정 8.75 rem; 상단바 `kbd` nowrap; 상태바 `min-h-7 flex-wrap` + 항목 nowrap(좁으면
  두 줄로 늘고 잘리지 않음); `text-[10px]`/`[11px]` 25곳 → `text-[0.625rem]`/`[0.6875rem]`, `min-h-[60px]`·`grid-cols-[…120px…]`·
  `max-w-[420px]`·`w-[640px]` → rem. 남은 px는 텍스트 레이어 박스의 `rounded-[2px]`·`text-[0px]`와 포인터 임계값뿐.
- **남는 제약(의도)**: 150 %에 1280 px 폭 창이면 라이브러리 필터가 2~3줄로 줄바꿈되고 목록은 가로 스크롤이 생긴다. 최소 창(960×600)에
  150 %면 상단바 항목이 가로로 넘쳐 화면이 가로 스크롤된다(유효 폭 640 px급) — B안 범위. 150 %는 1600 px 이상 폭에서 쓰는 편이 낫다.
- **검증**: Playwright `scale.spec.ts` 3건 추가 — 150 %(슬라이더 End)에서 카드(제목·쪽수·배지 상자 안, 행 겹침 없음)·목록 행·썸네일
  1/2쪽·검토 큐 행·상단바·상태바 잘림 없음, 100 %에서 배지 두 줄 카드의 제목·쪽수 표시, 슬라이더 최대 150 %. 자동 점검 재실행에서
  실제 잘림 0(남은 항목은 sr-only와 모의 텍스트 레이어 박스뿐, 960×600 150 %의 가로 넘침만 그대로).


### 0.3.0 1차 — 추출 스키마 내장 양식을 SW 개발 문서로 (2026-11-10; 사용자 지시 "순차적으로 실행", 계획 `docs/plan/v0.3.0.md` §3, 결정 1·2 "구형은 숨김")

- **코어 `pipeline/schema.rs`**: `software_builtins()` 13종 — rfp(제안요청서)·proposal·sow(과업지시서)·srs(요구사항 정의서)·
  dev_contract(소프트웨어 개발 계약서)·wbs·progress_report·meeting_minutes·test_report·change_request·acceptance(검수확인서)·
  estimate(견적서, 인력·MM 기준)·general — 와 `legacy_builtins()` 6종(세금계산서·계산서·영수증·사업자등록증·계약서·신청서, `hidden: true`)을
  `builtins()`가 합친다. `builtin_doc_types()`는 보이는 유형만, `builtin(key)`, `BUILTIN_VERSION = 2`. 유형 별칭표
  `doc_type_aliases(slug)` 하나를 플래너 `doc_type()`·프로젝트 `type_aliases`·모의 `src/lib/doc-type-aliases.ts`가 함께 쓴다.
  검증 규칙 2종 추가: `percent_range`(`*_percent`·`*_rate`·표 열 `progress` 0~100), `test_counts_sum`(총 케이스 = 통과+실패+보류).
- **저장·승격**: `repo::ensure_builtins`가 숨긴 양식을 `enabled = 0`으로 넣고, 버전 갱신 때 사용자가 편집하지 않은 구형 행을
  `enabled = 0, is_default = 0`으로 바꾼다(편집한 행은 그대로). `enable_legacy_builtins(conn)`은 테스트·탈출구. `extract.rs` 승격 열:
  거래처 ← contractor/proposer/supplier/issuer/client.name…, 문서 날짜 ← contract/meeting/report/test/acceptance/request_date,
  합계 ← price/budget/impact_cost; `aggregate::default_total`·`projects/relations.rs` 키·`projects/check.rs` 별칭·`forms.rs`
  슬러그 표(회의록→meeting_minutes, 계약서→dev_contract, 견적서→estimate)도 같은 기준.
- **프로젝트 템플릿** `builtin_templates()` 3종 — 외주 개발 사업(필수 8종, 규칙 6: 계약금액 = 제안 금액, ≤ 사업 예산, 과업 금액 ±5 %,
  계약서·검수확인서 서명, 테스트 총 케이스), 유지보수(3종, 규칙 3), 내부 개발(3종, 규칙 3) — 와 `legacy_templates()`(DB에 넣지 않음;
  테스트만 `ensure_templates(conn, &legacy_templates())`).
- **플래너 규칙(`rules.rs`)**: 별칭으로 유형 인식, `total_field` 우선순위(grand_total → total → amount → price → budget → 합계금액·총액
  라벨 → 아무 total), `fields_mentioned`는 유형 이름 안의 잘린 라벨("계약명"→"계약" ⊂ "개발 계약서")을 필드로 세지 않음, 검색어에서
  별칭도 제거("'위약금' 들어간 계약서" → 위약금), "이 견적서 / this contract"는 열린 문서 참조(집계가 아니라 답변·요약, `doc_ids`),
  최대·최소 트리거에 "가장 큰/작은/비싼/싼". 모델 프롬프트의 few-shot은 4차에서 이미 SW 문서로 바꿨다.
- **골든셋**: `eval/golden/planner.json` 31건을 SW 문구로(영수증 → 견적서, 세금계산서 → 견적서·개발 계약서, 사업자등록증 → 요구사항
  정의서·테스트 결과서, quotation → estimate), `planner-context.json` 유형 13종·스키마 3종(estimate·dev_contract·srs),
  `eval/baselines/planner-rules.json` 재생성 — 99/100(미달은 이전과 같은 mx-10 규칙 되묻기 1건).
- **프런트**: 모의 백엔드(`mock-pipeline.ts` SW 12종 정의·파일명 분류 규칙·OCR 텍스트·추출 템플릿·승격 열, 구형은 `hidden`;
  `mock-projects.ts` 템플릿 3종, NL 규칙 파서가 별칭을 쓰고 2글자 별칭("계약"·"견적")은 다른 글자에 붙어 있으면 무시, 조사는 뒤에 공백·쉼표가
  올 때만 떼어냄("과업 금액"·"이름" 보존); `mock-planner.ts` 별칭·`totalField`), `doc-type-aliases.ts`, i18n `documents.doc_types`
  12종 추가·`projects.templates.builtin` 3종·`schemas.list.hidden`, `labels.ts`. 양식 화면: 꺼진 내장 양식은 "숨긴 기본 양식 N개"
  접기(`schemas-hidden-builtins`) 안에, 행마다 `data-schema-enabled`.
- **E2E**: 9개 스펙을 SW 샘플 문서(견적서-1p.jpg·개발계약서-2p.pdf·요구사항정의서-v1.2.pdf·WBS-한빛포털.xlsx·테스트결과서-1.2.pdf·
  회의록-3p.pdf)로 재작성. 프로젝트 스펙은 내부 개발 템플릿(규칙 3 + 자연어 2 → 통과 4·불일치 1, 관계 kind `name`), 양식 등록
  스펙은 견적서 양식이 내장 견적서와 같은 유형(`estimate`)의 두 번째 스키마가 되는 흐름(재분류 배너 없음).
- **문서**: RUN-GUIDE 예시 명령·템플릿 이름, user-pc-checklist "0.3.0 1차" 절, README 첫 문단, CLAUDE.md 레이아웃, 계획 §3 진행 표시.
- 사용자 결정대로 구형 양식·템플릿은 삭제하지 않고 숨겼다. 기존 작업 공간은 `BUILTIN_VERSION` 갱신으로 열 때 자동 전환된다
  (사용자 편집 구형 양식은 유지).


### 0.3.0 2차 — 문서 생성 내장 템플릿을 SW 개발 문서로 (2026-11-10; 계획 §1, 결정 1·2 "목록 그대로, 구형은 숨김")

- **코어 `generate/mod.rs`**: `TemplateKind` 19종 — SW 12종(rfp·proposal·sow·srs·dev_contract·wbs·progress_report·meeting_minutes·
  test_report·change_request·acceptance·estimate) + supplement_request + 숨긴 구형 6종(official_letter·quotation·transaction_statement·
  contract·report·minutes; `hidden()`, `visible()`). 생성 slug = 추출 doc_type slug(테스트가 `pipeline::schema::builtin(slug)`로 확인).
  `templates.rs`: `list()`(보이는 13) / `list_all()`(19) / `get()`은 숨김 포함; `match_keywords()`는 보이는 템플릿만, `match_keywords_any()`는
  명시 형식 참조용("A4 공문 형식" → 공문); 필드 키는 추출 스키마와 동일(`project_name`·`amount`·`supplier.name`·견적 `items[]`
  역할·등급·투입 MM·단가·공급가액); 절 목록은 계획 §1 표. 견적서는 인력·MM 기준으로 재정의해 포함(구형 품목 견적서는 숨김) —
  어시스턴트·RUN-GUIDE의 견적서 예시와 추출 `estimate` 어휘를 유지하기 위한 결정.
- **흐름·렌더·검증**: `resolve_spec`은 명시 `form_ref`면 숨긴 템플릿도 이름·키워드로 찾고, 내용 문장은 보이는 템플릿만 보며, 못 찾으면
  종전처럼 `report`(숨김이지만 범용 레이아웃)로 떨어진다. 렌더의 갑·을 인터리브·서명 블록을 개발 계약서(client/contractor)·검수확인서에도
  적용, 견적서는 표 우선·공급자 서명. 검증은 `months`(투입 MM) 열을 수량으로 인정(MM×단가=공급가액, 합계·세액 자동 채움, 메시지에 열 이름).
  시스템 프롬프트의 문서 목록 갱신.
- **플래너**: `TEMPLATE_WORDS`를 SW 13종 우선으로(진척·테스트·완료 보고서가 범용 보고서보다 먼저), 공문·보고서는 자기 단어로만 도달;
  `aggregate_first`는 "문장의 유형 ≠ 선택된 템플릿"일 때로("지난달 견적서 집계 결과로 보고서 초안"); few-shot 견적서 예시를 estimate로;
  `planner::validate`는 `templates_all()`로 숨긴 이름도 경고 없이 통과.
- **프런트**: `TemplateKind`·`TEMPLATE_KINDS`(보이는 13)·`HIDDEN_TEMPLATE_KINDS`, i18n `generate.templates.*` 12종 추가(구형 키 유지),
  `describePlan` 템플릿·프로젝트 템플릿 키, 모의 플래너 `DRAFT_TEMPLATES`(구체 → 범용 순)·`PROJECT_TEMPLATE_WORDS`(외주·유지보수·내부 개발),
  모의 생성기: SW 13종 + 구형 6종(`ALL_TEMPLATES`, 명시 참조만), 긴 키워드 우선 매칭, 인력 라인 파서("백엔드개발 고급 3MM 단가
  2,500,000원"), 합계 실수 시뮬레이션(세액 분리 양식은 합계에 세액 누락 → 검증기가 8,250,000 제안), 새 절 문장.
- **테스트**: `generate::tests`(13/19·slug=doc_type·키워드·숨김 참조·견적서 열), `planner::tests`(estimate·dev_contract), vitest
  `mock-generate.test`(견적서·숨김 참조·진척 보고서 매칭), E2E `generate.spec`(견적서 흐름·진척 보고서 재생성·어시스턴트 견적서).
  `playwright.config.ts`는 `PW_PORT`·`PW_OUT_DIR`로 여러 실행을 나란히 돌릴 수 있게 함(병렬 에이전트용).
- **문서**: RUN-GUIDE 8번·v1.1 예시, 설계 §3.13 템플릿 목록·NL 예시, 체크리스트, 계획 §1 진행, CLAUDE.md 레이아웃.


### 0.3.0 3차 — 프로젝트 상태를 시작·종료일로, 프로젝트별 정리·내보내기와 추적 사본 (2026-11-10; 계획 §5·§6, 결정 5 "종료일 지난 미완은 기한 경과 배지, 종료일 없으면 진행중"·결정 6 "여러 프로젝트 문서는 양쪽에 사본"; 병렬 에이전트)

- **코어 `projects/`**: 수동 상태(`ProjectStatus`) 제거. `ProjectPhase { Planned, Active, Done }`은 `phase_for(start, end, created, today)`로
  읽을 때 계산(시작 전 진행예정, 종료일 지나면 완료, 그 외·종료일 없음은 진행중). `Project.overdue`(기한 경과)는 단계가 아니라 배지 —
  완료이면서 종료일이 있고 필수 서류 누락·최근 불일치가 남았거나 검사한 적이 없을 때. `ProjectInput.start_date/end_date`(새 프로젝트는
  화면에서 오늘이 기본 시작), `repo::set_dates`, "완료 처리"는 종료일 = 오늘, "다시 열기"는 종료일 비움. `ProjectSettings`(JSON,
  `projects.settings`): folder_template·name_template·group_by·export_format·export_layout·export_path·copies(기본 true), 기본 폴더
  `DEFAULT_PROJECT_FOLDER = documents/{{project}}/{{doc_type}}`. 리포트·보완요청서 요청 JSON에 phase/overdue/start/end.
- **마이그레이션 0008** `0008_project_dates.sql`: `projects.start_date/end_date/settings` 추가, 시작 ← created_at, 종료 ← done이면
  updated_at 아니면 due_date, `status` 열 DROP COLUMN(SQLite ≥ 3.35; `due_date` 열은 남지만 쓰지 않음), `document_copies` 테이블.
- **추적 사본 `projects/copies.rs`**: 레코드 하나 + `document_copies(document_id, project_id, rel_path UNIQUE, content_hash, size_bytes,
  state synced|modified|missing)`. 문서가 두 번째 이상 프로젝트에 들어갈 때만 그 프로젝트의 폴더 템플릿으로 물리 사본을 쓴다(첫 프로젝트는
  원본이 사본; 같은 이름이면 `_2`). `add_documents`·`remove_document`·`remove_project_copies`·`remove_all_for_document`,
  `sync_document`(원본 변경 → 재작성, 이동·개명 → 따라감, 밖에서 고친 사본은 `modified` 표시만), `sync_project`(강제 재작성 = "다시 동기화"),
  `rescan`(파일 없는 사본 복구, 모듈을 거치지 않은 소속 — 자동 분류·inbox 매핑·프로젝트로 가져오기 — 의 빠진 사본 생성; `import::rescan`
  끝에서 호출). 가져오기 계획은 사본 경로를 `Duplicate`로 돌려 재등록을 막고, 문서 삭제·프로젝트 삭제는 사본을 먼저 지운다.
  정리 `apply`는 이동한 문서의 사본을 동기화한다. 셸 `project_sync_copies`(구 `project_set_status` 자리).
- **정리·내보내기·집계**: 템플릿 변수 `{{project}} {{project_code}} {{project_phase}} {{project_start}} {{project_end}} {{project_owner}}`
  (범위의 프로젝트, 없으면 문서의 첫 프로젝트; 없으면 미상 + needs_review), `GroupBy::Project` 폴더 프리셋 "프로젝트별",
  `ScopeFilter.project_id`(scope → 소속 문서). 내보내기: 프로젝트가 있는 문서가 있거나 `ExportLayout::SheetPerProject`면 "프로젝트" 열,
  프로젝트별 시트·CSV 파일(`<파일>_<프로젝트>.csv`, "프로젝트 없음"은 마지막), JSONL `project`, 프로젝트 범위 기본 경로
  `exports/<날짜>_<프로젝트>.<ext>`. 집계 `group_by = project`.
- **플래너**: `GroupBy::Project`("프로젝트별"·"by project" — 프로젝트 참조가 아님), 이름이 든 "○○ 프로젝트 문서 …"는 `scope.filter.project_id`,
  `create_project { start_date, end_date }`(YYYY-MM-DD 검증), 스키마 enum·설명 갱신; `ProjectSummary.status`에는 phase 문자열(평가 문맥의
  프로젝트 status "intake" → "active" 한 줄만 변경, 골든셋·기준선은 그대로 99/100). 규칙 문구는 `planner/tests.rs::project_group_words_and_project_scope`.
- **프런트**: 칸반 3열(진행예정·진행중·완료, 이동 버튼 없음) + 카드 `data-project-phase/overdue`, 상세 헤더 단계·기한 경과 배지와
  시작·종료 날짜 입력(즉시 저장)·완료 처리·다시 열기, 새 프로젝트 대화상자 시작·종료일, 문서 탭 경로 열 + 사본 배지(사본/원본과 다름/사본 없음)
  + "다시 동기화", 새 탭 "정리·내보내기"(`ProjectSettingsTab`: 폴더·파일명 템플릿, 사본 보관, 내보내기 형식·레이아웃·경로, "이 프로젝트 정리"·
  "이 프로젝트 내보내기" → `/organize?project=<id>`), 정리·내보내기 범위에 "프로젝트" 모드 + "프로젝트 설정 사용" 스위치. 모의
  백엔드(`mock-projects.ts` 단계·날짜·설정·가상 사본, `mock-planner.ts` 프로젝트 범위·그룹·변수·시트)·i18n ko/en 동기.
- **E2E**: `projects.spec` 단계 흐름(종료일 과거 → 완료 + 기한 경과, 다시 열기, 시작일 미래 → 진행예정)과 공유 문서 사본(두 번째 프로젝트의
  `archive/{{project}}/{{doc_type}}` 사본 → 다시 동기화 → 소속 해제로 소멸 → 재추가), `organize.spec` "이 프로젝트 정리"(프로젝트 범위·설정 채움·
  `{{project}}`·`{{project_code}}`·`{{project_phase}}`·프로젝트별 프리셋·미상·실행).
- **남은 것**: 템플릿 단위 정리 설정 상속 없음(프로젝트별만), 분류·inbox 매핑으로 들어온 소속의 사본은 재검사/다시 동기화 때 생성, 사본 이름 변경
  추적 시 유형 라벨은 한국어 고정, 외부 변경은 재검사·다시 동기화 시점에만 감지(fswatch 연동 없음), `due_date` 열 잔존, 내보내기 탭의
  프로젝트 설정 채움은 E2E 미포함(Rust 테스트만), 모의 백엔드의 사본은 가상(파일 없음).

### 0.3.0 4차 — 양식 등록을 문서 1~N개의 자동 스키마 도출로 (2026-11-10; 계획 §2, 결정 3 "편집 후 저장 버튼 · 이름 중복은 팝업 · 같은 이름이면 저장 비활성"; 병렬 에이전트)

- **코어 `pipeline/forms.rs`**: `discover(deps, vlm, docs, hint, cancel, progress)` — 샘플 문서마다 `discover_document`(오피스 형식은 표 구조,
  스캔·PDF는 모델이 첫 쪽 + 표가 있는 쪽 최대 3장을 쪽마다 읽어 병합, 모델이 없거나 실패하면 텍스트 규칙으로 대체하고 메모에 남김)로 초안을
  만들고 `merge_drafts`로 하나로 합친다: 라벨의 표준 키(`key_from_label`)로 필드를 맞추고 ⌈n/2⌉개 이상의 샘플에 나온 필드만 채택, 빈도 → 첫
  등장 순 정렬, 라벨·종류는 다수결(동률이면 string보다 구체적인 종류), enum 선택지·표 열은 합집합(≤ 12·≤ 20), `required`는 모든 샘플에 있을
  때만, 이름은 힌트 → 다수결, 슬러그는 `slug_for_name`. `infer_rules`는 합계·세액·기간·퍼센트·테스트 건수 규칙을 `KNOWN_RULES` 안에서만 붙인다.
  결과 `DiscoveryOutcome { kind: "schema_discovery", draft, documents[], merged_fields, dropped_fields }`는 저장되지 않고 편집기로 간다. 실패한
  샘플은 다른 샘플이 성공했으면 `source: "failed"`로 보고만 한다. `SchemaDraft.sample_document_ids`, `register`가 샘플 문서 전부를 `role=form`·
  `doc_type=slug`로 표시.
- **이름 중복 안전장치 `pipeline/repo.rs`**: `name_exists(name, excluding)`(대소문자·공백·구두점 무시), `free_name`("이름 (2)"…), `save_schema`는
  중복이면 `schema_name_duplicate`로 거부(내장 샘플 이름 포함 — 사용자 양식 "견적서"는 "견적서 (2)"가 된다).
- **작업화**: `JobSpec::SchemaDiscovery { doc_ids, name_hint }`(엔진 레인) → 셸 `pipeline_bridge::schema_discovery_job` / `run_schema_discovery`
  (스캔 샘플이 있을 때만 비전 모델 자동 로드, 실패 시 텍스트 규칙; `batch_begin`/`checkpoint`; 진행 "<제목> · discover (p/q)"), 명령
  `pipeline_schema_discover`·`pipeline_schema_name_exists`, `JobKind::SchemaDiscovery`.
- **플래너**: `register_form` op에 `doc_ids[]`(규칙은 선택 문서 전체를 넣음, 미리보기 "문서 n개에서 필드를 도출·병합…"), UI 실행은
  `/schemas?register=id1,id2&name=`.
- **프런트**: 양식 등록 대화상자가 체크 목록(문서 여러 개·드롭·파일 가져오기)·이름 힌트·작업 진행 카드(진행률·취소)로 바뀌고 결과 초안을 편집기에
  "저장되지 않은 변경"으로 연다. 편집기 위에 도출 요약(`schema-discovery-summary`: 문서별 출처(표 구조/모델/텍스트 규칙/실패)·필드 수). 이름
  입력마다 중복 검사(250 ms) → 중복이면 저장 버튼 비활성 + 입력란 아래 오류, 새 초안의 이름이 이미 있으면 팝업이 "이름 (2)"를 제안
  (`schema-name-taken`). 작업 화면 결과 카드 "편집기에서 열기"(`useSchemasStore` 핸드오버). 라이브러리 일괄 바 "선택 문서로 양식 만들기"(문서 1개
  이상). 모의 백엔드가 같은 병합·중복 규칙을 흉내(`mergeDrafts`·`inferRules`·`schemaDiscoveryWorker`). 옛 단일 문서 명령 `pipeline_form_derive`는
  남겨 두되 UI는 쓰지 않음.
- **검증(에이전트 환경)**: 코어 단위 테스트 3건 추가(`forms::tests` 병합·규칙 2, `pipeline::tests::schema_discovery_merges_samples_and_the_name_guard_refuses_duplicates`),
  vitest `mock-schema-discovery.test.ts` 4건, Playwright `pipeline.spec` 양식 등록 구간 재작성(샘플 2개 → 작업 → 병합 10필드 → "견적서 (2)" 팝업
  → 저장 → 두 카드 모두 견적서)·`dnd.spec` 그대로 — 통합 검증 수치는 아래 표.

### 0.3.0 5차 — 문서 생성 토큰 예산 + 개요→절별 분할 생성 (2026-11-10; 계획 §4, 결정 4 "GPU 상향·CPU 제한")

- **코어 `generate/`**: `DraftRequest.max_tokens: Option<u32>`(없으면 길이 프리셋 1,200/1,800/2,600), `prompts::reply_budget`·`split_needed`
  (예산 > 3,000 토큰 또는 컨텍스트의 35 %)·`section_budget`(전체 × 비중, 600~3,000). `draft()`는 작은 예산이면 종전 단일 호출, 크면
  `draft_split`: ① 절 계획 — 템플릿 절 목록이 있으면 그대로(비중 1), 없으면 `draft_outline` 문법(제목·절 4~12개·비중·지침, 700 토큰)으로
  모델이 정하고 실패 시 범용 5절; ② 머리 호출 — 종전 `draft_document` 문법으로 필드·표 전부 + 1절 본문만(절 예산 + 600); ③ 절마다
  `draft_section` 문법(blocks ≤ 24)으로 호출하되 [확정 정보](제목·채워진 필드 ≤ 12)·[앞 절 요약](직전 본문 꼬리 600자)·절 지침·절별 지식
  재검색(절 제목 질의, ≤ 3건, 번호는 공용 자료 뒤에 이어짐)을 넣음; 모델이 쓴 소제목은 level 3, 절 제목(level 2)은 조립기가 보장; ④ 사용량
  합산, 역방향 검증 1회, 메모 "토큰 예산 N로 n개 절을 나눠 생성했습니다". 취소(플래그·`Cancelled`)는 절 사이에서 멈추고 생성된 절을 보존
  (메모 "취소되어 n/m절까지만 생성했습니다"), JSON 실패 절은 제목만 남기고 메모. `GenerateDeps.progress`(0.25~0.95, "outline" / "section i/n"
  / "finishing"), `DraftOutcome.sections`. `schema.rs`에 outline·section 문법(문법 안전 검사 포함), `parse_body`·`call_json_with_retry` 분리.
- **설정 `GenerationSettings`**: `max_tokens_gpu` 32,768(1,200~65,536)·`max_tokens_cpu` 8,192(1,200~12,288), `cap(gpu)`; `AppSettings.generation`.
- **셸**: `generate_bridge::draft`가 요청 예산을 활성 백엔드 상한으로 자르고(`budget_info`: 모델 컨텍스트·GPU 여부·상한·분할 임계),
  `run_draft`가 절 진행률을 작업 책에 넘기며(`detail` "section 3/8"), 취소된 분할 초안도 저장 뒤 취소 보고(초안 목록에서 열림). 명령
  `generate_budget_info`.
- **프런트**: 생성 화면 "토큰 예산" 컨트롤(`generate-token-budget-select`: 길이 설정에 따름 / 2,600 / 6,000 / 12,000 / 24,000 문서 한 통 /
  직접 입력 `generate-token-budget-input`), 모델 컨텍스트·상한 표시, 힌트(한 번에 / 절별 + 예상 시간: GPU 25 tok/s·CPU 6 tok/s 가정),
  CPU 전용 경고(≥ 6,000); 설정 > 처리에 "문서 생성" 카드(GPU·CPU 상한, `settings-generation`); 작업 상세 "n/m절 작성 중"·"개요 작성 중";
  `types.ts`(`max_tokens?`, `TOKEN_BUDGET_PRESETS`, `LENGTH_TOKENS`, `BudgetInfo`, `GenerationSettings`), `useGenerateBudgetInfo`. 모의
  백엔드: 예산 > 3,000이면 절별 진행률·모든 절 생성(≥ 12,000은 절당 문단 추가)·메모, `generate_budget_info`(8K 컨텍스트·GPU·상한 32,768).
- **테스트**: `generate::tests` 2건(예산 도우미·문법 안전; 진척 보고서 5절 분할 — 제목 순서·소제목 level 3·사용량 합산·진행률·절 프롬프트의
  확정 정보/앞 절 요약·예산 배분 1,600(+600) — 3절 요청 시 취소 → 2절 보존 — 작은 예산은 단일 호출), vitest(모의 생성기), E2E
  `generate.spec` "토큰 예산: 문서 한 통 → 8절".
- **보류**: 실시간 스트리밍 미리보기(`generate://delta`, 계획의 2차 선택), 생성 중 컨텍스트 32K 재기동(분할이 더 안정적이라 두지 않음),
  tok/s 실측 기반 예상 시간(현재 상수).
- **문서**: RUN-GUIDE 8번 토큰 예산 문단, 체크리스트(GPU·CPU), 계획 §4 진행.


### 0.3.0 6차 — 뷰어 글자 수정 이력 + Ctrl+Z 범위 분리 (2026-11-10; 계획 §10 7번, 사용자 보고 "되돌리기가 동작하지 않음"; 병렬 에이전트)

- **DB 마이그레이션 0009** `page_text_history`(document_id·page_no, `kind` edit|revert|redo, `source` layer|add|manual, `target_id`, 이전/이후
  텍스트·레이아웃 JSON·출처, `edited_at`; 문서 삭제 시 CASCADE, `SCHEMA_VERSION = 9`). 쪽마다 최근 50건 유지, 유지보수 작업이 저널 보존 기간으로
  함께 삭제(`purge_page_text_history_before`, 집계는 "저널"에 포함).
- **코어 `documents/import.rs`**: `edit_words`·`add_words`·새 `set_page_text`가 수정 전후 상태(텍스트·레이아웃·출처)를 기록. `undo_page_text`
  (가장 최근 적용 수정 되돌림)·`redo_page_text`(가장 오래된 되돌린 수정 재적용)·`revert_page_text_to`(이력 항목과 그 뒤 수정을 모두 되돌림)·
  `page_text_history`(쪽/문서 단위, 바뀐 단어 요약 before → after, `can_undo`/`can_redo`). 되돌리면 `text_source`도 원래 값(layer/ocr)으로 복귀,
  되돌린 뒤 새 수정은 재실행 분기 삭제(편집기 규칙). 원본 파일 불변. 적용된 수정은 항상 접두 구간이라 되돌리기 = 최신 적용 수정, 다시 실행 = 가장
  오래된 되돌린 수정.
- **셸 명령** `documents_page_text_history`, `documents_undo_page_text`, `documents_redo_page_text`, `documents_revert_page_text`
  (`documents://changed` + 지식 재색인); `documents_set_page_text`도 이력 기록.
- **UI**: 뷰어에서 Ctrl+Z는 **현재 쪽에 되돌릴 글자 수정이 있으면 그것을**, 없으면 종전처럼 파일 저널을 되돌림(Ctrl+Shift+Z·설정의 다시 실행
  단축키 = 재적용; 전역 단축키 훅은 화면이 처리한 키(`defaultPrevented`)를 건너뜀). 레이어 바에 "수정 n건" 배지 + 되돌리기/다시 실행 버튼
  (`viewer-text-edits-count`·`viewer-text-undo`·`viewer-text-redo`), 이력 탭에 "글자 수정" 목록(쪽 · 전 → 후 · 출처 · 시각, 되돌려짐 배지,
  "이 수정 되돌리기" `history-text-revert`). 토스트 "글자 수정을 되돌렸습니다 / 다시 적용했습니다". 모의 백엔드 `mock-text-history.ts`(같은 규칙의
  TS 트윈) + `MockDoc.text_history`.
- **검증(에이전트 환경)**: Rust `documents::import` 1건 추가(기록·되돌리기·재실행·출처 복귀·분기 삭제·revert_to·상한 50·전체 목록·purge) —
  `cargo test -p doculens-core --lib -- documents:: knowledge:: db::` 111 통과; Playwright `documents.spec` 5/5(수정 → Ctrl+Z → Ctrl+Shift+Z →
  이력 탭 2건 → "이 수정 되돌리기"). 통합 검증 수치는 아래 표.


### 0.3.0 릴리스 (2026-11-10; 사용자 지시 "최종 종료 후에는 0.3.0 으로 릴리즈 생성 및 LITE 만 빌드해서 배포")

- 버전 0.3.0: `Cargo.toml`(workspace, 4개 크레이트 상속)·`Cargo.lock`, `package.json`·`package-lock.json`(`npm version`), `src-tauri/tauri.conf.json`,
  모의 `app_info`/`settings` 버전 문자열, `npm run notices`(SBOM 버전), RUN-GUIDE의 설치파일 이름·릴리스 링크(v0.3.0 Lite · Full은 v0.2.0 · CUDA 팩은
  v0.1.0), 체크리스트 릴리스 항목.
- 릴리스 노트 `docs/releases/v0.3.0.md`(한국어; 설치파일 표는 Lite만, CUDA 팩·Full 없음 안내, 0.2.0 작업 공간 자동 마이그레이션 0008·0009).
  `release-windows.yml`이 `v0.3.0` 생성 때 이 파일을 `--notes-file`로 씀(0.2.0 릴리스 때 추가한 경로).
- 배포: Actions "Windows installer (manual)" `variant=lite`, `publish=true`, `ref=<검증된 커밋>` 1회만(Full·CUDA 팩 빌드 없음 — Full은 v0.2.0에서
  그대로 받음). 실행·자산 크기는 아래 표.
- 결과: 릴리스 커밋 772782f CI 7개 잡 성공(run 42771724261) → `ref=772782f` Lite 빌드 run 42772557097 성공(약 11분) →
  GitHub Release `v0.3.0`(https://github.com/kimhershey/docHarnes/releases/tag/v0.3.0, draft·prerelease 아님) 자산 4개:
  `DocuLens-Setup-Lite-0.3.0-x64.exe` 50,802,102 B(48.4 MiB) · `.sha256` · `sbom.cdx.json` · `THIRD-PARTY-NOTICES.md`.

### 0.4.0 — 중국계 LLM 제외, Gemma 4 + EmbeddingGemma 2 탑재 (2026-11-10; 사용자 지시 "0.4.0 버전을 준비. 모든 중국계 LLM 을 제외하고, 새로운 8GB 이내 LLM 모델을 준비하여 소프트웨어에 탑재. 기존과 같은 방식으로 모델과 임베딩을 다운로드하도록 변경."; 계획 `docs/plan/v0.4.0.md`; 병렬 에이전트 2 + 직접)

- **선정**: 웹 조사(2026-11-10)와 고정된 llama.cpp b12066(2026-11-03) 소스 확인으로 **Gemma 4 E2B / E4B / 12B**(Google DeepMind, Apache-2.0, 2026-04·06)와
  **EmbeddingGemma 2**(Google, Apache-2.0, 2026-10, 768차원, ggml-org 공식 GGUF Q8_0 310 MB)를 골랐다. b12066은 `gemma4`·`gemma-embedding2` 아키텍처,
  `gemma4v`(E 계열)·`gemma4uv`(12B unified) 프로젝터, Gemma 4 전용 채팅 파서(PEG)와 `--reasoning off`를 이미 포함 → 엔진 팩·CUDA 팩 재빌드 없음.
  후보·탈락 사유(Pixtral·Ministral·Granite·Phi-4-mm·SmolVLM·LFM2.5-VL·HyperCLOVA X SEED·Kanana·EXAONE·A.X; multilingual-e5·arctic-embed·nomic)는 계획 §1.
  "8 GB 이내"는 파일 1개 ≤ 8 GB로 해석(12B는 Q4_K_M 7.12 GB만, Q5_K_M 8.41 GB·26B/31B 제외).
- **레지스트리** `models/registry.json`(7종, 모두 `sha256: null`·`verified: false` — 기존과 같이 HF 접근 가능한 PC에서 고정): `gemma4-e2b-q4km`(3.11 GB, CPU·6·8 GB)
  · `gemma4-e2b-q8`(5.05 GB) · `gemma4-e4b-q4km`(4.98) / `q5km`(5.48) / `q6k`(7.07) · `gemma4-12b-q4km`(7.12 GB, mmproj 175 MB) · `embed-gemma2-q8`(309,855,456 B);
  mmproj-F16 986/990 MB(unsloth 저장소). `tier_defaults`: cpu_only → E2B Q4_K_M, vram_6gb → E2B Q8_0, vram_8gb → `auto:gemma4-e4b`(≥ 7,100 MiB Q6_K / ≥ 6,000 Q5_K_M / Q4_K_M),
  12·16·24 GB+ → 12B Q4_K_M. `min_free_vram_mib`는 "파일 − PLE 표 추정 + mmproj + KV(SWA) + 512 MiB + 1 GiB"의 추정값(체크리스트에서 실측 조정).
  VLM 기본값 `image_min_tokens 512 / image_max_tokens 1120 / top_k 64 / top_p 0.95 / thinking false`, 임베딩 `pooling mean` + `query_prefix`/`document_prefix`.
  다운로드·드롭인·USB 복사·"필요 파일 목록 내보내기"는 그대로(레지스트리 주도).
- **코어** (`crates/doculens-core`):
  - `modelstore.rs` `SUPPORTED_ARCHS`에 `gemma4`(프로필 `gemma4-doc-v1`)·`gemma-embedding2`(embedding) 추가; Qwen·BERT 계열 행은 **0.1~0.3 드롭인 호환으로 유지**
    (레지스트리·추천·다운로드에서만 제외). 드롭인 기본값은 아키텍처별 비전 기하(`image_min/max_tokens`)와 EmbeddingGemma 접두어를 따르고, VRAM 요구치는
    호스트 상주 PLE 표를 뺀다.
  - `pipeline/image.rs` `VisionGeometry { px_per_token, max_tokens, default_min_tokens }`(Gemma 4 48 px·1,120·512 / Qwen 32 px·4,096·1,024; `for_arch`/`for_profile`);
    `Vlm.geometry`(+`with_geometry`), `run.rs`·`forms.rs`·`extract.rs`의 페이지 이미지가 모델 기하로 축소되고 단계 예산은 인코더 상한으로 클램프
    (잔글씨 3,072 → 1,120). 셸 `pipeline_bridge::vision_geometry`(병합 레지스트리의 `arch`)를 pipeline·generate·projects 브리지가 전달.
  - `engine/presets.rs`: `image-min-tokens`를 전역 `[*]`에서 **모델 섹션**으로(기본값 또는 기하 하한, 상한 이하로 클램프), `image-max-tokens`는 기하 상한으로 클램프;
    Gemma 4 중 E2B/E4B(hidden 1,536/2,560)가 아닌 모델(12B unified, CI 픽스처)은 이미지 토큰을 **비인과(non-causal) 어텐션**으로 디코드하므로 이미지 한 장이
    micro-batch에 들어가도록 `ubatch-size = ⌈max_tokens/64⌉·64`(1,152)·`batch-size 2,048`을 모델 섹션에 추가(없으면 서버가 `image_max_tokens`를 n_ubatch로 조용히
    깎거나 `n_ubatch >= n_tokens` 단언으로 죽음 — 픽스처로 재현·확인). `--reasoning off --reasoning-budget 0`·`enable_thinking: false`는 유지(Gemma 4 템플릿도 수용).
  - `gguf.rs`/`memory.rs`: `ModelMeta`에 `sliding_window`·`swa_pattern`·`shared_kv_layers`·`head_count_kv_layers`·`key_length_swa`·`host_resident_bytes`
    (`per_layer_token_embd.weight`), `ModelArch.swa: Option<SwaLayout>`로 Gemma 3/4 KV 추정(전역층 × ctx + SWA층 × min(ctx, window); 공유 KV층 제외),
    `LoadPlan.host_bytes`로 GPU 추정에서 PLE 표 제외; 상수 `GEMMA4_E4B/E2B/12B`, `synthetic_gemma4_metadata`. E4B 32K q8_0 KV ≈ 138 MiB(Qwen3.5-4B 512 MiB).
  - `knowledge/embed.rs`: `EmbedPrefixes`(`defaults.query_prefix`/`document_prefix`) — 문서는 길이 캡 뒤 접두어, 질의도 동일; 저장 텍스트는 접두어 없음.
    셸 `knowledge_bridge`: 레지스트리에도 드롭인에도 없는 설정 id(0.3.0의 `embed-bge-m3-q8`)는 무시하고 `default_embedding`으로; `engine_bridge::assess`도
    사라진 `active_model` id를 티어 추천으로 대체(설정값은 그대로 둠). 차원 1024 → 768은 기존 `ensure_vec_table`이 재생성.
  - `planner`: 예시·테스트·골든셋(ko-07·ko-53·mx-07, `planner-context.json`) 모델 id를 Gemma 4로, `rules::model_id`가 "4B"를 `e4b` 부분에도 매칭; 기준선 재생성(99/100, +0.0 pt).
- **프런트·모의·E2E**: 모의 백엔드(`gemma4v`·`gemma4-doc-v1`·`gemma4`·임베딩 아키텍처 판정 `isEmbeddingArch`)·지식 모의(`embed-gemma2-q8`, 768)·파이프라인/생성/플래너 모의 id,
  가져오기 자리표시자, i18n 3키(ko/en), `engine.spec`(`gemma-4-E4B-it-Q5_K_M.gguf` → 폴더 id `gemma-4-e4b-it-q5_k_m`, 레지스트리 `gemma4-e4b-q5km`)·`smoke.spec`.
- **CI 픽스처**: `scripts/gen-tiny-gemma4.py`(torch 불필요: `gguf`(PyPI)·`tokenizers`·`numpy`로 텍스트 모델(4층 SWA 3 + 전역 1, n_embd 64, p-RoPE 주파수 텐서,
  `gemma4` BPE 토크나이저·공식 제어 토큰·Gemma 4 채팅 템플릿)과 GEMMA4V mmproj(ViT 2층·패치 16·3×3 풀링·위치 표 2×128)를 직접 씀) →
  `tests/fixtures/tiny-gemma4/`(556 KB + 122 KB); `tiny-qwen35`·`gen-tiny-qwen35.py` 삭제, CI 잡 "Real llama-server (CPU) + tiny Gemma 4 fixture".
  로컬 b12066 CPU 빌드로 확인한 것: 공백은 ▁로 이스케이프, 텍스트·스트리밍·이미지 채팅, `response_format: json_schema`는 Gemma 4 파서가 ```` ```json ```` 펜스를
  문법에 넣고 응답에서 벗김(어휘에 백틱이 없으면 문법 스택이 비는 오류 → 어휘에 추가), 제어 토큰 출력행을 0으로 둬 무작위 모델이 `<channel|>`만 뱉지 않게 함,
  비인과 이미지 디코드의 micro-batch 요구(위 presets).
- **문서**: `docs/plan/v0.4.0.md`(후보·근거·레지스트리·영향·순서·확인 항목), `docs/releases/v0.4.0.md`, `docs/LICENSE-POLICY.md`(Gemma 4·EmbeddingGemma 2 허용, Gemma ≤ 3·1세대
  EmbeddingGemma 금지 유지, "중국계 모델 제외" 원칙), 설계 §2.3/2.5/2.6 대체 안내 + §2.8, 부록 B 안내, RUN-GUIDE(모델 표·권장·설치파일 0.4.0), 체크리스트 "0.4.0 — 모델 교체",
  README·CLAUDE.md·engines/README·eval/README.
- **버전 0.4.0**: `Cargo.toml`·`Cargo.lock`·`package.json`/`package-lock.json`·`src-tauri/tauri.conf.json`·모의 버전 문자열·`npm run notices`(고지·SBOM).
- **빌드·배포**: CI run 42781321912(7개 잡 성공) → "Windows installer (manual)" run 42782108585(`variant=lite`, `publish=false`, `ref=d238dc4`) 아티팩트로 먼저 빌드.
  사용자 지시("우선 릴리즈 실행. 검증은 내가 할게.")로 run 42784292140(`publish=true`, `ref=d238dc4`) → **GitHub Release `v0.4.0` 게시**
  (https://github.com/kimhershey/docHarnes/releases/tag/v0.4.0; `DocuLens-Setup-Lite-0.4.0-x64.exe` 50,802,945 B · `.sha256` · `sbom.cdx.json` · `THIRD-PARTY-NOTICES.md`;
  릴리스 노트 `docs/releases/v0.4.0.md`). 모델 품질·VRAM 검증은 사용자 PC에서(체크리스트 "0.4.0" 절).
- **남은 것 / 사용자 PC**: 계획 §9 = 체크리스트 "0.4.0" 절(다운로드 저장소 파일 이름, VRAM 실측 → `min_free_vram_mib`·적응형 경계, 한국어 골든셋 측정, EmbeddingGemma 2
  재임베딩·유사도 임계, SHA-256 고정, 0.3.0 Qwen 드롭인 호환). E2B/E4B의 PLE 표가 GPU에 올라가지 않는다는 가정은 `print_info`/실측으로 확인.

### 0.5.0 — 프로젝트 중심 통합(A안) + GUI 정리 + 사이드바 용어 (2026-11-10; 사용자 지시 "A 실행, 위에 검토했던 GUI 검토는 제안내용대로 정리. 그리고, 사이드 바의 메뉴들을 AIDP 및 오피스에이전트의 용어들을 참조하여 사람이 사용하는 SW 용어로 변경."; 계획 `docs/plan/v0.5.0.md`; 코어·셸 직접 + 프런트 포크 에이전트 2)

- **배경**: 2026-11-10 검토 2건(GUI에서 불필요한 요소 — AIDP·오피스에이전트 참고; 지식 저장소와 프로젝트 역할 통합)에서 사용자가 **A안**(프로젝트 = 업무 문서 + 참고 자료,
  프로젝트 밖 참고 문서는 공통 참고 자료)과 GUI 정리 제안, 메뉴 용어 변경을 택했다. 계획 §0~§5가 결정 사항이다.
- **데이터**: 마이그레이션 `0010_reference_groups.sql` — `collections.kind TEXT NOT NULL DEFAULT 'reference'`(`reference` | `project`), `collections.project_id TEXT REFERENCES projects(id)
  ON DELETE CASCADE` + 인덱스. 기존 컬렉션은 모두 `reference`·`project_id NULL` = 공통 참고 자료의 묶음(데이터 이동 없음). `watch_folder` 열은 남기고
  읽기·쓰기를 모두 뺐다(`Collection`·`CollectionInput`에서 제거). **`0011_collections_rebuild.sql`**(`SCHEMA_VERSION = 11`): 프로젝트 묶음이 프로젝트 이름을 그대로 쓰도록
  0002의 표 전체 `UNIQUE(name)`을 없앴다 — SQLite는 인라인 제약을 못 지우므로 `collections`를 다시 만들고(`collection_docs`도 함께: RENAME이 자식 FK를 옛 표로 돌려
  놓고 그 표를 지우면 멤버십이 연쇄 삭제됨) 인덱스(`project_id`, `name COLLATE NOCASE`)를 다시 건다. 공통 묶음끼리의 대소문자 무시 유일성은 `save_collection`이,
  프로젝트당 1묶음은 `project_reference_group`/`save_collection`(`project_group_exists`)이 지킨다. `db::tests::migrates_collections_from_schema_9_without_losing_memberships`가
  0.4.0 모양(스키마 9) DB를 심어 10·11을 거친 뒤 멤버십·연쇄 삭제·`foreign_key_check`를 확인한다.
- **코어** (`crates/doculens-core`):
  - `knowledge/mod.rs`: `CollectionKind`, `Collection.kind`/`project_id`(+`is_common()`), `CollectionInput.project_id`(생성 때만 의미), `SearchScope.project_id` + `SearchScope::project(id)`,
    `is_unrestricted`에 포함.
  - `knowledge/repo.rs`: `COLLECTION_COLS`에 kind/project_id; `save_collection`은 생성 시 `project_id` → kind `project`(프로젝트 없음 `project_not_found`, 이미 묶음이 있으면
    `project_group_exists`), 갱신은 저장된 kind·project 유지, 이름 유일성(`collection_name_taken`)은 공통 묶음끼리만(모의와 동일);
    `reference_group_of_project` / `project_reference_group(conn, project_id, project_name)`(없으면 생성, 이름 = 프로젝트 이름 — 공통 묶음과 같아도 됨; 모르는 프로젝트는
    `project_not_found`) / `sync_reference_group_name` / `delete_reference_groups_of_project` / `common_reference_doc_ids`(활성·프로젝트 없음) / `project_work_doc_ids` /
    `project_reference_doc_ids`. **`scope_doc_ids`**: `project_id`가 있으면 허용 집합 = 업무 문서(`project_documents`) ∪ 프로젝트 묶음 문서 ∪ 활성 공통 묶음 문서;
    `documents`가 함께 오면 그 문서들이 업무 문서를 대신한다(선택·첨부 문서로 물어도 참고 자료는 함께 봄 — 플래너 실행기의 working set도 이 경로); `collections`·`roles`·`folder`는
    종전대로 교집합; 프로젝트 자신의 문서는 구판·비활성 묶음 제외 규칙에서 면제; 모르는 프로젝트는 빈 집합(플래너 범위와 같음).
  - `projects/repo.rs`: `delete_project`가 참고 자료 묶음을 먼저 지움(FK CASCADE와 별개로 명시), `save_project` 갱신에서 이름이 바뀌면 묶음 이름 동기화.
  - `planner`: `Op::AddToProject { as_reference: bool }`(serde 기본 false, 스펙 표·문법에 `as_reference` Bool 추가, 설명 "문서를 프로젝트에 넣기(업무 문서 또는 참고 자료)");
    실행기는 `as_reference`면 묶음 보장 → `add_docs_to_collection` → `set_role(knowledge)`(트랜잭션), 아니면 종전 `copies::add_documents`; 요약 "프로젝트 참고 자료에 문서 n건 추가".
    규칙: "참고 자료로/참고로/지침으로/규정으로/기준으로/as reference/guideline" → `as_reference`; 삭제 규칙의 보호어에 "참고 자료·묶음" 추가. `ExplainScope::Collection` →
    `Reference`(직렬화 `reference`, 옛 `collection`은 alias로 역직렬화; `EXPLAIN_SCOPES` 갱신). 실행기의 `Search`·`Answer`·`Explain`(문서 범위 제외)이 `plan.scope.filter.project_id`를
    `SearchScope.project_id`로 넘겨 플랜의 프로젝트 범위도 합집합 규칙을 쓴다. **`PlanContext.scope_project`**(셸 `UiPlanContext.project_id` — 어시스턴트 "현재 프로젝트" 칩):
    문서·폴더·프로젝트를 스스로 말하지 않은 명령의 기본 범위(`rules::scope_of` 끝과 `validate`에서 — 모델 플랜도; `add_to_project`·`import`는 제외, 유형·날짜 필터와는 교집합),
    프롬프트 컨텍스트에 "범위 프로젝트" 줄. **`exec::plan_scope`**: `add_to_project`의 대상 프로젝트는 소속 필터가 아니라 목적지이므로 문서 집합(실행·미리보기)에서 뺀다 —
    "선택한 문서를 한빛 프로젝트에 추가해"가 선택 ∩ 프로젝트 멤버로 비어 `no_documents`가 되던 0.3.0 버그 수정(`exec_tests::add_to_project_takes_outside_documents_and_reference_material`).
  - `generate`: `DraftRequest.project_id`(serde 기본 없음) — `use_knowledge`·`SourceRef::Knowledge`·절별 추가 검색이 `SearchScope::project(id)`로 돈다(`KnowledgeLookup` 묶음 인자).
  - 테스트: `knowledge::tests::project_scope_unions_work_documents_and_reference_material`(묶음 생성·단일성·이름 충돌 번호·`save_collection(project_id)`·합집합·문서 대체·역할 교집합·
    구판/비활성 면제·모르는 프로젝트·이름 동기화·삭제 연쇄·옛 컬렉션 = 공통), `planner::tests`에 "참고 자료로 넣어줘"·영어 "as reference material" 케이스.
- **셸** (`src-tauri`): `project_add_documents(id, doc_ids, as_reference?)`(참고 자료면 묶음 보장 + 추가 + 역할 knowledge, `knowledge://changed collections`·`documents://changed updated`
  발행), 신규 `project_reference_group(id) -> Collection`(없으면 생성), `project_delete`가 묶음 삭제를 발행, `project_update` 뒤 `collections` 변경 발행. `knowledge_search`·
  `knowledge_answer`는 `SearchScope.project_id`를 그대로 받는다(명령 서명 변경 없음). 기존 `knowledge_collection_add {id: 묶음, set_role: true}`로 참고 자료 묶음에 넣어도 같다.
- **프런트**(포크 F1 — 셸·문서함·어시스턴트·설정·작업 현황; 포크 F2 — 프로젝트·참고 자료·넣기 대화상자·모의; 사용자 허용 "에이전트 여러 개"):
  - F1: `App.tsx`(index → `/library`, `/search` → 어시스턴트 근거 모드, `/models`·`/schemas` → 설정 탭 리다이렉트, 쿼리 보존 — `/schemas?register=…` 포함, `/knowledge` →
    `/projects/common` 심), `Sidebar.tsx` 7개(`nav.*`; 드롭 대상 문서함·프로젝트·문서 작성, 나머지는 스프링 로딩만), `CommandPalette.tsx`, `StatusBar.tsx`(AI 엔진 상태 + 모델 이름 +
    작업 배지 + 게이트 안내), `LibraryPage.tsx`(오늘 할 일 띠 `TodayStrip.tsx`, 집계 바 삭제, 정리·내보내기 버튼, 프로젝트에 넣기, 우클릭 `LibraryContextMenu.tsx`; 삭제 `HomePage`·
    `SearchPage`·`AggregateBar`·`aggregate.ts`), `AssistantPage.tsx`(범위 전체·현재 프로젝트·선택 문서, 근거만 찾기 `assistant-evidence-only`/`assistant-evidence-mode` →
    `knowledge_search`만 호출, 해석 선택 제거; `evidence.tsx` `EvidenceCard`), `SettingsPage.tsx`(탭 7개 `?tab=`, 일반 탭에 테마·단축키·정보·고급(해석 `settings-routing`),
    처리 탭에 다시 검사·모든 문서 다시 렌더, `tabs.ts`), `ModelsPage.tsx`(추천 카드 + `settings-models-advanced`), `SchemasPage.tsx`(양식 등록 기본), `JobsPage.tsx`(정리 이력
    `jobs-tab-history`), `OrganizePage.tsx`(이력 탭 제거), `dnd/store.ts` `hoverTargetAt`; i18n 용어 일괄 교체; E2E `helpers.ts`(`openSettingsTab`·`openModelsAdvanced`·
    `openAssistantScreen`) + smoke·documents·dnd·engine·organize·planner·pipeline 스펙 재작성.
  - F2: `types.ts`(`CollectionKind`, `Collection.kind/project_id`, `CollectionInput.project_id`, `SearchScope.project_id`, `add_to_project.as_reference`, `COMMON_REFERENCE_ID`,
    `watch_folder` 제거), 모의(`mock-knowledge.ts` 묶음 종류·프로젝트 묶음 생성/이름 동기화/삭제·범위 합집합, `mock-projects.ts` `project_reference_group`, `mock-planner.ts`
    `as_reference`), `ipc.ts` `projectReferenceGroup`, `ProjectsPage.tsx`(고정 카드 `projects-common-reference`, `?put=`/`?classify=`, 드롭 → 넣기), `ProjectDetailPage.tsx`(탭 문서·
    참고 자료·규칙·결과·설정, `/projects/common`), `ResultsTab.tsx`(관계 + 리포트), `ReferenceTab.tsx`, `CommonReferencePage.tsx`, `PutInProjectDialog.tsx`(대상·종류·자동 분류 제안·
    파일에서 가져오기), `ReferenceDocsPanel.tsx`·`ReferenceGroupsView.tsx`·`ReferenceAdvancedPanel.tsx`·`reference.ts`·`hit.tsx`, `IngestWizard.tsx`(`lockCollection`),
    `AddToCollectionDialog.tsx`는 넣기 대화상자 래퍼(뷰어), `KnowledgePage.tsx`는 리다이렉트 심; 삭제 `PickProjectDialog`·`CollectionsPanel`; `mock-reference.test.ts` 3건,
    `e2e/reference.spec.ts`(옛 `knowledge.spec`) 5건, `projects.spec.ts` 수정.
  - 통합(직접): 설명 범위 와이어 값 `collection` → `reference`(`ExplainScope` 타입·`opSpecs`·모의 검증은 둘 다 수락·i18n `explain_scope_reference`), `UiPlanContext.project_id`
    타입 필드(F1의 느슨한 캐스트 제거), 모의 버전 문자열 0.5.0, 묶음 이름 규칙을 모의와 맞춤(위 0011).
- **문서**: `docs/plan/v0.5.0.md`, `docs/releases/v0.5.0.md`, 설계 §3.7.2 머리말·§3.14·§3.15 머리말, RUN-GUIDE(메뉴 이름·첫 실행 흐름), 체크리스트 "0.5.0" 절, README·CLAUDE.md.
- **브랜치·CI**: 사용자 결정(버전별 브랜치)에 따라 `main`(PR #18 병합 커밋 4c2a7ca)에서 `release/0.5.0`을 분기해 통합 커밋 81e8e8d, `ci.yml`에 `release/**` 트리거 추가(3a2a26f).
  CI run 42795675001(3a2a26f) 7개 잡 모두 성공. `main`의 run 42791778103(4c2a7ca)은 Web 잡 실패 — WIP 커밋에 포크가 스테이징한 옛 화면 파일 삭제만 먼저 들어가
  타입체크가 깨진 상태였고, `release/0.5.0`을 병합하면 해소된다.
- **Lite 설치파일**: "Windows installer (manual)" run 42797299609(`--ref release/0.5.0`, `ref=3a2a26f`, `variant=lite`, `publish=false`) 성공 — 아티팩트
  `DocuLens-Setup-Lite-0.5.0-x64` 50,824,823 B(2027-02-08까지 보관). 게시(`publish=true`, 같은 `ref=3a2a26f`)는 사용자 확인 뒤(계획 §4 5); 그 뒤 `release/0.5.0` →
  `main` PR 병합.

### 0.6.0 — AI 어시스턴트가 소프트웨어 기능 전부 수행 (2026-11-10; 사용자 지시 "설정 내용 변경이나 파일 추가, 변경 등의 소프트웨어 수행 기능을 AI 에이전트가 모두 수행할 수 있도록 … 0.6.0 으로 실행하자. 완료 후에 0.6.0 릴리즈도 만들어"; 계획 `docs/plan/v0.6.0.md`; 코어·셸 직접 + 프런트 포크 에이전트 1; 브랜치 `release/0.6.0` — `release/0.5.0` 끝에서 분기)

- **op 43 → 62** (`planner/mod.rs` `OpKind`·`Op`, `schema.rs` `OP_SPECS`): 신규 `rotate`·`rerender`·`split`·`merge`·`set_title`·`edit_text`·`set_role`·`manage_reference`·`define_term`·`set_version`·
  `update_project`·`delete_project`·`remove_from_project`·`manage_template`·`classify_projects`·`manage_schema`(코어 실행) + `manage_model`·`workspace`·`maintenance`(UI 액션, `runs_in_ui`);
  확장 `import.paths`(source=files, 확인 카드), `set_setting.key: String`(카탈로그 경로), `engine.start/stop`. 파라미터 열거 `RoleParam`·`ReferenceAction`·`TermAction`·`TemplateAction`·
  `SchemaAction`·`ModelAction`·`WorkspaceAction`·`MaintenanceAction`. 부작용·확인 분류: split/merge/edit_text/delete_project/묶음·템플릿·양식 삭제/classify apply/모델 download·delete/
  engine stop/workspace close·forget/purge_now/`settings_catalog::needs_confirmation`(engine_binary·db_encryption) 확인.
- **설정 카탈로그** `planner/settings_catalog.rs`: 46키(`ui.*`·`models.*`·`processing.*`·`cache.*`·`privacy.*`·`notifications.*`·`knowledge.*`·`generation.*` + `journal_days`) + 옛 6키 별칭,
  `ValueType`(Bool·Int 범위·IntChoice·Float(percent)·Enum·Str)·`coerce`("켜/꺼"·"90%"·한국어 열거명·`classify`→`classify_only`)·`find`(별칭·대소문자). `SETTING_KEYS`는 문법용 리터럴
  (테스트가 카탈로그와 일치 확인). 검증기 `check_setting`이 키를 정규 경로로 바꾸고 값을 형변환한다.
- **실행기** (`exec.rs`): 각 op의 실행 + 페이로드(프런트 계약: rotate `{documents,pages,degrees}` — 상대 회전, split `{document_id,created}`, merge `{document_id,sources}`,
  edit_text `{documents,replaced}`(이력 기록·`set_page_text`), manage_reference `{action,group,documents}`(add는 역할 knowledge), define_term `{action,term}`, set_version
  `{documents,group_key,latest}`, update_project `{project}`, delete_project `{project_id,deleted}`, remove_from_project `{project_id,removed}`, manage_template `{action,template}`,
  classify_projects `{suggested,applied,suggestions}`, manage_schema `{action,schema}`); 헬퍼 `non_empty`·`one_document`·`split_boundaries`·`resolve_group`·`resolve_schema`.
  미리보기(`preview`)에 확인 op의 수치(파일 수, 분할 쪽, 병합 수, 글자 일치 수, 프로젝트 문서 수, 묶음 문서 수, 템플릿/양식 존재, 분류 제안 수).
- **2단계 플래닝** (`prompts.rs`·`llm.rs`·`schema.rs`): `OpFamily` 8계열(ask·documents·reference·projects·forms·compose·output·system; `open`·`help`·`clarify`·`noop` 공통),
  1단계 `family_messages`(계열 목록·한 줄 설명·압축 컨텍스트·예시 4, 문법 `family_schema` = families 1~2, `ROUTE_TOKENS 48`, 스키마 이름 `families`) → 2단계 `messages_kinds`/
  `plan_schema_for`(계열 op 표·few-shot 필터·문법 제한). 라우팅 실패·빈 응답은 종전 1단계로, 라우팅 호출 자체가 컨텍스트 초과면 서버 보고 수치로 예산 보정(`request_and_slot`),
  계열 문법으로 2회 실패하면 전체 op로 1회 더(`widened`, `attempts` 3). `PlanResult.families`. 프롬프트 크기: 계열 호출 ≈ 전체 티어의 1/4 미만(테스트).
- **규칙** (`rules.rs` `agent_ops`, 엔진 뒤·보존 앞): maintenance·workspace·manage_model(티어/백엔드/받기/가져오기/삭제/검증/벤치마크/로드)·schema_admin·reference(묶음 만들기/이름/삭제/
  사용/넣기/빼기·용어·판)·project_admin(템플릿·자동 분류·삭제·빼기·이름/기간/담당/접수번호/키워드)·document_edit(역할·회전·다시 렌더·분할·병합·제목·글자 바꾸기)·generic_setting
  (불리언 18종·숫자 10종 + 토큰 상한); 헬퍼 `numbers`·`quoted_all`·`unquoted`(따옴표 안 이름은 규칙어 아님)·`absolute_date`·`path_in`·`on_off`. `engine`에 start/stop, 삭제 보호어에
  프로젝트·템플릿·양식·워크스페이스, 프리라우터는 "엔진 켜줘"를 화면 열기로 보지 않음. "돌려줘"는 90/180/270이나 방향어가 있을 때만 회전("정합성 검사 돌려줘"는 그대로).
- **골든셋** 125건(+25: ko-71~90·en-21~25; ko-58·en-18의 기대 키는 카탈로그 경로로) 99.2%, 기준선 재생성.
- **프런트**(포크): `types.ts`(Op 유니언·OpFamily·action 유니언·`PlanResult.families`), `settingsCatalog.ts`(46키·형변환·경로 setter), `opSpecs.ts`·`describePlan.ts`(+`agentStepDetail`)·
  `ResultCard.tsx`(`result-agent-detail`·문서/프로젝트 열기), `commands.ts`(범용 set_setting·manage_model·engine start/stop·workspace·maintenance), `mock-planner.ts`(규칙 19·검증·미리보기·
  실행), i18n(ops 19·params 59·ui 24·result.detail 18·settings 46), `e2e/agent.spec.ts` 6건. Playwright 60/60(포크 실행).
- **문서**: `docs/plan/v0.6.0.md`, `docs/releases/v0.6.0.md`, RUN-GUIDE §4 명령, 체크리스트 "0.6.0", 설계 §3.5 플랜 스키마 머리말, CLAUDE.md.
- **빌드·배포**: 통합 커밋 2efc181의 CI run 42808591342 7개 잡 모두 성공(Rust·Windows 컴파일·Web·라이선스·고지/SBOM·골든셋·실제 llama-server) → "Windows installer (manual)"
  run 42810685950(`--ref release/0.6.0`, `ref=2efc181`, `variant=lite`, `publish=true` — 사용자 지시 "완료 후에 0.6.0 릴리즈도 만들어") 성공 → **GitHub Release `v0.6.0` 게시**
  (2026-11-10 12:01 UTC, 대상 2efc181, https://github.com/kimhershey/docHarnes/releases/tag/v0.6.0): `DocuLens-Setup-Lite-0.6.0-x64.exe` 51,010,802 B + `.sha256` + `THIRD-PARTY-NOTICES.md` +
  `sbom.cdx.json`(아티팩트 `DocuLens-Setup-Lite-0.6.0-x64` 51,011,254 B, 2027-02-08까지 보관). Full·CUDA 팩은 빌드하지 않음(엔진 b12066 그대로 — v0.2.0 Full·v0.1.0 CUDA 팩 사용).
- **`main` 상태**: 사용자가 PR #19(`release/0.5.0`, CI 성공)와 PR #20(`release/0.6.0`의 첫 커밋 e6a2e4f = 버전 범프 + 계획)을 병합했다. e6a2e4f는 `Cargo.lock`·SBOM이 0.5.0인 채 `[skip ci]`로
  올라간 커밋이라 `main`의 run 42801573165는 고지/SBOM 잡만 실패(`cargo metadata --locked`가 잠금 파일 갱신 거부; 나머지 6개 잡 성공). 2efc181에 갱신된 잠금 파일·SBOM이 있으므로
  `release/0.6.0`을 다시 `main`에 병합하면 해소된다(위 "버전별 브랜치" 규칙에 재발 방지 추가).

### 0.7.0 — 7건 일괄 (2026-11-10; 사용자 지시 7개 항목; 계획 `docs/plan/v0.7.0.md`; 코어·셸 직접 + 프런트 포크 에이전트 3(워크트리); 브랜치 `release/0.7.0` — `release/0.6.0` 끝(0491812)에서 분기, 범프 커밋 d7c1f3e는 `Cargo.lock`·SBOM 포함 CI 7개 잡 통과)

- **조사**: 탐색 에이전트 5개(프로젝트 탭 / 문서 작성 / 양식 등록 / 어시스턴트 대화 / 긴 이름)로 현황·원인을 모아 계획 §0에 표로 정리. 결정: 프로젝트 통합은 **UI 통합(A안)**(저장 구조·검색 범위 그대로),
  초안 수정은 `DraftRequest.revise` 경로 + op `revise_draft`, 대화는 **DB 저장 세션**(보존 기간 연동).
- **(4) 양식 등록 불능 — 원인·수정(4429e57)**: `FormRegisterDialog`·병합 대화상자가 `DialogContent className="relative"`를 넘겨 tailwind-merge가 `fixed`를 지움 → 대화상자가 화면 아래
  1~1.5 화면 밖에 그려져 배경만 어두워짐(Playwright는 `toBeVisible`·자동 스크롤 클릭이라 못 잡음). `relative` 제거 + 작업이 `queued`면 "다른 엔진 작업이 끝나면 시작" 안내
  (`form-register-pending-hint`, `schemas.form.waiting_hint`). 재발 방지: `DialogContent`에 위치 클래스를 넘기지 않음; E2E `toBeInViewport()`(포크 B).
- **(5) 초안 어시스턴트 — 코어** (`generate/mod.rs`·`draft.rs`·`prompts.rs`·`schema.rs`): `DraftRequest.revise: Option<ReviseScope>`(`section{section}` / `insert_after{section?,heading?}` / `whole`; serde
  default라 옛 요청 호환), `Section`·`sections_of()`(수준 ≤ 2 heading에서 자른 구간; 제목 없는 앞부분은 1절, 제목 없으면 본문 전체가 1절), `draft()`가 `Whole`이 아닌 범위를 `revise()`로 분기:
  확정 사실(제목·필드) + 자료 + `[앞 내용 요약]`/`[현재 절 본문]`/`[뒤 내용 요약]` + `[수정 요청]`을 담은 `prompts::revise_user`(`RevisePrompt`) 한 호출(스키마 이름 `draft_revise`, 모양은
  `section_schema`), `parse_body` → 구간 교체/삽입(새 절 제목: 지정값 → 모델 첫 heading → "새 절"; 모델 소제목은 수준 3), 참고 출처 병합, `request`는 `previous`·`feedback`·`revise` 없이 저장,
  메모 "AI 수정(n절 '제목'): 지시", `validate()` 재실행(이슈 재색인), `DraftOutcome.sections = 1`. `Whole`은 종전 재생성이되 분할 생성의 모든 절 호출에 `SectionRevision{feedback,
  previous_section}`(제목이 같은 이전 절 본문)을 전달(종전엔 머리 호출만). 예산은 `reply_budget`을 600~3,000 토큰으로 클램프. 셸 `generate_bridge::draft`가 `revise`면 `previous`·지시 필수 검사,
  작업 라벨 "<초안 제목> · AI 수정", 진행 단계 `revising`. 테스트 `generate::tests::revise_edits_one_section_in_place_and_split_regeneration_carries_the_instruction`(절 교체·제목 있는 삽입·끝 삽입·오류·
  전체 재생성의 지시 전달).
- **(5) 플래너 op `revise_draft`**(compose 계열, op 62 → 63): `{draft_id?, scope: section|insert_after|whole, section?, heading?, instruction}`; `PlanContext.draft_id`(셸 `UiPlanContext.draft_id` =
  문서 작성 화면에 열린 초안)로 `draft_id`를 채우고 없으면 `draft_required`; 실행기는 저장 초안을 읽어 `previous`+`feedback`+`revise`로 `generate::draft` → 저장 → 페이로드 `DraftOutcome`
  (결과 카드가 `draft_document`처럼 문서 작성 화면으로 넘김). 규칙 `revise_draft_rule`(`draft_document` 앞): "3절 다시 써줘"·"2절을 더 자세히"(절 서수 `section_ordinal`), "결론 절 추가해줘"·
  "2절 뒤에 '위험 관리' 절 추가"(`new_section_heading`: 따옴표 → 절 앞 단어 → 영어 "add a X section"), "초안 전체 격식체로"(whole), "이 절 다시 써줘" → 되묻기; "절차·절반" 등은 `has_section_word`가
  걸러냄. few-shot 1건, 골든셋 125 → 129(ko-91~93·en-26, 항목 `context.draft_id`) 99.2%(기준선 재생성). 테스트 `planner::tests::revise_draft_rules_target_the_open_draft`.
- **(6) 채팅 세션 — 코어·셸**: 마이그레이션 `0012_chat_sessions.sql`(`chat_sessions` id·title·title_auto·created/updated, `chat_messages` session_id·seq·payload(JSON 그대로)·created_at,
  CASCADE; `SCHEMA_VERSION = 12`), `knowledge/chat.rs`(`list_sessions`·`get_session`·`create_session`·`rename_session`(빈 제목 = 자동 제목 복원)·`delete_session`·`list_messages`·`put_message`
  (upsert, `updated_at`, 첫 사용자 메시지 40자로 자동 제목)·`truncate_messages`·`purge_sessions_before`; 테스트 1건), 명령 `chat_sessions_list`·`chat_session_create`·`chat_session_rename`(빈 제목 거부)·
  `chat_session_delete`·`chat_messages_list`·`chat_message_put`(2 MB 상한)·`chat_messages_truncate`, 유지보수 작업 3단계("대화 기록" `chats_days`)가 오래된 세션도 지움.
- **(1)(7)(4-E2E) 포크 B**(병합 ab369f1): `ProjectDetailPage` 탭 4개(`reference` → `documents` 리다이렉트, 헤더 `project-doc-counts` "업무 n · 참고 m"), `DocumentsTab` 재작성(행 `project-doc-${id}`
  `data-kind`/`data-both`, 구분 전환 `project-doc-kind`, 필터 `project-docs-filter-*`, `project-reference-ingest`(묶음 고정 마법사), 무효화 projects+reference+documents), `AddDocumentsDialog` 구분 라디오
  `${prefix}-kind-{work|reference}`, `PutInProjectDialog` → `?tab=documents`, `reference.ts` `useProjectReferenceDocs`, IPC `projectAddDocuments(id, docIds, asReference)`, 모의 `addReferenceDocuments`,
  `ReferenceTab.tsx` 삭제, i18n(`projects.tabs.reference` 삭제, `projects.documents.*`·`projects.detail.doc_counts`); 긴 이름: `dialog.tsx` `grid-cols-[minmax(0,1fr)]`, `badge.tsx` `max-w-full`,
  `SchemasPage` 도출 요약 배지·`FormRegisterDialog` 행·`ImportDocumentsDialog`/`IngestWizard` 배지(보간 `of_rel_path`/`old_rel_path` 버그 포함)·`RequestForm`/`SourcePickerDialog` `title`·`ResultsTab` 근거
  버튼 `max-w-[16rem]`; E2E `scale.spec` `expectInsideHorizontally` + 70자 이름 양식 등록(150 %·90 %, `toBeInViewport`), `pipeline.spec`·`dnd.spec` `toBeInViewport`, `projects.spec` 탭 4개,
  `reference.spec` 통합 탭·구분 전환·필터. 포크 검증: typecheck, vitest 147, build, Playwright 30/30(projects·reference·scale·pipeline·dnd·organize).
- **(6) 포크 C**(병합 0026c15): `stores/assistant.ts` 재구성(`sessions`·`activeId`·`messagesBySession`·`hydrate`·세션 포착 `push(sid,m)`/`patch(sid,id,f)`·`newSession`(빈 세션 재사용)·`select`(지연 로드)·
  `rename`·`remove`, 메시지 `createdAt`, 복원 보정(대기 답변 → `chat_interrupted` 오류, 실행 중 플랜 → 대기), 저장은 push 즉시·patch 300 ms 디바운스, 활성 세션 id는 워크스페이스별 localStorage),
  `attachments.ts` 세션별 첨부(`switchSession`·`dropSession`), `SessionList.tsx`(레일 `assistant-sessions`, `assistant-new-chat`, 행 `assistant-session-${id}`, 메뉴 이름 바꾸기 인라인 입력·삭제 확인,
  `md` 미만 `assistant-session-picker`), `AssistantPage` 요청 경로 전부 세션 포착·세션별 `planningSession`·헤더 "새 대화" = `assistant-new-chat-header`, IPC 래퍼 6개, `mock-chat.ts`(워크스페이스별
  localStorage), i18n `assistant.sessions.*`·`errors.chat_interrupted`, `assistant.test.ts` 9건, `e2e/assistant.spec.ts` 2건. 포크 검증: typecheck, vitest 152, build, Playwright 전체 62/62.
- **(2)(3)(5-UI) 포크 A**(병합 a64b3b1; `e2e/scale.spec.ts`만 포크 B와 충돌 → 두 헬퍼·두 테스트 모두 유지): `types.ts`(`ReviseScope`·`DraftRequest.revise`·`revise_draft` op·`UiPlanContext.draft_id`),
  `src/lib/draft-sections.ts`(`sectionsOf`·`sectionAt`·`reviseScopeLabel`; 화면·모의 공용), `draft.ts`(`insertBlock`·`removeBlock`·`moveBlock`·`setBlockLevel`·`newBlock` + `body[i]` 이슈 재색인,
  `reviseRequest`; 테스트 +3), `DraftEditor`(블록 도구 막대 `draft-block-add-{paragraph,heading,list}-${i}`·`-up/-down/-remove`·`draft-block-level`, `draft-body-add`, 절 제목 `draft-section-revise-${i}`,
  표 래퍼 `data-hscroll="ok"`), `DraftAssistantPanel.tsx`(`generate-revise-{panel,scope,heading,instruction,run,log}`, Ctrl+Enter), `DraftPreview`(ResizeObserver 폭 맞춤 — iframe transform
  축소, `generate-preview-zoom` `data-zoom=fit|full`, `generate-preview-box` `data-scale`, 메모 목록 상한), `GeneratePage`(수정 상태·로그, `revise()`는 일반 초안 작업, **컨테이너 쿼리 레이아웃**:
  `@5xl`(64 rem) 이상 3열·미만 2행 — 150 % 배율에서 고정 rem 양쪽 열이 1280 px를 넘어 미리보기가 0 px로 접히던 버그를 함께 고침, 모든 셀 overflow 봉쇄), `RequestForm`(재생성 패널 제거),
  어시스턴트 `opSpecs`·`describePlan`(+테스트, OP_KINDS 63)·`ResultCard` 초안 핸드오프·`AssistantPage` `planContext().draft_id`, `mock-generate.ts`(`reviseDraft`, 단계 `revising`, `previous`가 있으면 빈
  내용 허용), `mock-planner.ts`(`ruleReviseDraft`, 초안 없으면 되묻기, `buildContext.draft_id`), `jobs/describe.ts`(`revising`), i18n(`generate.revise.*`·`generate.editor.*`·`generate.preview.zoom_*`·
  `planner.ops.revise_draft`·`planner.params.revise_*`·`jobs.detail.draft_revising`; `generate.form.{feedback,feedback_placeholder,regenerate}` 삭제), E2E `generate.spec`(+2: 블록 편집·절/새 절 수정·
  확대 전환, 어시스턴트 "2절 다시 써줘" 초안 없음 → 되묻기 / 있음 → 결과가 수정 초안을 연다), `scale.spec` `expectNoHorizontalOverflow` + 문서 작성 150 %·90 %(`main` 스크롤 없음, 시트 축소).
  포크 검증: typecheck, vitest 150, build, Playwright generate 6·scale 4·agent 6.
- **리뷰(code-review, medium, `release/0.6.0..release/0.7.0`)** 10건 → 8건 수정: ① 절 다시 쓰기가 그 절의 `table_ref`를 버려 표가 본문에서 빠짐 → 교체 블록 뒤에 그 절의 표 참조를 그대로 붙임(테스트 보강);
  ② 보존 파기가 지운 세션에 메시지를 쓰면 `put_message`가 false를 돌려 조용히 버려짐 → 세션 행을 다시 만들고 저장(빈 id만 거부); ③ `notes.truncate(12)`가 최신 메모(AI 수정 메모)를 버림 → 앞쪽을 잘라
  최신 유지 + 자료 메모 중복 제거; ④ "요약해"가 수정 동사여서 "초안 전체 요약해줘"·"3절 요약해줘"가 `revise_draft`로 → 요약/summar 제외(테스트 2건); ⑤ 세션 삭제 시 `dropSession` 뒤 `switchSession`이
  지운 id 아래에 첨부를 다시 보관 → 전환 뒤 삭제; ⑥ `generateDraft`가 거부되면 `pendingRevise`가 남아 다음 생성이 수정으로 기록됨 → 실패 시 해제; ⑦ `finish_draft`가 `previous`만 지워 전체 재생성의
  `revise: whole`이 저장 요청에 남음 → `revise`도 제거(+ 재열기 시 프런트도 `revise: null`); ⑧ 플랜 카드가 `context.history`·`response.context.history`째로 저장돼 세션이 길어질수록 이력이 제곱으로 중복
  → 저장 시 `history: []`; ⑩ 죽은 `regenerationRequest` 제거(테스트는 `reviseRequest`로). 남김: ⑨ 파기 요약이 지운 세션을 `answers` 카운터에 더함 — 보존 항목 "대화 기록"이 답변·세션을 함께 가리키므로
  의도된 합산으로 두고 메모만 남김.
- **문서**: `docs/plan/v0.7.0.md`, `docs/releases/v0.7.0.md`, RUN-GUIDE(§1.0·§3 6~9·§4 0.7.0 명령), 체크리스트 "0.7.0", CLAUDE.md(planner 63·knowledge chat·generate revise·schema 12), 설계 §3.5 0.7.0 메모.
- **검증·배포**: 아래 검증 표(통합 트리 전체 실행) → 통합 커밋 CI → Lite 설치파일 빌드(게시는 사용자 확인 뒤).
- **빌드·배포**: 통합 커밋 3dcb27c의 CI run 42821160605 7개 잡 모두 성공(Rust·Windows 컴파일·Web·라이선스·고지/SBOM·골든셋·실제 llama-server) → "Windows installer (manual)"
  run 42822456037(`--ref release/0.7.0`, `ref=3dcb27c`, `variant=lite`, `publish=false`) 성공 — 아티팩트 `DocuLens-Setup-Lite-0.7.0-x64` 51,093,766 B(2027-02-08까지 보관).
  **Release v0.7.0 게시는 사용자 확인 뒤**(0.7.0 요청에 게시 지시가 없었음; 게시하려면 같은 `ref=3dcb27c`로 `publish=true` 1회 — 릴리스 노트 `docs/releases/v0.7.0.md`가 본문이 됨).
  `main` 병합은 사용자 PR: `release/0.6.0`(0491812, 2efc181 이후 문서만)과 `release/0.7.0`(3dcb27c + 이 기록) — 0.7.0 브랜치가 0.6.0 끝을 포함하므로 `release/0.7.0` 하나만 병합해도 됨.

### 0.7.1 — 0.7.0 전체 동작 점검 → 수정 필요 목록 (2026-11-11; 사용자 지시 "동작을 모두 점검하고 수정이 필요한 부분을 체크해서 리스트로 남겨줘"; 브랜치 `release/0.7.1` — `main` dbb598e에서 분기)

- **상태**: **목록만 작성했고 코드는 고치지 않았다**(지시가 목록). 목록은 `docs/audit/v0.7.0.md` — 153건(A 4 · B 34 · C 72 · D 43; 처음 149건 + 같은 날 사용자 추가 보고로 4건), 항목마다 체크 상자·문제(시나리오)·수정 방향·위치·확인 수준.
  영역 11절(USR 사용자 보고 7 · UI 화면 16 · PLN 플래너 17 · PLT 플랫폼 16 · DOC 문서 15 · KNO 참고 자료·채팅 15 · PIP 파이프라인 15 · GEN 문서 작성 14 · PRJ 프로젝트 16 · PKG 패키징·고지·문서 14 · CON 계약 8),
  §14 문제없음으로 확인한 것 + 0.7.0 일곱 항목 재확인, §15 권장 수정 순서(0.7.1용 6묶음 55건 = A·B 38건 전부 + 함께 끝나는 C·D) + 결정이 필요한 것 7건, 부록 A 합친 보고 대응표, 부록 B 재현 조건.
- **선행**: 사용자가 PR #22로 `release/0.7.0`을 `main`에 병합(dbb598e; 트리는 53197a1과 같음, main CI run 42938205630 7개 잡 통과). **Release v0.7.0은 게시하지 않았다**(최신 Release는 v0.6.0).
  범프 커밋 16c9067(0.7.1: `Cargo.toml`·`Cargo.lock`·`package.json`/lock·`tauri.conf.json`·모의 백엔드·고지/SBOM)을 `[skip ci]` 없이 푸시 → CI run 42938573265 7개 잡 통과.
- **방법**: ① 위 두 CI 기준선 ② 브라우저 빌드(모의 백엔드) Playwright 순회 — 온보딩~설정 7탭·명령 팔레트, 19개 화면 × 배율 100·120·150 %, 대화상자는 150 %와 최소 창 960×600, 화면마다 루트 넘침·가로 넘침·줄 수·접근성 이름·콘솔 오류·번역 키 자동 측정, 스크린샷 128장
  ③ 화면↔코어 명령 180개 이름·인수 기계 대조(불일치 없음, 미사용 `chat_messages_truncate` 1개) ④ 읽기 전용 점검 에이전트 9개(플래너·플랫폼·문서·참고 자료/어시스턴트·파이프라인·문서 작성·프로젝트·패키징/문서/라이선스·계약)
  ⑤ A·B 등급 35건은 원본 코드를 다시 읽거나 재현(나머지 "점검 보고" 표시 항목은 고치기 전에 재현부터) ⑥ 설정 문제는 로컬 llama-server + `tiny-gemma4` 픽스처에 실제 플랜 문법을 걸어 실험.
- **같은 날 사용자 추가 보고**: ① 실기 스크린샷(gemma4-e4b-q4km) — "UI 배율 120%"·"배율을 130 으로 변경해줘" → "프로파일: 균형", "DPI 를 200 으로 변경해줘" → "테마를 '시스템'(으)로 변경했습니다", "안녕하세요" → 빈 플랜 카드(USR-1~3)
  ② "실패작업 재실행" — 사용량 한도로 실패한 점검 에이전트 9개를 재개해 모두 완료 ③ 스크린샷 — 설정 > 양식·추출 항목의 "유형 → 추출 항목 매핑" 표에서 내장/사용자 열을 없애도 된다(USR-5).
- **가장 중요한 발견**
  - **PLN-1(USR-1의 근본 원인)**: 플래너 스키마를 `serde_json::Value`(BTreeMap)로 만들어 `properties`가 알파벳순으로 직렬화되고, llama.cpp의 json-schema→문법 변환이 그 순서를 강제한다. `set_setting`은 `{"key":…,"op":…,"value":…}`로만 쓸 수 있어 모델이 `"op"`부터 쓰면
    system 계열에서 set_profile·set_theme·open·undo·redo·help·clarify·noop만 남는다(실기 증상과 일치). op 63개 중 24개가 같은 깔때기. 최상위도 confidence·intent가 operations보다 먼저. 실험: system 계열 요청 40회 중 `set_setting` 객체 16/16이 `{"key":`로 시작.
    few-shot 응답도 `serde_json::to_string(Value)`라 같은 순서이고, 추출(KIE)·분류·초안 스키마도 같은 방식(초안은 body가 title·fields보다 먼저 — 품질 영향 미측정). `preserve_order` 기능은 작업 공간 전체의 키 순서를 바꾸므로 플래너 스키마에 국한한 순서 보존 직렬화를 권장.
  - **USR-4**: `AppSettings::from_value`(`settings.rs:598`)가 `knowledge`·`generation` 섹션을 읽지 않아 재시작마다 기본값(다음 저장 때 파일도 덮임).
  - **UI-1**: 0.7.0의 3번이 덜 고쳐짐 — `DraftEditor.tsx`의 `sr-only` caption/th가 `position: static` 스크롤 상자 밖으로 나가 문서 루트가 1441×1242(창 1280×800)가 되고 휠로 창 전체가 442 px 밀린다. E2E가 `main`만 검사해 놓쳤다 → `documentElement` 단언 필요.
  - **UI-3**: `DialogContent` 기본형에 최대 높이가 없어 배율 150 %에서 프로젝트 대화상자 959 px(창 800), 최소 창에서도 640 px — 버튼에 닿지 못한다.
  - **확인 없이 덮어쓰기**: 어시스턴트 `extract`(PLN-3), 재연결 문서 자동 재추출(PIP-1), 검토함 재추출·양식 테스트(PIP-2), 취소한 초안 작업의 저장본 덮어쓰기(GEN-3), 결과 카드의 옛 스냅샷(GEN-4), 기본 프로젝트 범위가 문서 지정 필수 검사를 통과(PLN-5·PLN-6).
  - **기능 불능**: HTML 엔티티 디코더의 바이트 슬라이스 패닉(DOC-1, `text.rs:338` — "R&D 센터 소개"), 목록 블록 입력(GEN-5), 보완요청서의 초안 어시스턴트(GEN-1), 등록된 파일 끌어다 놓기(PRJ-1), 모델 폴더 가져오기(PLT-12), `review` correct/show 무동작(PLN-8).
  - **배포**: 공개 저장소의 Release v0.2.0(Full 1.16 GB)·v0.1.0(CUDA 팩 2개)에 NVIDIA 재배포 파일이 올라가 있는데 `cuda-packs.yml` 주석과 체크리스트가 전제로 적은 앱 EULA가 미결(PKG-1, 사용자 결정), `THIRD-PARTY-NOTICES.md:109-111`의 모델 항목이 아직 Qwen·bge(PKG-2).
- **이 환경에서 못 본 것**: Windows 실기(설치·WebView2 인쇄·알림·탐색기 끌어다 놓기·폴더 감시), GPU 팩 선택과 추론, 실제 Gemma 4의 플랜·추출·초안·답변 품질, 대용량, 한컴/Word 호환 — 목록 §2 "한계".
- **점검 도구**는 세션 스크래치에만 있다(커밋하지 않음). 고칠 때 E2E로 옮길 단언: 화면마다 `documentElement.scrollWidth/Height ≤ 창`·휠 뒤 `window.scrollY = 0`(UI-1), 배율 150 %·최소 창에서 모든 대화상자가 창 안(UI-3), 어시스턴트 플랜 카드 가로 넘침(UI-2).
- **하네스 검토(2026-11-11, 사용자 요청 "다른 소프트웨어는 어떻게 하네스를 구성하는지 먼저 검토해서 추천사항을")**: `docs/audit/v0.7.0-harness-review.md`. 사용자가 "전부 수정 실행"을 보냈다가 중단하고 이 검토를 먼저 요청했다 — **수정은 아직 시작하지 않았다**.
  조사: Home Assistant(내장 문장 해석기 먼저, 못 맞추면 LLM — 2024.12 릴리스 글 원문), Windows 11 설정 에이전트(짧은 질의는 검색, 긴 질의만 설정 전용 소형 모델, 변경은 제안), Rasa CALM(NLU 어댑터 먼저·LLM은 소수 명령만·실행은 결정적),
  FunctionGemma(미세조정 전 58 % → 후 85 %), VS Code(설정을 찾아 보여 주고 사용자가 변경), Apple App Intents(타입 있는 매개변수·모호하면 고르기·부작용은 확인), MCP 명세(사람이 거부할 수 있어야 함), OpenAI·Anthropic 도구 지침, llama.cpp 변환기 소스(필수 속성은 스키마 순서로 고정 — PLN-1의 기전을 원문으로 재확인).
  **권장 = 세 겹**: ① 문법·프롬프트 수정(op 먼저, 키별 값 타입, 라벨 표와 예시) ② 확인 없이 실행되는 op의 사후 검증(대상 이름·값이 문장에 근거가 없으면 확인 카드; 모델의 confidence는 실행 기준에서 제외 — `planner::guard`와 같은 자리)
  ③ 문장 전체가 "설정 이름 + 값" 꼴일 때만 모델 없이 처리하는 좁은 빠른 경로(설정 카탈로그 기반; 2026-11-09에 걷어낸 낱말 포함 규칙과 다름). 결정은 사용자. 외부 사이트 다수는 세션 네트워크 정책으로 직접 열지 못해 검색 요약에 의존(문서 §7).
- **검토 2차 개정(같은 날, 사용자 지적 "근거 확인 필요 없는 질문을 판별하지 못하는 부분과 잘못 인식되는 근본적 원인은 해결되지 않는 것으로 보임" + 스크린샷 2장)**: 지적이 맞다 — 1차 권장안은 설정 문장만 다뤘다.
  스크린샷: ① "안녕하세요." → 신뢰도 0.90 플랜 카드 "No operation requested, just a greeting." · "없음 · 대상 문서 11건" → 결과 "없음 건너뜀" ② "프로젝트 별 내용을 요약해줘" → "모든 프로젝트의 내용을 묶음으로 요약합니다 · 요약 묶음 하나로 · 문서 11건"(0.80)이 바로 실행(작업 16 %, 2/11).
  **두 경우 모두 모델은 뜻을 알아들었고(요약 문장), 플랜에 그 답을 담을 칸이 없었다.** 코드로 확인한 근본 원인 다섯: ⓐ 출력 순서가 주석의 의도("summary comes first")와 반대 — 직렬화가 알파벳순이라 문법은 confidence → intent → operations → requires_confirmation → scope → summary를 강제(신뢰도가 맨 앞, 해석이 맨 뒤; 모든 문장)
  ⓑ 명령이 아닌 말의 출구 없음 — 1단계는 계열 8개 중 하나를 반드시 고르고(`SYSTEM_FAMILY_KO`), 2단계는 op ≥ 1(글 답변 op 없음), 프롬프트는 모든 입력을 "명령"으로 규정, 질문은 모두 근거 기반 `answer`(일반 지식은 검색 결과가 비었을 때만, 기본값 꺼짐)
  ⓒ 표현 못 하는 요청을 가장 가까운 op로 대신 — `summarize.target`은 `each_doc`/`set`뿐(묶는 기준 없음)이고 **`set`은 실행기에서 미구현**(`exec.rs:679` 두 분기가 같은 코드 → 문서별 요약만), 되묻기 규칙은 유형·양식·필드 이름이 없을 때만(규칙 5)
  ⓓ 자동 실행 기준이 부작용 유무 + 자기 신뢰도 0.6(`MIN_CONFIDENCE`)뿐 — 비용 무시 ⓔ 모델 경로 평가 부재 — CI 골든셋 게이트는 규칙 경로, 실엔진 잡은 무작위 가중치 모델, 모의 백엔드는 규칙 파서를 '모델'로 표시(`doculens-eval planner --engine`은 ModelFirst를 타지만 실제 Gemma로 돌린 적 없음).
  목록에 **USR-6**(대화·일반 질문의 출구 없음, B) · **USR-7**(프로젝트별 요약, B) · **PLN-20**(오래 걸리는 작업 자동 실행, C) · **PLN-21**(모델 경로 평가 부재, B)을 추가하고 PLN-1에 최상위 순서 근거를 보강.
  **권장(개정) = 여덟 가지**: 가. 출력 계약 — ① 순서(해석 먼저·op 먼저·신뢰도 맨 뒤) ② 글로 답하는 길(1단계 분류에 대화·일반 질문, 2단계 플랜 호출을 답변 호출로 대체) ③ 못 하는 요청의 길(되묻기 규칙 + '반영하지 못한 부분' 칸) ④ 빠진 표현(요약의 묶는 기준, `set` 구현)
  나. 실행 전 관문 — ⑤ 근거 검증 ⑥ 비용 기준("실행" 한 번) 다. ⑦ 좁은 빠른 경로(설정 완전 일치, 정해진 인사말) 라. ⑧ 실모델 평가 세트. 조사 추가: 도구 호출 API의 기본값(글 답변이 정식 출력), Rasa CALM의 ChitChat·SearchAndReply·Clarify 명령과 cannot_handle 패턴,
  Adaptive-RAG(검색 불필요 분류의 47 % 혼동 → 분류만 믿지 말 것), LlamaIndex 라우터·하위 질문, 딥 리서치 제품의 사전 계획 확인, Microsoft 사람-AI 상호작용 지침 9·10·11번.
  **사용자 결정 4건**(검토 문서 §8): 설정 빠른 경로, 일반 질문에 답할지(지금 기본값은 일반 지식 꺼짐), 오래 걸리는 작업의 자동 실행, 버전(구조 변경 → 규칙상 마이너; 0.7.1에 함께 넣는 쪽을 권장).
- **다음**: 사용자 확인 뒤 `release/0.7.1`에서 §15의 묶음 1(PLN-1 → USR-1~4)부터 수정. 결정이 필요한 것 7건(PKG-1 EULA/자산, v0.7.0 게시 여부 — 0.7.1 게시 권장, 설정 문장을 규칙 먼저 볼지, USR-5 범위, 동작하지 않는 설정, 업무/참고 구분의 의미, Full 설치파일 정책)은 목록 §15.

### 0.8.0 — 어시스턴트 해석(하네스) 재설계 + 0.7.0 점검 목록 153건 처리 (2026-11-11; 브랜치 `release/0.8.0`, `main` dbb598e에서 분기; 계획 `docs/plan/v0.8.0.md`, 릴리스 노트 `docs/releases/v0.8.0.md`)

- **사용자 결정(2026-11-11)**: "문서와 무관한 지식 질문에 답하게 할지 → 응답하도록 하자." · "설정의 좁은 빠른 경로를 둘지 → 두기" · "오래 걸리는 작업의 자동 실행을 끌지 → 자동실행 켜두자." ·
  "0.8.0 에 적용하고, 0.7.1 도 같이 수정해서 0.8.0 에 적용하자." · "작업이 다 끝나고 커밋이 성공하면 0.8.0 RELEASE 를 만들자." — 0.7.1은 따로 내지 않았다(`release/0.7.1`은 목록 문서만 가진 채 남음).
- **진행 방식**: 리드가 하네스 코어·평가·통합을, 작업자 9(워크트리, 영역별 파일 소유: D1 배포·문서, F1 화면 공통, F2 기능 화면, F3 어시스턴트 화면, R2 문서·파이프라인, R3 플랫폼, R4 참고 자료·프로젝트,
  R5 문서 작성, R6 플래너 실행기)가 점검 목록을 나눠 고친 뒤 병합. 하네스 코어는 독립 검토 1회(9건 → 8건 반영: 빠른 경로의 접두 일치가 부정문·상대 표현을 설정으로 읽던 것, 단위, 근거 검사 구멍 등),
  통합분은 단계 말 검토 1회, 화면 후속 작업자 1. 공용 `target/`은 워크트리별 `codegen-units` 값으로 산출물 이름을 갈라 썼다(cargo `--config 'profile.dev.package.<crate>.codegen-units=N'` — 다음에도 같은 방법).
- **하네스(플래너) — 원인과 계약**(`docs/plan/v0.8.0.md` §1, 검토 `docs/audit/v0.7.0-harness-review.md`):
  - PLN-1: llama.cpp 문법은 스키마 `properties`의 순서대로 키를 강제하는데 serde_json 맵이 이름순이라 `clarification_question`·`confidence`가 먼저, 단계의 `op`가 인수 뒤였다 →
    `crate::jsonorder`가 `required` 순서로 직렬화(플랜: summary → unmet → intent → scope → operations → requires_confirmation → confidence; 단계는 `op`부터). `tests/real_engine.rs`가 실제 llama-server로 순서를 확인(CI 실엔진 잡).
  - `planner/fastpath.rs`: 문장 전체가 인사이거나 설정 하나일 때만 모델 없이 플랜(`PlanSource::Fast`, 두 해석 모드 공통). 낱말 단위 인식(붙임말·어미 표), 부정·질문·복합 문장 제외, 단위(분 ×60, % → 비율),
    되묻기 답으로 값 완성. 설정 문법은 값 종류별 대안(`keys_of_shape`), 카탈로그 46키(`privacy.db_encryption` 제거).
  - 계열 호출의 `chat`/`general` 경로 → 플랜 호출 없이 `answer{mode}` 한 단계(`llm::reply_plan`); `answer.reply` = 준비된 답(인사).
  - `guard::contract_checks`: `Plan.unmet`, 나눌 수 없는 '~별'(`split_check`; 가능한 기준은 `summarize.group_by`/`aggregate.group_by`로 채움), 문장에 이름·값이 없는 설정 변경 →
    `requires_confirmation = true` + 경고 이슈(`unmet_request`·`grouping_dropped`·`ungrounded_change`). 근거는 사용자가 쓴 말만(`rules::user_words` — 되묻기 문장의 어시스턴트 질문 제외).
  - `knowledge::AnswerStyle`(grounded / general / chat): chat은 검색·저장 없이 짧게, general은 설정과 무관하게 답하고 근거가 질문에 답하면 근거로, 아니면 첫 줄 "(일반 지식 기반 답변)" → `used_general_knowledge`(쓰지 않은 근거는 출처에서 뺌).
  - 화면: 답 카드(`reply`), 설정 카드의 전후 값·되돌리기(`ExecuteOutcome.undo`), 묶음 요약, 확인 사유 강조, `uses_scope`가 거짓이면 문서 수 숨김. 모의 백엔드가 같은 계약을 따른다(`e2e/harness.spec.ts`).
- **점검 목록 결과**: `docs/audit/v0.7.0.md` §0 — 153건 중 147건 수정, 4건 부분(KNO-14 · PRJ-4 · PKG-11 · PKG-14), 2건 손대지 않음(PKG-1 결정 대기, PLN-20 사용자 결정) + 추가 PKG-16(번들 팩의 llama.cpp MIT 전문·libomp 고지) 수정.
  결정 없이 기본값으로 처리한 것: USR-5(열만 제거), PLT-10(통합·숨김), PRJ-4(데이터 모델 유지), PKG-9(문서만).
- **평가(PLN-21)**: 골든셋 129 → 163(종류 chat 5 · question 27 · command 100 · setting 31; `expected.confirm`/`unmet`/`alt`, "확인 없이 실행됐을 잘못된 변경" 지표, `--ids`/`--kinds`, `scripts/rescore-planner.py`).
  규칙 경로 기준선 154/163 = 94.5 %(0.7.0을 같은 기준으로 다시 채점하면 140/163, 잘못된 변경 6 → 1). 실제 모델(Gemma 4 E4B Q4_K_M, CPU, llama-server 로컬 빌드): 결과와 한계는
  `eval/results/planner-real-model/README.md` — 사용자 보고 문장 5개를 포함한 같은 23문장에서 0.7.0 4/23(확인 없이 실행됐을 잘못된 변경 11건), 0.8.0 20/23(0건).
  처음 측정(실행파일 A = 3f51704)에서 틀린 세 문장을 보고 검사 세 가지(문장에 없는 묶음 기준, 질문·알림 문장의 설정 변경, 문장이 말한 문서 유형을 범위에 넣기 — 642f20c)를 넣고 그 문장들을 다시 쟀다(B).
  남은 오답: ko-107(검색 대신 되물음 — 0.7.0보다 나빠진 문장), ko-109(알림을 변경으로 읽지만 확인 대기), ko-121(붙여 넣은 글을 초안 작성으로 — 측정 뒤 `prefer_text_only_answer`가 그런 플랜도 되돌리게 했고 재측정은 안 함).
  **골든셋 전체의 실제 모델 정확도는 측정하지 않았다**(CPU에서 플랜 한 건에 수 분; 응답 시간 값은 CPU를 나눠 쓴 상태라 의미 없음).
- **동작이 달라지는 점**(릴리스 노트 §3): 강제 재처리가 손으로 고친 글자를 남김(`reocr_manual`), 표 열 키 `page` 금지, 모호한 금액·날짜는 검토로, 종료일이 오늘이면 완료, 사본 폴더는 한국어 유형 이름, 앱 단일 실행,
  마스킹은 새로 색인하는 문서부터(`INDEX_VERSION`은 올리지 않음).
- **남은 것 / 다음**: ① PKG-1(CUDA 자산·앱 EULA) 결정. ② PKG-11 제거 시 `%LOCALAPPDATA%\DocuLens\` 정리(NSIS 훅 `installerHooks`; 설치파일로 시험 필요). ③ PKG-14 빌드·게시 잡 분리. ④ KNO-14 취소된 요청이 프롬프트 평가 중에도
  엔진을 놓게 하기(`chat_stream` 요청에 `return_progress` — 고정 엔진 b12066에서 지원 확인 뒤). ⑤ PRJ-4 프로젝트별 역할(데이터 모델). ⑥ 규칙 모드의 두 요청 문장(ko-105). ⑦ 실제 모델 전체 측정(GPU PC) — E2B에서 "일반 지식" 표시 누락 여부 포함.
  ⑧ `run::process`가 `Vlm`에 `.with_geometry`를 주지 않아 Qwen 드롭인이 파이프라인에서 Gemma 기하를 쓴다(기존 문제, R2 보고). ⑨ `ProcessReport.resumed`·`CheckRun.partial`을 화면에 표시(타입만 있음). ⑩ 체크리스트 "0.8.0" 절(Windows·GPU·실모델).
  ⑪ 실측에서 남은 오답: "다크 테마 문서 찾아줘"가 검색 대신 되물음(프롬프트 쪽), "…바꿨어" 같은 알림을 모델이 변경으로 읽음(확인 대기로 막아 둠 — 계열 호출이 대화로 보내게 하는 것이 근본 수정). ⑫ 슬래시 명령 카드(`/테마 다크`)에는 되돌리기가 없음.
  ⑬ 모의 백엔드와 코어의 차이: 오류 모양(`{code: <사유>}` 대 `{code:"invalid", message}`), 기본 프로젝트 범위(`scope_project`) 미적용, `filterIsEmpty`가 `project_id`를 무시. ⑭ 구분 전환(업무 → 참고)이 프로젝트 사본을 확인 없이 지움. ⑮ 정리된 카드의 채팅 쓰기는 여전히 비동기라
  변경 직후 수 밀리초 안에 창을 닫으면 잃을 수 있음(KNO-9 잔여). ⑯ 쓰지 않는 번역 키 `planner.ui.inbox_watch_on/off`.
- **진행 기록**: 사용자가 작업 도중 PR #26으로 `release/0.8.0`(5c2cc30)을 `main`에 병합(45a2c1b) → `main` CI run 42983528996의 Web 잡이 `e2e/planner.spec.ts` 되묻기 테스트에서 실패. 원인은 테스트(플랜 카드 3개를 기대했으나 집계의 플랜 카드까지 4개가 맞고,
  새 카드가 도착하기 직전에 검사가 돌 때만 통과) — bcd2447에서 수정. 브랜치는 병합된 `main`에서 다시 시작했고(같은 이름), 그 뒤의 작업(화면 후속 25항목 병합 f4dbe66, 실측 반영 3ace0ef)은 PR #27. 같은 종류의 간헐 실패 2건(`planner.spec.ts` 새로 고침 테스트, `smoke.spec.ts` 테마 단축키)도 테스트에서 고침.
- **릴리스 기록(2026-11-11)**: 최종 커밋 **d002079**(코드는 3ace0ef, 그 위에 문서) — CI run 42992119115(push) 7개 잡 통과(3ace0ef는 run 42991891006, 되묻기 테스트 수정 bcd2447은 run 42989989599).
  설치파일: Actions "Windows installer (manual)" run **42993106705**(`--ref release/0.8.0`, `ref=d002079…`, `variant=lite`, `publish=true`) 성공 → **Release v0.8.0** 게시(태그 `v0.8.0` = d002079, 2026-11-11T09:27:56Z).
  자산: `DocuLens-Setup-Lite-0.8.0-x64.exe` 51,402,789 B, `.sha256`(43578a8670fc7352065c12639bbe9f34c2563d0d5143aa42ca2a27b9731870ea — GitHub가 계산한 자산 다이제스트와 일치), `THIRD-PARTY-NOTICES.md`, `sbom.cdx.json`. 설치·실행은 하지 않았다.
  `main`: 사용자가 PR #26(5c2cc30까지; CI의 Web 잡이 되묻기 테스트로 실패)과 PR #27(d002079까지)을 병합 — 병합 커밋 69adf24의 CI run 42993194709 7개 잡 통과. `release/0.8.0`은 그 뒤 `main`에서 다시 시작했고 이 기록 커밋만 앞서 있다(다음 PR 때 함께 병합).
- **사용자 결정(2026-11-11, 릴리스 뒤)**: PKG-1에 대해 "앞으로 lite 만 쓸려고" — Full 설치파일·CUDA 팩은 더 만들지 않는다. 이미 게시된 자산(v0.2.0의 Full 설치파일, v0.1.0의 CUDA 팩)은 그대로 두었고(내리라는 지시 없음), 문서의 "NVIDIA PC는 v0.1.0의 CUDA 팩" 안내도 그대로다 —
  Lite 전용으로 굳히려면 그 안내와 `release-windows.yml`의 `full` 변형·`cuda-packs.yml`을 정리하는 것이 다음 일이다(앱 EULA는 그러면 필요 없음).

### 0.8.1 — 프로젝트의 참고 자료 · 문장이 말한 범위 (2026-11-11; 브랜치 `release/0.8.1`, `main` 69adf24에서 분기; 릴리스 노트 `docs/releases/v0.8.1.md`)

- **사용자 보고(0.8.0 설치 뒤)**: ① 검색·요약이 참고 자료가 아니라 업무 문서에서 읽는다 ② 검색이 프로젝트별로 실행되지 않는다(프로젝트를 연 채 검색 / "프로젝트별로 검색") ③ 폴더별 정리에서 참고 자료의 문서가 프로젝트로 인식되지 않는다 ④ "프로젝트 열어줘"가 문서함 폴더 화면으로 간다.
  사용자 결정: "1, 2, 3, 4 전부 수행 및 0.8.1 릴리즈", 검증은 토큰을 아끼도록 줄인다(항목별 단위 테스트 + 타입 검사 + 관련 E2E + CI 한 번; 독립 검토·실모델 재측정 없음). 참고 자료 구분은 없애지 않고 조건을 추가하는 쪽(사용자 선택).
- **원인**: 프로젝트의 문서가 두 군데(업무 = `project_documents`, 참고 = 프로젝트 참고 자료 묶음 `collections.project_id` + `collection_docs`)에 있는데 플랜 범위·정리·묶음이 앞쪽만 읽었다. 플랜 문법의 범위에는 프로젝트(`project_id`는 문법 밖)와 문서 종류를 적을 자리가 없었고, `open`에는 프로젝트 대상이 없었다.
- **구현**: `ScopeFilter.role`(work | knowledge; 프로젝트와 함께면 knowledge = 그 프로젝트의 참고 묶음) — `scope::resolve`, 검색·답변의 `SearchScope.roles`; `Op::Search.group_by` + `exec::search_groups`(payload `groups: [{key,label,hits:[색인]}]`);
  `OpenTarget::Project`(검증기가 이름 → id); `guard::where_to_look`(모델·규칙 플랜 공통: 사용자가 쓴 말의 종류·프로젝트 이름을 범위에 채움 `scope_filled`, "X 프로젝트 열어줘"를 프로젝트 열기로 바꿈; 읽기 전용 op에만 적용);
  `guard::keep_the_split`이 검색도 봄; `validate::per_project`에 검색 포함; `projects::repo::all_memberships_with_reference` / `memberships_with_reference`(정리의 `project_vars`, 요약·검색 묶음). 화면: 결과 카드의 묶음 검색(`result-search-group`), `open project`, 답변 요청의 `roles`,
  선택 범위가 풀리면 열린 프로젝트로 복귀. 모의 백엔드 동기화 + `e2e/harness.spec.ts` 1건.
- **하지 않은 것**: 문서 종류를 말하지 않았을 때의 기본값은 그대로(검색·질문은 업무 + 참고, 프로젝트 범위의 요약·파일 작업은 업무 문서만). 플랜 카드의 범위 줄에 종류를 따로 표시하지 않음(경고 줄로만 알림). 설명(`explain`) op의 역할 필터. 실제 모델 측정.
- **검증·릴리스 기록(2026-11-11)**: 로컬 — 코어 `planner::`·`organize::`·`knowledge::`·`projects::` 179건, clippy·fmt, 골든셋 게이트 94.5 %, `npm run typecheck`, vitest 46파일 331건, `e2e/harness.spec.ts` 10건.
  워크스페이스 전체 `cargo test`·전체 Playwright·Windows 컴파일은 CI로만 확인: 범프 49516c3과 최종 커밋 **c18b896**의 CI(push) 7개 잡 통과.
  설치파일: run **43001923489**(`--ref release/0.8.1`, `ref=c18b896…`, `variant=lite`, `publish=true`) 성공 → **Release v0.8.1** 게시(태그 `v0.8.1` = c18b896, 2026-11-11T10:29:56Z).
  자산: `DocuLens-Setup-Lite-0.8.1-x64.exe` 51,413,865 B, `.sha256`(efb6e09162df5595e0bfc7c65da40e2f6d7210fb7148c15f8595928adb483602 — GitHub 자산 다이제스트와 일치), 고지문, SBOM. 설치·실행은 하지 않았다.
  `main` 병합은 사용자 PR(`release/0.8.1` — 0.8.0 릴리스 기록 커밋 포함).

### 테스트·검증

| 항목 | 결과 (2026-11-08) |
|---|---|
| `cargo fmt --all -- --check` · `cargo clippy --workspace --all-targets -- -D warnings` | 통과 |
| `cargo test --workspace` (PDFium·ko-dic 켬) | 코어 lib 244 통과(3 ignored = 생성기; 모듈별 generate 18·projects 15·planner 13·retention 6·documents 26 …); 셸 lib 3(프로토콜 스로틀·`.last_used` 마커); 프로세스 테스트 5; eval 15 |
| `cargo deny check licenses bans sources` | 통과 (새 의존성: `tauri-plugin-notification`, `windows-sys` Power 기능만) |
| 플래너 골든셋 `doculens-eval planner` | 99/100 (ko 70/70 · en 20/20 · mixed 9/10), 기준선 재기록 |
| 실엔진 `tests/real_engine.rs` (tiny Qwen3.5, CPU) | 통과 — 43개 op로 커진 프롬프트가 4K 슬롯에서 "응답 잘림"으로 나타나는 문제를 재현해 Tiny 단계·서버 토큰 수 기반 축소·응답 예산 1,024로 고침; 랜덤 가중치의 문법 내 장광설(1,024 토큰 잘림)은 모델 아티팩트로 허용 |
| `npm run typecheck` · `npm test` · `npm run build` · `npm run licenses:check` | 통과 · vitest 21파일 124건 · 통과 · 모두 허용 |
| Playwright(모의 백엔드) | 34/34 (`generate.spec.ts`·`projects.spec.ts` 추가; 모의 백엔드가 Rust 페이로드 모양만 내므로 E2E가 실제 계약을 검증) |
| `npm run notices` 결정성 | 두 번 실행 결과 동일 — crates 628(runtime 354)·npm 75·SBOM 컴포넌트 709 |
| GitHub CI(License·Web·Rust Linux·Windows compile·notices·eval-gate·실엔진) | 커밋 a0fcafb·9308de3에서 실엔진 잡만 실패(위 수정으로 0e200ec에서 7개 잡 모두 통과). 리뷰 반영 커밋 84e5fdd에서는 License gate·Windows compile 잡이 **GitHub 릴리스 자산 다운로드 장애**(cargo-deny-action의 바이너리 tar가 손상 수신 "gzip: invalid magic", PDFium 다운로드 HTTP 504)로 실패 — 코드와 무관. ece44cc: License gate를 `taiki-e/install-action`(체크섬 검증 prebuilt cargo-deny 0.20.2)으로 바꿔 통과, 나머지 6개 잡 통과; Windows 잡은 여전히 504. c505b81: `scripts/lib/fetch-retry.mjs`(5xx·네트워크 오류·잘린 본문 지수 백오프 재시도 3회)를 fetch-pdfium/engine/kodic에 적용하고 Windows 잡에도 PDFium `actions/cache` 추가 — 장애가 이어져 Windows 잡은 재시도 3회 후에도 504. 6975410: Windows 잡의 PDFium 다운로드를 `continue-on-error`(경고 주석)로 바꿔 컴파일·테스트는 항상 수행(PDFium 테스트는 라이브러리 없으면 자체 skip, Linux 잡은 엄격 유지) → **7개 잡 모두 통과**(다운로드도 복구되어 Windows 잡이 PDFium 테스트까지 실제 수행) |
| 2026-11-09 추가 기능(드래그 앤 드롭 · XLSX/PPTX/텍스트 · 내용 감지 · 뷰어 텍스트 직접 편집) | `cargo test --workspace` 코어 257(3 ignored)·셸 3·평가 15 통과, `cargo deny` 통과(`encoding_rs` 추가), clippy·fmt 통과, vitest 22파일 131건, Playwright 41/41(`dnd.spec.ts` 5, 뷰어 편집 2 포함; 전체 실행 1회에서 검토 큐 테스트가 1번 실패했으나 단독 3회·재실행 전체 통과 → 플레이크), CI: 4fe082d·314e503·c848d81은 Markdown 등록 테스트 1건으로 Rust 잡 실패 → 14577a6에서 7개 잡 통과 |
| 2026-11-09 사용자 PC 보고 수정 2차 1~3번 (ffb8618·705808b·c78c27b) | `cargo test --workspace` 코어 282(3 ignored)·셸 3·평가 15·통합 1, clippy·fmt·`cargo deny` 통과(calamine·cfb 추가); vitest 23파일 139건; Playwright 43건(`dnd.spec` 2건 추가; 검토 큐 테스트의 목록 로드 전 count 경합을 대기 조건으로 고침); CI 7개 잡 |
| 2026-11-09 사용자 PC 보고 수정 2차 4·5번 + 3차 6·7번 (9ca340d·6014ab9·b04660d) | `cargo test --workspace` 코어 294(4 ignored; `documents::layout` 15·`knowledge::answer` 7·`projects` 18 포함)·셸 3·평가 15·통합 1, clippy·fmt·`cargo deny` 통과(새 의존성 없음, 고지·SBOM 변경 없음); vitest 25파일 147건(`conversation.test.ts` 4·`textLayer.test.ts` 4 추가); Playwright 45건(`knowledge.spec` 세션 기억·`projects.spec` 자동 분류 추가, `documents.spec` 수정 글자 색 단언); `npm run build`·`licenses:check` 통과; CI 7개 잡 모두 성공(b04660d 푸시 이벤트; 9ca340d·6014ab9도 성공) |
| 2026-11-09 어시스턴트 모델 우선 해석 + 되묻기·취소 (3467ad4) | `cargo test --workspace` 코어 296(4 ignored; `planner` 15·`prerouter` 8 포함)·셸 3·평가 15·통합 1, clippy·fmt·`cargo deny` 통과; vitest 25파일 147건; Playwright 46건(`planner.spec` 되묻기·취소 1건 추가); `npm run build`·`licenses:check` 통과; CI 7개 잡 모두 성공(3467ad4 푸시 이벤트) |
| 2026-11-09 UI 배율 대응 A안(고정 px 상자 제거) | 프런트 전용 변경(Rust·고지·SBOM 변경 없음): `npm run typecheck`·`npm run build`·`licenses:check` 통과, vitest 25파일 147건, Playwright 49건(`scale.spec` 3건 추가; 1280×800 기본 뷰포트에서 150 % 검사); 모의 백엔드 15화면 × 4조건 자동 점검에서 실제 잘림 0; CI 7개 잡 모두 성공(be0b82d 푸시 이벤트, run 42601541022) |
| 2026-11-10 요약 컨텍스트 초과 · 불필요한 RAG (4차, e7bf331·c5a8aa2) | `cargo test --workspace` 코어 303(4 ignored; `knowledge::tests` 3건·`planner::guard` 3건·`planner::tests` 1건 추가)·셸 3·평가 15·통합 1, clippy·fmt·`cargo deny` 통과, 새 의존성 없음; vitest 25파일 147건; Playwright 50건(`planner.spec` 붙여 넣은 글 1건 추가); `npm run build`·`licenses:check` 통과; CI 7개 잡 모두 성공(c5a8aa2 푸시 이벤트, run 42749113495) |
| 2026-11-10 0.3.0 1차 — 내장 양식 SW 개발 문서화 (b561120 + 후속 커밋) | `cargo test --workspace` 코어 303(4 ignored; `pipeline::schema` 13 보임/6 숨김 검사·`planner` 18·`projects` 18)·셸 3·평가 15·통합 1, clippy·fmt·`cargo deny` 통과(새 의존성 없음); 플래너 골든셋 99/100(SW 문구 31건·문맥 재작성, 기준선 재생성, 미달은 이전과 같은 mx-10 1건); vitest 25파일 147건(`mock-generate.test` 규칙 파서·프로젝트 템플릿 SW화, `template.test` 예시 견적서); Playwright 전체 실행 50건 중 48 통과 → 정리 예시 문서·어시스턴트 테스트 템플릿(유지보수) 수정 뒤 `organize`·`projects` 스펙 7/7 재실행 통과; `npm run build`·`licenses:check` 통과; CI 7개 잡 모두 성공(93fb59b 푸시 이벤트, run 42761926963) |
| 2026-11-10 0.3.0 2~6차 통합 — 생성 템플릿 SW화(1번) · 토큰 예산/절별 생성(4번) · 프로젝트 단계·프로젝트별 정리·추적 사본(5+6번) · 문서 N개 스키마 도출(2번) · 글자 수정 이력(7번) | `cargo test --workspace` 코어 317(4 ignored; `generate` 토큰 예산·분할 2건, `projects`·`organize`·`export`·`aggregate`·`planner` 프로젝트 8건, `pipeline` 스키마 도출 3건, `documents::import` 이력 1건 포함)·셸 3·평가 15·통합 1, clippy(-D warnings)·fmt·`cargo deny` 통과(새 의존성 없음); 플래너 골든셋 99/100(기준선 +0.0 pt, 미달은 mx-10 1건); `npm run typecheck`·`npm run build`·`licenses:check` 통과, vitest 26파일 150건, Playwright 53/53(`generate` 4·`projects` 6·`organize` 4·`pipeline` 2·`dnd`·`documents` 5 재작성 포함); 병렬 에이전트 3개가 같은 트리에서 작업해 중간 WIP 스냅샷 3건(1c885b4·97acb87·d742501, `[skip ci]`)을 거쳤고 통합 커밋에서 CI 실행 — CI 7개 잡 모두 성공(d166b5b 푸시 이벤트, run 42771458691) |
| 2026-11-10 0.3.0 릴리스 (772782f + 문서 77e8b69) | 버전 범프·고지/SBOM 재생성 뒤 `cargo test --workspace`·clippy·fmt·`cargo deny`·vitest·`npm run build`·`licenses:check`·`nsis:check` 통과; CI 7개 잡 성공(run 42771724261); Lite 설치파일 빌드 run 42772557097 성공, Release v0.3.0 게시(Lite 48.4 MiB + sha256 + SBOM + 고지) |
| 2026-11-11 0.8.0 하네스 재설계 + 점검 목록 처리 (브랜치 `release/0.8.0`; 범프 CI run 42954835524, 병합 트리 f9ff128 CI run 42978110330 — 7개 잡 통과; 최종 커밋의 CI·빌드는 위 "0.8.0" 절의 릴리스 기록) | 로컬(Linux, PDFium 켬): `cargo test --workspace` 코어 lib 486(5 ignored)·`engine_process` 5·셸 18·평가 16 통과, `cargo fmt --check`·`cargo clippy --workspace --all-targets -D warnings`·`cargo deny check licenses` 통과, 플래너 골든셋 게이트 154/163 = 94.5 %(기준선과 같음), `node scripts/check-versions.mjs` 13항목 일치, `npm run notices` 차이 없음, `licenses:check`·`nsis:check` 통과, `node --test scripts/tests/*.test.mjs` 36건, `npm run typecheck`·vitest 46파일 331건, Playwright 124건(화면 후속 브랜치의 전체 실행; 병합 뒤 로컬은 어시스턴트 관련 40건, 전체는 CI). 실엔진 테스트(`tests/real_engine.rs`, 문법 키 순서 포함)는 로컬에서 끝까지 돌리지 못했고 CI 실엔진 잡으로 확인. 실제 모델 플래너 측정은 23문장(`eval/results/planner-real-model`). **하지 않은 것**: Windows 실행, GPU, 설치파일 설치, 골든셋 전체의 실제 모델 측정 |
| 2026-11-11 0.7.1 범프 + 0.7.0 전체 점검 (브랜치 `release/0.7.1`; 범프 16c9067, 코드 변경 없음) | 기준선: `main` dbb598e CI run 42938205630 · 범프 CI run 42938573265 모두 7개 잡 성공. 점검: 브라우저 빌드 Playwright 순회(19개 화면 × 배율 3종 + 대화상자, 스크린샷 128장; 콘솔 오류·번역 키 노출 0), 명령 대조 180개 일치, 플랜 문법 실험(llama-server + `tiny-gemma4`, system 계열 40회), 규칙 플래너에 설정 문장 11개(5개 실패 = USR-2). 결과 `docs/audit/v0.7.0.md` 149건(뒤에 사용자 추가 보고 4건으로 153건) — 수정은 아직 없음 |
| 2026-11-10 0.7.0 7건 일괄 (브랜치 `release/0.7.0`; 범프 d7c1f3e CI 통과, WIP be4e798·0c8c8ad·4bf1298, 포크 병합 ab369f1·0026c15 → 통합은 릴리스 기록 참고) | 코어: `DOCULENS_PDFIUM=… cargo test --workspace --no-fail-fast` 코어 336(4 ignored; `knowledge::chat` 1건, `generate::tests` revise 1건, `planner::tests` revise_draft 규칙 1건, `db` 0012 포함)·셸 3·평가 15·통합 1, clippy(-D warnings)·fmt·`cargo deny` 통과(새 의존성 없음); 플래너 골든셋 129건 99.2%(+0.0 pt, 기준선 재생성). 웹(통합 트리): `npm run typecheck`·`npm run build`·`licenses:check`·`nsis:check` 통과, vitest 26파일 155건(`draft.test` +3, `assistant.test` 9, `describePlan.test` 63 op, `mock-reference.test` asReference), Playwright 전체 66/66(2.9분; 새 `assistant.spec` 2, `generate.spec` 6, `scale.spec` 4, `reference.spec`·`projects.spec` 통합 탭) + 리뷰 반영 뒤 generate·assistant·agent·planner 20/20 재실행; CI·빌드는 릴리스 기록 참고 |
| 2026-11-10 0.6.0 에이전트 전기능 (브랜치 `release/0.6.0`; WIP 34af57f·ee718ff → 통합 커밋은 릴리스 기록 참고) | `DOCULENS_PDFIUM=… cargo test --workspace --no-fail-fast` 코어 333(4 ignored; `settings_catalog` 2건, `planner::tests` 규칙 커버리지·2단계 라우팅·`exec_tests::agent_ops_run_against_the_workspace` 포함)·셸 3·평가 15·통합 1, clippy(-D warnings)·fmt·`cargo deny` 통과(새 의존성 없음); 플래너 골든셋 125건 99.2%(기준선 재생성); `npm run typecheck`·`npm run build`·`licenses:check`·`nsis:check` 통과, vitest 26파일 147건, Playwright 60/60(`agent.spec` 6건 포함, 통합 전체 1회 2.7분); CI run 42808591342(2efc181) 7개 잡 성공; Lite 빌드·게시 run 42810685950 → Release v0.6.0(51,010,802 B) |
| 2026-11-10 0.5.0 프로젝트 중심 통합 (브랜치 `release/0.5.0`; WIP 94b6803·2661bf5·fcf34d7·bc806be → 통합 커밋은 CI 기록 참고) | `DOCULENS_PDFIUM=… cargo test --workspace --no-fail-fast` 코어 328(4 ignored; `db` 스키마 9→11 마이그레이션 1건, `knowledge::tests` 프로젝트 범위 1건, `planner` 참고 자료 문구·범위 프로젝트·`exec_tests` add_to_project 3건 포함)·셸 3·평가 15·통합 1, clippy(-D warnings)·fmt·`cargo deny` 통과(새 의존성 없음); 플래너 골든셋 99/100(+0.0 pt); `npm run typecheck`·`npm run build`·`licenses:check`·`nsis:check` 통과, vitest 26파일 147건(`aggregate.test` 삭제, `mock-reference.test` 3건 추가), Playwright 54/54(`reference.spec` 5건 + 재작성 스펙; 포크별 실행 44/44·10/10 뒤 통합 전체 1회); CI run 42795675001(3a2a26f) 7개 잡 성공; Lite 설치파일 빌드 run 42797299609 성공(아티팩트 50,824,823 B, publish=false) |
| 2026-11-10 0.4.0 모델 교체 (WIP 7b939ab·d973eb1·483daae → 통합 커밋은 CI 기록 참고) | `DOCULENS_PDFIUM=… cargo test --workspace --no-fail-fast` 코어 324(4 ignored; `registry`·`modelstore`·`gguf`·`memory`·`engine::presets`·`pipeline::image` Gemma 4 테스트, `knowledge` 접두어 1건, `planner` "4B" 1건 포함)·셸 3·평가 15·통합 1, clippy(-D warnings)·fmt·`cargo deny` 통과(새 의존성 없음); 플래너 골든셋 99/100(기준선 재생성 +0.0 pt, 미달은 mx-10 1건); 실제 엔진 테스트(로컬 b12066 CPU 빌드 + `tiny-gemma4`): 1 passed(137 s; 로드 2.5 s, 채팅·스트리밍 8 델타·json_schema `{"ok": true}`·이미지 채팅(프롬프트 557 토큰)·분류 JSON·KIE 접두·플래너(모델 오류 허용 경로)·양식/NL 스키마(잘림 허용)·초안·규칙 문법 수락; 모델 섹션 `image-min-tokens 512 / image-max-tokens 1024 / ubatch-size 1024`); `npm run typecheck`·`npm run build`·`licenses:check`·`nsis:check` 통과, vitest 26파일 150건, Playwright 53/53; 병렬 에이전트 2개(프런트·문서 / 임베딩·플래너·평가); CI 7개 잡 모두 성공(d238dc4 푸시 이벤트, run 42781321912 — 실제 llama-server 잡은 tiny-gemma4 픽스처); Lite 설치파일 빌드 run 42782108585 성공(`ref=d238dc4`, `publish=false` — 아티팩트 `DocuLens-Setup-Lite-0.4.0-x64` 50,798,147 B, 2027-02-08까지 보관; 이후 사용자 지시로 run 42784292140 `publish=true` → Release v0.4.0 게시, Lite 50,802,945 B) |
| 2026-11-09 이미지 위 글자 레이어 · 우측 패널 AI 검색/이력 (488af39·9e56dd6) | `cargo test --workspace` 코어 266(3 ignored; `documents::layout` 9·`render` 실 PDFium 문자 박스·`import` 레이아웃 왕복 추가)·셸 3·평가 15·통합 1, clippy·fmt·`cargo deny` 통과; vitest 23파일 139건(`text-layer.test.ts` 8 추가); Playwright 41/41(`documents`·`knowledge`·`pipeline` 스펙을 레이어·검토 큐 기준으로 재작성); CI 7개 잡 두 커밋 모두 성공 |
| 설치파일 빌드(Actions "Windows installer (manual)", Windows 러너) | Lite 0.1.0 빌드 성공 3회(run 42471185353·42475854955 solid 43 MB, 42547230697 파일별 압축 47.6 MB; 159 파일 검사 통과) · 1~3번 반영 Lite 빌드 성공(run 42577030466, 검증된 커밋 a461405 고정, 아티팩트 `DocuLens-Setup-Lite-0.1.0-x64` 50.5 MB; Release 게시는 권한 분류기 거부로 publish=false) · **1~8번(어시스턴트 모델 우선·되묻기·취소 포함) 반영 Lite 빌드 성공(run 42592935148, 검증된 커밋 3467ad4 고정, 아티팩트 `DocuLens-Setup-Lite-0.1.0-x64` 50.6 MB, 2027-02-07까지 보관; publish=false — Release 게시는 사용자 작업)** · **0.2.0 릴리스(2026-11-09, https://github.com/kimhershey/docHarnes/releases/tag/v0.2.0)**: 검증된 커밋 3d55db8 고정, publish=true — Full run 42613728278(`DocuLens-Setup-Full-0.2.0-x64.exe` 1,156,786,994 B) → Lite run 42613985631(`DocuLens-Setup-Lite-0.2.0-x64.exe` 50,579,259 B); 자산 6개(설치파일 2종 + `.sha256` 2 + `THIRD-PARTY-NOTICES.md` + `sbom.cdx.json`); 이번에는 publish 디스패치가 거부되지 않음 · **4차 수정 반영 Lite 빌드 성공(2026-11-10, run 42753243021, 검증된 커밋 c5a8aa2 고정, 아티팩트 `DocuLens-Setup-Lite-0.2.0-x64` 50,593,409 B, 2027-02-08까지 보관; publish=false — 버전은 아직 0.2.0, 릴리스 v0.2.0의 설치파일은 3d55db8 빌드 그대로)** · **Full 0.1.0 빌드 성공(run 42547228170): 1.1 GB, 219 파일, CUDA 팩 cu121·cu129 동봉 검사 통과** · CUDA 팩 빌드 성공(cu121 run 42533483890 · cu129 run 42533486332 → Release v0.1.0) — 위 "패키징·고지" 참조 |

이 환경에서 못 한 것: 설치파일의 릴리스 게시(권한 정책; 설치파일·CUDA 팩 빌드는 Windows 러너에서 완료), 실제 Qwen3.5-4B로 초안 품질·
규칙 변환 품질·파이프라인 지표, 한컴/Word에서 생성 HWPX/DOCX 열기, OS 토스트·절전 방지 실동작 — 모두 `docs/user-pc-checklist.md`
P6·P7 항목.

### 0.8.2 — 유형이 필요 없는 질문 (2026-11-12; 브랜치 `release/0.8.2`, `release/0.8.1` 6514e99에서 분기 — 0.8.1이 아직 `main`에 병합되지 않음; 릴리스 노트 `docs/releases/v0.8.2.md`)

- **사용자 보고(0.8.1 설치 뒤)**: "AAA 프로젝트 내에서 AAA 가 무엇인지 요약해서 설명해줘" → 어시스턴트가 문서 유형을 되물음 → "유형 상관없이 전부" → "명령을 해석하지 못했습니다(unknown document type 'all'; known: …)".
  사용자 지시: "유형이 조건에 없는 질문에 대해서 출력 가능하게 만들어줘. 이 부분이 수정될 경우에 내 질문에 문제 없이 응답이 가능한지 확인해줘. 남은 플랜 한도 내에서 작업 후 릴리즈까지 병행해줘" + "설명/검색/알려줘 의 차이가 무엇인지 확인해줘".
- **원인**: ① 모델이 쓰는 범위(`scope_schema`)에 프로젝트를 적을 자리가 없어(프로젝트는 `guard::where_to_look`이 문장에서 채움) 모델이 가장 가까운 `filter.doc_type`을 골랐다. ② `doc_type`은 문법상 자유 문자열인데 검증기는 목록에 없는 값을 차단 오류로 처리 →
  재질의·넓힌 재시도까지 실패하면 검증기의 영어 메시지가 `clarification_text`에 그대로 들어갔다. ③ 모델의 `clarify`는 필요 여부를 따지지 않고 통과했고, 0.8.1의 `where_to_look`은 되묻기·검증 실패에서는 돌지 않았다. ④ 지시문 규칙 5("유형이 목록에 없으면 clarify")가 유형 되묻기를 부추겼다.
- **구현**: `guard::untyped_scope`(`parse_and_validate`에서 검증기 앞) — 유형 칸의 `all`·"전체"류는 조건 없음(모든 플랜), 없는 유형은 묻기만 하는 플랜(`guard::only_asks`: answer·search·explain·compare·summarize)에서 뺌(`doc_type_ignored`; 그 값이 프로젝트 이름이면 알림 없이),
  문장에 없는 유형은 answer·search·explain·compare에서 뺌(요약은 모델이 고른 유형 유지 — 범위의 문서를 다 읽으므로). 문서를 바꾸는 명령은 검증기 오류·재질의 그대로, 두 번 실패하면 `llm::doc_type_question`이 한국어로 유형 목록을 안내.
  `guard::answer_instead_of_asking` — 계열 호출이 `ask` 하나로 읽은 문장에서 모델이 되묻거나(유형을 묻는 질문 / 문장에 프로젝트 이름 / 프로젝트가 열려 있음) 낮은 신뢰도로 묻기만 하는 플랜을 내면, 되묻기는 근거 답변(`answer`)으로 바꾸고 낮은 신뢰도 플랜은 그대로 실행. 그 밖의 되묻기는 유지.
  `guard::explain_what_was_said` — 모델의 `explain`에서 `selection`이 사용자가 쓴 말의 일부(전체의 70 % 이하)가 아니면 사용자의 문장을 질문으로 하는 `answer`로 바꿈(실모델이 selection에 문장 전체·자기 메모를 적음).
  `rules::plan`은 되묻기 문장에서 `user_words`만 읽음(규칙 우선 모드에서 어시스턴트 질문의 유형 이름이 `set_doc_type`으로 읽히던 것 — 실모델 실행의 규칙 패스에서 발견). 지시문 규칙 3(ko/en)·축약 지시문에 "유형은 사용자가 말했을 때만, 되묻지 않음, 프로젝트 이름은 유형이 아님".
- **설명/검색/알려줘**(사용자 질문에 답한 내용, 릴리스 노트 표): `search` = 대목 목록(글 생성 없음), `answer` = 검색 + 모델이 근거로 답, `explain` = 선택 문장·용어를 가리킬 때의 설명(그 밖의 "설명해줘"는 answer), `summarize` = 범위 문서 요약.
- **하지 않은 것**: 모델 범위 문법에 프로젝트 칸 추가(문법·지시문 변경이라 실모델 전체 재측정이 필요), `doc_type`을 알려진 슬러그 enum으로 제한, 모의 백엔드 동기화(모의는 규칙 기반이라 해당 실패가 없음), 골든셋 항목 추가(모델 경로 문제라 규칙 기준선으로는 잴 수 없음 — 단위 테스트 `a_question_needs_no_document_type`이 관찰된 모델 응답을 재생).
- **실제 모델 확인(Gemma 4 E4B Q4_K_M, CPU, `/tmp/engine/bin/llama-server`)**: 사용자 문장 5개(`eval/results/planner-real-model/user-sentences-0.8.2.golden.json` + `.context.json`, 결과 `gemma4-e4b-q4km-0.8.2-user-sentences.json`) —
  5/5 한 번에 계획(되묻기·검증 오류 없음), 범위는 전부 `project_id = p-aaa`. 모델 출력: explain(selection에 자기 메모) / summarize + `doc_type: "project:AAA"` / search + `doc_type: "project"` / explain(selection "AAA") / explain(selection = 문장 전체). 계획 한 건 30초~4분.
  `explain_what_was_said`는 이 측정 뒤에 넣었고 같은 모델 출력을 `the_plans_the_real_model_wrote_for_a_project_question_are_answered`가 재생한다. 답변 단계(검색 + 생성)는 실제 모델·실제 문서로 확인하지 않았다.
- **검증·릴리스 기록(2026-11-12)**: 로컬 — 코어 `planner::` 85건, clippy·fmt, 골든셋 게이트 94.5 %(154/163, 변함없음). 워크스페이스 전체 `cargo test`·vitest·Playwright·Windows 컴파일은 CI로만 확인:
  범프 e416df2(run 43137130141), 63c2243(run 43138856194), 최종 커밋 **05ad4e3**(run 43140374065) 모두 통과. 독립 코드 검토·Windows 설치 실행 없음.
  설치파일: run **43141224749**(`--ref release/0.8.2`, `ref=05ad4e3…`, `variant=lite`, `publish=true`) 성공 → **Release v0.8.2** 게시(태그 `v0.8.2` = 05ad4e3, 2026-11-12T02:35:52Z).
  `DocuLens-Setup-Lite-0.8.2-x64.exe` 51,422,142 B, sha256 `021139ec1fbb2f5f5132ad08b3d23cb4afbdef4ad84dfd04fdf54a70d715fdcb`(게시된 `.sha256`·자산 다이제스트와 일치) + `.sha256`·`THIRD-PARTY-NOTICES.md`·`sbom.cdx.json`.
- **다음**: `release/0.8.2`를 `main`에 병합(0.8.1·0.8.0 릴리스 기록 포함 — 0.8.1 브랜치는 따로 병합할 필요 없음). 모델 범위 문법에 프로젝트 칸을 주는 구조 수정과 E2B·골든셋 전체 실모델 측정은 남아 있다.


### 1.0.0 — 진행 중 (2026-11-12 착수; 브랜치 `release/1.0.0`, `main` 6deedc2에서 분기; 계획 `docs/plan/v1.0.0.md`, 릴리스 노트 초안 `docs/releases/v1.0.0.md`)

- **이어서 할 때 읽을 것**: `docs/plan/v1.0.0.md` 0절의 "다음 시작 지점"과 진행 표. 항목 하나가 끝날 때마다 커밋 · 푸시하고 그 표를 고친다. 이 절은 요약이다.
- **끝난 항목**(절 번호는 계획 문서): 5(고른 작업 표시 설정, 기본 꺼짐) · 10(문서함의 프로젝트 배지) · 7(문서 작성이 인용 문서에서만 가져옴) · 9(전체 선택 / 전체 해제) · 8(문서 고르는 화면의 프로젝트로 좁히기) ·
  12(프로젝트 여러 개에 넣기 + 문서별 배정표 + 2순위 추천 임계값 설정; 코어 `projects::assign`, 명령 `project_assign`) · 6(이미지 내 글자 추출: 결함 두 건 + 그림만 OCR해 쪽 글자 층에 넣기 + 설정).
- **남은 항목**: 1(PPTX 표 · 배경 · 차트) · 4(비슷한 동작 줄이기 — 실제 모델 재측정 필요) · 2-1(서식 유지 수정 저장), 그리고 진행 표의 각 절 "남은 것"(9-3, 8-4, 12-5, 6-6, 골든셋 문장과 기준선 등).
- **1.1.0으로 넘긴 것**(계획 문서 13절): 그림 안 글자의 낱말 위치를 블록 단위(2차원)로 추정하기 — 사용자 결정 2026-11-12. 1.0.0에서는 그림 글의 내용과 넣는 자리는 맞고, 가로로 나란한 라벨 · 글 옆의 표에서 선택 상자가 어긋난다(릴리스 노트 "알려진 제한").
- **릴리스 전에 사용자에게 확인할 것**: 계획 문서 진행 표 아래 "사용자 확인이 필요한 결정"(현재 프로젝트 미리 체크 안 함, 제안이 다르면 배정표로 전환, 미결정 문서의 2순위 처리, 문서 작성의 참고 자료 검색 기본 꺼짐 등).
- **설치파일은 아직 만들지 않았다**(작업 방식 5 — 그 판에 넣기로 한 항목이 다 끝나고 CI가 통과한 뒤 한 번만). 끝난 항목까지만 묶어 1.0.0으로 낼 수도 있다.
- **실제 모델 확인에 쓰는 것**(저장소 밖, 환경이 사라지면 다시 받는다): 모델 · 엔진 · PDFium 준비는 `eval/README.md`; 그림 글자 확인은 `doculens-eval pipeline --figures`.

## P6·P7 단계 말 리뷰 반영

리뷰 서브에이전트 2건(Rust, 프런트엔드)의 지적을 다음과 같이 반영했다.

**프런트엔드 리뷰(High 4·Medium 10·Low 11) — 반영**

- 결과 카드 페이로드 모양: 실행기의 실제 모양(`DraftOutcome{draft,…}`, `{project_id, saved, draft: RuleDraft}`,
  `{project_id, added, project}`)과 모의의 단순 모양을 모두 수용하고 모의를 Rust 모양으로 통일(데스크톱에서 카드가 비던 문제);
  `saved`면 "저장됨" 배지. 규칙 정의 확인 카드는 Rust 미리보기가 구문 규칙으로 만든 `RuleDraft`를 실어 보내도록 바꿔(모델 없이
  결정적) 그리드를 실제 데이터로 표시.
- 열린 프로젝트: 상세 화면을 떠나도 마지막 프로젝트를 유지(워크스페이스 전환·삭제 때만 초기화), 어시스턴트 범위 줄에
  "프로젝트: 이름" 칩; 모의의 "프로젝트가 하나면 그것" 폴백 제거(Rust 검증기와 동일하게 `project_required`).
- 저장 경고: Rust가 보내는 현지화 문장(`PDF 인쇄 모듈 없음 → MD 폴백`, 양식 포맷 전환, 등록 후 처리 실패, 검토 전 저장)을 그대로
  토스트로, 요청 형식과 다르게 저장되면 실제 형식 표기; "덮어쓰기" 라벨을 Rust 의미(기존 파일 덮어씀)에 맞춤.
- 재생성 중 편집 레이스(편집기 `<fieldset disabled>`·저장 타이머 해제·`jobId`가 있으면 자동 저장 생략), "완료" 이동 시 불일치·누락·
  미검사면 확인 다이얼로그(칸반·상세 공통 `doneMoveWarning`), 작업 진행 문구(`loading model`·`drafting`) 번역(모의 전용 단계 문구
  삭제), 초안 출처 표시는 `draft.model` 기준(작업 출처 키는 `job_source`로 분리, `DraftOutcome`에 선택 필드), 결과 탭 이력 선택
  초기화, 템플릿 다이얼로그의 불완전 규칙·음수/비유한 허용오차 차단, lg 폭 3열 레이아웃(그 미만은 편집기가 2행 전체 폭), 리스트 뷰
  이름 버튼으로 키보드 진입, 작업 종료 상태(`interrupted`/`cancelled`/결과 없음) 안내와 중복 생성 방지, 이벤트 병합의 전체 무효화
  보존, 초안 삭제 확인·타이머 해제, 진행 중 초안 작업이 있을 때 다른 초안 열기는 비활성 대신 확인(생성 화면 밖에서 `jobId`가 남아
  버튼이 영구 비활성화되는 것을 피함), 이동 버튼 라벨에 이름·단계, Markdown 번호 목록은 1~3자리만(날짜 줄은 문단), 유지보수
  타입·카드 보강(`skipped`·`cancelled`·`enabled`·`purge_hour_local`, 캐시 용량·휴지통 수·다음 실행), 미사용 export/키 정리,
  문서 선택기는 검색어를 백엔드 `query`로(이름 조회 한도 2,000), 뷰어·썸네일 이미지는 오류 시 1.5초 간격 3회 재시도(`RetryImage`,
  캐시 미스 재렌더와 짝).
- 프로젝트 편집 계약: 코어 `save_project`는 생략/`null` 필드를 유지하고 빈 문자열만 비우므로, 다이얼로그는 비운 필드를 `""`로
  보내고(지워짐) 다이얼로그에 없는 `notes`는 생략(보존); 모의 백엔드도 같은 의미. `ProjectInput` 타입 주석에 명시.
- 테스트: `describePlan` Rust 모양·삭제된 프로젝트 픽스처, 보존 헬퍼 표, Markdown 렌더러, 유지보수 헬퍼, 프로젝트 스토어, 이미지
  재시도 URL; E2E 저장 충돌 → 덮어쓰기·재생성·완료 확인·템플릿 관리·규칙 켜기/끄기·프로젝트 칩 추가(`vitest.config.ts` include를
  `src/**/*.test.{ts,tsx}`로). 작업 취소 E2E는 가속 모의에서 초안 작업이 1초 안에 끝나 타이밍이 불안정해 넣지 않음(취소 토스트
  경로는 단위 수준에서만).

**Rust 리뷰 — 반영**

리뷰(High 3·Medium 7·Low 11) 반영:

- **생성 문서 저장이 원본·등록 양식을 덮어쓰지 않음**: 대상 경로가 라이브러리 문서(`role != generated`)면 `export_over_document`,
  양식 채우기의 원본 양식과 같은 파일이면 `export_over_form`으로 거부(P5 내보내기의 "문서 위에 쓰지 않음" 규칙과 동일); 자기 자신과
  비교하던 양식 불변 테스트를 저장 전 텍스트와 비교하도록 고침.
- **플래너 규칙의 파괴적 오탐 차단**(스크래치 골든셋 64문장 결과 반영): 보존 기간 규칙은 질문·서술("3년 보관 의무가 있나요?",
  "5년 보존이 법적 의무야", "6개월 보관이 기본이지?")을 제외하고 명령형(보관해·설정·keep·retain·무기한…)만 수용; 프로젝트 생성은
  명시적 동사(만들어·생성·create·new project)와 구분 가능한 이름(따옴표·"<이름> 프로젝트"·`project <name>` 중 전치사 없는 짧은
  꼬리)만, "케이스/case" 단어 제거, 생성 op는 확인 카드 필수; 규칙 정의·정합성 검사·보완요청서는 질문·서술 제외(보완요청서는 작성
  동사 필요); 양식 등록은 질문·"화면/어디" 제외; 색인 재구축은 "색인/search index" + 재구축 동사만("인덱스 펀드" 제외); DPI·임계값
  설정은 설정 동사 필요("300으로 스캔했어" 제외); 초안 생성은 선별 키워드 표만 쓰고(카탈로그 키워드 "회의" 제외) 등록 유형은
  "<유형> 양식/서식"일 때만 양식으로, 템플릿 없는 요청은 "초안/문서 작성/draft a…"가 있어야 함; "프로젝트에 … 추가" 비연속 어구
  인식. 오탐 회귀 테스트 추가(`p6_rules_ignore_statements_and_questions`), 골든셋 99/100 유지.
- **워크스페이스 휴지통 파기**: 휴지통으로 옮긴 파일의 mtime을 이동 시각으로 갱신(옛 mtime 때문에 첫 파기에서 되돌리기 대상이
  삭제되던 문제).
- 같은 유형 문서 간 규칙은 같은 문서 안에서만 짝지음(교차 쌍으로 인한 거짓 불일치 제거; 다른 유형은 기존처럼 교차); 페이지 캐시
  LRU는 서빙 시 분당 1회 갱신하는 `.last_used` 마커를 기준으로 하고(일괄 재렌더가 LRU를 FIFO로 만들지 않음) 캐시 미스(404) 시
  문서당 60초 스로틀로 재렌더 작업을 큐잉(뷰어는 이미지 오류 시 재시도); 문서 파기는 양식·지식 역할 제외(`skipped`에 기록), 파일
  삭제 성공 후에만 DB 행 삭제; 배치가 엔진을 쥐고 있으면(`batch_busy`) 규칙 정의·검사 대화상자는 모델 없이 즉시 실행; 유지보수는
  `maintenance.last_attempt`를 시작 직후 기록해 1시간 내 재시도 금지, `last_run`은 취소가 아닐 때만; 프로젝트 없이 템플릿을 지정한
  `define_rule`은 템플릿(id 또는 이름)을 먼저 해석해 확신 초안(≥0.6, 미해석·이슈 없음)만 사용자 템플릿 규칙에 저장, 빌트인은
  `template_builtin`, 모르는 템플릿은 `NotFound`(실행기 테스트 추가); `compare` 문서 id 검증·중복 제거; 실행기의 검사는 로드된
  모델을 명칭 비교에 사용, 보완요청서는 셸과 같은 규칙 경로; `cache/converted`도 LRU 상한에 포함(pages 8/10·thumbs 1/10·
  converted 1/10); inbox 매핑은 `inbox_folder`가 빈 동명 프로젝트 재사용; 프로젝트 갱신에서 생략/`null` 필드 보존·빈 문자열은
  비움; 빌트인 템플릿 갱신 시 `created_at`/`updated_at` 보존; `power::release` 제거(스레드별 상태라 효과 없음, 프로세스 종료로
  해소); 초안 요청 입력 상한(내용 6,000자·텍스트 출처 20,000자·피드백 2,000자).
- 생성 HTML에 CSP(`default-src 'none'; style-src 'unsafe-inline'; img-src data:`) 추가(무네트워크 방어선).
- 문서화: `cache/tmp`(마스킹 전 임시 데이터)는 예약 폴더로 아직 쓰는 코드가 없음 — 설계 §3.8 항목 미구현으로 기록.

## 다음 단계 메모 (v1.x)

- **1.0.0 할 일 목록**: `docs/plan/v1.0.0.md`(2026-11-12 작성, 실행 전) — PPTX 변환본의 배경·표·차트 보강(사용자 보고), HWP · PPT reader/writer 검토 결과, 기존 문서를 서식 그대로 고쳐 저장하기(§2-1), 비슷한 동작 줄이기(§4: answer/explain 합치기, 설정 전용 op 없애기, 참고 자료 지정 한 길로), 어시스턴트가 고른 작업 표시 설정(§5, 기본 꺼짐), 문서 안 그림의 글자 인식(§6: 그림만 OCR → 글자 레이어에 넣어 드래그 선택, 설정 "이미지 내 글자 추출" 기본 꺼짐; 실모델 재현 포함), 문서 작성이 인용 문서 밖의 내용을 쓰는 문제(§7), 문서 고르는 화면에서 프로젝트 단위 일괄 선택(§8), 모든 선택 화면의 전체 선택 / 전체 해제(§9), 문서함 카드의 배지 줄에 소속 프로젝트 배지(§10, 표시만), 추가되는 화면 요소의 넘침 확인(§11), 프로젝트에 넣을 때 프로젝트 여러 개 선택(§12) + 문서별로 프로젝트를 지정하는 배정표(§12-B: 문서 × 프로젝트 체크 표, 쌍 일괄 적용).

0000000. **0.8.0 — 하네스 재설계 + 점검 목록 처리(2026-11-11)**: 위 "0.8.0" 절대로 구현. 남은 것은 그 절의 "남은 것 / 다음" 10항목과 체크리스트 "0.8.0" 절, `release/0.8.0` → `main` 병합(사용자 PR).
000000. **0.7.1 — 0.7.0 점검 목록 처리(2026-11-11)**: `docs/audit/v0.7.0.md` 153건(A 4 · B 34 · C 72 · D 43) — **0.8.0에서 처리함(위 항목; 0.7.1은 내지 않음)**. 아래는 당시 기록: 권장 순서는 목록 §15: 묶음 1(PLN-1 문법 키 순서 → USR-1~4 설정) → 2(화면: UI-1 창 전체 스크롤·UI-3 대화상자) → 3(확인 없는 덮어쓰기) → 4(기능 불능) → 5(실행 환경) → 6(배포·고지).
   결정이 필요한 것 7건(PKG-1 EULA, v0.7.0 게시 여부 등)은 사용자 확인. Release v0.7.0은 게시하지 않았고 0.7.1 게시를 권장. "점검 보고" 표시 항목은 고치기 전에 재현부터.
00000. **0.7.0 7건 일괄(2026-11-10)**: `docs/plan/v0.7.0.md` — 위 "0.7.0" 절대로 구현·검증·CI·Lite 빌드(run 42822456037, publish=false) 완료; Release v0.7.0 게시와 `main` 병합(PR)은 사용자 확인. 남은 것은 체크리스트 "0.7.0" 절(실모델로 절 수정 품질·세션 저장·보존 파기 확인)과
   계획 §4의 범위 밖 항목(프로젝트 저장 구조 통합, 초안 어시스턴트 다중 턴, 세션 검색·내보내기).
0000. **0.6.0 에이전트 전기능(2026-11-10)**: `docs/plan/v0.6.0.md` — 위 "0.6.0" 절대로 구현·게시(Release v0.6.0, 2efc181). 남은 것은 `release/0.6.0` → `main` 재병합(사용자 PR), 체크리스트 "0.6.0" 절(실모델로 계열 선택 정확도·지연 확인)과
   §5의 범위 밖 항목(초안 편집·휴지통 복원).
000. **0.5.0 프로젝트 중심 통합(2026-11-10)**: `docs/plan/v0.5.0.md` — 위 "0.5.0" 절대로 구현. 남은 것은 체크리스트 "0.5.0" 절(사용자 PC 확인)과 플래너 `open` op의 옛 화면
   이름(`models`·`knowledge`·`search`·`schemas`·`organize`·`home`)이 새 위치(설정 탭·문서함·어시스턴트)로 열리는지의 확인.
00. **0.4.0 모델 교체(2026-11-10)**: `docs/plan/v0.4.0.md` — 위 "0.4.0" 절대로 구현·검증 완료. 남은 것은 계획 §9(사용자 PC 실측·골든셋·SHA-256 고정)와
   체크리스트 "0.4.0" 절.
0. **0.3.0 계획(사용자 요청 6건, 2026-11-10)**: `docs/plan/v0.3.0.md` — 생성 템플릿·추출 내장 양식을 SW 개발 문서로, 문서 N개로
   스키마 자동 저장, 생성 토큰 예산(절별 분할 생성), 프로젝트 상태를 시작·종료일 기준으로, 정리·내보내기의 프로젝트(설정)별 분류.
   항목별 현재 구현·변경·영향 파일·사용자 결정(§9, 2026-11-10 답변 반영: 옛 양식 숨김, 저장은 편집 후 버튼 + 이름 중복 검사, 토큰 상한
   GPU 32K/CPU 8K, 기한 경과 배지, 여러 프로젝트 문서는 추적되는 사본)·순서(3 → 1 → 5+6 → 2 → 4 → 7, 약 16.5 작업일). §10: 뷰어
   수정은 앱 내부 데이터만 바꾸고 원본은 불변 — 되돌리기는 파일 저널 전용이라 글자 수정 이력·되돌리기를 7번으로 추가(우선은 그대로 둠).
   **진행**: 3·1·5+6·2·4·7번 모두 2026-11-10 완료(위 "0.3.0 1차~6차" 절; 1·4번은 직접, 2·5+6·7번은 병렬 에이전트 — 사용자 허용).
   0.3.0 릴리스도 완료(위 "0.3.0 릴리스" 절: Release v0.3.0, Lite만). 남은 것은 각 절의 "남은 것"·체크리스트 항목.
1. 남은 부록 D op(GUI 전용: `render`·`rotate`·`split`·`merge`·`add_to_kb`·`manage_collection`·`define_term`·`set_tier`·
   `set_backend`·`import_model`·`download_model`·`benchmark`·`open_workspace`)을 플래너에서 UI 액션으로 연결.
2. P4·P5 숙제 중 미착수: bbox 그라운딩(디지털 PDF 문자 박스), 배치 PDF 자동 경계 탐지, 컬렉션 폴더 감시, OCR 출력 건전성
   검사, 스키마 화면 라우트 이동 차단(데이터 라우터 전환), 실모델 플래너 토큰 수·지연 보정.
3. P7 후속: 실모델 골든셋 CI(self-hosted GPU), 앱 EULA(부록 E, Full 설치파일 전제), Full 설치파일 축소(실측 1.1 GB / 설치 3.1 GB;
   cu121 `-Xfatbin=-compress-all`, 팩의 도구 DLL·실행파일 제거, CUDA 팩 해시의 매니페스트 고정), Release 게시(`publish=true`),
   NSIS 설치 경로와 앱데이터 폴더 겹침 확인.

## 알려진 제약

- HF 차단으로 `models/registry.json`의 크기·SHA-256 미고정(`verified: false`); 다운로드 검증은
  핀이 생기면 자동 적용. `engines/manifest.json`의 cpu·vulkan 자산 해시는 첫 설치파일 빌드 로그로 고정(2026-11-08), `linux`만 미고정.
- GPU 없음: CUDA 팩은 CI에서 빌드했지만 실행 검증(`--list-devices`, `CUDA0` 로그)·Vulkan 팩·실모델 VRAM 실측은 사용자 PC
  (`docs/user-pc-checklist.md` P1 항목).
- Linux 빌드는 CI 전용(배포 범위 외). Windows 실창 E2E(tauri-driver)는 사용자 PC.
- 모의 백엔드의 프리라우터는 Rust 구현의 부분 집합(브라우저 데모 용도).
- 라우터의 `memory` 실측은 CPU 전용 빌드에서는 비어 있음(GPU 장치가 있을 때만 채워짐).
- WebView2 `PrintToPdf`(HWPX/DOCX/HWP 변환본)는 이 환경에서 실행 불가 — Linux WebKitGTK 경로로 동일 파이프라인을
  검증했고 Windows 경로는 컴파일·clippy 검사(교차 타깃 + CI)만 거침(`docs/user-pc-checklist.md` P2 항목). 숨은 창에서
  `PrintToPdf`가 완료되는지, 실패한 내비게이션(오류 페이지)이 인쇄되지 않는지는 사용자 PC에서 확인.
- 이 환경에는 한글 글꼴이 제한적이라 변환본 렌더 품질(글꼴 대체)은 사용자 PC에서 확인.
- 실제 bge-m3·Qwen3.5 모델을 받을 수 없어 임베딩·답변 생성은 해시 임베더·대본 모델(테스트)과 모의 라우터로만 검증했다
  (경로·스트리밍·인용 파싱). 실제 검색 품질(재현율·인용 정확도)과 답변 품질은 사용자 PC 골든셋 항목
  (`docs/user-pc-checklist.md` P3).
- lindera.dev 차단으로 ko-dic 원본은 bitbucket에서 받는다(sha256 핀). CI는 빌드 결과를 캐시한다.
