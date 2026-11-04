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
| P1 엔진·모델 | **완료 (`main`에 병합, PR #2)** | 아래 "P1 산출물" 참조. 병합 후 CI 엔진 빌드 수정은 `phase/p1-engine` 후속 PR |
| P2 워크스페이스·문서 | **완료 (`main`에 병합, PR #3)** | 아래 "P2 산출물" 참조 |
| P3 지식 베이스 | **완료 (`main`에 병합, PR #4; 리뷰 반영 3커밋은 `phase/p3-knowledge` 후속 PR 대기)** | 아래 "P3 산출물" 참조. 청킹, ko-dic FTS5 + trigram + LIKE, bge-m3 + sqlite-vec, 하이브리드 검색, 근거 기반 Q&A·설명·요약·비교 |
| P4 파이프라인 | **완료 (`phase/p4-pipeline`, PR #7 열림 — 사용자 생성; `phase/p3-knowledge` 위에 쌓음, 단계 말 리뷰 반영 완료)** | 아래 "P4 산출물" 참조. 분류·OCR·KIE 호출기, 내장 양식 7종 + 양식 등록 + NL 스키마, 검증기·신뢰도, 검토 큐, 추출 미러, 분할/병합 |
| P5 자연어·작업 | 다음(핵심 op만 1차) | 플래너 PlanSchema, 확인 카드, Job 러너, 정리/되돌리기, 내보내기 |

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
- 뷰어: 지식 배지(컬렉션), "지식 베이스에 추가"·"버전"·"설명(Ctrl+E)" 버튼, 텍스트 탭 검색어 하이라이트(`?q=`), Q&A 탭
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

## P5 착수 메모

1. 플래너(PlanSchema, 부록 D op 전수 중 1차 범위) + 확인 카드 + Job 러너(우선 큐·일시정지·재시도): `pipeline_bridge::process_job`의
   대기열(`pipeline_queue`)과 `JobBook`을 영속 러너로 교체(설계 §3.9 스케줄링).
2. 정리/이름변경/이동 + 저널·되돌리기, 집계(검토 미완료 분리), 내보내기 5종(필드·표 CSV는 `ocr::table_csv`·`fields` 재사용).
3. P4 숙제: bbox 그라운딩(디지털 PDF 문자 박스), 배치 PDF 자동 경계 탐지, 답변/필드 내보내기, 컬렉션 폴더 감시, 워크스페이스
   전환 후 파이프라인 대기열 보존(P5 러너), OCR 출력 건전성 검사, 스키마 화면 라우트 이동 차단(데이터 라우터 전환), 표 XLSX.

## 알려진 제약

- HF 차단으로 `models/registry.json`의 크기·SHA-256 미고정(`verified: false`); 다운로드 검증은
  핀이 생기면 자동 적용. `engines/manifest.json`의 자산 해시도 미고정.
- GPU 없음: CUDA/Vulkan 팩, 실모델 VRAM 실측, `--list-devices` GPU 출력은 사용자 PC
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
