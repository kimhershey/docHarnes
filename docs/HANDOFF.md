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
| P0 스캐폴드 | **완료 (이 브랜치)** | 아래 "P0 산출물" 참조 |
| P1 엔진·모델 | 다음 | llama-server 사이드카, GGUF 검증, 드롭인 폴더 감시, 모델 다운로드 |
| P2 워크스페이스·문서 | 대기 | SQLite 마이그레이션, 가져오기, HWP/HWPX/DOCX 변환, pdfium 렌더, 뷰어 |
| P3 지식 베이스 | 대기 | 청킹, lindera FTS5 + trigram, bge-m3 + sqlite-vec, 근거 기반 Q&A |
| P4 파이프라인 | 대기 | 분류·OCR·KIE, 양식 등록, 검증기, 검토 큐 |
| P5 자연어·작업 | 대기(핵심 op만 1차) | 플래너 PlanSchema, 확인 카드, Job 러너, 정리/되돌리기, 내보내기 |

## P0 산출물

- Cargo 워크스페이스: `crates/doculens-core` (순수 Rust, 31 단위 테스트) +
  `src-tauri` (Tauri 2 셸, 23 commands).
  - `settings`(settings.json, 손상 복구·정규화), `workspace`(.doculens 초기화,
    잠금·stale 복구, inspect), `hw`(sysinfo + NVML 프로브, 티어 표, 백엔드 선택),
    `memory`(하이브리드 KV 공식, 로드 전 예산), `registry`(내장 registry.json,
    적응형 양자화, 설치 상태, 필요 파일 목록), `retention`(보존/파기 정책),
    `prerouter`(슬래시/문장 규칙 + 상대 날짜 해석), `paths`.
- 프런트엔드: React 19 + Vite 8 + Tailwind v4 + Radix/shadcn 패턴 + cmdk +
  i18next(ko 기본/en) + zustand + TanStack Query.
  - 테마 4종(`html[data-theme]`, 시스템 추적, forced-colors, 깜빡임 방지 인라인
    스크립트, 창 제목표시줄 동기화), UI 배율, 단축키(Ctrl+K, Ctrl+Shift+T …).
  - 화면: 온보딩 3단계, 홈(자연어 명령 바), 어시스턴트(프리라우터 실행/플래너
    대기 카드), 모델 관리자(하드웨어·티어·레지스트리·필요 파일), 설정 7탭
    (일반/테마/워크스페이스/처리/프라이버시·보존/단축키/정보), 나머지 화면은
    단계 배지가 있는 플레이스홀더.
  - 브라우저 모의 백엔드(`src/lib/mock-backend.ts`)로 Tauri 없이 전체 UI 실행 가능.
- 테스트: `cargo test`, `vitest`(테마 해석, 토큰 대비 검사기, i18n 키 패리티,
  단축키, 모의 백엔드), Playwright 스모크 7건(온보딩→홈, 테마 4종 전환·유지,
  언어 전환, 전 라우트·팔레트, 자연어 명령, 설정·보존 저장, 모델 관리자).
- 라이선스 게이트: `deny.toml`(cargo-deny), `scripts/check-licenses.mjs`(npm),
  `THIRD-PARTY-NOTICES.md`, `docs/LICENSE-POLICY.md`.
- CI: `.github/workflows/ci.yml`(rust / web / license / windows-check),
  `release-windows.yml`(수동 NSIS 빌드).

## P1 착수 메모

1. `crates/doculens-core/src/engine/` 신설: `llama-server` 프로세스 매니저
   (라우터 모드, `--models-preset`, 헬스/`/models` 상태 대기, 재기동 백오프,
   Windows Job Object), `presets.ini` 생성기(§3.3.2, 절대경로), `/models/load|unload`,
   `/models/sse` 구독 → Tauri 이벤트.
2. GGUF 헤더 파서(`general.architecture`, chat_template, `clip.projector_type`,
   `split.count`, 양자화 타입) + blake3/sha256 해시 → 드롭인 폴더 감시(notify).
3. `registry::install_state` 를 헤더 검증 기반으로 교체, `verified` 고정.
4. 엔진 바이너리 배치: `<AppData>/DocuLens/engines/<version>/{cpu|vulkan|cu121|cu129}`,
   `engines/README.md` 의 해시 고정 스크립트.
5. 이 환경에서는 llama.cpp CPU 빌드 + 2층 랜덤 초기화 Qwen3.5 GGUF로 생명주기
   통합 테스트(§4.2). 실모델은 사용자 PC(부록 B).
6. UI: 모델 관리자 다운로드/가져오기 활성화, 상태바 엔진 상태 실데이터,
   Jobs 화면에 모델 로드 진행률.

## 알려진 제약

- HF 차단으로 `models/registry.json` 의 크기·SHA-256 미고정(`verified: false`).
- Linux 빌드는 CI 전용(배포 범위 외). Windows 실창 E2E(tauri-driver)는 사용자 PC.
- 모의 백엔드의 프리라우터는 Rust 구현의 부분 집합(브라우저 데모 용도).
