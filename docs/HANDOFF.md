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
| P2 워크스페이스·문서 | **완료 (`phase/p2-documents`, PR 대기)** | 아래 "P2 산출물" 참조. `phase/p1-engine` 후속 커밋(CI 엔진 빌드) 위에 쌓였으므로 P1 후속 PR → P2 PR 순으로 병합 |
| P3 지식 베이스 | 다음 | 청킹, lindera FTS5 + trigram, bge-m3 + sqlite-vec, 근거 기반 Q&A |
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
| `cargo test -p doculens-core` (lib) | 91 | db 4, documents 24(변환 10, 렌더 4, 가져오기 4, 저장소 2, 픽스처 1 …), fswatch 1; PDFium 테스트는 `DOCULENS_PDFIUM` 설정 시 실행(없으면 건너뜀) |
| `tests/engine_process.rs` / `tests/real_engine.rs` | 5 / 1 | P1과 동일 |
| vitest | 27 | i18n 패리티 + 정적 키 해석(새 화면 키 포함) |
| Playwright | 13 | P0 7 + P1 3 + **P2 3**(가져오기 미리보기→렌더→라이브러리·필터·홈 / 뷰어 쪽 이동·회전·태그·유형·삭제 / 일괄 작업·inbox 토글) |

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

## P3 착수 메모

1. 청킹: `pages.text`(디지털 PDF·변환본) 기반 구조 인지 청킹(제목 경로·조항·표 셀) → `chunks` 테이블(마이그레이션
   0002), 문서 역할 `knowledge`·컬렉션(`collections`, `collection_docs`).
2. 색인: lindera + ko-dic(사전 벤더링) FTS5 토크나이저 + trigram 보조 테이블 + 2자 이하 LIKE 폴백; sqlite-vec는
   임베딩 모델(bge-m3 GGUF, `llama-server --embeddings`, P1 `engine::client::embeddings`) 설치 시 백그라운드 작업.
3. 적재 마법사·컬렉션·버전·용어집 UI, 검색 화면(하이브리드), 뷰어 Q&A 탭·드래그→설명, 문장별 출처 칩.
4. P2에서 넘기는 숙제: 페이지 캐시 LRU 정리, 뷰어 썸네일 스트립 가상화(수백 쪽), 가져오기 "예상 소요 시간",
   TIFF 다중 페이지, `engines/manifest.json`·`models/registry.json` 해시 고정(사용자 PC).

## 알려진 제약

- HF 차단으로 `models/registry.json`의 크기·SHA-256 미고정(`verified: false`); 다운로드 검증은
  핀이 생기면 자동 적용. `engines/manifest.json`의 자산 해시도 미고정.
- GPU 없음: CUDA/Vulkan 팩, 실모델 VRAM 실측, `--list-devices` GPU 출력은 사용자 PC
  (`docs/user-pc-checklist.md` P1 항목).
- Linux 빌드는 CI 전용(배포 범위 외). Windows 실창 E2E(tauri-driver)는 사용자 PC.
- 모의 백엔드의 프리라우터는 Rust 구현의 부분 집합(브라우저 데모 용도).
- 라우터의 `memory` 실측은 CPU 전용 빌드에서는 비어 있음(GPU 장치가 있을 때만 채워짐).
- WebView2 `PrintToPdf`(HWPX/DOCX/HWP 변환본)는 이 환경에서 실행 불가 — Linux WebKitGTK 경로로 동일 파이프라인을
  검증했고 Windows 경로는 컴파일 검사만 거침(`docs/user-pc-checklist.md` P2 항목).
- 이 환경에는 한글 글꼴이 제한적이라 변환본 렌더 품질(글꼴 대체)은 사용자 PC에서 확인.
